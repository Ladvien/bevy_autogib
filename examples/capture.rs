//! **The fracture, rendered headless, one PNG per frame — and colour-coded by what the audit says.**
//!
//! `explode` is the demo you watch; this is the one you *measure*. It renders the same subject and the
//! same burst, but with no window, no winit and no wall clock: the update loop is pumped by hand and
//! the integrator is driven by a fixed `DT`, so frame 47 of one run is frame 47 of the next. That is
//! what makes the output diffable — a GIF from before a ticket and a GIF from after differ only where
//! the geometry differs.
//!
//! **The skin colour is the finding, not decoration.** Each fragment is audited with
//! [`bevy_autogib::audit_proxy`] and tinted by the verdict:
//!
//! | colour | meaning |
//! |---|---|
//! | green | watertight **and** manifold — a closed solid, the thing we want |
//! | amber | watertight but **not** manifold — closed, yet not a surface a solver can trust |
//! | red | **open cut edges** — a cap that never closed, so this piece is not a solid at all |
//!
//! Cut faces keep the dark interior material regardless, because that contrast is what makes a break
//! read as a break at all. See `explode.rs`.
//!
//! The verdict is taken on the **proxy cell** — the artefact that is a solid — never on the render
//! skin, which is a surface subset and open by construction. Under Tier A every fragment should come
//! back green: a plane through a convex cell yields two convex cells, with no input for which that can
//! fail. Magenta here means the cell clipper is wrong, not that the subject was awkward.
//!
//! Frames land in `--out <dir>` (default `frames/`) as `frame0000.png`. Turn them into a GIF with:
//!
//! ```text
//! ffmpeg -y -framerate 30 -i frames/frame%04d.png \
//!        -vf "scale=640:-1:flags=lanczos,split[a][b];[a]palettegen[p];[b][p]paletteuse" out.gif
//! ```
//!
//! Run: `cargo run --release --example capture -- --out frames`

use bevy::{
    app::SubApps,
    asset::RenderAssetUsages,
    camera::RenderTarget,
    prelude::*,
    render::{
        RenderPlugin,
        render_resource::{Extent3d, PollType, TextureDimension, TextureFormat, TextureUsages},
        renderer::RenderDevice,
        view::screenshot::{Screenshot, save_to_disk},
    },
    window::ExitCondition,
    winit::WinitPlugin,
};
use bevy_autogib::{CutSettings, FragmentGeometry, ProxyCell, audit_proxy, fracture_mesh, hash_f32};

/// Capture size. Small enough that 100 PNGs and the GIF built from them stay a reasonable thing to
/// commit; large enough to see a cut face.
const WIDTH: u32 = 720;
const HEIGHT: u32 = 540;

/// Frames held on the intact subject before the break, so the swap has something to swap *from*.
const INTACT_FRAMES: u32 = 14;
/// Frames of debris after it.
const BROKEN_FRAMES: u32 = 86;

/// **Fixed timestep — the reason this example exists alongside `explode`.** `explode` integrates
/// against `Time`, so its trajectories depend on how fast the machine rendered. Here dt is a constant,
/// which makes the whole animation a pure function of the seed.
const DT: f32 = 1.0 / 30.0 * 0.4; // 30 fps, played back at explode's 0.4× so the burst is legible.

// Burst and settle constants, matching `explode` so the two show the same motion.
const GRAVITY: f32 = 18.0;
const RESTITUTION: f32 = 0.35;
const GROUND_DRAG: f32 = 4.0;
const TARGET: usize = 18;
const MIN_FRACTION: f32 = 0.12;
/// How many cuts deep the hierarchy may go — slack enough here that `TARGET` is what binds.
const MAX_DEPTH: u16 = 64;

/// The geometry dials for this example's bake. `plane_jitter` and `size_spread` are what keep the
/// pieces from all coming out the same size — at `0.0` each cut halves its piece through the centre
/// and the result reads as uniform shards rather than debris.
fn cut(seed: u32) -> CutSettings {
    CutSettings { max_depth: MAX_DEPTH, ..CutSettings::new(TARGET, MIN_FRACTION, seed) }
}

const SEED: u32 = 0x00C0_FFEE;
const ORIGIN: Vec3 = Vec3::new(0.0, 1.0, 0.0);

/// The example's own physics, exactly as `explode` defines it. Not this crate's business.
#[derive(Component)]
struct Chunk {
    velocity: Vec3,
    spin: Vec3,
    drop_to_rest: f32,
}

/// The unbroken subject, before the swap.
#[derive(Component)]
struct Intact;

/// What the audit said about one fragment, reduced to the three cases worth a colour.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// Closed and manifold — a solid.
    Solid,
    /// Closed, but not a manifold surface.
    ClosedNonManifold,
    /// Has boundary edges: a cap that never closed.
    Open,
}

impl Verdict {
    /// Reduce a fragment to its verdict. An unauditable fragment counts as [`Verdict::Open`] rather
    /// than being hidden — a piece we cannot measure is not a piece we get to call clean.
    ///
    /// **This asks [`audit_proxy`], not `audit_render`, and the difference is the whole point.** A
    /// fragment is two artefacts: a closed convex *cell*, and a *subset of the subject's own surface*
    /// which is open because a surface subset is open. Colouring by the render mesh's watertightness
    /// paints almost everything magenta and says nothing — it measures the wrong artefact.
    fn of(frag: &FragmentGeometry) -> Self {
        match audit_proxy(frag) {
            Ok(a) if a.is_closed() && a.is_manifold() => Verdict::Solid,
            Ok(a) if a.is_closed() => Verdict::ClosedNonManifold,
            _ => Verdict::Open,
        }
    }

    /// **Why "open" is magenta and not red.** The cut faces are dark red — that is the crate's
    /// established visual language and `explode.rs` argues for it. A red *verdict* on top of that read
    /// as more cut face in the first capture, which is exactly the confusion this colouring exists to
    /// prevent. Magenta appears nowhere else in the scene.
    fn color(self) -> Color {
        match self {
            Verdict::Solid => Color::srgb(0.24, 0.62, 0.36),
            Verdict::ClosedNonManifold => Color::srgb(0.90, 0.65, 0.12),
            Verdict::Open => Color::srgb(0.85, 0.18, 0.72),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Verdict::Solid => "watertight + manifold",
            Verdict::ClosedNonManifold => "watertight, non-manifold",
            Verdict::Open => "open cut edges",
        }
    }
}

fn main() {
    let out = out_dir();
    if let Err(e) = std::fs::create_dir_all(&out) {
        error!("capture: cannot create {out}: {e}");
        return;
    }

    let mut app = headless_app();
    let target = new_render_target(&mut app);
    spawn_camera(&mut app, target.clone());

    let total = INTACT_FRAMES + BROKEN_FRAMES;
    for frame in 0..total {
        if frame == INTACT_FRAMES {
            break_it(&mut app);
        }
        screenshot(&mut app, &target, &out, frame);
        update(&mut app);
    }
    // Screenshots read back on the GPU a frame or two behind, so pump past the last one.
    for _ in 0..4 {
        update(&mut app);
    }
    info!("capture: wrote {total} frames to {out}");
}

/// `--out <dir>`, defaulting to `frames`. Deliberately hand-parsed: a CLI crate has no business in
/// this repo's dependency graph, not even as a dev-dependency, when the whole need is one flag.
fn out_dir() -> String {
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--out" {
            match args.next() {
                Some(dir) => return dir,
                // Malformed input is warned and skipped, never silently substituted — the same rule
                // the crate applies to a mesh with no positions.
                None => {
                    warn!("capture: --out given with no directory; falling back to the default");
                    break;
                }
            }
        }
    }
    "frames".to_string()
}

/// `DefaultPlugins` with no window and no winit, so nothing is mapped and we own the update loop.
fn headless_app() -> SubApps {
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: ExitCondition::DontExit,
                ..default()
            })
            // Every shader must be ready before the first frame, or frame 0 renders empty.
            .set(RenderPlugin { synchronous_pipeline_compilation: true, ..default() })
            .disable::<WinitPlugin>(),
    )
    .add_systems(Startup, setup_scene);

    // `run()` is never called, so the two things the runner would have done must be done here.
    app.finish();
    app.cleanup();
    std::mem::take(app.sub_apps_mut())
}

fn new_render_target(app: &mut SubApps) -> Handle<Image> {
    let mut target = Image::new_uninit(
        Extent3d { width: WIDTH, height: HEIGHT, depth_or_array_layers: 1 },
        TextureDimension::D2,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    target.texture_descriptor.usage |= TextureUsages::RENDER_ATTACHMENT;
    app.main.world_mut().resource_mut::<Assets<Image>>().add(target)
}

fn spawn_camera(app: &mut SubApps, target: Handle<Image>) {
    app.main.world_mut().spawn((
        Camera3d::default(),
        RenderTarget::from(target),
        // The same framing as `explode`, so the two are comparable side by side.
        Transform::from_xyz(3.4, 2.2, 4.6).looking_at(Vec3::new(0.0, 0.9, 0.0), Vec3::Y),
    ));
}

/// One world update, then block until the GPU has actually finished the frame — without the wait, a
/// screenshot can read back a half-drawn target.
fn update(app: &mut SubApps) {
    app.update();
    let device = app.main.world().resource::<RenderDevice>().wgpu_device().clone();
    if let Err(e) = device.poll(PollType::Wait { submission_index: None, timeout: None }) {
        warn!("capture: device poll failed, frame may be torn: {e:?}");
    }
}

fn screenshot(app: &mut SubApps, target: &Handle<Image>, out: &str, frame: u32) {
    let path = format!("{out}/frame{frame:04}.png");
    app.main
        .world_mut()
        .spawn(Screenshot::image(target.clone()))
        .observe(save_to_disk(path));
}

/// The two shells the subject is made of — a torso and a head, the honest non-manifold case.
fn subject() -> [(Mesh, Mat4); 2] {
    [
        (Mesh::from(Cuboid::new(0.7, 1.1, 0.4)), Mat4::IDENTITY),
        (Mesh::from(Cuboid::new(0.4, 0.4, 0.4)), Mat4::from_translation(Vec3::new(0.0, 0.74, 0.0))),
    ]
}

fn setup_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        DirectionalLight { illuminance: 9_000.0, shadow_maps_enabled: true, ..default() },
        Transform::from_xyz(4.0, 8.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        Mesh3d(meshes.add(Mesh::from(Plane3d::default().mesh().size(14.0, 14.0)))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.16, 0.16, 0.18),
            perceptual_roughness: 0.95,
            ..default()
        })),
    ));
    // The intact subject wears the neutral skin — it has no verdict, because it has not been cut.
    let skin = materials.add(StandardMaterial {
        base_color: Color::srgb(0.30, 0.42, 0.52),
        perceptual_roughness: 0.85,
        ..default()
    });
    for (mesh, xform) in subject() {
        commands.spawn((
            Intact,
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(skin.clone()),
            Transform::from_matrix(Mat4::from_translation(ORIGIN) * xform),
        ));
    }
}

/// Despawn the intact subject, fracture it, and spawn every piece tinted by its audit verdict.
fn break_it(app: &mut SubApps) {
    let world = app.main.world_mut();

    let intact: Vec<Entity> = world.query_filtered::<Entity, With<Intact>>().iter(world).collect();
    for e in intact {
        world.entity_mut(e).despawn();
    }

    let owned = subject();
    let parts: Vec<(&Mesh, Mat4)> = owned.iter().map(|(m, x)| (m, *x)).collect();
    let proxy = vec![
        ProxyCell::from_box(Vec3::ZERO, Vec3::new(0.35, 0.55, 0.2)),
        ProxyCell::from_box(Vec3::new(0.0, 0.74, 0.0), Vec3::splat(0.2)),
    ];
    let pieces = fracture_mesh(&parts, &proxy, &cut(SEED)).into_leaves();

    // Audit first, so the tally can be logged next to the frames it describes.
    let verdicts: Vec<Verdict> = pieces.iter().map(Verdict::of).collect();
    for v in [Verdict::Solid, Verdict::ClosedNonManifold, Verdict::Open] {
        let n = verdicts.iter().filter(|&&x| x == v).count();
        info!("capture: {n:>2} of {} fragments — {}", pieces.len(), v.label());
    }

    let interior = world.resource_mut::<Assets<StandardMaterial>>().add(StandardMaterial {
        base_color: Color::srgb(0.30, 0.05, 0.05),
        perceptual_roughness: 0.55,
        ..default()
    });
    // One material per verdict, made once rather than per fragment.
    let skins: Vec<(Verdict, Handle<StandardMaterial>)> =
        [Verdict::Solid, Verdict::ClosedNonManifold, Verdict::Open]
            .into_iter()
            .map(|v| {
                let h = world.resource_mut::<Assets<StandardMaterial>>().add(StandardMaterial {
                    base_color: v.color(),
                    perceptual_roughness: 0.85,
                    ..default()
                });
                (v, h)
            })
            .collect();

    for (i, (piece, verdict)) in pieces.into_iter().zip(verdicts).enumerate() {
        let (velocity, spin) = launch(i, piece.center_local);
        // The collider a real game builds: `Collider::convex_hull(piece.cell.points())`.
        let lowest = piece.cell.points().iter().map(|p| p.y).fold(f32::INFINITY, f32::min);
        let drop_to_rest = (piece.cell.center().y - lowest).max(0.0);
        let skin = skins.iter().find(|(v, _)| *v == verdict).map(|(_, h)| h.clone());

        let outer = piece.outer.map(|m| world.resource_mut::<Assets<Mesh>>().add(m));
        let cap = piece.cap.map(|m| world.resource_mut::<Assets<Mesh>>().add(m));

        let chunk = world
            .spawn((
                Chunk { velocity, spin, drop_to_rest },
                Transform::from_translation(ORIGIN + piece.center_local),
                Visibility::default(),
            ))
            .id();
        world.entity_mut(chunk).with_children(|parent| {
            if let (Some(mesh), Some(skin)) = (outer, skin) {
                parent.spawn((Mesh3d(mesh), MeshMaterial3d(skin)));
            }
            if let Some(mesh) = cap {
                parent.spawn((Mesh3d(mesh), MeshMaterial3d(interior.clone())));
            }
        });
    }

    // The integrator is added here rather than at startup so the intact frames are perfectly still.
    app.main.add_systems(Update, integrate);
}

/// Deterministic per-fragment launch, from the crate's own frozen hash — no RNG dependency.
fn launch(i: usize, center: Vec3) -> (Vec3, Vec3) {
    let base = SEED.wrapping_mul(2_246_822_519).wrapping_add((i as u32).wrapping_mul(2_654_435_761));
    let (h1, h2, h3, h4) = (
        hash_f32(base.wrapping_add(1)),
        hash_f32(base.wrapping_add(2)),
        hash_f32(base.wrapping_add(3)),
        hash_f32(base.wrapping_add(4)),
    );
    let angle = h1 * std::f32::consts::TAU;
    let jitter = Vec3::new(angle.cos(), 0.0, angle.sin()) * 0.5;
    let dir = (center.normalize_or_zero() + jitter + Vec3::Y * (0.6 + 0.8 * h3)).normalize_or_zero();
    let spin = Vec3::new(h1 - 0.5, h2 - 0.5, h4 - 0.5).normalize_or_zero() * (8.0 + 8.0 * h2);
    (dir * (3.2 + 2.4 * h4), spin)
}

/// Gravity, a ground bounce and tumbling — on a fixed `DT`, so the run is reproducible.
fn integrate(mut chunks: Query<(&mut Chunk, &mut Transform)>) {
    for (mut chunk, mut transform) in &mut chunks {
        chunk.velocity.y -= GRAVITY * DT;
        transform.translation += chunk.velocity * DT;
        transform.rotate_local_x(chunk.spin.x * DT);
        transform.rotate_local_y(chunk.spin.y * DT);
        transform.rotate_local_z(chunk.spin.z * DT);

        let floor = chunk.drop_to_rest;
        if transform.translation.y < floor {
            transform.translation.y = floor;
            if chunk.velocity.y < 0.0 {
                chunk.velocity.y = -chunk.velocity.y * RESTITUTION;
                let damp = (1.0 - GROUND_DRAG * DT).max(0.0);
                chunk.velocity.x *= damp;
                chunk.velocity.z *= damp;
                chunk.spin *= damp;
                if chunk.velocity.y.abs() < 0.4 {
                    chunk.velocity.y = 0.0;
                }
            }
        }
    }
}

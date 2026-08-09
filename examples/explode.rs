//! **Prefracture and swap, on screen — the thing the crate is actually for.**
//!
//! The subject stands there intact. A moment later it is replaced by its baked fragments, which fly.
//! That swap *is* the technique: the fracture was computed before anything happened, and the "break"
//! is one despawn and a spawn. Watch it loop.
//!
//! Press **Space** to break it early, or to break it again with a new seed.
//!
//! There is no physics engine here, and that is the point of the example rather than a shortcut. This
//! crate hands you a mesh, a local centre and a half-extent per piece and stops; everything below
//! `integrate` — gravity, the bounce, the spin — is thirty lines the example owns. Swap in Avian,
//! Rapier or your own solver and nothing in `bevy_autogib` changes.
//!
//! Note the two materials. Each fragment comes back as two meshes — the subject's own outer skin and
//! the cut faces alone — and that contrast is the entire visual read. Give the cap the same material
//! as the skin and the result immediately stops looking broken and starts looking disassembled.
//!
//! **Some breaks log `dropping unclosed cut boundary` warnings, and they mean working, not broken.**
//! The subject is a torso and a head — two closed shells that meet — so the merged solid is not a
//! manifold at the seam, and a plane through that region can produce a boundary chain with no way to
//! close. The slicer drops such a chain instead of fanning garbage over it, which leaves that face
//! open. A real character (body + head + weapon) is non-manifold in exactly the same way; this is the
//! honest case, not a rigged one.
//!
//! It is **per-seed, not per-break** — measured over one run of this loop: 4 dropped loops on break
//! #0, none at all on #1 or #2. Whether a cut lands on the seam is the whole difference, which is
//! also why a single observation of this example proves very little about the slicer either way.
//!
//! This is the only example here that needs a GPU.
//!
//! Run: `cargo run -p bevy_autogib --example explode`

use bevy::prelude::*;
use bevy_autogib::{fracture_mesh, hash_f32};

/// Target fragment count for one break.
const TARGET: usize = 18;
/// Stop cutting a piece below this fraction of the whole solid's extent.
const MIN_FRACTION: f32 = 0.12;
/// Downward acceleration, m/s². Exaggerated — gibs read better when they fall fast.
const GRAVITY: f32 = 18.0;
/// How much speed survives a bounce off the ground plane.
const RESTITUTION: f32 = 0.35;
/// Horizontal drag per second while sliding, so pieces settle instead of skating forever.
const GROUND_DRAG: f32 = 4.0;

/// **Playback rate, and the reason this example has one.** The launch above is tuned for a game, where
/// a gib set is meant to read in a fraction of a second. At 1.0 the whole burst is over before a
/// freshly-mapped window has finished its first few frames, so the first thing anyone saw was a
/// settled pile — the interesting part had already happened offscreen. Slowing playback rather than
/// weakening gravity keeps the trajectories exactly the ones a game would get.
const PLAYBACK_SPEED: f32 = 0.4;

/// Seconds the intact subject is shown before it breaks. Long enough that the window exists, the
/// render pipeline has warmed, and a viewer has seen what is about to be destroyed — without which
/// the swap has nothing to swap *from*.
const INTACT_SECS: f32 = 2.5;
/// Seconds the debris is left to fly and settle before the subject is restored and the loop repeats.
const BROKEN_SECS: f32 = 7.0;

/// The example's own physics. In a real game this is a rigid body from whichever solver you use —
/// `bevy_autogib` never names one.
#[derive(Component)]
struct Chunk {
    velocity: Vec3,
    spin: Vec3,
    /// Half-height, so the piece rests on the ground rather than sinking to its centre.
    half_y: f32,
}

/// The unbroken subject, before the swap.
#[derive(Component)]
struct Intact;

/// Shared materials, made once. The subject's own surface, and the raw interior the cut exposed.
#[derive(Resource)]
struct DemoMaterials {
    skin: Handle<StandardMaterial>,
    interior: Handle<StandardMaterial>,
}

#[derive(PartialEq, Clone, Copy)]
enum Phase {
    Intact,
    Broken,
}

/// Drives the intact → broken → intact loop, so an observer who looked away still catches a break.
#[derive(Resource)]
struct Cycle {
    timer: Timer,
    phase: Phase,
    /// Bumped every break, so each one slices along different planes.
    breaks: u32,
}

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "bevy_autogib — prefracture and swap (Space to break)".into(),
                // 0.19 takes physical pixels as `u32` — there is no `(f32, f32)` conversion.
                resolution: (900u32, 640u32).into(),
                ..default()
            }),
            ..default()
        }))
        .insert_resource(Cycle {
            timer: Timer::from_seconds(INTACT_SECS, TimerMode::Once),
            phase: Phase::Intact,
            breaks: 0,
        })
        .add_systems(Startup, setup_scene)
        .add_systems(Update, (drive_cycle, integrate))
        .run();
}

/// The two shells the subject is made of, each with its transform relative to the subject root —
/// the same `(&Mesh, Mat4)` pairs the ECS bake assembles by walking a scene's children.
fn subject() -> [(Mesh, Mat4); 2] {
    [
        (Mesh::from(Cuboid::new(0.7, 1.1, 0.4)), Mat4::IDENTITY),
        (
            Mesh::from(Cuboid::new(0.4, 0.4, 0.4)),
            Mat4::from_translation(Vec3::new(0.0, 0.74, 0.0)),
        ),
    ]
}

/// Where the subject stands: feet on the floor.
const ORIGIN: Vec3 = Vec3::new(0.0, 1.0, 0.0);

fn setup_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(3.4, 2.2, 4.6).looking_at(Vec3::new(0.0, 0.9, 0.0), Vec3::Y),
    ));
    commands.spawn((
        // 0.19 spells this `shadow_maps_enabled`; it was `shadows_enabled` in earlier releases.
        DirectionalLight { illuminance: 9_000.0, shadow_maps_enabled: true, ..default() },
        Transform::from_xyz(4.0, 8.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    // A floor to land on — purely so the chunks have somewhere to settle.
    commands.spawn((
        Mesh3d(meshes.add(Mesh::from(Plane3d::default().mesh().size(14.0, 14.0)))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.16, 0.16, 0.18),
            perceptual_roughness: 0.95,
            ..default()
        })),
    ));

    let mats = DemoMaterials {
        skin: materials.add(StandardMaterial {
            base_color: Color::srgb(0.30, 0.42, 0.52),
            perceptual_roughness: 0.85,
            ..default()
        }),
        interior: materials.add(StandardMaterial {
            base_color: Color::srgb(0.52, 0.09, 0.08),
            perceptual_roughness: 0.55,
            ..default()
        }),
    };
    spawn_intact(&mut commands, &mut meshes, &mats);
    commands.insert_resource(mats);
}

/// The subject before anything happens to it — one entity per shell, the skin material on both.
fn spawn_intact(commands: &mut Commands, meshes: &mut Assets<Mesh>, mats: &DemoMaterials) {
    for (mesh, xform) in subject() {
        commands.spawn((
            Intact,
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(mats.skin.clone()),
            Transform::from_matrix(Mat4::from_translation(ORIGIN) * xform),
        ));
    }
}

/// Advance the loop, and honour Space. Breaking early just runs the same swap the timer would.
fn drive_cycle(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mats: Res<DemoMaterials>,
    mut cycle: ResMut<Cycle>,
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    intact: Query<Entity, With<Intact>>,
    chunks: Query<Entity, With<Chunk>>,
) {
    cycle.timer.tick(time.delta());
    let forced = keys.just_pressed(KeyCode::Space);
    if !forced && !cycle.timer.just_finished() {
        return;
    }

    // Space during the debris phase means "again, now" — so both paths end in a fresh break.
    let restore = !forced && cycle.phase == Phase::Broken;
    for e in &intact {
        commands.entity(e).despawn();
    }
    for e in &chunks {
        commands.entity(e).despawn();
    }

    if restore {
        info!("restoring the intact subject");
        spawn_intact(&mut commands, &mut meshes, &mats);
        cycle.phase = Phase::Intact;
        cycle.timer = Timer::from_seconds(INTACT_SECS, TimerMode::Once);
        return;
    }

    info!("break #{} — fracturing", cycle.breaks);
    break_it(&mut commands, &mut meshes, &mats, cycle.breaks);
    cycle.breaks = cycle.breaks.wrapping_add(1);
    cycle.phase = Phase::Broken;
    cycle.timer = Timer::from_seconds(BROKEN_SECS, TimerMode::Once);
}

/// Fracture the subject and spawn every piece. This is the function a game's death handler replaces:
/// same loop, but each chunk gets a real rigid body and collider instead of a [`Chunk`].
fn break_it(commands: &mut Commands, meshes: &mut Assets<Mesh>, mats: &DemoMaterials, nth: u32) {
    let owned = subject();
    let parts: Vec<(&Mesh, Mat4)> = owned.iter().map(|(m, x)| (m, *x)).collect();

    let seed = 0x00C0_FFEE_u32.wrapping_add(nth.wrapping_mul(2_654_435_761));
    let extent = 0.74;
    let pieces = fracture_mesh(&parts, TARGET, extent * MIN_FRACTION, seed, None);

    for (i, piece) in pieces.into_iter().enumerate() {
        // Deterministic per-fragment variation from the crate's own frozen hash — no rand dependency.
        let base = seed.wrapping_mul(2_246_822_519).wrapping_add((i as u32).wrapping_mul(2_654_435_761));
        let (h1, h2, h3, h4) = (
            hash_f32(base.wrapping_add(1)),
            hash_f32(base.wrapping_add(2)),
            hash_f32(base.wrapping_add(3)),
            hash_f32(base.wrapping_add(4)),
        );

        // Burst outward from where the piece actually sat, with an upward bias.
        let outward = piece.center_local.normalize_or_zero();
        let angle = h1 * std::f32::consts::TAU;
        let jitter = Vec3::new(angle.cos(), 0.0, angle.sin()) * 0.5;
        let dir = (outward + jitter + Vec3::Y * (0.6 + 0.8 * h3)).normalize_or_zero();
        let velocity = dir * (3.2 + 2.4 * h4);
        let spin = Vec3::new(h1 - 0.5, h2 - 0.5, h4 - 0.5).normalize_or_zero() * (8.0 + 8.0 * h2);

        let chunk = commands
            .spawn((
                Chunk { velocity, spin, half_y: piece.half_extents.y },
                Transform::from_translation(ORIGIN + piece.center_local),
                Visibility::default(),
            ))
            .id();

        // Both meshes are already recentred on the fragment's own centre, so the chunk spins about
        // itself rather than orbiting the origin.
        commands.entity(chunk).with_children(|parent| {
            if let Some(outer) = piece.outer {
                parent.spawn((Mesh3d(meshes.add(outer)), MeshMaterial3d(mats.skin.clone())));
            }
            if let Some(cap) = piece.cap {
                parent.spawn((Mesh3d(meshes.add(cap)), MeshMaterial3d(mats.interior.clone())));
            }
        });
    }
}

/// The example's whole solver: gravity, a ground bounce, and tumbling. Replace with yours.
fn integrate(time: Res<Time>, mut chunks: Query<(&mut Chunk, &mut Transform)>) {
    let dt = time.delta_secs() * PLAYBACK_SPEED;
    if dt <= 0.0 {
        return;
    }
    for (mut chunk, mut transform) in &mut chunks {
        chunk.velocity.y -= GRAVITY * dt;
        transform.translation += chunk.velocity * dt;
        transform.rotate_local_x(chunk.spin.x * dt);
        transform.rotate_local_y(chunk.spin.y * dt);
        transform.rotate_local_z(chunk.spin.z * dt);

        // Rest on the floor rather than through it.
        let floor = chunk.half_y;
        if transform.translation.y < floor {
            transform.translation.y = floor;
            if chunk.velocity.y < 0.0 {
                chunk.velocity.y = -chunk.velocity.y * RESTITUTION;
                // Bleed horizontal speed and spin on contact so the pile settles.
                let damp = (1.0 - GROUND_DRAG * dt).max(0.0);
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

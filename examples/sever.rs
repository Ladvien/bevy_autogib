//! **It comes apart where you hit it.**
//!
//! `explode.rs` shows the other half of this crate: a subject stands there, then it is all of its
//! fragments at once. That is the right shape for a death, and the wrong shape for everything else —
//! it is the same burst however the thing died, which is what makes a demo read as *froze, then
//! shattered*.
//!
//! This example keeps the subject standing and takes pieces off it. One bake, cached once at
//! startup; every blow is a region query against it plus a threshold, and whatever stops being
//! connected falls off. Hit it again and it comes apart further.
//!
//! ```text
//!   arrows / WASD   move the aim marker
//!   1               a projectile   — nearest fragment, then outward along the bonds
//!   2               a slash        — falloff from the segment a blade travelled
//!   3               a swept blade  — every bond the swing passed through, no falloff
//!   4               a blast        — falloff from a point in open space
//!   5               a pull         — weighted by how squarely each face meets it
//!   G               granularity — cycle which frontier of the bake is standing
//!   T               soften — cycle how hard the drawn fragments are rounded (re-bakes)
//!   R               reset
//! ```
//!
//! **Nothing here is in the crate.** `bevy_autogib` hands out a reach — a severity per bond — and
//! `common::body` picks the threshold at which one gives way, decides which island is still "the
//! body", and throws the rest. A game scales that severity by material and by how much damage the
//! blow carried; none of those are facts the crate has.
//!
//! The subject and those rules live in `common::body` because `capture_sever.rs` drives them too —
//! so the GIF in the README is this example on rails, not a re-implementation that could drift.
//!
//! Needs a GPU.
//!
//! Run: `cargo run -p bevy_autogib --example sever`

use bevy::prelude::*;

mod common;
use common::body::{self, Blow, BodyMaterials, Chunk, GRANULARITIES, ORIGIN, SOFTENINGS};
use common::light_and_floor;

/// Where the next blow lands, in subject-local space.
#[derive(Resource)]
struct Aim(Vec3);

/// Marks the little sphere that shows [`Aim`].
#[derive(Component)]
struct AimMarker;

/// Which frontier of the hierarchy is currently standing — the granularity dial, on a key.
#[derive(Resource)]
struct Granularity(usize);

/// Marks the line that reports what the last blow did.
#[derive(Component)]
struct HudStatus;

/// What to say about the last blow.
///
/// **This exists because the demo silently dead-ends without it.** Once everything reachable from
/// one aim point has been severed, every further keypress is a legitimate no-op — the blow lands, it
/// reaches bonds, and nothing is left for it to break. On screen that is indistinguishable from a
/// dropped keypress, and a play session ended with thirty presses that appeared to do nothing.
#[derive(Resource)]
struct Status(String);

impl Default for Status {
    fn default() -> Self {
        Status("hit it: 1 projectile  2 slash  3 blade  4 blast  5 pull".into())
    }
}

/// How hard the drawn fragments are rounded — index into [`SOFTENINGS`].
///
/// **On a key because it is worth seeing back to back.** At `0.0` the pieces keep the hard dihedral
/// edges a plane cut leaves, which is the visual language of ice and cleaved stone however good the
/// fracture underneath is. Press `T` and the same cuts read as torn instead.
#[derive(Resource)]
struct Soften(usize);

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "bevy_autogib — sever (1-5 to hit, arrows to aim, G granularity, R reset)"
                    .into(),
                resolution: (960u32, 680u32).into(),
                ..default()
            }),
            ..default()
        }))
        .insert_resource(Aim(Vec3::new(0.0, 0.25, 0.0)))
        .insert_resource(Granularity(GRANULARITIES.len() - 1))
        .insert_resource(Soften(2))
        .init_resource::<Status>()
        .add_systems(Startup, setup)
        .add_systems(Update, (aim_marker, strike, integrate, hud))
        .run();
}

fn setup(world: &mut World) {
    let camera = Transform::from_xyz(2.25, 1.35, 2.95).looking_at(Vec3::new(0.0, 0.76, 0.0), Vec3::Y);
    world.spawn((Camera3d::default(), camera));
    light_and_floor(world);

    let soften = SOFTENINGS[world.resource::<Soften>().0];
    let baked = body::Baked::bake(world, soften);
    let materials = BodyMaterials::new(world);
    let granularity = world.resource::<Granularity>().0;
    let damage = body::Damage::fresh(&baked, granularity);

    let marker = world.resource_mut::<Assets<Mesh>>().add(Mesh::from(Sphere::new(0.05)));
    world.spawn((
        AimMarker,
        Mesh3d(marker),
        MeshMaterial3d(materials.aim.clone()),
        Transform::from_translation(ORIGIN),
    ));

    world.insert_resource(baked);
    world.insert_resource(materials);
    world.insert_resource(damage);
    body::stand(world, granularity);
    spawn_hud(world);
}

/// **An on-screen legend, because without one the feature set is invisible.**
///
/// Everything this example can do lives on keys, and a window that opens with no text tells you
/// none of it. Watching someone use it: they pressed the number keys — the obvious ones — never
/// found the aim marker, never found `G` or `T`, and concluded it had broken when the subject ran
/// out of pieces to lose.
fn spawn_hud(world: &mut World) {
    world.spawn((
        Text::new(
            "arrows / WASD  aim\n             1 projectile   2 slash   3 blade   4 blast   5 pull\n             G granularity   T soften   R reset",
        ),
        TextFont { font_size: FontSize::Px(15.0), ..default() },
        TextColor(Color::srgba(1.0, 1.0, 1.0, 0.85)),
        Node { position_type: PositionType::Absolute, top: px(12), left: px(14), ..default() },
    ));
    world.spawn((
        HudStatus,
        Text::new(""),
        TextFont { font_size: FontSize::Px(15.0), ..default() },
        TextColor(Color::srgba(1.0, 0.92, 0.55, 0.95)),
        Node { position_type: PositionType::Absolute, bottom: px(14), left: px(14), ..default() },
    ));
}

/// Keep the status line current: what the last blow did, and the state of the two dials.
fn hud(
    status: Res<Status>,
    granularity: Res<Granularity>,
    soften: Res<Soften>,
    standing: Query<(), With<body::Attached>>,
    mut line: Query<&mut Text, With<HudStatus>>,
) {
    let text = format!(
        "{}\n{} of {} standing  |  soften {:.2}  |  granularity {}",
        status.0,
        standing.iter().count(),
        GRANULARITIES[granularity.0],
        SOFTENINGS[soften.0],
        GRANULARITIES[granularity.0],
    );
    for mut t in &mut line {
        if t.0 != text {
            t.0 = text.clone();
        }
    }
}

/// Move the aim marker, and keep the sphere on it.
fn aim_marker(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut aim: ResMut<Aim>,
    mut marker: Query<&mut Transform, With<AimMarker>>,
) {
    let step = 1.1 * time.delta_secs();
    let mut d = Vec3::ZERO;
    for (key, delta) in [
        (KeyCode::ArrowUp, Vec3::Y),
        (KeyCode::KeyW, Vec3::Y),
        (KeyCode::ArrowDown, -Vec3::Y),
        (KeyCode::KeyS, -Vec3::Y),
        (KeyCode::ArrowLeft, -Vec3::X),
        (KeyCode::KeyA, -Vec3::X),
        (KeyCode::ArrowRight, Vec3::X),
        (KeyCode::KeyD, Vec3::X),
    ] {
        if keys.pressed(key) {
            d += delta;
        }
    }
    aim.0 += d * step;
    aim.0 = aim.0.clamp(Vec3::new(-0.8, -0.7, -0.6), Vec3::new(0.8, 1.2, 0.6));
    for mut t in &mut marker {
        t.translation = ORIGIN + aim.0;
    }
}

/// Read the keyboard and hand the chosen region to [`body::strike`].
///
/// An exclusive system, because the blow itself works on `&mut World` — that is the shape the
/// headless recorder can drive too, and sharing it is what keeps the GIF honest.
fn strike(world: &mut World) {
    let pressed = |world: &World, key: KeyCode| world.resource::<ButtonInput<KeyCode>>().just_pressed(key);

    let (reset, coarser, rounder) = (
        pressed(world, KeyCode::KeyR),
        pressed(world, KeyCode::KeyG),
        pressed(world, KeyCode::KeyT),
    );
    if reset || coarser || rounder {
        if coarser {
            let mut g = world.resource_mut::<Granularity>();
            g.0 = (g.0 + 1) % GRANULARITIES.len();
            let now = g.0;
            info!("granularity: standing at {} pieces — same bake, different frontier", GRANULARITIES[now]);
        }
        if rounder {
            let mut t = world.resource_mut::<Soften>();
            t.0 = (t.0 + 1) % SOFTENINGS.len();
            let now = t.0;
            info!(
                "soften: {:.2} — re-baking, because the rounding is built into the drawn mesh rather \
                 than applied by a shader. The colliders come out identical either way.",
                SOFTENINGS[now]
            );
        }
        if reset {
            info!("reset");
        }
        let granularity = world.resource::<Granularity>().0;
        body::clear(world);
        // **Granularity re-reads one bake; softening needs a new one.** A frontier is a query against
        // a hierarchy that already exists, but the rounding is applied when the drawn mesh is built,
        // so changing it means cutting again. Cheap enough at this size to do on a keypress.
        if rounder {
            let soften = SOFTENINGS[world.resource::<Soften>().0];
            let baked = body::Baked::bake(world, soften);
            world.insert_resource(baked);
        }
        let damage = {
            let baked = world.resource::<body::Baked>();
            body::Damage::fresh(baked, granularity)
        };
        world.insert_resource(damage);
        body::stand(world, granularity);
        let (g, t) = (GRANULARITIES[granularity], SOFTENINGS[world.resource::<Soften>().0]);
        world.resource_mut::<Status>().0 = if rounder {
            format!("soften {t:.2} - re-baked; the colliders are identical either way")
        } else if coarser {
            format!("granularity {g} - same bake, read at a different frontier")
        } else {
            "reset".into()
        };
        return;
    }

    // Each key is a region, and the crate has no idea which weapon any of them is.
    let blow = [
        (KeyCode::Digit1, Blow::Projectile),
        (KeyCode::Digit2, Blow::Slash),
        (KeyCode::Digit3, Blow::SweptBlade),
        (KeyCode::Digit4, Blow::Blast),
        (KeyCode::Digit5, Blow::Pull),
    ]
    .into_iter()
    .find(|(key, _)| pressed(world, *key))
    .map(|(_, blow)| blow);

    if let Some(blow) = blow {
        let at = world.resource::<Aim>().0;
        let out = body::strike(world, blow, at);
        // **A blow that severs nothing new is a legitimate outcome, and has to say so.** Everything
        // reachable from this aim point is already gone; the fix is to move the aim or reset, and
        // nothing on screen conveys that unless it is written down.
        let said = match (out.newly, out.off) {
            (0, _) if out.reached == 0 => {
                format!("{}: landed on nothing - the aim is off the body", blow.label())
            }
            (0, _) => format!(
                "{}: nothing left to break here. Move the aim (arrows) or reset (R)",
                blow.label()
            ),
            (n, 0) => format!(
                "{}: severed {n} bond(s), but nothing came loose yet. Hit it again",
                blow.label()
            ),
            (n, off) => format!("{}: severed {n} bond(s), {off} fragment(s) came off", blow.label()),
        };
        world.resource_mut::<Status>().0 = said;
    }
}

/// The example's whole solver — the crate names none.
fn integrate(time: Res<Time>, mut chunks: Query<(&mut Chunk, &mut Transform)>) {
    let dt = time.delta_secs() * 0.55;
    if dt <= 0.0 {
        return;
    }
    for (mut chunk, mut transform) in &mut chunks {
        body::integrate(&mut chunk, &mut transform, dt);
    }
}

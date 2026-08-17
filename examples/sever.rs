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
        .add_systems(Startup, setup)
        .add_systems(Update, (aim_marker, strike, integrate))
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
        body::strike(world, blow, at);
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

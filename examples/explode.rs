//! **The same fracture, on screen — and the launch is the caller's.**
//!
//! Press **Space** to break the shape again with a new seed. Chunks fly, tumble and settle.
//!
//! There is no physics engine here, and that is the point of the example rather than a shortcut. This
//! crate hands you a mesh, a local centre and a half-extent per piece and stops; everything below
//! `integrate` — gravity, the bounce, the spin — is thirty lines the example owns. Swap in Avian,
//! Rapier or your own solver and nothing in `bevy_autogib` changes.
//!
//! Note the two materials. Each fragment comes back as two meshes — the subject's own outer skin and
//! the cut faces alone — and that contrast is the entire visual read. Comment out the `cap` spawn
//! below and the result immediately stops looking broken and starts looking disassembled.
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

/// The example's own physics. In a real game this is a rigid body from whichever solver you use —
/// `bevy_autogib` never names one.
#[derive(Component)]
struct Chunk {
    velocity: Vec3,
    spin: Vec3,
    /// Half-height, so the piece rests on the ground rather than sinking to its centre.
    half_y: f32,
}

/// Bumped every time Space is pressed, so each break gets a different seed.
#[derive(Resource, Default)]
struct BreakCount(u32);

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "bevy_autogib — press Space to break it again".into(),
                // 0.19 takes physical pixels as `u32` — there is no `(f32, f32)` conversion.
                resolution: (900u32, 640u32).into(),
                ..default()
            }),
            ..default()
        }))
        .init_resource::<BreakCount>()
        .add_systems(Startup, (setup_scene, break_it).chain())
        .add_systems(Update, (rebreak_on_space, integrate))
        .run();
}

fn setup_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(3.4, 2.2, 4.6).looking_at(Vec3::new(0.0, 0.7, 0.0), Vec3::Y),
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
}

/// Fracture the subject and spawn every piece. This is the function a game's death handler replaces:
/// same loop, but each chunk gets a real rigid body and collider instead of a [`Chunk`].
fn break_it(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut breaks: ResMut<BreakCount>,
    existing: Query<Entity, With<Chunk>>,
) {
    for e in &existing {
        commands.entity(e).despawn();
    }

    // A two-part solid, exactly as the ECS bake would see a character's scene.
    let torso = Mesh::from(Cuboid::new(0.7, 1.1, 0.4));
    let head = Mesh::from(Cuboid::new(0.4, 0.4, 0.4));
    let parts = [
        (&torso, Mat4::IDENTITY),
        (&head, Mat4::from_translation(Vec3::new(0.0, 0.74, 0.0))),
    ];

    let seed = 0x00C0_FFEE_u32.wrapping_add(breaks.0.wrapping_mul(2_654_435_761));
    breaks.0 = breaks.0.wrapping_add(1);
    let extent = 0.74;
    let pieces = fracture_mesh(&parts, TARGET, extent * MIN_FRACTION, seed, None);

    // The subject's own surface, and the raw interior the cut exposed. Two materials, one read.
    let skin = materials.add(StandardMaterial {
        base_color: Color::srgb(0.30, 0.42, 0.52),
        perceptual_roughness: 0.85,
        ..default()
    });
    let interior = materials.add(StandardMaterial {
        base_color: Color::srgb(0.52, 0.09, 0.08),
        perceptual_roughness: 0.55,
        ..default()
    });

    // Spawn origin: the solid stood with its feet on the floor.
    let origin = Vec3::new(0.0, 1.0, 0.0);

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
                Transform::from_translation(origin + piece.center_local),
                Visibility::default(),
            ))
            .id();

        // Both meshes are already recentred on the fragment's own centre, so the chunk spins about
        // itself rather than orbiting the origin.
        commands.entity(chunk).with_children(|parent| {
            if let Some(outer) = piece.outer {
                parent.spawn((Mesh3d(meshes.add(outer)), MeshMaterial3d(skin.clone())));
            }
            if let Some(cap) = piece.cap {
                parent.spawn((Mesh3d(meshes.add(cap)), MeshMaterial3d(interior.clone())));
            }
        });
    }
}

fn rebreak_on_space(
    keys: Res<ButtonInput<KeyCode>>,
    commands: Commands,
    meshes: ResMut<Assets<Mesh>>,
    materials: ResMut<Assets<StandardMaterial>>,
    breaks: ResMut<BreakCount>,
    existing: Query<Entity, With<Chunk>>,
) {
    if keys.just_pressed(KeyCode::Space) {
        break_it(commands, meshes, materials, breaks, existing);
    }
}

/// The example's whole solver: gravity, a ground bounce, and tumbling. Replace with yours.
fn integrate(time: Res<Time>, mut chunks: Query<(&mut Chunk, &mut Transform)>) {
    let dt = time.delta_secs();
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

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
//!   R               reset
//! ```
//!
//! **Nothing here is in the crate.** `bevy_autogib` hands out a reach — a severity per bond — and
//! this example picks the threshold at which one gives way, decides which island is still "the
//! body", and throws the rest. A game scales that severity by material and by how much damage the
//! blow carried; none of those are facts the crate has.
//!
//! Needs a GPU.
//!
//! Run: `cargo run -p bevy_autogib --example sever`

use std::collections::HashSet;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy_autogib::{
    BondGraph, BondSet, CutSettings, FragmentId, FragmentTree, ProxyCell, capsule, directional,
    fracture_mesh, hash_f32, radial, spread, swept_triangle,
};

/// Finest fragment count. Higher than `explode.rs`'s because a localised hit should be able to take
/// something *small* off — at a dozen pieces every hit removes a quarter of the body.
const TARGET: usize = 34;
/// Stop cutting a piece below this fraction of the whole solid's extent.
const MIN_FRACTION: f32 = 0.08;
/// How many cuts deep the hierarchy may go.
const MAX_DEPTH: u16 = 64;

/// **The line between "reached" and "severed", and it lives here rather than in the crate.**
/// A game would scale each bond's severity by what the thing is made of and how much damage the blow
/// carried before comparing; this example takes the reach at face value.
const GIVES_WAY: f32 = 0.5;

const GRAVITY: f32 = 18.0;
const RESTITUTION: f32 = 0.3;
const GROUND_DRAG: f32 = 4.0;
const PLAYBACK_SPEED: f32 = 0.55;

/// Where the subject stands: feet on the floor.
const ORIGIN: Vec3 = Vec3::new(0.0, 1.0, 0.0);

/// The two shells the subject is made of, each with its transform relative to the subject root.
///
/// **The head sits exactly on the torso — `0.55 + 0.2 = 0.75` — and that is load-bearing here in a
/// way it is not in `explode.rs`.** Two cells are neighbours only when they share a *coplanar* face;
/// cells that merely overlap or abut without agreeing on a plane get no bond, by design. `explode.rs`
/// puts the head at `0.74`, overlapping the torso by a centimetre, which is fine when the whole
/// subject bursts at once — and wrong here, because an unbonded head is its own island from the
/// start and drops off at the first blow anywhere. Caught by
/// `a_hit_takes_part_of_the_subject_and_leaves_the_rest_standing`, not by looking at it.
fn subject() -> [(Mesh, Mat4); 2] {
    [
        (Mesh::from(Cuboid::new(0.7, 1.1, 0.4)), Mat4::IDENTITY),
        (Mesh::from(Cuboid::new(0.4, 0.4, 0.4)), Mat4::from_translation(Vec3::new(0.0, 0.75, 0.0))),
    ]
}

/// One convex cell per shell — the caller's decomposition, matching `subject()` exactly.
fn proxy() -> Vec<ProxyCell> {
    vec![
        ProxyCell::from_box(Vec3::ZERO, Vec3::new(0.35, 0.55, 0.2)),
        ProxyCell::from_box(Vec3::new(0.0, 0.75, 0.0), Vec3::splat(0.2)),
    ]
}

/// What one baked fragment needs to be spawned, resolved once so a reset costs no geometry work.
struct Part {
    outer: Option<Handle<Mesh>>,
    cap: Option<Handle<Mesh>>,
    center_local: Vec3,
    /// How far the piece's lowest point sits below its centre, from the cell rather than the bound.
    drop_to_rest: f32,
    /// **Kept, because adjacency is per frontier.** A game hands this to `Collider::convex_hull`;
    /// here it is also what `BondGraph::of` needs to bond whichever frontier is standing.
    cell: ProxyCell,
}

/// **The bake, and it happens once.** Everything a blow does is a query against this.
#[derive(Resource)]
struct Baked {
    tree: FragmentTree,
    /// Indexed by [`FragmentId`], parallel to the tree.
    parts: Vec<Part>,
}

impl Baked {
    /// The adjacency for one frontier.
    ///
    /// **Not the leaf graph.** A fragment off a graph's frontier has no incident bonds, so reading a
    /// coarse frontier against `Fracture::bonds` reports every piece as its own island and the
    /// subject falls apart on the first blow. Rebuilt per frontier instead — cheap, because the
    /// match is over a few dozen convex cells.
    fn graph_for(&self, ids: &[FragmentId]) -> BondGraph {
        let members: Vec<(FragmentId, &ProxyCell)> =
            ids.iter().filter_map(|&id| self.parts.get(id.index()).map(|p| (id, &p.cell))).collect();
        BondGraph::of(&members, self.tree.len())
    }
}

/// The caller's accumulated damage — the graph of whatever frontier is standing, which of its bonds
/// have gone, and which fragments have already left.
///
/// The graph lives here rather than beside the bake because it is per frontier, and so is the
/// `BondSet`: `BondId`s are positions in one graph, so changing granularity means starting both over.
#[derive(Resource)]
struct Damage {
    bonds: BondGraph,
    broken: BondSet,
    gone: HashSet<FragmentId>,
}

/// Where the next blow lands, in subject-local space.
#[derive(Resource)]
struct Aim(Vec3);

/// Marks the little sphere that shows [`Aim`].
#[derive(Component)]
struct AimMarker;

/// A fragment still attached to the body.
#[derive(Component)]
struct Attached(FragmentId);

/// A fragment that came loose and is now the example's problem rather than the crate's.
#[derive(Component)]
struct Chunk {
    velocity: Vec3,
    spin: Vec3,
    drop_to_rest: f32,
}

#[derive(Resource)]
struct DemoMaterials {
    skin: Handle<StandardMaterial>,
    interior: Handle<StandardMaterial>,
    aim: Handle<StandardMaterial>,
}

/// Which frontier of the hierarchy is currently standing — the granularity dial, on a key.
#[derive(Resource)]
struct Granularity(usize);

/// The counts `G` cycles through. One bake answers all of them.
const GRANULARITIES: [usize; 4] = [3, 8, 16, TARGET];

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
        .add_systems(Startup, setup)
        .add_systems(Update, (aim_marker, strike, integrate))
        .run();
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    granularity: Res<Granularity>,
) {
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(3.0, 2.0, 4.2).looking_at(Vec3::new(0.0, 0.95, 0.0), Vec3::Y),
    ));
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
        aim: materials.add(StandardMaterial {
            base_color: Color::srgb(0.95, 0.85, 0.25),
            emissive: LinearRgba::rgb(0.6, 0.5, 0.1),
            ..default()
        }),
    };

    // **One bake, at startup, and never again.** Every blow below is a query against it.
    let owned = subject();
    let parts: Vec<(&Mesh, Mat4)> = owned.iter().map(|(m, x)| (m, *x)).collect();
    let cut = CutSettings { max_depth: MAX_DEPTH, ..CutSettings::new(TARGET, MIN_FRACTION, 0x00C0_FFEE) };
    let baked = fracture_mesh(&parts, &proxy(), &cut);

    info!(
        "baked {} fragments ({} finest, {} cuts) with {} bonds between them",
        baked.fragments.len(),
        baked.tree.leaves().len(),
        baked.tree.cuts(),
        baked.bonds.len()
    );

    let resolved: Vec<Part> = baked
        .fragments
        .into_iter()
        .map(|f| {
            let lowest = f.cell.points().iter().map(|p| p.y).fold(f32::INFINITY, f32::min);
            Part {
                outer: f.outer.map(|m| meshes.add(m)),
                cap: f.cap.map(|m| meshes.add(m)),
                center_local: f.center_local,
                drop_to_rest: (f.cell.center().y - lowest).max(0.0),
                cell: f.cell,
            }
        })
        .collect();

    let baked = Baked { tree: baked.tree, parts: resolved };
    let damage = fresh_damage(&baked, granularity.0);

    commands.spawn((
        AimMarker,
        Mesh3d(meshes.add(Mesh::from(Sphere::new(0.05)))),
        MeshMaterial3d(mats.aim.clone()),
        Transform::from_translation(ORIGIN),
    ));

    spawn_standing(&mut commands, &baked, &mats, granularity.0);
    commands.insert_resource(baked);
    commands.insert_resource(damage);
    commands.insert_resource(mats);
}

/// Stand the subject up at one frontier of the hierarchy.
fn spawn_standing(commands: &mut Commands, baked: &Baked, mats: &DemoMaterials, granularity: usize) {
    for id in baked.tree.frontier_of(GRANULARITIES[granularity]) {
        spawn_fragment(commands, baked, mats, id, None);
    }
}

/// An undamaged state for one frontier: its own graph, and an empty set over it.
fn fresh_damage(baked: &Baked, granularity: usize) -> Damage {
    let ids = baked.tree.frontier_of(GRANULARITIES[granularity]);
    let bonds = baked.graph_for(&ids);
    let broken = BondSet::new(&bonds);
    info!("standing at {} pieces, held together by {} bonds", ids.len(), bonds.len());
    Damage { bonds, broken, gone: HashSet::new() }
}

/// One fragment, attached if `launch` is `None` and flying if it is.
fn spawn_fragment(
    commands: &mut Commands,
    baked: &Baked,
    mats: &DemoMaterials,
    id: FragmentId,
    launch: Option<(Vec3, Vec3)>,
) {
    let Some(part) = baked.parts.get(id.index()) else { return };
    let mut e = commands.spawn((
        Transform::from_translation(ORIGIN + part.center_local),
        Visibility::default(),
    ));
    match launch {
        Some((velocity, spin)) => {
            e.insert(Chunk { velocity, spin, drop_to_rest: part.drop_to_rest });
        }
        None => {
            e.insert(Attached(id));
        }
    }
    let entity = e.id();
    commands.entity(entity).with_children(|parent| {
        if let Some(outer) = &part.outer {
            parent.spawn((Mesh3d(outer.clone()), MeshMaterial3d(mats.skin.clone())));
        }
        if let Some(cap) = &part.cap {
            parent.spawn((Mesh3d(cap.clone()), MeshMaterial3d(mats.interior.clone())));
        }
    });
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

/// **The whole feature, on five keys.** Pick a region, threshold the reach it comes back with, sever
/// what gave way, and re-run island detection to see what is no longer holding on.
#[derive(SystemParam)]
struct Scene<'w, 's> {
    baked: Res<'w, Baked>,
    mats: Res<'w, DemoMaterials>,
    aim: Res<'w, Aim>,
    attached: Query<'w, 's, (Entity, &'static Attached)>,
    chunks: Query<'w, 's, Entity, With<Chunk>>,
}

fn strike(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    scene: Scene,
    mut damage: ResMut<Damage>,
    mut granularity: ResMut<Granularity>,
) {
    if keys.just_pressed(KeyCode::KeyR) || keys.just_pressed(KeyCode::KeyG) {
        if keys.just_pressed(KeyCode::KeyG) {
            granularity.0 = (granularity.0 + 1) % GRANULARITIES.len();
            info!(
                "granularity: standing at {} pieces — same bake, different frontier",
                GRANULARITIES[granularity.0]
            );
        } else {
            info!("reset");
        }
        for (e, _) in &scene.attached {
            commands.entity(e).despawn();
        }
        for e in &scene.chunks {
            commands.entity(e).despawn();
        }
        *damage = fresh_damage(&scene.baked, granularity.0);
        spawn_standing(&mut commands, &scene.baked, &scene.mats, granularity.0);
        return;
    }

    let at = scene.aim.0;
    // Each key is a region, and the crate has no idea which weapon any of them is.
    let (label, reach) = if keys.just_pressed(KeyCode::Digit1) {
        ("projectile", spread(&damage.bonds, at, 0.06, 0.34))
    } else if keys.just_pressed(KeyCode::Digit2) {
        ("slash", capsule(&damage.bonds, at - Vec3::X * 0.45, at + Vec3::X * 0.45, 0.05, 0.16))
    } else if keys.just_pressed(KeyCode::Digit3) {
        // A blade sweeping down and across through the aim point: two corners above, one below.
        (
            "swept blade",
            swept_triangle(
                &damage.bonds,
                at + Vec3::new(-0.9, 0.35, -0.9),
                at + Vec3::new(0.9, 0.35, -0.9),
                at + Vec3::new(0.0, -0.35, 0.9),
            ),
        )
    } else if keys.just_pressed(KeyCode::Digit4) {
        ("blast", radial(&damage.bonds, at, 0.15, 1.10))
    } else if keys.just_pressed(KeyCode::Digit5) {
        ("pull", directional(&damage.bonds, at, Vec3::Y, 0.20, 0.85))
    } else {
        return;
    };

    let gave_way = reach.above(GIVES_WAY);
    let newly = damage.broken.sever_all(&gave_way);
    info!(
        "{label} at {:.2},{:.2},{:.2} — reached {} bonds, {} gave way ({} newly)",
        at.x,
        at.y,
        at.z,
        reach.len(),
        gave_way.len(),
        newly
    );
    if newly == 0 {
        return;
    }

    // What is still standing, and which parts of it are still connected to each other.
    let standing: Vec<FragmentId> = scene.attached.iter().map(|(_, a)| a.0).collect();
    let islands = damage.bonds.islands(&standing, &damage.broken);
    if islands.len() < 2 {
        return;
    }

    // **The body is the biggest island.** That is this example's rule, not the crate's — a game with
    // a floor would more likely keep whichever island is still standing on it.
    let body = islands
        .iter()
        .enumerate()
        .max_by_key(|(_, i)| i.len())
        .map(|(k, _)| k)
        .unwrap_or(0);

    for (k, island) in islands.iter().enumerate() {
        if k == body {
            continue;
        }
        for id in island {
            damage.gone.insert(*id);
        }
    }
    let leaving: HashSet<FragmentId> = damage.gone.clone();

    for (entity, a) in &scene.attached {
        if !leaving.contains(&a.0) {
            continue;
        }
        commands.entity(entity).despawn();
        let Some(part) = scene.baked.parts.get(a.0.index()) else { continue };

        // Thrown away from where the blow landed, with a little deterministic variation from the
        // crate's own frozen hash — no rand dependency in an example either.
        let h = |n: u32| hash_f32(a.0.0.wrapping_mul(2_654_435_761).wrapping_add(n));
        let away = (part.center_local - at).normalize_or_zero();
        let dir = (away + Vec3::Y * (0.35 + 0.5 * h(3))).normalize_or_zero();
        let velocity = dir * (2.4 + 2.0 * h(4));
        let spin = Vec3::new(h(1) - 0.5, h(2) - 0.5, h(3) - 0.5).normalize_or_zero() * (7.0 + 7.0 * h(2));
        spawn_fragment(&mut commands, &scene.baked, &scene.mats, a.0, Some((velocity, spin)));
    }
    info!("  {} fragment(s) came off; {} still standing", leaving.len(), islands[body].len());
}

/// The example's whole solver — the crate names none. Identical to `explode.rs`'s on purpose.
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

        let floor = chunk.drop_to_rest;
        if transform.translation.y < floor {
            transform.translation.y = floor;
            if chunk.velocity.y < 0.0 {
                chunk.velocity.y = -chunk.velocity.y * RESTITUTION;
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

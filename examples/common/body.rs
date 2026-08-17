//! **The subject `sever` takes apart, and the rules for taking it apart.**
//!
//! Shared by the interactive example and by the recorder that turns it into a GIF, so the animation
//! in the README is the same code you can play with rather than a re-implementation of it. A
//! recorder that reimplements its subject drifts from it silently, and the drift is invisible in
//! exactly the place you would look for it — the picture.
//!
//! Everything here works on `&mut World` rather than on `Commands`, because that is the shape both
//! callers can drive: a Bevy system can take `&mut World`, and a hand-pumped headless loop has
//! nothing else.
//!
//! **None of this is in the crate.** `bevy_autogib` hands out a reach — a severity per bond — and
//! everything below picks the threshold at which one gives way, decides which island is still "the
//! body", and throws the rest.

#![allow(dead_code)]

use std::collections::HashSet;

use bevy::prelude::*;
use bevy_autogib::{
    BondGraph, BondSet, CutSettings, FragmentId, FragmentTree, ProxyCell, Reach, capsule,
    directional, fracture_mesh, hash_f32, radial, spread, swept_triangle,
};

use super::material;

/// Finest fragment count. Higher than `explode.rs`'s because a localised hit should be able to take
/// something *small* off — at a dozen pieces every hit removes a quarter of the body.
pub const TARGET: usize = 34;
/// Stop cutting a piece below this fraction of the whole solid's extent.
pub const MIN_FRACTION: f32 = 0.08;
/// How many cuts deep the hierarchy may go.
pub const MAX_DEPTH: u16 = 64;
/// The seed. Same seed, same pieces, every run — which is what makes the GIF diffable.
pub const SEED: u32 = 0x00C0_FFEE;

/// **The line between "reached" and "severed", and it lives here rather than in the crate.**
/// A game would scale each bond's severity by what the thing is made of and how much damage the blow
/// carried before comparing; these examples take the reach at face value.
pub const GIVES_WAY: f32 = 0.5;

/// Where the subject stands: feet on the floor. The lowest point is the leg bottom at `y = -0.92`.
pub const ORIGIN: Vec3 = Vec3::new(0.0, 0.92, 0.0);

/// The rounding strengths the `T` key cycles through. Changing this re-bakes, because the softening
/// happens when the drawn mesh is built — it is not something a shader can be toggled.
pub const SOFTENINGS: [f32; 4] = [0.0, 0.25, 0.5, 0.75];

/// The piece counts the granularity dial cycles through. One bake answers all of them.
///
/// **The first entry is the six body parts**, because `frontier_of(6)` on a six-cell proxy is the
/// roots — uncut. That is dismemberment; the last entry is gibs; the two in between are the range.
pub const GRANULARITIES: [usize; 4] = [6, 12, 20, TARGET];

pub const GRAVITY: f32 = 18.0;
pub const RESTITUTION: f32 = 0.3;
pub const GROUND_DRAG: f32 = 4.0;

/// **A blocked-out humanoid: one convex cell per body part.**
///
/// This replaced a torso box and a head box, and the reason is the whole point of the crate. Cutting
/// a limbless mass with pseudorandom planes produces wedges sliced diagonally out of a blob — which
/// reads as a frozen statue shattering, however good the fracture is, because none of the pieces is
/// a *part of a body*. Nothing tuned in the cutter fixes that; the subject had no anatomy to break
/// along.
///
/// **The joints come for free, and that is not a coincidence.** A joint is two body parts meeting
/// over a shared surface, and [`BondGraph::of`](bevy_autogib::BondGraph::of) already finds exactly
/// that — coplanar faces, opposite normals, positive overlap. Laid out this way the bond graph comes
/// back with one bond per joint, its area the joint's own cross-section:
///
/// ```text
/// torso <-> head    area 0.0676   the neck
/// torso <-> arm.L   area 0.1040   the shoulder
/// torso <-> leg.L   area 0.0528   the hip
/// ```
///
/// So a hit on the shoulder takes off the arm, at every granularity, with no code that knows what an
/// arm is. Read the bake at [`GRANULARITIES`]`[0]` and the pieces *are* the body parts; read it at
/// the finest and they are gibs.
///
/// # Two placements that are load-bearing
///
/// **Every part touches its neighbour exactly, never overlapping.** Cells that interpenetrate share
/// no coplanar face and get no bond, so an overlapping limb would hang off its own island and drop
/// at the first blow anywhere. Each part's face sits *on* the torso's, not inside it.
///
/// **The legs are held apart by 0.04.** Touching at `x = 0` they would share a coplanar face and the
/// graph would correctly — and uselessly — bond the legs to each other, so severing a hip would
/// leave a leg dangling from its twin. Anatomy is the caller's business, and this is what that costs.
pub fn subject() -> Vec<(Mesh, Mat4)> {
    parts()
        .into_iter()
        .map(|(_, c, h)| (Mesh::from(Cuboid::new(h.x * 2.0, h.y * 2.0, h.z * 2.0)), Mat4::from_translation(c)))
        .collect()
}

/// One convex cell per body part — the caller's decomposition, matching [`subject`] exactly.
pub fn proxy() -> Vec<ProxyCell> {
    parts().into_iter().map(|(_, c, h)| ProxyCell::from_box(c, h)).collect()
}

/// The blockout: `(name, centre, half-extents)`, in subject-local space. The name is for logging —
/// nothing in the crate ever sees it, and nothing here branches on it.
pub fn parts() -> Vec<(&'static str, Vec3, Vec3)> {
    vec![
        ("torso", Vec3::new(0.00, 0.00, 0.0), Vec3::new(0.22, 0.32, 0.14)),
        ("head", Vec3::new(0.00, 0.46, 0.0), Vec3::new(0.13, 0.14, 0.13)),
        ("arm.L", Vec3::new(-0.32, 0.06, 0.0), Vec3::new(0.10, 0.26, 0.10)),
        ("arm.R", Vec3::new(0.32, 0.06, 0.0), Vec3::new(0.10, 0.26, 0.10)),
        ("leg.L", Vec3::new(-0.13, -0.62, 0.0), Vec3::new(0.11, 0.30, 0.12)),
        ("leg.R", Vec3::new(0.13, -0.62, 0.0), Vec3::new(0.11, 0.30, 0.12)),
    ]
}

/// Which body part a fragment came out of, by walking the hierarchy back to its root cell.
pub fn part_of(id: FragmentId, tree: &FragmentTree) -> &'static str {
    let root = tree.root_of(id).unwrap_or(id);
    parts().get(root.index()).map(|(n, _, _)| *n).unwrap_or("?")
}

/// What one baked fragment needs to be spawned, resolved once so a reset costs no geometry work.
pub struct Part {
    pub outer: Option<Handle<Mesh>>,
    pub cap: Option<Handle<Mesh>>,
    pub center_local: Vec3,
    /// How far the piece's lowest point sits below its centre, from the cell rather than the bound.
    pub drop_to_rest: f32,
    /// **Kept, because adjacency is per frontier.** A game hands this to `Collider::convex_hull`;
    /// here it is also what `BondGraph::of` needs to bond whichever frontier is standing.
    pub cell: ProxyCell,
    /// The fragment's volume, which is its mass at uniform density. Drives how fast it is thrown.
    pub volume: f32,
}

/// **The bake, and it happens once.** Everything a blow does is a query against this.
#[derive(Resource)]
pub struct Baked {
    pub tree: FragmentTree,
    /// Indexed by [`FragmentId`], parallel to the tree.
    pub parts: Vec<Part>,
}

impl Baked {
    /// Fracture the subject and register every fragment's meshes.
    pub fn bake(world: &mut World, soften: f32) -> Baked {
        let owned = subject();
        let parts: Vec<(&Mesh, Mat4)> = owned.iter().map(|(m, x)| (m, *x)).collect();
        let cut = CutSettings {
            max_depth: MAX_DEPTH,
            soften,
            ..CutSettings::new(TARGET, MIN_FRACTION, SEED)
        };
        let baked = fracture_mesh(&parts, &proxy(), &cut);

        info!(
            "baked {} fragments ({} finest, {} cuts) with {} bonds between them",
            baked.fragments.len(),
            baked.tree.leaves().len(),
            baked.tree.cuts(),
            baked.bonds.len()
        );
        info!("soften {soften:.2} — rounding the drawn surface only; the colliders are unchanged");

        let parts = baked
            .fragments
            .into_iter()
            .map(|f| {
                let lowest = f.cell.points().iter().map(|p| p.y).fold(f32::INFINITY, f32::min);
                let meshes = &mut world.resource_mut::<Assets<Mesh>>();
                Part {
                    outer: f.outer.map(|m| meshes.add(m)),
                    cap: f.cap.map(|m| meshes.add(m)),
                    center_local: f.center_local,
                    drop_to_rest: (f.cell.center().y - lowest).max(0.0),
                    volume: f.cell.volume().max(1.0e-6),
                    cell: f.cell,
                }
            })
            .collect();
        Baked { tree: baked.tree, parts }
    }

    /// The adjacency for one frontier.
    ///
    /// **Not the leaf graph.** A fragment off a graph's frontier has no incident bonds, so reading a
    /// coarse frontier against `Fracture::bonds` reports every piece as its own island and the
    /// subject falls apart on the first blow. Rebuilt per frontier instead — cheap, because the
    /// match is over a few dozen convex cells.
    pub fn graph_for(&self, ids: &[FragmentId]) -> BondGraph {
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
pub struct Damage {
    pub bonds: BondGraph,
    pub broken: BondSet,
    pub gone: HashSet<FragmentId>,
}

impl Damage {
    /// An undamaged state for one frontier: its own graph, and an empty set over it.
    pub fn fresh(baked: &Baked, granularity: usize) -> Damage {
        let ids = baked.tree.frontier_of(GRANULARITIES[granularity]);
        let bonds = baked.graph_for(&ids);
        let broken = BondSet::new(&bonds);
        info!("standing at {} pieces, held together by {} bonds", ids.len(), bonds.len());
        Damage { bonds, broken, gone: HashSet::new() }
    }
}

/// A fragment still attached to the body.
#[derive(Component)]
pub struct Attached(pub FragmentId);

/// A fragment that came loose and is now the example's problem rather than the crate's.
#[derive(Component)]
pub struct Chunk {
    pub velocity: Vec3,
    pub spin: Vec3,
    pub drop_to_rest: f32,
}

#[derive(Resource)]
pub struct BodyMaterials {
    pub skin: Handle<StandardMaterial>,
    pub interior: Handle<StandardMaterial>,
    pub aim: Handle<StandardMaterial>,
}

impl BodyMaterials {
    pub fn new(world: &mut World) -> BodyMaterials {
        let aim = world.resource_mut::<Assets<StandardMaterial>>().add(StandardMaterial {
            base_color: Color::srgb(0.95, 0.85, 0.25),
            emissive: LinearRgba::rgb(0.6, 0.5, 0.1),
            ..default()
        });
        BodyMaterials {
            skin: material(world, Color::srgb(0.30, 0.42, 0.52), 0.85),
            // **Darker and much glossier than a "red material".** At roughness 0.55 a flat cut face
            // is lit evenly across its whole area, which is what stone and ice look like; dropping it
            // to 0.25 puts a moving highlight on each face instead, and that specular travel is most
            // of what reads as wet. The colour goes down, not up: bright red on a flat plane reads as
            // paint.
            interior: material(world, Color::srgb(0.46, 0.07, 0.07), 0.42),
            aim,
        }
    }
}

/// **The five regions, as this example aims them.** The crate has no idea which weapon any of them
/// is; the names here are the example's reading, and the radii are its tuning.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Blow {
    Projectile,
    Slash,
    SweptBlade,
    Blast,
    Pull,
}

impl Blow {
    pub fn reach(self, graph: &BondGraph, at: Vec3) -> Reach {
        match self {
            // **0.6, and it was measured rather than picked.** At 0.34 a projectile freed a piece
            // only where the body is thin — the head — because an interior fragment is held by six
            // or eight bonds and the region has to reach past all of them. Freed pieces per hit at
            // head / shoulder / flank / hip: 0.34 -> [1,0,0,0], 0.45 -> [1,0,0,1], 0.60 -> [2,1,1,1].
            Blow::Projectile => spread(graph, at, 0.08, 0.60),
            // **The radius has to beat the subject's depth, not look reasonable.** At `max = 0.16`
            // the effective cut is about 0.105 at the shipped threshold and the body is 0.4 deep, so
            // the bonds at the front and back survived, the halves stayed joined around them, and a
            // slash severed eleven bonds while detaching nothing at all. Freed pieces per slash at
            // four heights: 0.30 -> [0,0,1,1], 0.36 -> [1,14,2,2], 0.42 -> [4,15,16,9]. Cleaving is
            // what a slash is for, so 0.42.
            Blow::Slash => capsule(graph, at - Vec3::X * 0.5, at + Vec3::X * 0.5, 0.10, 0.42),
            // A blade sweeping down and across through the aim point: two corners above, one below.
            Blow::SweptBlade => swept_triangle(
                graph,
                at + Vec3::new(-0.9, 0.35, -0.9),
                at + Vec3::new(0.9, 0.35, -0.9),
                at + Vec3::new(0.0, -0.35, 0.9),
            ),
            Blow::Blast => radial(graph, at, 0.15, 1.10),
            Blow::Pull => directional(graph, at, Vec3::Y, 0.20, 0.85),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Blow::Projectile => "projectile",
            Blow::Slash => "slash",
            Blow::SweptBlade => "swept blade",
            Blow::Blast => "blast",
            Blow::Pull => "pull",
        }
    }
}

/// Stand the subject up at one frontier of the hierarchy.
pub fn stand(world: &mut World, granularity: usize) {
    let ids = {
        let baked = world.resource::<Baked>();
        baked.tree.frontier_of(GRANULARITIES[granularity])
    };
    for id in ids {
        spawn_fragment(world, id, None);
    }
}

/// Despawn everything the body is currently made of, attached or flying.
pub fn clear(world: &mut World) {
    let doomed: Vec<Entity> = world
        .query_filtered::<Entity, Or<(With<Attached>, With<Chunk>)>>()
        .iter(world)
        .collect();
    for e in doomed {
        world.entity_mut(e).despawn();
    }
}

/// One fragment, attached if `launch` is `None` and flying if it is.
///
/// Both meshes are already recentred on the fragment's own centre, so a chunk spins about itself
/// rather than orbiting the origin.
pub fn spawn_fragment(world: &mut World, id: FragmentId, launch: Option<(Vec3, Vec3)>) {
    let Some((outer, cap, center, rest)) = world.get_resource::<Baked>().and_then(|b| {
        b.parts
            .get(id.index())
            .map(|p| (p.outer.clone(), p.cap.clone(), p.center_local, p.drop_to_rest))
    }) else {
        return;
    };
    let Some(mats) = world.get_resource::<BodyMaterials>().map(|m| (m.skin.clone(), m.interior.clone()))
    else {
        return;
    };

    let mut e = world.spawn((Transform::from_translation(ORIGIN + center), Visibility::default()));
    match launch {
        Some((velocity, spin)) => {
            e.insert(Chunk { velocity, spin, drop_to_rest: rest });
        }
        None => {
            e.insert(Attached(id));
        }
    }
    let entity = e.id();
    world.entity_mut(entity).with_children(|parent| {
        if let Some(outer) = outer {
            parent.spawn((Mesh3d(outer), MeshMaterial3d(mats.0)));
        }
        if let Some(cap) = cap {
            parent.spawn((Mesh3d(cap), MeshMaterial3d(mats.1)));
        }
    });
}

/// What one blow actually did — enough for a caller to tell "nothing happened" from "nothing
/// happened *yet*", which are different and look identical on screen without it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Outcome {
    /// Bonds the region touched at all.
    pub reached: usize,
    /// Bonds whose severity cleared the threshold.
    pub gave_way: usize,
    /// Of those, how many were still intact before this blow.
    pub newly: usize,
    /// Fragments that stopped being connected to the body.
    pub off: usize,
}

/// **The whole feature in one function.** Pick a region, threshold the reach it comes back with,
/// sever what gave way, and re-run island detection to see what is no longer holding on.
pub fn strike(world: &mut World, blow: Blow, at: Vec3) -> Outcome {
    let standing: Vec<(Entity, FragmentId)> =
        world.query::<(Entity, &Attached)>().iter(world).map(|(e, a)| (e, a.0)).collect();
    if standing.is_empty() {
        return Outcome::default();
    }
    let mut outcome = Outcome::default();

    // Which fragments stopped being connected to the body. Scoped so every borrow of `Damage` and
    // `Baked` has ended before anything is spawned.
    let leaving: Vec<(Entity, FragmentId)> = {
        let Some(mut damage) = world.remove_resource::<Damage>() else { return Outcome::default() };
        let reach = blow.reach(&damage.bonds, at);
        let gave_way = reach.above(GIVES_WAY);
        let newly = damage.broken.sever_all(&gave_way);
        outcome.reached = reach.len();
        outcome.gave_way = gave_way.len();
        outcome.newly = newly;
        info!(
            "{} at {:.2},{:.2},{:.2} — reached {} bonds, {} gave way ({newly} newly)",
            blow.label(),
            at.x,
            at.y,
            at.z,
            reach.len(),
            gave_way.len()
        );

        let ids: Vec<FragmentId> = standing.iter().map(|(_, id)| *id).collect();
        let islands = damage.bonds.islands(&ids, &damage.broken);
        // **The body is the biggest island.** That is this example's rule, not the crate's — a game
        // with a floor would more likely keep whichever island is still standing on it.
        let body = islands.iter().enumerate().max_by_key(|(_, i)| i.len()).map(|(k, _)| k);
        for (k, island) in islands.iter().enumerate() {
            if Some(k) != body {
                damage.gone.extend(island.iter().copied());
            }
        }
        let gone = damage.gone.clone();
        world.insert_resource(damage);
        standing.into_iter().filter(|(_, id)| gone.contains(id)).collect()
    };

    for (entity, id) in &leaving {
        world.entity_mut(*entity).despawn();
        let center = world
            .get_resource::<Baked>()
            .and_then(|b| b.parts.get(id.index()).map(|p| p.center_local))
            .unwrap_or(Vec3::ZERO);
        let volume = world
            .get_resource::<Baked>()
            .and_then(|b| b.parts.get(id.index()).map(|p| p.volume))
            .unwrap_or(REFERENCE_VOLUME);
        let (velocity, spin) = launch(*id, center, at, volume);
        spawn_fragment(world, *id, Some((velocity, spin)));
    }
    if !leaving.is_empty() {
        info!("  {} fragment(s) came off", leaving.len());
    }
    outcome.off = leaving.len();
    outcome
}

/// Thrown away from where the blow landed, with deterministic variation from the crate's own frozen
/// hash — no RNG dependency in an example either.
///
/// **Speed and spin scale down with mass, and that is most of the difference between "gibs" and
/// "shrapnel".** Throwing every piece at the same speed is what made the old burst read as an
/// explosion in a quarry: a severed arm and a fingernail left at identical velocity. A blow delivers
/// roughly an impulse, so light pieces should leave fast and heavy ones should barely move and flop.
pub fn launch(id: FragmentId, center: Vec3, at: Vec3, volume: f32) -> (Vec3, Vec3) {
    let h = |n: u32| hash_f32(id.0.wrapping_mul(2_654_435_761).wrapping_add(n));
    let away = (center - at).normalize_or_zero();
    let dir = (away + Vec3::Y * (0.35 + 0.5 * h(3))).normalize_or_zero();
    // Cube root, because velocity from a fixed impulse goes as 1/mass and mass goes as the cube of
    // size — so the linear dimension is the honest scale, and it keeps the spread narrow enough that
    // a heavy piece still moves.
    let heft = heft(volume);
    let spin = Vec3::new(h(1) - 0.5, h(2) - 0.5, h(3) - 0.5).normalize_or_zero() * (7.0 + 7.0 * h(2));
    (dir * (2.4 + 2.0 * h(4)) * heft, spin * heft)
}

/// The fragment size that leaves at unmodified speed — roughly a mid-sized chunk of this subject.
/// Everything smaller goes faster, everything larger slower.
pub const REFERENCE_VOLUME: f32 = 0.012;

/// How much faster than a reference-sized chunk a fragment of this volume leaves.
///
/// Cube root, because velocity from a fixed impulse goes as `1/mass` and mass goes as the cube of
/// size — so the linear dimension is the honest scale, and it keeps the spread narrow enough that a
/// heavy piece still moves rather than sitting perfectly still.
pub fn heft(volume: f32) -> f32 {
    (REFERENCE_VOLUME / volume.max(1.0e-6)).cbrt().clamp(0.45, 2.2)
}

/// The examples' whole solver — the crate names none. Gravity, a ground bounce, and tumbling.
pub fn integrate(chunk: &mut Chunk, transform: &mut Transform, dt: f32) {
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

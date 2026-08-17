//! The bake: walk a subject's loaded scene once, fracture it, and keep the result forever.
//!
//! This is the half that touches the ECS and the asset arena. Everything here exists to answer one
//! question — *what does this asset look like broken?* — exactly once per source asset, and to answer
//! it the same way on every machine.

use std::collections::{HashMap, HashSet};

use bevy::asset::AssetPath;
use bevy::prelude::*;

use crate::FractureSettings;
use crate::bond::BondGraph;
use crate::mesh::{append_mesh, geometry_from_piece, geometry_from_soup};
use crate::proxy::ProxyCell;
use crate::soup::{Soup, fracture};
use crate::tree::{FragmentId, FragmentTree};

/// **Marks an entity whose descendants should be pre-fractured, and names the asset to key that bake
/// by.** Put it on the root the scene hangs under; the bake walks `Children` from there.
///
/// The handle is the cache key *and* the seed source, so two subjects sharing one asset share one
/// bake — and swapping the asset needs no code change.
#[derive(Component)]
pub struct FractureSubject(pub Handle<WorldAsset>);

/// **The caller's convex decomposition of the subject, in subject-local space.**
///
/// Required alongside [`FractureSubject`]: this crate cuts a proxy, and computing a convex
/// decomposition is not its job — a consumer already running V-HACD or CoACD for colliders has one,
/// and forcing a second, different decomposition would be the fracture disagreeing with the physics
/// about what the object is. A blocked-out subject can build cells with [`ProxyCell::from_box`].
///
/// A subject with this component missing is `error!`-refused and never baked. That is deliberate: the
/// alternative is synthesising a bounding box and silently fracturing the wrong shape.
#[derive(Component)]
pub struct FractureProxy(pub Vec<ProxyCell>);

/// **Marks a subtree to be pruned out of the body and baked as one intact chunk**, keeping its own
/// material — a carried weapon, a hat, a backpack. The walk does not descend past it.
///
/// Present-but-empty is treated as "still streaming", never as "this subject has no detached part":
/// see the bake gate below for what that distinction cost.
#[derive(Component)]
pub struct DetachedPart;

/// One baked body fragment, in subject-**local** units (render scale is applied at spawn). Both
/// meshes are recentered to `center_local` (their shared bounding-box center), so a physics body
/// placed at `origin + center_local*scale` with a `half_extents*scale` box collider lines up exactly
/// with the rendered chunk. Either mesh may be `None` (a fragment with no cut faces has no cap; a
/// pure-cap sliver has no outer skin).
pub struct Fragment {
    /// Which node of this source's [`FragmentTree`] this is — and its own index in the array
    /// [`FractureCache::fragments`] returned.
    pub id: FragmentId,
    pub outer_mesh: Option<Handle<Mesh>>,
    pub cap_mesh: Option<Handle<Mesh>>,
    /// **The fragment as a solid: one convex cell.** This is what a solver wants — a single convex
    /// collider, no decomposition at spawn time and no trimesh. See `AG-007`.
    pub cell: ProxyCell,
    pub center_local: Vec3,
    /// Half the bounding box per axis (local units). **A coarse bound, not the collider** —
    /// [`Self::cell`] is the collider.
    pub half_extents: Vec3,
}

/// The pruned [`DetachedPart`], flung intact as a single tumbling chunk (it kept its own material).
/// Baked in the same subject-local space as body fragments.
pub struct DetachedChunk {
    pub mesh: Handle<Mesh>,
    pub material: Handle<StandardMaterial>,
    pub center_local: Vec3,
    pub half_extents: Vec3,
}

/// Baked fracture data, keyed by the subject's source scene asset id so multiple distinct subjects
/// each get their own bake and swapping the asset needs zero code change.
#[derive(Resource, Default)]
pub struct FractureCache {
    body: HashMap<AssetId<WorldAsset>, Vec<Fragment>>,
    trees: HashMap<AssetId<WorldAsset>, FragmentTree>,
    graphs: HashMap<AssetId<WorldAsset>, BondGraph>,
    detached: HashMap<AssetId<WorldAsset>, DetachedChunk>,
    baked: HashSet<AssetId<WorldAsset>>,
}

impl FractureCache {
    /// **Every** baked fragment for a source, interior pieces of the hierarchy included, or `None`
    /// if that source hasn't been baked.
    ///
    /// Index-parallel with [`tree`](Self::tree). Do not spawn this whole slice — it holds parents
    /// and their children both, and spawning both puts the same volume in the scene twice. Spawn a
    /// frontier: [`leaves`](Self::leaves) for the finest, [`frontier_of`](Self::frontier_of) for a
    /// chosen granularity.
    pub fn fragments(&self, source: AssetId<WorldAsset>) -> Option<&[Fragment]> {
        self.body.get(&source).map(|v| v.as_slice())
    }

    /// The fracture hierarchy for a source: which fragments nest inside which, and the frontier
    /// queries that read one bake at any granularity from the proxy cells up to the finest cut.
    pub fn tree(&self, source: AssetId<WorldAsset>) -> Option<&FragmentTree> {
        self.trees.get(&source)
    }

    /// Which of a source's finest fragments **touch** which, over how much shared face.
    ///
    /// Nesting and neighbouring are different questions: [`tree`](Self::tree) answers the first and
    /// this answers the second. Pair it with a [`BondSet`](crate::BondSet) the caller owns and
    /// [`BondGraph::islands`] to take one piece off and leave the rest standing.
    pub fn bonds(&self, source: AssetId<WorldAsset>) -> Option<&BondGraph> {
        self.graphs.get(&source)
    }

    /// The finest granularity — every fragment that was never cut further. **This is the set the
    /// cache handed out before it kept a hierarchy**, so a caller that just wants the old behaviour
    /// wants this.
    pub fn leaves(&self, source: AssetId<WorldAsset>) -> Vec<&Fragment> {
        self.pick(source, |t| t.leaves())
    }

    /// The frontier holding roughly `count` fragments, clamped to what this bake can offer — the
    /// granularity dial. Three pieces for a cleaving blow, all of them for a blast, from one bake.
    pub fn frontier_of(&self, source: AssetId<WorldAsset>, count: usize) -> Vec<&Fragment> {
        self.pick(source, |t| t.frontier_of(count))
    }

    /// The frontier at most `depth` cuts from the caller's proxy cells.
    pub fn at_depth(&self, source: AssetId<WorldAsset>, depth: u16) -> Vec<&Fragment> {
        self.pick(source, |t| t.at_depth(depth))
    }

    /// Resolve the ids a frontier query chose against this source's fragment array. An id outside
    /// the array is skipped rather than fatal.
    fn pick<F>(&self, source: AssetId<WorldAsset>, choose: F) -> Vec<&Fragment>
    where
        F: FnOnce(&FragmentTree) -> Vec<FragmentId>,
    {
        let (Some(frags), Some(tree)) = (self.body.get(&source), self.trees.get(&source)) else {
            return Vec::new();
        };
        choose(tree).into_iter().filter_map(|id| frags.get(id.index())).collect()
    }

    /// The baked [`DetachedPart`] chunk for a source, if any.
    pub fn detached_chunk(&self, source: AssetId<WorldAsset>) -> Option<&DetachedChunk> {
        self.detached.get(&source)
    }

    /// Has this source been baked at all? True even when the bake produced no fragments (a degenerate
    /// mesh), because "baked and empty" and "not yet baked" are different states and a caller waiting
    /// on the bake must be able to stop waiting.
    pub fn is_baked(&self, source: AssetId<WorldAsset>) -> bool {
        self.baked.contains(&source)
    }
}

/// **Sort by a key that must be a TOTAL order — and prove it, don't assert it in a comment.**
///
/// The one site that uses it is the one site in this crate whose input is an ECS query — which is
/// exactly where a runtime check earns its keep, because query order is not stable across `App`
/// instances. A comment asserting the key is total cannot fail; this can, and it is what caught the
/// bug described below.
///
/// Under `debug_assertions` or the `strict-order` feature it **panics naming the call site and the
/// duplicated key** the moment a tie occurs. A release build pays nothing.
fn sort_total_by_key_at<T, K, F>(site: &'static str, v: &mut [T], mut f: F)
where
    K: Ord + std::fmt::Debug,
    F: FnMut(&T) -> K,
{
    v.sort_unstable_by_key(&mut f);
    #[cfg(any(debug_assertions, feature = "strict-order"))]
    {
        for w in v.windows(2) {
            let (a, b) = (f(&w[0]), f(&w[1]));
            assert!(
                a != b,
                "{site}: sort key is NOT a total order — two elements produced {a:?}. \
                 `sort_unstable` then resolves them by input order, which for an ECS query is not \
                 stable across `App` instances. Widen the key, or use a canonical whole-value sort."
            );
        }
    }
    #[cfg(not(any(debug_assertions, feature = "strict-order")))]
    let _ = site;
}

/// Derive the per-source fracture seed from the asset's **path**.
///
/// **This used to hash the `AssetId`, and that was a real, expensive bug.**
///
/// ```ignore
/// fn seed_from(id: AssetId<WorldAsset>) -> u32 {
///     let mut h = DefaultHasher::new();
///     id.hash(&mut h);
///     h.finish() as u32
/// }
/// ```
///
/// An `AssetId` is a **slot index in the asset arena**, assigned by async load order — so the same
/// asset gets a different id run to run, hashes to a different seed, and `fracture` slices the body
/// along **completely different planes**. Measured before the fix: two same-seed builds produced
/// **23 of 23 fragments differing**, in `half_extents` as well as `center_local` — the
/// mesh was being partitioned differently, not merely rounded differently. Every downstream symptom
/// (chunk positions differing by ULPs, the load-dependence that made it look like a race) follows from
/// the fracture planes moving.
///
/// The old doc comment said "deterministic **within a run**", which was true and was the tell — nothing
/// compared two runs' bakes until it did.
///
/// The asset **path** is the stable identity: it is authored, not allocated, and identical across runs,
/// processes and machines. Hashed with a hand-rolled FNV-1a because `DefaultHasher` is not guaranteed
/// stable across toolchains, so it has no business seeding anything whose output is compared between
/// builds.
fn seed_from_path(path: &AssetPath) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in path.to_string().as_bytes() {
        h ^= u32::from(*b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// Turn one finished piece into cached mesh handles. Total, like [`geometry_from_piece`]: the
/// fragment array is index-parallel with the hierarchy, so a piece that draws nothing still occupies
/// its slot — with no meshes, and still a usable convex collider.
fn build_fragment(id: FragmentId, piece: crate::soup::Piece, meshes: &mut Assets<Mesh>) -> Fragment {
    let g = geometry_from_piece(id, piece);
    Fragment {
        id: g.id,
        outer_mesh: g.outer.map(|m| meshes.add(m)),
        cap_mesh: g.cap.map(|m| meshes.add(m)),
        cell: g.cell,
        center_local: g.center_local,
        half_extents: g.half_extents,
    }
}

/// Bake the pruned part into a single intact chunk (no fracture), keeping its own material.
/// `None` if its soup is empty or it had no material.
fn bake_detached(
    part: &Soup,
    material: Option<Handle<StandardMaterial>>,
    meshes: &mut Assets<Mesh>,
) -> Option<DetachedChunk> {
    let g = geometry_from_soup(part)?;
    let material = material?;
    let mesh = g.outer.or(g.cap).map(|m| meshes.add(m))?;
    Some(DetachedChunk { mesh, material, center_local: g.center_local, half_extents: g.half_extents })
}

/// Once a subject's whole scene has streamed in, bake its fracture set (and its detached chunk)
/// exactly once per source. Walk the subject's descendants, prune the [`DetachedPart`] subtree into a
/// separate chunk, merge the rest into one soup in subject-local space, and fracture. Self-gates on
/// all sub-meshes being present in `Assets<Mesh>`.
pub fn bake_fractures(
    mut cache: ResMut<FractureCache>,
    mut meshes: ResMut<Assets<Mesh>>,
    settings: Res<FractureSettings>,
    subjects: Query<(&FractureSubject, &FractureProxy, &Children)>,
    children_q: Query<&Children>,
    transforms: Query<&Transform>,
    mesh_q: Query<&Mesh3d>,
    mat_q: Query<&MeshMaterial3d<StandardMaterial>>,
    is_detached: Query<(), With<DetachedPart>>,
) {
    for (subject, proxy, children) in &subjects {
        let source = subject.0.id();
        if cache.baked.contains(&source) {
            continue;
        }
        // The fracture seed comes from the asset PATH, never the `AssetId` — see `seed_from_path`.
        // One path, no fallback: a handle with no asset path cannot be baked reproducibly, so it is
        // not baked at all, loudly.
        let Some(asset_path) = subject.0.path().map(|p| p.clone_owned()) else {
            error!(
                "autogib: a FractureSubject handle has no asset path — refusing to bake a fracture \
                 whose seed would depend on asset load order. No fragments for this source."
            );
            continue;
        };

        let mut body = Soup::default();
        let mut part = Soup::default();
        let mut part_material: Option<Handle<StandardMaterial>> = None;
        let mut all_loaded = true;

        // DFS stack of (entity, transform-relative-to-subject-root, inside-detached-subtree).
        let mut stack: Vec<(Entity, Mat4, bool)> = Vec::new();
        for child in children.iter() {
            let m = transforms.get(child).map(|t| t.to_matrix()).unwrap_or(Mat4::IDENTITY);
            stack.push((child, m, is_detached.get(child).is_ok()));
        }

        // Collect the meshes first, then append them in a CANONICAL order.
        //
        // Appending during the walk was a real determinism bug, and a subtle one because nothing
        // downstream hashed it directly. `Children` order for a glTF scene is the order the async
        // instantiation happened to add nodes, which is wall-clock dependent — so the vertex soup was
        // assembled in a different order between two same-seed runs. `fracture` then computes fragment
        // centroids as float sums over that soup, and float addition is not associative, so
        // `Fragment::center_local` came out a few ULPs apart. Every chunk spawns at
        // `origin + center_local * scale`, so the *positions* of an otherwise identical chunk set
        // diverged: same count, same order, coordinates off in the last few bits.
        //
        // The key is `(mesh asset PATH, world-matrix bits)`; the matrix disambiguates two entities that
        // share one mesh datablock at different transforms. Deliberately NOT the `Entity` id — id
        // allocation order is the instability being erased here.
        //
        // **It used to be the mesh's `AssetId`, and that was the same bug as `seed_from_path`'s, ninety
        // lines above.** The comment there even stated the assumption — "the asset id is stable across
        // same-seed runs (measured)" — and an `AssetId` is an *arena slot assigned by async load order
        // and slot recycling*, which is precisely what the seed was condemned for hashing. The
        // measurement behind that claim was taken idle; the residual only reproduced under heavy load.
        //
        // Note the tie check proves the key is *unique*, which is not the same as **stable** — a unique
        // key drawn from a load-order-dependent allocator still permutes the list. Uniqueness was never
        // the property this needed.
        //
        // A path is authored rather than allocated, so it is identical across runs, processes and
        // machines. glTF sub-meshes are path-backed (`enemy.glb#Mesh0/Primitive0`).
        let mut parts: Vec<(String, [u32; 16], Mat4, bool, Entity, Handle<Mesh>)> = Vec::new();
        let mut unpathed_mesh = false;
        while let Some((e, mat, in_part)) = stack.pop() {
            if let Ok(mesh3d) = mesh_q.get(e) {
                if meshes.get(&mesh3d.0).is_some() {
                    let mut bits = [0u32; 16];
                    for (i, v) in mat.to_cols_array().iter().enumerate() {
                        bits[i] = v.to_bits();
                    }
                    match mesh3d.0.path() {
                        Some(path) => {
                            parts.push((path.to_string(), bits, mat, in_part, e, mesh3d.0.clone()))
                        }
                        // One path, no fallback. Falling back to the `AssetId` for this one mesh would
                        // reintroduce exactly the instability this key exists to remove, on a subset of
                        // the soup — which is worse than not baking, because it would be intermittent.
                        None => unpathed_mesh = true,
                    }
                } else {
                    all_loaded = false; // sub-mesh still streaming
                }
            }
            if let Ok(ch) = children_q.get(e) {
                for child in ch.iter() {
                    let ct = transforms.get(child).map(|t| t.to_matrix()).unwrap_or(Mat4::IDENTITY);
                    let child_part = in_part || is_detached.get(child).is_ok();
                    stack.push((child, mat * ct, child_part));
                }
            }
        }
        if unpathed_mesh {
            error!(
                "autogib: a sub-mesh of {asset_path} has no asset path — refusing to assemble a vertex \
                 soup whose order would depend on asset load order. No fragments for this source."
            );
            continue;
        }
        sort_total_by_key_at(
            concat!(file!(), ":", line!()),
            &mut parts,
            |p: &(String, [u32; 16], Mat4, bool, Entity, Handle<Mesh>)| (p.0.clone(), p.1),
        );
        for (_, _, mat, in_part, e, mesh_handle) in parts {
            let Some(m) = meshes.get(&mesh_handle) else { continue };
            if in_part {
                append_mesh(&mut part, m, mat, false);
                if part_material.is_none() {
                    part_material = mat_q.get(e).ok().map(|mm| mm.0.clone());
                }
            } else {
                append_mesh(&mut body, m, mat, false);
            }
        }

        // Wait until the async scene has actually instantiated its body meshes AND they're loaded into
        // `Assets<Mesh>`. Before a glTF scene spawns its descendants there are simply no body `Mesh3d`
        // entities to find, so an empty body here means "still streaming", not "no geometry" — retry
        // next frame rather than caching an empty fracture set.
        //
        // **`part.is_empty()` is part of that gate, and leaving it out was a determinism bug.** When the
        // held item is a SEPARATE scene from the body, it streams on its own schedule. The
        // `DetachedPart` entity exists immediately, but until its scene instantiates it has no `Mesh3d`
        // descendants to find — so `all_loaded` stays true (nothing unloaded was *seen*) and `body` is
        // non-empty, and this baked a source with an EMPTY detached chunk and then marked it `baked`
        // **permanently**. Whether that race was won decided, for the whole run, whether a death flung
        // the weapon at all. If anything downstream is a fixed-size pool or a numbered sequence, losing
        // one chunk shifts every later one. Measured: 11 of 12 runs had the chunk and 1 did not.
        //
        // Empty part ⇒ "still streaming", never "this subject has no detached part". If a subject
        // without one is ever supported, this gate must learn to tell "absent" from "not yet", not be
        // relaxed back.
        if !all_loaded || body.is_empty() || part.is_empty() {
            continue;
        }

        let ext = body.extent();
        if ext <= 1.0e-5 {
            warn!("autogib: source body is degenerate (zero extent); marking baked with no fragments");
            cache.body.insert(source, Vec::new());
            cache.baked.insert(source);
            continue;
        }
        // **Refused, not substituted.** Synthesising a bounding box here would fracture a shape the
        // subject is not, and would do it silently. `baked` is left unset so a caller that adds the
        // component later still gets a bake.
        if proxy.0.is_empty() {
            error!("autogib: {asset_path} has no FractureProxy cells; refusing to bake");
            continue;
        }

        // Bounding-box-driven sizing: bigger/denser meshes yield more, appropriately-sized pieces.
        let ref_ext = settings.ref_extent.max(1.0e-4);
        let raw = (settings.pieces_base as f32 * (ext / ref_ext)).round() as i32;
        let target = raw.clamp(settings.min_pieces, settings.max_pieces).max(1) as usize;

        // **The bake runs here, on the main thread, and `AG-011` settled that by measuring rather than
        // arguing.** The torso-and-head fixture at its finest 12 fragments measures **~2.2 ms**
        // (release, `cargo run --release --example fracture_cube`), up from ~1.4 ms before the bake
        // kept its hierarchy — the ratio tracks node count, 23 nodes built instead of 12, which is
        // what keeping every piece the loop split costs. The ticket's own threshold was "a fix is
        // warranted at 50 ms and not at 5 ms", so this stays well the safe side of it.
        //
        // The figure recorded here was **0.33 ms**, and re-measuring found that stale on this
        // machine even before the change: the pre-hierarchy code measures ~1.4 ms today. Both
        // numbers are below the threshold, so the conclusion is unchanged — but the old one was
        // being quoted as if it had been re-checked, and it had not.
        //
        // Recording the alternative so nobody re-derives it: moving this to `AsyncComputeTaskPool`
        // would need `bevy/multi_threaded`, which this crate deliberately does not declare. Without it
        // that pool is single-threaded and `spawn` runs the work inline anyway — so the "async" bake
        // would be async only in builds where some *other* crate happened to turn the feature on, via
        // feature unification. One code path that is concurrent in some consumers' builds and not
        // others is exactly the ambiguity `CLAUDE.md`'s one-path rule exists to prevent, and buying it
        // for 0.33 ms would be a bad trade twice over.
        let (pieces, tree) =
            fracture(body, &proxy.0, &settings.cut_for(target, seed_from_path(&asset_path)));
        let graph = crate::mesh::bond_graph(&pieces, &tree);
        let frags: Vec<Fragment> = pieces
            .into_iter()
            .enumerate()
            .map(|(i, piece)| build_fragment(FragmentId(i as u32), piece, &mut meshes))
            .collect();
        info!(
            "autogib: baked {} fragments for {asset_path} ({} in the finest frontier, {} cuts, \
             {} bonds)",
            frags.len(),
            tree.leaves().len(),
            tree.cuts(),
            graph.len()
        );
        cache.body.insert(source, frags);
        cache.trees.insert(source, tree);
        cache.graphs.insert(source, graph);

        // The detached chunk (single intact piece, keeps its own material).
        if let Some(chunk) = bake_detached(&part, part_material, &mut meshes) {
            cache.detached.insert(source, chunk);
        }

        cache.baked.insert(source);
    }
}

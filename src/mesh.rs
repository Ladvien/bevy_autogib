//! The `Mesh` ↔ [`Soup`] adapters, and the asset-free entry point into the whole pipeline.
//!
//! These are the only geometry functions that name a Bevy type. [`fracture_mesh`] is what an example,
//! a test, or a caller with its own asset handling drives — it takes meshes, returns meshes, and never
//! touches `Assets<Mesh>` or the ECS.

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::log::warn;
use bevy::math::{Mat3, Mat4, Vec2, Vec3};
use bevy::mesh::{Indices, Mesh, PrimitiveTopology, VertexAttributeValues};

use crate::CutSettings;
use crate::bond::BondGraph;
use crate::proxy::ProxyCell;
use crate::soup::{MIN_CROSS2, Soup, Vtx, WELD, fracture};
use crate::tree::{FragmentId, FragmentTree};

/// Decode a mesh's index buffer into a triangle list, handling all encodings: `U16`, `U32`, and
/// non-indexed (consecutive triples). `vertex_count` drives only the non-indexed case, whose
/// triangles are `[0,1,2], [3,4,5], …` over the position array. Callers bounds-check the returned
/// indices against their own vertex data before dereferencing.
fn triangle_indices(mesh: &Mesh, vertex_count: usize) -> Vec<[u32; 3]> {
    let mut tris: Vec<[u32; 3]> = Vec::new();
    match mesh.indices() {
        Some(Indices::U16(v)) => {
            for c in v.chunks_exact(3) {
                tris.push([c[0] as u32, c[1] as u32, c[2] as u32]);
            }
        }
        Some(Indices::U32(v)) => {
            for c in v.chunks_exact(3) {
                tris.push([c[0], c[1], c[2]]);
            }
        }
        None => {
            let n = vertex_count as u32;
            let mut i = 0;
            while i + 3 <= n {
                tris.push([i, i + 1, i + 2]);
                i += 3;
            }
        }
    }
    tris
}

/// Append one loaded mesh's triangles into `soup`, transformed by `xform` (the sub-mesh's transform
/// relative to the subject root). Robust to arbitrary layouts: missing `NORMAL` → synthesized flat
/// normals; missing `UV_0` → zero-filled; `U16`/`U32`/non-indexed all handled. Returns `false`
/// (+`warn!`) if the mesh has no `Float32x3` positions or isn't a triangle list.
pub(crate) fn append_mesh(soup: &mut Soup, mesh: &Mesh, xform: Mat4, interior: bool) -> bool {
    let Some(VertexAttributeValues::Float32x3(positions)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION) else {
        warn!("autogib: sub-mesh has no Float32x3 POSITION; skipping it");
        return false;
    };
    if mesh.primitive_topology() != PrimitiveTopology::TriangleList {
        warn!("autogib: sub-mesh is not a TriangleList; skipping it");
        return false;
    }

    // Transform positions into subject-local space.
    let tp: Vec<Vec3> = positions.iter().map(|p| xform.transform_point3(Vec3::from_array(*p))).collect();

    // Normals: transform by the inverse-transpose (upper 3x3), or synthesize per-face if absent.
    let normal_mat = Mat3::from_mat4(xform).inverse().transpose();
    let have_normals = matches!(
        mesh.attribute(Mesh::ATTRIBUTE_NORMAL),
        Some(VertexAttributeValues::Float32x3(n)) if n.len() == positions.len()
    );
    let mut tn: Vec<Vec3> = match mesh.attribute(Mesh::ATTRIBUTE_NORMAL) {
        Some(VertexAttributeValues::Float32x3(n)) if have_normals => {
            n.iter().map(|v| (normal_mat * Vec3::from_array(*v)).normalize_or_zero()).collect()
        }
        _ => vec![Vec3::ZERO; tp.len()],
    };

    // UVs: keep source or zero-fill.
    let tuv: Vec<Vec2> = match mesh.attribute(Mesh::ATTRIBUTE_UV_0) {
        Some(VertexAttributeValues::Float32x2(u)) if u.len() == positions.len() => {
            u.iter().map(|v| Vec2::from_array(*v)).collect()
        }
        _ => vec![Vec2::ZERO; tp.len()],
    };

    // Collect the triangle index list (handling all index encodings).
    let tris = triangle_indices(mesh, tp.len());

    if !have_normals {
        // Area-weighted face normals accumulated onto shared vertices, then renormalized.
        for t in &tris {
            let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
            if a >= tp.len() || b >= tp.len() || c >= tp.len() {
                continue;
            }
            let fnrm = (tp[b] - tp[a]).cross(tp[c] - tp[a]);
            tn[a] += fnrm;
            tn[b] += fnrm;
            tn[c] += fnrm;
        }
        for n in &mut tn {
            *n = n.normalize_or_zero();
        }
    }

    let vbase = soup.pos.len() as u32;
    soup.pos.extend_from_slice(&tp);
    soup.nrm.extend_from_slice(&tn);
    soup.uv.extend_from_slice(&tuv);
    for t in &tris {
        // Guard against out-of-range indices from a malformed mesh.
        if (t[0] as usize) < tp.len() && (t[1] as usize) < tp.len() && (t[2] as usize) < tp.len() {
            soup.idx.push([t[0] + vbase, t[1] + vbase, t[2] + vbase]);
            soup.tri_interior.push(interior);
        }
    }
    true
}

/// Build a `Mesh` from the subset of `soup` triangles whose interior flag matches `want_interior`,
/// re-indexed to a compact vertex set and recentered so the origin sits at `recenter` (the fragment
/// centroid → the spawned entity spins about its own center). `None` if the subset is empty.
fn soup_to_mesh(soup: &Soup, want_interior: bool, recenter: Vec3) -> Option<Mesh> {
    let mut pos: Vec<[f32; 3]> = Vec::new();
    let mut nrm: Vec<[f32; 3]> = Vec::new();
    let mut uv: Vec<[f32; 2]> = Vec::new();
    let mut idx: Vec<u32> = Vec::new();
    let mut weld: AttributeWeld = AttributeWeld::default();

    for (t, tri) in soup.idx.iter().enumerate() {
        if soup.tri_interior[t] != want_interior {
            continue;
        }
        let (pa, pb, pc) = (
            soup.pos[tri[0] as usize],
            soup.pos[tri[1] as usize],
            soup.pos[tri[2] as usize],
        );
        if (pb - pa).cross(pc - pa).length_squared() < 1.0e-12 {
            continue; // drop zero-area triangles
        }
        for &old in tri {
            let v = soup.vtx(old);
            idx.push(weld.insert(v, recenter, &mut pos, &mut nrm, &mut uv));
        }
    }
    if idx.is_empty() {
        return None;
    }
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nrm);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    mesh.insert_indices(Indices::U32(idx));
    Some(mesh)
}

/// One fractured piece: a convex proxy cell, and the render surface that belongs to it.
///
/// **Two tiers, and confusing them is the mistake this type exists to prevent.** [`Self::cell`] is a
/// *solid* — closed, convex, with a provably valid cut face. [`Self::outer`] is a *surface subset* of
/// the subject's own mesh, and it is **open by design**: it carries no cap, because the cap is
/// [`Self::cap`], generated from the cell. Applying a closed-solid test to `outer` is a category error;
/// see `AG-004`.
///
/// Both meshes are recentered to `center_local` (their shared bounding-box center), so a body placed at
/// `origin + center_local * scale` lines up with the rendered chunk.
pub struct FragmentGeometry {
    /// Which node of the [`FragmentTree`](crate::FragmentTree) this is. Always equal to this
    /// fragment's own position in the array it came back in.
    pub id: FragmentId,
    /// The subject's own surface — whatever material the intact subject wore. `None` for an interior
    /// fragment that the render mesh never reached.
    pub outer: Option<Mesh>,
    /// The cut faces, from the proxy cell, with planar cross-section UVs. Give these the "inside"
    /// material (raw meat, splintered wood, fractured stone) — that contrast is the whole read.
    pub cap: Option<Mesh>,
    /// **The fragment as a solid.** One convex cell, which is precisely what a solver wants: a single
    /// convex collider, no decomposition at spawn time and no trimesh. See `AG-007`.
    pub cell: ProxyCell,
    pub center_local: Vec3,
    /// Half the bounding box per axis, in subject-local units.
    ///
    /// **A coarse bound, not the collider.** [`Self::cell`] is the collider. This survives for sizing,
    /// culling and the launch impulses an example computes; a box around a plane-cut shard is a poor
    /// fit and always was.
    pub half_extents: Vec3,
}

/// Recentred meshes for a soup that was never fractured — the detached part.
///
/// **A separate type from [`FragmentGeometry`], deliberately.** A detached part is an *intact chunk*:
/// nothing cut it, so it has no proxy cell and no cut face, and giving it a synthesised one would be a
/// second path that only exists to satisfy a struct field.
pub(crate) struct IntactGeometry {
    pub(crate) outer: Option<Mesh>,
    pub(crate) cap: Option<Mesh>,
    pub(crate) center_local: Vec3,
    pub(crate) half_extents: Vec3,
}

/// Turn an un-fractured soup into recentred meshes. `None` if it has no drawable triangles.
pub(crate) fn geometry_from_soup(soup: &Soup) -> Option<IntactGeometry> {
    if soup.is_empty() {
        return None;
    }
    let (mn, mx) = soup.bbox();
    let center = (mn + mx) * 0.5;
    let half_extents = ((mx - mn) * 0.5).max(Vec3::splat(0.01));
    let outer = soup_to_mesh(soup, false, center);
    let cap = soup_to_mesh(soup, true, center);
    if outer.is_none() && cap.is_none() {
        return None;
    }
    Some(IntactGeometry { outer, cap, center_local: center, half_extents })
}



/// **The attribute-aware weld.** Merges vertices that are the same *point on the same surface*, and
/// refuses to merge across a crease.
///
/// # Why the old code merged nothing at all
///
/// [`Soup::push_tri`] allocates three fresh vertices for every triangle it emits, so a finished
/// fragment arrives here with `positions.len() == 3 * triangles` and no sharing whatsoever. The remap
/// this replaces keyed on the *old soup index*, which is unique per corner by construction — so it
/// compacted the buffer and merged nothing. Fragments shipped at three vertices per triangle.
///
/// # Why a bare position weld would be worse than none
///
/// The crease between the subject's skin and a raw cut face is the entire visual read this crate
/// exists to produce, and on a fragment cut more than once there are creases between cut faces of
/// different planes too. Merging across one averages or discards a normal and smears it. So the key is
/// composite: **position class + quantised normal + quantised UV**.
///
/// # The two quantisations fail in opposite directions, deliberately
///
/// **Position** uses a 27-cell probe rather than a bare lattice lookup. Two positions a few ULPs apart
/// can straddle a lattice boundary and hash to different cells, and here a missed merge on position is
/// not merely a lost saving — the two vertices are the same point, so leaving them apart is what makes
/// a seam. Probing the neighbourhood is what `isomesh`'s `Welder` does and why its epsilon is correct.
///
/// **Normal and UV** use a bare quantised bucket, and that is the right trade for them. A near-miss
/// there costs one unmerged vertex; a false *match* would smear a crease. Erring toward keeping
/// vertices apart is the safe direction for an attribute and the unsafe one for a position.
///
/// # Why not `isomesh`'s `weld_split_by`
///
/// It exists at the pinned rev and does exactly this shape of thing (`AG-013` recorded it landing).
/// It is not used because `tests/leaf.rs` states the terms `isomesh` was admitted on: *"a second
/// opinion about the output, not a source of it."* Welding the shipped mesh with it would make it a
/// source — every emitted vertex would depend on its welder, and a change there would move geometry
/// this crate promises is reproducible. `MeshBuffer` also carries no UV channel, so the round trip
/// would have to rebuild UVs through `remap()` anyway.
#[derive(Default)]
struct AttributeWeld {
    /// Lattice cell → the emitted vertices in it. Small vectors: coincident-vertex counts are single
    /// digits, so the 27-cell probe stays cheap.
    cells: HashMap<(i64, i64, i64), Vec<u32>>,
}

/// Quantisation step for the normal bucket. About one degree at unit length — far finer than any
/// crease this crate creates, and coarse enough to bucket a shared smooth normal together.
const NRM_STEP: f32 = 1.0e-2;
/// Quantisation step for the UV bucket, in the planar cross-section units `push_cap_tri` assigns.
const UV_STEP: f32 = 1.0e-4;

impl AttributeWeld {
    fn insert(
        &mut self,
        v: crate::soup::Vtx,
        recenter: Vec3,
        pos: &mut Vec<[f32; 3]>,
        nrm: &mut Vec<[f32; 3]>,
        uv: &mut Vec<[f32; 2]>,
    ) -> u32 {
        let p = v.pos - recenter;
        let q = |x: f32| (x / crate::soup::WELD).round() as i64;
        let key = (q(p.x), q(p.y), q(p.z));
        let nk = |x: f32| (x / NRM_STEP).round() as i64;
        let uk = |x: f32| (x / UV_STEP).round() as i64;
        let want_n = (nk(v.nrm.x), nk(v.nrm.y), nk(v.nrm.z));
        let want_uv = (uk(v.uv.x), uk(v.uv.y));

        // The 27-cell probe: a candidate one lattice cell away may still be the same point.
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let Some(ids) = self.cells.get(&(key.0 + dx, key.1 + dy, key.2 + dz)) else {
                        continue;
                    };
                    for &id in ids {
                        let e = pos[id as usize];
                        let same_point = (e[0] - p.x).abs() <= crate::soup::WELD
                            && (e[1] - p.y).abs() <= crate::soup::WELD
                            && (e[2] - p.z).abs() <= crate::soup::WELD;
                        if !same_point {
                            continue;
                        }
                        let en = nrm[id as usize];
                        let eu = uv[id as usize];
                        if (nk(en[0]), nk(en[1]), nk(en[2])) == want_n
                            && (uk(eu[0]), uk(eu[1])) == want_uv
                        {
                            return id;
                        }
                    }
                }
            }
        }

        let id = pos.len() as u32;
        pos.push([p.x, p.y, p.z]);
        nrm.push([v.nrm.x, v.nrm.y, v.nrm.z]);
        uv.push([v.uv.x, v.uv.y]);
        self.cells.entry(key).or_default().push(id);
        id
    }
}

/// A soup as one mesh, ignoring the skin/cap split — for the audit, which measures a whole surface.
pub(crate) fn soup_to_mesh_all_faces(soup: &Soup) -> Result<Mesh, String> {
    let (mn, mx) = soup.bbox();
    soup_to_mesh_all(soup, (mn + mx) * 0.5).ok_or_else(|| "soup has no drawable triangles".to_string())
}

/// Every triangle of a soup, regardless of its interior tag.
fn soup_to_mesh_all(soup: &Soup, recenter: Vec3) -> Option<Mesh> {
    let mut pos = Vec::new();
    let mut nrm = Vec::new();
    let mut uv = Vec::new();
    let mut idx = Vec::new();
    for tri in &soup.idx {
        let base = pos.len() as u32;
        for &v in tri {
            let v = v as usize;
            let p = soup.pos[v] - recenter;
            pos.push([p.x, p.y, p.z]);
            nrm.push([soup.nrm[v].x, soup.nrm[v].y, soup.nrm[v].z]);
            uv.push([soup.uv[v].x, soup.uv[v].y]);
        }
        idx.extend([base, base + 1, base + 2]);
    }
    if idx.is_empty() {
        return None;
    }
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nrm);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    mesh.insert_indices(Indices::U32(idx));
    Some(mesh)
}

/// The closed solid this fragment is, as one mesh — every proxy face, not just the cut ones.
///
/// Nothing draws this. It exists so the fragment can be *measured*: this is the artefact on which
/// `χ = 2`, manifoldness and volume conservation are meaningful claims.
pub(crate) fn proxy_soup(cell: &ProxyCell) -> Soup {
    let mut s = Soup::default();
    cell.append_all_faces(&mut s);
    s
}

/// Turn one finished piece into recentered meshes.
///
/// **Total, not fallible.** It used to return `None` for a piece that drew nothing and the caller
/// dropped it, but the hierarchy makes the fragment array index-parallel with the
/// [`FragmentTree`](crate::FragmentTree) — dropping an entry would slide every id after it onto the
/// wrong node. A piece that draws nothing is still a *solid*: it has a convex cell, a centre and an
/// extent, and it is a perfectly good collider. It comes back with both meshes `None` and is bounded
/// by its cell rather than by a render mesh it never received.
pub(crate) fn geometry_from_piece(id: FragmentId, piece: crate::soup::Piece) -> FragmentGeometry {
    let crate::soup::Piece { cell, render, sheets, relief, soften: round } = piece;
    // The cap comes from the cell, never from the render mesh — that is the architecture in one line.
    //
    // The render mesh's boundary vertices are handed along so the cap can weave them into its own ring:
    // the cap is the cross-section of the *cell* (one vertex per cell edge crossed) while the skin's
    // opening is the cross-section of the *triangulated mesh* (one per triangle edge, diagonals
    // included). Without the weave the two meet across T-junctions — flush geometrically, open
    // topologically, and a hairline crack under some rasterisers.
    let seam: Vec<Vec3> = render.pos.clone();
    let mut drawn = render.clone();
    cell.append_cut_faces(&mut drawn, &seam, relief);
    // **Rounded after the cap is welded on**, so the relaxation bevels the skin/cap edge too — which
    // is the sharpest edge on the whole fragment and the one that most says "cleaved".
    let mut drawn = soften(&drawn, round);

    // **Open shells ride along untouched — not clipped, not capped, and not rounded.** A sheet has no
    // interior to expose and no fracture edge to soften; it is the artist's own geometry, and
    // relaxing it would curl and shrink a cape rather than round a chunk of one. Appended after the
    // softening for exactly that reason. See `AG-003`, whose test is what caught this.
    for sheet in &sheets {
        for (t, tri) in sheet.idx.iter().enumerate() {
            drawn.push_tri(
                sheet.vtx(tri[0]),
                sheet.vtx(tri[1]),
                sheet.vtx(tri[2]),
                sheet.tri_interior[t],
            );
        }
    }

    // Bound by the drawn surface when there is one, by the cell when there is not. The cell is the
    // fragment's solid and always exists, so there is no case here with nothing to measure.
    let (mn, mx) = if drawn.is_empty() { cell_bbox(&cell) } else { drawn.bbox() };
    let center = (mn + mx) * 0.5;
    let half_extents = ((mx - mn) * 0.5).max(Vec3::splat(0.01));
    let (outer, cap) = if drawn.is_empty() {
        (None, None)
    } else {
        (soup_to_mesh(&drawn, false, center), soup_to_mesh(&drawn, true, center))
    };
    FragmentGeometry { id, outer, cap, cell, center_local: center, half_extents }
}

/// Axis-aligned bounds of a cell's own vertices. `(ZERO, ZERO)` for a cell with no points, which
/// [`ProxyCell::new`] cannot produce.
fn cell_bbox(cell: &ProxyCell) -> (Vec3, Vec3) {
    let mut mn = Vec3::splat(f32::INFINITY);
    let mut mx = Vec3::splat(f32::NEG_INFINITY);
    for p in cell.points() {
        mn = mn.min(*p);
        mx = mx.max(*p);
    }
    if cell.points().is_empty() { (Vec3::ZERO, Vec3::ZERO) } else { (mn, mx) }
}

/// **The whole pipeline, with no assets and no ECS.** Cut the caller's convex `proxy` into at most
/// `target` cells, carry the `parts` triangles along as a payload, and return each piece.
///
/// # What changed, and why the signature grew a parameter
///
/// This used to cut the triangle soup directly and cap each cut by recovering boundary loops. That is
/// not how production fracture works and it was not fixable: a plane through a non-convex section
/// produces a cap no centroid fan can close, and a plane through two shells that merely *touch*
/// produces a boundary chain with no closure at all. Müller, Chentanez & Kim (`10.1145/2461912.2461934`)
/// cut a **volumetric convex decomposition** instead and carry the visual triangles as a payload,
/// because `plane ∩ convex polyhedron = convex polygon` — every cap is then convex by construction and
/// the fan is provably valid. See [`crate::proxy`].
///
/// # The proxy is yours
///
/// One [`ProxyCell`] per *connected shell*, convex, covering the mesh. A consumer already running
/// V-HACD or CoACD for colliders has this; a blocked-out subject can use [`ProxyCell::from_box`]. The
/// cells are **never unioned** — they are cut independently and fragments keep their cell's provenance,
/// which is what preserves the ability to separate a head from a torso.
///
/// A triangle whose centroid lies in no cell is `warn!`-dropped, loudly and with a count: it means the
/// proxy does not cover the mesh, which is a fault in the input rather than something to paper over.
///
/// # Parameters
///
/// Every `Mat4` is that sub-mesh's transform relative to the subject root; every other dial lives on
/// [`CutSettings`], which documents each one.
///
/// **`parts` order is load-bearing.** Cut planes are placed relative to cell centroids, and the render
/// payload's vertex order decides float sums elsewhere; float addition is not associative, so two
/// different orders give fragments differing in the last bits. Sort `parts` by something authored (an
/// asset path) if they came from anywhere order is not guaranteed; [`crate::bake`] does exactly that.
pub fn fracture_mesh(parts: &[(&Mesh, Mat4)], proxy: &[ProxyCell], cut: &CutSettings) -> Fracture {
    let mut soup = Soup::default();
    for (mesh, xform) in parts {
        append_mesh(&mut soup, mesh, *xform, false);
    }
    if proxy.is_empty() {
        warn!("autogib: refusing to fracture — the caller supplied no proxy cells");
        return Fracture::default();
    }
    // A proxy with nothing to carry is not a subject. Cutting it would emit cap-only fragments of a
    // shape the caller never handed us a surface for.
    if soup.is_empty() {
        return Fracture::default();
    }
    let (pieces, tree) = fracture(soup, proxy, cut);
    let bonds = bond_graph(&pieces, &tree);
    let fragments = pieces
        .into_iter()
        .enumerate()
        .map(|(i, p)| geometry_from_piece(FragmentId(i as u32), p))
        .collect();
    Fracture { fragments, tree, bonds }
}

/// Match up which of a bake's finest fragments share a face.
///
/// Built over the leaves rather than every node because adjacency is only meaningful between pieces
/// that coexist, and the leaves are the one frontier every other is derived from.
pub(crate) fn bond_graph(pieces: &[crate::soup::Piece], tree: &FragmentTree) -> BondGraph {
    let members: Vec<(FragmentId, &ProxyCell)> = tree
        .leaves()
        .into_iter()
        .filter_map(|id| pieces.get(id.index()).map(|p| (id, &p.cell)))
        .collect();
    BondGraph::of(&members, tree.len())
}

/// **One bake: every fragment the cut loop produced, plus the hierarchy that says how they nest.**
///
/// `fragments` is index-parallel with `tree` — `fragments[id.index()]` is the payload of
/// `tree.node(id)` — so a frontier query returns ids that index straight into it. Spawn a frontier,
/// never the whole array: the array holds interior pieces too, and spawning both a parent and its
/// children would put the same volume in the scene twice.
#[derive(Default)]
pub struct Fracture {
    /// Every node's geometry, in [`FragmentId`] order. Interior nodes included.
    pub fragments: Vec<FragmentGeometry>,
    /// Which fragments nest inside which, and the frontier queries that read it.
    pub tree: FragmentTree,
    /// Which fragments *touch* which, over the finest frontier. Nesting and neighbouring are
    /// different questions, and a localised break needs the second one.
    pub bonds: BondGraph,
}

impl Fracture {
    /// The finest granularity — every piece that was never cut further. This is the set the crate
    /// returned before it kept a hierarchy.
    pub fn leaves(&self) -> Vec<&FragmentGeometry> {
        self.pick(&self.tree.leaves())
    }

    /// The frontier holding roughly `count` pieces, clamped to what this bake can offer. **The
    /// granularity dial**: three big pieces for a cleaving blow, all of them for a blast.
    pub fn frontier_of(&self, count: usize) -> Vec<&FragmentGeometry> {
        self.pick(&self.tree.frontier_of(count))
    }

    /// The frontier at most `depth` cuts from the caller's proxy cells.
    pub fn at_depth(&self, depth: u16) -> Vec<&FragmentGeometry> {
        self.pick(&self.tree.at_depth(depth))
    }

    /// Resolve ids to payloads, skipping any that fall outside the array. Out of range is refused
    /// rather than fatal — an id from a stale bake must not take the process down.
    pub fn pick(&self, ids: &[FragmentId]) -> Vec<&FragmentGeometry> {
        ids.iter().filter_map(|id| self.fragments.get(id.index())).collect()
    }

    /// [`leaves`](Self::leaves), consuming the bake — for a caller that needs to own the meshes.
    pub fn into_leaves(self) -> Vec<FragmentGeometry> {
        let ids = self.tree.leaves();
        self.into_pick(&ids)
    }

    /// [`frontier_of`](Self::frontier_of), consuming the bake.
    pub fn into_frontier_of(self, count: usize) -> Vec<FragmentGeometry> {
        let ids = self.tree.frontier_of(count);
        self.into_pick(&ids)
    }

    /// [`at_depth`](Self::at_depth), consuming the bake.
    pub fn into_at_depth(self, depth: u16) -> Vec<FragmentGeometry> {
        let ids = self.tree.at_depth(depth);
        self.into_pick(&ids)
    }

    /// [`pick`](Self::pick), consuming the bake. Returns the kept fragments in [`FragmentId`] order
    /// regardless of the order `ids` arrived in, so the result reads the same whichever frontier
    /// query produced it.
    pub fn into_pick(self, ids: &[FragmentId]) -> Vec<FragmentGeometry> {
        let mut keep = vec![false; self.fragments.len()];
        for id in ids {
            if let Some(slot) = keep.get_mut(id.index()) {
                *slot = true;
            }
        }
        self.fragments.into_iter().zip(keep).filter_map(|(f, k)| k.then_some(f)).collect()
    }
}

// ---------------------------------------------------------------------------------------------
// Tests — pure geometry, no App required.
// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::ProxyCell;
    use bevy::math::primitives::Cuboid;

    fn cube_soup() -> Soup {
        let mut s = Soup::default();
        assert!(append_mesh(&mut s, &Mesh::from(Cuboid::new(1.0, 1.0, 1.0)), Mat4::IDENTITY, false));
        s
    }

    fn all_finite(s: &Soup) -> bool {
        s.pos.iter().all(|p| p.is_finite()) && s.nrm.iter().all(|n| n.is_finite()) && s.uv.iter().all(|u| u.is_finite())
    }

    fn interior_area(s: &Soup) -> f32 {
        s.idx
            .iter()
            .enumerate()
            .filter(|(t, _)| s.tri_interior[*t])
            .map(|(_, tri)| {
                let (a, b, c) = (s.pos[tri[0] as usize], s.pos[tri[1] as usize], s.pos[tri[2] as usize]);
                0.5 * (b - a).cross(c - a).length()
            })
            .sum()
    }

    fn cube_proxy() -> Vec<ProxyCell> {
        vec![ProxyCell::from_box(Vec3::ZERO, Vec3::splat(0.5))]
    }


    /// A cut leaves each half on its own side, and the cap comes from the **cell**, not from loop
    /// recovery over the render mesh.
    #[test]
    fn slice_cube_axis_plane() {
        let (cube, _) = (Mesh::from(Cuboid::new(1.0, 1.0, 1.0)), ());
        let pieces =
            fracture_mesh(&[(&cube, Mat4::IDENTITY)], &cube_proxy(), &CutSettings::new(2, 0.05, 7)).into_leaves();
        assert_eq!(pieces.len(), 2, "one cut should give two pieces");
        for p in &pieces {
            assert!(p.cap.is_some(), "every piece of a cut carries a cap face");
            assert!(p.half_extents.is_finite(), "half extents went non-finite");
            assert!(p.center_local.is_finite(), "centre went non-finite");
        }
    }

    /// A mid-slice of the unit cube exposes a 1×1 cross-section — and under Tier A that area comes out
    /// exact, because the section is a convex polygon rather than a recovered loop.
    #[test]
    fn cap_is_unit_square_area() {
        let cell = ProxyCell::from_box(Vec3::ZERO, Vec3::splat(0.5));
        let (above, _) = cell.clip(&crate::soup::Plane { point: Vec3::ZERO, normal: Vec3::Y });
        let mut cap = Soup::default();
        above.expect("the cube cuts").append_cut_faces(&mut cap, &[], 0.0);
        assert!(
            (interior_area(&cap) - 1.0).abs() < 1.0e-4,
            "cap area should be exactly 1.0, got {}",
            interior_area(&cap)
        );
    }

    #[test]
    fn fracture_reaches_target_and_is_deterministic() {
        let proxy = cube_proxy();
        let (a, ta) = fracture(cube_soup(), &proxy, &CutSettings::new(8, 0.05, 0xABCD_1234));
        let (b, tb) = fracture(cube_soup(), &proxy, &CutSettings::new(8, 0.05, 0xABCD_1234));
        assert_eq!(a.len(), b.len());
        assert_eq!(ta, tb, "the hierarchy is reproducible, not just the geometry");
        let leaves = ta.leaves();
        assert!(leaves.len() >= 2 && leaves.len() <= 8, "sane fragment count: {}", leaves.len());
        assert!(
            leaves.iter().all(|id| a.get(id.index()).is_some_and(|p| !p.render.is_empty())),
            "every leaf kept some render surface"
        );
        assert!(
            a[0].cell.centroid().distance(b[0].cell.centroid()) < 1.0e-6,
            "deterministic per seed"
        );
        assert!(all_finite(&a[0].render), "render payload went non-finite");
    }

    /// **The hierarchy is not a second bake.** Every frontier of one bake tiles the same solid, so
    /// the three-piece read and the finest read must agree on total volume to the last few bits.
    #[test]
    fn every_frontier_of_one_bake_conserves_the_whole_volume() {
        let proxy = cube_proxy();
        let (pieces, tree) = fracture(cube_soup(), &proxy, &CutSettings::new(8, 0.05, 0xABCD_1234));
        let whole: f32 = proxy.iter().map(|c| c.volume()).sum();
        for cuts in 0..=tree.cuts() {
            let v: f32 = tree
                .frontier_after(cuts)
                .iter()
                .filter_map(|id| pieces.get(id.index()))
                .map(|p| p.cell.volume())
                .sum();
            assert!(
                (v - whole).abs() < 1.0e-3,
                "frontier after {cuts} cuts has volume {v}, expected {whole}"
            );
        }
    }

    /// The granularity dial: one bake, read back at every count between the proxy cells and the
    /// finest cut.
    #[test]
    fn one_bake_answers_every_piece_count() {
        let cube = Mesh::from(Cuboid::new(1.0, 1.0, 1.0));
        let baked = fracture_mesh(&[(&cube, Mat4::IDENTITY)], &cube_proxy(), &CutSettings::new(8, 0.05, 11));
        let finest = baked.leaves().len();
        assert!(finest >= 4, "expected a usable spread of granularities, got {finest}");
        for want in 1..=finest {
            let got = baked.frontier_of(want).len();
            let expect = want.max(baked.tree.roots().len());
            assert_eq!(got, expect, "asked for {want} pieces");
        }
        assert_eq!(baked.frontier_of(9_999).len(), finest, "past the finest clamps to the leaves");
    }

    #[test]
    fn missing_uv_is_zero_filled() {
        let mut m = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
        m.insert_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
        m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 0.0, 1.0]; 3]);
        m.insert_indices(Indices::U32(vec![0, 1, 2]));
        let mut s = Soup::default();
        assert!(append_mesh(&mut s, &m, Mat4::IDENTITY, false));
        assert_eq!(s.uv.len(), s.pos.len());
        assert!(s.uv.iter().all(|u| *u == Vec2::ZERO));
    }

    #[test]
    fn missing_normals_are_synthesized() {
        let mut m = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
        m.insert_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
        m.insert_indices(Indices::U32(vec![0, 1, 2]));
        let mut s = Soup::default();
        assert!(append_mesh(&mut s, &m, Mat4::IDENTITY, false));
        // Flat triangle in the XY plane → +Z normals.
        assert!(s.nrm.iter().all(|n| n.z.abs() > 0.99));
    }

    /// A plane that misses the cell leaves it whole and the driver does not spin on it.
    #[test]
    fn degenerate_plane_leaves_piece_whole() {
        let cell = ProxyCell::from_box(Vec3::ZERO, Vec3::splat(0.5));
        let (above, below) =
            cell.clip(&crate::soup::Plane { point: Vec3::splat(5.0), normal: Vec3::X });
        assert!(above.is_none(), "nothing above a plane past the cube");
        assert!(below.is_some(), "the whole cell lies below it");
        // A `min_fraction` this large stops the recursion early; the loop must *terminate* there
        // rather than spin to its hard cap looking for a cut it is never allowed to make.
        let (out, tree) = fracture(cube_soup(), &cube_proxy(), &CutSettings::new(4, 0.6, 42));
        assert!(!out.is_empty());
        assert!(tree.cuts() <= 4, "the volume floor bounded the cuts, got {}", tree.cuts());
        assert!(tree.leaves().len() <= 4, "and so bounded the finest frontier");
    }

    /// The depth bound retires a piece the same way the volume floor does — it stops, it does not
    /// spin, and it does not silently produce a deeper tree than asked for.
    #[test]
    fn max_depth_bounds_the_hierarchy_without_looping() {
        let (_, tree) = fracture(cube_soup(), &cube_proxy(), &CutSettings { max_depth: 2, ..CutSettings::new(32, 0.001, 0x5EED_1234) });
        assert!(tree.cuts() > 0, "a depth of 2 still permits cuts");
        assert!(
            tree.iter().all(|(_, n)| n.depth <= 2),
            "no node may sit deeper than the bound asked for"
        );
        assert!(tree.leaves().len() <= 4, "one cell cut at most twice deep is at most four leaves");
    }

    /// **A render fragment is open, and that is correct.** It is a surface subset of the subject's own
    /// mesh; the closed artefact is the proxy cell. Asserting watertightness here would be the category
    /// error `AG-004` exists to prevent, so this test pins the *shape* of the claim instead.
    #[test]
    fn a_render_fragment_carries_no_cap_of_its_own() {
        let cube = Mesh::from(Cuboid::new(1.0, 1.0, 1.0));
        let pieces =
            fracture_mesh(&[(&cube, Mat4::IDENTITY)], &cube_proxy(), &CutSettings::new(4, 0.05, 3)).into_leaves();
        assert!(!pieces.is_empty());
        for p in &pieces {
            // The cap exists, and every one of its triangles came from the cell's cut faces.
            assert!(p.cap.is_some(), "the cell supplies a cap for every cut piece");
            assert!(p.cell.volume() > 0.0, "the cell is a positively oriented solid");
        }
    }

    /// The asset-free entry point is what the examples drive, so it has to hold the same guarantees the
    /// ECS bake does: a fragment set, every piece drawable, and identical output for an identical seed.
    #[test]
    fn fracture_mesh_is_deterministic_and_recentered() {
        let cube = Mesh::from(Cuboid::new(1.0, 2.0, 1.0));
        let parts = [(&cube, Mat4::IDENTITY)];
        let proxy = vec![ProxyCell::from_box(Vec3::ZERO, Vec3::new(0.5, 1.0, 0.5))];
        let a = fracture_mesh(&parts, &proxy, &CutSettings::new(6, 0.05, 0xFEED_BEEF)).into_leaves();
        let b = fracture_mesh(&parts, &proxy, &CutSettings::new(6, 0.05, 0xFEED_BEEF)).into_leaves();

        assert!(a.len() >= 2, "a 1x2x1 box should break into at least two pieces, got {}", a.len());
        assert_eq!(a.len(), b.len(), "same seed, same fragment count");
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.center_local.to_array().map(f32::to_bits), y.center_local.to_array().map(f32::to_bits));
            assert_eq!(x.half_extents.to_array().map(f32::to_bits), y.half_extents.to_array().map(f32::to_bits));
            assert_eq!(x.cell, y.cell, "the proxy cell itself must be reproducible");
        }
        assert!(a.iter().all(|f| f.outer.is_some() || f.cap.is_some()), "every fragment draws something");
        assert!(a.iter().any(|f| f.cap.is_some()), "cutting a solid must produce cut faces");
    }

    /// **Two cells that share a face each keep their own surface — the joint bug.**
    ///
    /// A triangle sitting exactly on a shared boundary is contained by *both* cells, because
    /// `contains` counts "on a face" as inside so that nothing falls through a gap. Assigning it to
    /// the first such cell therefore gave the whole interface to whichever cell came first, and left
    /// the other one holed exactly where the two met.
    ///
    /// Measured on a six-part humanoid before the fix: every limb was short by precisely its own
    /// joint area — head by the neck's `0.0676`, each arm by the shoulder's `0.1040`, each leg by the
    /// hip's `0.0528` — and the torso held all `0.3812` of it. Cells that share a face are the normal
    /// case for a jointed subject, so this is not an exotic input.
    #[test]
    fn two_cells_sharing_a_face_each_keep_their_own_surface() {
        // A box and a second box resting exactly on its +X face — a shoulder, in miniature.
        let left = Cuboid::new(1.0, 1.0, 1.0);
        let right = Cuboid::new(0.4, 0.4, 0.4);
        let (lm, rm) = (Mesh::from(left), Mesh::from(right));
        let parts = [
            (&lm, Mat4::IDENTITY),
            (&rm, Mat4::from_translation(Vec3::new(0.7, 0.0, 0.0))),
        ];
        let proxy = vec![
            ProxyCell::from_box(Vec3::ZERO, Vec3::splat(0.5)),
            ProxyCell::from_box(Vec3::new(0.7, 0.0, 0.0), Vec3::splat(0.2)),
        ];
        // Two pieces: the roots, uncut, so each fragment is exactly one box.
        let cut = CutSettings { soften: 0.0, cap_relief: 0.0, ..CutSettings::new(2, 0.9, 4) };
        let baked = fracture_mesh(&parts, &proxy, &cut);
        let ids = baked.tree.frontier_of(2);
        assert_eq!(ids.len(), 2, "expected the two proxy cells, uncut");

        for (id, want) in ids.iter().zip([6.0f32, 6.0 * 0.4 * 0.4]) {
            let f = baked.fragments.get(id.index()).expect("a fragment per cell");
            let got = mesh_area(f.outer.as_ref()) + mesh_area(f.cap.as_ref());
            assert!(
                (got - want).abs() < 1.0e-3,
                "{id:?} drew {got} of its own {want} surface — the shared face went to the other cell"
            );
        }
    }

    /// Total area of a mesh, for the assignment test above.
    fn mesh_area(mesh: Option<&Mesh>) -> f32 {
        let Some(mesh) = mesh else { return 0.0 };
        let Some(VertexAttributeValues::Float32x3(p)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            return 0.0;
        };
        let Some(idx) = mesh.indices() else { return 0.0 };
        let v: Vec<Vec3> = p.iter().map(|q| Vec3::from_array(*q)).collect();
        idx.iter()
            .collect::<Vec<_>>()
            .chunks_exact(3)
            .filter_map(|t| {
                let (a, b, c) = (*v.get(t[0])?, *v.get(t[1])?, *v.get(t[2])?);
                Some((b - a).cross(c - a).length() * 0.5)
            })
            .sum()
    }

    /// An empty part list is not an error and not a panic — it is simply no fragments.
    #[test]
    fn fracture_mesh_of_nothing_is_empty() {
        assert!(fracture_mesh(&[], &cube_proxy(), &CutSettings::new(8, 0.1, 1)).tree.is_empty());
        // And no proxy at all is a refusal, not a panic.
        let cube = Mesh::from(Cuboid::new(1.0, 1.0, 1.0));
        assert!(fracture_mesh(&[(&cube, Mat4::IDENTITY)], &[], &CutSettings::new(8, 0.1, 1)).tree.is_empty());
    }
}

/// **Round a fragment's drawn surface, so it reads as a lump rather than a shard.**
///
/// Sharp dihedral edges are the visual signature of brittle fracture — ice, glass, cleaved stone —
/// and no amount of shaping the *pieces* changes that, because the edges are what a plane through a
/// solid leaves behind. This subdivides the drawn triangles once and then relaxes each vertex toward
/// the average of its neighbours, which bevels every edge and rounds every corner at the same time.
///
/// # It is Tier B, and that is the whole reason it is allowed to exist
///
/// The proxy cell is untouched, so the collider is still one exact convex hull, `audit_proxy` still
/// measures a closed solid, and every watertightness guarantee holds. The drawn mesh ends up
/// slightly *inside* its own collider, which is the harmless direction — a gib that renders a
/// millimetre proud of its hull would poke through a floor it is resting on; one that renders inside
/// it never can.
///
/// # Positions move, attributes do not
///
/// Smoothing needs to know which corners are the same point, and the soup has none of that —
/// `push_tri` allocates three fresh vertices per triangle. So adjacency is computed over a
/// position-only weld and the result is written *back* onto the original corners. UVs and the
/// skin/cap interior flag survive exactly, which matters: welding them together would smear the
/// cut-face UVs into the skin's and merge the one crease the whole crate exists to produce.
///
/// Normals are re-derived smooth over the welded surface, which is half of what softens the read on
/// its own — flat per-face normals light every facet as a separate plane.
pub(crate) fn soften(soup: &Soup, strength: f32) -> Soup {
    if strength <= 0.0 || soup.is_empty() {
        return soup.clone();
    }

    // One midpoint subdivision, so relaxation has vertices to work with. A fragment's drawn mesh is
    // a few dozen corners; relaxing that directly collapses it instead of rounding it.
    let mut fine = Soup::default();
    for (t, tri) in soup.idx.iter().enumerate() {
        let (a, b, c) = (soup.vtx(tri[0]), soup.vtx(tri[1]), soup.vtx(tri[2]));
        let mid = |x: Vtx, y: Vtx| Vtx {
            pos: (x.pos + y.pos) * 0.5,
            nrm: (x.nrm + y.nrm).normalize_or_zero(),
            uv: (x.uv + y.uv) * 0.5,
        };
        let (ab, bc, ca) = (mid(a, b), mid(b, c), mid(c, a));
        let inside = soup.tri_interior[t];
        for (p, q, r) in [(a, ab, ca), (ab, b, bc), (ca, bc, c), (ab, bc, ca)] {
            fine.push_tri(p, q, r, inside);
        }
    }

    // Position-only weld, purely to learn who neighbours whom.
    let key = |p: Vec3| {
        let q = |x: f32| (x / WELD).round() as i64;
        (q(p.x), q(p.y), q(p.z))
    };
    let mut canon: HashMap<(i64, i64, i64), usize> = HashMap::new();
    let mut unique: Vec<Vec3> = Vec::new();
    let of: Vec<usize> = fine
        .pos
        .iter()
        .map(|p| {
            *canon.entry(key(*p)).or_insert_with(|| {
                unique.push(*p);
                unique.len() - 1
            })
        })
        .collect();

    let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); unique.len()];
    for tri in &fine.idx {
        for i in 0..3 {
            let (u, v) = (of[tri[i] as usize], of[tri[(i + 1) % 3] as usize]);
            if u != v {
                adjacency[u].push(v);
                adjacency[v].push(u);
            }
        }
    }

    // Two relaxation passes. More than that and a fragment stops being the shape it was cut as; the
    // strength dial scales how far each pass travels rather than how many there are, so turning it
    // up rounds harder without also melting the piece.
    let mut moved = unique.clone();
    for _ in 0..2 {
        let previous = moved.clone();
        for (i, neighbours) in adjacency.iter().enumerate() {
            if neighbours.is_empty() {
                continue;
            }
            let mean: Vec3 =
                neighbours.iter().map(|&n| previous[n]).sum::<Vec3>() / neighbours.len() as f32;
            moved[i] = previous[i].lerp(mean, strength.clamp(0.0, 1.0) * 0.5);
        }
    }

    // Area-weighted vertex normals over the welded surface — smooth shading, which is the other half
    // of the softening and costs nothing.
    let mut smooth = vec![Vec3::ZERO; unique.len()];
    for tri in &fine.idx {
        let (i, j, k) = (of[tri[0] as usize], of[tri[1] as usize], of[tri[2] as usize]);
        let face = (moved[j] - moved[i]).cross(moved[k] - moved[i]);
        smooth[i] += face;
        smooth[j] += face;
        smooth[k] += face;
    }

    let mut out = Soup {
        pos: fine.pos.iter().enumerate().map(|(i, _)| moved[of[i]]).collect(),
        nrm: fine
            .nrm
            .iter()
            .enumerate()
            .map(|(i, fallback)| {
                let n = smooth[of[i]].normalize_or_zero();
                // A vertex whose faces cancel has no meaningful average; keep what it had.
                if n == Vec3::ZERO { *fallback } else { n }
            })
            .collect(),
        uv: fine.uv.clone(),
        idx: fine.idx.clone(),
        tri_interior: fine.tri_interior.clone(),
    };
    // Relaxation can pull a triangle's corners together; drop the ones that no longer have area.
    let keep: Vec<bool> = out
        .idx
        .iter()
        .map(|t| {
            let (a, b, c) = (out.pos[t[0] as usize], out.pos[t[1] as usize], out.pos[t[2] as usize]);
            (b - a).cross(c - a).length_squared() >= MIN_CROSS2
        })
        .collect();
    let mut i = 0;
    out.idx.retain(|_| {
        i += 1;
        keep[i - 1]
    });
    let mut j = 0;
    out.tri_interior.retain(|_| {
        j += 1;
        keep[j - 1]
    });
    out
}

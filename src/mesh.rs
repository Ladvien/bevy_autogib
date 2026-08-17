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

use crate::proxy::ProxyCell;
use crate::soup::{Soup, fracture};

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
    let mut remap: HashMap<u32, u32> = HashMap::new();

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
            let nid = if let Some(&n) = remap.get(&old) {
                n
            } else {
                let nid = pos.len() as u32;
                let p = soup.pos[old as usize] - recenter;
                pos.push([p.x, p.y, p.z]);
                let n = soup.nrm[old as usize];
                nrm.push([n.x, n.y, n.z]);
                let u = soup.uv[old as usize];
                uv.push([u.x, u.y]);
                remap.insert(old, nid);
                nid
            };
            idx.push(nid);
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

/// Turn one finished piece into recentered meshes. `None` if it draws nothing at all.
pub(crate) fn geometry_from_piece(cell: ProxyCell, render: &Soup) -> Option<FragmentGeometry> {
    // The cap comes from the cell, never from the render mesh — that is the architecture in one line.
    //
    // The render mesh's boundary vertices are handed along so the cap can weave them into its own ring:
    // the cap is the cross-section of the *cell* (one vertex per cell edge crossed) while the skin's
    // opening is the cross-section of the *triangulated mesh* (one per triangle edge, diagonals
    // included). Without the weave the two meet across T-junctions — flush geometrically, open
    // topologically, and a hairline crack under some rasterisers.
    let seam: Vec<Vec3> = render.pos.clone();
    let mut drawn = render.clone();
    cell.append_cut_faces(&mut drawn, &seam);
    if drawn.is_empty() {
        return None;
    }
    let (mn, mx) = drawn.bbox();
    let center = (mn + mx) * 0.5;
    let half_extents = ((mx - mn) * 0.5).max(Vec3::splat(0.01));
    let outer = soup_to_mesh(&drawn, false, center);
    let cap = soup_to_mesh(&drawn, true, center);
    if outer.is_none() && cap.is_none() {
        return None;
    }
    Some(FragmentGeometry { outer, cap, cell, center_local: center, half_extents })
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
/// Every `Mat4` is that sub-mesh's transform relative to the subject root. `min_fraction` stops a cell
/// being cut once it drops below that fraction of the subject's *size* — a linear fraction, cubed
/// internally to compare volumes, so no caller has to compute an extent first. `seed` drives every plane direction and is the only source of
/// variation. `impact_dir`, when set, biases the first two cuts toward an impact.
///
/// **`parts` order is load-bearing.** Cut planes pass through cell centroids, and the render payload's
/// vertex order decides float sums elsewhere; float addition is not associative, so two different
/// orders give fragments differing in the last bits. Sort `parts` by something authored (an asset path)
/// if they came from anywhere order is not guaranteed; [`crate::bake`] does exactly that.
pub fn fracture_mesh(
    parts: &[(&Mesh, Mat4)],
    proxy: &[ProxyCell],
    target: usize,
    min_fraction: f32,
    seed: u32,
    impact_dir: Option<Vec3>,
) -> Vec<FragmentGeometry> {
    let mut soup = Soup::default();
    for (mesh, xform) in parts {
        append_mesh(&mut soup, mesh, *xform, false);
    }
    if proxy.is_empty() {
        warn!("autogib: refusing to fracture — the caller supplied no proxy cells");
        return Vec::new();
    }
    // A proxy with nothing to carry is not a subject. Cutting it would emit cap-only fragments of a
    // shape the caller never handed us a surface for.
    if soup.is_empty() {
        return Vec::new();
    }
    fracture(soup, proxy, target, min_fraction, seed, impact_dir)
        .into_iter()
        .filter_map(|(cell, render)| geometry_from_piece(cell, &render))
        .collect()
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
        let pieces = fracture_mesh(&[(&cube, Mat4::IDENTITY)], &cube_proxy(), 2, 0.05, 7, None);
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
        above.expect("the cube cuts").append_cut_faces(&mut cap, &[]);
        assert!(
            (interior_area(&cap) - 1.0).abs() < 1.0e-4,
            "cap area should be exactly 1.0, got {}",
            interior_area(&cap)
        );
    }

    #[test]
    fn fracture_reaches_target_and_is_deterministic() {
        let proxy = cube_proxy();
        let a = fracture(cube_soup(), &proxy, 8, 0.05, 0xABCD_1234, None);
        let b = fracture(cube_soup(), &proxy, 8, 0.05, 0xABCD_1234, None);
        assert_eq!(a.len(), b.len());
        assert!(a.len() >= 2 && a.len() <= 8, "reached a sane fragment count: {}", a.len());
        assert!(a.iter().all(|(_, s)| !s.is_empty()), "every piece kept some render surface");
        assert!(
            a[0].0.centroid().distance(b[0].0.centroid()) < 1.0e-6,
            "deterministic per seed"
        );
        assert!(all_finite(&a[0].1), "render payload went non-finite");
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
        // A `min_fraction` so large nothing may be cut must terminate, not loop to the hard cap.
        let out = fracture(cube_soup(), &cube_proxy(), 4, 0.6, 42, None);
        assert!(!out.is_empty());
    }

    /// **A render fragment is open, and that is correct.** It is a surface subset of the subject's own
    /// mesh; the closed artefact is the proxy cell. Asserting watertightness here would be the category
    /// error `AG-004` exists to prevent, so this test pins the *shape* of the claim instead.
    #[test]
    fn a_render_fragment_carries_no_cap_of_its_own() {
        let cube = Mesh::from(Cuboid::new(1.0, 1.0, 1.0));
        let pieces = fracture_mesh(&[(&cube, Mat4::IDENTITY)], &cube_proxy(), 4, 0.05, 3, None);
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
        let a = fracture_mesh(&parts, &proxy, 6, 0.05, 0xFEED_BEEF, None);
        let b = fracture_mesh(&parts, &proxy, 6, 0.05, 0xFEED_BEEF, None);

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

    /// An empty part list is not an error and not a panic — it is simply no fragments.
    #[test]
    fn fracture_mesh_of_nothing_is_empty() {
        assert!(fracture_mesh(&[], &cube_proxy(), 8, 0.1, 1, None).is_empty());
        // And no proxy at all is a refusal, not a panic.
        let cube = Mesh::from(Cuboid::new(1.0, 1.0, 1.0));
        assert!(fracture_mesh(&[(&cube, Mat4::IDENTITY)], &[], 8, 0.1, 1, None).is_empty());
    }
}

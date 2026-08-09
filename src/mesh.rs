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

/// One fractured piece as plain meshes, before anything has been handed to an asset arena.
///
/// Both meshes are recentered to `center_local` (their shared bounding-box center), so a body placed
/// at `origin + center_local * scale` with a `half_extents * scale` box collider lines up exactly with
/// the rendered chunk. Either mesh may be `None`: a fragment with no cut faces has no cap, and a
/// pure-cap sliver has no outer skin.
pub struct FragmentGeometry {
    /// The subject's own surface — whatever material the intact subject wore.
    pub outer: Option<Mesh>,
    /// The cut faces this fracture created, with planar cross-section UVs. Give these the "inside"
    /// material (raw meat, splintered wood, fractured stone) — that contrast is the whole read.
    pub cap: Option<Mesh>,
    pub center_local: Vec3,
    /// Half the bounding box per axis, in subject-local units → sizes the chunk's box collider.
    pub half_extents: Vec3,
}

/// Turn a fragment soup into recentered meshes. `None` if it has no drawable triangles.
pub(crate) fn geometry_from_soup(soup: &Soup) -> Option<FragmentGeometry> {
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
    Some(FragmentGeometry { outer, cap, center_local: center, half_extents })
}

/// **The whole pipeline, with no assets and no ECS.** Merge `parts` into one triangle soup in subject-
/// local space, recursively plane-cut it into at most `target` pieces, and return each piece as
/// recentered meshes.
///
/// Every `Mat4` is that sub-mesh's transform relative to the subject root, so a multi-part character
/// fractures as one solid rather than as independent limbs. `min_extent` stops a piece being cut below
/// that size; `seed` drives every plane direction and is the only source of variation. `impact_dir`,
/// when set, biases the first two cuts toward an impact — a reserved seam, unused by the ECS bake.
///
/// **`parts` order is load-bearing.** The merged soup's vertex order decides the float sums that
/// produce each cut plane's origin, and float addition is not associative — so two different orders
/// give fragments that differ in the last bits. Sort `parts` by something authored (an asset path) if
/// they came from anywhere order is not guaranteed; [`crate::bake`] does exactly that, and the comment
/// there explains what it cost to learn.
pub fn fracture_mesh(
    parts: &[(&Mesh, Mat4)],
    target: usize,
    min_extent: f32,
    seed: u32,
    impact_dir: Option<Vec3>,
) -> Vec<FragmentGeometry> {
    let mut soup = Soup::default();
    for (mesh, xform) in parts {
        append_mesh(&mut soup, mesh, *xform, false);
    }
    if soup.is_empty() {
        return Vec::new();
    }
    fracture(soup, target, min_extent, seed, impact_dir)
        .iter()
        .filter_map(geometry_from_soup)
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Tests — pure geometry, no App required.
// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::soup::{Plane, cap_side, split_soup};
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

    #[test]
    fn slice_cube_axis_plane() {
        let s = cube_soup();
        let (above, below) = split_soup(&s, &Plane { point: Vec3::ZERO, normal: Vec3::X });
        assert!(!above.is_empty() && !below.is_empty());
        assert!(above.pos.iter().all(|p| p.x >= -1.0e-3), "above stays on +X side");
        assert!(below.pos.iter().all(|p| p.x <= 1.0e-3), "below stays on -X side");
        assert!(above.tri_interior.iter().any(|&i| i), "above has a cap");
        assert!(below.tri_interior.iter().any(|&i| i), "below has a cap");
        assert!(all_finite(&above) && all_finite(&below));
    }

    #[test]
    fn cap_is_unit_square_area() {
        let s = cube_soup();
        let (above, _) = split_soup(&s, &Plane { point: Vec3::ZERO, normal: Vec3::Y });
        // A mid-slice of the unit cube leaves a 1x1 cross-section.
        assert!((interior_area(&above) - 1.0).abs() < 0.05, "cap area ~1.0, got {}", interior_area(&above));
    }

    #[test]
    fn fracture_reaches_target_and_is_deterministic() {
        let a = fracture(cube_soup(), 8, 0.05, 0xABCD_1234, None);
        let b = fracture(cube_soup(), 8, 0.05, 0xABCD_1234, None);
        assert_eq!(a.len(), b.len());
        assert!(a.len() >= 2 && a.len() <= 8, "reached a sane fragment count: {}", a.len());
        assert!(a.iter().all(|s| !s.is_empty()));
        assert!(a[0].centroid().distance(b[0].centroid()) < 1.0e-6, "deterministic per seed");
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

    #[test]
    fn open_boundary_is_dropped() {
        // Path a-b-c-d (open, never returns to a) → no cap emitted, no panic.
        let (a, b, c, d) = (Vec3::ZERO, Vec3::X, Vec3::new(1.0, 1.0, 0.0), Vec3::new(0.0, 2.0, 0.0));
        let segs = vec![[a, b], [b, c], [c, d]];
        let mut out = Soup::default();
        cap_side(&segs, &Plane { point: Vec3::ZERO, normal: Vec3::Z }, Vec3::Z, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn degenerate_plane_leaves_piece_whole() {
        let s = cube_soup();
        // Plane far outside the cube (all vertices on one side).
        let (above, below) = split_soup(&s, &Plane { point: Vec3::splat(5.0), normal: Vec3::X });
        assert!(above.is_empty(), "nothing above a plane past the cube");
        assert!(!below.is_empty());
        // And the fracture driver must not spin on such a piece.
        let out = fracture(cube_soup(), 4, 0.6, 42, None);
        assert!(!out.is_empty());
    }

    /// The asset-free entry point is what the examples drive, so it has to hold the same guarantees the
    /// ECS bake does: a fragment set, every piece drawable, and identical output for an identical seed.
    #[test]
    fn fracture_mesh_is_deterministic_and_recentered() {
        let cube = Mesh::from(Cuboid::new(1.0, 2.0, 1.0));
        let parts = [(&cube, Mat4::IDENTITY)];
        let a = fracture_mesh(&parts, 6, 0.1, 0xFEED_BEEF, None);
        let b = fracture_mesh(&parts, 6, 0.1, 0xFEED_BEEF, None);

        assert!(a.len() >= 2, "a 1x2x1 box should break into at least two pieces, got {}", a.len());
        assert_eq!(a.len(), b.len(), "same seed, same fragment count");
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.center_local.to_array().map(f32::to_bits), y.center_local.to_array().map(f32::to_bits));
            assert_eq!(x.half_extents.to_array().map(f32::to_bits), y.half_extents.to_array().map(f32::to_bits));
        }
        assert!(a.iter().all(|f| f.outer.is_some() || f.cap.is_some()), "every fragment draws something");
        assert!(a.iter().any(|f| f.cap.is_some()), "cutting a solid must produce cut faces");
    }

    /// An empty part list is not an error and not a panic — it is simply no fragments.
    #[test]
    fn fracture_mesh_of_nothing_is_empty() {
        assert!(fracture_mesh(&[], 8, 0.1, 1, None).is_empty());
    }
}

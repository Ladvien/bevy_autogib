//! **What the fracture actually produced** — the measurement this crate spent its whole life without.
//!
//! `examples/fracture_cube.rs` has always been honest that its one quality number counts closed *loops*
//! and is "NOT a watertightness proof". This module is the proof, or the refusal of it: it takes a
//! finished [`FragmentGeometry`] and reports whether that shard is closed, manifold, consistently
//! wound, and safe to hand a physics engine as a triangle mesh.
//!
//! # Two things here are load-bearing, and both are easy to get backwards
//!
//! **A fragment is audited as skin ∪ cap, never as two meshes.** The crate hands out the subject's
//! outer surface and the newly-cut faces separately so they can take different materials — but
//! *neither one is a closed surface on its own*, and never can be. The skin is missing exactly the
//! faces the cut created; the cap is a bare disc. Only their union can be watertight, so auditing
//! either alone would produce a large, confident, meaningless boundary-edge count.
//!
//! **The mesh is welded before it is measured.** [`crate::soup::Soup::push_tri`] allocates three fresh
//! vertices for every triangle it emits, and every triangle in a cut piece came from `push_tri` — so a
//! fragment reaches this module with `positions.len() == 3 * triangles` and no two triangles sharing an
//! index. Topology over that buffer is not merely inaccurate, it is *vacuous*: every edge is incident
//! to exactly one face, so everything is a boundary edge and the Euler characteristic is noise.
//! `isomesh` learned this about its own subgrid extractor and now documents "weld before you validate";
//! this module does the weld so no caller has to know that.
//!
//! The weld is position-only, which is right here and wrong everywhere else — see [`WELD_EPSILON`].

use bevy::log::warn;
use bevy::mesh::{Indices, Mesh, VertexAttributeValues};
use isomesh::MeshBuffer;
use isomesh::collider::{self, ColliderReadiness};
use isomesh::validate::{MeshReport, ValidateConfig};
use isomesh::weld::Welder;

use crate::mesh::FragmentGeometry;

/// The distance below which two vertices are the same vertex.
///
/// **[`crate::soup::WELD`] itself, not a number chosen here.** That is the lattice `cap_side` already
/// snaps cut-boundary endpoints onto, so it is already this pipeline's definition of "the same point".
/// Adopting it means the audit and the slicer agree about what a seam is; picking independently would
/// mean measuring a mesh nobody shipped.
///
/// **The weld is position-only, which is right for exactly one purpose: asking topological questions.**
/// Whether a surface closes is a property of where its vertices *are*, not what normals they carry, so
/// merging a hard edge's two normals costs nothing — the welded buffer is measured and dropped.
///
/// It would be badly wrong to weld the *shipped* meshes this way. `isomesh`'s `Welder` never compares
/// normals and discards the merged-away one, which would smear the crease between skin and cut face —
/// the entire visual read this crate exists to produce — and, on a fragment cut more than once, the
/// creases between cut faces of different planes too. A weld that ships needs a composite
/// position+normal+UV key. That is a different piece of work, and it is not this.
const WELD_EPSILON: f32 = crate::soup::WELD;

/// The length scale `isomesh`'s two thresholds are derived from — **back-solved from
/// [`WELD_EPSILON`], not asserted.**
///
/// `ValidateConfig` takes a grid spacing and derives `weld_epsilon = cell_size * WELD_EPSILON_REL`
/// from it. This crate's "grid" is the cap-assembly lattice, so the spacing that reproduces it is
/// `WELD / WELD_EPSILON_REL` — which comes out at `1.0`, the subject-local unit the slicer's constants
/// were tuned in. Writing the division rather than the `1.0` is what keeps the two in step if either
/// constant ever moves.
///
/// The derived degenerate-area threshold is then `1e-6 * cell_size²`, about twice the `1.0e-12`
/// squared-cross-product floor `soup_to_mesh` drops triangles on — so a small non-zero
/// `degenerate_triangles` count is expected, and is a useful warning if that floor ever drifts.
const CELL_SIZE: f64 = WELD_EPSILON as f64 / ValidateConfig::WELD_EPSILON_REL;

/// What one finished fragment turned out to be.
///
/// The counts come from `isomesh`'s validator over the welded skin ∪ cap; the three predicates are its
/// documented collider policy rather than this crate's opinion.
#[derive(Clone, Debug, PartialEq)]
pub struct FragmentAudit {
    /// Triangles the validator actually considered.
    pub triangles: u64,
    /// Vertices in the buffer as shipped, before the audit's weld — `3 * triangles` today.
    pub vertices_before_weld: u64,
    /// Vertices after welding coincident positions. The ratio is how much the un-shared soup costs.
    pub vertices_after_weld: u64,

    /// Edges incident to exactly one face. **Zero is watertight**; anything else is an open cut, and
    /// this is the number `fracture_cube`'s "carries at least one closed cut face" could never give.
    pub boundary_edges: u64,
    /// Edges incident to three or more faces.
    pub non_manifold_edges: u64,
    /// Vertices whose incident faces do not form a single fan — bowties and umbrella branching.
    pub non_manifold_vertices: u64,
    /// Edges whose two faces traverse them the *same* way, i.e. one of them is inside out.
    pub inconsistently_oriented_edges: u64,

    /// `V − E + F` over the welded surface. A closed shard should be `2`.
    pub euler_characteristic: i64,
    /// Genus, when the surface is a single oriented manifold component and the formula applies.
    pub genus: Option<i64>,

    /// The fragment is structurally sound enough to hand a solver as a triangle mesh at all.
    pub usable_as_trimesh: bool,
    /// The fragment is closed, manifold and consistently wound — so a solver may build inside/outside
    /// pseudo-normals from it. **This is the strong "it is really a solid" answer.**
    pub supports_inside_outside: bool,

    /// Signed volume of the welded surface, `(1/6)·Σ (a × b)·c`.
    ///
    /// **The only field here that can see a wrongly-filled hole.** If a cut passes through a hollow
    /// and the inner boundary loop is capped as a solid disc instead of punched out, the result is a
    /// perfectly ordinary closed manifold — two faces per edge, consistent winding, `χ = 2`. Every
    /// topological field above reports it clean, because it *is* clean; it is simply the wrong solid.
    /// Volume is the invariant that notices, and it is only meaningful when [`Self::is_closed`].
    ///
    /// Negative means the surface is inside out. Recentering does not change it.
    pub signed_volume: f32,
}

/// Signed volume of a closed triangle surface, via the divergence theorem: `(1/6)·Σ (a × b)·c`.
///
/// Meaningless for an open surface — read it only alongside [`FragmentAudit::is_closed`]. Computed on
/// the welded buffer so it matches the topology the rest of the audit reports.
fn signed_volume(positions: &[[f32; 3]], indices: &[u32]) -> f32 {
    let mut v6 = 0.0f32;
    for t in indices.chunks_exact(3) {
        // The buffer came out of `validate_indexed`'s own input, but this function does not get to
        // assume that: an out-of-range index here would panic, and this crate does not panic on data.
        let (Some(a), Some(b), Some(c)) = (
            positions.get(t[0] as usize),
            positions.get(t[1] as usize),
            positions.get(t[2] as usize),
        ) else {
            continue;
        };
        let cross = [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ];
        v6 += cross[0] * c[0] + cross[1] * c[1] + cross[2] * c[2];
    }
    v6 / 6.0
}

impl FragmentAudit {
    /// No boundary edges: the shard's surface closes on itself.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.boundary_edges == 0
    }

    /// No non-manifold edges or vertices.
    #[must_use]
    pub fn is_manifold(&self) -> bool {
        self.non_manifold_edges == 0 && self.non_manifold_vertices == 0
    }

    /// Every structural fault at once — the single number to trend.
    #[must_use]
    pub fn violations(&self) -> u64 {
        self.boundary_edges
            + self.non_manifold_edges
            + self.non_manifold_vertices
            + self.inconsistently_oriented_edges
    }
}

/// Append one mesh's positions, normals and triangles to `buf`, offsetting indices.
///
/// Returns `false` (and `warn!`s) for a mesh this crate could not itself have produced: no
/// `Float32x3` positions, or a normal array that does not match the position count. `isomesh`'s
/// `MeshBuffer` requires normals parallel to positions, so a mismatch cannot be papered over.
///
/// UVs are dropped on purpose. `MeshBuffer` has no UV channel and the audit asks no question a UV
/// could answer, so they stay in the shipped `Mesh` where they belong.
fn append(buf: &mut MeshBuffer<f32>, mesh: &Mesh) -> bool {
    let Some(VertexAttributeValues::Float32x3(pos)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION) else {
        warn!("autogib: audit skipped a fragment mesh with no Float32x3 POSITION");
        return false;
    };
    let Some(VertexAttributeValues::Float32x3(nrm)) = mesh.attribute(Mesh::ATTRIBUTE_NORMAL) else {
        warn!("autogib: audit skipped a fragment mesh with no Float32x3 NORMAL");
        return false;
    };
    if nrm.len() != pos.len() {
        warn!(
            "autogib: audit skipped a fragment mesh whose NORMAL count ({}) differs from its POSITION \
             count ({})",
            nrm.len(),
            pos.len()
        );
        return false;
    }

    let base = buf.positions.len() as u32;
    buf.positions.extend_from_slice(pos);
    buf.normals.extend_from_slice(nrm);
    // `geometry_from_soup` always emits `Indices::U32`; a `U16` buffer would mean the mesh came from
    // somewhere else, and it costs one arm to accept it rather than silently audit nothing.
    match mesh.indices() {
        Some(Indices::U32(v)) => buf.indices.extend(v.iter().map(|i| i + base)),
        Some(Indices::U16(v)) => buf.indices.extend(v.iter().map(|i| u32::from(*i) + base)),
        None => {
            warn!("autogib: audit skipped a non-indexed fragment mesh");
            return false;
        }
    }
    true
}

/// Measure one finished fragment: weld skin ∪ cap, then validate.
///
/// # Errors
///
/// A `String` describing why the fragment could not be measured — it carried no drawable triangles, or
/// the weld rejected its epsilon. Both are loud rather than silent, because an audit that quietly
/// reported "no violations" for a fragment it never looked at is worse than no audit.
pub fn audit_fragment(frag: &FragmentGeometry) -> Result<FragmentAudit, String> {
    let mut buf: MeshBuffer<f32> = MeshBuffer::new();
    // Skin and cap together — see the module docs for why measuring either alone is meaningless.
    if let Some(outer) = frag.outer.as_ref() {
        append(&mut buf, outer);
    }
    if let Some(cap) = frag.cap.as_ref() {
        append(&mut buf, cap);
    }
    if buf.indices.is_empty() {
        return Err("fragment has no drawable triangles to audit".to_string());
    }

    let vertices_before_weld = buf.positions.len() as u64;
    let report = weld_then_validate(&mut buf)?;
    let readiness = collider::from_report(&report);

    Ok(FragmentAudit {
        triangles: report.faces,
        vertices_before_weld,
        vertices_after_weld: buf.positions.len() as u64,
        boundary_edges: report.boundary_edges,
        non_manifold_edges: report.non_manifold_edges,
        non_manifold_vertices: report.non_manifold_vertices,
        inconsistently_oriented_edges: report.inconsistently_oriented_edges,
        euler_characteristic: report.euler_characteristic,
        genus: report.genus,
        usable_as_trimesh: readiness.is_usable(),
        supports_inside_outside: ColliderReadiness::supports_inside_outside(&readiness),
        signed_volume: signed_volume(&buf.positions, &buf.indices),
    })
}

/// Weld `buf` in place, then validate it. Split out so the ordering — weld *first*, always — is one
/// statement in one place rather than a convention every caller has to remember.
fn weld_then_validate(buf: &mut MeshBuffer<f32>) -> Result<MeshReport, String> {
    let mut welder = Welder::<f32>::new();
    welder
        .weld(buf, WELD_EPSILON)
        .map_err(|e| format!("weld rejected epsilon {WELD_EPSILON}: {e}"))?;

    let cfg = ValidateConfig::from_cell_size(CELL_SIZE)
        .map_err(|e| format!("audit cell size {CELL_SIZE} is not a usable length scale: {e}"))?;
    Ok(isomesh::validate::validate_indexed(&buf.positions, &buf.indices, &cfg))
}

/// Audit every fragment of a fracture. Fragments that cannot be measured are `warn!`-skipped and
/// omitted, so the returned length may be shorter than `frags` — deliberately, because padding the
/// result with a fabricated clean audit is exactly the lie this module exists to stop telling.
#[must_use]
pub fn audit_fragments(frags: &[FragmentGeometry]) -> Vec<FragmentAudit> {
    frags
        .iter()
        .enumerate()
        .filter_map(|(i, f)| match audit_fragment(f) {
            Ok(a) => Some(a),
            Err(e) => {
                warn!("autogib: fragment {i} could not be audited: {e}");
                None
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Tests — the measurements this crate could not previously take.
// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{fracture_mesh, geometry_from_soup};
    use crate::soup::{Plane, Soup, split_soup};
    use bevy::math::{Mat4, Vec3, primitives::Cuboid};
    use isomesh::validate::check_determinism;

    /// A U-shaped cross-section, centred on the origin, wound counter-clockwise in XY.
    ///
    /// **The notch is the whole point.** Its vertex centroid is `(0, 0.25)`, which lies inside the
    /// notch and therefore *outside the solid* — so a cap fanned from that centroid lays triangles
    /// across empty space. A convex section (every fixture this crate had before) cannot show that.
    const U_OUTLINE: [[f32; 2]; 8] = [
        [-1.5, -1.5],
        [1.5, -1.5],
        [1.5, 1.5],
        [0.5, 1.5],
        [0.5, -0.5],
        [-0.5, -0.5],
        [-0.5, 1.5],
        [-1.5, 1.5],
    ];

    /// The U's area: a 3×3 square less the 1×2 notch.
    const U_AREA: f32 = 9.0 - 2.0;

    /// A correct ear-clipped triangulation of [`U_OUTLINE`], by index.
    ///
    /// Hand-derived and hand-checked, deliberately: the fixture must not be built with the very fan
    /// this test exists to indict, or it would be measuring itself.
    const U_TRIS: [[usize; 3]; 6] = [[2, 3, 4], [1, 2, 4], [1, 4, 5], [0, 1, 5], [0, 5, 6], [0, 6, 7]];

    /// Extrude [`U_OUTLINE`] along Z into a closed, watertight, non-convex prism.
    ///
    /// Built directly as a `Soup` rather than as a `Mesh` so the winding is explicit: side quads take
    /// their orientation from the CCW outline, the `+Z` cap uses [`U_TRIS`] as written and the `-Z`
    /// cap uses it reversed.
    fn u_prism(half_depth: f32) -> Soup {
        let mut s = Soup::default();
        let at = |i: usize, z: f32| Vec3::new(U_OUTLINE[i][0], U_OUTLINE[i][1], z);
        let v = |p: Vec3| crate::soup::Vtx { pos: p, nrm: Vec3::ZERO, uv: bevy::math::Vec2::ZERO };

        // Sides: for a CCW outline, `(a,-h) (b,-h) (b,+h)` faces outward.
        for i in 0..U_OUTLINE.len() {
            let j = (i + 1) % U_OUTLINE.len();
            let (a0, b0) = (at(i, -half_depth), at(j, -half_depth));
            let (a1, b1) = (at(i, half_depth), at(j, half_depth));
            s.push_tri(v(a0), v(b0), v(b1), false);
            s.push_tri(v(a0), v(b1), v(a1), false);
        }
        // Caps: +Z as wound, -Z reversed, so both face away from the solid.
        for t in U_TRIS {
            s.push_tri(v(at(t[0], half_depth)), v(at(t[1], half_depth)), v(at(t[2], half_depth)), false);
            s.push_tri(v(at(t[0], -half_depth)), v(at(t[2], -half_depth)), v(at(t[1], -half_depth)), false);
        }
        s
    }

    /// Total area of a soup's cut-cap triangles.
    fn cap_area(s: &Soup) -> f32 {
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

    fn cube_parts() -> Mesh {
        Mesh::from(Cuboid::new(1.0, 2.0, 1.0))
    }

    /// **The crate's central promise, tested against every byte for the first time.**
    ///
    /// The README says two runs of the same build on the same asset produce bit-identical fragments,
    /// and until now nothing compared fragment *vertex data* between runs at all: `mesh.rs` compared
    /// only `center_local` and `half_extents`, and `soup.rs` compared a centroid within `1e-6`. A
    /// fracture whose planes were right but whose emitted triangles differed would have passed both.
    ///
    /// `check_determinism` runs the closure three times — twice into fresh buffers, once into a buffer
    /// that was used, `reset()` and used again — and compares every position, normal and index under
    /// IEEE `totalOrder`, so `+0.0`/`-0.0` and `NaN` are distinguished rather than papered over.
    #[test]
    fn fracture_output_is_bit_identical_across_runs() {
        let cube = cube_parts();
        let report = check_determinism(|out: &mut MeshBuffer<f32>| {
            let pieces = fracture_mesh(&[(&cube, Mat4::IDENTITY)], 8, 0.1, 0xC0FF_EE00, None);
            for p in &pieces {
                // Base on the buffer's current length, not zero — the third run is handed a `reset()`
                // buffer, and assuming it starts empty is exactly the bug that run exists to catch.
                if let Some(m) = p.outer.as_ref() {
                    append(out, m);
                }
                if let Some(m) = p.cap.as_ref() {
                    append(out, m);
                }
            }
        });
        assert!(
            report.is_deterministic(),
            "the fracture moved between two runs of the same build: {:?}",
            report.divergence
        );
        assert!(report.vertices > 0, "the determinism check ran on an empty mesh, so it proved nothing");
    }

    /// Cutting a closed solid yields closed pieces. That is a theorem about plane-cutting, not a
    /// measurement — so this asserts equality, and a failure means the capper is wrong.
    #[test]
    fn every_fragment_of_a_closed_solid_is_closed() {
        let cube = cube_parts();
        let pieces = fracture_mesh(&[(&cube, Mat4::IDENTITY)], 8, 0.1, 0x5EED, None);
        assert!(pieces.len() >= 2, "expected the cube to break, got {}", pieces.len());

        for (i, p) in pieces.iter().enumerate() {
            let a = audit_fragment(p).unwrap_or_else(|e| panic!("fragment {i} could not be audited: {e}"));
            assert_eq!(a.boundary_edges, 0, "fragment {i} has an open cut: {a:?}");
            assert!(a.is_manifold(), "fragment {i} is not a manifold: {a:?}");
            assert_eq!(a.inconsistently_oriented_edges, 0, "fragment {i} has an inside-out face: {a:?}");
            assert_eq!(a.euler_characteristic, 2, "fragment {i} is not a topological sphere: {a:?}");
            assert!(a.supports_inside_outside, "fragment {i} is not solid enough for a collider: {a:?}");
        }
    }

    /// The pieces must add up to the thing they came from. This is the one invariant that survives a
    /// capping bug which is topologically perfect — see [`FragmentAudit::signed_volume`].
    #[test]
    fn fracture_conserves_volume() {
        let cube = cube_parts();
        let pieces = fracture_mesh(&[(&cube, Mat4::IDENTITY)], 8, 0.1, 0x5EED, None);
        let total: f32 = pieces.iter().filter_map(|p| audit_fragment(p).ok()).map(|a| a.signed_volume).sum();
        // `Cuboid::new(1, 2, 1)` encloses 2.0.
        assert!(
            (total - 2.0).abs() < 1.0e-3,
            "fragments enclose {total}, but the source cube encloses 2.0 — the fracture gained or lost solid"
        );
    }

    /// **A known defect, locked open on purpose.**
    ///
    /// `push_cap_tri` fans every boundary loop from its centroid, which is only valid when the loop is
    /// star-shaped about that centroid. [`U_OUTLINE`]'s centroid sits in the notch, so the fan lays
    /// triangles over space that is not inside the cut, and the cap comes out larger than the section
    /// it is supposed to close.
    ///
    /// The winding flip at `push_cap_tri` hides this from any *normal* check — every triangle is
    /// turned to face `outward` individually. It cannot hide from an *edge orientation* check: two
    /// neighbouring fan triangles share the spoke `centroid→p`, and flipping exactly one of them makes
    /// both traverse that spoke the same way, which is precisely `inconsistently_oriented_edges`.
    ///
    /// **This test asserts the bug is still here.** When the cap triangulation is fixed, it fails —
    /// that is the design. Flip both assertions to their correct form (`== 0`, and area equal to
    /// `U_AREA`) in the same commit that fixes it.
    #[test]
    fn known_defect_cap_fan_folds_on_a_non_convex_section() {
        let (above, _) = split_soup(&u_prism(1.0), &Plane { point: Vec3::ZERO, normal: Vec3::Z });
        assert!(!above.is_empty(), "the U-prism fixture did not cut");

        let area = cap_area(&above);
        assert!(
            area > U_AREA + 1.0e-3,
            "the fan no longer overshoots the cross-section (got {area}, section is {U_AREA}). If the \
             capper was fixed, change this to `assert!((area - U_AREA).abs() < 1e-3)`."
        );

        let g = geometry_from_soup(&above).expect("the cut piece draws something");
        let a = audit_fragment(&g).expect("the cut piece can be audited");
        assert!(
            a.inconsistently_oriented_edges > 0,
            "the fan fold no longer shows as inconsistent edge orientation. If the capper was fixed, \
             change this to `assert_eq!(a.inconsistently_oriented_edges, 0)`. Audit: {a:?}"
        );
    }

    /// The audit's own weld must actually be doing something — if it silently became a no-op, every
    /// topology number above would quietly turn into nonsense while still reporting cleanly.
    #[test]
    fn the_audit_welds_before_it_measures() {
        let cube = cube_parts();
        let pieces = fracture_mesh(&[(&cube, Mat4::IDENTITY)], 4, 0.1, 1, None);
        let a = audit_fragment(&pieces[0]).expect("first fragment audits");
        assert!(
            a.vertices_after_weld < a.vertices_before_weld,
            "the weld merged nothing ({} -> {}), so the topology counts describe an unwelded soup and \
             mean nothing",
            a.vertices_before_weld,
            a.vertices_after_weld
        );
    }
}

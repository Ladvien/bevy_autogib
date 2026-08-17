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

    /// Signed volume of the welded surface, `(1/6)·Σ (a × b)·c`. Negative means inside out.
    ///
    /// **Read this before trusting the number. Two claims that used to live here were measured and
    /// found false** — by `known_defect_nested_cut_boundary_is_filled_solid`, which exists to pin them.
    ///
    /// It said this was *"the only field here that can see a wrongly-filled hole"*, and that a cut
    /// through a hollow whose inner loop is capped solid would come back a perfectly ordinary closed
    /// manifold that only volume could indict. Neither holds. Filling a bore is a **genus reduction**,
    /// so [`Self::euler_characteristic`] moves (0 → 2 on that fixture); and the paving disagrees with
    /// the bore wall about which way is out, so [`Self::inconsistently_oriented_edges`] goes positive
    /// and [`Self::supports_inside_outside`] goes false. Volume, measured where the geometry actually
    /// sits, came back *exactly correct* — the two same-facing sheets over the bore cancel against the
    /// rim walls. It is the field that misses that defect, not the field that catches it.
    ///
    /// It also said *"recentering does not change it"*. Translation preserves this sum only for a
    /// surface that is closed **and consistently oriented**; drop the second condition and it does not.
    /// Since [`crate::mesh::FragmentGeometry`] is recentred on its bbox before it ever reaches here,
    /// **the volume reported for an inconsistently-oriented fragment is offset by an amount that
    /// depends on where the fragment happened to sit.** On the hollow prism that offset is a tidy
    /// `bore_area × length / 3` in all 24 configurations tested, which is exactly the kind of clean
    /// number that invites being mistaken for a measurement of the defect. It is not one.
    ///
    /// So: meaningful when [`Self::is_closed`] **and** `inconsistently_oriented_edges == 0`. Outside
    /// that, it is a number, not a volume.
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
    use crate::soup::{Plane, Soup, cap_side, split_soup};
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

    /// Outer boundary of the hollow prism's cross-section: a 3×3 square, CCW in XY.
    const BORE_OUTER: [[f32; 2]; 4] = [[-1.5, -1.5], [1.5, -1.5], [1.5, 1.5], [-1.5, 1.5]];
    /// Inner boundary — the bore: a 1×1 square, also written CCW. It is *walked in reverse* when the
    /// bore walls are built, which is what turns it into a hole rather than a second solid.
    const BORE_INNER: [[f32; 2]; 4] = [[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]];

    /// Cross-section area of the hollow prism: a 3×3 square less its 1×1 bore.
    ///
    /// Both this and [`BORE_HOLE_AREA`] describe `hollow_prism(_, 0.5)`, the only configuration the
    /// committed test uses. `bore_half` stays a parameter so the `bore_area × length / 3` result
    /// recorded on `known_defect_nested_cut_boundary_is_filled_solid` can be re-derived rather than
    /// taken on trust.
    const BORE_SECTION_AREA: f32 = 9.0 - 1.0;
    /// Cross-section area of the bore alone — the region a fan will wrongly fill.
    const BORE_HOLE_AREA: f32 = 1.0;

    /// A **hollow** square prism: closed, manifold, and genus 1 (χ = 0).
    ///
    /// **The fixture `u_prism` cannot replace.** A U-section is non-convex but *simply connected*, so
    /// its cut boundary is one loop. This one's cut boundary is **two nested loops**, and nesting is a
    /// property no counter in `MeshReport` looks for: a cap that fills the bore solid can still be
    /// closed and consistently wound. It would simply be the wrong solid, and
    /// [`FragmentAudit::signed_volume`] is the field most likely to notice.
    ///
    /// Winding, stated because it is the thing that makes it a hole: the outer wall takes its
    /// orientation from [`BORE_OUTER`] as written, and the bore wall walks [`BORE_INNER`] **in
    /// reverse**, so its faces point into the bore — which is *outward* from the solid. Both end caps
    /// are annuli, triangulated by hand rather than fanned, for the same reason `U_TRIS` is.
    fn hollow_prism(half_depth: f32, bore_half: f32) -> Soup {
        let mut s = Soup::default();
        let v = |p: Vec3| crate::soup::Vtx { pos: p, nrm: Vec3::ZERO, uv: bevy::math::Vec2::ZERO };
        let o = |i: usize, z: f32| Vec3::new(BORE_OUTER[i][0], BORE_OUTER[i][1], z);
        let n = |i: usize, z: f32| Vec3::new(BORE_INNER[i][0] * bore_half * 2.0, BORE_INNER[i][1] * bore_half * 2.0, z);

        // Outer wall: same construction as `u_prism`, faces away from the solid.
        for i in 0..4 {
            let j = (i + 1) % 4;
            s.push_tri(v(o(i, -half_depth)), v(o(j, -half_depth)), v(o(j, half_depth)), false);
            s.push_tri(v(o(i, -half_depth)), v(o(j, half_depth)), v(o(i, half_depth)), false);
        }
        // Bore wall: the inner outline walked backwards, so these faces point into the hole.
        for i in 0..4 {
            let j = (i + 3) % 4; // the previous vertex — the next one going backwards
            s.push_tri(v(n(i, -half_depth)), v(n(j, -half_depth)), v(n(j, half_depth)), false);
            s.push_tri(v(n(i, -half_depth)), v(n(j, half_depth)), v(n(i, half_depth)), false);
        }
        // End caps, as annuli: each side of the square contributes a quad spanning outer to inner.
        for i in 0..4 {
            let j = (i + 1) % 4;
            // +Z, wound CCW seen from +Z.
            s.push_tri(v(o(i, half_depth)), v(o(j, half_depth)), v(n(j, half_depth)), false);
            s.push_tri(v(o(i, half_depth)), v(n(j, half_depth)), v(n(i, half_depth)), false);
            // -Z, reversed.
            s.push_tri(v(o(j, -half_depth)), v(o(i, -half_depth)), v(n(j, -half_depth)), false);
            s.push_tri(v(n(j, -half_depth)), v(o(i, -half_depth)), v(n(i, -half_depth)), false);
        }
        s
    }


    /// The five segments of a regular pentagram `{5/2}`, unit circumradius, in the plane `z = 0`.
    ///
    /// **The counterexample to "fold ⟺ inconsistent orientation".** A pentagram is drawn by joining
    /// every *second* vertex of a regular pentagon, so the closed path winds around the centre
    /// **twice, without ever reversing**. Fan it from its centroid and all five triangles carry the
    /// same signed area — so `push_cap_tri`'s per-triangle flip has nothing to disagree about, every
    /// shared spoke is traversed oppositely by its two triangles, and the surface is *consistently
    /// oriented*. The fan still folds: it covers the inner pentagon twice.
    ///
    /// Each segment's endpoints are distinct pentagon vertices and the crossings are not vertices, so
    /// every vertex has degree 2 and `assemble_loops` recovers one loop with no ambiguity to resolve.
    fn pentagram_segments() -> Vec<[Vec3; 2]> {
        let vertex = |i: usize| {
            let a = std::f32::consts::TAU * (i as f32) / 5.0;
            Vec3::new(a.cos(), a.sin(), 0.0)
        };
        // 0 → 2 → 4 → 1 → 3 → 0: step two vertices at a time, which is what `{5/2}` means.
        (0..5).map(|k| [vertex((2 * k) % 5), vertex((2 * (k + 1)) % 5)]).collect()
    }

    /// Signed volume of a soup's triangles **as they sit**, with no recentring — the same
    /// `(1/6)·Σ (a × b)·c` the audit uses.
    ///
    /// Exists because the difference between this and [`FragmentAudit::signed_volume`] is itself a
    /// finding; see `known_defect_nested_cut_boundary_is_filled_solid`.
    fn soup_volume(s: &Soup) -> f32 {
        let mut v6 = 0.0f32;
        for tri in &s.idx {
            let (a, b, c) = (s.pos[tri[0] as usize], s.pos[tri[1] as usize], s.pos[tri[2] as usize]);
            v6 += a.cross(b).dot(c);
        }
        v6 / 6.0
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

    /// The two-shell subject: a torso box and a head box that overlap at the neck.
    ///
    /// **This is the honest case, not a rigged one.** Two closed shells that meet are not a manifold at
    /// the seam, and an artist-exported glTF character (body, head, held item) is non-manifold in
    /// exactly the same way. Kept identical to `examples/fracture_cube.rs` so the example and the test
    /// are measuring one thing.
    fn torso_and_head() -> [Mesh; 2] {
        [Mesh::from(Cuboid::new(0.6, 1.0, 0.35)), Mesh::from(Cuboid::new(0.34, 0.34, 0.34))]
    }

    /// The fracture `examples/fracture_cube.rs` runs, to the digit.
    fn torso_and_head_fracture(parts: &[Mesh; 2]) -> Vec<FragmentGeometry> {
        let placed = [
            (&parts[0], Mat4::IDENTITY),
            (&parts[1], Mat4::from_translation(Vec3::new(0.0, 0.67, 0.0))),
        ];
        // `extent` is the merged solid's largest bounding half-dimension; `MIN_FRACTION` is 0.15.
        fracture_mesh(&placed, 12, 0.67 * 0.15, 0x00C0_FFEE, None)
    }

    /// **AG-006 — a fan can fold with every counter reading zero.**
    ///
    /// `docs/isomesh-upstream-asks.md` §5 offered upstream a cheap exact fold detector:
    ///
    /// > Fold ⟺ mixed signs ⟺ `inconsistently_oriented_edges > 0`, given the mesh is welded first.
    ///
    /// **That equivalence needs two qualifiers, and this test is the one that supplies the second.**
    ///
    /// 1. It holds only for fans built with [`crate::soup::push_cap_tri`]'s per-triangle flip. The flip
    ///    is what converts "mixed signed area" into "two triangles traverse their shared spoke the same
    ///    way". A capper that wound its fan consistently and set normals some other way would fold
    ///    without ever tripping the counter.
    /// 2. **It requires the loop to reverse.** A fan apex *outside* a simply-connected loop puts some
    ///    triangles on each side, so the signs are mixed — that is the `u_prism` case. A loop that winds
    ///    around its own centroid **twice in the same direction** has no mixed signs at all: every
    ///    triangle agrees, the surface is consistently oriented, and the fan folds anyway.
    ///
    /// A pentagram is the minimal witness. Fanned from its centre it covers the inner pentagon twice,
    /// so the emitted area exceeds the star's true area **by exactly the inner pentagon's area** — and
    /// `inconsistently_oriented_edges`, `non_manifold_edges` and `non_manifold_vertices` are all zero.
    ///
    /// The consequence for the ask is not that the detector is useless — it is that it is a *sufficient*
    /// condition for a fold and not a necessary one, so §5 (self-intersection inside a fan) is worth
    /// more to us than the ask says, not less.
    #[test]
    fn known_defect_a_doubly_wound_fan_folds_with_every_counter_at_zero() {
        // Analytic, from the unit circumradius — written as formulas so they can be re-derived.
        let r: f32 = 1.0;
        // Five triangles from the centre, each spanning two pentagon steps: 2·(2π/5) = 4π/5.
        let fan_area = 5.0 * 0.5 * r * r * (4.0 * std::f32::consts::PI / 5.0).sin();
        // The inner pentagon's circumradius is R/φ², and it is the region the fan covers twice.
        let phi = (1.0 + 5.0f32.sqrt()) / 2.0;
        let inner_r = r / (phi * phi);
        let inner_pentagon = 2.5 * inner_r * inner_r * (2.0 * std::f32::consts::PI / 5.0).sin();
        let true_star_area = fan_area - inner_pentagon;

        let mut cap = Soup::default();
        cap_side(
            &pentagram_segments(),
            &Plane { point: Vec3::ZERO, normal: Vec3::Z },
            Vec3::Z,
            &mut cap,
        );
        assert!(!cap.is_empty(), "the pentagram loop produced no cap at all");

        let area = cap_area(&cap);
        assert!(
            (area - fan_area).abs() < 1.0e-3,
            "the fan emitted {area}, expected {fan_area}. The star's true area is {true_star_area}; the \
             excess {} is exactly the inner pentagon, covered a second time. If the capper was fixed, \
             change this to `assert!((area - {true_star_area}).abs() < 1e-3)`.",
            fan_area - true_star_area
        );
        assert!(
            area > true_star_area + 1.0e-3,
            "the fan no longer overshoots the pentagram — the fold is gone, so flip this test"
        );

        // The point of the whole ticket: the fold is real and every counter says the mesh is fine.
        let g = geometry_from_soup(&cap).expect("the cap draws something");
        let a = audit_fragment(&g).expect("the cap can be audited");
        let note = "AG-006: a doubly-wound fan folds *without* mixed signs, so this counter cannot see \
                    it. If this fires, the equivalence in docs/isomesh-upstream-asks.md §5 may have \
                    become true and that document needs revisiting, not just this number.";
        assert_eq!(a.inconsistently_oriented_edges, 0, "{note} Audit: {a:?}");
        assert_eq!(a.non_manifold_edges, 0, "{note} Audit: {a:?}");
        assert_eq!(a.non_manifold_vertices, 0, "{note} Audit: {a:?}");
    }

    /// **AG-002 — a cut through a hollow fills the bore. Asserts the bug is still here.**
    ///
    /// One plane through [`hollow_prism`] produces a cut boundary of **two nested loops** — the outer
    /// rim and the bore rim. `assemble_loops` returns them as two independent loops and `cap_side` fans
    /// each one *solid*, with no notion that one lies inside the other. The outer fan therefore paves
    /// the entire outer square, bore included, and the bore's own fan paves the bore a second time.
    ///
    /// # The pre-registered prediction was mostly wrong, and that is recorded rather than quietly fixed
    ///
    /// AG-002 predicted the capper would "conserve χ and manifoldness while overstating volume by
    /// exactly (bore cross-section area × length)", and that *"every `MeshReport` field reports it
    /// healthy and only volume notices."* Measured, across 24 configurations (two depths × two bore
    /// areas × four cut heights × both sides):
    ///
    /// - **χ is not conserved.** A correctly cut piece is still a tube: genus 1, χ = 0. Every emitted
    ///   piece reports **χ = 2**. Filling the bore is precisely a genus reduction, so χ sees it.
    /// - **`inconsistently_oriented_edges` is 8, never 0**, so `supports_inside_outside` is false. Two
    ///   fields notice, not zero fields.
    /// - **Manifoldness is conserved** — `non_manifold_edges` and `non_manifold_vertices` stay 0. That
    ///   half of the prediction held.
    /// - **Volume is the field that does *not* notice.** Cut this fixture through the origin and
    ///   [`soup_volume`] of the emitted piece is `8.0` — exactly right. The bore is paved twice with
    ///   opposite-facing... no: with *same*-facing sheets whose flux cancels against the rim walls.
    ///
    /// The audit's volume *does* differ from the truth, by an exact `bore_area × length / 3` in all 24
    /// cases — but **that is an artefact of recentring, not a measurement of the defect.**
    /// `geometry_from_soup` recentres each fragment on its bbox, and translation only preserves the
    /// divergence-theorem sum for a surface that is closed *and consistently oriented*. This one is not.
    /// The doc on [`FragmentAudit::signed_volume`] claimed "recentering does not change it"; that was
    /// corrected in this commit, because it is false for exactly the surfaces the field exists to judge.
    ///
    /// # What this test asserts instead
    ///
    /// **Cap area**, which is translation-invariant and checkable by hand: the emitted cap is
    /// `outer + bore` where the truth is `outer − bore`, so it is over by exactly `2 × bore`. AG-008's
    /// flood fill over a PSLG is what fixes it, and when it does, flip each assertion as its message
    /// says.
    #[test]
    fn known_defect_nested_cut_boundary_is_filled_solid() {
        const OUTER_AREA: f32 = 9.0; // the 3×3 outer square
        let whole = hollow_prism(1.0, 0.5);

        // The fixture must itself be the solid it claims to be, or nothing below means anything.
        let wg = geometry_from_soup(&whole).expect("the fixture draws something");
        let wa = audit_fragment(&wg).expect("the fixture can be audited");
        assert!(wa.is_closed() && wa.is_manifold(), "the hollow-prism fixture is not a closed manifold: {wa:?}");
        assert_eq!(wa.euler_characteristic, 0, "the fixture should be genus 1 (χ = 0): {wa:?}");
        assert_eq!(wa.genus, Some(1), "the fixture should have exactly one hole: {wa:?}");
        assert!(
            (wa.signed_volume - BORE_SECTION_AREA * 2.0).abs() < 1.0e-3,
            "the fixture encloses {}, but a 3×3 prism of depth 2 less a 1×1 bore is {}",
            wa.signed_volume,
            BORE_SECTION_AREA * 2.0
        );

        // One cut, perpendicular to the extrusion, so the cross-section is the annulus itself.
        let (above, _) = split_soup(&whole, &Plane { point: Vec3::ZERO, normal: Vec3::Z });
        assert!(!above.is_empty(), "the hollow-prism fixture did not cut");

        // The primary assertion: the bore is paved rather than punched out.
        let area = cap_area(&above);
        assert!(
            (area - (OUTER_AREA + BORE_HOLE_AREA)).abs() < 1.0e-3,
            "cap area is {area}; the fan paves the outer square ({OUTER_AREA}) and then paves the bore \
             again ({BORE_HOLE_AREA}), for {}. The true annulus is {}. If the capper was fixed, change \
             this to `assert!((area - {}).abs() < 1e-3)`.",
            OUTER_AREA + BORE_HOLE_AREA,
            BORE_SECTION_AREA,
            BORE_SECTION_AREA
        );

        let a = audit_fragment(&geometry_from_soup(&above).expect("the cut piece draws"))
            .expect("the cut piece can be audited");
        assert_eq!(
            a.euler_characteristic, 2,
            "χ is no longer 2. A correctly cut piece of a tube is still a tube — genus 1, χ = 0 — so if \
             the capper was fixed, change this to `assert_eq!(a.euler_characteristic, 0)` and assert \
             `genus == Some(1)`. Audit: {a:?}"
        );
        assert!(
            a.inconsistently_oriented_edges > 0,
            "the filled bore no longer shows as inconsistent edge orientation. If the capper was fixed, \
             change this to `assert_eq!(a.inconsistently_oriented_edges, 0)`. Audit: {a:?}"
        );
        assert!(
            !a.supports_inside_outside,
            "the piece is now solid enough for inside/outside queries; flip this assertion. Audit: {a:?}"
        );
        // Manifoldness and closedness survive — the half of AG-002's prediction that held.
        assert!(a.is_closed(), "the filled bore should still leave a closed surface: {a:?}");
        assert!(a.is_manifold(), "the filled bore should not create non-manifold edges: {a:?}");

        // **The falsified half, pinned so it cannot be quietly re-asserted.** Volume, measured where the
        // fragment actually sits, is *correct* — so `signed_volume` is not the field that catches this.
        let raw = soup_volume(&above);
        assert!(
            (raw - BORE_SECTION_AREA).abs() < 1.0e-3,
            "un-recentred volume is {raw}, expected {BORE_SECTION_AREA}. AG-002 predicted volume would \
             be the field that notices a filled bore; measured, it is not — the paving cancels against \
             the rim walls. If this ever fails, the *prediction* may have become true and this comment \
             is what needs revisiting, not just the number."
        );
    }

    /// **AG-012 — a baseline, not a target. Read the second paragraph before you touch it.**
    ///
    /// These four numbers are the ones the whole architectural argument in `BACKLOG.md` rests on, and
    /// until this test they were asserted by *nothing*: they were runtime output of an example and
    /// prose in `docs/research-brief.md`. Regressing the fracture to 3 of 12 watertight and 40 open cut
    /// edges would have left every test green and only made the prose quietly wrong. The one fixture CI
    /// actually locked was the convex `Cuboid` — precisely the case that was never broken.
    ///
    /// **This test is expected to fail when Phase 1 lands, and that failure is the deliverable.**
    /// AG-001 pre-registers 12/12 closed and 0 open cut edges on the *proxy*. A prediction measured
    /// against an unpinned baseline cannot be falsified in the direction that matters, so this pins
    /// where we started. AG-004 retires it: once the proxy and the render mesh are audited separately,
    /// asserting a closed-solid test on a render mesh stops being meaningful. Until then, **do not
    /// re-bless these numbers to make a red test green** — if they moved, say which change moved them.
    ///
    /// The counts are computed exactly as `examples/fracture_cube.rs` prints them, so the two cannot
    /// drift apart.
    #[test]
    fn known_baseline_torso_and_head_is_mostly_not_solid() {
        let parts = torso_and_head();
        let pieces = torso_and_head_fracture(&parts);
        assert_eq!(pieces.len(), 12, "the baseline is 12 fragments; the fracture returned a different count");

        let audits = audit_fragments(&pieces);
        // `audit_fragments` silently omits anything it cannot measure. If it ever does, every count
        // below is taken over a smaller population and the comparison is meaningless.
        assert_eq!(audits.len(), 12, "a fragment could not be audited, so these counts are not comparable");

        let watertight = audits.iter().filter(|a| a.is_closed()).count();
        let manifold = audits.iter().filter(|a| a.is_manifold()).count();
        let collider_ready = audits.iter().filter(|a| a.supports_inside_outside).count();
        let open_edges: u64 = audits.iter().map(|a| a.boundary_edges).sum();
        let bowties = audits.iter().filter(|a| a.non_manifold_vertices > 0).count();

        let note = "AG-012 baseline moved. This is not a test to re-bless — name the change that moved \
                    it, in the commit. AG-001 is the ticket allowed to move it, and AG-004 retires it.";
        assert_eq!(watertight, 7, "watertight fragments: {note}");
        assert_eq!(manifold, 2, "manifold fragments: {note}");
        assert_eq!(open_edges, 22, "total open cut edges: {note}");
        // **Re-blessed once, by AG-013, and the reason is on the record.** This read `4` under isomesh
        // `4369e3c`, whose `supports_inside_outside` checked boundary edges, non-manifold *edges* and
        // orientation — but not non-manifold *vertices*. A bowtie vertex breaks the pseudonormal
        // construction exactly as an edge does, so the old 4 was an overcount and this crate published
        // it. `22c3b35` adds the missing clause and the honest figure is 1.
        assert_eq!(collider_ready, 1, "collider-ready fragments: {note}");
        // Pinned because it is what explains the line above: ten of twelve fragments carry a bowtie,
        // which is the seam between two closed shells showing up as a vertex fault.
        assert_eq!(bowties, 10, "fragments with non-manifold vertices: {note}");
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
    /// # That detector is sufficient, not necessary — scope it before quoting it
    ///
    /// It is tempting to read the paragraph above as *fold ⟺ `inconsistently_oriented_edges > 0`*, and
    /// `docs/isomesh-upstream-asks.md` §5 did offer it upstream in that form. **Two qualifiers, both
    /// load-bearing:**
    ///
    /// 1. It is specific to `push_cap_tri`'s **per-triangle** flip. That flip is the mechanism turning
    ///    mixed signed area into a shared spoke traversed twice the same way. A capper that wound its
    ///    fan consistently would fold without tripping the counter.
    /// 2. **It needs the loop to reverse.** This fixture's apex falls outside a simply-connected loop,
    ///    which puts triangles on both sides and mixes the signs. A loop that winds around its centroid
    ///    *twice in the same direction* mixes nothing — see
    ///    `known_defect_a_doubly_wound_fan_folds_with_every_counter_at_zero`, where a pentagram folds
    ///    with every counter at zero.
    ///
    /// # A related observation, and the half of it that turned out to be false
    ///
    /// `cap_side`'s apex is a plain **vertex average, not an area centroid**, so it is already pulled
    /// toward whichever part of the loop carries the most vertices rather than the most area. That is
    /// true, and it is not worth fixing, because AG-001 removes non-convex sections entirely.
    ///
    /// The backlog paired that with a claim that `assemble_loops` duplicates each loop's first vertex
    /// at the end, double-weighting it. **It does not.** `loop_v` starts as `vec![s0, s1]` and the walk
    /// breaks on `cur == s0` *before* pushing it again, so every vertex appears exactly once — which is
    /// also why `cap_side` closes the fan with `lp[(k + 1) % n]`. A duplicated first vertex would make
    /// that modulo wrap emit a degenerate final triangle.
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

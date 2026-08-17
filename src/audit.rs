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
pub struct SolidAudit {
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
/// Meaningless for an open surface — read it only alongside [`SolidAudit::is_closed`]. Computed on
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

impl SolidAudit {
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

/// Measure a fragment's **drawn surface** — the subject's own skin plus the cut face, welded.
///
/// **Returns counts, not verdicts, and that is the whole point of the type.** A render fragment is a
/// *subset* of the subject's surface: it is open because a subset is open, and asking whether it is
/// watertight is a category error rather than a hard question. [`SurfaceReport`] therefore has no
/// `is_closed`, no `supports_inside_outside` and no volume — there is nothing for them to mean.
///
/// For the closed artefact, ask [`audit_proxy`].
///
/// # What this cost before the type existed
///
/// The crate had one audit and pointed it at the drawn surface, so `examples/fracture_cube.rs`
/// reported "2 of 12 manifold" and it read as a defect. Part of that was the slicer genuinely failing
/// on a non-convex section; the rest was a closed-solid test applied to something that is not a solid.
/// Splitting the types is what makes the second half impossible to report again.
///
/// # Errors
///
/// A `String` describing why the fragment could not be measured — it carried no drawable triangles, or
/// the weld rejected its epsilon. Both are loud rather than silent, because an audit that quietly
/// reported "no violations" for a fragment it never looked at is worse than no audit.
pub fn audit_render(frag: &FragmentGeometry) -> Result<SurfaceReport, String> {
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
    Ok(SurfaceReport {
        triangles: report.faces,
        vertices_before_weld,
        vertices_after_weld: buf.positions.len() as u64,
        open_edges: report.boundary_edges,
        non_manifold_edges: report.non_manifold_edges,
        non_manifold_vertices: report.non_manifold_vertices,
        inconsistently_oriented_edges: report.inconsistently_oriented_edges,
    })
}

/// What a fragment's **drawn surface** turned out to be. Every field is a count to be *recorded*.
///
/// Deliberately missing: any predicate about closure, solidity or volume. Those are properties of a
/// solid, and this is not one — [`SolidAudit`] is. See [`audit_render`].
#[derive(Clone, Debug, PartialEq)]
pub struct SurfaceReport {
    /// Triangles the validator considered.
    pub triangles: u64,
    /// Vertices as shipped, before the audit's weld.
    pub vertices_before_weld: u64,
    /// Vertices after welding coincident positions.
    pub vertices_after_weld: u64,
    /// Edges incident to exactly one face.
    ///
    /// **Record this; never assert it is zero.** A nonzero count is expected — the skin ends where the
    /// cut begins. What it is good for is *tracking*: a count that jumps after a change to the clipper
    /// means the skin and the cap stopped meeting cleanly, which is a real regression even though no
    /// absolute value is the correct one.
    pub open_edges: u64,
    /// Edges incident to three or more faces.
    ///
    /// **Small nonzero counts are normal and were measured before this doc was written**: a lone
    /// cuboid cut into 8 pieces reports 3, because the audit's position-only weld merges vertices that
    /// the shipped mesh keeps apart. Like [`Self::open_edges`], track it rather than asserting on it.
    pub non_manifold_edges: u64,
    /// Vertices whose incident faces do not form a single fan.
    pub non_manifold_vertices: u64,
    /// Edges whose two faces traverse them the same way, i.e. one is inside out.
    ///
    /// **This one is zero for a single closed shell** — clipping preserves winding, and the cuboid
    /// above measures 0. It goes nonzero where *two shells meet*: a torso and a head that touch weld
    /// their coincident faces together, and those interior faces disagree with their neighbours about
    /// which way is out. That is a property of the subject, not of the fracture; see `AG-003`.
    pub inconsistently_oriented_edges: u64,
}

/// Weld, validate and reinterpret one assembled buffer.
///
/// Split out so the *proxy* can be measured by exactly the same path as the drawn surface — two tiers,
/// one definition of what the numbers mean.
pub(crate) fn audit_buffer(mut buf: MeshBuffer<f32>) -> Result<SolidAudit, String> {
    let vertices_before_weld = buf.positions.len() as u64;
    let report = weld_then_validate(&mut buf)?;
    let readiness = collider::from_report(&report);

    Ok(SolidAudit {
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


/// Audit a fragment **as the solid it is** — its proxy cell, every face, closed.
///
/// **The companion to [`audit_render`], and choosing between them is not a matter of taste.** A
/// fragment is two artefacts and only one of them is a solid:
///
/// | artefact | what it is | what may be asserted |
/// |---|---|---|
/// | [`FragmentGeometry::cell`] | a closed convex polyhedron | χ = 2, manifold, watertight, volume |
/// | [`FragmentGeometry::outer`] | a subset of the subject's own surface | **nothing about closure** |
///
/// Applying a closed-solid test to the render mesh is a category error: it is open because a surface
/// subset *is* open, not because anything went wrong. Before Tier A the crate had only one audit and
/// pointed it at the drawn surface, which is why "2 of 12 manifold" read as a defect rather than as a
/// measurement of the wrong thing.
pub fn audit_proxy(frag: &FragmentGeometry) -> Result<SolidAudit, String> {
    let soup = crate::mesh::proxy_soup(&frag.cell);
    let mesh = crate::mesh::soup_to_mesh_all_faces(&soup)?;
    let mut buf: MeshBuffer<f32> = MeshBuffer::new();
    if !append(&mut buf, &mesh) {
        return Err("the proxy cell produced no auditable triangles".to_string());
    }
    audit_buffer(buf)
}

/// Audit every fragment of a fracture. Fragments that cannot be measured are `warn!`-skipped and
/// omitted, so the returned length may be shorter than `frags` — deliberately, because padding the
/// result with a fabricated clean audit is exactly the lie this module exists to stop telling.
#[must_use]
pub fn audit_proxies(frags: &[FragmentGeometry]) -> Vec<SolidAudit> {
    frags
        .iter()
        .enumerate()
        .filter_map(|(i, f)| match audit_proxy(f) {
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
    use crate::CutSettings;
    use crate::mesh::fracture_mesh;
    use crate::proxy::ProxyCell;
    use bevy::math::{Mat4, Vec3, primitives::Cuboid};
    use isomesh::validate::check_determinism;
    /// A cuboid and the single convex cell that is exactly its own shape.
    ///
    /// The proxy being *exact* here is the point: it isolates the cutter. Any defect this fixture shows
    /// is the fracture's, not an artefact of a proxy that approximates the mesh.
    fn cube_parts() -> (Mesh, Vec<ProxyCell>) {
        (
            Mesh::from(Cuboid::new(1.0, 2.0, 1.0)),
            vec![ProxyCell::from_box(Vec3::ZERO, Vec3::new(0.5, 1.0, 0.5))],
        )
    }

    /// The two-shell subject: a torso box and a head box that overlap at the neck, plus **one proxy
    /// cell per shell**.
    ///
    /// **This is the honest case, not a rigged one.** Two closed shells that meet are not a manifold at
    /// the seam, and an artist-exported glTF character (body, head, held item) is non-manifold in
    /// exactly the same way. Kept identical to `examples/fracture_cube.rs` so the example and the test
    /// measure one thing. The cells are never unioned — that is what keeps a head separable from a
    /// torso.
    fn torso_and_head() -> ([Mesh; 2], Vec<ProxyCell>) {
        (
            [Mesh::from(Cuboid::new(0.6, 1.0, 0.35)), Mesh::from(Cuboid::new(0.34, 0.34, 0.34))],
            vec![
                ProxyCell::from_box(Vec3::ZERO, Vec3::new(0.3, 0.5, 0.175)),
                ProxyCell::from_box(Vec3::new(0.0, 0.67, 0.0), Vec3::splat(0.17)),
            ],
        )
    }

    /// Slack enough never to bind at any target these tests ask for, so nothing here is quietly
    /// measuring the depth bound instead of what it names.


    /// The fracture `examples/fracture_cube.rs` runs, to the digit — read at its finest frontier,
    /// which is the set this fixture measured before the bake kept a hierarchy.
    fn torso_and_head_fracture(parts: &[Mesh; 2], proxy: &[ProxyCell]) -> Vec<FragmentGeometry> {
        let placed = [
            (&parts[0], Mat4::IDENTITY),
            (&parts[1], Mat4::from_translation(Vec3::new(0.0, 0.67, 0.0))),
        ];
        fracture_mesh(&placed, proxy, &CutSettings::new(12, 0.15, 0x00C0_FFEE)).into_leaves()
    }

    /// **Watertightness across seeds, not at one lucky seed.**
    ///
    /// `every_proxy_fragment_of_a_closed_solid_is_closed` pins one seed, and that is exactly how this
    /// defect hid: sweeping 40 seeds found **one fragment in 320** coming back with
    /// `boundary_edges != 0`. The cause was a face too small to draw — repeated cutting leaves
    /// slivers whose vertices sit just past the weld — which `append_cut_faces` then dropped as
    /// zero-area, opening a cell the architecture proves is closed. `CellBuilder` now collapses such
    /// a face instead of shipping it; see `collapse_undrawable_faces`.
    ///
    /// Kept as a sweep rather than a pinned seed **because a pinned seed is what missed it**. The
    /// jitter levels matter too: an off-centre plane lands near an existing vertex more often, so a
    /// bake with `plane_jitter` at zero is the easy case and the ones below it are the real test.
    #[test]
    fn every_fragment_is_closed_at_every_seed_and_jitter() {
        let (cube, proxy) = cube_parts();
        for (jitter, size_spread) in [(0.0f32, 0.0f32), (0.35, 0.5), (0.6, 1.0)] {
            for seed in 0..60u32 {
                let cut = CutSettings {
                    plane_jitter: jitter,
                    size_spread,
                    ..CutSettings::new(10, 0.04, seed.wrapping_mul(2_654_435_761))
                };
                let pieces = fracture_mesh(&[(&cube, Mat4::IDENTITY)], &proxy, &cut).into_leaves();
                assert!(!pieces.is_empty(), "jitter {jitter}, seed {seed}: produced nothing");
                for (i, a) in audit_proxies(&pieces).into_iter().enumerate() {
                    assert_eq!(
                        a.boundary_edges, 0,
                        "jitter {jitter}, seed {seed}, fragment {i}: open cut — {a:?}"
                    );
                    assert!(
                        a.supports_inside_outside,
                        "jitter {jitter}, seed {seed}, fragment {i}: not a solid — {a:?}"
                    );
                }
            }
        }
    }

    /// **The dials that answer "it looks like a frozen statue that shattered".**
    ///
    /// Always cutting the largest piece through its own centroid halves it, and halving every piece
    /// every time drives fragment volumes toward each other — which is the uniform-shard read.
    /// Sellán et al. name the symptom directly: geometric prefracture "results in recognizable,
    /// unrealistic pieces". Real brittle fragments follow Mott's distribution, many small and few
    /// large, so the useful measure is how far apart the largest and smallest end up.
    ///
    /// Measured over 200 seeds, largest/smallest fragment volume, median:
    ///
    /// | `plane_jitter` | `size_spread` | ratio |
    /// |---|---|---|
    /// | 0.0 | 0.0 | ~2.5 |
    /// | 0.35 | 0.5 (the defaults) | ~4.1 |
    /// | 0.6 | 0.8 | ~10.6 |
    ///
    /// This test pins the *ordering*, not those numbers: the dials must widen the spread, and the
    /// defaults must widen it meaningfully over an unjittered bake. Pinning the ratios themselves
    /// would re-bless on any change to the cut sequence, which is not what is being claimed here.
    #[test]
    fn the_shape_dials_widen_the_fragment_size_spread() {
        let (cube, proxy) = cube_parts();
        let median_ratio = |jitter: f32, size_spread: f32| -> f32 {
            let mut ratios: Vec<f32> = Vec::new();
            for seed in 0..60u32 {
                let cut = CutSettings {
                    plane_jitter: jitter,
                    size_spread,
                    ..CutSettings::new(12, 0.04, seed.wrapping_mul(2_654_435_761))
                };
                let pieces = fracture_mesh(&[(&cube, Mat4::IDENTITY)], &proxy, &cut).into_leaves();
                let mut v: Vec<f32> = pieces.iter().map(|f| f.cell.volume()).collect();
                // SORT-OK: `total_cmp` over volumes; only the extremes are read, so ties are moot.
                v.sort_by(|a, b| a.total_cmp(b));
                if v.len() >= 4 && v[0] > 0.0 {
                    ratios.push(v[v.len() - 1] / v[0]);
                }
            }
            ratios.sort_by(|a, b| a.total_cmp(b));
            ratios[ratios.len() / 2]
        };

        let flat = median_ratio(0.0, 0.0);
        let default = median_ratio(0.35, 0.5);
        let wide = median_ratio(0.6, 0.8);
        assert!(
            default > flat * 1.3,
            "the shipped defaults must visibly widen the spread: {flat:.2} -> {default:.2}"
        );
        assert!(wide > default, "and turning them up must widen it further: {default:.2} -> {wide:.2}");
    }

    /// **AG-005 — the shipped mesh shares vertices, and still keeps its creases.**
    ///
    /// Before this, `Soup::push_tri` allocated three fresh vertices per triangle and `soup_to_mesh`'s
    /// remap keyed on the old soup index — unique per corner by construction — so it compacted the
    /// buffer and merged nothing. Fragments shipped at exactly 3.0 vertices per triangle.
    ///
    /// The two numbers below are a pair, and the gap between them is the point. The **shipped** count
    /// comes from the composite key (position class + quantised normal + quantised UV). The
    /// **audit-welded** count comes from a position-only weld, which is what a naive fix would have
    /// shipped. That the first is materially larger than the second is the crease survivingly intact:
    /// every vertex in the gap is a corner where the skin meets a cut face, or one cut face meets
    /// another, and merging it would smear the entire visual read this crate exists to produce.
    #[test]
    fn the_shipped_mesh_shares_vertices_without_smearing_creases() {
        let (parts, proxy) = torso_and_head();
        let pieces = torso_and_head_fracture(&parts, &proxy);
        let reports: Vec<_> = pieces.iter().filter_map(|p| audit_render(p).ok()).collect();
        assert_eq!(reports.len(), 12, "every fragment should be measurable");

        let shipped: u64 = reports.iter().map(|r| r.vertices_before_weld).sum();
        let position_only: u64 = reports.iter().map(|r| r.vertices_after_weld).sum();
        let triangles: u64 = reports.iter().map(|r| r.triangles).sum();
        let per_tri = shipped as f64 / triangles as f64;

        assert!(
            per_tri < 2.0,
            "shipped {per_tri:.2} vertices per triangle over {triangles} triangles — the weld merged \
             little or nothing. It was 3.00 before AG-005; anything at or near 3 means the composite \
             key stopped matching."
        );
        assert!(
            shipped > position_only,
            "the composite key merged as hard as a position-only weld ({shipped} vs {position_only}), \
             which means creases are being smeared — exactly what the normal and UV terms exist to stop"
        );
    }

    /// A single-quad "cape" hanging off the torso's back — an **open shell**: two triangles, four
    /// boundary edges, no interior whatsoever.
    fn cape() -> Mesh {
        let mut m = Mesh::new(
            bevy::mesh::PrimitiveTopology::TriangleList,
            bevy::asset::RenderAssetUsages::default(),
        );
        // Inside the torso's cell (z spans ±0.175), and deliberately **not** on any box face:
        // the torso's back is at -0.175 and the head's at -0.17, so -0.16 belongs to the cape alone.
        let z = -0.16f32;
        m.insert_attribute(
            Mesh::ATTRIBUTE_POSITION,
            vec![[-0.25, -0.4, z], [0.25, -0.4, z], [0.25, 0.4, z], [-0.25, 0.4, z]],
        );
        m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 0.0, -1.0]; 4]);
        m.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]);
        m.insert_indices(bevy::mesh::Indices::U32(vec![0, 1, 2, 0, 2, 3]));
        m
    }

    /// **AG-003 — a sheet is carried, never cut.**
    ///
    /// A cape, a hair card or a decal has no interior. A plane through one does not divide a volume;
    /// it separates geometry the artist drew as continuous, and the old capper would additionally try
    /// to close the "cut" and emit a degenerate solid. Under Tier A/B the danger changed shape but did
    /// not go away: nothing caps the render mesh any more, but a sheet would still be *clipped* into
    /// pieces by every plane crossing it.
    ///
    /// So the cape must come out of the fracture whole, in exactly one fragment, with its two triangles
    /// intact.
    #[test]
    fn an_open_shell_survives_the_fracture_whole() {
        let (parts, proxy) = torso_and_head();
        let cape = cape();
        let placed = [
            (&parts[0], Mat4::IDENTITY),
            (&parts[1], Mat4::from_translation(Vec3::new(0.0, 0.67, 0.0))),
            (&cape, Mat4::IDENTITY),
        ];
        let pieces = fracture_mesh(&placed, &proxy, &CutSettings::new(12, 0.15, 0x00C0_FFEE)).into_leaves();
        assert!(!pieces.is_empty(), "the subject did not fracture");

        // The cape's triangles are the only ones with a -Z normal at z = -0.17, and they are
        // recognisable by area: two triangles of 0.5 * 0.5 * 0.8 = 0.2 each.
        let mut holders = 0usize;
        let mut cape_tris = 0usize;
        for p in &pieces {
            let Some(outer) = p.outer.as_ref() else { continue };
            let n = cape_triangles(outer, p.center_local);
            if n > 0 {
                holders += 1;
                cape_tris += n;
            }
        }
        assert_eq!(holders, 1, "the cape was split across {holders} fragments; it must ride on exactly one");
        assert_eq!(cape_tris, 2, "the cape should arrive with both its triangles, got {cape_tris}");
    }

    /// Count triangles that lie in the cape's plane, undivided.
    fn cape_triangles(mesh: &Mesh, recenter: Vec3) -> usize {
        let Some(bevy::mesh::VertexAttributeValues::Float32x3(pos)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            return 0;
        };
        let Some(idx) = mesh.indices() else { return 0 };
        let v: Vec<u32> = idx.iter().map(|i| i as u32).collect();
        v.chunks_exact(3)
            .filter(|t| {
                t.iter().all(|&i| {
                    let p = pos[i as usize];
                    (p[2] + recenter.z + 0.16).abs() < 1.0e-4
                })
            })
            .count()
    }

    /// **The crate's central promise, tested against every byte.**
    ///
    /// `check_determinism` runs the closure three times — twice into fresh buffers, once into a buffer
    /// that was used, `reset()` and used again — and compares every position, normal and index under
    /// IEEE `totalOrder`, so `+0.0`/`-0.0` and `NaN` are distinguished rather than papered over.
    #[test]
    fn fracture_output_is_bit_identical_across_runs() {
        let (cube, proxy) = cube_parts();
        let report = check_determinism(|out: &mut MeshBuffer<f32>| {
            let pieces = fracture_mesh(&[(&cube, Mat4::IDENTITY)], &proxy, &CutSettings::new(8, 0.05, 0xC0FF_EE00)).into_leaves();
            for p in &pieces {
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

    /// **A theorem, not a measurement: every piece of a plane-cut convex solid is a closed solid.**
    ///
    /// Under Tier A this is asserted on the **proxy**, which is what makes it a theorem. A plane
    /// through a convex polyhedron yields two convex polyhedra; there is no input for which this can
    /// fail, so a failure here means the cell clipper is wrong, not that the subject was awkward.
    #[test]
    fn every_proxy_fragment_of_a_closed_solid_is_closed() {
        let (cube, proxy) = cube_parts();
        let pieces = fracture_mesh(&[(&cube, Mat4::IDENTITY)], &proxy, &CutSettings::new(8, 0.05, 0x5EED)).into_leaves();
        assert!(pieces.len() >= 2, "expected the cube to break, got {}", pieces.len());
        for (i, p) in pieces.iter().enumerate() {
            let a = crate::audit::audit_proxy(p).unwrap_or_else(|e| panic!("proxy {i} could not be audited: {e}"));
            assert_eq!(a.boundary_edges, 0, "proxy {i} has an open cut: {a:?}");
            assert!(a.is_manifold(), "proxy {i} is not a manifold: {a:?}");
            assert_eq!(a.inconsistently_oriented_edges, 0, "proxy {i} has an inside-out face: {a:?}");
            assert_eq!(a.euler_characteristic, 2, "proxy {i} is not a topological sphere: {a:?}");
            assert!(a.supports_inside_outside, "proxy {i} is not solid enough for a collider: {a:?}");
        }
    }

    /// The fracture neither gains nor loses solid.
    #[test]
    fn fracture_conserves_volume() {
        let (cube, proxy) = cube_parts();
        let pieces = fracture_mesh(&[(&cube, Mat4::IDENTITY)], &proxy, &CutSettings::new(8, 0.05, 0x5EED)).into_leaves();
        let total: f32 = pieces.iter().filter_map(|p| crate::audit::audit_proxy(p).ok()).map(|a| a.signed_volume).sum();
        assert!(
            (total - 2.0).abs() < 1.0e-3,
            "fragments enclose {total}, but the source cube encloses 2.0 — the fracture gained or lost solid"
        );
    }

    /// **AG-001's pre-registered prediction, recorded against the outcome.**
    ///
    /// The soup cutter scored 7 of 12 watertight and 2 of 12 manifold on this fixture, with 22 open cut
    /// edges, and `AG-012` pinned those numbers so this comparison would be possible. The prediction
    /// written before the rewrite was: **12/12 proxy fragments closed, manifold, χ = 2, volume
    /// conserved to 1e-3, and 0 open cut edges** — identical to the convex cuboid, because under Tier A
    /// every cross-section *is* convex.
    ///
    /// The secondary prediction was stated up front so the first run would not read as a failure: the
    /// **render** fragments still carry open edges, and that is correct behaviour rather than a
    /// regression — a render fragment is a surface subset, not a solid. It is measured here and
    /// deliberately not asserted on; see `AG-004`.
    #[test]
    fn every_proxy_fragment_of_the_two_shell_subject_is_closed() {
        let (parts, proxy) = torso_and_head();
        let pieces = torso_and_head_fracture(&parts, &proxy);
        assert_eq!(pieces.len(), 12, "expected 12 fragments, got {}", pieces.len());

        let mut volume = 0.0f32;
        for (i, p) in pieces.iter().enumerate() {
            let a = crate::audit::audit_proxy(p).unwrap_or_else(|e| panic!("proxy {i} could not be audited: {e}"));
            assert_eq!(a.boundary_edges, 0, "proxy {i} has an open cut: {a:?}");
            assert!(a.is_manifold(), "proxy {i} is not a manifold: {a:?}");
            assert_eq!(a.euler_characteristic, 2, "proxy {i} is not a topological sphere: {a:?}");
            assert_eq!(a.inconsistently_oriented_edges, 0, "proxy {i} has an inside-out face: {a:?}");
            assert!(a.supports_inside_outside, "proxy {i} is not collider-ready: {a:?}");
            volume += a.signed_volume;
        }
        // Torso 0.6×1.0×0.35 and head 0.34³ as separate cells — never unioned, so the overlap at the
        // neck is counted once per shell exactly as the caller's decomposition describes it.
        let expected = 0.6 * 1.0 * 0.35 + 0.34 * 0.34 * 0.34;
        assert!(
            (volume - expected).abs() < 1.0e-3,
            "proxy fragments enclose {volume}, the two cells enclose {expected}"
        );
    }

    /// The audit's weld is doing something, or every count above describes an unwelded soup.
    #[test]
    fn the_audit_welds_before_it_measures() {
        let (cube, proxy) = cube_parts();
        let pieces = fracture_mesh(&[(&cube, Mat4::IDENTITY)], &proxy, &CutSettings::new(4, 0.05, 1)).into_leaves();
        let a = audit_render(&pieces[0]).expect("the first fragment can be audited");
        assert!(
            a.vertices_after_weld < a.vertices_before_weld,
            "the weld merged nothing ({} -> {}), so the topology counts describe an unwelded soup and mean nothing",
            a.vertices_before_weld,
            a.vertices_after_weld
        );
    }
}

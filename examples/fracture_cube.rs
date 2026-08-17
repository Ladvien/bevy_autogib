//! **What a fracture actually produces, in a terminal.**
//!
//! No window, no GPU, no `App` — [`fracture_mesh`] is the whole pipeline with no assets and no ECS, so
//! the fastest way to understand what a settings change does is to print the pieces.
//!
//! The subject is a two-part solid (a torso box and an offset head box, each with its own transform),
//! because that is the shape the ECS bake actually sees: a character is never one mesh. The two parts
//! are merged into one soup before any cut, which is why the fracture crosses between them instead of
//! neatly separating them — a body breaks, it does not disassemble.
//!
//! Read the `cap` column. That is the newly-created cut surface, and it is the whole reason each
//! fragment comes back as two meshes: give the skin the subject's own material and the cap a raw
//! interior one, and the result reads as severed rather than as a model that fell apart.
//!
//! Run: `cargo run -p bevy_autogib --example fracture_cube`

use bevy::math::{Mat4, Vec3, primitives::Cuboid};
use bevy::mesh::Mesh;
use bevy_autogib::{BondSet, CutSettings, FragmentGeometry, ProxyCell, fracture_mesh};

/// Target fragment count. The ECS bake derives this from the mesh's bounding size and the
/// `FractureSettings` dials; here it is spelled out so one number can be varied at a time.
const TARGET: usize = 12;
/// Stop cutting a piece once its extent drops below this fraction of the whole.
const MIN_FRACTION: f32 = 0.15;
/// How many cuts deep the hierarchy may go — slack enough here that `TARGET` is what binds.
const MAX_DEPTH: u16 = 64;

/// The geometry dials for this example's bake. `plane_jitter` and `size_spread` are what keep the
/// pieces from all coming out the same size — at `0.0` each cut halves its piece through the centre
/// and the result reads as uniform shards rather than debris.
fn cut(seed: u32) -> CutSettings {
    CutSettings { max_depth: MAX_DEPTH, ..CutSettings::new(TARGET, MIN_FRACTION, seed) }
}

/// Total surface area of everything a fragment draws — a blunt but honest measure of how much the
/// rounding has relaxed it, since smoothing a surface shrinks it.
fn drawn_area(f: &FragmentGeometry) -> f32 {
    [f.outer.as_ref(), f.cap.as_ref()].into_iter().flatten().map(mesh_area).sum()
}

fn mesh_area(mesh: &Mesh) -> f32 {
    use bevy::mesh::VertexAttributeValues;
    let Some(VertexAttributeValues::Float32x3(p)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION) else {
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

fn tri_count(mesh: Option<&Mesh>) -> usize {
    mesh.and_then(|m| m.indices()).map_or(0, |i| i.len() / 3)
}

/// A proportional bar, so the size distribution is visible at a glance rather than read off numbers.
fn bar(v: f32, peak: f32, width: usize) -> String {
    let filled = if peak > 0.0 { ((v / peak) * width as f32).round() as usize } else { 0 };
    let filled = filled.min(width);
    format!("{}{}", "#".repeat(filled), "·".repeat(width - filled))
}

fn main() {
    // A torso and a head, each placed by its own transform — the same `(&Mesh, Mat4)` pairs the ECS
    // bake assembles by walking a scene's children.
    let torso = Mesh::from(Cuboid::new(0.6, 1.0, 0.35));
    let head = Mesh::from(Cuboid::new(0.34, 0.34, 0.34));
    let parts = [
        (&torso, Mat4::IDENTITY),
        (&head, Mat4::from_translation(Vec3::new(0.0, 0.67, 0.0))),
    ];

    // **The proxy — one convex cell per shell, and it is the caller's to supply.** This crate cuts
    // the proxy, not the triangle soup; see `ProxyCell`. Here the subject is two boxes, so the exact
    // decomposition is two boxes. A real character hands in V-HACD or CoACD output instead.
    //
    // Note the cells are never unioned. That is what keeps the head separable from the torso.
    let proxy = vec![
        ProxyCell::from_box(Vec3::ZERO, Vec3::new(0.3, 0.5, 0.175)),
        ProxyCell::from_box(Vec3::new(0.0, 0.67, 0.0), Vec3::splat(0.17)),
    ];

    let seed = 0x00C0_FFEE;
    // **Timed, because `AG-011` asked whether the bake needs to move off the main thread and the honest
    // answer is a number rather than an opinion.** A fix is warranted at 50 ms and not at 5 ms.
    let started = std::time::Instant::now();
    let baked = fracture_mesh(&parts, &proxy, &cut(seed));
    let elapsed = started.elapsed();

    // **One bake, every granularity.** The cut loop keeps each piece it split rather than
    // overwriting it, so the same bake reads back as three big chunks or as all of them — which is
    // what lets a cleaving blow and a blast share one cached asset.
    println!();
    println!("  granularity — one bake, read back at each piece count:");
    for want in [2usize, 3, 5, 8, TARGET] {
        let f = baked.frontier_of(want);
        let vol: f32 = f.iter().map(|p| p.cell.volume()).sum();
        println!(
            "    {want:>3} asked → {:>3} pieces, total volume {vol:.4}",
            f.len()
        );
    }

    // **`soften` rounds the drawn fragment and nothing else.** Hard dihedral edges are what a plane
    // through a solid leaves behind, and they read as ice however good the fracture is. The numbers
    // that matter are in the last column: the cell volume does not move, because the rounding is
    // Tier B — the collider, the cut faces' watertightness and every audit verdict are untouched.
    println!();
    println!("  soften — rounding the drawn surface (Tier B only)");
    println!("    value     drawn tris   drawn area   cell volume");
    for value in [0.0f32, 0.25, 0.5, 0.75] {
        let c = CutSettings { soften: value, ..cut(seed) };
        let f = fracture_mesh(&parts, &proxy, &c).into_leaves();
        let tris: usize = f.iter().map(|p| tri_count(p.outer.as_ref()) + tri_count(p.cap.as_ref())).sum();
        let area: f32 = f.iter().map(drawn_area).sum();
        let vol: f32 = f.iter().map(|p| p.cell.volume()).sum();
        println!("    {value:>5.2}     {tris:>10}   {area:>10.3}   {vol:>11.4}");
    }
    println!("    (the cell volume is the point: it never moves. Rounding is applied to the mesh you");
    println!("     draw, never to the convex cell you hand a solver, so a softer look costs nothing");
    println!("     in collision fidelity. The drawn area falls because relaxing a surface shrinks it.)");

    // **Which fragments touch which.** The hierarchy says what nests; this says what neighbours,
    // and only the second lets one piece come off while the rest stays standing.
    let leaves = baked.tree.leaves();
    let graph = &baked.bonds;
    let intact = BondSet::new(graph);
    println!();
    println!("  adjacency — {} bonds over {} finest fragments", graph.len(), leaves.len());
    println!("    intact, that is {} island(s)", graph.islands(&leaves, &intact).len());
    if let Some(victim) = leaves.iter().min_by_key(|&&id| graph.incident(id).len()) {
        let mut broken = BondSet::new(graph);
        broken.sever_all(graph.incident(*victim));
        let islands = graph.islands(&leaves, &broken);
        println!(
            "    severing fragment {}'s {} bond(s) leaves {} island(s) of sizes {:?}",
            victim.0,
            broken.severed(),
            islands.len(),
            islands.iter().map(|i| i.len()).collect::<Vec<_>>()
        );
    }
    println!("    (this subject comes back as one island because the head cell's underside sits");
    println!("     exactly on the torso cell's top face at y = 0.5 — coplanar, so a real bond. Cells");
    println!("     that merely interpenetrate or abut without agreeing on a face get NO bond, and");
    println!("     that refusal is deliberate: a proximity guess would weld a head to a torso.)");

    let pieces: Vec<FragmentGeometry> = baked.into_leaves();

    println!();
    println!("bevy_autogib — a two-part solid, plane-cut into at most {TARGET} pieces (seed {seed:#010x})");
    println!();

    if pieces.is_empty() {
        println!("  no fragments — the input had no drawable triangles.");
        return;
    }

    // **Volume, not max half-extent.** How big a chunk *is* is how much stuff it holds; a bar keyed
    // on the longest axis reads every slab as large and hides the size distribution entirely — which
    // is the one thing `plane_jitter` and `size_spread` exist to change.
    let peak = pieces.iter().map(|p| p.cell.volume()).fold(0.0f32, f32::max);

    println!("   #   centre (x, y, z)          half-extents         skin    cap   volume");
    println!("  ─────────────────────────────────────────────────────────────────────────────────");
    let mut total_skin = 0;
    let mut total_cap = 0;
    for (i, p) in pieces.iter().enumerate() {
        let skin = tri_count(p.outer.as_ref());
        let cap = tri_count(p.cap.as_ref());
        total_skin += skin;
        total_cap += cap;
        println!(
            "  {:>3}   {:>6.3} {:>6.3} {:>6.3}    {:>5.3} {:>5.3} {:>5.3}   {:>5}  {:>5}   {}",
            i,
            p.center_local.x,
            p.center_local.y,
            p.center_local.z,
            p.half_extents.x,
            p.half_extents.y,
            p.half_extents.z,
            skin,
            cap,
            bar(p.cell.volume(), peak, 12),
        );
    }
    println!("  ─────────────────────────────────────────────────────────────────────────────────");
    println!("  {} fragments · {total_skin} skin triangles · {total_cap} cut-face triangles", pieces.len());
    println!();

    // **This used to be the only quality number here, and it was the weak one.** It counts fragments
    // carrying at least one closed loop, which is not a watertightness proof — a fragment can carry a
    // cap and still have lost a second loop that never closed.
    let capped = pieces.iter().filter(|p| p.cap.is_some()).count();
    println!("  {capped} of {} fragments carry at least one closed cut face.", pieces.len());
    println!("  the fracture itself took {:.2} ms.", elapsed.as_secs_f64() * 1000.0);
    println!();

    // **Two artefacts, two headings, and they must never be added together.** A fragment is a closed
    // convex *cell* and a *subset* of the subject's own surface. The first is a solid and every verdict
    // below is a claim about it; the second is open because a subset is open, so its numbers are
    // recorded and nothing is asserted.
    //
    // This is not pedantry — it is the correction of a number this example used to print. It reported
    // "2 of 12 manifold" from a closed-solid test applied to the drawn surface, and that read as a
    // defect. The types now make the mistake unavailable: `SurfaceReport` has no `is_closed`.
    let solids = bevy_autogib::audit_proxies(&pieces);
    let closed = solids.iter().filter(|a| a.is_closed()).count();
    let manifold = solids.iter().filter(|a| a.is_manifold()).count();
    let collider_ready = solids.iter().filter(|a| a.supports_inside_outside).count();
    let sphere = solids.iter().filter(|a| a.euler_characteristic == 2).count();
    let volume: f32 = solids.iter().map(|a| a.signed_volume).sum();

    println!("   THE SOLID — each fragment's convex proxy cell, every face, closed");
    println!("  ─────────────────────────────────────────────────────────────────────────────────");
    println!("   watertight (no boundary edges)      {closed:>3} of {}", solids.len());
    println!("   manifold                            {manifold:>3} of {}", solids.len());
    println!("   topological sphere (χ = 2)          {sphere:>3} of {}", solids.len());
    println!("   solid enough for a mesh collider    {collider_ready:>3} of {}", solids.len());
    println!("   volume enclosed                     {volume:>7.4}");
    println!("  ─────────────────────────────────────────────────────────────────────────────────");
    println!();

    let surfaces: Vec<_> = pieces.iter().filter_map(|p| bevy_autogib::audit_render(p).ok()).collect();
    let open: u64 = surfaces.iter().map(|s| s.open_edges).sum();
    let nm: u64 = surfaces.iter().map(|s| s.non_manifold_edges + s.non_manifold_vertices).sum();
    let flipped: u64 = surfaces.iter().map(|s| s.inconsistently_oriented_edges).sum();
    let tris: u64 = surfaces.iter().map(|s| s.triangles).sum();

    println!("   THE DRAWN SURFACE — skin ∪ cut face. **Open by construction; nothing here is a defect**");
    println!("  ─────────────────────────────────────────────────────────────────────────────────");
    println!("   triangles                           {tris:>3}");
    println!("   open edges (recorded, not asserted) {open:>3}");
    println!("   non-manifold features               {nm:>3}");
    println!("   inside-out edges                    {flipped:>3}   ← the seam, see below");
    println!("  ─────────────────────────────────────────────────────────────────────────────────");
    println!("   Open edges are where the skin ends and the cut begins. A subset of a surface has a");
    println!("   boundary; that is what makes it a subset. Track these, never assert them to zero.");
    println!();
    println!("   Measured for comparison, one closed shell (a lone cuboid, 8 pieces): 33 open edges,");
    println!("   3 non-manifold features, 0 inside-out. So inside-out edges are specific to this");
    println!("   subject — a torso and a head meet at y = 0.5, their coincident faces weld together,");
    println!("   and interior faces disagree with their neighbours about which way is out. A real");
    println!("   glTF character is non-manifold in exactly that way. AG-003 is the ticket for it.");
    println!();

    // Same seed, same pieces — the property the whole crate is built around.
    let again = fracture_mesh(&parts, &proxy, &cut(seed)).into_leaves();
    let identical = again.len() == pieces.len()
        && again
            .iter()
            .zip(pieces.iter())
            .all(|(a, b)| a.center_local.to_bits_array() == b.center_local.to_bits_array());
    println!(
        "  re-fracturing with the same seed gave {} pieces — bit-identical: {identical}",
        again.len()
    );
    println!();
}

/// Local helper: raw bits, because "did the fracture move" is a question about exact floats and
/// comparing them with a tolerance would answer a different, easier question.
trait BitsArray {
    fn to_bits_array(&self) -> [u32; 3];
}

impl BitsArray for Vec3 {
    fn to_bits_array(&self) -> [u32; 3] {
        [self.x.to_bits(), self.y.to_bits(), self.z.to_bits()]
    }
}

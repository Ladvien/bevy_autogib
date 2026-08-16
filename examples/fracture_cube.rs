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
use bevy_autogib::{FragmentGeometry, fracture_mesh};

/// Target fragment count. The ECS bake derives this from the mesh's bounding size and the
/// `FractureSettings` dials; here it is spelled out so one number can be varied at a time.
const TARGET: usize = 12;
/// Stop cutting a piece once its extent drops below this fraction of the whole.
const MIN_FRACTION: f32 = 0.15;

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

    let seed = 0x00C0_FFEE;
    let extent = 0.67; // the merged solid's largest bounding half-dimension
    let pieces: Vec<FragmentGeometry> = fracture_mesh(&parts, TARGET, extent * MIN_FRACTION, seed, None);

    println!();
    println!("bevy_autogib — a two-part solid, plane-cut into at most {TARGET} pieces (seed {seed:#010x})");
    println!();

    if pieces.is_empty() {
        println!("  no fragments — the input had no drawable triangles.");
        return;
    }

    let peak = pieces.iter().map(|p| p.half_extents.max_element()).fold(0.0f32, f32::max);

    println!("   #   centre (x, y, z)          half-extents         skin    cap   size");
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
            bar(p.half_extents.max_element(), peak, 12),
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
    println!();

    // **And this is the proof, or the refusal of it.** `audit_fragments` welds each fragment's skin and
    // cut faces together and asks a mesh validator what it actually is: closed or open, manifold or
    // not, consistently wound or inside out, and whether a solver could build inside/outside
    // pseudo-normals from it.
    //
    // Open shards are EXPECTED for this input and are not a regression. A torso and a head are two
    // closed shells that meet, so the merged solid is not a manifold at the seam, and a plane through
    // that region produces boundary chains with no way to close — the slicer drops those rather than
    // fanning garbage over them. What was missing until now was the count.
    let audits = bevy_autogib::audit_fragments(&pieces);
    let closed = audits.iter().filter(|a| a.is_closed()).count();
    let manifold = audits.iter().filter(|a| a.is_manifold()).count();
    let solid = audits.iter().filter(|a| a.supports_inside_outside).count();
    let open_edges: u64 = audits.iter().map(|a| a.boundary_edges).sum();
    let volume: f32 = audits.iter().filter(|a| a.is_closed()).map(|a| a.signed_volume).sum();

    println!("   audit of {} fragments (skin ∪ cut faces, welded, then validated)", audits.len());
    println!("  ─────────────────────────────────────────────────────────────────────────────────");
    println!("   watertight (no boundary edges)      {closed:>3} of {}", audits.len());
    println!("   manifold                            {manifold:>3} of {}", audits.len());
    println!("   solid enough for a mesh collider    {solid:>3} of {}", audits.len());
    println!("   open cut edges, total               {open_edges:>3}");
    println!("   volume enclosed by the closed ones  {volume:>7.4}");
    println!("  ─────────────────────────────────────────────────────────────────────────────────");

    // Same seed, same pieces — the property the whole crate is built around.
    let again = fracture_mesh(&parts, TARGET, extent * MIN_FRACTION, seed, None);
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

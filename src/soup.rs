//! The slicer: a CPU triangle soup and the plane cuts that break it apart.
//!
//! No asset types, no ECS, no `App` — this half is pure geometry and unit-tests without any of them.
//! Everything Bevy-shaped lives one module over, in [`crate::mesh`].

use std::collections::HashMap;
use std::f32::consts::TAU;

use bevy::log::warn;
use bevy::math::{Vec2, Vec3};

/// Classification tolerance: a vertex within `EPS` of the cut plane is treated as lying *on* it, so
/// slicing near-coincident geometry doesn't spawn zero-area slivers. Positions are in subject-local
/// units (~1.0 tall for a character), so this is a tight tolerance.
const EPS: f32 = 1.0e-5;
/// Endpoint-weld lattice step for boundary-loop assembly (quantize positions to this grid so cut
/// segments from adjacent triangles share canonical vertex ids even on non-watertight input).
const WELD: f32 = 1.0e-4;

/// The crate's only random source: a 32-bit integer hash mapped into `[0, 1)`.
///
/// **Hand-rolled, and pinned.** There is deliberately no RNG crate here. The fracture's whole
/// reproducibility argument rests on this function returning the same bits on every machine and every
/// toolchain, and a dependency that reserves the right to change its stream between minor versions
/// cannot promise that. Its exact output is frozen by a test in this crate, so the fracture cannot move
/// underneath you without something going red.
pub fn hash_f32(x: u32) -> f32 {
    let mut h = x.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    h = ((h >> ((h >> 28).wrapping_add(4))) ^ h).wrapping_mul(277_803_737);
    h = (h >> 22) ^ h;
    (h as f32) / (u32::MAX as f32)
}

/// A vertex sample carried through clipping (interpolated at edge–plane crossings).
#[derive(Clone, Copy)]
pub(crate) struct Vtx {
    pub(crate) pos: Vec3,
    pub(crate) nrm: Vec3,
    pub(crate) uv: Vec2,
}

/// A cut plane: a point on the plane and a unit normal.
pub(crate) struct Plane {
    pub(crate) point: Vec3,
    pub(crate) normal: Vec3,
}

/// CPU triangle soup. Parallel per-vertex arrays plus one triangle per `idx` entry; `tri_interior`
/// tags a triangle as a **cut-cap** face (gets the interior material) vs original **skin** (the
/// subject's own surface). Every vertex always carries a UV (zero-filled when the source lacked
/// `UV_0`).
#[derive(Default, Clone)]
pub(crate) struct Soup {
    pub(crate) pos: Vec<Vec3>,
    pub(crate) nrm: Vec<Vec3>,
    pub(crate) uv: Vec<Vec2>,
    pub(crate) idx: Vec<[u32; 3]>,
    pub(crate) tri_interior: Vec<bool>,
}

impl Soup {
    pub(crate) fn is_empty(&self) -> bool {
        self.idx.is_empty()
    }

    pub(crate) fn vtx(&self, i: u32) -> Vtx {
        let i = i as usize;
        Vtx { pos: self.pos[i], nrm: self.nrm[i], uv: self.uv[i] }
    }

    pub(crate) fn push_tri(&mut self, a: Vtx, b: Vtx, c: Vtx, interior: bool) {
        let base = self.pos.len() as u32;
        for v in [a, b, c] {
            self.pos.push(v.pos);
            self.nrm.push(v.nrm);
            self.uv.push(v.uv);
        }
        self.idx.push([base, base + 1, base + 2]);
        self.tri_interior.push(interior);
    }

    /// Axis-aligned bounds over all vertices (min, max). `(ZERO, ZERO)` when empty.
    pub(crate) fn bbox(&self) -> (Vec3, Vec3) {
        let mut mn = Vec3::splat(f32::INFINITY);
        let mut mx = Vec3::splat(f32::NEG_INFINITY);
        for p in &self.pos {
            mn = mn.min(*p);
            mx = mx.max(*p);
        }
        if self.pos.is_empty() {
            (Vec3::ZERO, Vec3::ZERO)
        } else {
            (mn, mx)
        }
    }

    /// Vertex-average center. `ZERO` when empty.
    pub(crate) fn centroid(&self) -> Vec3 {
        if self.pos.is_empty() {
            return Vec3::ZERO;
        }
        self.pos.iter().copied().sum::<Vec3>() / self.pos.len() as f32
    }

    /// Largest bounding half-dimension — the "how big is this piece" measure driving fragment sizing.
    pub(crate) fn extent(&self) -> f32 {
        let (mn, mx) = self.bbox();
        ((mx - mn) * 0.5).max_element()
    }
}

/// Signed distance from `p` to the plane (positive on the `+normal` side).
fn signed_dist(p: Vec3, plane: &Plane) -> f32 {
    (p - plane.point).dot(plane.normal)
}

/// `+1` above / `-1` below / `0` on the plane (within `EPS`).
fn classify(s: f32) -> i32 {
    if s > EPS {
        1
    } else if s < -EPS {
        -1
    } else {
        0
    }
}

/// Vertex interpolated where segment `a→b` crosses the plane at parameter `t`.
fn lerp_vtx(a: Vtx, b: Vtx, t: f32) -> Vtx {
    Vtx {
        pos: a.pos.lerp(b.pos, t),
        nrm: a.nrm.lerp(b.nrm, t).normalize_or_zero(),
        uv: a.uv.lerp(b.uv, t),
    }
}

/// Clip one triangle to the half-space we keep (Sutherland–Hodgman on the 3-gon), fan-triangulate
/// the kept polygon, and append it to `out`. On-plane vertices (`classify == 0`) are kept for *both*
/// half-spaces so the seam geometry is shared. Original `interior` tag is inherited.
fn clip_half(v: [Vtx; 3], s: [f32; 3], keep_above: bool, interior: bool, out: &mut Soup) {
    let mut poly: Vec<Vtx> = Vec::with_capacity(4);
    for i in 0..3 {
        let j = (i + 1) % 3;
        let (ci, cj) = (classify(s[i]), classify(s[j]));
        let keep_i = if keep_above { ci >= 0 } else { ci <= 0 };
        if keep_i {
            poly.push(v[i]);
        }
        // Strict crossing (opposite strict sides) → insert the intersection vertex.
        if ci != 0 && cj != 0 && ci != cj {
            let t = s[i] / (s[i] - s[j]);
            poly.push(lerp_vtx(v[i], v[j], t));
        }
    }
    if poly.len() >= 3 {
        for i in 1..poly.len() - 1 {
            out.push_tri(poly[0], poly[i], poly[i + 1], interior);
        }
    }
}

/// The single cut segment a straddling triangle contributes to the plane (its entry/exit points).
/// `None` when the triangle only touches the plane at a point (no real cut).
fn cut_segment(v: &[Vtx; 3], s: &[f32; 3]) -> Option<[Vec3; 2]> {
    let mut pts: Vec<Vec3> = Vec::new();
    let mut add = |p: Vec3| {
        if !pts.iter().any(|q| q.distance_squared(p) < 1.0e-10) {
            pts.push(p);
        }
    };
    for i in 0..3 {
        let j = (i + 1) % 3;
        let (ci, cj) = (classify(s[i]), classify(s[j]));
        if ci == 0 {
            add(v[i].pos);
        }
        if ci != 0 && cj != 0 && ci != cj {
            let t = s[i] / (s[i] - s[j]);
            add(v[i].pos.lerp(v[j].pos, t));
        }
    }
    if pts.len() == 2 {
        Some([pts[0], pts[1]])
    } else {
        None
    }
}

/// Two orthonormal in-plane axes for a given plane normal (for cross-section UVs).
fn plane_basis(n: Vec3) -> (Vec3, Vec3) {
    let a = if n.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
    let u = n.cross(a).normalize_or_zero();
    let v = n.cross(u);
    (u, v)
}

/// Weld a point to a canonical vertex id on the quantized [`WELD`] lattice (robust loop assembly on
/// non-watertight input).
fn weld(verts: &mut Vec<Vec3>, table: &mut HashMap<(i64, i64, i64), u32>, p: Vec3) -> u32 {
    let q = |x: f32| (x / WELD).round() as i64;
    let key = (q(p.x), q(p.y), q(p.z));
    if let Some(&id) = table.get(&key) {
        return id;
    }
    let id = verts.len() as u32;
    verts.push(p);
    table.insert(key, id);
    id
}

/// Chain undirected boundary edges into closed loops. Handles multiple disjoint loops (e.g. a plane
/// through two legs). Open chains (non-watertight input) are `warn!`-dropped, never emitted.
fn assemble_loops(edges: &[(u32, u32)]) -> Vec<Vec<u32>> {
    let mut adj: HashMap<u32, Vec<usize>> = HashMap::new();
    for (ei, &(a, b)) in edges.iter().enumerate() {
        adj.entry(a).or_default().push(ei);
        adj.entry(b).or_default().push(ei);
    }
    let mut used = vec![false; edges.len()];
    let mut loops: Vec<Vec<u32>> = Vec::new();

    for start in 0..edges.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let (s0, s1) = edges[start];
        let mut loop_v = vec![s0, s1];
        let (mut prev, mut cur) = (s0, s1);
        let mut closed = false;

        for _ in 0..=edges.len() {
            if cur == s0 {
                closed = true;
                break;
            }
            // Prefer an unused edge that doesn't immediately backtrack; fall back to any unused edge.
            let pick = |avoid_prev: bool| -> Option<(usize, u32)> {
                let eis = adj.get(&cur)?;
                for &ei in eis {
                    if used[ei] {
                        continue;
                    }
                    let (a, b) = edges[ei];
                    let other = if a == cur {
                        b
                    } else if b == cur {
                        a
                    } else {
                        continue;
                    };
                    if avoid_prev && other == prev {
                        continue;
                    }
                    return Some((ei, other));
                }
                None
            };
            match pick(true).or_else(|| pick(false)) {
                Some((ei, other)) => {
                    used[ei] = true;
                    loop_v.push(other);
                    prev = cur;
                    cur = other;
                }
                None => break,
            }
        }

        if closed {
            loops.push(loop_v);
        } else {
            warn!("autogib: dropping unclosed cut boundary ({} verts)", loop_v.len());
        }
    }
    loops
}

/// Fan-triangulate one cap loop around its centroid, giving every cap triangle the `outward` normal
/// (winding fixed to match) and a planar cross-section UV. Tags triangles `interior = true`.
#[allow(clippy::too_many_arguments)]
fn push_cap_tri(out: &mut Soup, c: Vec3, p1: Vec3, p2: Vec3, outward: Vec3, bu: Vec3, bv: Vec3, origin: Vec3) {
    let face = (p1 - c).cross(p2 - c);
    if face.length_squared() < 1.0e-12 {
        return; // skip degenerate fan slice
    }
    let (a, b, d) = if face.dot(outward) >= 0.0 { (c, p1, p2) } else { (c, p2, p1) };
    let uv = |p: Vec3| Vec2::new((p - origin).dot(bu), (p - origin).dot(bv));
    out.push_tri(
        Vtx { pos: a, nrm: outward, uv: uv(a) },
        Vtx { pos: b, nrm: outward, uv: uv(b) },
        Vtx { pos: d, nrm: outward, uv: uv(d) },
        true,
    );
}

/// Close one side of a cut: weld the recorded segments, assemble boundary loops, fan-cap each with
/// the given `outward` normal. Needs at least a triangle's worth of segments.
pub(crate) fn cap_side(segs: &[[Vec3; 2]], plane: &Plane, outward: Vec3, out: &mut Soup) {
    if segs.len() < 3 {
        return;
    }
    let mut verts: Vec<Vec3> = Vec::new();
    let mut table: HashMap<(i64, i64, i64), u32> = HashMap::new();
    let mut edges: Vec<(u32, u32)> = Vec::new();
    for seg in segs {
        let ia = weld(&mut verts, &mut table, seg[0]);
        let ib = weld(&mut verts, &mut table, seg[1]);
        if ia != ib {
            edges.push((ia, ib));
        }
    }
    let (bu, bv) = plane_basis(plane.normal);
    for lp in assemble_loops(&edges) {
        if lp.len() < 3 {
            continue;
        }
        let c: Vec3 = lp.iter().map(|&i| verts[i as usize]).sum::<Vec3>() / lp.len() as f32;
        let n = lp.len();
        for k in 0..n {
            let p1 = verts[lp[k] as usize];
            let p2 = verts[lp[(k + 1) % n] as usize];
            push_cap_tri(out, c, p1, p2, outward, bu, bv, plane.point);
        }
    }
}

/// Split a soup into (above, below) halves by a plane, capping each cut watertight. The cap normals
/// face *out* of each piece: the above piece's cap faces `-normal`, the below piece's faces `+normal`.
pub(crate) fn split_soup(src: &Soup, plane: &Plane) -> (Soup, Soup) {
    let mut above = Soup::default();
    let mut below = Soup::default();
    let mut segs: Vec<[Vec3; 2]> = Vec::new();

    for (t, tri) in src.idx.iter().enumerate() {
        let interior = src.tri_interior[t];
        let v = [src.vtx(tri[0]), src.vtx(tri[1]), src.vtx(tri[2])];
        let s = [
            signed_dist(v[0].pos, plane),
            signed_dist(v[1].pos, plane),
            signed_dist(v[2].pos, plane),
        ];
        clip_half(v, s, true, interior, &mut above);
        clip_half(v, s, false, interior, &mut below);
        if let Some(seg) = cut_segment(&v, &s) {
            segs.push(seg);
        }
    }
    cap_side(&segs, plane, -plane.normal, &mut above);
    cap_side(&segs, plane, plane.normal, &mut below);
    (above, below)
}

/// Random unit vector on the sphere from a hash seed (always exactly unit length — never zero).
fn random_dir(seed: u32) -> Vec3 {
    let h1 = hash_f32(seed.wrapping_add(0x1234_5678));
    let h2 = hash_f32(seed.wrapping_add(0x9E37_79B9));
    let z = 2.0 * h1 - 1.0;
    let r = (1.0 - z * z).max(0.0).sqrt();
    let phi = h2 * TAU;
    Vec3::new(r * phi.cos(), z, r * phi.sin())
}

/// Fracture a soup into up to `target` fragments by repeatedly splitting the current largest piece
/// with a plane through its centroid. `min_extent` stops a piece from being cut below that size.
/// `seed` drives every plane direction deterministically. `impact_dir`, when set, biases the first
/// couple of cuts toward the impact (reserved seam for impact-located fracture, cf. Müller 2013).
pub(crate) fn fracture(src: Soup, target: usize, min_extent: f32, seed: u32, impact_dir: Option<Vec3>) -> Vec<Soup> {
    let mut pieces: Vec<Soup> = vec![src];
    let mut unsplittable: Vec<bool> = vec![false];
    let mut cut_index: u32 = 0;
    let mut iters = 0usize;
    let hard_cap = target.saturating_mul(16).saturating_add(32);

    while pieces.len() < target.max(1) {
        iters += 1;
        if iters > hard_cap {
            break;
        }
        // Largest splittable piece by extent.
        let pick = pieces
            .iter()
            .enumerate()
            .filter(|(i, _)| !unsplittable[*i])
            // SORT-OK: `pieces` is a Vec built in deterministic order from authored mesh geometry —
            // no query anywhere. An extent tie (or NaN → Equal) resolves to the last tied index,
            // which is the same index every run.
            .max_by(|a, b| a.1.extent().partial_cmp(&b.1.extent()).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i);
        let Some(i) = pick else {
            break; // nothing left worth cutting
        };
        if pieces[i].extent() < min_extent {
            unsplittable[i] = true;
            continue;
        }

        let s = seed
            .wrapping_add(cut_index.wrapping_mul(2_654_435_761))
            .wrapping_add(pieces.len() as u32);
        let base_dir = random_dir(s);
        let normal = match impact_dir {
            Some(d) if cut_index < 2 => {
                let blended = base_dir * 0.5 + d.normalize_or_zero() * 0.5;
                if blended.length_squared() > 1.0e-6 {
                    blended.normalize()
                } else {
                    base_dir
                }
            }
            _ => base_dir,
        };
        let plane = Plane { point: pieces[i].centroid(), normal };

        let piece = std::mem::take(&mut pieces[i]);
        let (a, b) = split_soup(&piece, &plane);
        cut_index = cut_index.wrapping_add(1);
        if a.is_empty() || b.is_empty() {
            pieces[i] = piece; // put it back; this plane didn't separate it
            unsplittable[i] = true;
            continue;
        }
        pieces[i] = a;
        pieces.push(b);
        unsplittable.push(false);
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The fracture RNG is frozen.**
    ///
    /// These bits are the whole reproducibility story: [`hash_f32`] drives every cut plane's direction,
    /// so a changed constant re-partitions every mesh this crate has ever fractured. Treat this test as
    /// a lock, not a snapshot to re-bless: if it goes red, the fracture moved.
    #[test]
    fn hash_f32_is_frozen() {
        let got: Vec<u32> = (0..8u32).map(|i| hash_f32(i).to_bits()).collect();
        assert_eq!(
            got,
            [1022846460, 1059634922, 1056243097, 1056841197, 1042407458, 1057018071, 1064390834, 1056755236],
            "the fracture RNG moved. Every cut plane's direction comes from these bits, so a change \
             here re-partitions every mesh this crate has ever fractured."
        );
        // Every value must land in [0, 1) — the contract `random_dir` multiplies against.
        for i in 0..1024u32 {
            let v = hash_f32(i);
            assert!((0.0..1.0).contains(&v), "hash_f32({i}) = {v} escaped [0, 1)");
        }
    }

    #[test]
    fn random_dir_is_unit_length_and_never_zero() {
        for i in 0..512u32 {
            let d = random_dir(i.wrapping_mul(2_654_435_761));
            assert!((d.length() - 1.0).abs() < 1.0e-5, "random_dir({i}) length {}", d.length());
        }
    }
}

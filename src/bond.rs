//! Which fragments actually touch — the adjacency a localised break needs.
//!
//! A hierarchy says which fragments *nest*. It does not say which ones are *neighbours*, and those
//! are different questions: two leaves cut from a common ancestor need not share a face. Without
//! neighbours the only expressible outcome is "all of it comes apart at once", which is the whole
//! complaint this phase exists to answer.
//!
//! # The match is exact, and that is deliberate
//!
//! Müller, Chentanez & Kim's §3.3 gives the algorithm, and it is exact for convex cells:
//!
//! > "create a list with entries for all the faces of all the convexes… each entry contains a link
//! > to the convex and the absolute value |d| of the plane equation n·x + d = 0 of the face. After
//! > sorting by the value |d|, entries with identical values (up to a numerical epsilon) can be
//! > identified during a single pass through the list… To compute the amount of overlap s ∈ [0,1] we
//! > project the faces into their common plane and perform a planar convex-convex intersection."
//!
//! Two cells are neighbours iff one has a face that lies in the same plane as a face of the other,
//! wound the opposite way, with positive overlap area. Every cut this crate makes produces exactly
//! that: [`ProxyCell::clip`](crate::proxy::ProxyCell) hands the *same* cut ring to both halves,
//! reversed for one of them, so the two faces are coplanar to the bit and overlap completely.
//!
//! # What this deliberately does not find, and why it is not patched
//!
//! **Cells that touch without sharing a coplanar face get no bond.** That is the normal case
//! *between* the caller's original proxy cells: V-HACD and CoACD produce cells that abut without
//! their boundary polygons agreeing, and two separate shells — a head sunk into a torso — properly
//! interpenetrate rather than meet. So each root's subtree comes out as its own island.
//!
//! Closing that gap would mean a proximity or overlap-volume heuristic, and a heuristic here is
//! exactly the wrong trade: it would silently weld a head to a torso, which is the *correctness*
//! loss `BACKLOG.md` records Sacht et al. measuring, and it would do it with a tolerance nobody
//! could tune from the outside. A caller who wants their root cells bonded should hand in a
//! decomposition whose cells share faces — a grid, or a volumetric decomposition in Müller's sense.
//! Refusing is the one path; approximating would be two.
//!
//! # No damage model lives here
//!
//! A [`Bond`] carries geometry — where the shared face is, which way it faces, and how large it is.
//! It carries no health and no strength, because what an area is *worth* depends on what the thing
//! is made of and what hit it, and this crate knows neither. [`BondSet`] is likewise pure state the
//! caller owns: this module only ever answers "given these bonds are broken, what is still
//! connected".

use std::collections::HashMap;

use bevy::math::Vec3;

use crate::proxy::ProxyCell;
use crate::soup::plane_basis;
use crate::tree::FragmentId;

/// Coincidence tolerance for two faces being *the same plane*.
///
/// Derived from the slicer's own weld lattice rather than picked here, for the reason
/// [`crate::audit`] gives about its own tolerance: a bond test that disagreed with the cutter about
/// what "the same point" means would be answering a question about a different mesh.
const PLANE_EPS: f32 = crate::soup::WELD;

/// Index of one bond in a [`BondGraph`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BondId(pub u32);

impl BondId {
    /// This id as an array index.
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// The shared face between two neighbouring fragments.
///
/// **Geometry, not strength.** `area` is how much surface holds them together; converting that into
/// how much force it survives is the caller's material model, not this crate's.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Bond {
    /// The two fragments this joins. Always `a < b`, so a pair appears once.
    pub a: FragmentId,
    pub b: FragmentId,
    /// Centre of the shared face, in subject-local space.
    pub centroid: Vec3,
    /// Unit normal of the shared face, pointing from `a` toward `b`.
    pub normal: Vec3,
    /// Area of the overlap, in subject-local units squared.
    pub area: f32,
}

/// Which fragments touch which, and over how much surface.
///
/// Built over a **single frontier** of the hierarchy, because adjacency is only meaningful between
/// pieces that coexist: a parent and its own child are not neighbours, they are the same volume
/// twice. [`crate::Fracture::bonds`] is the graph for the finest frontier; [`BondGraph::of`] builds
/// one for any other.
///
/// **A coarser frontier is not a special case.** Two frontier cells that touch were separated by a
/// cut at their common ancestor, so the faces they present each other lie exactly on that plane —
/// the same coplanar match works unchanged, at any depth, including a frontier that mixes depths.
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BondGraph {
    bonds: Vec<Bond>,
    /// Per fragment id, the bonds touching it. Sized to cover every id in the bake, so a fragment
    /// that is not on this graph's frontier simply has no incident bonds.
    incident: Vec<Vec<BondId>>,
    /// Per fragment id, its cell's centre — `None` for an id off this graph's frontier. Carried
    /// here so a region query needs nothing but the graph: a blade sweeping between two fragments
    /// is a question about where those fragments *are*.
    centers: Vec<Option<Vec3>>,
    /// The frontier this graph was built over, in ascending id order.
    members: Vec<FragmentId>,
}

impl BondGraph {
    /// **Müller's coplanar-face match** over the given fragments.
    ///
    /// `members` are the `(id, cell)` pairs of one frontier — any frontier, of any depth, including
    /// one that mixes depths. `capacity` is the bake's total node count, so ids off this frontier
    /// still index into [`incident`](Self::incident) safely; pass [`FragmentTree::len`].
    ///
    /// **Reach for this when you spawn something other than the leaves.** [`crate::Fracture::bonds`]
    /// covers the finest frontier only, and a fragment off a graph's frontier has no incident bonds
    /// at all — so running [`islands`](Self::islands) for a coarse frontier against the leaf graph
    /// reports every piece as its own island, and the subject falls apart on the first blow.
    ///
    /// ```ignore
    /// let ids = baked.tree.frontier_of(8);
    /// let cells: Vec<_> = ids.iter().filter_map(|id| cell_of(*id).map(|c| (*id, c))).collect();
    /// let graph = BondGraph::of(&cells, baked.tree.len());
    /// ```
    ///
    /// [`BondId`]s are positions in *this* graph, so a [`BondSet`] does not carry across a change of
    /// frontier — build a fresh one alongside.
    pub fn of(members: &[(FragmentId, &ProxyCell)], capacity: usize) -> BondGraph {
        // One entry per face of every member: the plane in `n·x + d = 0` form, plus where it came
        // from. `d` is signed; `|d|` is the sort key, because a face and its opposite-facing partner
        // share a plane and therefore share `|d|` while their signs differ.
        struct Face {
            member: usize,
            face: usize,
            normal: Vec3,
            d: f32,
        }
        let mut faces: Vec<Face> = Vec::new();
        for (m, (_, cell)) in members.iter().enumerate() {
            for fi in 0..cell.face_count() {
                let Some((origin, normal)) = cell.face_plane(fi) else { continue };
                faces.push(Face { member: m, face: fi, normal, d: -normal.dot(origin) });
            }
        }
        // SORT-OK: `|d|` with `(member, face)` as tie-break — a total order over distinct faces, so
        // the bond list is a function of the geometry and not of the vector's incidental layout.
        faces.sort_unstable_by(|x, y| {
            x.d.abs().total_cmp(&y.d.abs()).then((x.member, x.face).cmp(&(y.member, y.face)))
        });

        /// One matched pair's accumulated shared surface, before it becomes a [`Bond`].
        struct Patch {
            centroid: Vec3,
            /// Points from the lower-index member toward the higher one.
            normal: Vec3,
            area: f32,
        }
        // One forward scan. Because the list is sorted by |d|, every partner of entry `i` lies in
        // the run of entries whose |d| is within tolerance, so the inner loop stops early rather
        // than sweeping the whole list.
        let mut found: HashMap<(usize, usize), Patch> = HashMap::new();
        for i in 0..faces.len() {
            for j in i + 1..faces.len() {
                if (faces[j].d.abs() - faces[i].d.abs()).abs() > PLANE_EPS {
                    break;
                }
                let (x, y) = (&faces[i], &faces[j]);
                if x.member == y.member {
                    continue;
                }
                // Same plane, opposite facings: the normals oppose and the signed offsets cancel.
                if x.normal.dot(y.normal) > -1.0 + 1.0e-4 || (x.d + y.d).abs() > PLANE_EPS {
                    continue;
                }
                let ring_x = members[x.member].1.face_ring(x.face);
                let ring_y = members[y.member].1.face_ring(y.face);
                let Some((area, centroid)) = overlap(&ring_x, &ring_y, x.normal) else { continue };
                if area <= 1.0e-9 {
                    continue;
                }
                // Key on the member pair so two cells meeting over more than one coplanar patch
                // still yield a single bond, with the areas summed and the centroid area-weighted.
                let key = (x.member.min(y.member), x.member.max(y.member));
                // `normal` is stored pointing from the lower-index member toward the higher one.
                let toward = if x.member < y.member { x.normal } else { y.normal };
                found
                    .entry(key)
                    .and_modify(|p| {
                        p.centroid = (p.centroid * p.area + centroid * area) / (p.area + area);
                        p.area += area;
                    })
                    .or_insert(Patch { centroid, normal: toward, area });
            }
        }

        // SORT-OK: the fragment-id pair is unique per bond by construction of the map key.
        let mut pairs: Vec<((usize, usize), Patch)> = found.into_iter().collect();
        pairs.sort_unstable_by_key(|&((m, n), _)| (m, n));

        let mut bonds = Vec::with_capacity(pairs.len());
        let mut incident = vec![Vec::new(); capacity];
        for ((m, n), Patch { centroid, normal, area }) in pairs {
            let (a, b) = (members[m].0, members[n].0);
            let id = BondId(bonds.len() as u32);
            bonds.push(Bond { a, b, centroid, normal, area });
            for end in [a, b] {
                if let Some(slot) = incident.get_mut(end.index()) {
                    slot.push(id);
                }
            }
        }

        let mut centers = vec![None; capacity];
        for (id, cell) in members {
            if let Some(slot) = centers.get_mut(id.index()) {
                *slot = Some(cell.center());
            }
        }
        let mut ids: Vec<FragmentId> = members.iter().map(|(id, _)| *id).collect();
        ids.sort_unstable();
        BondGraph { bonds, incident, centers, members: ids }
    }

    /// The frontier this graph covers, in ascending id order.
    pub fn members(&self) -> &[FragmentId] {
        &self.members
    }

    /// Where a fragment sits — its cell's centre, in subject-local space. `None` for a fragment off
    /// this graph's frontier.
    pub fn center(&self, fragment: FragmentId) -> Option<Vec3> {
        self.centers.get(fragment.index()).copied().flatten()
    }

    /// Every bond, in id order.
    pub fn bonds(&self) -> &[Bond] {
        &self.bonds
    }

    /// How many bonds this graph holds.
    pub fn len(&self) -> usize {
        self.bonds.len()
    }

    /// Does nothing hold anything together?
    pub fn is_empty(&self) -> bool {
        self.bonds.is_empty()
    }

    /// One bond, or `None` if the id is out of range.
    pub fn bond(&self, id: BondId) -> Option<&Bond> {
        self.bonds.get(id.index())
    }

    /// The bonds touching `fragment`. Empty for a fragment off this graph's frontier.
    pub fn incident(&self, fragment: FragmentId) -> &[BondId] {
        self.incident.get(fragment.index()).map_or(&[], |v| v.as_slice())
    }

    /// The neighbour across `bond` from `fragment`, or `None` if that bond does not touch it.
    pub fn across(&self, bond: BondId, fragment: FragmentId) -> Option<FragmentId> {
        let b = self.bond(bond)?;
        if b.a == fragment {
            Some(b.b)
        } else if b.b == fragment {
            Some(b.a)
        } else {
            None
        }
    }

    /// **The island detection.** Which groups of `members` are still connected once `broken` bonds
    /// are gone.
    ///
    /// Stateless by design: the caller owns the accumulated [`BondSet`] and calls this again after
    /// each blow, which is what makes repeated partial damage work without this crate holding a
    /// damage model. Müller calls the equivalent step "crucial… it is this step that makes sure
    /// that objects collapse in the correct way".
    ///
    /// A member with no surviving bond comes back as its own single-fragment island — that is the
    /// piece that just left. Islands are returned in ascending order of their lowest member id, and
    /// each island's members are sorted, so the result is reproducible.
    pub fn islands(&self, members: &[FragmentId], broken: &BondSet) -> Vec<Vec<FragmentId>> {
        let present: HashMap<FragmentId, usize> =
            members.iter().enumerate().map(|(i, &id)| (id, i)).collect();
        let mut seen = vec![false; members.len()];
        let mut out: Vec<Vec<FragmentId>> = Vec::new();

        for start in 0..members.len() {
            if seen[start] {
                continue;
            }
            seen[start] = true;
            let mut island = vec![members[start]];
            let mut stack = vec![members[start]];
            while let Some(cur) = stack.pop() {
                for &bid in self.incident(cur) {
                    if broken.is_broken(bid) {
                        continue;
                    }
                    let Some(next) = self.across(bid, cur) else { continue };
                    // A bond to something off this frontier is not a connection *here*.
                    let Some(&slot) = present.get(&next) else { continue };
                    if seen[slot] {
                        continue;
                    }
                    seen[slot] = true;
                    island.push(next);
                    stack.push(next);
                }
            }
            island.sort_unstable();
            out.push(island);
        }
        out.sort_unstable_by_key(|i| i.first().copied());
        out
    }
}

/// The caller's accumulated damage state: which bonds have been severed so far.
///
/// **Deliberately just a set.** Progressive destruction — hit it, hit it again, watch it come apart
/// — is this set growing between calls to [`BondGraph::islands`]. Keeping it on the caller's side is
/// what lets the crate support repeated partial damage without ever learning what health is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BondSet {
    broken: Vec<bool>,
}

impl BondSet {
    /// An empty set sized for `graph`.
    pub fn new(graph: &BondGraph) -> Self {
        BondSet { broken: vec![false; graph.len()] }
    }

    /// Sever one bond. Returns `true` if this call is what broke it, `false` if it was already
    /// broken or the id is out of range.
    pub fn sever(&mut self, id: BondId) -> bool {
        match self.broken.get_mut(id.index()) {
            Some(slot) if !*slot => {
                *slot = true;
                true
            }
            _ => false,
        }
    }

    /// Sever every bond in `ids`, returning how many were newly broken.
    pub fn sever_all(&mut self, ids: &[BondId]) -> usize {
        ids.iter().filter(|&&id| self.sever(id)).count()
    }

    /// Is this bond gone? An out-of-range id reads as intact, so a stale id cannot silently
    /// dismantle something.
    pub fn is_broken(&self, id: BondId) -> bool {
        self.broken.get(id.index()).copied().unwrap_or(false)
    }

    /// How many bonds have been severed.
    pub fn severed(&self) -> usize {
        self.broken.iter().filter(|b| **b).count()
    }

    /// Has nothing been severed yet?
    pub fn is_intact(&self) -> bool {
        !self.broken.iter().any(|b| *b)
    }

    /// The severed bonds, in id order.
    pub fn iter(&self) -> impl Iterator<Item = BondId> + '_ {
        self.broken.iter().enumerate().filter(|(_, b)| **b).map(|(i, _)| BondId(i as u32))
    }
}

/// Area and centroid of the overlap between two convex rings known to lie in the same plane.
///
/// `None` when either ring is degenerate. Both rings are re-wound counter-clockwise about `normal`
/// before clipping — one of them arrives wound the other way, because each is counter-clockwise seen
/// from *outside its own cell* and the two cells face each other.
fn overlap(ring_a: &[Vec3], ring_b: &[Vec3], normal: Vec3) -> Option<(f32, Vec3)> {
    if ring_a.len() < 3 || ring_b.len() < 3 {
        return None;
    }
    let origin = ring_a[0];
    let (u, v) = plane_basis(normal);
    let to_2d = |p: Vec3| {
        let q = p - origin;
        (q.dot(u), q.dot(v))
    };

    let mut a: Vec<(f32, f32)> = ring_a.iter().map(|p| to_2d(*p)).collect();
    let mut b: Vec<(f32, f32)> = ring_b.iter().map(|p| to_2d(*p)).collect();
    if signed_area(&a) < 0.0 {
        a.reverse();
    }
    if signed_area(&b) < 0.0 {
        b.reverse();
    }

    let clipped = clip_convex(&a, &b);
    if clipped.len() < 3 {
        return None;
    }
    let area = signed_area(&clipped).abs();
    let (cx, cy) = centroid_2d(&clipped);
    Some((area, origin + u * cx + v * cy))
}

/// Twice-signed area halved: positive for a counter-clockwise ring.
fn signed_area(p: &[(f32, f32)]) -> f32 {
    let mut s = 0.0;
    for i in 0..p.len() {
        let (x0, y0) = p[i];
        let (x1, y1) = p[(i + 1) % p.len()];
        s += x0 * y1 - x1 * y0;
    }
    s * 0.5
}

/// Area-weighted centroid of a simple polygon; falls back to the vertex mean only when the polygon
/// has no area, where the weighted formula is undefined rather than merely imprecise.
fn centroid_2d(p: &[(f32, f32)]) -> (f32, f32) {
    let mut a = 0.0;
    let (mut cx, mut cy) = (0.0, 0.0);
    for i in 0..p.len() {
        let (x0, y0) = p[i];
        let (x1, y1) = p[(i + 1) % p.len()];
        let cross = x0 * y1 - x1 * y0;
        a += cross;
        cx += (x0 + x1) * cross;
        cy += (y0 + y1) * cross;
    }
    if a.abs() < 1.0e-12 {
        let n = p.len().max(1) as f32;
        return (p.iter().map(|q| q.0).sum::<f32>() / n, p.iter().map(|q| q.1).sum::<f32>() / n);
    }
    (cx / (3.0 * a), cy / (3.0 * a))
}

/// Sutherland–Hodgman: clip `subject` against every edge of the convex `window`. Both must be wound
/// counter-clockwise, which [`overlap`] guarantees before calling.
fn clip_convex(subject: &[(f32, f32)], window: &[(f32, f32)]) -> Vec<(f32, f32)> {
    let mut out: Vec<(f32, f32)> = subject.to_vec();
    for i in 0..window.len() {
        if out.is_empty() {
            break;
        }
        let (ax, ay) = window[i];
        let (bx, by) = window[(i + 1) % window.len()];
        // Positive when the point is left of a→b, i.e. inside a counter-clockwise window.
        let side = |(px, py): (f32, f32)| (bx - ax) * (py - ay) - (by - ay) * (px - ax);
        let input = std::mem::take(&mut out);
        for k in 0..input.len() {
            let cur = input[k];
            let prev = input[(k + input.len() - 1) % input.len()];
            let (sc, sp) = (side(cur), side(prev));
            // The edge is crossed exactly when the two endpoints sit on opposite sides of it, in
            // either direction; emit that crossing first, then keep the current point if it is in.
            if (sc >= 0.0) != (sp >= 0.0)
                && let Some(x) = intersect(prev, cur, sp, sc)
            {
                out.push(x);
            }
            if sc >= 0.0 {
                out.push(cur);
            }
        }
    }
    out
}

/// Where segment `p→q` crosses the clip edge, from the two side values. `None` when they are equal,
/// which means the segment lies along the edge and has no single crossing.
fn intersect(p: (f32, f32), q: (f32, f32), sp: f32, sq: f32) -> Option<(f32, f32)> {
    let denom = sp - sq;
    if denom.abs() < 1.0e-20 {
        return None;
    }
    let t = sp / denom;
    Some((p.0 + (q.0 - p.0) * t, p.1 + (q.1 - p.1) * t))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CutSettings;
    use crate::soup::{Plane, fracture};

    fn unit_cube_cells() -> Vec<ProxyCell> {
        vec![ProxyCell::from_box(Vec3::ZERO, Vec3::splat(0.5))]
    }

    /// Two halves of one cut share their whole cut face — the exact case every cut produces.
    #[test]
    fn a_single_cut_bonds_its_two_halves_over_the_full_cut_face() {
        let cell = ProxyCell::from_box(Vec3::ZERO, Vec3::splat(0.5));
        let (above, below) = cell.clip(&Plane { point: Vec3::ZERO, normal: Vec3::Y });
        let (above, below) = (above.expect("cuts"), below.expect("cuts"));
        let members = [(FragmentId(0), &above), (FragmentId(1), &below)];
        let g = BondGraph::of(&members, 2);

        assert_eq!(g.len(), 1, "one cut, one bond");
        let b = &g.bonds()[0];
        assert_eq!((b.a, b.b), (FragmentId(0), FragmentId(1)));
        assert!((b.area - 1.0).abs() < 1.0e-4, "the shared face is the unit square, got {}", b.area);
        assert!(b.centroid.length() < 1.0e-4, "it is centred on the cut, got {:?}", b.centroid);
        assert!(b.normal.dot(Vec3::Y).abs() > 0.99, "and its normal is the cut plane's");
    }

    /// **A partial overlap yields the overlap's area, not the whole face's.** This is the
    /// convex-convex intersection Müller specifies, and getting it wrong would over-report how much
    /// surface holds a small piece onto a large one.
    ///
    /// A 1×1 slab and a 0.4×0.4 post meeting at `y = 0` share exactly the post's footprint, 0.16.
    #[test]
    fn a_partial_face_overlap_reports_only_the_shared_area() {
        let slab = ProxyCell::from_box(Vec3::new(0.0, -0.5, 0.0), Vec3::new(0.5, 0.5, 0.5));
        let post = ProxyCell::from_box(Vec3::new(0.0, 0.2, 0.0), Vec3::new(0.2, 0.2, 0.2));
        let members = [(FragmentId(0), &slab), (FragmentId(1), &post)];
        let g = BondGraph::of(&members, 2);

        assert_eq!(g.len(), 1);
        let b = &g.bonds()[0];
        assert!((b.area - 0.16).abs() < 1.0e-5, "expected the post's 0.4x0.4 footprint, got {}", b.area);
        assert!(b.centroid.y.abs() < 1.0e-5, "the shared face is at y = 0, got {:?}", b.centroid);
    }

    /// Cells that merely sit near each other, without a shared face, get **no** bond. This is the
    /// refusal the module docs argue for, pinned so nobody adds a proximity fallback by accident.
    #[test]
    fn cells_that_do_not_share_a_face_are_not_bonded() {
        let a = ProxyCell::from_box(Vec3::ZERO, Vec3::splat(0.5));
        // Overlapping, like a head sunk into a torso — interpenetrating, not face-sharing.
        let b = ProxyCell::from_box(Vec3::new(0.4, 0.0, 0.0), Vec3::splat(0.5));
        // And a third that is simply far away.
        let c = ProxyCell::from_box(Vec3::new(9.0, 0.0, 0.0), Vec3::splat(0.5));
        let members = [(FragmentId(0), &a), (FragmentId(1), &b), (FragmentId(2), &c)];
        assert_eq!(BondGraph::of(&members, 3).len(), 0);
    }

    /// The graph a real bake produces: connected, symmetric, and every fragment reachable from every
    /// other while nothing is broken.
    #[test]
    fn a_baked_graph_is_symmetric_and_connected() {
        let (pieces, tree) = fracture(crate::soup::Soup::default(), &unit_cube_cells(), &CutSettings::new(8, 0.05, 0x1234));
        let leaves = tree.leaves();
        let members: Vec<_> =
            leaves.iter().filter_map(|&id| pieces.get(id.index()).map(|p| (id, &p.cell))).collect();
        let g = BondGraph::of(&members, tree.len());

        assert!(g.len() >= leaves.len() - 1, "a connected graph needs at least n-1 bonds");
        for (i, b) in g.bonds().iter().enumerate() {
            let id = BondId(i as u32);
            assert!(b.a < b.b, "each pair is stored once, lower id first");
            assert!(g.incident(b.a).contains(&id) && g.incident(b.b).contains(&id), "symmetric");
            assert_eq!(g.across(id, b.a), Some(b.b));
            assert_eq!(g.across(id, b.b), Some(b.a));
            assert!(b.area > 0.0 && b.area.is_finite());
        }
        let intact = BondSet::new(&g);
        assert_eq!(g.islands(&leaves, &intact).len(), 1, "nothing broken is one island");
    }

    /// **A coarse frontier is bonded too, and that is not obvious.**
    ///
    /// The leaf graph knows nothing about interior nodes, so running `islands` for a coarse frontier
    /// against it reports every piece as its own island — a subject that falls apart on the first
    /// blow. Building the graph *for that frontier* is the answer, and it works because two frontier
    /// cells that touch were separated by a cut at their common ancestor, so the faces they present
    /// each other are exactly coplanar however deep either one sits.
    #[test]
    fn every_frontier_has_its_own_connected_graph() {
        let (pieces, tree) =
            fracture(crate::soup::Soup::default(), &unit_cube_cells(), &CutSettings::new(12, 0.03, 0x0FF1_CE));
        assert!(tree.cuts() >= 6, "need a deep enough bake to have coarse frontiers");

        for want in 2..=tree.leaves().len() {
            let ids = tree.frontier_of(want);
            let members: Vec<_> =
                ids.iter().filter_map(|&id| pieces.get(id.index()).map(|p| (id, &p.cell))).collect();
            let g = BondGraph::of(&members, tree.len());
            let islands = g.islands(&ids, &BondSet::new(&g));
            assert_eq!(
                islands.len(),
                1,
                "the {want}-piece frontier came back as {} islands — it is one solid",
                islands.len()
            );
        }

        // And the leaf graph really is the wrong tool for a coarse frontier, which is why `of` is
        // public: pinned so the trap stays visible rather than being rediscovered.
        let leaf_graph = crate::mesh::bond_graph(&pieces, &tree);
        let coarse = tree.frontier_of(4);
        assert!(
            leaf_graph.islands(&coarse, &BondSet::new(&leaf_graph)).len() > 1,
            "a coarse frontier read against the leaf graph should look disconnected"
        );
    }

    /// **The localised break, in miniature.** Severing every bond around one fragment detaches that
    /// fragment alone; the rest stays a single connected body.
    #[test]
    fn severing_one_fragments_bonds_detaches_only_it() {
        let (pieces, tree) = fracture(crate::soup::Soup::default(), &unit_cube_cells(), &CutSettings::new(8, 0.05, 0x1234));
        let leaves = tree.leaves();
        let members: Vec<_> =
            leaves.iter().filter_map(|&id| pieces.get(id.index()).map(|p| (id, &p.cell))).collect();
        let g = BondGraph::of(&members, tree.len());

        // Pick a fragment that is not a cut vertex of the graph, so the remainder stays whole: the
        // one with the fewest bonds is the safest such choice on a convex subdivision.
        let victim = *leaves
            .iter()
            .min_by_key(|&&id| g.incident(id).len())
            .expect("the bake produced fragments");
        let mut broken = BondSet::new(&g);
        assert_eq!(broken.severed(), 0, "and it starts intact");
        broken.sever_all(g.incident(victim));

        let islands = g.islands(&leaves, &broken);
        assert!(islands.len() >= 2, "the victim left, so there is more than one island");
        assert!(
            islands.iter().any(|i| i == &vec![victim]),
            "and it left alone, not dragging neighbours with it"
        );
        let total: usize = islands.iter().map(|i| i.len()).sum();
        assert_eq!(total, leaves.len(), "every fragment lands in exactly one island");
    }

    /// Progressive damage: the set only grows, and islands only ever fragment further.
    #[test]
    fn repeated_severing_only_ever_breaks_things_further() {
        let (pieces, tree) = fracture(crate::soup::Soup::default(), &unit_cube_cells(), &CutSettings::new(8, 0.05, 0xBEEF));
        let leaves = tree.leaves();
        let members: Vec<_> =
            leaves.iter().filter_map(|&id| pieces.get(id.index()).map(|p| (id, &p.cell))).collect();
        let g = BondGraph::of(&members, tree.len());

        let mut broken = BondSet::new(&g);
        let mut last = g.islands(&leaves, &broken).len();
        for i in 0..g.len() {
            assert!(broken.sever(BondId(i as u32)), "the first severing of a bond reports true");
            assert!(!broken.sever(BondId(i as u32)), "and the second reports false");
            let now = g.islands(&leaves, &broken).len();
            assert!(now >= last, "severing never re-joins anything: {last} -> {now}");
            last = now;
        }
        assert_eq!(last, leaves.len(), "every bond gone is every fragment alone");
        assert_eq!(broken.severed(), g.len());
        assert_eq!(broken.iter().count(), g.len());
    }

    /// A stale id is refused, never fatal, and never silently dismantles anything.
    #[test]
    fn an_out_of_range_id_is_refused() {
        let g = BondGraph::default();
        assert!(g.bond(BondId(7)).is_none());
        assert!(g.incident(FragmentId(7)).is_empty());
        assert!(g.across(BondId(7), FragmentId(0)).is_none());
        let mut s = BondSet::new(&g);
        assert!(!s.sever(BondId(7)), "severing a bond that does not exist changes nothing");
        assert!(!s.is_broken(BondId(7)));
        assert!(s.is_intact());
    }
}

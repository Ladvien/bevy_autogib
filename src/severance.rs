//! **Where a blow lands, in geometry.** Region queries that say how strongly something reaches each
//! bond — and nothing about what it was, how hard it hit, or what happens next.
//!
//! # The shape, and why it is two steps
//!
//! Every query returns a [`Reach`]: a severity in `[0, 1]` for each bond the region touched, `1` at
//! full effect and falling to `0` at the edge. Turning that into severed bonds is
//! [`Reach::above`], and the threshold is the caller's number.
//!
//! Splitting it there is deliberate. A game scales severity by what the thing is made of, how much
//! damage the blow carried, or how much the bond has already taken — and all three are facts this
//! crate does not have. Folding a threshold into the query would mean either inventing a damage
//! model here or handing back a decision the caller could not adjust. Reach out, threshold yours.
//!
//! # Why this is a runtime query and not a bake parameter
//!
//! Müller, Chentanez & Kim state the problem exactly: with a static pre-fracture "there is no way to
//! align fracture patterns with the impact location at run time… When a gamer shoots at a glass
//! window, she expects the spider-web-shaped fracture pattern to be centered around the location
//! where the bullet hit the glass. Anything else clearly destroys the illusion." Their answer, and
//! PhysX Blast's after them, is to bake a decomposition once and *select* against it per impact.
//! That is what this module is: the bake stays reproducible and cached, and every blow is a pure
//! function of it plus a region.
//!
//! # The five regions
//!
//! They follow Blast's damage-shader set, because that set was arrived at by shipping games and it
//! covers the cases without overlapping:
//!
//! | query | the thing it models |
//! |---|---|
//! | [`spread`] | a projectile — nearest fragment, then outward *along the bonds*, so a hit takes a connected chunk rather than everything within a sphere |
//! | [`capsule`] | a swung edge — falloff from the segment the blade travelled |
//! | [`swept_triangle`] | a swept blade proper — every bond the swing passed *through* gives way, regardless of distance |
//! | [`radial`] | a blast — falloff from a point in open space |
//! | [`directional`] | a pull — falloff weighted by how squarely each shared face meets the direction of the tear |
//!
//! Nothing here is named for a weapon. `spread` is not "bullet" and `capsule` is not "sword",
//! because the crate that knows which is which is yours.

use bevy::math::Vec3;

use crate::bond::{BondGraph, BondId};
use crate::tree::FragmentId;

/// How strongly a region reached each bond it touched.
///
/// Severity is `1.0` at full effect and falls to `0.0` at the region's edge; a bond the region never
/// reached is simply absent. Entries are in ascending [`BondId`] order, so a `Reach` reads the same
/// on every run.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reach {
    hits: Vec<(BondId, f32)>,
}

impl Reach {
    /// Build from unsorted pairs, dropping anything that did not actually land.
    fn from_hits(mut hits: Vec<(BondId, f32)>) -> Reach {
        hits.retain(|(_, s)| *s > 0.0 && s.is_finite());
        // SORT-OK: a bond appears at most once, so the id alone is a total order here.
        hits.sort_unstable_by_key(|(id, _)| *id);
        Reach { hits }
    }

    /// Every bond reached, with its severity, in id order.
    pub fn iter(&self) -> impl Iterator<Item = (BondId, f32)> + '_ {
        self.hits.iter().copied()
    }

    /// How strongly this reached one bond. `0.0` for a bond it never touched.
    pub fn severity(&self, id: BondId) -> f32 {
        self.hits.binary_search_by_key(&id, |(b, _)| *b).map_or(0.0, |i| self.hits[i].1)
    }

    /// The bonds whose severity is at least `threshold` — **the line between "reached" and
    /// "severed", and it is yours to draw.**
    ///
    /// Scale by material, by how much damage the blow carried, or by what the bond has already
    /// taken, then pick the threshold that means "this gives way" in your game.
    pub fn above(&self, threshold: f32) -> Vec<BondId> {
        self.hits.iter().filter(|(_, s)| *s >= threshold).map(|(id, _)| *id).collect()
    }

    /// How many bonds were reached at all.
    pub fn len(&self) -> usize {
        self.hits.len()
    }

    /// Did this land on nothing?
    pub fn is_empty(&self) -> bool {
        self.hits.is_empty()
    }

    /// The most strongly reached bond, if any — where the blow was centred, in effect.
    pub fn strongest(&self) -> Option<(BondId, f32)> {
        // Ties resolve to the lowest id: `max_by` keeps the last maximum, so scanning in id order
        // and comparing strictly greater would keep the first. Do that explicitly.
        self.hits.iter().copied().reduce(|a, b| if b.1 > a.1 { b } else { a })
    }
}

/// Full effect inside `min`, linearly to nothing at `max`, nothing beyond.
///
/// The shape Blast uses: "if distance is smaller than minRadius, full damage is applied. From
/// minRadius to maxRadius it linearly falls off to zero."
fn falloff(distance: f32, min: f32, max: f32) -> f32 {
    if distance <= min {
        return 1.0;
    }
    if distance >= max || max <= min {
        return 0.0;
    }
    1.0 - (distance - min) / (max - min)
}

/// **A blast.** Falloff by straight-line distance from `center` to each shared face.
///
/// Reaches through the object rather than around it, which is what an explosion in open space does
/// and what makes this the wrong query for a projectile — see [`spread`].
pub fn radial(graph: &BondGraph, center: Vec3, min_radius: f32, max_radius: f32) -> Reach {
    Reach::from_hits(
        graph
            .bonds()
            .iter()
            .enumerate()
            .map(|(i, b)| (BondId(i as u32), falloff(b.centroid.distance(center), min_radius, max_radius)))
            .collect(),
    )
}

/// **A swung edge.** Falloff by distance from the segment `a → b` to each shared face.
///
/// The blade's path as a capsule: everything within `min_radius` of the swing gives fully, out to
/// nothing at `max_radius`. Use this for a cut that should bite deepest along its length rather than
/// around a single point.
pub fn capsule(graph: &BondGraph, a: Vec3, b: Vec3, min_radius: f32, max_radius: f32) -> Reach {
    Reach::from_hits(
        graph
            .bonds()
            .iter()
            .enumerate()
            .map(|(i, bond)| {
                let d = point_to_segment(bond.centroid, a, b);
                (BondId(i as u32), falloff(d, min_radius, max_radius))
            })
            .collect(),
    )
}

/// **A pull.** Radial falloff, weighted by how squarely each shared face meets `direction`.
///
/// A face whose normal lies along the pull takes the full falloff; a face edge-on to it takes
/// nothing. That is the difference between tearing an arm off along its length and trying to tear it
/// off sideways, and it is why this query and [`radial`] give different answers from the same point.
///
/// `direction` need not be normalised; a zero direction reaches nothing, because a pull with no
/// direction is not a pull.
pub fn directional(
    graph: &BondGraph,
    origin: Vec3,
    direction: Vec3,
    min_radius: f32,
    max_radius: f32,
) -> Reach {
    let d = direction.normalize_or_zero();
    if d == Vec3::ZERO {
        return Reach::default();
    }
    Reach::from_hits(
        graph
            .bonds()
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let near = falloff(b.centroid.distance(origin), min_radius, max_radius);
                (BondId(i as u32), near * b.normal.dot(d).abs())
            })
            .collect(),
    )
}

/// **A swept blade.** Every bond whose two fragments sit on opposite sides of the triangle
/// `a, b, c`, and whose joining segment crosses inside it, gives way completely.
///
/// No falloff and no radius: a blade either passed between two pieces or it did not. Blast provides
/// the same test and notes it is "useful for sweeping-blade effects" — sample the weapon's edge at
/// the start and end of a swing and hand in the quad as two triangles.
///
/// Bonds whose fragments this graph has no position for are skipped rather than guessed at.
pub fn swept_triangle(graph: &BondGraph, a: Vec3, b: Vec3, c: Vec3) -> Reach {
    Reach::from_hits(
        graph
            .bonds()
            .iter()
            .enumerate()
            .filter_map(|(i, bond)| {
                let (p, q) = (graph.center(bond.a)?, graph.center(bond.b)?);
                segment_hits_triangle(p, q, a, b, c).then_some((BondId(i as u32), 1.0))
            })
            .collect(),
    )
}

/// **A projectile.** Find the fragment nearest `point`, then spread outward **along the bonds**,
/// with falloff on distance travelled through the object rather than through space.
///
/// This is the query that takes a chunk off and leaves the rest standing, and the graph walk is why:
/// a sphere centred on a hit reaches across a gap to anything nearby, while a walk can only reach
/// what is actually connected to what was struck. Blast's `ImpactSpread` does the same thing —
/// "looks for nearest chunk to position and damages it, then does breadth-first support graph
/// traversal with radial falloff metric measured along graph edges".
///
/// Distance is measured out to a fragment's centre and then on to the shared face, so
/// `max_radius` genuinely controls how much comes off rather than always stripping the struck
/// fragment bare.
///
/// **Stateless: this walks the bake's adjacency, not the current damage.** A blow spreads through
/// bonds a previous blow already severed. That is deliberate — the query stays a pure function of
/// the bake and the region, and the caller's [`BondSet`](crate::BondSet) stays the caller's.
///
/// Reaches nothing when the graph is empty or has no positions.
pub fn spread(graph: &BondGraph, point: Vec3, min_radius: f32, max_radius: f32) -> Reach {
    let Some(seed) = nearest(graph, point) else { return Reach::default() };

    // Dijkstra over fragments, edge weight = the distance between the two fragments' centres. The
    // graphs here are tens of nodes, so the O(n²) scan is both fast enough and free of the
    // float-ordering hazards a binary heap would introduce.
    let members = graph.members();
    let mut dist: Vec<f32> = vec![f32::INFINITY; members.len()];
    let mut done = vec![false; members.len()];
    let slot = |id: FragmentId| members.binary_search(&id).ok();
    let Some(seed_slot) = slot(seed) else { return Reach::default() };
    dist[seed_slot] = 0.0;

    for _ in 0..members.len() {
        // SORT-OK: the frontier is scanned in ascending id order and ties keep the first, so the
        // visit order is a function of the geometry rather than of iteration order.
        let mut best: Option<usize> = None;
        for s in 0..members.len() {
            if !done[s] && dist[s].is_finite() && best.is_none_or(|b| dist[s] < dist[b]) {
                best = Some(s);
            }
        }
        let Some(cur) = best else { break };
        done[cur] = true;
        let here = members[cur];
        let Some(here_at) = graph.center(here) else { continue };
        for &bid in graph.incident(here) {
            let Some(next) = graph.across(bid, here) else { continue };
            let (Some(next_slot), Some(next_at)) = (slot(next), graph.center(next)) else { continue };
            let step = dist[cur] + here_at.distance(next_at);
            if step < dist[next_slot] {
                dist[next_slot] = step;
            }
        }
    }

    // **How far the blow had to travel to reach the face itself**, by whichever side is nearer:
    // out to a fragment's centre, then on to the shared face.
    //
    // Measuring only to the nearer *fragment* would put every bond touching the struck piece at
    // distance zero, so the radius would stop controlling anything and any hit at all would strip
    // the struck fragment bare. Carrying on to the face is what makes `max_radius` mean "how much
    // comes off".
    let reach_of = |end: FragmentId, face: Vec3| -> f32 {
        match (slot(end), graph.center(end)) {
            (Some(s), Some(c)) => dist[s] + c.distance(face),
            _ => f32::INFINITY,
        }
    };
    Reach::from_hits(
        graph
            .bonds()
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let d = reach_of(b.a, b.centroid).min(reach_of(b.b, b.centroid));
                (BondId(i as u32), falloff(d, min_radius, max_radius))
            })
            .collect(),
    )
}

/// The graph member whose centre is closest to `point`, ties going to the lower id.
fn nearest(graph: &BondGraph, point: Vec3) -> Option<FragmentId> {
    let mut best: Option<(FragmentId, f32)> = None;
    for &id in graph.members() {
        let Some(c) = graph.center(id) else { continue };
        let d = c.distance_squared(point);
        if best.is_none_or(|(_, bd)| d < bd) {
            best = Some((id, d));
        }
    }
    best.map(|(id, _)| id)
}

/// Shortest distance from `p` to the segment `a → b`. Degenerates to point-to-point when the
/// segment has no length, which is correct rather than a special case.
fn point_to_segment(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let ab = b - a;
    let len2 = ab.length_squared();
    if len2 < 1.0e-20 {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Does segment `p → q` pass through triangle `a, b, c`? Möller–Trumbore, with the parameter
/// clamped to the segment rather than the whole ray.
fn segment_hits_triangle(p: Vec3, q: Vec3, a: Vec3, b: Vec3, c: Vec3) -> bool {
    let dir = q - p;
    let (e1, e2) = (b - a, c - a);
    let h = dir.cross(e2);
    let det = e1.dot(h);
    // Parallel to the triangle's plane: a blade lying flat in the seam severs nothing.
    if det.abs() < 1.0e-12 {
        return false;
    }
    let inv = 1.0 / det;
    let s = p - a;
    let u = s.dot(h) * inv;
    if !(0.0..=1.0).contains(&u) {
        return false;
    }
    let g = s.cross(e1);
    let v = dir.dot(g) * inv;
    if v < 0.0 || u + v > 1.0 {
        return false;
    }
    let t = e2.dot(g) * inv;
    (0.0..=1.0).contains(&t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CutSettings;
    use crate::bond::BondSet;
    use crate::proxy::ProxyCell;
    use crate::soup::{Soup, fracture};
    use crate::tree::FragmentTree;

    /// A row of four unit cubes in a line, each sharing a full face with the next:
    /// `0 — 1 — 2 — 3` along +X, centres at x = 0, 1, 2, 3.
    fn row() -> (BondGraph, Vec<FragmentId>) {
        let cells: Vec<ProxyCell> =
            (0..4).map(|i| ProxyCell::from_box(Vec3::new(i as f32, 0.0, 0.0), Vec3::splat(0.5))).collect();
        let members: Vec<(FragmentId, &ProxyCell)> =
            cells.iter().enumerate().map(|(i, c)| (FragmentId(i as u32), c)).collect();
        let g = BondGraph::of(&members, 4);
        let ids = (0..4).map(FragmentId).collect();
        (g, ids)
    }

    /// The row is what the other tests assume: three bonds, in a line, unit area each.
    #[test]
    fn the_row_fixture_is_a_chain() {
        let (g, ids) = row();
        assert_eq!(g.len(), 3, "four cubes in a line share three faces");
        assert_eq!(g.islands(&ids, &BondSet::new(&g)).len(), 1);
        for b in g.bonds() {
            assert!((b.area - 1.0).abs() < 1.0e-5);
        }
    }

    #[test]
    fn falloff_is_full_inside_min_and_gone_past_max() {
        assert_eq!(falloff(0.0, 1.0, 3.0), 1.0);
        assert_eq!(falloff(1.0, 1.0, 3.0), 1.0);
        assert!((falloff(2.0, 1.0, 3.0) - 0.5).abs() < 1.0e-6, "halfway is half");
        assert_eq!(falloff(3.0, 1.0, 3.0), 0.0);
        assert_eq!(falloff(9.0, 1.0, 3.0), 0.0);
        // A degenerate band reaches only what is already inside it, rather than dividing by zero.
        assert_eq!(falloff(0.5, 1.0, 1.0), 1.0);
        assert_eq!(falloff(1.5, 1.0, 1.0), 0.0);
    }

    /// **The localised break.** A hit on the far end takes the end cube off and leaves the other
    /// three standing — the behaviour the whole phase exists for.
    #[test]
    fn a_spread_at_one_end_detaches_only_that_end() {
        let (g, ids) = row();
        let hit = spread(&g, Vec3::new(3.0, 0.0, 0.0), 0.5, 1.5);
        let mut broken = BondSet::new(&g);
        broken.sever_all(&hit.above(0.5));

        let islands = g.islands(&ids, &broken);
        assert_eq!(islands.len(), 2, "one piece left, got {islands:?}");
        assert!(islands.contains(&vec![FragmentId(3)]), "and it was the struck end");
        assert!(islands.contains(&vec![FragmentId(0), FragmentId(1), FragmentId(2)]));
    }

    /// Spread measures distance **through the bonds**, so severity falls monotonically as you walk
    /// away from the hit. This is what separates it from `radial`.
    #[test]
    fn spread_falls_off_along_the_chain_not_through_space() {
        let (g, _) = row();
        let hit = spread(&g, Vec3::new(0.0, 0.0, 0.0), 0.0, 10.0);
        let s: Vec<f32> = (0..3).map(|i| hit.severity(BondId(i))).collect();
        assert!(s[0] > s[1] && s[1] > s[2], "severity must decay along the chain, got {s:?}");
        assert_eq!(hit.strongest().map(|(id, _)| id), Some(BondId(0)), "strongest at the hit");
    }

    /// A blast reaches by straight-line distance, so a bond behind the object is reached as
    /// strongly as one in front at the same range.
    #[test]
    fn radial_reaches_by_distance_alone() {
        let (g, _) = row();
        // Centred on the middle bond, off to one side.
        let hit = radial(&g, Vec3::new(1.5, 5.0, 0.0), 0.0, 10.0);
        assert!(hit.severity(BondId(1)) > hit.severity(BondId(0)));
        assert!((hit.severity(BondId(0)) - hit.severity(BondId(2))).abs() < 1.0e-5, "symmetric");
    }

    /// A swing along the row cuts everything it passes; a swing beside it cuts nothing.
    #[test]
    fn a_capsule_cuts_along_its_length() {
        let (g, _) = row();
        let along = capsule(&g, Vec3::new(0.0, 0.0, 0.0), Vec3::new(3.0, 0.0, 0.0), 0.1, 0.2);
        assert_eq!(along.above(0.9).len(), 3, "the swing ran the length of the row");

        let beside = capsule(&g, Vec3::new(0.0, 9.0, 0.0), Vec3::new(3.0, 9.0, 0.0), 0.1, 0.2);
        assert!(beside.is_empty(), "a swing nine units away touches nothing");
    }

    /// **The swept blade.** A triangle passing between cubes 1 and 2 severs exactly that bond —
    /// no radius, no falloff, and nothing else gives.
    #[test]
    fn a_swept_triangle_severs_only_what_it_passed_through() {
        let (g, ids) = row();
        // A blade sweeping down through the x = 1.5 plane, wide enough to clear the row in y and z.
        let hit = swept_triangle(
            &g,
            Vec3::new(1.5, -5.0, -5.0),
            Vec3::new(1.5, 5.0, -5.0),
            Vec3::new(1.5, 0.0, 5.0),
        );
        assert_eq!(hit.above(1.0), vec![BondId(1)], "exactly the bond it passed through");

        let mut broken = BondSet::new(&g);
        broken.sever_all(&hit.above(1.0));
        let islands = g.islands(&ids, &broken);
        assert_eq!(islands.len(), 2, "cleaved in two");
        assert_eq!(islands[0], vec![FragmentId(0), FragmentId(1)]);
        assert_eq!(islands[1], vec![FragmentId(2), FragmentId(3)]);
    }

    /// A blade that misses passes through nothing.
    #[test]
    fn a_swept_triangle_that_misses_severs_nothing() {
        let (g, _) = row();
        let hit = swept_triangle(
            &g,
            Vec3::new(1.5, 20.0, -5.0),
            Vec3::new(1.5, 30.0, -5.0),
            Vec3::new(1.5, 25.0, 5.0),
        );
        assert!(hit.is_empty());
    }

    /// A pull along the row's axis meets every shared face squarely; a pull across it meets none of
    /// them, even from the same point at the same range.
    #[test]
    fn a_pull_only_reaches_faces_that_meet_it() {
        let (g, _) = row();
        let along = directional(&g, Vec3::new(1.5, 0.0, 0.0), Vec3::X, 10.0, 20.0);
        let across = directional(&g, Vec3::new(1.5, 0.0, 0.0), Vec3::Y, 10.0, 20.0);
        assert_eq!(along.len(), 3, "every face in the row faces along X");
        assert!(along.iter().all(|(_, s)| (s - 1.0).abs() < 1.0e-4));
        assert!(across.is_empty(), "and none of them face along Y");
        assert!(directional(&g, Vec3::ZERO, Vec3::ZERO, 1.0, 2.0).is_empty(), "a pull with no direction");
    }

    /// Repeated blows accumulate in the caller's set, and the object comes apart as they land.
    #[test]
    fn repeated_blows_take_it_apart_progressively() {
        let (g, ids) = row();
        let mut broken = BondSet::new(&g);
        let mut counts = Vec::new();
        for x in [3.0f32, 0.0, 1.5] {
            let hit = spread(&g, Vec3::new(x, 0.0, 0.0), 0.5, 1.5);
            broken.sever_all(&hit.above(0.5));
            counts.push(g.islands(&ids, &broken).len());
        }
        assert_eq!(counts, vec![2, 3, 4], "each blow takes another piece off");
        assert!(broken.severed() == g.len(), "and by the third, nothing holds");
    }

    /// Every query is a pure function of the bake and the region: same inputs, same answer.
    #[test]
    fn the_queries_are_pure_and_reproducible() {
        let cells = vec![ProxyCell::from_box(Vec3::ZERO, Vec3::splat(0.5))];
        let build = || -> (BondGraph, FragmentTree) {
            let (pieces, tree) = fracture(Soup::default(), &cells, &CutSettings::new(8, 0.05, 0xD00D));
            (crate::mesh::bond_graph(&pieces, &tree), tree)
        };
        let (ga, _) = build();
        let (gb, _) = build();
        assert_eq!(ga, gb, "the graph itself is reproducible");
        for (a, b) in [
            (spread(&ga, Vec3::X * 0.4, 0.1, 0.6), spread(&gb, Vec3::X * 0.4, 0.1, 0.6)),
            (radial(&ga, Vec3::ZERO, 0.1, 0.6), radial(&gb, Vec3::ZERO, 0.1, 0.6)),
            (
                capsule(&ga, -Vec3::X, Vec3::X, 0.1, 0.6),
                capsule(&gb, -Vec3::X, Vec3::X, 0.1, 0.6),
            ),
            (
                directional(&ga, Vec3::ZERO, Vec3::Y, 0.1, 0.6),
                directional(&gb, Vec3::ZERO, Vec3::Y, 0.1, 0.6),
            ),
            (
                swept_triangle(&ga, Vec3::new(0.0, -1.0, -1.0), Vec3::new(0.0, 1.0, -1.0), Vec3::new(0.0, 0.0, 1.0)),
                swept_triangle(&gb, Vec3::new(0.0, -1.0, -1.0), Vec3::new(0.0, 1.0, -1.0), Vec3::new(0.0, 0.0, 1.0)),
            ),
        ] {
            assert_eq!(a, b, "a region query must be a pure function of the bake");
        }
    }

    /// **The whole loop, on the subject the examples actually use.** Bake a torso and a head, stand
    /// them up, hit one spot, and check that *some but not all* of it comes off and the rest is
    /// still one connected body. This is what `examples/sever.rs` does on screen, and the thing the
    /// crate could not express before this phase: the answer used to be all-or-nothing.
    #[test]
    fn a_hit_takes_part_of_the_subject_and_leaves_the_rest_standing() {
        let cells = vec![
            ProxyCell::from_box(Vec3::ZERO, Vec3::new(0.35, 0.55, 0.2)),
            ProxyCell::from_box(Vec3::new(0.0, 0.75, 0.0), Vec3::splat(0.2)),
        ];
        let (pieces, tree) = fracture(Soup::default(), &cells, &CutSettings::new(34, 0.08, 0x00C0_FFEE));
        let standing = tree.leaves();
        let graph = crate::mesh::bond_graph(&pieces, &tree);
        assert!(standing.len() > 12, "need a fine enough bake to take a small piece off");
        assert_eq!(
            graph.islands(&standing, &BondSet::new(&graph)).len(),
            1,
            "the subject starts as one body"
        );

        // A hit up on the head — the crate is not told it is a head, only where the blow landed.
        let mut broken = BondSet::new(&graph);
        let hit = spread(&graph, Vec3::new(0.0, 0.82, 0.0), 0.06, 0.34);
        assert!(!hit.is_empty(), "the blow reached nothing at all");
        broken.sever_all(&hit.above(0.5));

        let islands = graph.islands(&standing, &broken);
        assert!(islands.len() >= 2, "something should have come off");
        let biggest = islands.iter().map(|i| i.len()).max().unwrap_or(0);
        let off: usize = standing.len() - biggest;
        assert!(off > 0, "nothing detached");
        assert!(
            off < standing.len() / 2,
            "{off} of {} came off — a localised hit must not take most of the body",
            standing.len()
        );

        // The pieces that left are near where it landed, not scattered over the whole subject.
        let detached: Vec<FragmentId> =
            islands.iter().filter(|i| i.len() != biggest).flatten().copied().collect();
        for id in &detached {
            let Some(c) = graph.center(*id) else { continue };
            assert!(c.y > 0.2, "fragment {id:?} left from {c:?}, nowhere near a hit at y = 0.82");
        }

        // **Keep hitting it and it keeps coming apart.** Not every blow detaches something — a
        // fragment can lose bonds and still be held on by the ones the region missed, which is the
        // behaviour that makes repeated damage read as wearing a thing down rather than as a switch.
        // What must hold is that a blow never *re-joins* anything, and that the sequence progresses.
        let mut count = islands.len();
        let mut progressed = false;
        for y in [-0.30f32, 0.0, 0.30, -0.45, 0.45] {
            broken.sever_all(&spread(&graph, Vec3::new(0.0, y, 0.0), 0.08, 0.45).above(0.5));
            let now = graph.islands(&standing, &broken).len();
            assert!(now >= count, "a blow at y = {y} re-joined something: {count} -> {now}");
            progressed |= now > count;
            count = now;
        }
        assert!(progressed, "five more blows and nothing else came off");
        assert!(count > islands.len(), "the subject ended up no more broken than after one blow");
    }

    /// An empty graph is answered, not crashed into.
    #[test]
    fn an_empty_graph_reaches_nothing() {
        let g = BondGraph::default();
        assert!(spread(&g, Vec3::ZERO, 1.0, 2.0).is_empty());
        assert!(radial(&g, Vec3::ZERO, 1.0, 2.0).is_empty());
        assert!(capsule(&g, Vec3::ZERO, Vec3::X, 1.0, 2.0).is_empty());
        assert!(directional(&g, Vec3::ZERO, Vec3::X, 1.0, 2.0).is_empty());
        assert!(swept_triangle(&g, Vec3::ZERO, Vec3::X, Vec3::Y).is_empty());
        assert_eq!(Reach::default().severity(BondId(0)), 0.0);
        assert!(Reach::default().strongest().is_none());
    }
}

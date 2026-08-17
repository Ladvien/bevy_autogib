//! The fracture hierarchy: one bake, every granularity.
//!
//! The cut loop in [`crate::soup`] already computes this and used to throw it away. Each cut splits
//! one piece into two, so the sequence of cuts *is* a binary forest — one tree per proxy cell — and
//! the state of the piece set after `k` cuts is a valid `(cells + k)`-piece decomposition of the
//! subject. Recording the parent/child links costs no geometry work at all; keeping the parent's
//! payload instead of overwriting it costs memory, which is what
//! [`max_depth`](crate::FractureSettings::max_depth) is for.
//!
//! # Why a forest and not a list
//!
//! A caller asking "break this into three pieces" and a caller asking "break this into a hundred"
//! want the same bake read at two depths, not two bakes. Müller, Chentanez & Kim's objection to
//! static pre-fracturing is precisely that "the number of hierarchical fracture levels is fixed";
//! the answer, which PhysX Blast and Chaos both landed on independently, is a chunk hierarchy that
//! a runtime query reads at whatever depth it needs — including **different depths in different
//! places**, which is what lets a struck arm come apart while the torso stays whole.
//!
//! # What this module deliberately does not know
//!
//! Nothing here knows what hit the subject, how hard, or what it was made of. A [`FragmentTree`] is
//! a topology over convex cells; choosing a frontier from it is [`crate::severance`]'s job, and
//! deciding *why* is the caller's.

/// Index of one fragment — a node in the [`FragmentTree`] and, equivalently, a position in the
/// parallel fragment payload array (`Vec<Fragment>` or `Vec<FragmentGeometry>`).
///
/// **The two are always the same length and always in the same order.** Topology and payload are
/// separate types so that a bond graph can reference a fragment without cloning its meshes, but the
/// index is shared and that is load-bearing: `fragments[id.index()]` is the payload of `tree.node(id)`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FragmentId(pub u32);

impl FragmentId {
    /// This id as an array index.
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// One node of the hierarchy. Roots are the caller's proxy cells, unsplit.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TreeNode {
    /// The piece this one was cut from. `None` for a root — one of the caller's proxy cells.
    pub parent: Option<FragmentId>,
    /// The two halves this piece was cut into, `[above, below]` relative to the cut plane's normal.
    /// `None` for a leaf: nothing split it, either because the target count was reached or because
    /// it hit [`min_fraction`](crate::FractureSettings::min_fraction) or `max_depth`.
    pub children: Option<[FragmentId; 2]>,
    /// Cuts from the root: `0` for a proxy cell, `n+1` for either half of a depth-`n` piece.
    pub depth: u16,
    /// **Which cut split this node**, as an index into the bake's cut sequence. `None` for a leaf.
    ///
    /// This is what makes [`FragmentTree::frontier_after`] exact rather than approximate: cuts are
    /// numbered in the order the bake performed them, and each one grows the frontier by exactly
    /// one piece, so "the `k`-piece decomposition" is a well-defined set rather than a heuristic.
    pub split_at: Option<u32>,
}

impl TreeNode {
    /// Was this piece never cut further?
    pub fn is_leaf(&self) -> bool {
        self.children.is_none()
    }
}

/// The binary forest a bake produced: one tree per proxy cell, plus the frontier queries that read
/// it at a chosen granularity.
///
/// Every query returns an **antichain** — a set of nodes with no node an ancestor of another — so
/// the returned fragments tile the subject exactly once, with no gaps and no double cover. That
/// property is what makes a frontier safe to spawn directly.
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FragmentTree {
    nodes: Vec<TreeNode>,
    roots: Vec<FragmentId>,
    cuts: u32,
}

impl FragmentTree {
    /// Build from a node array whose roots are exactly the nodes with no parent.
    pub(crate) fn from_nodes(nodes: Vec<TreeNode>, cuts: u32) -> Self {
        let roots = nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.parent.is_none())
            .map(|(i, _)| FragmentId(i as u32))
            .collect();
        FragmentTree { nodes, roots, cuts }
    }

    /// Total node count — roots, interior pieces and leaves together. This is also the length of the
    /// parallel fragment payload array.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Did this bake produce nothing at all?
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// How many cuts the bake performed. The coarsest frontier has `roots().len()` pieces and the
    /// finest has `roots().len() + cuts()`.
    pub fn cuts(&self) -> u32 {
        self.cuts
    }

    /// One node, or `None` if the id is out of range. **There is no panicking index here** — an id
    /// from a stale bake must be refusable, not fatal.
    pub fn node(&self, id: FragmentId) -> Option<&TreeNode> {
        self.nodes.get(id.index())
    }

    /// The caller's original proxy cells, uncut.
    pub fn roots(&self) -> &[FragmentId] {
        &self.roots
    }

    /// Every node, in id order.
    pub fn iter(&self) -> impl Iterator<Item = (FragmentId, &TreeNode)> {
        self.nodes.iter().enumerate().map(|(i, n)| (FragmentId(i as u32), n))
    }

    /// The finest frontier: every piece that was never cut further. This is what the crate returned
    /// before the hierarchy existed.
    pub fn leaves(&self) -> Vec<FragmentId> {
        self.iter().filter(|(_, n)| n.is_leaf()).map(|(id, _)| id).collect()
    }

    /// The frontier reached after `cuts` cuts — the **`roots().len() + cuts` piece decomposition**.
    ///
    /// A node is in it when it has been born (its parent was split strictly before `cuts`) and has
    /// not yet been split (`split_at` is absent or at/after `cuts`). Clamping is deliberate:
    /// asking for more cuts than the bake performed yields the leaves, not an error.
    pub fn frontier_after(&self, cuts: u32) -> Vec<FragmentId> {
        self.iter()
            .filter(|(_, n)| {
                let born = match n.parent.and_then(|p| self.node(p)).and_then(|p| p.split_at) {
                    Some(t) => t < cuts,
                    // A root is born at time zero; a node whose parent is missing or unsplit cannot
                    // exist, so treating it as unborn is the refusing branch, not a fallback.
                    None => n.parent.is_none(),
                };
                born && n.split_at.is_none_or(|t| t >= cuts)
            })
            .map(|(id, _)| id)
            .collect()
    }

    /// The frontier holding roughly `count` pieces — the granularity dial.
    ///
    /// Clamped to `[roots().len(), roots().len() + cuts()]`, so `frontier_of(3)` on a two-cell
    /// subject gives three pieces and `frontier_of(1000)` gives the leaves.
    pub fn frontier_of(&self, count: usize) -> Vec<FragmentId> {
        let floor = self.roots.len();
        let cuts = count.saturating_sub(floor).min(self.cuts as usize) as u32;
        self.frontier_after(cuts)
    }

    /// The frontier at most `depth` cuts from the roots: every node that is either a leaf shallower
    /// than `depth` or sits exactly at `depth`.
    ///
    /// Unlike [`frontier_after`](Self::frontier_after) this cuts every branch to the same *level*
    /// rather than the same *count*, which is the shape a level-of-detail selector wants.
    pub fn at_depth(&self, depth: u16) -> Vec<FragmentId> {
        self.iter()
            .filter(|(_, n)| n.depth <= depth && (n.is_leaf() || n.depth == depth))
            .map(|(id, _)| id)
            .collect()
    }

    /// Every node beneath `id`, excluding `id` itself, in id order.
    pub fn descendants(&self, id: FragmentId) -> Vec<FragmentId> {
        let mut out = Vec::new();
        let mut stack = vec![id];
        while let Some(n) = stack.pop() {
            let Some(node) = self.node(n) else { continue };
            if let Some(kids) = node.children {
                for k in kids {
                    out.push(k);
                    stack.push(k);
                }
            }
        }
        // SORT-OK: FragmentIds by the whole value — ties are identical, and the walk starts from
        // a caller-named node over the bake's own tree, never a query.
        out.sort_unstable();
        out
    }

    /// The root the given node descends from, or `None` if the id is out of range.
    pub fn root_of(&self, id: FragmentId) -> Option<FragmentId> {
        let mut cur = id;
        // Bounded by the node count: `parent` strictly decreases toward a root because a child is
        // always allocated after its parent, so this cannot loop.
        for _ in 0..=self.nodes.len() {
            let node = self.node(cur)?;
            match node.parent {
                Some(p) => cur = p,
                None => return Some(cur),
            }
        }
        None
    }

    /// Is `ancestor` on the path from `id` to its root?
    pub fn is_ancestor(&self, ancestor: FragmentId, id: FragmentId) -> bool {
        let mut cur = id;
        for _ in 0..=self.nodes.len() {
            let Some(node) = self.node(cur) else { return false };
            match node.parent {
                Some(p) if p == ancestor => return true,
                Some(p) => cur = p,
                None => return false,
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A forest of one root cut three times:
    ///
    /// ```text
    ///            0                cut 0 splits 0 -> 1,2
    ///          /   \              cut 1 splits 1 -> 3,4
    ///         1     2             cut 2 splits 2 -> 5,6
    ///        / \   / \
    ///       3   4 5   6
    /// ```
    fn forest() -> FragmentTree {
        let n = |parent, children, depth, split_at| TreeNode { parent, children, depth, split_at };
        FragmentTree::from_nodes(
            vec![
                n(None, Some([FragmentId(1), FragmentId(2)]), 0, Some(0)),
                n(Some(FragmentId(0)), Some([FragmentId(3), FragmentId(4)]), 1, Some(1)),
                n(Some(FragmentId(0)), Some([FragmentId(5), FragmentId(6)]), 1, Some(2)),
                n(Some(FragmentId(1)), None, 2, None),
                n(Some(FragmentId(1)), None, 2, None),
                n(Some(FragmentId(2)), None, 2, None),
                n(Some(FragmentId(2)), None, 2, None),
            ],
            3,
        )
    }

    #[test]
    fn every_frontier_is_an_antichain_that_covers_once() {
        let t = forest();
        for cuts in 0..=t.cuts() {
            let f = t.frontier_after(cuts);
            assert_eq!(f.len(), t.roots().len() + cuts as usize, "cuts={cuts} wrong piece count");
            for &a in &f {
                for &b in &f {
                    assert!(
                        a == b || !t.is_ancestor(a, b),
                        "cuts={cuts}: {a:?} is an ancestor of {b:?} — the frontier double-covers"
                    );
                }
            }
        }
    }

    #[test]
    fn the_coarsest_frontier_is_the_roots_and_the_finest_is_the_leaves() {
        let t = forest();
        assert_eq!(t.frontier_after(0), t.roots());
        assert_eq!(t.frontier_after(t.cuts()), t.leaves());
    }

    #[test]
    fn frontier_of_clamps_instead_of_failing() {
        let t = forest();
        assert_eq!(t.frontier_of(0).len(), 1, "below the root count clamps up to the roots");
        assert_eq!(t.frontier_of(3).len(), 3);
        assert_eq!(t.frontier_of(9_999), t.leaves(), "above the cut count clamps down to the leaves");
    }

    #[test]
    fn at_depth_cuts_every_branch_to_the_same_level() {
        let t = forest();
        assert_eq!(t.at_depth(0), vec![FragmentId(0)]);
        assert_eq!(t.at_depth(1), vec![FragmentId(1), FragmentId(2)]);
        assert_eq!(t.at_depth(2), t.leaves());
        assert_eq!(t.at_depth(9), t.leaves(), "past the deepest level is the leaves, not empty");
    }

    #[test]
    fn descendants_and_ancestry_agree() {
        let t = forest();
        assert_eq!(
            t.descendants(FragmentId(0)),
            vec![FragmentId(1), FragmentId(2), FragmentId(3), FragmentId(4), FragmentId(5), FragmentId(6)]
        );
        assert_eq!(t.descendants(FragmentId(3)), vec![]);
        for d in t.descendants(FragmentId(1)) {
            assert!(t.is_ancestor(FragmentId(1), d));
            assert!(t.is_ancestor(FragmentId(0), d), "ancestry is transitive to the root");
        }
        assert!(!t.is_ancestor(FragmentId(1), FragmentId(5)));
        assert_eq!(t.root_of(FragmentId(6)), Some(FragmentId(0)));
    }

    #[test]
    fn an_out_of_range_id_is_refused_rather_than_fatal() {
        let t = forest();
        assert!(t.node(FragmentId(99)).is_none());
        assert!(t.root_of(FragmentId(99)).is_none());
        assert!(!t.is_ancestor(FragmentId(0), FragmentId(99)));
        assert_eq!(t.descendants(FragmentId(99)), vec![]);
    }
}

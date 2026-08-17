# bevy_autogib — BACKLOG ARCHIVE

Completed tickets, newest last. **The annotation is the point; the checkmark is not.** Every entry
records what actually happened — amendments, deviations, and above all *falsified premises*, including
the ones that were falsified in our own favour.

---

## Phase A — Independence

### ☑ A-1 · Port Stage 1 into this repository, verbatim

Copied `src/audit.rs`, `docs/research-brief.md`, `docs/isomesh-upstream-asks.md`, `BACKLOG.md` and the
working-tree state of `Cargo.toml`, `src/lib.rs`, `src/soup.rs`, `tests/leaf.rs` and
`examples/fracture_cube.rs` out of `foundation_vs_slop/crates/bevy_autogib/`. No behavioural edit, so
that every ticket after it has a real diff base.

> **Falsified premise — the one that made this ticket necessary.** The backlog was written as though
> Stage 1 had shipped. It had not shipped anywhere. `src/audit.rs` — 473 lines, the only thing in the
> project able to say whether a fragment is closed, manifold or consistently wound — **had never been
> committed to any repository**, and neither had the `isomesh` dependency, either research document, or
> `BACKLOG.md` itself. All of it sat untracked in one working tree.
>
> The consequence was not cosmetic: **nine of the eleven original tickets named files that did not exist
> in the published crate**, and the backlog's own "correction #1" — an agent reporting that `isomesh` was
> absent from our manifest — turns out to have been a *fair reading of published history*, not the
> careless one the correction implies. The manifest entry existed only in a working tree.
>
> It also killed AG-009 outright: a `git subtree split` carries commits, so no amount of re-splitting
> could ever have delivered an uncommitted file. See A-4.

**Deviation from plan:** `Cargo.lock` was committed alongside, which the plan had as a separate step.
`isomesh` is a git dependency pinned to a rev; without the lockfile that pin is not reproducible for
anyone cloning this, so the two belong in one commit.

**Verified:** `cargo build --release` passes on the declared feature set alone — the check that matters,
since `cargo test` pulls the full `bevy` umbrella through dev-dependencies and cannot see a missing
feature. 16 unit tests, `leaf.rs` and the doctests green with no `-p` flag.

### ☑ A-2 · A way to see the defect, not just count it

`examples/capture.rs` renders the fracture headless and tints every fragment by what `audit_fragment`
says about it — green for a closed manifold solid, amber for closed-but-not-manifold, magenta for open
cut edges. `tools/gif.sh` holds the encode. `docs/fracture-baseline.gif` is the before picture.

> **Amendment made during the work.** The first capture coloured open fragments *red*. The cut faces are
> already dark red — it is the crate's established visual language and `explode.rs` argues for it — so
> the verdict read as more cut face and the colouring failed at the one job it had. Magenta now, which
> appears nowhere else in the scene.

**Why fixed-timestep and headless, rather than screen-recording `explode`:** two GIFs must differ *only*
where the geometry differs. `explode` integrates against `Time`, so its trajectories depend on how fast
the machine rendered; here the update loop is pumped by hand on a constant `DT`, and the encode lives in
a script so a palette or dither change cannot masquerade as a change in the fracture.

**Measured, and it is not the number the backlog quotes:** 15 of 18 fragments solid, 3 open, at
`TARGET = 18`, seed `0x00C0_FFEE`. The backlog's 7/12 is a *different* configuration (`TARGET = 12`), and
neither number was pinned by any test — which is what AG-012 exists to fix.

**Boundary held:** examples take the full `bevy` umbrella from `[dev-dependencies]`, so none of this
reaches a consumer's graph, and `tests/leaf.rs` — which reads `[dependencies]` alone — is untouched.

---

## Phase 0 — Baseline before the rewrite

### ☑ AG-012 · Pin the torso+head baseline in a test

`known_baseline_torso_and_head_is_mostly_not_solid` in `src/audit.rs` now asserts all four figures the
architecture argument rests on: **7 of 12 watertight, 2 of 12 manifold, 4 of 12 collider-ready, 22 open
cut edges**, at `TARGET = 12`, `seed = 0x00C0_FFEE`. Counts are computed exactly as
`examples/fracture_cube.rs` prints them.

> **The premise held, and it was worse than stated.** The ticket said these numbers were unpinned. They
> were: no test referenced the torso+head fixture at all, and the only fixture CI locked was the convex
> `Cuboid` — the one case that was never broken. So the suite was green *because* it only ever measured
> the case the capper handles correctly.

**Amendment:** the test also asserts `audits.len() == 12`. `audit_fragments` silently omits any fragment
it cannot measure, so without that line a fragment dropping out of the population would make every count
below it a comparison against a different denominator — and it would read as an improvement.

**Known duplication, deliberately left.** The fixture exists twice: `torso_and_head()` in the test module
and the same two `Cuboid`s in `examples/fracture_cube.rs`. Sharing it would mean exporting a test fixture
from the crate's public API, which is a worse trade than a doc comment on each naming the other. AG-004
touches both and should re-check they still agree.

**This test is expected to go red when AG-001 lands, and that is the deliverable** — it is the baseline
half of a pre-registered prediction, not a target. AG-004 retires it.

### ☑ AG-002 · Hollow-prism fixture — make the invisible bug measurable

`hollow_prism` (3×3 outer square, 1×1 bore, closed, manifold, genus 1, χ = 0) now sits beside `u_prism`
in `src/audit.rs`, and `known_defect_nested_cut_boundary_is_filled_solid` cuts it and pins what happens.

> **Falsified premise — most of the pre-registered prediction.** AG-002 predicted the capper would
> "conserve χ and manifoldness while overstating volume by exactly (bore cross-section area × length)",
> and that *"every `MeshReport` field reports it healthy and only volume notices."* Measured across 24
> configurations — two depths × two bore areas × four cut heights × both sides — three of those four
> claims are wrong:
>
> - **χ is not conserved.** A correctly cut piece of a tube is still a tube: genus 1, χ = 0. Every
>   emitted piece reports **χ = 2**. Filling a bore *is* a genus reduction, so χ is precisely the field
>   that sees it.
> - **`inconsistently_oriented_edges` is 8, never 0**, so `supports_inside_outside` is false. Two fields
>   notice the defect, not zero.
> - **Volume is the field that misses it.** Cut through the origin and the un-recentred volume of the
>   emitted piece is `8.0` — exactly right. The two same-facing sheets over the bore cancel against the
>   rim walls.
> - **Manifoldness is conserved.** `non_manifold_edges` and `non_manifold_vertices` stay 0. This half
>   held.
>
> The overstatement is real but not what the ticket described: **`bore_area × length / 3`**, exact in
> all 24 cases.

> **Second falsified premise, found while chasing the first — and this one was a false claim in our own
> source.** That `/3` is an artefact of *recentring*, not a measurement of the defect. The doc on
> `FragmentAudit::signed_volume` asserted "recentering does not change it". Translation preserves the
> divergence-theorem sum only for a surface that is closed **and consistently oriented**; this one is
> not, and `geometry_from_soup` recentres every fragment on its bbox before the audit ever sees it. So
> the reported volume of an inconsistently-oriented fragment is offset by an amount depending on where
> the fragment happened to sit — and the offset is a tidy enough number to be mistaken for a
> measurement. The doc comment is corrected in the same commit, and now states the two conditions under
> which the field means anything.

**What the test asserts instead of volume:** **cap area**, which is translation-invariant and checkable
by hand. The outer fan paves the whole 3×3 square, bore included; the bore's own fan then paves the bore
a second time. Emitted area is `outer + bore = 10` where the truth is `outer − bore = 8` — over by
exactly `2 × bore`, which is also the mechanism stated in one line.

**Ticket amended:** the un-recentred volume assertion is kept *as the falsified half*, with a comment
saying that if it ever fails, the prediction may have become true and the comment is what needs
revisiting. AG-008 flips the rest.

### ☑ AG-006 · Scope the fan-fold claim, and commit the fixture that breaks it

`known_defect_a_doubly_wound_fan_folds_with_every_counter_at_zero` commits a regular pentagram `{5/2}`
and feeds its five segments straight to `cap_side`. The fan covers the inner pentagon twice, so emitted
area exceeds the star's true area by exactly the inner pentagon's area — and
`inconsistently_oriented_edges`, `non_manifold_edges` and `non_manifold_vertices` are **all zero**. All
figures are written as formulas from the unit circumradius, so they can be re-derived rather than trusted.

The equivalence is restated as scoped in both places it was asserted: the doc comment on
`known_defect_cap_fan_folds_on_a_non_convex_section`, and `docs/isomesh-upstream-asks.md` §5 — where it
had been offered *upstream* in the unqualified form, which is the version that mattered most to fix.

**The two qualifiers, stated the way the ticket asked:**

1. It is specific to `push_cap_tri`'s **per-triangle** flip. That flip is what converts mixed signed area
   into a shared spoke traversed twice the same way; it is a fact about our capper, not a property of
   `MeshReport`.
2. **The loop has to reverse.** An apex outside a *simply-connected* loop mixes the signs — the `u_prism`
   case. A loop winding twice around its centroid in the same direction mixes nothing.

> **Consequence that runs the other way from the ticket's framing.** AG-006 was written as a narrowing —
> "the claim is weaker than we said". It is, but the conclusion is that **Ask 5 is worth *more* than the
> asks doc implied**, not less: the topological route cannot see a doubly-wound fold, and a narrow-phase
> check inside a fan is the only thing that can. The summary table is corrected to say so.

> **Falsified premise, carried in from the ticket's own side-note.** It claimed `assemble_loops`
> "returns loops whose first vertex is duplicated at the end", double-weighting the fan apex. It does
> not: `loop_v` starts as `vec![s0, s1]` and the walk breaks on `cur == s0` *before* pushing again, so
> every vertex appears exactly once — which is also why `cap_side` closes the fan with `lp[(k + 1) % n]`.
> A duplicated first vertex would make that modulo wrap emit a degenerate final triangle. The other half
> of the note is true and is now recorded in the doc comment: the apex is a plain vertex average, not an
> area centroid.

### ☑ AG-010 · Correct `docs/isomesh-upstream-asks.md`

Four edits, not the three the ticket listed.

**(a) The on-demand premise is retired.** Ask 2 argued for an evaluate-on-demand `impl Sdf` partly
because "sampling on demand is the right shape for Manifold Dual Contouring, which queries where it
needs to rather than reading a precomputed grid". MDC does no such thing: `DualMesher::extract` calls
`self.sample(...)` at `dual.rs:251` before anything else runs, and that function loops every one of the
N³ grid points into a `Vec<R>` (`dual.rs:272-289`). The `Sdf` reference survives only to supply
gradients. The claim came from a summary of the paper rather than the source — and upstream has since
written the same refutation into its own tree (`construct/from_mesh.rs:458-465`), so it is now citable
to them rather than only to us.

**(b) `S-001…S-007` recorded as uncommitted intent — with the qualifier that makes it true.** They did
not exist at `4369e3c`. They exist at `HEAD` now. The correction as originally written ("zero lines of
Rust changed") would itself have become false; it is wrong about *when*, not about *what*.

**(c) Ask 2 re-scoped from "the only hard blocker" to optional**, and the reason recorded: isomesh did
not change, autogib's critical path did. Tier A/B repairs the cutter by cutting a convex proxy, so an
SDF backend stops being the route to correct fragments.

> **Finding the ticket did not anticipate, and it inverts the ask.** Upstream measured the thing Ask 2
> asked for and the answer is no *in that shape*. The `MeshField` that shipped is **pseudonormal**-signed
> — the `S-006` route this very document argues does not serve autogib, because it needs closed
> consistently-oriented input and our subject is non-manifold exactly where its shells meet. The
> winding-number variant exists only as a **batch** function over a grid, and upstream records why an
> on-demand twin cannot exist: `winding_numbers` casts one ray per grid *row* and amortises it across
> that row, so a per-point query would cast N³ rays — "a factor of N, not a constant". So if the pin ever
> moves, the SDF backend is unblocked **by exactly the route this ask said we did not want.**

**(d) Added — per-ask status at `HEAD`, in a banner and a new summary-table column.** Ask 3 is
**granted** (`weld_split_by`, `weld.rs:338`), which is the one with a direct consequence: AG-005 was
scoped to hand-roll it. Ask 1 is **not granted as written** — `TriangleGrid` is still `pub(crate)`;
upstream solved it a level up by exporting `MeshField` and keeping the grid private. Asks 4 and 5 are
untouched.

### ☑ AG-013 · Settle the isomesh pin

**Bumped, `4369e3c` → `22c3b35`.** Decided by measurement, as the ticket required.

**What it cost: exactly one re-blessed number, and it was a correction rather than a regression.**
The torso+head baseline's collider-ready count went **4 → 1**. `supports_inside_outside` gained a
`non_manifold_vertices == 0` clause; the old policy checked boundary edges, non-manifold *edges* and
orientation but let a **bowtie vertex** through — and a bowtie breaks the pseudonormal construction
exactly as a bad edge does. **Ten of the twelve fragments carry one**, which is the torso/head seam
surfacing as a vertex fault rather than an edge fault. So the old 4 was an overcount, this crate had
published it in `docs/research-brief.md`, and that table is corrected in the same commit.

**The pre-registered falsifier did not fire.** AG-013 said it would be falsified if the bump changed
emitted *geometry* rather than only reported topology. Watertight (7), manifold (2), open cut edges (22)
and enclosed volume (0.1971) are all unmoved, and the re-fracture is still bit-identical. That is the
expected result rather than a lucky one: the fracture is `soup.rs` and this dependency only ever
measures it — which is the claim `tests/leaf.rs` makes when it admits `isomesh` at all.

**What it bought:** `weld_split_by` (`weld.rs:338`), the composite-key weld **AG-005 was scoped to
hand-roll**; and a public `predicates` module with `orient2d` and `incircle` — Shewchuk's robust
predicates, which is the floor **AG-008**'s constrained Delaunay triangulator stands on. Still `no_std`
with `libm` as its only dependency, so `tests/leaf.rs` needed no widening.

> **Falsified premise, and it cost the first twenty minutes of this ticket.** AG-013 was written against
> "isomesh is 229 commits ahead at `9a321b1`". That commit is in the **sibling working copy and was
> never pushed** — a git dependency cannot resolve a commit that exists on one machine. `origin/main` is
> `22c3b35`, a *different and further-along* lineage. The lesson generalises past this ticket: we had
> already been burnt once by reading a stale mirror of our own crate (A-1), and this is the same mistake
> with the repositories swapped — reading a working copy and calling it upstream.

> **Unlooked-for confirmation of Phase 1.** isomesh's own `T-022` splits a CDT ticket in two, and its
> note reaches our Tier A conclusion independently: *"under the Tier A architecture a cap is a plane
> intersected with a convex cell, which is provably a convex polygon and needs no CDT at all."* Two
> code bases arrived at the same architecture from opposite ends. It also means AG-008 stays what its
> own ticket says it is — a safety net, over-engineered on purpose — rather than the main event.

### ☑ AG-009 · Retire the monorepo's copy of this crate

*Rewritten before it was done — the original ticket ("re-split and push the public mirror") was void; see
A-1.* `foundation_vs_slop` now consumes this crate as
`bevy_autogib = { git = "https://github.com/Ladvien/bevy_autogib", rev = "ba3b13b" }`, is no longer a
workspace member, and `crates/bevy_autogib/` is deleted. Committed there as `f6ddc0f`.

**The feature forward was verified, not assumed.** `test-harness` forwards `bevy_autogib/strict-order`,
and that forward is what keeps the vertex-soup tie check alive in a release-built harness — losing it
silently was the failure mode worth checking for. `cargo tree -e features -i bevy_autogib --features
test-harness` shows `bevy_autogib feature "strict-order"` reaching the crate from
`foundation_vs_slop feature "test-harness"`, and `cargo check --lib --features test-harness` compiles
with the directory gone.

**Order of operations, chosen so nothing was irreversible until it was proven safe:** push this branch →
rewire the manifest → *resolve and verify the feature forward* → only then delete. A copy of the deleted
directory was taken to the session scratchpad first, and every file in it was confirmed older than A-1's
port, so nothing there was newer than what this repository already holds.

**Deliberate restraint on the other repository.** That working tree carries ~60 files of unrelated
in-flight work. The commit uses an explicit pathspec — manifest, lockfile, deletion — and touches none
of it. The lockfile diff was checked first and is entirely this change: ten inserted lines, the git
sources for `bevy_autogib` and `isomesh`.

**Pinned to a branch rev, and that is temporary.** `ba3b13b` is on `backlog`. When that branch merges,
re-pin to the merge commit on `main`.

---

## Phase 1 — The architecture

### ☑ AG-001 · Tier A / Tier B split

The crate no longer cuts the triangle soup. `src/proxy.rs` is Tier A: a convex `ProxyCell` and the plane
cut that divides one into two. `fracture_mesh` takes `&[ProxyCell]`, cuts **only** cells, and carries the
render triangles as a payload — clipped by the same plane, never capped. The cap is the cell's new face.

**The pre-registered prediction, recorded against the outcome: CONFIRMED.**

| | predicted | measured |
|---|---|---|
| proxy fragments closed | 12/12 | **12/12** |
| manifold | 12/12 | **12/12** |
| χ = 2 | all | **all** |
| open cut edges (proxy) | 0 | **0** |
| volume conserved | 1e-3 | **1e-3** |

`every_proxy_fragment_of_the_two_shell_subject_is_closed` is the test. The soup cutter scored 7/12
watertight and 2/12 manifold on the same fixture with 22 open cut edges; `AG-012` pinned those numbers
so this comparison could exist at all.

**The secondary prediction also held, and it is why it was written down in advance:** the *render*
fragments still carry open edges, and that is correct rather than a regression — a render fragment is a
surface subset, not a solid. `audit_proxy` is added as the companion to `audit_fragment` so the two
artefacts can be measured separately, which is the API half of `AG-004`.

> **Deviation, deliberate: a fragment is exactly one cell, not "a set of cells on one side".** Each cut
> splits one cell in two, so the set never has more than one member. This pays twice — the fragment is
> trivially closed and convex, and `AG-007` gets a solver-ready collider with no decomposition at spawn.
> Partial fracture and compounds, which are what the "set" formulation is for, are not something this
> crate does.

> **Deviation: `min_extent` became `min_fraction`, a *linear* fraction cubed internally.** Cell selection
> is by **volume** — `Soup::extent`'s sliver pathology would otherwise have been inherited wholesale, so
> the first half of `AG-011` is delivered here out of necessity. Comparing the caller's 0.15 directly
> against a volume ratio proved ~4× stricter and silently returned 11 fragments where 12 were asked for;
> cubing restores what every existing caller meant.

> **Falsified premise, found by measuring rather than reasoning — T-junctions.** With a proxy that is
> *exactly* the subject's own box, the render skin plus its cap still came back open: 13–19 boundary
> edges per fragment. The cause is not the architecture but arity. The cap is the cross-section of the
> **cell**, so it has one vertex per cell edge crossed; the skin's opening is the cross-section of the
> **triangulated mesh**, so it has one per triangle edge — including the diagonals a quad is split with.
> Those extra points sit exactly on the cap's edges without being cap vertices.
>
> **The obvious fix corrupts the solid, and this was measured too.** Inserting the seam points into the
> cell's face leaves its neighbouring faces on the coarse edge, moving the T-junction inside the cell:
> proxy boundary edges 0 → 16, χ 2 → −3. The weave therefore happens at **emit** time only
> (`weave_seam`), leaving the cell pristine. Open edges fell from 13–19 to 3–9 per fragment. Not
> eliminated — `convex_ring` dedupes on the `WELD` lattice and drops seam points within 1e-4 of a corner
> — and deliberately not chased further, because `AG-004` is where the render metric gets its own
> treatment.

**Deleted, not deprecated** — `CLAUDE.md` forbids the old cutter surviving as a fallback: `cap_side`,
`assemble_loops`, `cut_segment`, `weld`, `push_cap_tri` and `split_soup` are gone, and with them the
three `known_defect_` tests that asserted their bugs. Those findings live in this archive and in
`docs/isomesh-upstream-asks.md`; the code they indicted does not exist to be re-broken.

**Also landed:** `FractureProxy` component (a subject without one is `error!`-refused, never given a
synthesised box); a fourth entry in `CLAUDE.md`'s "Where the boundary falls"; `README.md`'s wiring
snippet and "What it deliberately does not do"; all three examples; and `docs/fracture-tier-ab.gif`,
where every fragment is green.

> **Unlooked-for confirmation.** isomesh's own `T-022` note, found while working `AG-013`, reaches this
> architecture independently: *"under the Tier A architecture a cap is a plane intersected with a convex
> cell, which is provably a convex polygon and needs no CDT at all."*

### ☑ AG-004 · Move each metric to the artefact it describes

`FragmentAudit` is gone, split in two so the category error cannot be written again:

| type | artefact | carries |
|---|---|---|
| `SolidAudit` | the proxy cell — a closed convex polyhedron | χ, genus, volume, `is_closed`, `supports_inside_outside` |
| `SurfaceReport` | the drawn skin ∪ cut face — a surface *subset* | counts only. **No `is_closed`, no volume** |

`audit_render` returns the second; `audit_proxy` and `audit_proxies` the first. The point is not naming
hygiene — `SurfaceReport` has no closure predicate, so "is this render fragment watertight?" is a
question the type system now refuses to let anyone ask.

`examples/fracture_cube.rs` prints them under two headings that cannot be added together. The solid
reads **12 of 12** on watertight, manifold, χ = 2 and collider-ready, enclosing 0.2493 — which is
`0.21 + 0.0393`, the two cells exactly.

> **A claim written into this ticket was wrong within the hour, and the fix was to measure.** The first
> draft of the surface heading labelled non-manifold features and inside-out edges "← zero IS expected
> here". The two-shell subject reported 11 and 8. Rather than soften the wording, both were measured
> against a single closed shell (a lone cuboid, 8 pieces): **33 open edges, 3 non-manifold, 0
> inside-out**. So:
>
> - **inside-out edges** genuinely are zero for one shell — clipping preserves winding. They appear only
>   where *two shells meet*: torso and head touch at `y = 0.5`, their coincident faces weld together,
>   and those now-interior faces disagree with their neighbours about which way is out. A property of
>   the subject, not the fracture. `AG-003`.
> - **non-manifold features** are *not* zero even for one shell. Three of them, from the audit's
>   position-only weld merging vertices the shipped mesh keeps apart. Recorded in the field's doc as a
>   measured baseline rather than asserted away.

Both numbers are now printed side by side in the example, so the next reader gets the comparison rather
than an unqualified claim.

### ☑ AG-007 · Colliders from the proxy

`ProxyCell` gained the accessors a solver actually wants: `points()` (the collider), `faces()`,
`volume()` (mass properties) and `center()`. Both `Fragment` and `FragmentGeometry` carry the cell.

**`points()` is the whole ticket in one method.** Every convex-hull collider constructor takes a point
cloud — parry's `ConvexPolyhedron::from_convex_hull`, Rapier's `ColliderBuilder::convex_hull`, Avian's
`Collider::convex_hull`. A fragment *is* one convex cell, so there is no decomposition to run at spawn
and no trimesh to fall back to. That is Müller's architecture paying twice: the decomposition that makes
the cut robust is the one the solver wanted anyway.

`half_extents` survives, documented as **a coarse bound, not the collider**. It still sizes culling and
the launch impulses an example computes, and removing it would have broken callers for no gain.

Both examples now take their resting height from `cell.points()` rather than `half_extents.y`, with the
one-line collider a real game would build written in the comment beside it. On a plane-cut shard the
bounding box and the actual shape differ a lot, and having the example quietly use the box would have
undercut the ticket it is demonstrating.

**Boundary held:** the crate hands out cells and stops. It still names no solver.

## Phase 2 — Safety net and cleanup

### ☑ AG-011 · Stage-1 defects that outlive the rewrite

**The extent metric: fixed, and it had to be.** Cell selection is by **volume** (`soup.rs`, the
`fracture` driver). `Soup::extent` picked the largest bounding half-dimension, so a flat sliver with one
long axis kept winning "largest piece" and being re-cut while compact pieces were never touched. Tier A
would have inherited that wholesale, so this half was delivered inside `AG-001` out of necessity rather
than waiting its turn. `Soup::extent` survives at one call site — sizing the target piece count from the
subject's overall size — which is a legitimate use and not the selection bug.

**The async bake: settled by measurement, and the answer is no.** The fracture takes **0.33 ms** for a
12-fragment torso-and-head subject (release, stable to ±0.01 ms across four runs). The ticket's own
threshold was "warranted at 50 ms and not at 5 ms", so this is an order of magnitude the safe side. The
timer is committed in `examples/fracture_cube.rs` so the number can be re-checked rather than trusted,
and the reasoning is recorded at the call site in `bake.rs`.

> **Falsified premise.** AG-011 described the async bake as existing code with a latent defect —
> "`AsyncComputeTaskPool::spawn` resolves to the single-threaded pool and **runs the 'async' bake inline
> on the main thread**". There is no async bake. `AsyncComputeTaskPool` appears nowhere in this crate;
> the bake is a plain synchronous system. The hazard described is real but *prospective*: it is what
> would happen **if** someone added one, because `bevy/multi_threaded` is deliberately undeclared, so
> the pool would be single-threaded unless another crate turned the feature on through unification. One
> code path that is concurrent in some consumers' builds and not others is precisely what the one-path
> rule exists to prevent. That argument is now a comment beside the call, where whoever reaches for
> `spawn` will read it.

### ☑ AG-005 · Attribute-aware weld

`soup_to_mesh` welds on a composite key — **position class + quantised normal + quantised UV**.
Measured on the torso+head subject: **3.00 → 1.46 vertices per triangle**, a 51% reduction over 341
triangles. `fracture_output_is_bit_identical_across_runs` stays green and `explode` renders with its
creases crisp.

The ticket's diagnosis was exactly right: `push_tri` allocates three fresh vertices per triangle and the
old remap keyed on the *old soup index*, which is unique per corner by construction — so it compacted
the buffer and merged precisely nothing.

**The two quantisations fail in opposite directions, and that is deliberate.** Position uses a 27-cell
probe rather than a bare lattice lookup, because two positions a few ULPs apart can straddle a cell
boundary, and a missed merge there is not a lost saving — the vertices are the *same point*, so leaving
them apart is what makes a seam. Normal and UV use bare buckets, because a near-miss there costs one
vertex while a false match smears a crease. Erring toward keeping vertices apart is safe for an
attribute and unsafe for a position.

**The test asserts a pair, not a number.** Shipped vertices must be under 2.0 per triangle *and*
strictly greater than what a position-only weld yields. The gap between the two is the crease surviving:
every vertex in it is a corner where skin meets a cut face, or one cut face meets another. A fix that
merged as hard as a position weld would pass a naive count assertion and destroy the visual read.

> **Amendment: `isomesh`'s `weld_split_by` was available and deliberately not used.** `AG-013` recorded
> it landing and this ticket was re-scoped to use it. Doing so would put `isomesh` in the *shipping*
> path, and `tests/leaf.rs` states the terms it was admitted on: *"a second opinion about the output,
> not a source of it."* Every emitted vertex would then depend on its welder, and a change there would
> move geometry this crate promises is reproducible. `MeshBuffer` also carries no UV channel, so the
> round trip would have had to rebuild UVs through `remap()` regardless. Hand-rolled, with the 27-cell
> probe that made `Welder` worth citing, and the reasoning recorded on the type.

### ☑ AG-003 · Open shells as a separate class

`shells()` partitions the render soup into connected components (union-find over welded positions) and
marks any component with a boundary edge as **open**. Open shells are assigned whole to the fragment
whose cell contains their centroid, kept in `Piece::sheets`, and **never clipped**. On a split they move
wholly to one side, chosen by the sign of the centroid's distance to the plane — total, deterministic,
no fallback branch.

`an_open_shell_survives_the_fracture_whole` hangs a single-quad cape on the torso and asserts it comes
out on **exactly one** fragment with **both** triangles.

> **The danger changed shape between the ticket being written and being done.** AG-003 was written
> against the old cutter, where the hazard was *capping*: a sheet has no interior, so closing its "cut"
> emits a degenerate solid. Under Tier A/B nothing caps the render mesh any more, so that specific
> failure is gone — but the sheet would still have been **clipped** into pieces by every plane crossing
> it, separating geometry the artist drew as continuous. Same ticket, different mechanism; the fix is
> the same class distinction either way.

**This is also Müller's island detection**, which he lists as a required step rather than an
optimisation — *"crucial… it is this step that makes sure that objects collapse in the correct way."*
The pass does double duty here: it finds the sheets, and it is the same connectivity a compound fracture
would need.

> **An hour lost to a fixture collision, recorded so the next fixture is placed more carefully.** The
> first cape sat at `z = -0.16`… no: at `z = -0.17`, which is *exactly* the head box's back face. The
> test's detector matched the head's own triangles and reported the cape "split across 3 fragments"
> when it was carried correctly all along. The classification was right from the first run; the
> assertion was measuring three coplanar surfaces. The cape now sits at `-0.16`, a plane no box face
> occupies, with the reason written beside it.

### ☑ AG-008 · Replace loop recovery with a CDT over a PSLG — **resolved differently, and deliberately**

**No triangulator was written, and that is the finding rather than a shortcut.** The ticket's own body
predicted this: *"Under Tier A/B this capper only ever sees convex cross-sections, so it is
over-engineered by design."* Between the ticket being written and being reached, three things changed:

1. **The four failure modes it listed no longer exist.** Figure-eight loops, crossing segments,
   non-convex sections and nested-loop-as-disc were all properties of `assemble_loops`, which `AG-001`
   deleted. A plane meets a convex cell in a convex polygon; there is no loop to recover.
2. **Its acceptance criterion became unsatisfiable.** It asked that AG-002's and AG-006's
   `known_defect_` tests "flip to their correct form in this commit". Those tests were deleted with the
   code they indicted. Their findings live in this archive and in `docs/isomesh-upstream-asks.md`.
3. **Upstream reached the same conclusion independently.** isomesh's `T-022` splits its own CDT ticket
   and notes: *"under the Tier A architecture a cap is a plane intersected with a convex cell, which is
   provably a convex polygon and needs no CDT at all."*

**The residual risk is real, and it is answered by refusing rather than by surviving.** A caller can
still hand in a slightly concave cell — a decomposer's output is approximate — and a concave cell yields
concave cut faces whose centroid fan folds. AG-008 proposed a CDT so that input would not corrupt the
output. That is the wrong shape of fix under this project's own rules: `CLAUDE.md` says a primary path
that cannot produce a usable result must **fail loudly**, never write a degraded substitute. A
triangulator whose only job is to make a broken proxy look like it worked *is* the degraded substitute.

So `ProxyCell::new` checks convexity once, at the caller's boundary, and refuses with a message naming
the offending face and the deviation. `from_box` bypasses it because a box is convex by construction and
is this crate's own geometry. `clip` needs no check: convexity is an invariant once admitted, since a
plane through a convex polyhedron yields two convex polyhedra — re-verifying per cut would cost a pass
over every (face, vertex) pair for an answer that cannot change.

Two tests: a cube with one corner dented inward is refused (and the same geometry admitted while it is
still a box), and boxes at 0.01, 1.0 and 100.0 half-extent are all admitted — the tolerance scales with
the cell's size, so a large cell is not rejected for being large.

**If a caller ever genuinely needs concave cells supported**, this is the ticket to reopen, and the
route is Shewchuk's PSLG flood fill as originally described — with `isomesh`'s `predicates` module
(`orient2d`, `incircle`, landed at the rev `AG-013` pinned) as its exact-arithmetic floor.

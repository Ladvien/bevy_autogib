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

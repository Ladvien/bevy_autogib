# bevy_autogib — BACKLOG

**Updated:** 2026-08-16
**Companions:** `CLAUDE.md` (rules), `README.md` (what the crate promises),
`docs/research-brief.md` (the open problems), `docs/isomesh-upstream-asks.md` (what we need from the
validator).

**15 tickets archived, 0 open.** The architectural change this backlog opened with has landed: the
crate no longer cuts the triangle soup. It cuts a caller-supplied convex proxy and carries the render
triangles along as a payload.

**What survives here is the reasoning, not a work queue.** The sections below — the architecture
argument, and the two corrections carried in from research — are kept because they explain *why* the
crate is shaped as it is, and because both corrections turned out to need corrections of their own.
Every ticket, with what it cost and what it falsified, is in `BACKLOG_ARCHIVE.md`.

**One piece of history worth keeping at the top.** This crate is now an independent repository and
`foundation_vs_slop` consumes it as a pinned git dependency — the reverse of the arrangement most of
these tickets were written under. Everything they called "Stage 1" (the audit harness, the `isomesh`
dependency, both research docs, this file) had **never been committed anywhere**; it lived untracked in
the monorepo working tree, which is why nine of the original eleven tickets named files that did not
exist in the published crate. See `BACKLOG_ARCHIVE.md`, A-1.

---

## How this backlog was worked

1. Take the **topmost unblocked, unchecked ticket**. The order encodes dependencies.
2. One ticket = one commit (or a short stack). Commit message starts with the ticket ID.
3. **Check the box in this file as part of that same commit.** This file is the state.
4. If a ticket can't be finished, leave it unchecked, add a `> BLOCKED:` line saying exactly what is in
   the way, and move to the next unblocked ticket. Do not half-finish and check the box.
5. On completion, move the row to `BACKLOG_ARCHIVE.md` with an indented annotation recording any
   amendment, deviation, or **falsified premise**. The annotation is the point; the checkmark is not.
6. A ticket with a **pre-registered prediction** records the outcome against it, including when the
   prediction was right. A prediction nobody wrote down before the run is not evidence.

### Definition of done — applies to every ticket

- `cargo test` green. `cargo clippy --all-targets` introduces no new warnings (three pre-exist in
  `bake.rs`, `soup.rs` and `mesh.rs`). *No `-p` flag: this is a standalone repository now, not a
  workspace member.*
- **`cargo build --release` passes.** Not redundant with `cargo test`: the dev-dependency pulls the full
  `bevy` umbrella and enables features the trimmed `[dependencies]` set does not. A missing feature is
  only visible in the release build.
- **`cargo build --examples` passes**, and any ticket that changes emitted geometry re-runs
  `cargo run --release --example capture` and regenerates its GIF through `tools/gif.sh`. A change to
  the fracture that nobody looked at is a change nobody checked.
- `tests/leaf.rs` green — the crate stays game-free and its dependency list stays closed. Widening
  `ALLOWED_DEPS` requires the justification in the same commit.
- No `unwrap()`, no `expect` on caller data, no panicking index. Malformed input is `warn!`-skipped.
- Determinism holds: `fracture_output_is_bit_identical_across_runs` must stay green. If a ticket
  legitimately changes emitted geometry, say so in the commit and re-bless deliberately.
- Anything with a sign convention, winding order, or coordinate order says so **in the doc comment**.

**Size key:** `S` ≈ one sitting · `M` ≈ a day · `L` ≈ multi-day, consider splitting.

---

## The architectural change, in one section

Everything in Phase 1 follows from one finding, so it is stated once here rather than repeated per
ticket.

**Production fracture does not cut the mesh.** Müller, Chentanez & Kim (`10.1145/2461912.2461934` — the
NVIDIA lineage behind PhysX Blast, already cited in our README) cut a **volumetric convex
decomposition** and carry the visual triangles as a payload uniquely assigned to a cell. Booleans
become convex ∩ convex, which is trivially robust.

The load-bearing consequence: **plane ∩ convex polyhedron = convex polygon.** Every cap is therefore a
convex cross-section, and the existing centroid fan is *provably correct* for all of them, unchanged.

**This explains our own measurement.** Stage 1 found a cuboid fractures 8/8 clean while the torso+head
fixture scores 7/12 watertight and 2/12 manifold. That is not luck and not two bugs — the cuboid is
**convex**, so every cross-section it can produce is convex, so the fan is valid. The capper was never
broken for convex input and is not fixable for non-convex input. Sellán et al. (`10.1145/3549540`,
*Breaking Good*) reach the same architecture independently, and their transfer step yields *"the
exterior surface of each fragment component is exactly a subset of the input mesh"* — which is the
property that keeps skin UVs.

**The shape:**

- **Tier A — the proxy.** Convex cells, per *connected shell*. Recursively plane-cut **only the cells**.
  A fragment is a set of cells on one side. Colliders, cut caps and fragment identity all come from here.
- **Tier B — the render mesh, never topologically cut.** Assign each input triangle to the fragment
  whose proxy cell contains its centroid. Split only *straddling* triangles against the plane — a
  triangle-plane split is exact and **needs no loop recovery, ever**.
- **Never union the shells.** Cut each independently; associate fragments by proxy-cell provenance, not
  by surface overlap. This is measured, not theoretical: beyond Takayama et al.'s objection to using
  generalized winding number for mesh *repair*, Sacht et al. ran exactly this experiment on
  interpenetrating character limbs and report the legs sticking together and the arms sticking to the
  belly and head. For gibs that is not a quality loss but a **correctness** loss — it destroys the
  ability to separate the head from the torso, which is the entire point of the crate.

**The proxy is supplied by the caller.** This is a deliberate boundary decision, and it makes AG-001
unblocked rather than gated on a convex decomposition nobody has written. It also dodges the
solver-dependency problem: our game can hand in parry's VHACD output (already in the tree via
`avian3d`), while a consumer on a different solver hands in something else. It adds a fourth entry to
**`CLAUDE.md`'s** "Where the boundary falls" — *not the README's, which has no such section; its
analogue is "What it deliberately does not do".*

> **Do NOT use Convex Primitive Decomposition** for the proxy. CPD (`10.48550/arXiv.2602.07369`) *wraps
> the outside* in overlapping primitives — it is a collision proxy, not a filling. Two disqualifiers:
> there is **no interior to cut**, and its enclosure guarantee makes the wrapper strictly *larger* than
> the shape, so every fragment comes out fat. "Guarantees enclosure" is a virtue for *"did I bump into
> this?"* and the wrong sign for *"cut this."* Use **V-HACD or CoACD** — genuinely volumetric.

---

## Two corrections carried in from research — do not re-derive these

Both were reported to us as fact and both are **false**. They failed the same way: **reading intent as
implementation**. Recorded here because the cost of re-checking is small and the cost of building on
them is not.

1. **"`isomesh` is not in `bevy_autogib`'s `Cargo.toml`; it appears in test usage only."** False as a
   statement about the crate, and **the reading that produced it was fair**. It is a real
   `[dependencies]` entry pinned to `rev = "4369e3c"`, with `ALLOWED_DEPS` widened in the same commit —
   but at the time that entry existed only in the monorepo's *working tree*. It was in no commit, in
   either repository, so an agent reading published history could not have found it. **This is now
   fixed at the root**: `Ladvien/bevy_autogib` is the source of truth and everything is committed here.
   See `BACKLOG_ARCHIVE.md` A-1, and AG-009 for retiring the monorepo copy.
2. **"`signed_distance_from_mesh_winding → SampledField::new → ManifoldDualContouring` works end to end
   today, roughly three lines."** False **at the rev we pin**, which is the only rev that can affect a
   build: none of those symbols exist at `4369e3c`.<br><br>
   **But this correction has itself gone stale, and re-checking it is why AG-013 exists.** isomesh's
   `HEAD` is no longer `4369e3c` — it is **229 commits ahead**, and `signed_distance_from_mesh_winding`,
   `SampledField` and `MeshField` all exist there now. So the claim was wrong about *when*, not about
   *what*. Two things stay true regardless: convex decomposition is still absent, and the third link in
   that chain is refuted by upstream's own source — Manifold Dual Contouring reads an eager N³ grid like
   every other extractor, so "queries where it needs to" was never right about it. See AG-010.

**The architecture argument in the research is unaffected by either.** Its claims about *existing
capability* should be independently verified before anything depends on them.

---

## The backlog is clear

All fifteen tickets are in `BACKLOG_ARCHIVE.md`, each with what it cost and what it falsified. Six
predictions were pre-registered; **five came back different from what was predicted**, and those
differences are the most useful thing this backlog produced:

| ticket | prediction | outcome |
|---|---|---|
| AG-001 | 12/12 proxy fragments closed, manifold, χ = 2, 0 open cut edges | **confirmed exactly** |
| AG-002 | χ and manifoldness conserved; only volume notices a filled bore | **falsified** — χ moves, orientation moves, volume is the field that *misses* it |
| AG-006 | fold ⟺ inconsistent orientation | **narrowed** — sufficient, not necessary; a doubly-wound fan folds with every counter at zero |
| AG-013 | falsified if the bump moves geometry | **held** — only a reported number moved, and it was ours being wrong |
| AG-011 | the async bake runs inline on the main thread | **falsified** — there is no async bake |
| AG-008 | a CDT is needed as the safety net | **falsified by AG-001** — refuse concave input instead of surviving it |

Two false claims were found in this crate's own source (`signed_volume`'s translation invariance, and
the fold equivalence we had already sent upstream), and one in the backlog's own corrections section.

### What is deliberately not here

- **A convex decomposition.** The proxy is the caller's; see `CLAUDE.md`'s boundary list.
- **A constrained Delaunay triangulator.** AG-008 explains why refusing concave cells beats surviving
  them. Reopen it if a caller genuinely needs concave support; `isomesh`'s `predicates` module is the
  exact-arithmetic floor it would stand on.
- **An async bake.** Measured at 0.33 ms; see AG-011.
- **Full closure of the render mesh.** Open edges fell from 13–19 to 3–9 per fragment with the emit-time
  seam weave, and the remainder is `convex_ring` deduping seam points within `WELD` of a corner. It is
  recorded rather than asserted, because a surface subset has a boundary by definition.

---

## Reading order

1. **Müller, Chentanez & Kim 2013** — `10.1145/2461912.2461934`. §1–2 and the VACD section. The
   production answer; dissolves the capper problem as a side effect.
2. **Shewchuk 1996, *Triangle*** — `10.1007/bfb0014497`. The PSLG definition and the hole/concavity
   flood fill.
3. **Diazzi & Attene 2021** — `10.1145/3478513.3480564` (impl at `github.com/MarcoAttene/VolumeMesher`).
   §2 on why CDT-based methods fail on defective input, and the cell classification. Probably not usable
   as a dependency (C++), but it is the one method whose *stated* input tolerance matches a real glTF
   character — self-intersecting, non-manifold, disconnected, holes and gaps — so it tells you what the
   tidy-up step in V-HACD/CoACD is actually costing you.

Ten-minute runner-up: **Sacht et al.**, *Consistent Volumetric Discretizations Inside Self-Intersecting
Surfaces*, Figs. 10–11 — the picture of a generalized-winding-number union welding a character's limbs
to its torso. That figure is the whole argument for never unioning the shells.

# bevy_autogib — BACKLOG

**Updated:** 2026-08-16
**Companions:** `CLAUDE.md` (rules), `README.md` (what the crate promises),
`docs/research-brief.md` (the open problems), `docs/isomesh-upstream-asks.md` (what we need from the
validator).

**15 tickets archived, 4 open (AG-015 … AG-018).** The architectural change this backlog opened with
has landed: the crate no longer cuts the triangle soup. It cuts a caller-supplied convex proxy and
carries the render triangles along as a payload.

**Phase 3 opens the crate to gameplay-driven fracture** — see "Phase 3" below. It answers
`docs/research-brief.md`'s P4, which had been ranked last and never worked.

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

## Phase 3 — gameplay-driven fracture

**The problem, stated as it was reported:** the demo "looks like we took a guy, froze him, and then
shattered him." That is structural, not cosmetic, and it has three separate causes.

1. **One bake = one outcome.** Every death produced the same fragment set, all at once, all over the
   body, with no relation to what killed it. `examples/explode.rs` makes it literal: intact for
   2.5 s, then the whole subject is despawned and replaced.
2. **No fragment had an identity or a neighbour.** `Fragment` carried a centre, an extent and a
   cell. No id, no parent, no adjacency — so breaking off *one* piece was not expressible.
3. **The cut geometry produces uniform convex shards.** Always-split-the-largest-by-volume drives
   fragment volumes toward uniformity, and a plane through the centroid centres every piece.

**The answer the literature gives is the same in three places, and it is not "bake harder".** Müller,
Chentanez & Kim reject static pre-fracture precisely because "the number of hierarchical fracture
levels is fixed" and "there is no way to align fracture patterns with the impact location"; their fix
is a hierarchy plus runtime, impact-aligned *selection*, with island detection deciding what actually
separated. PhysX Blast generalises that to a chunk hierarchy, a support graph of bonds, and damage
programs mapping a geometric query onto which bonds break — its shader set (`ImpactSpread`,
`CapsuleFalloff`, `TriangleIntersection` "useful for sweeping-blade effects", `RadialFalloff`,
`Shear`) is one-for-one the behaviours asked for. Unreal's Chaos lands on a connection graph with
per-level damage thresholds independently.

**On the look specifically:** Sellán et al. (`10.1145/3549540`, already cited here) state that
Voronoi and plane-cut prefracture "results in recognizable, unrealistic pieces" because it is blind
to where a shape is weak — for a body, the thin cross-sections. DeepFracture
(`10.48550/arXiv.2310.13344`) gives the quantitative half: real fragment volumes follow Mott's
distribution, `P(V) = e^(−∛(ζV))` with `ζ = 6/V̄`. Many small, few large. Uniform sizes are the
signature of a geometric cutter.

**Where the boundary falls, decided before any code:** the crate gets *geometry only* — a hierarchy,
an adjacency graph, and pure functions from a geometric region to a fragment set. Health, damage
numbers, weapon identity, impulses and pooling stay with the caller. `CLAUDE.md`'s boundary list is
unchanged and `tests/leaf.rs` stays green; no new type is named for a weapon or a body part.

| | ticket | size |
|---|---|---|
| [x] | **AG-015 — the fracture hierarchy: one bake, every granularity.** Record the forest the cut loop already walks; keep parents instead of overwriting them. `FragmentTree`/`TreeNode`/`FragmentId`, frontier queries on `Fracture` and `FractureCache`, `FractureSettings::max_depth`. | M |
| [x] | **AG-016 — the bond graph: which fragments actually touch.** Parent–child bonds from the tree are free but insufficient — two leaves of a common ancestor need not touch. Müller's coplanar-face match (sort faces by \|d\|, match equal-\|d\| opposite normals, planar convex∩convex overlap for the area) is exact for convex cells. Plus stateless `islands(graph, broken)`. | M |
| [x] | **AG-017 — severance queries.** Five pure region→fragment-set functions: `spread` (nearest fragment then breadth-first along bonds with falloff — a bullet takes one chunk), `capsule`, `swept_triangle`, `radial`, `shear`. Falloff follows Blast: full inside `min_r`, linear to zero at `max_r`. | M |
| [ ] | **AG-018 — the cheap look fixes, and delete `impact_dir`.** Offset the cut plane along its normal by a hashed fraction instead of always through the centroid; weight piece selection by `volume * (0.5 + hash)` on a stable node id so sizes spread Mott-ward. **This is the stage that moves emitted geometry** — regenerate `docs/fracture-tier-ab.gif`. `impact_dir` biased only the first two cut *normals*, never the plane position, was hardcoded `None` by the bake and passed `None` by every caller; the runtime queries supersede it, so it goes. | S |

### AG-016, as landed

**Pre-registered prediction: the coplanar match recovers every cut-adjacency exactly and finds
nothing between the caller's root cells, so the torso and the head come back as two islands.**

**Half confirmed, half falsified — and the falsified half was ours, not the algorithm's.** Cut
adjacency is recovered exactly, as predicted: all seven bond tests passed on the first run, including
connectivity and the localised break. But the two-shell fixture comes back as **one** island, not
two. The head cell's underside sits at `y = 0.5` and the torso cell's top face sits at `y = 0.5` —
they are exactly coplanar, so there is a real shared face and the match correctly finds it. The
example had been written asserting the opposite before it was run.

That is worth keeping because the prediction was not idle: it was reasoning about interpenetrating
shells, which is a real case (`BACKLOG.md`'s Sacht et al. note), and this fixture simply is not one.
The two boxes *abut*. The refusal being tested is still pinned, by
`cells_that_do_not_share_a_face_are_not_bonded`, on cells that genuinely interpenetrate.

**Measured on the standard fixture:** 36 bonds over 12 finest fragments; intact, one island;
severing one fragment's 3 bonds leaves islands of sizes `[11, 1]` — the localised break-off working
on real baked geometry.

**No proximity fallback was added, deliberately.** Cells that touch without agreeing on a face get no
bond, which is the normal case between V-HACD or CoACD root cells. Approximating there would weld a
head to a torso with a tolerance no caller could tune, so it is refused and documented instead.

> **Weak-axis bias is deliberately deferred.** Choosing each cut normal to minimise cross-section
> area — a cheap stand-in for Sellán's fracture modes, and what would make a character come apart at
> neck and wrist rather than into shards — is the obvious next step after AG-018 and was scoped out
> of this phase on purpose.

### AG-015, as landed

**Pre-registered prediction: keeping the parents changes no emitted geometry, and every currently
green test stays green without re-blessing.**

**Confirmed.** The cut sequence is preserved by keeping selection and the seed mix on *frontier
slots* rather than node ids — a cut still reuses its own slot for the `above` half and pushes a new
slot for `below`, exactly as the flat loop did, so the fact that ids no longer coincide with frontier
positions is invisible to the plane sequence. All 36 unit tests, `tests/leaf.rs` and the doctests
passed unchanged, including `fracture_output_is_bit_identical_across_runs`, `hash_f32_is_frozen`,
and both `== 12` fragment-count assertions.

**One thing was falsified, and it was a number we had been quoting.** `bake.rs` recorded the bake at
**0.33 ms**. Re-measured on this machine, the *pre-change* code takes **~1.4 ms** and the hierarchy
takes **~2.2 ms** — the ratio tracks node count (23 built instead of 12), which is the honest cost of
keeping every piece the loop split. Both are far under AG-011's 50 ms threshold so the
main-thread conclusion is unchanged, but the 0.33 ms figure was being repeated as though it had been
re-checked, and it had not.

**Two shapes had to change to keep the array index-parallel with the tree.** `geometry_from_piece`
was fallible and its caller dropped the `None`s; dropping an entry would slide every id after it onto
the wrong node, so it is now total — a piece that draws nothing keeps its slot with both meshes
`None`, bounded by its cell, which is still a perfectly good convex collider. And `fracture_mesh`
now returns `Fracture { fragments, tree }` rather than a flat `Vec`, because the flat `Vec` no longer
has a single meaning: `into_leaves()` is the old one.

---

## The rest of the backlog is clear

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

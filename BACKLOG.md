# bevy_autogib — BACKLOG

**Updated:** 2026-08-16
**Companions:** `CLAUDE.md` (rules), `README.md` (what the crate promises),
`docs/research-brief.md` (the open problems), `docs/isomesh-upstream-asks.md` (what we need from the
validator).

**8 tickets archived, 7 open.** Phase 0 is complete. This backlog opens with an architectural change: the crate is going to
stop cutting the triangle soup.

**Read this before working any ticket below.** This crate is now an independent repository and
`foundation_vs_slop` consumes it as a pinned git dependency — the reverse of the arrangement most of
these tickets were written under. Everything the tickets call "Stage 1" (the audit harness, the
`isomesh` dependency, both research docs, this file) had **never been committed anywhere**; it lived
untracked in the monorepo working tree, which is why nine of the original eleven tickets named files
that did not exist in the published crate. See `BACKLOG_ARCHIVE.md`, A-1.

---

## How to work this backlog

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

## Phase A — Independence

The crate stands on its own now. These are here for the record and because AG-009 is the half of it that
reaches into another repository.

| | ID | Ticket | Size | Blocked by |
|---|---|---|---|---|
| ☑ | **A-1** | **Port Stage 1 into this repository, verbatim.** Archived — see `BACKLOG_ARCHIVE.md`. | S | — |
| ☑ | **A-2** | **A way to see the defect, not just count it.** Archived — see `BACKLOG_ARCHIVE.md`. | S | — |
| ☑ | **A-3** | **Invert `CLAUDE.md`'s "Source of truth".** This repository is authoritative; the monorepo consumes it as a pinned git dependency. Keep the stale-read hazard on the record, re-pointed at the monorepo copy. | S | A-1 |
| ☑ | **A-4** | **Make this file honest about all of the above.** Header, definition of done (no `-p` flag), both "corrections carried in from research" — one of which had itself gone stale — plus AG-009 rewritten, AG-012 and AG-013 added, and three wrong anchors fixed. | S | A-1 |

---

## Phase 0 — Baseline before the rewrite

These run first because they measure the current behaviour. After Phase 1 the defects they capture
dissolve, and a fix nobody measured beforehand is indistinguishable from a fix that never happened.

| | ID | Ticket | Size | Blocked by |
|---|---|---|---|---|

---

## Phase 1 — The architecture

| | ID | Ticket | Size | Blocked by |
|---|---|---|---|---|
| ☐ | **AG-001** | **Tier A / Tier B split.** The change everything else depends on; see the section above for the full argument. Caller supplies convex cells per connected shell. Recursively plane-cut **only the cells**. Assign each input triangle to the fragment whose cell contains its centroid; split only straddlers against the plane. Never union shells; associate fragments by proxy-cell provenance.<br><br>**Pre-registered prediction**, on the existing torso-box + head-box fixture (currently 7/12 watertight, 2/12 manifold, 22 open cut edges) decomposed per connected shell — which for that fixture is trivially two cells, since each box is already convex — and cut with the same 12-plane sequence:<br>**→ 12/12 proxy fragments closed, manifold, χ = 2, volume conserved to 1e-3 — identical to the cuboid — and 0 open cut edges.**<br>**Falsified if** any *proxy* fragment reports open cut edges: that locates the defect in plane–cell intersection rather than in the shells.<br>**Secondary prediction, stated up front because otherwise the first run reads as a failure:** the **render** fragments still carry nonzero open edges, and **that is correct behaviour, not a regression** — a render fragment is a surface subset, not a solid. See AG-004.<br><br>**Implementation notes from the source papers, read before starting so they are not re-derived:**<br><br>• **Clip the cell's planes against the convex, not the convex against the cell.** Müller §3.3: *"we clip the convex against all the planes of the cell one by one and not vice versa. This way, each clip plane creates new vertices on the edges of the partially clipped convex for which the interpolation of vertex attributes such as normals is straight forward."* Attribute interpolation falls out of the loop order; get it backwards and it becomes a separate problem.<br>• **Represent a cell as a convex polyhedron with *polygonal* faces, never as triangles.** This is Müller's Figure 9 and it is stated as a warning: a hexagon split twice with triangular faces only produces *"many ill shaped triangles… even after only two cuts"*, where general polygonal faces produce none. Our slicer is triangles-only today, and this is the same sliver pathology **AG-011** records for `Soup::extent` — so Tier A must not inherit it. Triangulate faces once, at emit time.<br>• **Island detection is a required step, not an optimisation.** Müller lists it fifth and calls it *"crucial… it is this step that makes sure that objects collapse in the correct way"*: clipping can leave one compound holding two disconnected islands of cells. Under our shell rule this is also how a fragment learns it has split — see **AG-003**, which needs the same connectivity pass.<br>• **A fully-enclosed cell needs no clipped visual mesh.** Müller's welding step compares a cell's precomputed volume against the summed volume of the convexes assigned to it, and replaces the lot with the cell shape when they match. Worth having because an interior fragment is the common case in a dense fracture.<br><br>**One path:** this is a cutover, not a second backend. `CLAUDE.md` forbids the soup cutter surviving alongside it as a fallback.<br>**Acceptance:** the prediction above, recorded against the outcome either way; a fourth entry in **`CLAUDE.md`'s** "Where the boundary falls" naming the proxy as the caller's (*the ticket originally said the README's; that section lives in `CLAUDE.md`, and the README's nearest analogue is "What it deliberately does not do", which needs the matching line*); `fracture_mesh`'s signature and both examples updated to take a proxy. | L | — |
| ☐ | **AG-004** | **Move each metric to the artefact it describes.** A measurement fix, not a code fix. We are applying a **closed-solid test** (χ = 2, manifold, watertight) to a **render mesh that is not a solid**, and "2/12 manifold" partly reads as a bug because of that.<br>• The **proxy** is a solid → *assert* χ = 2, manifoldness, volume conservation.<br>• The **render mesh** is a surface subset → *record* open-edge count; **never assert on it**.<br>**Acceptance:** `FragmentAudit` splits along that line, `every_fragment_of_a_closed_solid_is_closed` targets the proxy, and `examples/fracture_cube.rs` reports the two classes under separate headings so they cannot be read as one number. | S | AG-001 |
| ☐ | **AG-003** | **Open shells as a separate class.** Capes, hair cards, decals and single-sided sheets have no interior, so they have no proxy cell and must **never be cut and capped** — capping a sheet produces a degenerate solid and the current code will try. Assign each open shell to a fragment by proximity and carry it whole.<br>**Acceptance:** a single-quad "cape" fixture attached to the torso+head fixture survives a fracture intact, attached to exactly one fragment, with no cap triangles generated for it. | M | AG-001 |
| ☐ | **AG-007** | **Colliders from the proxy.** Drop box-from-half-extents. A fragment is a set of convex cells, which is precisely what a solver wants — one convex collider per cell, no decomposition at spawn time and no trimesh. This is what makes the Müller architecture pay off twice.<br>**Acceptance:** `Fragment` exposes its cells; `half_extents` is either removed or documented as a coarse bound rather than the collider; `examples/explode.rs` uses cells. Keep the crate solver-agnostic — hand out cells, never a rigid body. | M | AG-001 |

---

## Phase 2 — Safety net and cleanup

| | ID | Ticket | Size | Blocked by |
|---|---|---|---|---|
| ☐ | **AG-008** | **Replace loop recovery with a CDT over a PSLG.** Shewchuk's `Triangle` (`10.1007/bfb0014497`) takes a **PSLG — a set of vertices and segments — not a polygon**, and that single change kills four of our failure modes at once:<br><br>• **figure-eight loop** — cannot be constructed; a self-touching vertex is just a degree-4 PSLG vertex<br>• **crossing segments** — resolved by inserting the intersection vertex; local, exact, no loop has to close<br>• **U / non-convex section** — the triangulation is constrained to segments, so the notch is never spanned and star-shapedness never arises<br>• **nested loop filled as a disc** — a flood fill halted at constrained edges, with **no containment query at all**, which is robust to welded shared vertices in a way parity testing is not<br><br>Shewchuk's own parenthetical is the design note: holes are handled this way to avoid *"a common outlook wherein one must define oriented curves whose insides are clearly distinguishable from their outsides."*<br><br>**An asymmetry in our favour:** the CDT-existence pathology Diazzi & Attene name (*"not guaranteed to exist for arbitrary input triangles"*) is **3D-only**. In a cut plane it never arises.<br><br>Under Tier A/B this capper only ever sees convex cross-sections, so it is **over-engineered by design** — that is the point. It is the safety net for non-convex proxy cells (a decomposer that returns slightly concave cells will not corrupt output) and it deletes the epsilon fan.<br>**Acceptance:** AG-002's and AG-006's `known_defect_` tests flip to their correct form in this commit. **If CDT + flood fill also fills the bore, the seeding is wrong, not the triangulator.** | L | AG-001 |
| ☐ | **AG-005** | **Attribute-aware weld.** Fragments currently ship **3 vertices per triangle** — `Soup::push_tri` (`soup.rs:76`) allocates fresh vertices for every triangle and `soup_to_mesh`'s remap keys on the old index, so it merges nothing. A bare position weld is not the fix: `isomesh`'s `Welder` never compares normals and discards the merged-away one, which would smear the skin↔cut-face crease — the entire visual read — and, on a fragment cut more than once, the creases between cut faces of different planes.<br>Use a composite key: **position class** (from `Welder::remap()`, whose epsilon-correct 27-cell probe a bare quantised key cannot match) **+ quantised normal + quantised UV**. Handle the new drop path: a sliver can survive the area filter and still collapse under the weld.<br>**Acceptance:** vertex count drops materially; `fracture_output_is_bit_identical_across_runs` stays green; `explode` renders with creases intact. | M | AG-001 |
| ☐ | **AG-011** | **Stage-1 defects that outlive the rewrite.** Two, deliberately grouped because neither justifies its own ticket:<br>• `Soup::extent` (`soup.rs:111`) is the max half-dimension, so a flat sliver with one large axis keeps being chosen as "largest piece" and re-cut. Under Tier A/B this applies to cell selection.<br>• **The async bake stays deferred, and this ticket records why rather than doing it.** autogib declares `bevy` with `default-features = false` and a list that excludes `multi_threaded`; `bevy_internal` takes `bevy_tasks` with defaults off and `std` does not forward to it. So `AsyncComputeTaskPool::spawn` resolves to the single-threaded pool and **runs the "async" bake inline on the main thread** — working only because a consumer's `DefaultPlugins` happens to enable the feature, i.e. behaviour arriving via feature unification. Both exits are bad: adding `multi_threaded` imposes a threading model on every consumer, and not adding it leaves one code path that is async in some builds and synchronous in others.<br>**Acceptance:** the extent metric fixed; the async question settled by *measuring* the bake first (put a timer around it in `fracture_cube`) and only then deciding — a fix is warranted at 50 ms and not at 5 ms. | M | AG-001 |

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

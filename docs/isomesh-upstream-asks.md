# What `bevy_autogib` needs from `isomesh`

Written from the consuming side, against `isomesh` at `4369e3c` — the rev `Cargo.toml` pins. Each ask
says what autogib does with it and what stays blocked without it, so the priority argument is legible
rather than asserted.

> **Status. Updated by AG-013, which moved the pin to `22c3b35` (`origin/main`).** Three of these five
> have been answered upstream. Current state, verified at both revs:
>
> | Ask | At `4369e3c` (the old pin) | At `22c3b35` (pinned now) |
> |---|---|---|
> | 1 — `TriangleGrid` public | `pub(crate)` | **still `pub(crate)`** — not granted as written |
> | 2 — mesh field | absent | **`MeshField` exists**, but pseudonormal-signed, not winding-signed |
> | 3 — attribute-aware weld | absent | **granted** — `weld_split_by` (`weld.rs:338`) |
> | 4 — convex decomposition | absent | still absent, and still declined |
> | 5 — fold inside a fan | absent | still absent |
>
> Also new and not an ask: a public `predicates` module with `orient2d` and `incircle` — Shewchuk's
> robust predicates, which is the floor AG-008's constrained Delaunay triangulator stands on. isomesh's
> own `T-022a` is a CDT ticket, not shipped code; **its note independently reaches our Tier A
> conclusion** — "under the Tier A architecture a cap is a plane intersected with a convex cell, which
> is provably a convex polygon and needs no CDT at all."
>
> **Two corrections about *where* upstream's work lives, both of which cost us time.** The sibling
> working copy at `~/isomesh` had an `HEAD` 229 commits past the old pin that was **never pushed**, and
> a git dependency cannot resolve a commit that exists on one machine. `origin/main` is a different and
> further-along lineage. Read the remote, not the working copy.
>
> **And the correction carried into `BACKLOG.md` as "correction #2" needs a qualifier of its own.** The
> `S-001…S-007` tickets this document argues against were **uncommitted intent** at the time of our
> audit — `signed_distance_from_mesh_winding` and `SampledField` did not exist at `4369e3c`. They exist
> now. So the claim we corrected was wrong about *when*, not about *what*.

**Context.** autogib pre-fractures a mesh by recursively plane-cutting a triangle soup and capping each
cut. isomesh is now a real dependency of it (`no_std`, one transitive dep, `[f32; 3]` public API — that
last property is why it was admissible at all: a crate pinning `glam` would have been refused, because
Bevy 0.19 wants 0.32 and `parry3d` wants 0.33). It is used today only to *measure* fractures. Asks 1
and 2 are what it would take to let isomesh *produce* them.

---

## Ask 1 — Make `TriangleGrid` and `point_triangle_distance_squared` public

**Where:** `crates/isomesh/src/validate/tri_grid.rs:82,141`. Both are `pub(crate)` and re-exported
nowhere.

**Cost:** a visibility change and doc comments. No new code, no new dependency, no new failure mode.

**Why autogib needs it.** These are the unsigned half of a mesh field: a CSR uniform grid anchored at
the mesh AABB, plus Ericson §5.1.5 point-triangle distance with the region/Voronoi classification. It
is the most portable geometry in the repo and autogib would otherwise reimplement it worse.

**One addition worth making while it is open:** `nearest_distance_squared` returns a scalar only. A
variant returning the winning triangle index and the closest point would serve normal reconstruction
and would cost nothing extra at the query site — the information is already computed and discarded.

**Blocked without it:** ask 2, and therefore autogib's whole SDF fracture backend — which is itself no
longer a blocker; see the re-scoping note on ask 2.

> **Not granted as written, and worth understanding why.** At `HEAD`, `TriangleGrid` and
> `point_triangle_distance_squared` are **still `pub(crate)`** at the same two lines. Upstream solved the
> problem a level up instead, by growing a public `MeshField` that performs the query itself and keeps
> the grid private. That is a defensible call — it exports a capability rather than an internal — but it
> means the "reimplement it worse" outcome this ask was trying to avoid is still live for any consumer
> that wants the raw distance query rather than a field.

---

## Ask 2 — A mesh field: distance magnitude × winding-number sign

> **Re-scoped. This was "the only hard blocker"; it is now optional.** The reason is not that isomesh
> changed — it is that autogib's critical path did. Tier A/B (AG-001) repairs the cutter by cutting a
> convex proxy, so an SDF backend stops being the route to correct fragments and becomes one possible
> route to a *different* kind of fragment. Nothing below is withdrawn; it is simply no longer blocking.
>
> **And the premise this ask was built on is false.** It argued for an on-demand `impl Sdf` partly
> because "sampling on demand is the right shape for Manifold Dual Contouring, which queries where it
> needs to rather than reading a precomputed grid". **MDC does no such thing.** `DualMesher::extract`
> calls `self.sample(sdf, shape, origin, cell_size)` (`dual.rs:251`) before anything else runs, and that
> function (`dual.rs:272-289`) loops every one of the N³ grid points into a `Vec<R>`. It reads a dense
> grid like every other extractor in the crate; the `Sdf` reference survives only to supply gradients.
> The claim came from a summary of the paper rather than from the source — and upstream has since
> written the same refutation into its own tree (`construct/from_mesh.rs:458-465`).

`S-007` ("Mesh → SDF by generalized winding number") is blocked by `S-006`, which is blocked by
`S-001` (exact Euclidean distance transform). autogib wants none of that chain: not a sampled distance
*volume*, but a `impl Sdf` whose magnitude comes from ask 1's grid and whose sign comes from a winding
number.

**Suggested split:** a ticket that depends on ask 1 alone, delivering roughly

```rust
pub struct MeshField<'a, R: Real> { /* positions, tris, TriangleGrid */ }
impl<R: Real> Sdf for MeshField<'_, R> { type Scalar = R; /* … */ }
```

S-007's own research notes already carry the important corrections and should be kept verbatim: do not
cite Barill 2018 as state of the art (the 2026 Antipodal paper, `10.1145/3811323`, calls its order-0/1
expansions "very imprecise… not useful for applications"); prefer Antipodal or Xie, Hafner & Wojtan
(`10.1145/3811339`); and use GWN to *classify points*, never to repair meshes (Takayama et al. 2014,
the GWN authors' own paper, calls the orientation-repair application "fundamentally flawed").

**The property that makes this cheap for autogib specifically:** the exact formulations reduce the
winding number to one ray-surface intersection plus a sum over **boundary** edges, so cost scales with
holes rather than triangles. autogib's input is artist-exported glTF characters — nearly closed, with
a handful of seams where a torso, a head and a held item meet. Nearly closed is nearly free.

**Why the pseudonormal route (`S-006`) does not serve autogib.** Bærentzen & Aanæs is a proof, and the
ticket is right that it is the correct tool for geometry isomesh produced itself. autogib's input is
the opposite case: S-007's framing, "for imported or damaged input", is a precise description of it.
A character merged from several closed shells is non-manifold exactly where those shells meet, and
that is where the pseudonormal's precondition fails.

**Blocked without it:** the SDF backend, and nothing else. **Not a hard blocker** — see the re-scoping
note at the top of this ask.

> **Upstream has measured this and the answer is no, at least in the shape asked for.** `HEAD` grew a
> `MeshField` (`construct/from_mesh.rs:501`), but it is **pseudonormal-signed**, which is the `S-006`
> route this ask argues does not serve autogib: it requires closed, consistently oriented input, and
> "an open mesh has boundary edges whose pseudonormal answers a question that has no answer, because
> there is no inside" (`from_mesh.rs:480-495`). The winding-number variant exists only as a *batch*
> function over a grid, and upstream records why an on-demand twin is not viable: `winding_numbers`
> casts **one ray per grid row** and amortises it across every sample in that row, so a per-point query
> would cast N³ rays for the same grid — "a factor of N, not a constant… there is no on-demand twin of
> it."
>
> So if the pin ever moves, the SDF backend is unblocked **by a different route than this ask
> describes** — a batch GWN field over a grid, which is precisely the thing this ask said autogib did
> not want. That trade deserves re-deciding on its merits rather than being inherited.

---

## Ask 3 — An attribute-aware weld

**Where:** `crates/isomesh/src/weld.rs`.

`Welder` keys on position alone. Normals are never compared and the merged-away vertex's normal is
silently discarded. That is correct for isomesh's own extractors, whose output has no hard edges to
lose, and it is the wrong default for any consumer that has them.

**Measured on autogib's side:** a position-only weld of a fracture fragment destroys the crease between
the subject's outer skin and the cut face — which is the entire visual read the crate exists to
produce — and, on a fragment cut more than once, the creases between cut faces of different planes too.
`Mesh::from(Cuboid)` is 24 vertices, three per corner with distinct normals *and* distinct UVs; a
position-only weld collapses each corner to one vertex and one arbitrary normal.

`remap()` is the documented escape hatch and it is genuinely useful — it carries parallel attribute
arrays through a merge — but it is a many→one map, so it can gather a UV through a merge already
decided, and cannot signal that a vertex *should have stayed split*. That information is destroyed
before `remap` is written.

**What would help, in preference order:**

1. A composite-key mode: weld positions, then split back apart where a caller-supplied key differs.
2. Failing that, document the two-stage recipe explicitly — `Welder` decides which positions coincide
   (its 27-cell probe is epsilon-correct in a way a bare quantised key is not, because two positions
   one ULP apart can straddle a lattice boundary), and the caller re-splits on `(class, normal, uv)`.

**Not blocked without it** — autogib can write its own composite key — but every consumer with hard
edges will hit this, and each will solve it differently.

> **Granted, at `HEAD`.** `weld_split_by` (`weld.rs:338`) is option 1 above: weld positions, then split
> back apart on a caller-supplied key. `epsilon_for` (`weld.rs:220`) came with it. This is the ask with
> the most direct consequence for us — **AG-005** was scoped to hand-roll exactly this against
> `Welder::remap()`, and if AG-013 moves the pin it should use `weld_split_by` instead and shrink to
> almost nothing.

---

## Ask 4 — Convex decomposition

**Where:** absent. `README.md:65` lists it under "Not yet"; `parry3d` is a dev-dependency only.

autogib currently hands each shard a box collider sized from its half-extents, which is a poor fit for
a plane-cut shard. Müller, Chentanez & Kim 2013 — already cited in autogib's own README — is
specifically about approximate convex decomposition for fracture, so this is the collider answer the
literature points at for exactly this workload.

**Until it exists**, autogib reports `collider::readiness()` per shard and leaves the collider choice to
the caller, which is the right boundary anyway: the crate hands out a mesh and stops. That is a stable
position, not a holding pattern — so this is the lowest-priority ask here, and it is listed because it
is the honest answer to "what would make the colliders good" rather than because it blocks anything.

---

## Ask 5 — Let the self-intersection counter see inside a fan

**Where:** `crates/isomesh/src/validate/self_intersection.rs:266-269`, and isomesh's own `M-83`.

`self_intersections` skips any triangle pair sharing a vertex index. autogib's caps are fans around a
shared apex, so every intra-fan pair is skipped — and a fan fold is the single most likely defect in
any capping or Steiner-fan triangulator, in both crates. isomesh already knows this about itself; M-83
records that the counter is blind to folds inside a Steiner fan.

An opt-in mode that tests vertex-adjacent (but not edge-adjacent) pairs would serve both.

**Not blocked without it.** autogib found its fan fold by another route, and that route is worth
passing back upstream — but **it is a sufficient condition, not an equivalence**, and an earlier
revision of this document offered it as one. Scoped correctly:

> A fan whose apex lies outside a **simply-connected** loop produces triangles of mixed signed area.
> `push_cap_tri` flips winding **per triangle** to face outward, so a folded triangle and its neighbour
> end up traversing their shared spoke edge in the *same* direction — which is exactly
> `inconsistently_oriented_edges`. Given a per-triangle flip and a welded mesh:
> mixed signs ⇒ `inconsistently_oriented_edges > 0`.

**Two qualifiers, both measured rather than reasoned:**

1. **The per-triangle flip is the mechanism, not an incidental detail.** It is what converts mixed
   signed area into a shared spoke traversed twice the same way. A capper that wound its fan
   consistently and assigned normals some other way would fold without ever moving the counter — so
   this is a fact about `push_cap_tri`, not a general property of `MeshReport`.

2. **The loop has to reverse, and it need not.** A closed path that winds around its own centroid
   **twice in the same direction** has no mixed signs at all: every fan triangle agrees, the surface is
   consistently oriented, and the fan still folds. A pentagram `{5/2}` is the minimal witness — fanned
   from its centre it covers the inner pentagon twice, so emitted area exceeds the star's true area by
   exactly the inner pentagon's area, while `inconsistently_oriented_edges`, `non_manifold_edges` and
   `non_manifold_vertices` are **all zero**. autogib commits it as
   `known_defect_a_doubly_wound_fan_folds_with_every_counter_at_zero`.

So `MeshReport` detects *one common class* of fan fold topologically, tolerance-free and with no narrow
phase, as long as the caller welds first. That is worth a line in the `validate` docs. **It also means
ask 5 is worth more to us than this document previously implied**, not less: the topological route
cannot see a doubly-wound fold, and a narrow-phase check inside a fan is the only thing that can.

---

## Summary

| Ask | Cost | Blocks | Status at `HEAD` |
|---|---|---|---|
| 1 — `TriangleGrid` / `point_triangle_distance_squared` public | visibility change | ask 2 | **not granted as written** — still `pub(crate)`; solved a level up by `MeshField` |
| 2 — mesh field (grid distance × GWN sign) | L | the SDF backend — **no longer a hard blocker**, Tier A/B repairs the cutter instead | partially: `MeshField` exists but is pseudonormal-signed; on-demand GWN measured **not viable** |
| 3 — attribute-aware weld | M | nothing, but every hard-edged consumer hits it | **granted** — `weld_split_by`; see AG-005 |
| 4 — convex decomposition | L | nothing; it is the collider *answer*, not a dependency | still absent, still declined |
| 5 — self-intersection inside a fan | S | nothing, but the cheaper topological route above is **sufficient, not necessary** — it cannot see a doubly-wound fold | still absent |

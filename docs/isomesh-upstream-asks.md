# What `bevy_autogib` needs from `isomesh`

Written from the consuming side, against `isomesh` at `4369e3c` — the rev `Cargo.toml` pins. Each ask
says what autogib does with it and what stays blocked without it, so the priority argument is legible
rather than asserted. Two of the five already exist as isomesh tickets and mostly need re-aiming.

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

**Blocked without it:** ask 2, and therefore autogib's whole SDF fracture backend.

---

## Ask 2 — A mesh field: distance magnitude × winding-number sign

**This is the one that matters, and it is narrower than the backlog currently makes it look.**

`S-007` ("Mesh → SDF by generalized winding number") is blocked by `S-006`, which is blocked by
`S-001` (exact Euclidean distance transform). **autogib needs none of that chain.** It does not want a
sampled distance *volume*; it wants a `impl Sdf` it can evaluate on demand, whose magnitude comes from
ask 1's grid and whose sign comes from a winding number. Sampling on demand is also the right shape for
Manifold Dual Contouring, which queries where it needs to rather than reading a precomputed grid.

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

**Blocked without it:** the entire SDF backend. This is the only hard blocker in the list.

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

| Ask | Cost | Blocks |
|---|---|---|
| 1 — `TriangleGrid` / `point_triangle_distance_squared` public | visibility change | ask 2, and thus the SDF backend |
| 2 — mesh field (grid distance × GWN sign), **split from the `S-001→S-006→S-007` chain** | L | the SDF backend — the only hard blocker |
| 3 — attribute-aware weld | M | nothing, but every hard-edged consumer hits it |
| 4 — convex decomposition | L | nothing; it is the collider *answer*, not a dependency |
| 5 — self-intersection inside a fan | S | nothing, but the cheaper topological route above is **sufficient, not necessary** — it cannot see a doubly-wound fold |

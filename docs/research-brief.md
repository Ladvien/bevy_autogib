# Research brief — runtime mesh fracture on real game assets

For a research agent with corpus access. Each problem below states what we **measured**, what we have
already **ruled out**, the **question**, and the **decision it unblocks** — so effort can go where an
answer would actually change what we build. Please distinguish "the literature says" from "the
literature assumes"; several of these are places where published work quietly presumes an input we do
not have.

---

## The system, in one paragraph

`bevy_autogib` pre-fractures a game asset once, at bake time, and swaps the pieces in when the thing
dies. It merges whatever meshes an entity actually loaded into one triangle soup, then recursively
plane-cuts it: pick the largest piece, cut it with a pseudorandom plane through its centroid
(Sutherland–Hodgman triangle clip), recover the cut boundary as welded loops, and cap each loop.
Fragments come back as two meshes — the subject's original skin and the newly-created cut faces — so
they can take different materials. Everything is `f32`, deterministic per seed, and the whole thing is
~1900 lines with one engine dependency.

We have just added a validator (`isomesh`) and measured the output for the first time. The numbers
below are what prompted this brief.

---

## What we measured

Two fixtures, both fractured with the shipping code, then welded and validated per fragment
(skin ∪ cap together — neither is a closed surface alone):

**A plain cuboid, 8 target pieces.** Every fragment: closed, manifold, consistently wound, χ = 2,
collider-ready. Volume conserved to 1e-3.

**The crate's own two-part solid — a torso box with a head box stacked on it, 12 pieces.** The metric
this crate shipped with says *"12 of 12 fragments carry at least one closed cut face."*

| | |
|---|---|
| watertight (zero boundary edges) | **7 of 12** |
| manifold | **2 of 12** |
| collider-ready (closed + manifold + oriented + no bowties) | **1 of 12** |
| open cut edges, total | 22 |

> **Corrected, 2026-08-16 (AG-013).** This table published **4 of 12** collider-ready until the `isomesh`
> pin moved from `4369e3c` to `22c3b35`. The old figure was an overcount, not a regression: that rev's
> `supports_inside_outside` checked boundary edges, non-manifold *edges* and orientation, but **not
> non-manifold vertices** — and a bowtie vertex breaks the pseudonormal construction exactly as an edge
> does. **Ten of the twelve fragments carry a bowtie**, which is the torso/head seam surfacing as a
> vertex fault rather than an edge one. No emitted geometry changed: the fracture is `soup.rs` and owes
> `isomesh` nothing. Every other figure in this table is unmoved.

**A U-prism (closed, manifold, *non-convex*), cut perpendicular to its extrusion.** The cap comes back
larger than the cross-section it is supposed to close, and reports inconsistently oriented edges.

### The diagnosis those three produce — and it is sharper than we assumed

We had been treating "non-manifold artist input" as the single cause. It is two independent causes,
and only one of them is about manifoldness:

1. **Non-convex cross-sections break the capper.** Caps are fan-triangulated from the boundary loop's
   centroid, which is valid only when the loop is star-shaped about that centroid. The U-prism is a
   perfectly clean closed manifold solid and it still fails, because its cut section is a U and the
   centroid lands in the notch — outside the polygon. The fan then lays triangles over empty space.
   **The cuboid passes only because it is convex**, so every cross-section is convex. Our previous
   framing — "the cutter is correct on manifold input" — was wrong; it is correct on *convex* input,
   which is a much smaller claim.
2. **Non-manifold, multi-shell input breaks loop recovery.** Where a torso, a head and a held item
   meet, the merged soup is not a manifold, and a plane through that region produces boundary chains
   that never close. The crate drops those rather than fanning over them, which leaves the fragment
   open. This is the documented, accepted trade — but 22 open edges across 12 shards is the first time
   anyone has known its size.

Both are live in the torso+head fixture simultaneously, which is why it scores so much worse than
either failure alone would predict.

---

## P1 — Capping a cut through non-manifold, multi-shell geometry

**The central problem.** Real characters are several closed shells that interpenetrate — a body, a
head, a weapon, a hat — exported as one glTF. There is no single well-defined "inside".

**Observed:** boundary chains that do not close; the crate drops them, leaving holes.

**Ruled out:** fanning a triangle set over a non-loop (produces self-intersecting surface that shades
wrong from every angle — worse than a hole, which fast motion hides). Also ruled out by the GWN authors
themselves: using generalized winding number to *repair* mesh orientation (Takayama et al. 2014 calls
that application "fundamentally flawed").

**Questions:**
- What do production fracture tools actually do with interpenetrating shells — union them first
  (robust boolean / GWN-classified remesh), cut each shell independently and merge fragments, or
  accept the holes as we do?
- Is there a cutting formulation that is *shell-aware*: cut each closed component separately, then
  associate fragments across components by spatial overlap? What breaks?
- Does the "cost scales with holes, not triangles" property of the exact GWN formulations (Antipodal
  `10.1145/3811323`; Xie, Hafner & Wojtan `10.1145/3811339`) hold for *nearly*-closed multi-shell input,
  or does shell interpenetration inflate the boundary-edge set that cost depends on?

**Unblocks:** whether we repair the cutter or replace the representation (P3). This is the fork in the
road for the whole project.

---

## P2 — Triangulating a boundary loop that may not be a simple polygon

**Observed:** loop recovery chains undirected edges and, at a junction, will take any unused edge —
so it can close a figure-eight and call it a loop. Ear clipping needs a simple polygon. Nested loops
(a cut through a hollow) are currently each filled solid, so an inner rim becomes a disc instead of a
hole — **and that failure is topologically invisible**: the result is a clean closed manifold, just the
wrong solid. Only volume catches it.

**Ruled out:** the existing centroid fan, for the reason in the diagnosis above.

**Questions:**
- For boundary loops recovered from a cut through imperfect geometry, what is the robust standard —
  constrained Delaunay, sweep-line with explicit self-intersection resolution, or something that
  tolerates near-degenerate input better than ear clipping?
- Is there a principled way to resolve a self-touching loop into several simple ones, rather than
  dropping it?
- Nesting: is parity-by-containment the accepted approach, and how is it made robust when loops share
  welded vertices?

**Unblocks:** the immediate repair. This is the highest-value fix we can make without changing
representation.

---

## P3 — Can an SDF/remesh backend keep UVs?

The alternative to repairing the cutter is to stop cutting triangles: sample the mesh into a signed
distance field, intersect with per-fragment cell fields, and re-extract each fragment with dual
contouring. That is watertight and manifold **by construction** on any input, which makes P1 and P2
disappear.

**The cost we know:** the surface is resampled, so texture coordinates have no correspondence to the
original parameterization. For a crate whose entire visual premise is "outer skin keeps the subject's
material, cut faces get a raw interior one", losing the skin's UVs may be fatal.

**Questions — this one decides whether the backend is worth building at all:**
- What is the state of the art in **attribute transfer across remeshing**? Closest-point projection
  from the resampled surface back onto the original triangles, carrying UV barycentrically, is the
  obvious approach — how badly does it fail near the cut, at shell seams, and across UV islands?
- Is there prior work on *hybrid* fracture: keep original triangles wherever the cut did not touch
  them, and use extracted geometry only for the cut faces? That would preserve skin UVs exactly and
  need the field only where we already have no correspondence.
- What grid resolution is required to preserve a character silhouette acceptably, and what does that
  cost per asset at bake time?

**Unblocks:** whether stage 3 happens. Currently blocked upstream regardless (see
`isomesh-upstream-asks.md`, ask 2), so an early "UVs cannot survive this" would save that work.

---

## P4 — What fracture *pattern* is worth computing at bake time?

We currently cut with pseudorandom planes through piece centroids. The references we already cite:
Müller, Chentanez & Kim 2013 (`10.1145/2461912.2461934`); Sellán et al., *Breaking Good*, 2022
(`10.1145/3549540`); Schvartzman & Otaduy 2014; Museth et al. 2021.

**Questions:**
- *Breaking Good*'s fracture modes are precomputed offline through tetrahedralization and a conic
  solve. Is any part of that tractable in a minimal-dependency Rust bake, or is the tetrahedralization
  dependency fatal?
- Between plane-cut, Voronoi, and noisy/bumpy-plane cutting — is there any **perceptual** study
  showing the difference matters at the speeds fragments actually move? Our whole justification for
  plane cutting is a claim that the artifacts are "hidden behind destruction dust or obscured by fast
  explosions". Is that measured anywhere, or is it folklore?
- Is impact-located fracture (biasing cuts toward the impact point) worth it, given fragments are baked
  once per asset and the impact is not known until runtime?

**Unblocks:** whether to invest in fracture *quality* at all, or treat the current pattern as adequate
and spend everything on watertightness.

---

## P5 — Convex decomposition for shard colliders

Each shard currently gets a box collider from its half-extents, which is a poor fit for a plane-cut
sliver. Müller 2013 — which we already cite — is specifically about approximate convex decomposition
for fracture.

**Questions:** what is the current best approximate convex decomposition (V-HACD, CoACD 2022, or
newer), how expensive is it per shard at bake time, and is any of it implementable without pulling a
heavy dependency into a crate whose whole pitch is that it has almost none? Is per-shard decomposition
even necessary, or does a single convex hull per shard suffice for pieces that only tumble briefly?

**Unblocks:** collider quality, which is currently the weakest thing we hand the caller.

---

## P6 — Attribute-aware vertex welding: literature or folklore?

Our fragments ship fully unwelded — three vertices per triangle — because the clipper allocates fresh
vertices per triangle. Welding on position alone destroys hard edges (a cube corner's three normals
collapse to one arbitrary one). The obvious fix is a composite key: position class, plus quantized
normal, plus quantized UV.

**Question:** is there any principled source for the quantization thresholds, or is every engine
picking them by eye? Specifically, what normal tolerance separates "same smooth surface" from "hard
edge" without either welding a crease flat or leaving a visible seam?

**Unblocks:** a 3× vertex-count reduction we want anyway, done safely.

---

## Cross-cutting: determinism

Two runs of the same build must produce bit-identical fragments; this is enforced and now covers every
position, normal and index. We deliberately do **not** claim cross-architecture bit-identity, because
the geometry is `f32` and float addition is not associative.

`isomesh` makes the stronger claim — every transcendental through `libm`, no `std` fast path, committed
golden hashes — specifically so its results do not differ between macOS and Linux.

**Question:** if geometry moves into the SDF path, could the combined pipeline promise cross-platform
reproducibility, or does our own `glam`/`f32` arithmetic upstream of it destroy that regardless? Worth
knowing before we advertise anything.

---

## What a useful answer looks like

Priority order for us: **P2 → P1 → P3 → P6 → P5 → P4.** P2 is the immediate repair, P1 decides whether
repair is even the right strategy, and P3 decides whether a second backend is worth building.

For each, we would rather have "here is the paper, here is what it actually assumes about its input,
and here is why that does or does not match a glTF character" than a survey. Where the literature
assumes a closed manifold — which we suspect is most of it — **say so explicitly**, because that
assumption is exactly what our input violates and it is the reason this brief exists.

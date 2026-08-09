# bevy_autogib

> ⚠️ **Vibe Coded** — written by an AI agent working from a human's direction. It is used in a shipping game and covered by tests, but it has had no line-by-line human audit. Read it before you trust it.

Runtime mesh fracture: take whatever meshes an entity actually loaded, recursively plane-cut them into watertight-capped chunks, bake that once per source asset, and swap the pieces in when the thing dies.

> **This repo is a read-only mirror.** It is split out of [`Ladvien/foundation_vs_slop`](https://github.com/Ladvien/foundation_vs_slop) with `git subtree split`, history intact. Issues and PRs belong upstream.

## The idea: break the asset once, not the frame

The tempting version of this feature computes a fracture at the moment of impact. The shipped version of this feature, in more or less every game that has one, does not — it **pre-fractures the asset and replaces the intact model at runtime**, which is exactly what Müller, Chentanez & Kim document as the practical norm (ACM TOG 2013) before presenting the volumetric decomposition most projects then don't use. Sellán et al.'s *Breaking Good* (ACM TOG 2022) is the most physically honest option available and its fragments are precomputed **offline**, through tetrahedralization and a conic solve in a Python/libigl toolchain — not something a minimal-dependency Rust crate can embed.

So this is the geometric plane-cutter family *Breaking Good* compares against (Schvartzman & Otaduy 2014; Museth et al. 2021 — "bumpy planes slicing through the input"): recursively slice the merged mesh with pseudorandom planes through each piece's centroid, always cutting the **largest** remaining piece, and **cap every cut watertight** — Sutherland–Hodgman triangle clip, welded boundary-loop assembly, fan-triangulated cap with a planar cross-section UV.

Those caps are the reason this is a crate. Slicing a triangle soup is undergraduate geometry; making the cut *close* on real, non-manifold, artist-exported input — welding a boundary loop on a lattice, chaining disjoint loops when a plane passes through two legs, and dropping an unclosed chain instead of emitting a hole — is the part the graphics literature leaves to engine code. And the literature is explicit that this is good enough: plane-cut prefracture artifacts are "hidden behind destruction dust or obscured by fast explosions."

Each fragment comes back as **two** meshes — the subject's original outer skin, and the cut faces alone. Give them different materials. That contrast, outfit against raw interior, is the entire visual read; a fracture rendered in one material just looks like the model fell apart.

```ignore
use bevy_autogib::{AutogibPlugin, AutogibSystems, FractureSubject, DetachedPart, FractureCache};

app.add_plugins(AutogibPlugin)
    .configure_sets(Update, AutogibSystems.run_if(in_state(GameState::Playing)));

// Mark what should break, and what should come off intact.
commands.spawn((FractureSubject(scene.clone()), WorldAssetRoot(scene), /* .. */));
commands.entity(rifle_node).insert(DetachedPart);

// Later, at the moment of death:
let Some(fragments) = cache.fragments(source) else { return };
for frag in fragments {
    // frag.center_local, frag.half_extents, frag.outer_mesh, frag.cap_mesh — the launch is yours.
}
```

You do not need an `App` to use the fracture itself. [`fracture_mesh`] is the whole pipeline with no assets and no ECS — meshes in, meshes out:

```rust
use bevy::math::{Mat4, primitives::Cuboid};
use bevy::mesh::Mesh;

let body = Mesh::from(Cuboid::new(1.0, 2.0, 1.0));
let pieces = bevy_autogib::fracture_mesh(
    &[(&body, Mat4::IDENTITY)],
    12,          // target fragment count
    0.15,        // stop cutting below this extent
    0xC0FFEE,    // seed — same seed, same pieces, every run
    None,        // optional impact direction to bias the first cuts
);

assert!(pieces.len() > 1);
// Every piece knows where it sat and how big it is, so a collider lines up with the render.
assert!(pieces.iter().all(|p| p.outer.is_some() || p.cap.is_some()));
```

## What it deliberately does not do

**It does not move anything.** No rigid bodies, no velocities, no physics dependency. The bake hands you a mesh, a local centre and a half-extent per piece; spawning them, sizing a collider and throwing them is your game's decision and your solver's job. `examples/explode.rs` integrates its own ballistics in thirty lines to make the point.

**It does not know what died.** No health, no factions, no damage types. It knows an entity carries a [`FractureSubject`] and that some subtree is marked [`DetachedPart`]. What makes a thing break is above this layer.

**It does not own your schedule.** The plugin adds one system to `Update` in one public set. Whether that runs while a menu is open is yours to configure.

**It fractures the bind pose.** Geometry is read from `Mesh3d` as authored, so a skinned character breaks from its rest pose, not its death pose. This is a documented limitation rather than a bug being hidden: a death-pose snapshot is the proper upgrade, and per the fracture literature above the gap is not visible when the chunks are flung fast. A rigid, `Transform`-driven bone-child node — a carried weapon — is placed correctly by the same bind-pose transform walk the body uses.

## Determinism

Two runs of the same build, on the same asset, must produce bit-identical fragments. Two things make that true, and both were learned the hard way:

**The seed comes from the asset PATH, never its `AssetId`.** An `AssetId` is a slot index in the asset arena, handed out by async load order — so the same file gets a different id run to run, hashes to a different seed, and the mesh is partitioned along completely different planes. Measured, before the fix: 23 of 23 fragments differed, in half-extents as well as centres.

**The vertex soup is assembled in a canonical order.** `Children` order for a glTF scene is whatever order async instantiation happened to add nodes in. Fragment centroids are float sums over the merged soup, float addition is not associative, and so an unsorted soup moves every centroid by a few ULPs — same fragment count, positions off in the last bits. Sub-meshes are therefore sorted by `(mesh asset path, world-matrix bits)` before a single vertex is appended, and the sort's key is checked at runtime for uniqueness under `debug_assertions` or the `strict-order` feature.

`hash_f32` is a hand-rolled integer hash with its output frozen in a test, for the same reason: there is no RNG dependency here, because a stream that may change between minor versions cannot underwrite any of the above.

Note what this does *not* claim. Fragment geometry is `f32` arithmetic, so cross-architecture bit-identity is not promised — only same-build, same-machine reproducibility, which is what a replay or a regression golden actually needs.

## What it exposes

| Item | Kind | Notes |
|---|---|---|
| `AutogibPlugin` | `Plugin` | Registers the cache, the settings, and the bake |
| `AutogibSystems` | `SystemSet` | On `Update`. Gate it and order against it |
| `FractureSubject(Handle<WorldAsset>)` | `Component` | What to break; the cache key and the seed source |
| `DetachedPart` | `Component` | Subtree pruned out and baked as one intact chunk |
| `FractureSettings` | `Resource` | Five bake dials; `init_resource`d, so yours wins if inserted first |
| `FractureCache` | `Resource` | `fragments()`, `detached_chunk()`, `is_baked()` |
| `Fragment` / `DetachedChunk` | struct | Mesh handles + `center_local` + `half_extents` |
| `fracture_mesh()` / `FragmentGeometry` | fn | The whole pipeline with no assets and no ECS |
| `hash_f32()` | fn | The frozen integer hash the fracture seeds from |

`bake_fractures` is public so it can be named in an ordering constraint, but prefer the set.

## Bevy compatibility

| `bevy_autogib` | `bevy` |
|---|---|
| 0.1 | 0.19 |

## Examples

```sh
cargo run -p bevy_autogib --example fracture_cube   # terminal only — no window, no GPU
cargo run -p bevy_autogib --example explode         # needs a GPU
```

`fracture_cube` drives `fracture_mesh` on a two-part solid and prints the resulting pieces as a table — sizes, triangle counts, and how much of each piece is newly-cut face. It is the fastest way to see what a settings change does.

`explode` is the same fracture on screen: click to break the shape, watch the chunks tumble under ballistics the example integrates itself. This is the only example here that needs a GPU.

## References

- Müller, Chentanez & Kim, "Real-Time Dynamic Fracture with Volumetric Approximate Convex Decompositions", ACM TOG 32(4), 2013. DOI [10.1145/2461912.2461934](https://doi.org/10.1145/2461912.2461934)
- Sellán, Luong, Mattos Da Silva, Ramakrishnan, Yang & Jacobson, "Breaking Good: Fracture Modes for Realtime Destruction", ACM TOG 41(4), 2022. DOI [10.1145/3549540](https://doi.org/10.1145/3549540)
- Schvartzman & Otaduy, "Fracture Animation Based on High-Dimensional Voronoi Diagrams", I3D 2014.
- Museth et al., "OpenVDB: A Deep Dive into Sparse Volumes", SIGGRAPH Courses 2021.
- Sutherland & Hodgman, "Reentrant Polygon Clipping", CACM 17(1), 1974.

## License

MIT OR Apache-2.0, at your option.

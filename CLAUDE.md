# bevy_autogib — notes for agents

Runtime mesh fracture: take whatever meshes an entity actually loaded, recursively plane-cut them into watertight-capped chunks, bake that once per source asset, and swap the pieces in when the thing dies.

## Source of truth

**This repository is the source of truth.** [`Ladvien/bevy_autogib`](https://github.com/Ladvien/bevy_autogib) owns the crate; changes are made here and nowhere else. `foundation_vs_slop` consumes it as a git dependency pinned to a rev, the same way any other consumer would.

**It was the other way round until this branch, and that inversion is a known stale-read hazard.** This repo used to be a read-only `git subtree split` mirror of `foundation_vs_slop/crates/bevy_autogib/`, and a `subtree split` carries only *commits* — so the whole audit harness, the `isomesh` dependency and both research docs, which lived uncommitted in the monorepo working tree, could never arrive by that route. A research agent read the mirror, found no `isomesh` in the manifest, and reported it as fact; the claim was true of what it read and false of the crate. If you find a `crates/bevy_autogib/` in any monorepo checkout, it is a corpse — read this repo instead.

## Build and test

A leaf — `bevy` with defaults off, optional `serde`, and `isomesh` for validation — so it builds and tests on its own, with no `-p` flag and no workspace above it:

```
cargo test              # 16 unit + leaf.rs + doctests
cargo build --release   # NOT redundant: see below
cargo build --examples
```

**`cargo build --release` is the load-bearing one.** `cargo test` compiles dev-dependencies, and the dev-dependency here is the *full* `bevy` umbrella (the windowed example needs winit and the render pipeline). Cargo unifies features, so under `cargo test` this crate silently gets every `bevy` feature there is and a missing entry in its own `[dependencies]` list cannot fail. That is not hypothetical — `WorldAsset` was reached for while only `bevy_scene` was declared, not `bevy_world_serialization`, and every test passed on a crate that did not build.

## The non-negotiable: the bake is reproducible, or it does not happen

Two runs of the same build on the same asset must produce bit-identical fragments. Everything below follows from that, and every line of it was paid for by a bug that took two sessions to find.

**Never seed from an `AssetId`.** An `AssetId` is a slot index in the asset arena, assigned by async load order and subject to slot recycling. The same file gets a different id run to run. When `seed_from_path` hashed the id instead of the path, two same-seed builds produced **23 of 23 fragments differing** — in `half_extents` as well as `center_local`, meaning the mesh was being *partitioned* differently, not merely rounded differently. The asset path is authored rather than allocated; that is the whole reason it is the key.

**Never append to the vertex soup during the `Children` walk.** glTF scene-child order is the order async instantiation happened to add nodes, which is wall-clock dependent. Fragment centroids are float sums over the merged soup, float addition is not associative, and so an unsorted soup moves every centroid by a few ULPs — identical fragment count, positions off in the last bits. Collect first, sort by `(mesh asset path, world-matrix bits)`, then append. The mesh's own `AssetId` was tried as that key and was the *same bug over again*, on a subset of the soup.

**`sort_total_by_key_at` earns its runtime check — do not downgrade it to a comment.** Its one call site is the one place in this crate whose input is an ECS query, and query order is not stable across `App` instances. A comment asserting the key is total cannot fail; the check can, and it is what caught the soup-order bug above. It compiles out in release and is turned back on by the `strict-order` feature, for a harness that builds in release and still wants it.

**`hash_f32` is frozen by a test.** It seeds every cut plane's direction, so a one-constant edit re-partitions every mesh this crate has ever fractured. Treat that test as a lock, not a snapshot to re-bless. There is deliberately no RNG dependency — a stream that may change between minor versions cannot underwrite any of this.

**The bake gate treats an empty detached part as "still streaming", never as "absent".** When the held item is a separate scene from the body it streams on its own schedule: the `DetachedPart` entity exists immediately but has no `Mesh3d` descendants yet, so `all_loaded` stays true and the bake would cache a source with an empty chunk and mark it baked **permanently**. Measured: 11 of 12 runs produced the chunk and 1 did not. If anything downstream is a fixed-size pool or a numbered sequence, losing one chunk shifts every later one, so the race does not stay cosmetic. If a subject with genuinely no detached part is ever supported, this gate must learn to tell "absent" from "not yet" — do not relax it back.

## Rules

- **Bevy 0.19 is pinned.** Read the vendored source (`~/.cargo/registry/src/index.crates.io-*/bevy-0.19.0/`, and its `examples/`), not bevy.org — that documents `main` and has been wrong for this pin more than once.
- Consult Bevy documentation often. It can be found at codex_fs/offline_reference_docs/bevy-0.19-book/
- **A missing `Res<T>` panics its system in 0.19**; it does not skip. Both resources this reads are `init_resource`d by the plugin, which is what keeps that true — a caller supplying `FractureSettings` inserts it *before* adding the plugin, and `init_resource` then no-ops.
- **All run conditions are evaluated — there is no short-circuit.** A bare `Res<T>` in a `.run_if(..)` closure panics whenever that resource is absent, even behind an earlier condition that returned false.
- **The caller owns the schedule.** The plugin adds one system to `Update` in `AutogibSystems` and configures no run condition. Anything inserting `DetachedPart` on a streamed-in subtree must run `.before(AutogibSystems)`.
- **No `unwrap()`**, no `expect` on caller data, no panicking index. Malformed input is `warn!`-skipped: a mesh with no `Float32x3` positions, a non-`TriangleList` topology, an unclosed cut boundary, an out-of-range index. A handle with no asset path is `error!`-refused rather than baked unreproducibly.
- **One path per feature.** No fallbacks, no legacy shims, no stub placeholders. `seed_from_path` refusing to bake an unpathed handle *is* the rule: falling back to the `AssetId` for one sub-mesh would reintroduce the instability intermittently, which is worse than not baking.
- **This crate never learns what died.** No health, no factions, no damage, no physics. `tests/leaf.rs` enforces the dependency half of that; the naming half is on you — nothing here should say "gun", "figurine", or "unit".

## Where the boundary falls

Four things belong to the caller, not here, and each has bitten someone who assumed otherwise:

- **The convex decomposition.** This crate cuts a proxy — `ProxyCell` per connected shell — and carries the render triangles along as a payload. Computing that decomposition is not its job: a consumer already running V-HACD or CoACD for colliders has one, and forcing a second, different decomposition would be the fracture disagreeing with the physics about what the object is. A subject with no `FractureProxy` is `error!`-refused rather than given a synthesised bounding box.
- **Naming the part that detaches.** Finding a weapon node by name is content, not fracture. Tag it `DetachedPart` from your own system, `.before(AutogibSystems)`.
- **Deciding when the bake may run.** The plugin sets no run condition. Gate `AutogibSystems` on your own state.
- **Everything after the fragment exists.** Rigid bodies, colliders, launch impulses, pooling, despawn. This crate hands out a mesh, a local centre and a half-extent, and stops.

If a change here would need to know what died, or which solver you use, it belongs on your side of that line.

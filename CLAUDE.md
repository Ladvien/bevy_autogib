# bevy_autogib — notes for agents

Runtime mesh fracture: take whatever meshes an entity actually loaded, recursively plane-cut them into watertight-capped chunks, bake that once per source asset, and swap the pieces in when the thing dies.

## Source of truth

The source of truth for this crate is [`Ladvien/foundation_vs_slop`](https://github.com/Ladvien/foundation_vs_slop) at `crates/bevy_autogib/`. If you are reading this in a standalone `Ladvien/bevy_autogib` checkout, that is a read-only `git subtree split` mirror — **changes made here cannot be pulled back**. Make them upstream.

## Build and test

A leaf — `bevy` with defaults off plus optional `serde` — so it builds and tests on its own: `cargo test -p bevy_autogib`.

## The non-negotiable: the bake is reproducible, or it does not happen

Two runs of the same build on the same asset must produce bit-identical fragments. Everything below follows from that, and every line of it was paid for by a bug that took two sessions to find.

**Never seed from an `AssetId`.** An `AssetId` is a slot index in the asset arena, assigned by async load order and subject to slot recycling. The same file gets a different id run to run. When `seed_from_path` hashed the id instead of the path, two same-seed builds produced **23 of 23 fragments differing** — in `half_extents` as well as `center_local`, meaning the mesh was being *partitioned* differently, not merely rounded differently. The asset path is authored rather than allocated; that is the whole reason it is the key.

**Never append to the vertex soup during the `Children` walk.** glTF scene-child order is the order async instantiation happened to add nodes, which is wall-clock dependent. Fragment centroids are float sums over the merged soup, float addition is not associative, and so an unsorted soup moves every centroid by a few ULPs — identical fragment count, positions off in the last bits. Collect first, sort by `(mesh asset path, world-matrix bits)`, then append. The mesh's own `AssetId` was tried as that key and was the *same bug over again*, on a subset of the soup.

**`sort_total_by_key_at` is a deliberate second copy** of the parent repo's `util::sort_total_by_key_at`. This crate is a leaf and cannot import the game's. The one call site is the one place here whose input is an ECS query, which is exactly where a runtime tie check earns its keep — so do not downgrade it to a `SORT-OK:` comment the way the other extracted crates legitimately do. Its check compiles out in release and is turned back on by the `strict-order` feature, which the parent game's `test-harness` feature forwards to.

**`hash_f32` is frozen by a test here and by `tests/rng_guard.rs` upstream.** Two copies, one value; if they ever disagree, one was edited and every fracture moved. There is deliberately no RNG dependency — a stream that may change between minor versions cannot underwrite any of this.

**The bake gate treats an empty detached part as "still streaming", never as "absent".** When the held item is a separate scene from the body it streams on its own schedule: the `DetachedPart` entity exists immediately but has no `Mesh3d` descendants yet, so `all_loaded` stays true and the bake would cache a source with an empty chunk and mark it baked **permanently**. Measured in the parent game: 11 of 12 runs flung the weapon and 1 did not, which shifted a fixed-size chunk ring by one and diverged everything foraging on it. If a subject with genuinely no detached part is ever supported, this gate must learn to tell "absent" from "not yet" — do not relax it back.

## Rules

- **Bevy 0.19 is pinned.** Read the vendored source (`~/.cargo/registry/src/index.crates.io-*/bevy-0.19.0/`, and its `examples/`), not bevy.org — that documents `main` and has been wrong for this pin more than once.
- **A missing `Res<T>` panics its system in 0.19**; it does not skip. Both resources this reads are `init_resource`d by the plugin, which is what keeps that true — a caller supplying `FractureSettings` inserts it *before* adding the plugin, and `init_resource` then no-ops.
- **All run conditions are evaluated — there is no short-circuit.** A bare `Res<T>` in a `.run_if(..)` closure panics whenever that resource is absent, even behind an earlier condition that returned false.
- **The caller owns the schedule.** The plugin adds one system to `Update` in `AutogibSystems` and configures no run condition. Anything inserting `DetachedPart` on a streamed-in subtree must run `.before(AutogibSystems)`.
- **No `unwrap()`**, no `expect` on caller data, no panicking index. Malformed input is `warn!`-skipped: a mesh with no `Float32x3` positions, a non-`TriangleList` topology, an unclosed cut boundary, an out-of-range index. A handle with no asset path is `error!`-refused rather than baked unreproducibly.
- **One path per feature.** No fallbacks, no legacy shims, no stub placeholders. `seed_from_path` refusing to bake an unpathed handle *is* the rule: falling back to the `AssetId` for one sub-mesh would reintroduce the instability intermittently, which is worse than not baking.
- **This crate never learns what died.** No health, no factions, no damage, no physics. `tests/leaf.rs` enforces the dependency half of that; the naming half is on you — nothing here should say "gun", "figurine", or "unit".

## In the monorepo

The game wraps this as `src/autogib.rs`, which keeps only what is that game's content: the system that finds VALKYRIE's rifle node by name and tags it `DetachedPart`, and the `RunState` gate on `AutogibSystems`. `src/squad.rs` re-exports `FractureSubject`/`DetachedPart` under the game's own names, and `src/gore.rs` owns the spawning — Avian bodies, colliders, the chunk ring, and the launch impulse. Root `CLAUDE.md` and `TESTING.md` carry the project-wide rules; neither is part of this mirror.

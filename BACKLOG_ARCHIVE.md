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

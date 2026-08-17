#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

mod audit;
mod bake;
mod bond;
mod mesh;
mod proxy;
mod severance;
mod soup;
mod tree;

pub use audit::{SolidAudit, SurfaceReport, audit_proxies, audit_proxy, audit_render};
pub use bond::{Bond, BondGraph, BondId, BondSet};
pub use bake::{
    DetachedChunk, DetachedPart, Fragment, FractureCache, FractureProxy, FractureSubject,
    bake_fractures,
};
pub use mesh::{Fracture, FragmentGeometry, fracture_mesh};
pub use proxy::ProxyCell;
pub use severance::{Reach, capsule, directional, radial, spread, swept_triangle};
pub use soup::hash_f32;
pub use tree::{FragmentId, FragmentTree, TreeNode};

use bevy::prelude::*;

/// **The geometry dials for one bake**, without the ECS sizing policy that chooses `target`.
///
/// [`FractureSettings`] is the resource a game authors once; this is what a single cut actually
/// needs, and it is what [`fracture_mesh`] takes. Build one from a `FractureSettings` with
/// [`FractureSettings::cut_for`], or write it out directly when there is no `App` in sight.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CutSettings {
    /// Finest fragment count to cut down to.
    pub target: usize,
    /// Stop cutting a piece once its extent drops below this fraction of the whole subject's.
    pub min_fraction: f32,
    /// Cuts from a proxy cell to the finest fragment — the hierarchy's memory bound.
    pub max_depth: u16,
    /// **How far a cut plane may slide off the piece's centre**, as a fraction of how far the piece
    /// reaches along that plane's own normal.
    ///
    /// `0.0` puts every plane exactly through the centroid, which splits every piece near enough in
    /// half and is why an unjittered bake reads as uniform shards rather than debris. Real fragment
    /// volumes follow Mott's distribution — many small, few large — and an off-centre cut is what
    /// produces that spread, compounding with depth.
    ///
    /// Measured against the piece's own span along the normal rather than its bounding box, so the
    /// plane cannot slide out of a cell that is thin in that direction and lose the cut. Values at
    /// or above `1.0` are refused by [`FractureSettings::validate`] for that reason.
    pub plane_jitter: f32,
    /// **How much the "cut the biggest piece next" rule may be nudged**, so the sequence does not
    /// march down a strict volume order and level every piece toward the same size.
    ///
    /// `0.0` is the strict largest-first rule. Higher values let a slightly smaller piece be chosen,
    /// widening the size distribution. The nudge is a stable hash of the piece's own node id, so it
    /// is reproducible and does not shift as the frontier grows.
    pub size_spread: f32,
    /// **How hard to break a piece across its narrowest dimension instead of at a random angle.**
    ///
    /// A uniformly-sampled cut normal slices diagonally through whatever it is given, which on a
    /// limb produces long oblique wedges — and on a body reads as a statue shattering rather than
    /// something coming apart. Sellán et al.'s finding is that geometric prefracture is blind to
    /// where a shape is *weak*, and a shape is weak across its thin cross-sections.
    ///
    /// This buys most of that cheaply: sample several candidate normals and keep the one the piece
    /// is *longest* along, because the cut face is perpendicular to the normal, so the longest axis
    /// gives the smallest cross-section. `0.0` samples once and is exactly the old behaviour; `1.0`
    /// samples eight and takes the best.
    pub weak_axis: f32,
    /// **How much the drawn cut face is crumpled**, as a fraction of its own radius.
    ///
    /// A flat cut face is the visual language of cleaved stone and ice, and no amount of fragment
    /// shaping changes that — it is what a plane through a solid leaves behind. This displaces the
    /// *interior* of the emitted cap, never its boundary, so the seam against the skin stays shut.
    ///
    /// **It touches Tier B only.** The proxy cell stays exactly planar, so the collider is still one
    /// convex hull and every watertightness guarantee is untouched — [`audit_proxy`] measures the
    /// cell, not this. `0.0` leaves the cap flat.
    pub cap_relief: f32,
    /// **How much to round the drawn fragment**, so it reads as a lump rather than a shard.
    ///
    /// Sharp dihedral edges are the visual signature of brittle fracture — ice, glass, cleaved stone.
    /// Shaping the pieces differently does not change that, because the edges are simply what a plane
    /// through a solid leaves behind. This subdivides the drawn surface and relaxes it, bevelling
    /// every edge and rounding every corner, and re-derives smooth normals so each facet stops being
    /// lit as a separate plane.
    ///
    /// **Tier B only, like [`cap_relief`](Self::cap_relief).** The proxy cell is untouched, so the
    /// collider is still one exact convex hull and every watertightness guarantee holds. The drawn
    /// mesh ends up slightly *inside* its hull, which is the harmless direction.
    ///
    /// `0.0` leaves the fragment exactly as cut. Around `0.5` reads as flesh; `1.0` is a pebble.
    pub soften: f32,
    /// Drives every plane direction and every jitter draw — the only source of variation.
    pub seed: u32,
}

impl CutSettings {
    /// The three dials a caller always has an opinion about, with [`FractureSettings`]'s shipped
    /// defaults for the rest. Assign to the remaining fields to change them.
    pub fn new(target: usize, min_fraction: f32, seed: u32) -> Self {
        let d = FractureSettings::default();
        CutSettings {
            target,
            min_fraction,
            max_depth: d.max_depth,
            plane_jitter: d.plane_jitter,
            size_spread: d.size_spread,
            weak_axis: d.weak_axis,
            cap_relief: d.cap_relief,
            soften: d.soften,
            seed,
        }
    }
}

/// How hard to break things. Eleven dials, all bake-time — nothing here decides how a chunk *moves*
/// after it exists, because that is the caller's physics, not this crate's business.
///
/// The piece count is driven by the mesh's own bounding size rather than authored per asset:
/// `pieces_base` is the count at `ref_extent`, scaled by how much bigger or smaller this mesh actually
/// is, then clamped. So a rat and an ogre both break sensibly from one setting.
///
/// **These set the *finest* granularity, not the only one.** A bake keeps the whole hierarchy it cut
/// through, so `max_pieces` is the ceiling a caller can ask for, not the count it must take —
/// [`FragmentTree::frontier_of`] reads the same bake back at three pieces or at all of them.
#[derive(Resource, Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct FractureSettings {
    /// Fragment count at [`ref_extent`](Self::ref_extent); scaled by the mesh's actual bounding size.
    pub pieces_base: i32,
    /// Reference subject half-extent that `pieces_base` is tuned for.
    pub ref_extent: f32,
    /// Clamp on the fragment count (lower).
    pub min_pieces: i32,
    /// Clamp on the fragment count (upper — this one bounds mesh and entity growth, so it is the
    /// dial that keeps a huge mesh from becoming a thousand rigid bodies).
    pub max_pieces: i32,
    /// Stop cutting a piece once its extent drops below this fraction of the whole mesh's extent.
    pub min_fraction: f32,
    /// **Cuts from a proxy cell to the finest fragment — the hierarchy's memory bound.**
    ///
    /// A bake keeps every piece it ever held, not only the final ones, because that forest is what
    /// lets one bake answer both "break this into three" and "break this into forty". The cost is
    /// payload: each level of the forest holds the whole subject over again, so total geometry is
    /// roughly this number times the subject's own triangle count.
    ///
    /// The default is deliberately slack enough never to bind at the default `max_pieces` — it is a
    /// guard against a pathologically unbalanced subject, not a tuning dial. Lower it when memory
    /// matters more than the finest granularity.
    pub max_depth: u16,
    /// How far a cut plane may slide off centre — see [`CutSettings::plane_jitter`].
    pub plane_jitter: f32,
    /// How much the largest-first cut order may be nudged — see [`CutSettings::size_spread`].
    pub size_spread: f32,
    /// How hard to cut across the narrow dimension — see [`CutSettings::weak_axis`].
    pub weak_axis: f32,
    /// How much the drawn cut face is crumpled — see [`CutSettings::cap_relief`].
    pub cap_relief: f32,
    /// How much the drawn fragment is rounded — see [`CutSettings::soften`].
    pub soften: f32,
}

impl Default for FractureSettings {
    fn default() -> Self {
        FractureSettings {
            pieces_base: 14,
            ref_extent: 0.5,
            min_pieces: 6,
            max_pieces: 40,
            min_fraction: 0.18,
            max_depth: 12,
            plane_jitter: 0.35,
            size_spread: 0.5,
            weak_axis: 0.75,
            cap_relief: 0.30,
            soften: 0.5,
        }
    }
}

impl FractureSettings {
    /// Reject a settings block that cannot produce a sane clamp.
    ///
    /// **This is a real crash, not a hypothetical.** [`bake_fractures`] feeds these straight into
    /// `i32::clamp(min, max)`, which panics when `min > max` — so an inverted pair authored in a data
    /// file takes the process down at the first death. Call this at load; failing loudly at the door is
    /// the one path, and silently swapping the pair would be a fallback.
    pub fn validate(&self) -> Result<(), String> {
        if self.min_pieces > self.max_pieces {
            return Err(format!(
                "bevy_autogib: min_pieces ({}) > max_pieces ({}) — `i32::clamp` panics on an inverted \
                 range, so fix the authored values",
                self.min_pieces, self.max_pieces
            ));
        }
        // Zero depth permits no cut at all, so the bake would return the caller's proxy cells
        // unchanged and report success. That is precisely the silent-degraded-result this crate
        // refuses to produce: a subject that never breaks is a settings bug, not a fracture.
        if self.max_depth == 0 {
            return Err(
                "bevy_autogib: max_depth is 0 — no cut is permitted, so the bake would return the \
                 proxy cells unfractured and call it done. Use 1 or more."
                    .to_string(),
            );
        }
        // At 1.0 the jitter can put the plane exactly on the far extreme of the piece, where it
        // divides nothing and the cut is lost — quietly, as one fewer fragment. Refuse at the door
        // rather than emit a bake that is short of what was asked for.
        if !(0.0..1.0).contains(&self.plane_jitter) {
            return Err(format!(
                "bevy_autogib: plane_jitter is {} — it must be in [0, 1). At 1.0 a plane can land on \
                 the edge of the piece it is meant to divide, silently costing a fragment.",
                self.plane_jitter
            ));
        }
        for (name, v) in
            [("weak_axis", self.weak_axis), ("cap_relief", self.cap_relief), ("soften", self.soften)]
        {
            if !(0.0..=1.0).contains(&v) {
                return Err(format!("bevy_autogib: {name} is {v} — it must be in [0, 1]."));
            }
        }
        if self.size_spread < 0.0 {
            return Err(format!(
                "bevy_autogib: size_spread is {} — negative would invert the cut order into \
                 smallest-first, which is not a spread. Use 0.0 or more.",
                self.size_spread
            ));
        }
        Ok(())
    }

    /// The geometry dials for one bake, with the piece count and seed the caller resolved.
    ///
    /// `target` comes from this resource's sizing policy applied to a particular mesh, and `seed`
    /// from that asset's path — neither is a property of the settings alone, which is why they are
    /// arguments rather than fields.
    pub fn cut_for(&self, target: usize, seed: u32) -> CutSettings {
        CutSettings {
            target,
            min_fraction: self.min_fraction,
            max_depth: self.max_depth,
            plane_jitter: self.plane_jitter,
            size_spread: self.size_spread,
            weak_axis: self.weak_axis,
            cap_relief: self.cap_relief,
            soften: self.soften,
            seed,
        }
    }
}

/// The set [`bake_fractures`] runs in. **Gate and order against this, not against the system.**
///
/// The plugin puts the bake on `Update` and nothing else: it is an asset bake gated on streaming, so
/// it has no business on a fixed timestep. What it must *not* do is decide when your game is running —
/// so the run condition is yours:
///
/// ```ignore
/// app.add_plugins(bevy_autogib::AutogibPlugin)
///     .configure_sets(Update, AutogibSystems.run_if(in_state(MyState::Playing)))
///     .add_systems(Update, tag_my_weapon.before(AutogibSystems));
/// ```
///
/// Anything that inserts [`DetachedPart`] onto a streamed-in subtree must run `.before` this set, or
/// the bake reads the scene a frame before the marker lands and swallows the part into the body.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct AutogibSystems;

/// Registers the fracture cache, its settings, and the one-shot bake.
///
/// [`FractureSettings`] is `init_resource`d, which does nothing when the resource is already present —
/// so a caller that loads the dials from a config file inserts them *before* adding this plugin and its
/// values win. There is no merge and no partial default: one owner, one value.
pub struct AutogibPlugin;

impl Plugin for AutogibPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FractureCache>()
            .init_resource::<FractureSettings>()
            .add_systems(Update, bake_fractures.in_set(AutogibSystems));
    }
}

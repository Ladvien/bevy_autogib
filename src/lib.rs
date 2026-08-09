#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

mod bake;
mod mesh;
mod soup;

pub use bake::{DetachedChunk, DetachedPart, Fragment, FractureCache, FractureSubject, bake_fractures};
pub use mesh::{FragmentGeometry, fracture_mesh};
pub use soup::hash_f32;

use bevy::prelude::*;

/// How hard to break things. Five dials, all bake-time — nothing here decides how a chunk *moves*
/// after it exists, because that is the caller's physics, not this crate's business.
///
/// The piece count is driven by the mesh's own bounding size rather than authored per asset:
/// `pieces_base` is the count at `ref_extent`, scaled by how much bigger or smaller this mesh actually
/// is, then clamped. So a rat and an ogre both break sensibly from one setting.
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
}

impl Default for FractureSettings {
    fn default() -> Self {
        FractureSettings {
            pieces_base: 14,
            ref_extent: 0.5,
            min_pieces: 6,
            max_pieces: 40,
            min_fraction: 0.18,
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
        Ok(())
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

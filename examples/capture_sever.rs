//! **`sever`, on rails, rendered headless — the GIF the README shows.**
//!
//! `sever` is interactive, which makes it the wrong thing to record: what you would capture depends
//! on what you happened to press. This runs the same subject through a fixed script on a fixed
//! timestep, so frame 62 of one run is frame 62 of the next and two GIFs differ only where the
//! geometry does.
//!
//! **The scene and the rules are `common::body`, not copies of them.** A recorder that
//! reimplements its subject drifts from it silently, and the drift is invisible in exactly the place
//! you would look for it. What lives here is only the script: where each blow lands, which kind it
//! is, and on which frame.
//!
//! The script is chosen to show the one thing `explode` cannot — that the subject *stays standing*
//! between blows, and that what comes off depends on where you hit it:
//!
//! | frame | what |
//! |---|---|
//! | 0 | intact, all 34 pieces standing |
//! | 18 | a projectile to the left shoulder — the arm goes, the body stays up |
//! | 48 | one to the head |
//! | 80 | a slash at the right shoulder |
//! | 112 | a swept blade through the waist, taking the legs |
//! | 144 | a blast in the chest, which finishes what is left |
//!
//! Frames land in `--out <dir>` (default `frames-sever/`). Turn them into a GIF with `tools/gif.sh`.
//!
//! Run: `cargo run --release --example capture_sever -- --out frames-sever`

use bevy::prelude::*;

mod common;
use common::body::{self, Blow, Chunk};
use common::{Recorder, arg, light_and_floor};

/// Capture size, matching `capture.rs` so the two GIFs sit together on a page.
const WIDTH: u32 = 720;
const HEIGHT: u32 = 540;

/// **Fixed timestep — the reason this exists alongside `sever`.** `sever` integrates against `Time`,
/// so its trajectories depend on how fast the machine rendered. Here dt is a constant, which makes
/// the whole animation a pure function of the seed.
const DT: f32 = 1.0 / 30.0 * 0.55;

/// The finest frontier: index into [`body::GRANULARITIES`].
const GRANULARITY: usize = 3;

/// How much to round the drawn fragments. See [`body::SOFTENINGS`] and the `T` key in `sever`.
const SOFTEN: f32 = 0.5;

/// Where each blow lands and what kind it is. **The whole script.**
const SCRIPT: [(u32, Blow, Vec3); 5] = [
    (18, Blow::Projectile, Vec3::new(-0.30, 0.16, 0.0)),
    (48, Blow::Projectile, Vec3::new(0.00, 0.48, 0.0)),
    (80, Blow::Slash, Vec3::new(0.30, 0.16, 0.0)),
    (112, Blow::SweptBlade, Vec3::new(0.00, -0.30, 0.0)),
    (144, Blow::Blast, Vec3::new(0.00, 0.00, 0.0)),
];

/// Frames to keep rolling after the last blow so the debris settles.
const TAIL: u32 = 34;

fn main() {
    let out = arg("--out").unwrap_or_else(|| "frames-sever".to_string());
    let camera = Transform::from_xyz(2.25, 1.35, 2.95).looking_at(Vec3::new(0.0, 0.76, 0.0), Vec3::Y);
    let Some(mut rec) = Recorder::new(WIDTH, HEIGHT, camera, &out) else { return };

    light_and_floor(rec.world());
    let baked = body::Baked::bake(rec.world(), SOFTEN);
    let materials = body::BodyMaterials::new(rec.world());
    let damage = body::Damage::fresh(&baked, GRANULARITY);
    rec.world().insert_resource(baked);
    rec.world().insert_resource(materials);
    rec.world().insert_resource(damage);
    body::stand(rec.world(), GRANULARITY);
    rec.warm_up(4);

    // The integrator is added after the scene rather than at startup, so the intact frames before
    // the first blow are perfectly still.
    rec.app().main.add_systems(Update, integrate);

    let last = SCRIPT.last().map(|(f, _, _)| *f).unwrap_or(0);
    for frame in 0..last + TAIL {
        for (at_frame, blow, at) in SCRIPT {
            if frame == at_frame {
                body::strike(rec.world(), blow, at);
            }
        }
        rec.shoot();
    }
    let n = rec.finish();
    info!("capture_sever: wrote {n} frames to {out}");
}

/// Gravity, a ground bounce and tumbling — on a fixed `DT`, so the run is reproducible.
fn integrate(mut chunks: Query<(&mut Chunk, &mut Transform)>) {
    for (mut chunk, mut transform) in &mut chunks {
        body::integrate(&mut chunk, &mut transform, DT);
    }
}

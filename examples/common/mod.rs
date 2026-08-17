//! **The headless recorder both capture examples drive.**
//!
//! Rendering a demo to PNGs needs the same eight awkward things every time: `DefaultPlugins` with no
//! window and no winit, shaders compiled synchronously so frame 0 is not empty, a render target that
//! is not a surface, a camera pointed at it, a hand-pumped update loop, a device poll so a screenshot
//! does not read a half-drawn frame, and a few extra pumps at the end because screenshot readback
//! lands a frame or two late. None of that is interesting, all of it is easy to get subtly wrong, and
//! there are two recorders — so it lives here once rather than twice.
//!
//! What each recorder still owns is its *scene* and its *script*: what stands there, what happens to
//! it, and on which frame. That is the part worth reading.
//!
//! Not an example itself — Cargo only auto-discovers `examples/*.rs` and `examples/*/main.rs`, so a
//! bare `mod.rs` in a subdirectory is compiled only by the examples that `mod common;` it.

// Each recorder uses a subset of this, so the other's share reads as dead to the compiler. The
// alternative is splitting the harness by consumer, which would put the awkward parts back in two
// places — exactly what this module exists to prevent.
#![allow(dead_code)]

pub mod body;

use bevy::{
    app::SubApps,
    asset::RenderAssetUsages,
    camera::RenderTarget,
    prelude::*,
    render::{
        RenderPlugin,
        render_resource::{Extent3d, PollType, TextureDimension, TextureFormat, TextureUsages},
        renderer::RenderDevice,
        view::screenshot::{Screenshot, save_to_disk},
    },
    window::ExitCondition,
    winit::WinitPlugin,
};

/// A headless Bevy app that writes one PNG per frame.
pub struct Recorder {
    app: SubApps,
    target: Handle<Image>,
    out: String,
    frame: u32,
}

impl Recorder {
    /// Build the app, its off-screen target and a camera looking through `camera`.
    ///
    /// `None` — with an `error!` naming the path — if the output directory cannot be created. A
    /// recorder that cannot write is refused at the door rather than discovering it 100 frames in.
    pub fn new(width: u32, height: u32, camera: Transform, out: &str) -> Option<Recorder> {
        if let Err(e) = std::fs::create_dir_all(out) {
            error!("capture: cannot create {out}: {e}");
            return None;
        }

        let mut app = App::new();
        app.add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: None,
                    exit_condition: ExitCondition::DontExit,
                    ..default()
                })
                // Every shader must be ready before the first frame, or frame 0 renders empty.
                .set(RenderPlugin { synchronous_pipeline_compilation: true, ..default() })
                .disable::<WinitPlugin>(),
        );
        // `run()` is never called, so the two things the runner would have done must be done here.
        app.finish();
        app.cleanup();
        let mut app = std::mem::take(app.sub_apps_mut());

        let mut image = Image::new_uninit(
            Extent3d { width, height, depth_or_array_layers: 1 },
            TextureDimension::D2,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::RENDER_WORLD,
        );
        image.texture_descriptor.usage |= TextureUsages::RENDER_ATTACHMENT;
        let target = app.main.world_mut().resource_mut::<Assets<Image>>().add(image);

        app.main.world_mut().spawn((Camera3d::default(), RenderTarget::from(target.clone()), camera));

        Some(Recorder { app, target, out: out.to_string(), frame: 0 })
    }

    /// The main world, for building a scene or changing it between frames.
    pub fn world(&mut self) -> &mut World {
        self.app.main.world_mut()
    }

    /// The sub-apps, for the one thing `world()` cannot do: adding a system after `cleanup`.
    pub fn app(&mut self) -> &mut SubApps {
        &mut self.app
    }

    /// Screenshot the world as it stands, then advance it one frame.
    pub fn shoot(&mut self) {
        let path = format!("{}/frame{:04}.png", self.out, self.frame);
        self.app.main.world_mut().spawn(Screenshot::image(self.target.clone())).observe(save_to_disk(path));
        self.frame += 1;
        self.step();
    }

    /// Draw and discard `frames` frames, so the first *recorded* one is not empty.
    ///
    /// **Needed, and it shows up in the worst place.** The first frame or two after a scene is built
    /// come back blank — the pipeline has not drawn yet — which is invisible mid-clip and is exactly
    /// what a viewer sees as the still preview of a GIF. Call it after the scene exists and before
    /// the recording loop; anything the scene animates has not started yet, so nothing moves.
    pub fn warm_up(&mut self, frames: u32) {
        for _ in 0..frames {
            self.step();
        }
    }

    /// Advance one frame without recording it.
    pub fn step(&mut self) {
        self.app.update();
        // Without the wait, a screenshot can read back a half-drawn target.
        let device = self.app.main.world().resource::<RenderDevice>().wgpu_device().clone();
        if let Err(e) = device.poll(PollType::Wait { submission_index: None, timeout: None }) {
            warn!("capture: device poll failed, frame may be torn: {e:?}");
        }
    }

    /// Pump past the last screenshot so its readback lands, and report how many frames were written.
    pub fn finish(mut self) -> u32 {
        for _ in 0..4 {
            self.step();
        }
        self.frame
    }
}

/// `--<flag> <value>`, hand-parsed.
///
/// Deliberately not a CLI crate: one flag does not justify an entry in this repo's dependency graph,
/// not even a dev-dependency. A flag given with no value is `warn!`ed and treated as absent, never
/// silently substituted — the same rule the crate applies to a mesh with no positions.
pub fn arg(flag: &str) -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == flag {
            match args.next() {
                Some(v) => return Some(v),
                None => {
                    warn!("capture: {flag} given with no value; ignoring it");
                    return None;
                }
            }
        }
    }
    None
}

/// The furniture every recorded scene shares: a key light, a fill, and a floor to land on.
pub fn light_and_floor(world: &mut World) {
    // **The fill is not a nicety.** With a single directional light, every surface turned away from
    // it renders at zero — and a cut face at zero, against a dark background, does not read as a
    // shadowed face. It reads as a *hole*, and the fragment looks like an open shell you can see
    // through. That was reported as missing geometry and was in fact missing light.
    world.insert_resource(GlobalAmbientLight {
        color: Color::srgb(0.62, 0.66, 0.78),
        brightness: 900.0,
        ..default()
    });
    world.spawn((
        DirectionalLight { illuminance: 9_000.0, shadow_maps_enabled: true, ..default() },
        Transform::from_xyz(4.0, 8.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    let floor = world
        .resource_mut::<Assets<Mesh>>()
        .add(Mesh::from(Plane3d::default().mesh().size(14.0, 14.0)));
    let dark = world.resource_mut::<Assets<StandardMaterial>>().add(StandardMaterial {
        base_color: Color::srgb(0.16, 0.16, 0.18),
        perceptual_roughness: 0.95,
        ..default()
    });
    world.spawn((Mesh3d(floor), MeshMaterial3d(dark)));
}

/// Add a material to the world in one line, since a recorder builds several by hand.
pub fn material(world: &mut World, color: Color, roughness: f32) -> Handle<StandardMaterial> {
    world.resource_mut::<Assets<StandardMaterial>>().add(StandardMaterial {
        base_color: color,
        perceptual_roughness: roughness,
        ..default()
    })
}

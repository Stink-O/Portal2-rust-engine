//! Portal and portal-gun effects for Bevy: Portal 2's particle systems
//! (`.pcf`) and portal sounds, read from the player's own Portal 2 install.
//! Nothing from the game is shipped and there are no stand-ins: an effect or
//! sound the game does not provide is logged once and skipped.
//!
//! # Use
//!
//! ```ignore
//! App::new()
//!     .add_plugins((DefaultPlugins, p2_core::CorePlugin, p2_fx::Plugin))
//!     .insert_resource(p2_fx::FxSettings { sounds: false, ..default() }) // optional knobs
//!     .run();
//! ```
//!
//! The plugin only reads the messages in `p2_core` (`PortalPlaced`,
//! `PortalClosed`, `PortalShotFailed`, `GunFired`) and the [`p2_core::Portal`]
//! and [`p2_core::PortalGun`] components. It spawns no cameras and adds no
//! Bevy plugins; it expects `DefaultPlugins` and `p2_core::CorePlugin` to be
//! added by the host, and [`p2_assets::AssetsPlugin`] for the game's files
//! (without it nothing is drawn or played).
//!
//! # Config
//!
//! [`FxSettings`] is the whole config: toggles for particles and sounds, the
//! master volume, the first-person flashes, and the projectile timing. Insert
//! it before the plugin to change defaults; the plugin inserts the defaults if
//! it is missing.
//!
//! See `crates/p2_fx/README.md` for the spec sections, the effects, and the gaps.
use bevy::prelude::*;

mod dmx;
mod effects;
mod pcf;
mod render;
mod sim;
mod sounds;
mod step_sounds;
mod transit_sound;

pub use effects::edge_system;
pub use pcf::{ParticleLibrary, SystemDef};
pub use render::{ParticleEffect, ParticleStats, Particles, SheetGrids, SpriteCardMaterial};
pub use sounds::{gain, PlaySound, PortalLoop, PortalSounds};
pub use step_sounds::StepSurface;
pub use transit_sound::{box_touches_rectangle, transit_sound_params};

/// Render layer of the main-view-only effects, the portal rims. The main camera
/// must include this layer; portal view cameras must not (their default layer
/// 0 is enough). A rim seen through a portal would otherwise be drawn twice,
/// and the engine culls it there at recursion depth 1 (`render_rope`'s
/// `cull system starting at this recursion depth`).
pub const LAYER_PORTAL_MAIN_ONLY: usize = p2_core::LAYER_MAIN_VIEW_ONLY;

/// Every knob of the effects. Defaults are Portal 2's values where the spec
/// gives them. Insert your own before adding [`Plugin`] to change them.
#[derive(Resource, Clone, Debug)]
pub struct FxSettings {
    /// Spawn particle effects (rims, bursts, projectile, flashes). Default `true`.
    pub particles: bool,
    /// Play sounds. Default `true`.
    pub sounds: bool,
    /// Multiplies every sound volume, 0 to 1. Default `1.0`.
    pub master_volume: f32,
    /// First-person muzzle flash on each shot (needs `ViewmodelAttachments::muzzle`). Default `true`.
    pub muzzle_flash: bool,
    /// First-person top light on the gun (needs `ViewmodelAttachments::top_light`). Default `true`.
    pub top_light: bool,
    /// Seconds from the shot until the projectile stream appears
    /// (`cl_portal_projectile_delay_sp`). Default `0.1`.
    pub projectile_delay: f32,
    /// Seconds the visual projectile takes from the muzzle to the hit point.
    /// Portal 2 (single player) always takes 0.1 s, whatever the distance:
    /// the client ends the shot at the later of start + 0.1 s and the server's
    /// arrival time, and the server places the portal in the same tick
    /// (research/specs/portal-rendering.md §8). Default `0.1`.
    pub projectile_flight: f32,
}

impl Default for FxSettings {
    fn default() -> Self {
        Self {
            particles: true,
            sounds: true,
            master_volume: 1.0,
            muzzle_flash: true,
            top_light: true,
            projectile_delay: 0.1,
            projectile_flight: 0.1,
        }
    }
}

/// The effects plugin: rims and bursts, the projectile, flashes and portal
/// sounds. The one public entry point of this crate.
pub struct Plugin;

impl bevy::app::Plugin for Plugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FxSettings>();
        render::plugin(app);
        sounds::plugin(app);
        transit_sound::plugin(app);
        step_sounds::plugin(app);
        effects::plugin(app);
    }
}

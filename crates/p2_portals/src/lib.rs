//! Portal 2's portals, portal gun and first-person controller for a Bevy +
//! avian3d game: one plugin group.
//!
//! ```ignore
//! use bevy::prelude::*;
//! use avian3d::prelude::*;
//!
//! App::new()
//!     .add_plugins((DefaultPlugins, PhysicsPlugins::default()))
//!     .add_plugins(p2_portals::PortalPlugins)
//!     .add_systems(Startup, |mut commands: Commands| {
//!         // Your world: any collider tagged `Portalable` takes portals.
//!         // ...
//!         p2_portals::core::spawn_player(&mut commands, Vec3::ZERO, 0.0);
//!     })
//!     .run();
//! ```
//!
//! Every piece can be dropped or replaced: disable it on the group
//! (`PortalPlugins.build().disable::<p2_portals::movement::InputPlugin>()`)
//! and drive the same components/messages yourself (`MoveInput`,
//! `ViewAngles`, `FirePortal`, see [`core`]). To use your own character
//! controller, disable [`movement::Plugin`] and follow `p2_transit`'s README.
//!
//! Portal 2 is required: its models, textures, particles and sounds are read
//! at runtime from the player's own install (nothing from the game is
//! shipped). Without it [`assets::AssetsPlugin`] logs
//! [`assets::PORTAL2_REQUIRED`] and exits the app with an error; a binary can
//! check first with [`assets::find_portal2`].
use bevy::app::{PluginGroup, PluginGroupBuilder};

pub use p2_assets as assets;
pub use p2_body as body;
pub use p2_core as core;
pub use p2_fx as fx;
pub use p2_gun as gun;
pub use p2_movement as movement;
pub use p2_render as render;
pub use p2_transit as transit;
pub use p2_view as view;

/// Everything: contract, assets, controller + input, camera + viewmodel,
/// gun + mouse input, portal rendering, effects, transit, body + ghosts.
///
/// Doesn't add `DefaultPlugins` or avian's `PhysicsPlugins`; add those
/// yourself. Portal 2's 60 Hz tick is opt-in: insert
/// `p2_core::P2Settings { set_fixed_timestep: true }` before this group.
pub struct PortalPlugins;

impl PluginGroup for PortalPlugins {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::start::<Self>()
            .add(p2_core::CorePlugin)
            .add(p2_assets::AssetsPlugin)
            .add(p2_movement::Plugin)
            .add(p2_movement::InputPlugin)
            .add(p2_view::Plugin)
            .add(p2_gun::Plugin)
            .add(p2_gun::MouseInputPlugin)
            .add(p2_render::Plugin::default())
            .add(p2_fx::Plugin)
            .add(p2_transit::Plugin)
            .add(p2_body::Plugin)
    }
}

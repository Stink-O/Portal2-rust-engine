//! Portal 2 first-person player controller.
//!
//! Add [`Plugin`] for the movement core. Add [`InputPlugin`] as well if you
//! want keyboard and mouse control. The core runs from the components
//! `p2_core::spawn_player` gives a player: [`p2_core::MoveInput`] and
//! [`p2_core::ViewAngles`] are the only inputs, so your own input can drive it.
//! Tune every number through [`MovementConfig`] (Portal 2's values by default).
//!
//! * The move runs in `FixedUpdate` in `P2Systems::Move`, and every step uses
//!   the fixed delta, so it works at any tick rate. Portal 2's own numbers
//!   assume 60 Hz (`p2_core::TICK_HZ`).
//! * Collisions are avian3d shape casts against the player's hull. Entities in
//!   `p2_core::IgnoreColliders` are skipped.
//! * Air funnelling into linked floor and ceiling portals is in [`funnel`].
//!   It is switched off by [`MovementConfig::funnel_into_portals`].
//!
//! Specs: `research/specs/portal-movement.md`, `view-feel.md` §5, `portal-funnel.md`.
use avian3d::prelude::Collider;
use bevy::prelude::*;
use p2_core::{
    CharacterVelocity, Crouch, Grounded, IgnoreColliders, InterpolatedFeet, MoveInput, P2Systems, Player,
    ViewAngles, HULL_DUCK, HULL_STAND,
};

mod config;
mod controller;
pub mod funnel;
mod input;
pub mod rules;

pub use config::MovementConfig;
pub use input::InputPlugin;

/// The movement core. Adds [`MovementConfig`] (defaults) if the host has not
/// inserted one. Requires `p2_core::CorePlugin` and avian3d's physics plugins.
pub struct Plugin;

impl bevy::app::Plugin for Plugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MovementConfig>()
            .add_systems(Update, attach_components.before(P2Systems::Input))
            .add_systems(FixedUpdate, controller::move_players.in_set(P2Systems::Move));
    }
}

/// Adds the private crouch and jump state, and any contract component a
/// player was spawned without, so a bare `Player` entity still moves.
fn attach_components(mut commands: Commands, added: Query<(Entity, &Transform), Added<Player>>) {
    for (e, tf) in &added {
        let feet = tf.translation;
        commands.entity(e).insert_if_new((
            controller::Duck::default(),
            Crouch::default(),
            Grounded::default(),
            CharacterVelocity::default(),
            InterpolatedFeet { prev: feet, curr: feet },
            MoveInput::default(),
            IgnoreColliders::default(),
            ViewAngles::default(),
        ));
    }
}

/// The player's hull in its current crouch state, for other crates' casts.
/// Its centre sits at `feet + crouch.hull_centre_offset()`.
pub fn hull_collider(crouch: &Crouch) -> Collider {
    let h = if crouch.ducked { HULL_DUCK } else { HULL_STAND };
    Collider::cuboid(h.x, h.y, h.z)
}

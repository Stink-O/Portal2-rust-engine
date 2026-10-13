//! The public tuning surface of the controller.

use bevy::prelude::*;

/// Every tunable of the Portal 2 player controller. Insert your own before
/// adding [`crate::Plugin`] to change them; the defaults are Portal 2's
/// values (research/specs/portal-movement.md, view-feel.md §5).
///
/// Distances and speeds are Source units (u, u/s); convert with
/// `p2_core::u` where you need metres.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct MovementConfig {
    /// Gravity, u/s^2 (`sv_gravity`).
    pub gravity: f32,
    /// Ground acceleration factor (`sv_accelerate`).
    pub accelerate: f32,
    /// Air acceleration factor (`sv_paintairacceleration`).
    pub air_accelerate: f32,
    /// Ground friction (`sv_friction`).
    pub friction: f32,
    /// Speed below which friction uses this as the floor (`sv_stopspeed`), u/s.
    pub stop_speed: f32,
    /// Friction multiplier with no floor ahead (`sv_edgefriction`).
    pub edge_friction: f32,
    /// Walking speed at full input (`sv_speed_normal`), u/s.
    pub max_speed: f32,
    /// Air wish-speed cap for the add-speed test, u/s.
    pub air_speed_cap: f32,
    /// Horizontal speed above which the air lock engages, u/s.
    pub air_lock_speed: f32,
    /// Per-axis speed above which an opposing wish is zeroed (half of `air_lock_speed`), u/s.
    pub air_lock_axis: f32,
    /// Jump speed is `sqrt(jump_factor * gravity)`; 90 gives a 45 u apex.
    pub jump_factor: f32,
    /// Highest step the player walks up, u (`sv_stepsize`).
    pub step_height: f32,
    /// Smallest normal.y that counts as walkable floor.
    pub min_walk_normal: f32,
    /// Per-axis velocity clamp (`sv_maxvelocity`), u/s.
    pub max_velocity: f32,
    /// Seconds for the eye to lower into a crouch on the ground.
    pub duck_time: f32,
    /// Seconds for the eye to rise from a crouch.
    pub unduck_time: f32,
    /// Source `m_yaw`: degrees of view turn per mouse count.
    pub mouse_m_yaw_deg: f32,
    /// Source `sensitivity`, the multiplier on `mouse_m_yaw_deg`.
    pub mouse_sensitivity: f32,
    /// Pitch clamp, degrees either way.
    pub pitch_limit_deg: f32,
    /// Steer airborne players into floor and ceiling portals (`sv_player_funnel_into_portals`).
    pub funnel_into_portals: bool,
}

impl Default for MovementConfig {
    fn default() -> Self {
        Self {
            gravity: 600.0,
            accelerate: 10.0,
            air_accelerate: 5.0,
            friction: 4.0,
            stop_speed: 100.0,
            edge_friction: 2.0,
            max_speed: 175.0,
            air_speed_cap: 60.0,
            air_lock_speed: 300.0,
            air_lock_axis: 150.0,
            jump_factor: 90.0,
            step_height: 18.0,
            min_walk_normal: 0.7,
            max_velocity: 3500.0,
            duck_time: 0.4,
            unduck_time: 0.2,
            mouse_m_yaw_deg: 0.022,
            mouse_sensitivity: 3.0,
            pitch_limit_deg: 89.0,
            funnel_into_portals: true,
        }
    }
}

impl MovementConfig {
    /// Initial jump speed in u/s.
    pub fn jump_speed(&self) -> f32 {
        (self.jump_factor * self.gravity).sqrt()
    }

    /// Jump apex from the ground in u (45 with the defaults).
    pub fn jump_apex(&self) -> f32 {
        let v = self.jump_speed();
        v * v / (2.0 * self.gravity)
    }

    /// Radians of view turn per mouse count.
    pub fn radians_per_count(&self) -> f32 {
        self.mouse_m_yaw_deg * self.mouse_sensitivity * std::f32::consts::PI / 180.0
    }
}

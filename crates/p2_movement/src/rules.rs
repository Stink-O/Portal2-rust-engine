//! Portal 2's movement rules as pure functions. Every number comes from a
//! [`MovementConfig`] argument, so a host can tune them; the defaults are
//! Portal 2's. Units are Source units (u, u/s, s) unless noted.
use std::f32::consts::PI;

use bevy::prelude::*;
use p2_core::{U, VIEW_DUCK, VIEW_STAND};

use crate::config::MovementConfig;

/// Air forward move is scaled by cos(pitch) from |sin(pitch)| >= 0.5 (30 deg) (portal-movement §5).
pub const AIR_PITCH_SCALE_SIN: f32 = 0.5;
/// Edge probe: start this far ahead of the feet along the velocity, u.
pub const EDGE_PROBE_AHEAD: f32 = 16.0;
/// Edge probe: trace this far down, u.
pub const EDGE_PROBE_DEPTH: f32 = 49.0;
/// Ducked ground speed multiplier (HandleDuckingSpeedCrop, portal-movement §2).
pub const DUCK_SPEED_SCALE: f32 = 1.0 / 3.0;
/// How far below the hull the ground check looks, u.
pub const GROUND_CHECK: f32 = 2.0;
/// Above this upward speed (u/s) the player is never grounded (NON_JUMP_VELOCITY).
pub const NON_JUMP_VELOCITY: f32 = 140.0;
/// Feet rise (air duck) or fall (air unduck) by 36 u: the hull shrinks around the head.
pub const FEET_RISE: f32 = (72.0 - 36.0) * U;
/// Gap kept between the hull and the world, metres.
pub const SKIN: f32 = 0.004;

/// Forward air scale: 1 below 30 deg of pitch, cos(pitch) from there.
pub fn air_forward_scale(pitch: f32) -> f32 {
    if pitch.sin().abs() < AIR_PITCH_SCALE_SIN {
        1.0
    } else {
        pitch.cos().abs()
    }
}

/// Yaw basis (forward, right) on the ground plane, Y-up world.
/// Yaw 0 looks down -Z; positive yaw turns left.
pub fn yaw_basis(yaw: f32) -> (Vec3, Vec3) {
    let (s, c) = yaw.sin_cos();
    (Vec3::new(-s, 0.0, -c), Vec3::new(c, 0.0, -s))
}

/// Horizontal wish velocity (u/s, world) from move input (`x` right, `y`
/// forward, each -1..1). Full input is `max_speed`; the diagonal is clamped
/// to it. `forward_scale` is 1 on the ground, [`air_forward_scale`] in the air.
pub fn wish_velocity(input: Vec2, yaw: f32, forward_scale: f32, max_speed: f32) -> Vec3 {
    let fmove = input.y * max_speed;
    let smove = input.x * max_speed;
    let (f, r) = yaw_basis(yaw);
    let mut w = f * (fmove * forward_scale) + r * smove;
    let len = w.length();
    if len > max_speed {
        w *= max_speed / len;
    }
    w
}

/// Ground friction on the 3D velocity (Source `Friction`). `factor` is the
/// friction coefficient with any edge multiplier applied. Speeds under
/// 0.1 u/s are zeroed so the player fully stops.
pub fn friction(v: Vec3, factor: f32, stop_speed: f32, dt: f32) -> Vec3 {
    let speed = v.length();
    if speed < 0.1 {
        return Vec3::ZERO;
    }
    let control = speed.max(stop_speed);
    let new_speed = (speed - dt * control * factor).max(0.0);
    v * (new_speed / speed)
}

/// Friction coefficient, with the edge multiplier when there is no floor ahead.
pub fn friction_factor(cfg: &MovementConfig, edge: bool) -> f32 {
    if edge {
        cfg.friction * cfg.edge_friction
    } else {
        cfg.friction
    }
}

/// Ground acceleration toward `wish_speed` along `dir` (Source `Accelerate`).
pub fn accelerate(v: Vec3, dir: Vec3, wish_speed: f32, accel: f32, dt: f32) -> Vec3 {
    let add = wish_speed - v.dot(dir);
    if add <= 0.0 {
        return v;
    }
    v + dir * (accel * dt * wish_speed).min(add)
}

/// Air acceleration: the add-speed test uses `min(wish, cap)`, the step uses
/// the full wish speed (Source `AirAccelerate`). `dir` should be horizontal.
pub fn air_accelerate(v: Vec3, dir: Vec3, wish_speed: f32, accel: f32, cap: f32, dt: f32) -> Vec3 {
    let capped = wish_speed.min(cap);
    let add = capped - v.dot(dir);
    if add <= 0.0 {
        return v;
    }
    v + dir * (accel * wish_speed * dt).min(add)
}

/// Air lock: above `lock_speed` horizontal, on each horizontal axis whose
/// speed exceeds `axis_speed`, a wish component opposing that speed is zeroed.
/// Only X and Z of `wish` and `v` are used.
pub fn air_lock(wish: Vec3, v: Vec3, lock_speed: f32, axis_speed: f32) -> Vec3 {
    if Vec2::new(v.x, v.z).length() <= lock_speed {
        return wish;
    }
    let mut w = wish;
    if v.x.abs() > axis_speed && w.x * v.x < 0.0 {
        w.x = 0.0;
    }
    if v.z.abs() > axis_speed && w.z * v.z < 0.0 {
        w.z = 0.0;
    }
    w
}

/// Remove the part of a horizontal wish that pushes into a steep surface normal `n`.
pub fn no_push_into(wish: Vec3, n: Vec3) -> Vec3 {
    let nh = Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let into = wish.dot(nh);
    if into < 0.0 {
        wish - nh * into
    } else {
        wish
    }
}

/// Per-axis velocity clamp.
pub fn clamp_velocity(v: Vec3, max: f32) -> Vec3 {
    v.clamp(Vec3::splat(-max), Vec3::splat(max))
}

/// Source's smoothstep on [0, 1].
pub fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Apply mouse counts to yaw and pitch. Moving the mouse right turns right
/// (yaw falls); moving it up looks up (pitch rises). Pitch is clamped.
pub fn look(cfg: &MovementConfig, yaw: f32, pitch: f32, counts: Vec2) -> (f32, f32) {
    let k = cfg.radians_per_count();
    let limit = cfg.pitch_limit_deg.to_radians();
    let yaw = (yaw - counts.x * k).rem_euclid(2.0 * PI);
    let pitch = (pitch - counts.y * k).clamp(-limit, limit);
    (yaw, pitch)
}

/// Eye height above the feet, eased with smoothstep over a duck (down) or an
/// unduck (up) transition. [`EyeEase::shift`] keeps the eye still in the world
/// when the feet move (an air duck with the head planted).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EyeEase {
    /// Eye height when the current transition started, m.
    pub from: f32,
    /// Eye height the transition is heading to, m.
    pub to: f32,
    /// Seconds since the transition started.
    pub elapsed: f32,
    /// Length of the current transition, s.
    pub dur: f32,
}

impl EyeEase {
    /// A standing eye at rest.
    pub fn standing() -> Self {
        Self { from: VIEW_STAND, to: VIEW_STAND, elapsed: 0.0, dur: 0.0 }
    }

    /// Current eye height above the feet, m.
    pub fn value(&self) -> f32 {
        let s = if self.dur <= 0.0 { 1.0 } else { (self.elapsed / self.dur).clamp(0.0, 1.0) };
        self.from + (self.to - self.from) * smoothstep(s)
    }

    /// Head toward `to` from wherever the eye is now. Going down takes
    /// `down_time`, going up `up_time`. Does nothing if the target is unchanged.
    pub fn retarget(&mut self, to: f32, down_time: f32, up_time: f32) {
        if (to - self.to).abs() < 1e-6 {
            return;
        }
        let now = self.value();
        self.dur = if to < now { down_time } else { up_time };
        self.from = now;
        self.to = to;
        self.elapsed = 0.0;
    }

    /// The feet moved by `delta` m: keep the eye where it is in the world.
    pub fn shift(&mut self, delta: f32) {
        self.from -= delta;
    }

    /// Advance the transition by `dt` seconds.
    pub fn advance(&mut self, dt: f32) {
        self.elapsed += dt;
    }

    /// The transition has finished.
    pub fn done(&self) -> bool {
        self.elapsed >= self.dur
    }
}

impl Default for EyeEase {
    fn default() -> Self {
        Self::standing()
    }
}

/// Eye height standing, m, re-exported for hosts that drive the eye themselves.
pub const STANDING_EYE: f32 = VIEW_STAND;
/// Eye height crouched, m.
pub const CROUCHED_EYE: f32 = VIEW_DUCK;

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;
    const C: fn() -> MovementConfig = MovementConfig::default;

    fn close(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn jump_apex_is_45_units() {
        assert!(close(C().jump_speed(), 232.379, 1e-2));
        assert!(close(C().jump_apex(), 45.0, 1e-3));
    }

    #[test]
    fn ground_accel_reaches_175_and_no_further() {
        let c = C();
        let mut v = Vec3::ZERO;
        for _ in 0..120 {
            v = friction(v, friction_factor(&c, false), c.stop_speed, DT);
            v = accelerate(v, Vec3::NEG_Z, c.max_speed, c.accelerate, DT);
        }
        assert!(close(v.length(), 175.0, 1e-3), "{v}");
    }

    #[test]
    fn ground_accel_per_tick_is_accel_wish_dt() {
        let v = accelerate(Vec3::ZERO, Vec3::X, 175.0, 10.0, DT);
        assert!(close(v.x, 10.0 * 175.0 * DT, 1e-4));
    }

    #[test]
    fn friction_stop_speed_and_edge() {
        let c = C();
        let v = friction(Vec3::new(50.0, 0.0, 0.0), c.friction, c.stop_speed, DT);
        assert!(close(v.x, 50.0 - 100.0 * 4.0 * DT, 1e-4));
        let v = friction(Vec3::new(300.0, 0.0, 0.0), c.friction, c.stop_speed, DT);
        assert!(close(v.x, 300.0 - 300.0 * 4.0 * DT, 1e-3));
        let e = friction(Vec3::new(300.0, 0.0, 0.0), friction_factor(&c, true), c.stop_speed, DT);
        assert!(close(300.0 - e.x, 2.0 * (300.0 - v.x), 1e-3));
        assert_eq!(friction_factor(&c, true), 8.0);
        assert_eq!(friction(Vec3::new(1.0, 0.0, 0.0), c.friction, c.stop_speed, DT), Vec3::ZERO);
    }

    #[test]
    fn stop_distance_from_full_speed_is_about_31_units() {
        // Analytic (continuous): 175 -> 100 at rate 4 is 18.75 u; 100 -> 0 at 400 u/s^2 is 12.5 u.
        // The 60 Hz discrete friction step lands about 2 u short of that, so allow 2.5 u.
        let c = C();
        let mut v = Vec3::new(175.0, 0.0, 0.0);
        let mut x = 0.0;
        for _ in 0..600 {
            v = friction(v, c.friction, c.stop_speed, DT);
            x += v.x * DT;
            if v.length() == 0.0 {
                break;
            }
        }
        assert!(close(x, 31.25, 2.5), "stopped after {x} u");
    }

    #[test]
    fn air_accel_is_capped_at_60_but_can_strafe() {
        let c = C();
        let mut v = Vec3::ZERO;
        for _ in 0..600 {
            v = air_accelerate(v, Vec3::X, c.max_speed, c.air_accelerate, c.air_speed_cap, DT);
        }
        assert!(close(v.x, 60.0, 1e-3), "{v}");
        let v = air_accelerate(Vec3::ZERO, Vec3::X, 175.0, 5.0, 60.0, DT);
        assert!(close(v.x, 5.0 * 175.0 * DT, 1e-4));
        let v0 = Vec3::new(0.0, 0.0, -250.0);
        let v1 = air_accelerate(v0, Vec3::X, 175.0, 5.0, 60.0, DT);
        assert!(v1.x > 0.0 && v1.length() > v0.length());
    }

    #[test]
    fn air_lock_stops_braking_a_fling() {
        let c = C();
        let (l, a) = (c.air_lock_speed, c.air_lock_axis);
        let v = Vec3::new(1000.0, 0.0, 0.0);
        assert_eq!(air_lock(Vec3::NEG_X, v, l, a), Vec3::ZERO);
        assert_eq!(air_lock(Vec3::Z, v, l, a), Vec3::Z);
        assert_eq!(air_lock(Vec3::new(-0.7, 0.0, 0.7), v, l, a), Vec3::new(0.0, 0.0, 0.7));
        assert_eq!(air_lock(Vec3::NEG_X, Vec3::new(200.0, 0.0, 0.0), l, a), Vec3::NEG_X);
        assert_eq!(air_lock(Vec3::NEG_X, Vec3::new(100.0, 0.0, 400.0), l, a), Vec3::NEG_X);
        let mut v = v;
        for _ in 0..120 {
            let w = air_lock(Vec3::NEG_X * c.max_speed, v, l, a);
            if w.length() > 0.0 {
                v = air_accelerate(v, w.normalize(), w.length(), c.air_accelerate, c.air_speed_cap, DT);
            }
        }
        assert_eq!(v.x, 1000.0);
    }

    #[test]
    fn air_forward_scale_by_pitch() {
        assert_eq!(air_forward_scale(0.0), 1.0);
        assert_eq!(air_forward_scale(29.0f32.to_radians()), 1.0);
        assert!(close(air_forward_scale(60.0f32.to_radians()), 0.5, 1e-4));
        assert!(close(air_forward_scale(-60.0f32.to_radians()), 0.5, 1e-4));
    }

    #[test]
    fn wish_is_full_speed_on_axis_and_clamped_on_diagonal() {
        let w = wish_velocity(Vec2::new(0.0, 1.0), 0.0, 1.0, 175.0);
        assert!(close(w.length(), 175.0, 1e-3));
        assert!(close(w.z, -175.0, 1e-3), "yaw 0 forward is -Z: {w}");
        assert!(close(wish_velocity(Vec2::ONE, 0.0, 1.0, 175.0).length(), 175.0, 1e-3));
    }

    #[test]
    fn duck_timings_and_smoothstep() {
        let c = C();
        let mut e = EyeEase::standing();
        assert_eq!(e.value(), VIEW_STAND);
        e.retarget(VIEW_DUCK, c.duck_time, c.unduck_time);
        assert_eq!(e.dur, c.duck_time);
        for _ in 0..12 {
            e.advance(DT);
        }
        assert!(close(e.value(), (VIEW_STAND + VIEW_DUCK) * 0.5, 1e-4), "{}", e.value());
        for _ in 0..12 {
            e.advance(DT);
        }
        assert!(e.done());
        assert!(close(e.value(), VIEW_DUCK, 1e-6));
        e.retarget(VIEW_STAND, c.duck_time, c.unduck_time);
        assert_eq!(e.dur, c.unduck_time);
        assert!(close(e.value(), VIEW_DUCK, 1e-6));
    }

    #[test]
    fn duck_retarget_mid_transition_starts_from_current_eye() {
        let c = C();
        let mut e = EyeEase::standing();
        e.retarget(VIEW_DUCK, c.duck_time, c.unduck_time);
        e.advance(0.2);
        let mid = e.value();
        e.retarget(VIEW_STAND, c.duck_time, c.unduck_time);
        assert!(close(e.value(), mid, 1e-6));
        assert_eq!(e.dur, c.unduck_time);
    }

    #[test]
    fn feet_shift_keeps_the_eye_in_the_world() {
        let c = C();
        let mut e = EyeEase::standing();
        let before = e.value();
        e.retarget(VIEW_DUCK, c.duck_time, c.unduck_time);
        e.shift(FEET_RISE);
        assert!(close(e.value() + FEET_RISE, before, 1e-6));
    }

    #[test]
    fn feet_rise_is_36_units() {
        assert!(close(FEET_RISE, 36.0 * U, 1e-6));
    }

    #[test]
    fn mouse_look_clamps_pitch_and_turns_right() {
        let c = C();
        let k = c.radians_per_count();
        let (yaw, _) = look(&c, 0.0, 0.0, Vec2::new(100.0, 0.0));
        assert!(close(yaw, (2.0 * PI) - 100.0 * k, 1e-5), "{yaw}");
        let (_, pitch) = look(&c, 0.0, 0.0, Vec2::new(0.0, -1.0e6));
        assert!(close(pitch, 89.0f32.to_radians(), 1e-6));
        let (_, pitch) = look(&c, 0.0, 0.0, Vec2::new(0.0, 1.0e6));
        assert!(close(pitch, -89.0f32.to_radians(), 1e-6));
    }

    #[test]
    fn velocity_clamp_is_per_axis() {
        let v = clamp_velocity(Vec3::new(5000.0, -4000.0, 10.0), 3500.0);
        assert_eq!(v, Vec3::new(3500.0, -3500.0, 10.0));
    }

    #[test]
    fn no_push_removes_only_the_inward_part() {
        let w = no_push_into(Vec3::new(1.0, 0.0, 1.0), Vec3::new(-1.0, 0.0, 0.0).normalize());
        assert!(close(w.x, 0.0, 1e-5) && close(w.z, 1.0, 1e-5), "{w}");
    }
}

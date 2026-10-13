//! Pure transit maths: velocity, view and position through a linked pair
//! (research/specs/portal-transit.md §1.2-1.7, §4). No ECS, so the numbers are
//! unit-tested against the spec.
use bevy::prelude::*;
use p2_core::math::{
    arc_speed, box_extent_along, clamp_exit_velocity, exit_min_speed, level_view, LevelledView, PortalFrame,
    PortalPair, FLOOR_PORTAL_COS,
};
use p2_core::U;

/// Tunables of the transit. Defaults are Portal 2's values (portal-transit.md).
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct TransitConfig {
    /// Gravity in m/s^2 used for the gravity fix-up and the exit arc.
    /// Default `sv_gravity` 600 u/s^2 (§1.3).
    pub gravity: f32,
    /// Multiplier on the re-applied gravity (§1.3, `* 1.008`).
    pub gravity_readd: f32,
    /// Fraction of the opening the hole covers (§4, 0.98).
    pub hole_scale: f32,
    /// Helper ring width and region radius, metres (`portal_environment_radius` 75 u, §4).
    pub env_radius: f32,
    /// How far the hole and its helper tunnel go behind the wall, metres (500 u, §4).
    pub hole_depth: f32,
    /// Allowance in front of the plane for the hole, metres (0.5 u, §4).
    pub hole_front: f32,
    /// Look-ahead, seconds, for the region test so a fast fall meets the ignored wall in time.
    pub region_lookahead: f32,
    /// Copy the exit's nearby geometry behind the entrance (§4 "remote geometry").
    pub mirror_exit_geometry: bool,
    /// Step size, metres, of the free-spot search after a teleport.
    pub unstick_step: f32,
    /// Number of steps of the free-spot search after a teleport.
    pub unstick_steps: u32,
}

impl Default for TransitConfig {
    fn default() -> Self {
        Self {
            gravity: 600.0 * U,
            gravity_readd: 1.008,
            hole_scale: 0.98,
            env_radius: 75.0 * U,
            hole_depth: 500.0 * U,
            hole_front: 0.5 * U,
            region_lookahead: 0.08,
            mirror_exit_geometry: true,
            unstick_step: 0.05,
            unstick_steps: 10,
        }
    }
}

/// Fraction of the tick spent past the plane (§1.3).
pub fn gravity_fraction(d0: f32, d1: f32) -> f32 {
    if d1 - d0 == 0.0 {
        0.5
    } else {
        1.0 - d1 / (d1 - d0)
    }
}

/// §1.3: un-apply the gravity gained past the plane, rotate through the pair,
/// re-apply that gravity in world space. Returns the velocity before the exit clamp.
pub fn rotate_velocity(v: Vec3, pair: &PortalPair, dt: f32, frac: f32, cfg: &TransitConfig) -> Vec3 {
    let gcorr = Vec3::NEG_Y * (dt * cfg.gravity * frac);
    pair.map_dir(v - gcorr) + gcorr * cfg.gravity_readd
}

/// §1.4: the exit's minimum (floor, or tilted arc) and the 1000 u/s cap.
pub fn exit_velocity(
    v_rot: Vec3,
    exit: &PortalFrame,
    exit_half: Vec2,
    entry_normal: Vec3,
    centre: Vec3,
    cfg: &TransitConfig,
) -> Vec3 {
    let arc = arc_speed(exit, exit_half, centre, cfg.gravity);
    let min = exit_min_speed(exit.normal(), entry_normal, true, arc);
    clamp_exit_velocity(v_rot, exit.normal(), min)
}

/// §1.6: forced crouch when the pair turns "up" by more than 30 deg, or a
/// tilted (not floor) exit fires the player up and out fast.
pub fn forced_duck(pair: &PortalPair, exit: &PortalFrame, v_out: Vec3) -> bool {
    if (pair.rotation * Vec3::Y).y.abs() < FLOOR_PORTAL_COS {
        return true;
    }
    let ny = exit.normal().y;
    ny > 0.1 && ny < 0.9 && v_out.y > U && v_out.dot(exit.normal()) > 200.0 * U
}

/// §1.5: keep the crouched hull's centre inside the exit opening
/// (offset along right within +-(hw - 5 u), along up within +-(hh - 5 u)).
pub fn nudge_into_opening(exit: &PortalFrame, half: Vec2, centre: Vec3) -> Vec3 {
    let l = exit.to_local(centre);
    let lx = (half.x - 5.0 * U).max(0.0);
    let ly = (half.y - 5.0 * U).max(0.0);
    exit.to_world(Vec3::new(l.x.clamp(-lx, lx), l.y.clamp(-ly, ly), l.z))
}

/// Result of one crossing (everything in world space, metres).
#[derive(Clone, Copy, Debug)]
pub struct Crossing {
    /// The pair that was applied (entry -> exit).
    pub pair: PortalPair,
    /// New hull centre (after the forced-duck nudge).
    pub centre: Vec3,
    /// Previous-tick hull centre, mapped through the pair (for render interpolation).
    pub prev_centre: Vec3,
    /// New velocity, m/s.
    pub velocity: Vec3,
    /// Exact view through the pair, roll split out for the view to ease out.
    pub view: LevelledView,
    /// The character must crouch (§1.6).
    pub force_duck: bool,
}

/// One teleport. `c0` / `c1`: hull centre at the start / end of the tick,
/// `v`: velocity at the end of the tick, `view`: full view rotation (roll included),
/// `dt`: the fixed tick length in seconds.
#[allow(clippy::too_many_arguments)]
pub fn cross(
    entry: &PortalFrame,
    exit: &PortalFrame,
    exit_half: Vec2,
    c0: Vec3,
    c1: Vec3,
    v: Vec3,
    view: Quat,
    dt: f32,
    cfg: &TransitConfig,
) -> Crossing {
    let pair = PortalPair::new(*entry, *exit);
    let d0 = entry.plane_distance(c0);
    let d1 = entry.plane_distance(c1);
    let rotated = rotate_velocity(v, &pair, dt, gravity_fraction(d0, d1), cfg);
    let centre_raw = pair.map_point(c1);
    let velocity = exit_velocity(rotated, exit, exit_half, entry.normal(), centre_raw, cfg);
    let force_duck = forced_duck(&pair, exit, velocity);
    let centre = if force_duck { nudge_into_opening(exit, exit_half, centre_raw) } else { centre_raw };
    Crossing {
        pair,
        centre,
        prev_centre: pair.map_point(c0),
        velocity,
        view: level_view(pair.map_rotation(view)),
        force_duck,
    }
}

/// Is a hull (centre `centre`, half extents `he`, world axes) inside a portal's
/// hole volume? Conservative: projects the hull onto the portal's axes (§4).
pub fn in_hole(frame: &PortalFrame, half: Vec2, centre: Vec3, he: Vec3, cfg: &TransitConfig) -> bool {
    let l = frame.to_local(centre);
    let ex = box_extent_along(he, frame.right());
    let ey = box_extent_along(he, frame.up());
    let ez = box_extent_along(he, frame.normal());
    let hw = half.x * cfg.hole_scale;
    let hh = half.y * cfg.hole_scale;
    l.x.abs() < hw + ex && l.y.abs() < hh + ey && l.z - ez < cfg.hole_front && l.z + ez > -cfg.hole_depth
}

/// The helper ring around a hole (§4): four slabs in the wall plane, from the
/// hole edge out to `env_radius`, extruded `hole_depth` behind the plane.
/// Returns (local centre, full size) for each piece.
pub fn ring_pieces(half: Vec2, cfg: &TransitConfig) -> [(Vec3, Vec3); 4] {
    let hw = half.x * cfg.hole_scale;
    let hh = half.y * cfg.hole_scale;
    let w = cfg.env_radius;
    let d = cfg.hole_depth;
    let z = -d / 2.0;
    [
        (Vec3::new(hw + w / 2.0, 0.0, z), Vec3::new(w, 2.0 * (hh + w), d)),
        (Vec3::new(-hw - w / 2.0, 0.0, z), Vec3::new(w, 2.0 * (hh + w), d)),
        (Vec3::new(0.0, hh + w / 2.0, z), Vec3::new(2.0 * hw, w, d)),
        (Vec3::new(0.0, -hh - w / 2.0, z), Vec3::new(2.0 * hw, w, d)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use p2_core::math::PORTAL_HALF_SIZE;
    use p2_core::ViewAngles;
    use std::f32::consts::FRAC_PI_2;

    const DT: f32 = 1.0 / 60.0;

    fn u(v: f32) -> f32 {
        v * U
    }
    fn close(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() <= tol
    }
    fn cfg() -> TransitConfig {
        TransitConfig::default()
    }
    /// Wall portal: normal `n`, up = world Y.
    fn wall(pos: Vec3, n: Vec3) -> PortalFrame {
        PortalFrame::from_normal_up(pos, n, Vec3::Y).unwrap()
    }
    fn floor(pos: Vec3) -> PortalFrame {
        PortalFrame::from_normal_up(pos, Vec3::Y, Vec3::NEG_Z).unwrap()
    }
    fn ceiling(pos: Vec3) -> PortalFrame {
        PortalFrame::from_normal_up(pos, Vec3::NEG_Y, Vec3::NEG_Z).unwrap()
    }

    /// A crossing where the centre moves from `d0` in front to `d1` behind the entry.
    fn crossing(entry: PortalFrame, exit: PortalFrame, v: Vec3, view: Quat, d0: f32, d1: f32) -> Crossing {
        let n = entry.normal();
        let c1 = entry.position + n * d1;
        let c0 = c1 + n * (d0 - d1);
        cross(&entry, &exit, PORTAL_HALF_SIZE, c0, c1, v, view, DT, &cfg())
    }

    #[test]
    fn default_config_is_portal_2() {
        let c = cfg();
        assert!(close(c.gravity, 600.0 * U, 1e-6));
        assert!(close(c.env_radius, 75.0 * U, 1e-6));
        assert!(close(c.hole_depth, 500.0 * U, 1e-6));
        assert!(close(c.hole_scale, 0.98, 1e-6));
        assert!(close(c.gravity_readd, 1.008, 1e-6));
    }

    #[test]
    fn gravity_fraction_matches_spec() {
        assert!(close(gravity_fraction(u(1.0), -u(1.0)), 0.5, 1e-6));
        // d0 = 4, d1 = -1: 1 - (-1 / -5) = 0.8.
        assert!(close(gravity_fraction(4.0, -1.0), 0.8, 1e-6));
    }

    /// wall -> wall, same height: horizontal velocity is preserved exactly; the only
    /// vertical change is the 0.008 * gcorr leak from the 1.008 re-application (§1.3).
    #[test]
    fn wall_to_wall_same_height() {
        let a = wall(Vec3::new(-12.0, 1.45, -4.0), Vec3::X);
        let b = wall(Vec3::new(12.0, 1.45, -4.0), Vec3::NEG_X);
        let v = Vec3::new(-u(300.0), 0.0, 0.0);
        let c = crossing(a, b, v, Quat::IDENTITY, u(1.0), -u(1.0));
        // gcorr.y = -dt * 600u * 0.5 = -5u; v_out.y = -5u * 1.008 + 5u = -0.04u.
        assert!(close(c.velocity.x, -u(300.0), 1e-4), "{:?}", c.velocity);
        assert!(close(c.velocity.y, -0.04 * U, 1e-4), "{:?}", c.velocity);
        assert!(close(c.velocity.z, 0.0, 1e-4));
        assert!(!c.force_duck, "wall to wall keeps the hull upright");
        assert!(close(c.velocity.length(), u(300.0), 0.1 * U));
    }

    /// wall -> floor: walking into the wall at 200 u/s comes out of the floor
    /// at exactly the 300 u/s floor minimum (§1.4).
    #[test]
    fn wall_to_floor_raises_to_floor_minimum() {
        let a = wall(Vec3::new(-12.0, 1.45, -4.0), Vec3::X);
        let f = floor(Vec3::new(-6.0, 0.0, -9.0));
        let v = Vec3::new(-u(200.0), 0.0, 0.0);
        let c = crossing(a, f, v, Quat::IDENTITY, u(1.0), -u(1.0));
        assert!(close(c.velocity.y, u(300.0), 1e-3), "{:?}", c.velocity);
        // Sideways leak from the gravity fix-up (5 u, toward -Z here).
        assert!(c.velocity.z < 0.0 && c.velocity.z.abs() < u(6.0), "{:?}", c.velocity);
        assert!(c.force_duck, "wall to floor turns the hull by 90 deg");
    }

    /// wall -> floor with a fast arrival: no floor minimum needed, the speed is kept.
    #[test]
    fn wall_to_floor_keeps_fast_speed() {
        let a = wall(Vec3::new(-12.0, 1.45, -4.0), Vec3::X);
        let f = floor(Vec3::new(-6.0, 0.0, -9.0));
        let c = crossing(a, f, Vec3::new(-u(600.0), 0.0, 0.0), Quat::IDENTITY, u(1.0), -u(1.0));
        // §1.3: half the tick is past the plane, so the gravity gained there
        // (600 u/s^2 * DT * 0.5) comes off and is re-added downward * 1.008.
        let g = 600.0 * DT * 0.5;
        assert!(close(c.velocity.y, u(600.0 - 1.008 * g), 0.05 * U), "{:?}", c.velocity);
    }

    /// floor -> ceiling: a fall into the floor comes out of the ceiling falling at the same speed.
    #[test]
    fn floor_to_ceiling_keeps_fall_speed() {
        let f = floor(Vec3::new(-6.0, 0.0, -6.0));
        let c_ = ceiling(Vec3::new(0.0, 10.0, -4.0));
        let c = crossing(f, c_, Vec3::new(0.0, -u(400.0), 0.0), Quat::IDENTITY, u(1.0), -u(1.0));
        // v_out.y = -400u + (-5u) * 0.008 = -400.04u (the exit is not a floor, no minimum).
        assert!(close(c.velocity.y, -u(400.04), 1e-3), "{:?}", c.velocity);
        assert!(close(c.velocity.x, 0.0, 1e-4) && close(c.velocity.z, 0.0, 1e-4));
        // Floor -> ceiling keeps the world up axis (map(Y) = Y): no forced crouch.
        assert!(!c.force_duck);
    }

    /// Fast fall into a floor, out of a wall: the 1000 u/s cap applies (§1.4).
    #[test]
    fn fast_fall_is_capped_at_1000_units() {
        let f = floor(Vec3::new(-6.0, 0.0, -6.0));
        let w = wall(Vec3::new(12.0, 1.45, -4.0), Vec3::NEG_X);
        let c = crossing(f, w, Vec3::new(0.0, -u(1500.0), 0.0), Quat::IDENTITY, u(1.0), -u(1.0));
        assert!(close(c.velocity.length(), 1000.0 * U, 1e-2), "{:?}", c.velocity);
        assert!(c.velocity.dot(Vec3::NEG_X) > 0.0);
    }

    /// 45 deg exit: the arc minimum (§1.4 tilted exit) raises the outward speed to
    /// sqrt(g X^2 / (X - Y)) with X = 100 u, Y = 0 -> sqrt(60000) u = 244.95 u/s.
    #[test]
    fn forty_five_degree_exit_arc_minimum() {
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let exit = PortalFrame::from_normal_up(Vec3::ZERO, Vec3::new(0.0, s, -s), Vec3::Y).unwrap();
        // Hull centre chosen so that the arc has X = 100 u, Y = 0 (see §1.4 formula).
        let hh = PORTAL_HALF_SIZE.y;
        let c = Vec3::new(0.0, 36.0 * U - hh * s, -hh * s + u(100.0));
        let entry = wall(Vec3::new(-12.0, 1.45, -4.0), Vec3::X);
        let v = Vec3::new(-u(100.0), 0.0, 0.0);
        let rotated = rotate_velocity(v, &PortalPair::new(entry, exit), DT, 0.5, &cfg());
        let out = exit_velocity(rotated, &exit, PORTAL_HALF_SIZE, entry.normal(), c, &cfg());
        let expected = (600.0 * U * (100.0 * U) * (100.0 * U) / (100.0 * U)).sqrt();
        assert!(close(out.dot(exit.normal()), expected, 1e-3), "outward {} vs {}", out.dot(exit.normal()), expected);
        assert!(close(expected, 244.949 * U, 1e-2));
        let tangential = out - exit.normal() * out.dot(exit.normal());
        let rot_tangential = rotated - exit.normal() * rotated.dot(exit.normal());
        assert!((tangential - rot_tangential).length() < 1e-4);
    }

    /// 45 deg exit with a fast arrival (400 u/s): no minimum, speed kept.
    #[test]
    fn forty_five_degree_exit_fast_keeps_speed() {
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let exit = PortalFrame::from_normal_up(Vec3::ZERO, Vec3::new(0.0, s, -s), Vec3::Y).unwrap();
        let entry = wall(Vec3::new(-12.0, 1.45, -4.0), Vec3::X);
        let v = Vec3::new(-u(400.0), 0.0, 0.0);
        let rotated = rotate_velocity(v, &PortalPair::new(entry, exit), DT, 0.5, &cfg());
        let out = exit_velocity(rotated, &exit, PORTAL_HALF_SIZE, entry.normal(), Vec3::ZERO, &cfg());
        assert!(close(out.length(), rotated.length().min(1000.0 * U), 1e-3));
    }

    /// The view: a straight-ahead look into a wall comes out along the exit normal.
    #[test]
    fn view_wall_to_wall_turns_around() {
        let a = wall(Vec3::new(-12.0, 1.45, -4.0), Vec3::X);
        let b = wall(Vec3::new(12.0, 1.45, -4.0), Vec3::NEG_X);
        let look = ViewAngles { yaw: FRAC_PI_2, pitch: 0.0, roll: 0.0 };
        let c = crossing(a, b, Vec3::NEG_X, look.rotation(), u(1.0), -u(1.0));
        let fwd = c.view.full_rotation() * Vec3::NEG_Z;
        assert!((fwd - Vec3::NEG_X).length() < 1e-4, "{fwd}");
        assert!(c.view.roll.abs() < 1e-4 && c.view.pitch.abs() < 1e-4);
        assert!((c.view.yaw - FRAC_PI_2).abs() < 1e-4);
    }

    /// The view: looking into a wall that leads to a floor keeps the exact transform
    /// (roll included), and leaves the floor going up.
    #[test]
    fn view_wall_to_floor_exact_transform() {
        let a = wall(Vec3::new(-12.0, 1.45, -4.0), Vec3::X);
        let f = floor(Vec3::new(-6.0, 0.0, -9.0));
        let look = Quat::from_rotation_y(FRAC_PI_2) * Quat::from_rotation_x(0.2) * Quat::from_rotation_z(0.3);
        let c = crossing(a, f, Vec3::NEG_X, look, u(1.0), -u(1.0));
        let exact = c.pair.map_rotation(look);
        let got = c.view.full_rotation();
        assert!(got.dot(exact).abs() > 0.9999, "{got:?} vs {exact:?}");
        let fwd = exact * Vec3::NEG_Z;
        assert!(fwd.dot(Vec3::Y) > 0.0);
    }

    /// The ring is four slabs covering the hole out to the env radius, 500 u deep.
    #[test]
    fn ring_covers_outside_the_hole() {
        let c_ = cfg();
        let pieces = ring_pieces(PORTAL_HALF_SIZE, &c_);
        let hw = PORTAL_HALF_SIZE.x * c_.hole_scale;
        let (c, s) = pieces[0];
        assert!(close(c.x - s.x / 2.0, hw, 1e-6));
        assert!(close(s.x, c_.env_radius, 1e-6));
        assert!(close(s.z, c_.hole_depth, 1e-6));
        let p = Vec3::new(hw + u(3.0), 0.0, -u(1.0));
        assert!(pieces.iter().any(|(c, s)| (p - *c).abs().cmple(*s / 2.0).all()));
        let q = Vec3::new(0.0, 0.0, -u(1.0));
        assert!(!pieces.iter().any(|(c, s)| (q - *c).abs().cmple(*s / 2.0).all()));
    }

    #[test]
    fn hole_test_hits_the_hull_and_not_far_away() {
        let frame = wall(Vec3::new(-12.0, 1.45, -4.0), Vec3::X);
        let he = Vec3::new(16.0 * U, 36.0 * U, 16.0 * U);
        let c_ = cfg();
        assert!(in_hole(&frame, PORTAL_HALF_SIZE, Vec3::new(-12.0, 1.0, -4.0), he, &c_));
        assert!(!in_hole(&frame, PORTAL_HALF_SIZE, Vec3::new(-8.0, 1.0, -4.0), he, &c_));
        assert!(!in_hole(&frame, PORTAL_HALF_SIZE, Vec3::new(-12.0, 1.0, -4.0 + 2.0), he, &c_));
    }
}

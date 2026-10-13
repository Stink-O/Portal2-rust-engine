//! Air funnelling into floor and ceiling portals (research/specs/portal-funnel.md
//! §1-§5). Pure maths on world metres and m/s; the controller calls it from
//! its air step with the list of linked portals.
use bevy::prelude::*;

use p2_core::U;

/// sv_player_funnel_snap_threshold (u/s).
/// Per-axis speed (u/s) below which the funnel snaps the axis to zero.
pub const SNAP_THRESHOLD: f32 = 10.0;
/// sv_player_funnel_speed_bonus.
/// Steering boost when rising at full speed (1 to this factor).
pub const SPEED_BONUS: f32 = 2.0;
/// sv_player_funnel_height_adjust (u).
/// Height above the portal the funnel aims the player at, u.
pub const HEIGHT_ADJUST: f32 = 128.0;
/// sv_player_funnel_gimme_dot.
/// Minimum vertical share of the speed for the exit gimme.
pub const GIMME_DOT: f32 = 0.9;

/// A linked portal the player may be funnelled into, in world space.
#[derive(Clone, Copy, Debug)]
pub struct FunnelPortal {
    /// Centre of the opening, m.
    pub centre: Vec3,
    /// Outward surface normal (the portal's local +Z).
    pub normal: Vec3,
    /// In-plane right axis (local +X).
    pub right: Vec3,
    /// In-plane up axis (local +Y).
    pub up: Vec3,
}

impl FunnelPortal {
    /// Build from a portal's `Transform` (its frame, see `p2_core::Portal`).
    pub fn from_transform(t: &Transform) -> Self {
        Self {
            centre: t.translation,
            normal: t.rotation * Vec3::Z,
            right: t.rotation * Vec3::X,
            up: t.rotation * Vec3::Y,
        }
    }
}

/// World (Y-up, metres) to Source (Z-up, units).
fn to_src(v: Vec3) -> Vec3 {
    Vec3::new(v.x, -v.z, v.y) / U
}

/// Source (Z-up, units) back to world (Y-up, metres).
fn to_world(v: Vec3) -> Vec3 {
    Vec3::new(v.x, v.z, -v.y) * U
}

/// Airborne steering (spec §1-§4). `gravity` is sv_gravity in u/s^2. `centre` is the hull centre (m),
/// `velocity` the character velocity (m/s): its horizontal part may be
/// damped here. `wish` is the horizontal wish velocity before air accel
/// (m/s). Returns the funnel term to add to the wish (m/s).
pub fn air_funnel(
    gravity: f32,
    portals: &[FunnelPortal],
    centre: Vec3,
    velocity: &mut Vec3,
    wish: Vec3,
    eye_forward: Vec3,
    dt: f32,
) -> Vec3 {
    let mut v = to_src(*velocity);
    let wish = to_src(wish);
    // Source-space eye forward.
    let fwd = Vec3::new(eye_forward.x, -eye_forward.z, eye_forward.y);
    if v.x * v.x + v.y * v.y > 300.0 * 300.0 {
        return Vec3::ZERO;
    }
    let c = to_src(centre);
    let g = gravity;

    let mut best: Option<(f32, Vec3)> = None;
    for p in portals {
        let pc = to_src(p.centre);
        let n = Vec3::new(p.normal.x, -p.normal.z, p.normal.y);
        let r = Vec3::new(p.right.x, -p.right.z, p.right.y);
        let up = Vec3::new(p.up.x, -p.up.z, p.up.y);
        let d = pc - c;
        if wish.x.abs() > 64.0 || wish.y.abs() > 64.0 {
            continue;
        }
        let vz = v.z;
        let ok = if vz <= 165.0 {
            fwd.z < -0.7 && vz < -165.0 && n.z > 0.8 && d.z >= -1024.0 && d.z < 0.0
        } else {
            n.z < -0.8 && d.z <= vz * vz / (2.0 * g) && d.z > 0.0 && d.z <= 1024.0
        };
        if !ok {
            continue;
        }
        let h = d.z.abs();
        let s = if h < 256.0 {
            1.5
        } else if h > 1024.0 {
            3.0
        } else {
            1.5 * (1.0 + (h - 256.0) / 768.0)
        };
        let r = (r - n * r.dot(n)).normalize_or_zero();
        let up = (up - n * up.dot(n)).normalize_or_zero();
        if d.dot(r).abs() > 32.0 * s || d.dot(up).abs() > 56.0 * s {
            continue;
        }
        if best.as_ref().is_none_or(|(bd, _)| d.length() < *bd) {
            best = Some((d.length(), d));
        }
    }
    let Some((_, d)) = best else { return Vec3::ZERO };

    // Time to reach 128 u above the portal centre: -g t^2 + 2 vz t - 2 (128 + d.z) = 0.
    let (a, b, cc) = (-g, 2.0 * v.z, -2.0 * (HEIGHT_ADJUST + d.z));
    let disc = b * b - 4.0 * a * cc;
    if disc < 0.0 {
        return Vec3::ZERO;
    }
    let sq = disc.sqrt();
    let roots = [(-b + sq) / (2.0 * a), (-b - sq) / (2.0 * a)];
    let Some(t) = roots.into_iter().filter(|t| *t > 0.0).reduce(f32::min) else { return Vec3::ZERO };
    let k = (v.z / 1065.0).clamp(0.0, 1.0);
    let extra = 1.0 + k * (SPEED_BONUS - 1.0);

    let h = d.z.abs();
    let half_life = if h < 128.0 {
        0.01
    } else if h <= 1024.0 {
        0.01 + 0.14 * (h - 128.0) / 896.0
    } else {
        0.15
    };
    let decay = (-std::f32::consts::LN_2 / half_life * dt).exp();
    let mut funnel = Vec3::ZERO;
    for i in 0..2 {
        if v[i] == 0.0 || d[i] / v[i] >= t {
            funnel[i] = 6.0 * extra * d[i] - v[i];
        } else if v[i].abs() > SNAP_THRESHOLD {
            v[i] *= decay;
        } else {
            v[i] = 0.0;
        }
    }
    *velocity = to_world(v);
    to_world(funnel)
}

/// Exit "gimme" (spec §5): out of an exactly horizontal portal within
/// ~25.8 deg of vertical, drop all horizontal speed.
pub fn exit_gimme(exit_normal: Vec3, velocity: Vec3) -> Vec3 {
    let len = velocity.length();
    if exit_normal.y.abs() > 0.999 && len > 0.0 && velocity.y.abs() / len > GIMME_DOT {
        Vec3::new(0.0, velocity.y, 0.0)
    } else {
        velocity
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn floor_portal(at: Vec3) -> Vec<FunnelPortal> {
        vec![FunnelPortal { centre: at, normal: Vec3::Y, right: Vec3::X, up: Vec3::NEG_Z }]
    }

    #[test]
    fn falling_toward_offset_floor_portal_steers() {
        let t = floor_portal(Vec3::new(1.0, 0.0, 0.0));
        let mut v = Vec3::new(0.0, -8.0, 0.0);
        let f = air_funnel(600.0, &t, Vec3::new(0.0, 5.0, 0.0), &mut v, Vec3::ZERO, Vec3::NEG_Y, 1.0 / 60.0);
        assert!(f.x > 0.0, "steers toward the portal: {f}");
    }

    #[test]
    fn not_when_looking_ahead_or_slow() {
        let t = floor_portal(Vec3::new(1.0, 0.0, 0.0));
        let mut v = Vec3::new(0.0, -8.0, 0.0);
        assert_eq!(air_funnel(600.0, &t, Vec3::new(0.0, 5.0, 0.0), &mut v, Vec3::ZERO, Vec3::NEG_Z, 0.016), Vec3::ZERO);
        let mut slow = Vec3::new(0.0, -1.0, 0.0);
        assert_eq!(air_funnel(600.0, &t, Vec3::new(0.0, 5.0, 0.0), &mut slow, Vec3::ZERO, Vec3::NEG_Y, 0.016), Vec3::ZERO);
    }

    #[test]
    fn gimme_straightens() {
        assert_eq!(exit_gimme(Vec3::Y, Vec3::new(1.0, 10.0, 0.5)), Vec3::new(0.0, 10.0, 0.0));
        assert_eq!(exit_gimme(Vec3::X, Vec3::new(1.0, 10.0, 0.5)), Vec3::new(1.0, 10.0, 0.5));
    }
}

//! Pure portal maths: pair transforms, view levelling, the oblique near
//! plane and placement fitting. No ECS here, so it is all unit-testable.
//!
//! Portal frame convention: origin at the centre of the opening, local +Z is
//! the surface normal (out of the wall, toward whoever is looking at it),
//! local +Y is the portal's "up", local +X is its right.
use bevy::math::{Affine3A, Mat4, Quat, Vec2, Vec3, Vec4};

/// Half the width of a Portal 2 portal in Source units (32; see
/// [`PORTAL_HALF_HEIGHT_UNITS`]).
pub const PORTAL_HALF_WIDTH_UNITS: f32 = 32.0;
/// Half the height of a Portal 2 portal in Source units: 56, i.e. a 64 x 112
/// unit opening (research/specs/portal-transit.md §0, read from the
/// decompiled Portal 2 server; Portal 1's constant was 54).
pub const PORTAL_HALF_HEIGHT_UNITS: f32 = 56.0;

/// Half extents of a Portal 2 portal opening in metres (0.8128 x 1.4224).
/// The full opening is 1.63 m x 2.84 m.
pub const PORTAL_HALF_SIZE: Vec2 = Vec2::new(
    PORTAL_HALF_WIDTH_UNITS * shared::SOURCE_UNIT,
    PORTAL_HALF_HEIGHT_UNITS * shared::SOURCE_UNIT,
);

/// A rigid portal frame (rotation + translation, no scale).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PortalFrame {
    pub position: Vec3,
    pub rotation: Quat,
}

impl PortalFrame {
    pub fn new(position: Vec3, rotation: Quat) -> Self {
        Self { position, rotation }
    }

    /// Frame from a centre, outward normal and up hint (the up hint is
    /// projected onto the plane).
    pub fn from_normal_up(position: Vec3, normal: Vec3, up_hint: Vec3) -> Option<Self> {
        let n = normal.try_normalize()?;
        let up = (up_hint - n * up_hint.dot(n)).try_normalize()?;
        let right = up.cross(n);
        let rot = Quat::from_mat3(&bevy::math::Mat3::from_cols(right, up, n)).normalize();
        Some(Self::new(position, rot))
    }

    pub fn normal(&self) -> Vec3 {
        self.rotation * Vec3::Z
    }
    pub fn up(&self) -> Vec3 {
        self.rotation * Vec3::Y
    }
    pub fn right(&self) -> Vec3 {
        self.rotation * Vec3::X
    }

    /// World point to portal-local coordinates.
    pub fn to_local(&self, p: Vec3) -> Vec3 {
        self.rotation.inverse() * (p - self.position)
    }
    /// Portal-local point to world.
    pub fn to_world(&self, l: Vec3) -> Vec3 {
        self.position + self.rotation * l
    }
    /// Signed distance of a point in front of the portal plane.
    pub fn plane_distance(&self, p: Vec3) -> f32 {
        (p - self.position).dot(self.normal())
    }

    pub fn affine(&self) -> Affine3A {
        Affine3A::from_rotation_translation(self.rotation, self.position)
    }
}

/// The mapping through a linked pair: whatever enters `entry` leaves `exit`.
///
/// A point `z` metres in front of `entry` ends up `z` metres *behind* `exit`
/// (and vice versa); left/right are mirrored so the result is a proper
/// rotation, as if the two openings were glued back to back.
#[derive(Clone, Copy, Debug)]
pub struct PortalPair {
    pub entry: PortalFrame,
    pub exit: PortalFrame,
    /// Rotation that takes entry-side directions to exit-side directions.
    pub rotation: Quat,
}

/// 180 degrees about local Y.
const FLIP: Quat = Quat::from_xyzw(0.0, 1.0, 0.0, 0.0);

impl PortalPair {
    pub fn new(entry: PortalFrame, exit: PortalFrame) -> Self {
        let rotation = (exit.rotation * FLIP * entry.rotation.inverse()).normalize();
        Self { entry, exit, rotation }
    }

    /// Maps a world point. Done relative to the frames (not by a composed
    /// matrix) so it stays precise when the portals are kilometres apart.
    pub fn map_point(&self, p: Vec3) -> Vec3 {
        let local = self.entry.rotation.inverse() * (p - self.entry.position);
        self.exit.position + self.exit.rotation * (FLIP * local)
    }
    pub fn map_dir(&self, v: Vec3) -> Vec3 {
        self.rotation * v
    }
    pub fn map_rotation(&self, r: Quat) -> Quat {
        (self.rotation * r).normalize()
    }
    /// Maps a rigid transform (rotation, translation).
    pub fn map_frame(&self, rotation: Quat, translation: Vec3) -> (Quat, Vec3) {
        (self.map_rotation(rotation), self.map_point(translation))
    }
    /// The same mapping as an affine matrix (for camera chains).
    pub fn affine(&self) -> Affine3A {
        self.exit.affine() * Affine3A::from_quat(FLIP) * self.entry.affine().inverse()
    }
    pub fn inverse(&self) -> Self {
        Self::new(self.exit, self.entry)
    }
}

/// Where a moving point crossed the front face of a portal opening.
///
/// Returns the crossing fraction `t` in `[0, 1]` if the segment `from -> to`
/// goes from the front side (or exactly on the plane) to strictly behind,
/// through the rectangle of half extents `half_size`.
pub fn segment_crosses_opening(frame: &PortalFrame, half_size: Vec2, from: Vec3, to: Vec3) -> Option<f32> {
    let d0 = frame.plane_distance(from);
    let d1 = frame.plane_distance(to);
    if !(d0 >= 0.0 && d1 < 0.0) {
        return None;
    }
    let t = d0 / (d0 - d1);
    let hit = from.lerp(to, t);
    let l = frame.to_local(hit);
    (l.x.abs() <= half_size.x && l.y.abs() <= half_size.y).then_some(t)
}

/// A first-person view split into the parts a character controller keeps
/// (yaw on the body, pitch on the camera) and the roll that Portal 2 eases
/// back to level after you come out of a portal at an angle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LevelledView {
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
}

impl LevelledView {
    pub fn body_rotation(&self) -> Quat {
        Quat::from_rotation_y(self.yaw)
    }
    pub fn camera_local_rotation(&self) -> Quat {
        Quat::from_rotation_x(self.pitch)
    }
    pub fn full_rotation(&self) -> Quat {
        Quat::from_rotation_y(self.yaw) * Quat::from_rotation_x(self.pitch) * Quat::from_rotation_z(self.roll)
    }
}

/// Splits a camera rotation (looking down -Z, +Y up) into yaw / pitch / roll
/// with `rotation == Ry(yaw) * Rx(pitch) * Rz(roll)`.
///
/// When looking straight up or down the heading comes from the camera's up
/// vector instead of its forward vector, so a floor -> wall fling keeps the
/// heading you had while falling.
pub fn level_view(rotation: Quat) -> LevelledView {
    let f = rotation * Vec3::NEG_Z;
    let up = rotation * Vec3::Y;
    let pitch = f.y.clamp(-1.0, 1.0).asin();
    let flat = Vec2::new(f.x, f.z);
    let heading = if flat.length_squared() > 1e-4 {
        flat
    } else {
        // Looking down: the top of the screen points where you are heading.
        let h = up * -f.y.signum();
        Vec2::new(h.x, h.z)
    };
    let heading = heading.try_normalize().unwrap_or(Vec2::new(0.0, -1.0));
    let yaw = (-heading.x).atan2(-heading.y);
    let level = Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch);
    let residual = (level.inverse() * rotation).normalize();
    // The residual is (almost exactly) a rotation about local Z.
    let mut roll = 2.0 * residual.z.atan2(residual.w);
    if roll > std::f32::consts::PI {
        roll -= std::f32::consts::TAU;
    } else if roll < -std::f32::consts::PI {
        roll += std::f32::consts::TAU;
    }
    LevelledView { yaw, pitch, roll }
}

/// Replaces the near plane of Bevy's reverse-Z infinite perspective matrix
/// with an arbitrary view-space plane (Lengyel, "Oblique View Frustum Depth
/// Projection and Clipping", 2005), adapted to reverse Z (near = 1, far = 0).
///
/// `plane` is `(n, w)` in view space with `n · p + w >= 0` for everything that
/// stays visible; the camera itself must be on the negative side (`w < 0`).
pub fn oblique_reverse_z(projection: Mat4, plane: Vec4) -> Mat4 {
    // In reverse-Z clip space the far plane (at infinity) is depth 0 and the
    // near plane is depth = w. Rows: r3 = depth numerator, r4 = w.
    // The near half-space is r4 - r3 >= 0. We want it to be `a * plane`,
    // so r3' = r4 - a * plane. `a` is chosen so the new far plane r3' still
    // passes through the frustum corner opposite the clip plane, which keeps
    // every point of the original frustum at depth >= 0.
    let row4 = Vec4::new(
        projection.x_axis.w,
        projection.y_axis.w,
        projection.z_axis.w,
        projection.w_axis.w,
    );
    let corner_clip = Vec4::new(plane.x.signum(), plane.y.signum(), 0.0, 1.0);
    let q = projection.inverse() * corner_clip;
    let denom = plane.dot(q);
    if denom.abs() < 1e-12 {
        return projection;
    }
    let a = row4.dot(q) / denom;
    let r3 = row4 - plane * a;
    let mut m = projection;
    m.x_axis.z = r3.x;
    m.y_axis.z = r3.y;
    m.z_axis.z = r3.z;
    m.w_axis.z = r3.w;
    m
}

/// Half extent of an axis-aligned box (world half extents `he`) projected on
/// a unit direction.
pub fn box_extent_along(he: Vec3, dir: Vec3) -> f32 {
    he.x * dir.x.abs() + he.y * dir.y.abs() + he.z * dir.z.abs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::{FRAC_PI_2, PI};

    fn approx(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-4
    }

    fn wall(pos: Vec3, normal: Vec3) -> PortalFrame {
        PortalFrame::from_normal_up(pos, normal, Vec3::Y).unwrap()
    }

    #[test]
    fn frame_axes() {
        let f = wall(Vec3::ZERO, Vec3::X);
        assert!(approx(f.normal(), Vec3::X));
        assert!(approx(f.up(), Vec3::Y));
        assert!(approx(f.right(), Vec3::Y.cross(Vec3::X)));
    }

    #[test]
    fn opposite_walls_are_a_corridor() {
        // A on the wall at x = 0 facing +X, B at x = 10 facing -X.
        let a = wall(Vec3::new(0.0, 1.0, 0.0), Vec3::X);
        let b = wall(Vec3::new(10.0, 1.0, 0.0), Vec3::NEG_X);
        let pair = PortalPair::new(a, b);
        // 1 m in front of A -> 1 m behind B.
        assert!(approx(pair.map_point(Vec3::new(1.0, 1.0, 0.0)), Vec3::new(11.0, 1.0, 0.0)));
        // Walking into A (-X) comes out of B moving -X (away from B, into the room).
        assert!(approx(pair.map_dir(Vec3::NEG_X), Vec3::NEG_X));
        // Lateral offsets are mirrored consistently (proper rotation).
        assert!(approx(pair.map_point(Vec3::new(0.0, 1.0, 0.5)), Vec3::new(10.0, 1.0, 0.5)));
        let m = pair.affine();
        assert!(approx(m.transform_point3(Vec3::new(1.0, 1.0, 0.3)), pair.map_point(Vec3::new(1.0, 1.0, 0.3))));
    }

    #[test]
    fn same_facing_walls_turn_you_around() {
        let a = wall(Vec3::ZERO, Vec3::Z);
        let b = wall(Vec3::new(5.0, 0.0, 0.0), Vec3::Z);
        let pair = PortalPair::new(a, b);
        // Going into A (-Z) comes out of B toward +Z.
        assert!(approx(pair.map_dir(Vec3::NEG_Z), Vec3::Z));
        assert!(approx(pair.map_point(Vec3::new(0.3, 0.0, 0.2)), Vec3::new(5.0 - 0.3, 0.0, -0.2)));
    }

    #[test]
    fn floor_to_wall_fling_keeps_speed() {
        // Floor portal facing up, wall portal facing +X.
        let floor = PortalFrame::from_normal_up(Vec3::ZERO, Vec3::Y, Vec3::NEG_Z).unwrap();
        let wallp = wall(Vec3::new(20.0, 3.0, 0.0), Vec3::X);
        let pair = PortalPair::new(floor, wallp);
        let v_in = Vec3::new(0.0, -17.0, 0.0);
        let v_out = pair.map_dir(v_in);
        assert!((v_out.length() - 17.0).abs() < 1e-4, "speed preserved");
        assert!(approx(v_out, Vec3::new(17.0, 0.0, 0.0)), "comes out of the wall: {v_out}");
        // Velocity along the floor maps onto the wall plane.
        let slide = pair.map_dir(Vec3::NEG_Z);
        assert!(slide.dot(Vec3::X).abs() < 1e-4);
        // Inverse pair undoes it.
        let back = pair.inverse();
        assert!(approx(back.map_point(pair.map_point(Vec3::new(0.2, -0.3, 0.1))), Vec3::new(0.2, -0.3, 0.1)));
    }

    #[test]
    fn crossing_detection() {
        let a = wall(Vec3::ZERO, Vec3::Z);
        let hs = PORTAL_HALF_SIZE;
        assert!(segment_crosses_opening(&a, hs, Vec3::new(0.0, 0.0, 0.2), Vec3::new(0.0, 0.0, -0.1)).is_some());
        // Coming from behind never counts.
        assert!(segment_crosses_opening(&a, hs, Vec3::new(0.0, 0.0, -0.2), Vec3::new(0.0, 0.0, 0.1)).is_none());
        // Outside the rectangle.
        assert!(segment_crosses_opening(&a, hs, Vec3::new(2.0, 0.0, 0.2), Vec3::new(2.0, 0.0, -0.1)).is_none());
        let t = segment_crosses_opening(&a, hs, Vec3::new(0.0, 0.0, 0.3), Vec3::new(0.0, 0.0, -0.1)).unwrap();
        assert!((t - 0.75).abs() < 1e-5);
    }

    #[test]
    fn levelling_round_trips() {
        for &(yaw, pitch, roll) in &[(0.3, 0.2, 0.0), (-2.0, -1.0, 0.7), (3.0, 1.2, -2.5), (1.0, 0.0, PI - 0.01)] {
            let q = Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch) * Quat::from_rotation_z(roll);
            let v = level_view(q);
            assert!((v.yaw - yaw).abs() < 1e-3, "{v:?}");
            assert!((v.pitch - pitch).abs() < 1e-3, "{v:?}");
            assert!((v.roll - roll).abs() < 1e-3, "{v:?}");
            assert!(v.full_rotation().dot(q).abs() > 0.9999);
        }
    }

    #[test]
    fn looking_straight_down_keeps_heading() {
        // Looking down with the top of the screen toward -X.
        let q = Quat::from_rotation_y(FRAC_PI_2) * Quat::from_rotation_x(-FRAC_PI_2);
        let v = level_view(q);
        assert!((v.pitch + FRAC_PI_2).abs() < 1e-3);
        let heading = Quat::from_rotation_y(v.yaw) * Vec3::NEG_Z;
        assert!(approx(heading, Vec3::NEG_X), "{heading}");
        assert!(v.roll.abs() < 1e-3);
    }

    #[test]
    fn fall_into_floor_look_out_of_wall() {
        // Falling into a floor portal while looking down, heading -Z.
        let floor = PortalFrame::from_normal_up(Vec3::ZERO, Vec3::Y, Vec3::NEG_Z).unwrap();
        let wallp = wall(Vec3::new(20.0, 3.0, 0.0), Vec3::X);
        let pair = PortalPair::new(floor, wallp);
        let cam = Quat::from_rotation_x(-FRAC_PI_2); // looking straight down, top of screen -Z
        let out = level_view(pair.map_rotation(cam));
        let fwd = out.full_rotation() * Vec3::NEG_Z;
        assert!(approx(fwd, Vec3::X), "looks out of the wall: {fwd}");
        assert!(out.pitch.abs() < 1e-3);
        assert!(out.roll.abs() < 1e-3, "{out:?}");
    }

    #[test]
    fn oblique_projection_clips_on_plane() {
        let proj = Mat4::perspective_infinite_reverse_rh(1.0, 1.5, 0.1);
        // Plane 3 m ahead, tilted: keep points with n.p + w >= 0.
        let n = Vec3::new(0.3, 0.1, -1.0).normalize();
        let on_plane = Vec3::new(0.2, -0.1, -3.0);
        let plane = n.extend(-n.dot(on_plane));
        let m = oblique_reverse_z(proj, plane);
        let depth = |p: Vec3| {
            let c = m * p.extend(1.0);
            c.z / c.w
        };
        // On the plane: depth == 1 (the new near plane).
        let d = depth(on_plane);
        assert!((d - 1.0).abs() < 1e-3, "on-plane depth {d}");
        // Just beyond it: visible (0 < depth < 1).
        let beyond = on_plane + n * 0.5;
        let d = depth(beyond);
        assert!(d > 0.0 && d < 1.0, "beyond depth {d}");
        // Between camera and plane: clipped (depth > 1).
        let before = on_plane - n * 0.5;
        assert!(depth(before) > 1.0);
        // Far away but inside the view: still visible.
        let far = Vec3::new(0.0, 0.0, -5000.0);
        let d = depth(far);
        assert!(d >= 0.0 && d < 1.0, "far depth {d}");
        // x/y unchanged.
        let c0 = proj * beyond.extend(1.0);
        let c1 = m * beyond.extend(1.0);
        assert!((c0.x / c0.w - c1.x / c1.w).abs() < 1e-5);
    }

    #[test]
    fn camera_chain_matches_affine() {
        let a = wall(Vec3::new(0.0, 1.0, 0.0), Vec3::X);
        let b = PortalFrame::from_normal_up(Vec3::new(3.0, 0.0, 7.0), Vec3::Y, Vec3::Z).unwrap();
        let pair = PortalPair::new(a, b);
        let cam = Affine3A::from_rotation_translation(Quat::from_rotation_y(0.4), Vec3::new(2.0, 1.6, 0.3));
        let virt = pair.affine() * cam;
        let (_, r, t) = virt.to_scale_rotation_translation();
        let (r2, t2) = pair.map_frame(Quat::from_rotation_y(0.4), Vec3::new(2.0, 1.6, 0.3));
        assert!(approx(t, t2));
        assert!(r.dot(r2).abs() > 0.9999);
    }
}

/// Portal 2's `sv_gravity` in m/s^2 (600 u/s^2).
pub const SV_GRAVITY: f32 = 600.0 * shared::SOURCE_UNIT;
/// Portal 2's exit speed cap: 1000 u/s.
pub const PORTAL_MAX_EXIT_SPEED: f32 = 1000.0 * shared::SOURCE_UNIT;
/// Minimum outward speed from a floor portal for the player: 300 u/s.
pub const PORTAL_FLOOR_EXIT_MIN_SPEED_PLAYER: f32 = 300.0 * shared::SOURCE_UNIT;
/// `sv_maxvelocity` (per-axis clamp): 3500 u/s.
pub const SV_MAXVELOCITY: f32 = 3500.0 * shared::SOURCE_UNIT;
/// cos(30 deg): an exit facing up more than 60 deg from the horizon is a floor portal.
pub const FLOOR_PORTAL_COS: f32 = 0.866_025_4;

/// Minimum outward exit speed (m/s) per Portal 2's rules
/// (portal-transit.md §1.4 / §2.2). `arc` is the player's tilted-portal
/// launch speed if it applies.
pub fn exit_min_speed(exit_normal: Vec3, entrance_normal: Vec3, is_player: bool, arc: Option<f32>) -> Option<f32> {
    let exit_floor = exit_normal.y > FLOOR_PORTAL_COS;
    let entrance_floor = entrance_normal.y > FLOOR_PORTAL_COS;
    if is_player {
        if exit_floor {
            Some(PORTAL_FLOOR_EXIT_MIN_SPEED_PLAYER)
        } else if exit_normal.y > 0.5 {
            arc.filter(|a| *a > 0.0).map(|a| a.min(PORTAL_FLOOR_EXIT_MIN_SPEED_PLAYER))
        } else {
            None
        }
    } else if exit_floor {
        Some(if entrance_floor { 225.0 } else { 50.0 } * shared::SOURCE_UNIT)
    } else {
        None
    }
}

/// Apply Portal 2's exit speed rules to a velocity that has already been
/// rotated through the pair: raise the outward component to the minimum, or
/// clamp total speed to 1000 u/s, then clamp each axis to sv_maxvelocity.
pub fn clamp_exit_velocity(v: Vec3, exit_normal: Vec3, min_speed: Option<f32>) -> Vec3 {
    let mut v = v;
    let out = v.dot(exit_normal);
    match min_speed {
        Some(m) if out < m => v += exit_normal * (m - out),
        _ => {
            let len = v.length();
            if len > PORTAL_MAX_EXIT_SPEED {
                v *= PORTAL_MAX_EXIT_SPEED / len;
            }
        }
    }
    v.clamp(Vec3::splat(-SV_MAXVELOCITY), Vec3::splat(SV_MAXVELOCITY))
}

/// Launch speed along a tilted exit portal's normal that arcs the player's
/// hull centre `c` to the exit's bottom edge raised 36 u (portal-transit.md
/// §1.4). World space, metres; `None` if there is no real positive root.
pub fn arc_speed(exit: &PortalFrame, exit_half: Vec2, c: Vec3, gravity: f32) -> Option<f32> {
    let u = shared::SOURCE_UNIT;
    let mut t = (exit.position - exit.up() * exit_half.y) - c + Vec3::Y * (36.0 * u);
    t -= exit.right() * t.dot(exit.right());
    let x = Vec2::new(t.x, t.z).length();
    let y = t.y;
    let f = exit.normal();
    let fh = Vec2::new(f.x, f.z).length();
    let fz = f.y;
    let a = -2.0 * fz * (x * fh - y * fz);
    if a >= 0.0 {
        return None;
    }
    let v2 = gravity * x * x / -a;
    (v2 > 0.0).then(|| v2.sqrt().min(PORTAL_FLOOR_EXIT_MIN_SPEED_PLAYER))
}

/// Portal 2's roll re-levelling step (portal-transit.md §1.7): exponential
/// approach with a 0.125 s half-life, at least 10 deg/s, snapping under
/// 0.1 deg. Returns the remaining roll (radians).
pub fn relevel_roll(roll: f32, dt: f32) -> f32 {
    let angle = roll.abs().to_degrees();
    if angle <= 0.1 {
        return 0.0;
    }
    let mut step = angle * (1.0 - (-5.545_177_5 * dt).exp());
    if dt > 0.0 && step / dt < 10.0 {
        step = 10.0 * dt;
    }
    let left = (angle - step).max(0.0);
    if left <= 0.1 {
        0.0
    } else {
        left.to_radians() * roll.signum()
    }
}

#[cfg(test)]
mod transit_rule_tests {
    use super::*;

    #[test]
    fn exit_speed_rules() {
        let u = shared::SOURCE_UNIT;
        let up = Vec3::Y;
        let wall = Vec3::X;
        // Floor exit: at least 300 u/s out for the player.
        let v = clamp_exit_velocity(Vec3::ZERO, up, exit_min_speed(up, wall, true, None));
        assert!((v.y - 300.0 * u).abs() < 1e-4);
        // Non-player floor->floor: 225 u/s.
        let v = clamp_exit_velocity(Vec3::ZERO, up, exit_min_speed(up, up, false, None));
        assert!((v.y - 225.0 * u).abs() < 1e-4);
        // Wall exit, fast: capped to 1000 u/s.
        let v = clamp_exit_velocity(wall * 40.0, wall, exit_min_speed(wall, up, true, None));
        assert!((v.length() - 1000.0 * u).abs() < 1e-3);
    }

    #[test]
    fn relevel_half_life() {
        let mut r = 90f32.to_radians();
        let mut t = 0.0;
        while r != 0.0 && t < 5.0 {
            r = relevel_roll(r, 1.0 / 60.0);
            t += 1.0 / 60.0;
        }
        assert!(t > 0.5 && t < 1.2, "90 deg roll levelled in {t}s");
    }
}

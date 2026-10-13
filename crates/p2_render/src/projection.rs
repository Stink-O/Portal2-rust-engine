//! A perspective projection whose near plane is replaced by an arbitrary
//! (oblique) plane, so a camera looking out of a portal never draws what is
//! behind the exit portal's wall.
use bevy::camera::{CameraProjection, SubCameraView};
use bevy::math::Vec3A;
use bevy::prelude::*;

use p2_core::math::oblique_reverse_z;

/// Reverse-Z infinite perspective, like Bevy's [`PerspectiveProjection`], plus
/// an optional oblique clip plane in view space.
#[derive(Clone, Debug)]
pub struct PortalProjection {
    pub fov: f32,
    pub aspect_ratio: f32,
    pub near: f32,
    pub far: f32,
    /// `(n, w)` in view space; geometry with `n · p + w < 0` is clipped. The
    /// camera must be on the clipped side. `None` = ordinary near plane.
    pub clip_plane: Option<Vec4>,
}

impl Default for PortalProjection {
    fn default() -> Self {
        let p = PerspectiveProjection::default();
        Self {
            fov: p.fov,
            aspect_ratio: p.aspect_ratio,
            near: p.near,
            far: p.far,
            clip_plane: None,
        }
    }
}

impl PortalProjection {
    fn base(&self) -> Mat4 {
        Mat4::perspective_infinite_reverse_rh(self.fov, self.aspect_ratio, self.near)
    }

    fn apply_clip(&self, m: Mat4) -> Mat4 {
        match self.clip_plane {
            // w must be negative (camera behind the plane); keep a hair of
            // distance so depth stays well conditioned.
            Some(plane) if plane.w < 0.0 => {
                let mut plane = plane;
                plane.w = plane.w.min(-1e-4);
                oblique_reverse_z(m, plane)
            }
            _ => m,
        }
    }
}

impl CameraProjection for PortalProjection {
    fn get_clip_from_view(&self) -> Mat4 {
        self.apply_clip(self.base())
    }

    fn get_clip_from_view_for_sub(&self, sub_view: &SubCameraView) -> Mat4 {
        // Same maths as Bevy's perspective sub view, then the clip plane.
        let full_width = sub_view.full_size.x as f32;
        let full_height = sub_view.full_size.y as f32;
        let sub_width = sub_view.size.x as f32;
        let sub_height = sub_view.size.y as f32;
        let offset_x = sub_view.offset.x;
        let offset_y = full_height - (sub_view.offset.y + sub_height);
        let full_aspect = full_width / full_height;
        let top = self.near * (0.5 * self.fov).tan();
        let right = top * full_aspect;
        let sub_left = -right + 2.0 * right * offset_x / full_width;
        let sub_right = sub_left + 2.0 * right * sub_width / full_width;
        let sub_bottom = -top + 2.0 * top * offset_y / full_height;
        let sub_top = sub_bottom + 2.0 * top * sub_height / full_height;
        let x = (2.0 * self.near) / (sub_right - sub_left);
        let y = (2.0 * self.near) / (sub_top - sub_bottom);
        let a = (sub_right + sub_left) / (sub_right - sub_left);
        let b = (sub_top + sub_bottom) / (sub_top - sub_bottom);
        let m = Mat4::from_cols(
            Vec4::new(x, 0.0, 0.0, 0.0),
            Vec4::new(0.0, y, 0.0, 0.0),
            Vec4::new(a, b, 0.0, -1.0),
            Vec4::new(0.0, 0.0, self.near, 0.0),
        );
        self.apply_clip(m)
    }

    fn update(&mut self, width: f32, height: f32) {
        if width > 0.0 && height > 0.0 {
            self.aspect_ratio = width / height;
        }
    }

    fn far(&self) -> f32 {
        self.far
    }

    fn get_frustum_corners(&self, z_near: f32, z_far: f32) -> [Vec3A; 8] {
        let t = (self.fov / 2.0).tan();
        let a = z_near.abs() * t;
        let b = z_far.abs() * t;
        let r = self.aspect_ratio;
        [
            Vec3A::new(a * r, -a, z_near),
            Vec3A::new(a * r, a, z_near),
            Vec3A::new(-a * r, a, z_near),
            Vec3A::new(-a * r, -a, z_near),
            Vec3A::new(b * r, -b, z_far),
            Vec3A::new(b * r, b, z_far),
            Vec3A::new(-b * r, b, z_far),
            Vec3A::new(-b * r, -b, z_far),
        ]
    }
}

//! The player's portal enter / exit sounds (research/specs/player-sounds.md §1).
//!
//! Each tick, a box around the player's hull centre is tested against the
//! rectangles of the linked portals. When it first touches one,
//! `PortalPlayer.EnterPortal` plays; when it no longer touches any (the larger
//! "exit" box, for hysteresis), `PortalPlayer.ExitPortal` plays. The teleport
//! itself plays nothing: walking through gives "enter" at the entry portal
//! and "exit" on stepping off the exit portal. Volume and soundlevel rise with
//! speed.
use bevy::prelude::*;
use p2_core::math::PortalFrame;
use p2_core::{CharacterVelocity, Crouch, P2Systems, Player, Portal, PortalLink, U};

use crate::sounds::PlaySound;
use crate::FxSettings;

/// Half-extents of the test box as a fraction of the hull size, while not
/// touching a portal (enter test) and while touching one (exit test).
const ENTER_BOX: f32 = 0.25;
const EXIT_BOX: f32 = 0.3;
/// Below this speed on every axis (u/s) the state is left alone.
const STILL: f32 = 0.01;

/// Whether a player's test box touches a portal rectangle.
#[derive(Component, Default)]
pub(crate) struct TouchingPortal(bool);

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(FixedUpdate, portal_touch_sounds.after(P2Systems::Transit));
}

/// Volume and soundlevel (dB) of the enter / exit sound at `speed` u/s.
pub fn transit_sound_params(speed: f32) -> (f32, f32) {
    let volume = if speed < 933.333_3 { 0.3 + 0.00075 * speed } else { 1.0 };
    let soundlevel = (80.0 + 0.03 * speed).trunc();
    (volume, soundlevel)
}

/// Separating-axis test: axis-aligned box (`centre`, half-extents `he`)
/// against the portal's opening rectangle (half size `half` in its plane).
pub fn box_touches_rectangle(centre: Vec3, he: Vec3, frame: &PortalFrame, half: Vec2) -> bool {
    let (r, u, n) = (frame.right(), frame.up(), frame.normal());
    let d = centre - frame.to_world(Vec3::ZERO);
    let box_r = |axis: Vec3| he.x * axis.x.abs() + he.y * axis.y.abs() + he.z * axis.z.abs();
    let rect_r = |axis: Vec3| half.x * r.dot(axis).abs() + half.y * u.dot(axis).abs();
    let mut axes = vec![n, r, u, Vec3::X, Vec3::Y, Vec3::Z];
    for edge in [r, u] {
        for b in [Vec3::X, Vec3::Y, Vec3::Z] {
            let c = edge.cross(b);
            if c.length_squared() > 1e-8 {
                axes.push(c.normalize());
            }
        }
    }
    axes.into_iter().all(|a| d.dot(a).abs() <= box_r(a) + rect_r(a))
}

fn portal_touch_sounds(
    mut commands: Commands,
    settings: Res<FxSettings>,
    mut players: Query<(Entity, &Transform, &CharacterVelocity, &Crouch, Option<&mut TouchingPortal>), With<Player>>,
    portals: Query<(&Portal, &Transform), With<PortalLink>>,
    mut sounds: MessageWriter<PlaySound>,
) {
    for (entity, tf, vel, crouch, state) in &mut players {
        let Some(mut touch) = state else {
            commands.entity(entity).insert(TouchingPortal::default());
            continue;
        };
        let v = vel.0 / U;
        if v.x.abs() < STILL && v.y.abs() < STILL && v.z.abs() < STILL {
            continue;
        }
        let centre = tf.translation + crouch.hull_centre_offset();
        let he = crouch.hull() * if touch.0 { EXIT_BOX } else { ENTER_BOX };
        let touches = portals.iter().any(|(p, ptf)| box_touches_rectangle(centre, he, &Portal::frame(ptf), p.half_size));
        if touches == touch.0 {
            continue;
        }
        touch.0 = touches;
        if !settings.sounds {
            continue;
        }
        let (volume, soundlevel) = transit_sound_params(v.length());
        let name = if touches { "PortalPlayer.EnterPortal" } else { "PortalPlayer.ExitPortal" };
        sounds.write(PlaySound::at(name, centre).with_volume(volume).with_soundlevel(soundlevel));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_scales_volume_and_level() {
        assert_eq!(transit_sound_params(0.0), (0.3, 80.0));
        let (v, l) = transit_sound_params(200.0);
        assert!((v - 0.45).abs() < 1e-6 && l == 86.0);
        assert_eq!(transit_sound_params(2000.0).0, 1.0);
    }

    #[test]
    fn box_must_touch_the_rectangle_not_just_the_plane() {
        let frame = PortalFrame::from_normal_up(Vec3::ZERO, Vec3::X, Vec3::Y).unwrap();
        let half = Vec2::new(0.6, 1.0);
        let he = Vec3::splat(0.2);
        assert!(box_touches_rectangle(Vec3::new(0.15, 0.0, 0.0), he, &frame, half));
        assert!(!box_touches_rectangle(Vec3::new(0.25, 0.0, 0.0), he, &frame, half), "in front, not touching");
        assert!(!box_touches_rectangle(Vec3::new(0.0, 0.0, 1.0), he, &frame, half), "on the plane beside the opening");
        assert!(box_touches_rectangle(Vec3::new(0.0, 0.0, 0.75), he, &frame, half), "overlapping the side edge");
    }
}

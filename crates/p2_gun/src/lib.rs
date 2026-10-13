//! Portal gun: shot tracing, portal placement and linking, plus an optional
//! mouse-input plugin (`p2_gun`). Spec: research/specs/portal-placement.md.
//!
//! # Using it
//!
//! Add [`Plugin`] (after `p2_core::CorePlugin`) and tag every collider a
//! portal may sit on with [`p2_core::Portalable`]. Shots need the shooter to
//! carry a [`p2_core::PortalGun`], [`p2_core::Crouch`] and [`p2_core::ViewAngles`]
//! on a `Transform` (feet), which `p2_core::spawn_player` provides.
//! Add [`MouseInputPlugin`] only if you want the built-in mouse buttons;
//! otherwise send [`p2_core::FirePortal`] yourself.
//!
//! Placement is instant on the server side: the portal opens in the same
//! tick as the shot. The visual projectile is `p2_fx`'s job, driven by
//! [`p2_core::GunFired`].
use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use p2_core::{
    Crouch, FirePortal, GunFired, InputLocked, InterpolatedFeet, P2Systems, Portal, PortalClosed, PortalGun, PortalHelperCollider,
    PortalKind, PortalLink, PortalPlaced, PortalShotFailed, Portalable, ShotFailReason, ViewAngles, PORTAL_HALF_SIZE,
};

pub mod rules;
mod world;

use rules::{place, trace_shot, Open, PortalWindow};
use world::PhysicsWorld;

/// Tunables of the portal gun. Defaults are Portal 2's values.
#[derive(Resource, Clone, Debug, PartialEq)]
pub struct GunConfig {
    /// Seconds both buttons stay locked after a shot (`portalgun_fire_delay`, 0.20 s).
    pub fire_lock: f32,
    /// Seconds between re-fires while a button is held
    /// (`portalgun_held_button_fire_fire_delay`, 0.50 s).
    pub held_refire: f32,
    /// Maximum shot length in metres (56755.84 Source units).
    pub max_range: f32,
    /// Half extents of a placed opening in metres (32 x 56 u, see [`PORTAL_HALF_SIZE`]).
    pub half_size: Vec2,
}

impl Default for GunConfig {
    fn default() -> Self {
        Self {
            fire_lock: 0.20,
            held_refire: 0.50,
            max_range: p2_core::u(56755.84),
            half_size: PORTAL_HALF_SIZE,
        }
    }
}

/// Private gun state: when the last shot left. `p2_gun` owns it.
#[derive(Component, Clone, Copy, Debug, Default)]
struct GunTimer {
    last_shot: Option<f32>,
}

/// Seconds since the last shot (infinite before the first one).
fn since_shot(last: Option<f32>, now: f32) -> f32 {
    last.map_or(f32::INFINITY, |t| now - t)
}

/// A shot may leave once the fire lock has passed.
fn may_fire(last: Option<f32>, now: f32, lock: f32) -> bool {
    since_shot(last, now) >= lock
}

/// Core portal gun: shots, placement, linking and the portal lifecycle
/// messages. Needs [`p2_core::CorePlugin`]. Inserts a [`GunConfig`] unless
/// the host already did.
pub struct Plugin;

impl bevy::app::Plugin for Plugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GunConfig>().add_systems(
            Update,
            (attach_timers.before(P2Systems::Input), fire.in_set(P2Systems::Fire)),
        );
    }
}

/// Optional mouse input: the left button fires blue and the right fires
/// orange, while the primary window's cursor is grabbed and
/// [`InputLocked`] is false. Needs [`Plugin`]. Skip it to send
/// [`FirePortal`] from your own input.
pub struct MouseInputPlugin;

impl bevy::app::Plugin for MouseInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, mouse_input.in_set(P2Systems::Input));
    }
}

/// Gives every `PortalGun` a [`GunTimer`] the first frame it exists.
fn attach_timers(mut commands: Commands, guns: Query<Entity, (With<PortalGun>, Without<GunTimer>)>) {
    for e in &guns {
        commands.entity(e).insert(GunTimer::default());
    }
}

/// Left button -> blue, right -> orange. Only while the cursor is grabbed
/// and input is not locked. A press during the fire lock is dropped; a held
/// button re-fires every `held_refire` seconds.
fn mouse_input(
    locked: Res<InputLocked>,
    config: Res<GunConfig>,
    buttons: Res<ButtonInput<MouseButton>>,
    time: Res<Time>,
    cursor: Query<&CursorOptions, With<PrimaryWindow>>,
    guns: Query<(Entity, &PortalGun, &GunTimer)>,
    mut fire: MessageWriter<FirePortal>,
) {
    if locked.0 {
        return;
    }
    let grabbed = cursor.single().is_ok_and(|c| c.grab_mode != CursorGrabMode::None);
    if !grabbed {
        return;
    }
    let now = time.elapsed_secs();
    for (button, kind) in [(MouseButton::Left, PortalKind::Blue), (MouseButton::Right, PortalKind::Orange)] {
        let just = buttons.just_pressed(button);
        let held = buttons.pressed(button);
        for (gun_e, gun, timer) in &guns {
            if !gun.can_fire[kind.index()] {
                continue;
            }
            let since = since_shot(timer.last_shot, now);
            let want = if just { since >= config.fire_lock } else { held && since >= config.held_refire };
            if want {
                fire.write(FirePortal { gun: gun_e, kind });
            }
        }
    }
}

/// The owner of a collider: its rigid body, or the entity itself.
fn owner(e: Entity, body_of: &Query<&ColliderOf>) -> Entity {
    body_of.get(e).map_or(e, |c| c.body)
}

/// Shots: trace from the eye, place the portal, or fail with a reason.
#[allow(clippy::too_many_arguments)]
fn fire(
    mut commands: Commands,
    config: Res<GunConfig>,
    time: Res<Time>,
    fixed: Res<Time<Fixed>>,
    mut reader: MessageReader<FirePortal>,
    mut guns: Query<(&mut PortalGun, &mut GunTimer, &InterpolatedFeet, &Crouch, &ViewAngles)>,
    portals: Query<(Entity, &Portal, &Transform, Option<&PortalLink>)>,
    spatial: SpatialQuery,
    (helpers, sensors, bodies, body_of, portalable_q): (
        Query<(), With<PortalHelperCollider>>,
        Query<(), With<Sensor>>,
        Query<&RigidBody>,
        Query<&ColliderOf>,
        Query<(), With<Portalable>>,
    ),
    (mut fired_w, mut failed_w, mut placed_w, mut closed_w): (
        MessageWriter<GunFired>,
        MessageWriter<PortalShotFailed>,
        MessageWriter<PortalPlaced>,
        MessageWriter<PortalClosed>,
    ),
) {
    let now = time.elapsed_secs();
    let half = config.half_size;
    for shot in reader.read() {
        let kind = shot.kind;
        let player = shot.gun;
        let Ok((mut gun, mut timer, feet, crouch, view)) = guns.get_mut(player) else { continue };
        if !gun.can_fire[kind.index()] || !may_fire(timer.last_shot, now, config.fire_lock) {
            continue;
        }
        timer.last_shot = Some(now);

        // The eye the player sees: interpolated feet (render position) plus the eye height.
        let origin = feet.at(fixed.overstep_fraction()) + Vec3::Y * crouch.eye_height;
        let dir = view.forward();

        // Shots pass through the shooter, helper colliders, sensors and dynamic bodies.
        let skip = |e: Entity| {
            if helpers.contains(e) || sensors.contains(e) {
                return true;
            }
            let body = owner(e, &body_of);
            body == player || matches!(bodies.get(body), Ok(RigidBody::Dynamic))
        };
        // Any collider (or its body) tagged Portalable takes a portal.
        let portalable = |e: Entity| portalable_q.contains(e) || body_of.get(e).is_ok_and(|c| portalable_q.contains(c.body));
        let world = PhysicsWorld {
            spatial: &spatial,
            filter: SpatialQueryFilter::from_excluded_entities([player]),
            skip: &skip,
            portalable: &portalable,
        };

        let windows: Vec<PortalWindow> = portals
            .iter()
            .map(|(e, p, t, link)| PortalWindow {
                entity: e,
                frame: Portal::frame(t),
                half: p.half_size,
                exit: link.and_then(|l| portals.get(l.0).ok()).map(|(_, _, pt, _)| Portal::frame(pt)),
            })
            .collect();
        let trace = trace_shot(&world, origin, dir, config.max_range, &windows);
        fired_w.write(GunFired { gun: player, kind, from: origin, to: trace.end });

        let Some(hit) = trace.hit else {
            failed_w.write(PortalShotFailed { gun: player, kind, reason: ShotFailReason::NoHit, point: None, normal: None });
            continue;
        };

        // Every open portal except the one this shot replaces.
        let replacing = gun.portal(kind);
        let others: Vec<Open> = portals
            .iter()
            .filter(|(e, ..)| Some(*e) != replacing)
            .map(|(_, p, t, _)| Open { frame: Portal::frame(t), half: p.half_size })
            .collect();

        match place(&world, hit, trace.dir, half, &others) {
            Err(reason) => {
                failed_w.write(PortalShotFailed {
                    gun: player,
                    kind,
                    reason,
                    point: Some(hit.point),
                    normal: Some(hit.normal),
                });
            }
            Ok(placed) => {
                // Replace our own same-colour portal (it closes, then the new one opens).
                if let Some(old) = replacing {
                    if let Ok((_, _, old_tf, _)) = portals.get(old) {
                        closed_w.write(PortalClosed { kind, transform: *old_tf });
                    }
                    commands.entity(old).despawn();
                }
                let tf = Transform::from_translation(placed.frame.position).with_rotation(placed.frame.rotation);
                let new = commands
                    .spawn((
                        Name::new(format!("{kind:?} portal")),
                        Portal { kind, half_size: half, surfaces: placed.surfaces, placed_at: now },
                        tf,
                    ))
                    .id();
                gun.portals[kind.index()] = Some(new);
                gun.last_placed = Some(kind);
                // Link both halves when both are open; re-insert on the partner so its link changes.
                if let Some(partner) = gun.portal(kind.other()) {
                    commands.entity(new).insert(PortalLink(partner));
                    commands.entity(partner).insert(PortalLink(new));
                }
                placed_w.write(PortalPlaced { gun: player, portal: new, kind, transform: tf });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fire_lock_is_point_two_seconds() {
        let lock = GunConfig::default().fire_lock;
        assert!(may_fire(None, 0.0, lock), "the first shot is never locked");
        assert!(!may_fire(Some(1.0), 1.19, lock), "a shot 0.19 s ago is still locked");
        assert!(may_fire(Some(1.0), 1.2, lock), "0.2 s after a shot may fire again");
    }

    #[test]
    fn held_refire_is_half_a_second() {
        let refire = GunConfig::default().held_refire;
        assert!(since_shot(Some(1.0), 1.49) < refire);
        assert!(since_shot(Some(1.0), 1.5) >= refire);
    }

    #[test]
    fn default_config_matches_portal_2() {
        let c = GunConfig::default();
        assert!((c.max_range / p2_core::U - 56755.84).abs() < 0.01);
        assert_eq!(c.half_size, PORTAL_HALF_SIZE);
    }
}

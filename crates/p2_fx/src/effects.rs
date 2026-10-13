//! Maps the portal and gun messages of `p2_core` onto particle effects and
//! sounds. Everything here is driven by messages and components only.
//!
//! * a looping rim (`portal_edge` blue / `portal_edge_reverse` orange) while a
//!   [`Portal`] exists, following its transform, with an ambient hum;
//! * `PortalPlaced`: `portal_success` at the new spot, the rim restarted, the open sound;
//! * `PortalClosed`: `portal_close` at the old spot, the close sound;
//! * `PortalShotFailed`: `portal_badsurface` at the hit point and a fizzle sound;
//! * `GunFired`: the fire sound, a first-person muzzle flash, and after
//!   `projectile_delay` a projectile stream from the muzzle to the hit point.
//! * the first-person top light, a looping effect on the viewmodel layer.
use bevy::math::Mat3;
use p2_core::{Crouch, InterpolatedFeet, ViewAngles};
use bevy::prelude::*;
use p2_core::{
    GunFired, P2Systems, Portal, PortalClosed, PortalGun, PortalKind, PortalPlaced, PortalShotFailed, ShotFailReason,
    ViewmodelAttachments, LAYER_VIEWMODEL,
};

use crate::render::ParticleEffect;
use crate::sounds::PlaySound;
use crate::FxSettings;

/// Effects that belong to a placed portal.
#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct PortalFx {
    /// The looping rim, if particles are on.
    pub edge: Option<Entity>,
}

/// Effects that belong to a gun (the first-person top light).
#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct GunFx {
    top_light: Option<Entity>,
    last_kind: PortalKind,
}

/// A shot in flight: the projectile stream starts after a delay and runs to the hit point.
#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct PendingShot {
    kind: PortalKind,
    from: Vec3,
    to: Vec3,
    /// Seconds since the shot.
    elapsed: f32,
    /// The stream effect, once started.
    stream: Option<Entity>,
}

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        (
            on_gun_fired,
            on_portal_placed,
            on_portal_closed,
            on_shot_failed,
            sync_portals,
            drive_shots,
        )
            .chain()
            .after(P2Systems::Fire),
    )
    // The gun's first-person effects follow this frame's gun pose (written in
    // P2Systems::View), and ride along through portals.
    .add_systems(
        PostUpdate,
        (follow_eye, sync_gun_top_light)
            .chain()
            .in_set(P2Systems::Effects)
            .before(crate::render::step_effects),
    )
    .add_observer(portal_removed);
}

/// Marks an effect simulated in the player's eye space (the first-person gun
/// effects): its `render_transform` follows the eye every frame.
#[derive(Component, Clone, Copy, Debug)]
pub struct FollowEye;

/// The eye the viewmodel is drawn from: interpolated feet + eye height, view
/// angles (no view punch; the gun doesn't punch).
fn eye_of(feet: &InterpolatedFeet, crouch: &Crouch, view: &ViewAngles, alpha: f32) -> Transform {
    Transform::from_translation(feet.at(alpha) + Vec3::Y * crouch.eye_height).with_rotation(view.rotation())
}

/// Keeps every eye-space effect at the current eye.
fn follow_eye(
    fixed: Res<Time<Fixed>>,
    players: Query<(&InterpolatedFeet, &Crouch, &ViewAngles), With<p2_core::Player>>,
    mut effects: Query<&mut ParticleEffect, With<FollowEye>>,
) {
    let Some((feet, crouch, view)) = players.iter().next() else { return };
    let eye = eye_of(feet, crouch, view, fixed.overstep_fraction());
    for mut fx in &mut effects {
        fx.render_transform = eye;
    }
}

/// The colour of a portal shot as a 0-255 vector for the colour control points.
fn colour(kind: PortalKind) -> Vec3 {
    kind.rgb255()
}

fn open_sound(kind: PortalKind) -> &'static str {
    match kind {
        PortalKind::Blue => "Portal.open_blue",
        PortalKind::Orange => "Portal.open_red",
    }
}

fn close_sound(kind: PortalKind) -> &'static str {
    match kind {
        PortalKind::Blue => "Portal.close_blue",
        PortalKind::Orange => "Portal.close_red",
    }
}

fn fire_sound(kind: PortalKind) -> &'static str {
    match kind {
        PortalKind::Blue => "Weapon_Portalgun.fire_blue",
        PortalKind::Orange => "Weapon_Portalgun.fire_red",
    }
}

/// The rim system of a colour.
pub fn edge_system(kind: PortalKind) -> &'static str {
    match kind {
        PortalKind::Blue => "portal_edge",
        PortalKind::Orange => "portal_edge_reverse",
    }
}

/// Basis whose Z axis is `normal` (for effects that face a surface).
fn basis_from_normal(normal: Vec3) -> Mat3 {
    Mat3::from_quat(Quat::from_rotation_arc(Vec3::Z, normal.normalize_or(Vec3::Z)))
}

fn spawn_burst(commands: &mut Commands, name: &str, pos: Vec3, basis: Mat3, kind: PortalKind) {
    commands.spawn((
        Name::new(format!("{name} effect")),
        ParticleEffect::new(name).with_cp(0, pos, basis).with_cp_vector(2, colour(kind)),
    ));
}

fn on_gun_fired(
    mut reader: MessageReader<GunFired>,
    settings: Res<FxSettings>,
    mut guns: Query<(Option<&ViewmodelAttachments>, Option<&mut GunFx>)>,
    eyes: Query<(&InterpolatedFeet, &Crouch, &ViewAngles)>,
    fixed: Res<Time<Fixed>>,
    mut sounds: MessageWriter<PlaySound>,
    mut commands: Commands,
) {
    for shot in reader.read() {
        sounds.write(PlaySound::at(fire_sound(shot.kind), shot.from));
        let muzzle = guns.get(shot.gun).ok().and_then(|(a, _)| a.and_then(|a| a.muzzle));
        if let Ok((_, Some(mut gfx))) = guns.get_mut(shot.gun) {
            gfx.last_kind = shot.kind;
        }
        if !settings.particles {
            continue;
        }
        if settings.muzzle_flash {
            if let (Some(m), Ok((feet, crouch, view))) = (muzzle, eyes.get(shot.gun)) {
                // Eye space, like the top light: the flash stays on the gun.
                let eye = eye_of(feet, crouch, view, fixed.overstep_fraction());
                let inv = eye.compute_affine().inverse();
                let mut fx = ParticleEffect::new("portalgun_muzzleflash_FP")
                    .with_cp(0, inv.transform_point3(m.translation), Mat3::from_quat(eye.rotation.inverse() * m.rotation))
                    .with_cp_vector(1, colour(shot.kind));
                fx.render_layer = Some(LAYER_VIEWMODEL);
                fx.render_transform = eye;
                commands.spawn((Name::new("muzzle flash (first person)"), fx, FollowEye));
            }
        }
        commands.spawn((
            Name::new("projectile"),
            PendingShot { kind: shot.kind, from: muzzle.map(|m| m.translation).unwrap_or(shot.from), to: shot.to, elapsed: 0.0, stream: None },
        ));
    }
}

fn on_portal_placed(
    mut reader: MessageReader<PortalPlaced>,
    settings: Res<FxSettings>,
    mut portals: Query<&mut PortalFx>,
    mut sounds: MessageWriter<PlaySound>,
    mut commands: Commands,
    mut effects: Query<&mut ParticleEffect>,
) {
    for placed in reader.read() {
        let (kind, transform) = (placed.kind, placed.transform);
        sounds.write(PlaySound::at(open_sound(kind), transform.translation));
        if !settings.particles {
            continue;
        }
        let basis = Mat3::from_quat(transform.rotation);
        spawn_burst(&mut commands, "portal_success", transform.translation, basis, kind);
        // A moved or re-placed portal restarts its rim.
        if let Ok(mut fx) = portals.get_mut(placed.portal) {
            if let Some(old) = fx.edge.take() {
                if let Ok(mut e) = effects.get_mut(old) {
                    e.stop();
                }
            }
            fx.edge = Some(spawn_rim(&mut commands, kind, &transform, basis));
        }
    }
}

fn spawn_rim(commands: &mut Commands, kind: PortalKind, tf: &Transform, basis: Mat3) -> Entity {
    // Only CP0 (the portal) and CP7 (colour) come from the game, as in Portal 2:
    // the system's own operators place CP1 25 u in front of the portal and
    // tumble it, and orbit CP2-CP5 around it. The rim ribbons face those
    // points, which tilts them up out of the wall into the 3D crown.
    let mut fx = ParticleEffect::new(edge_system(kind))
        .with_cp(0, tf.translation, basis)
        .with_cp_vector(7, colour(kind));
    fx.restart_when_done = true;
    fx.despawn_when_done = false;
    commands.spawn((Name::new("portal rim"), fx)).id()
}

fn on_portal_closed(mut reader: MessageReader<PortalClosed>, settings: Res<FxSettings>, mut sounds: MessageWriter<PlaySound>, mut commands: Commands) {
    for closed in reader.read() {
        sounds.write(PlaySound::at(close_sound(closed.kind), closed.transform.translation));
        if settings.particles {
            spawn_burst(
                &mut commands,
                "portal_close",
                closed.transform.translation,
                Mat3::from_quat(closed.transform.rotation),
                closed.kind,
            );
        }
    }
}

fn on_shot_failed(mut reader: MessageReader<PortalShotFailed>, settings: Res<FxSettings>, mut sounds: MessageWriter<PlaySound>, mut commands: Commands) {
    for fail in reader.read() {
        let Some(point) = fail.point else { continue };
        let normal = fail.normal.unwrap_or(Vec3::Z);
        let sound = match fail.reason {
            ShotFailReason::DoesNotFit => Some("Portal.fizzle_invalid_surface"),
            ShotFailReason::NotPortalable | ShotFailReason::Overlap => Some("Portal.fizzle_moved"),
            ShotFailReason::NoHit => None,
        };
        if let Some(s) = sound {
            sounds.write(PlaySound::at(s, point));
        }
        if settings.particles {
            spawn_burst(&mut commands, "portal_badsurface", point, basis_from_normal(normal), fail.kind);
        }
    }
}

/// Gives every portal a rim and a looping hum, and keeps the rim on its transform.
fn sync_portals(
    settings: Res<FxSettings>,
    added: Query<(Entity, &Portal, &Transform), Without<PortalFx>>,
    mut all: Query<(&Portal, &Transform, &PortalFx)>,
    mut effects: Query<&mut ParticleEffect>,
    mut sounds: MessageWriter<PlaySound>,
    mut commands: Commands,
) {
    for (e, portal, tf) in &added {
        let edge = settings.particles.then(|| spawn_rim(&mut commands, portal.kind, tf, Mat3::from_quat(tf.rotation)));
        commands.entity(e).insert(PortalFx { edge });
        sounds.write(PlaySound::looping_on("Portal.ambient_loop", e));
    }
    for (portal, tf, fx) in &mut all {
        let Some(edge) = fx.edge else { continue };
        if let Ok(mut eff) = effects.get_mut(edge) {
            let basis = Mat3::from_quat(tf.rotation);
            eff.set_cp(0, tf.translation, basis);
            eff.set_cp_vector(7, colour(portal.kind));
        }
    }
}

/// Removing a portal stops its rim (the end-cap plays out) and leaves the hum to go with the entity.
fn portal_removed(remove: On<Remove, Portal>, fx: Query<&PortalFx>, mut effects: Query<&mut ParticleEffect>) {
    if let Some(edge) = fx.get(remove.entity).ok().and_then(|f| f.edge) {
        if let Ok(mut e) = effects.get_mut(edge) {
            e.stop();
        }
    }
}

/// Moves each shot's stream along its path after the delay, and stops it at the hit point.
fn drive_shots(
    time: Res<Time>,
    settings: Res<FxSettings>,
    mut shots: Query<(Entity, &mut PendingShot)>,
    mut effects: Query<&mut ParticleEffect>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    for (e, mut shot) in &mut shots {
        shot.elapsed += dt;
        let delay = settings.projectile_delay;
        let delta = shot.to - shot.from;
        let flight = settings.projectile_flight.max(1e-3);
        if shot.elapsed < delay {
            continue;
        }
        let t = ((shot.elapsed - delay) / flight).clamp(0.0, 1.0);
        let head = shot.from.lerp(shot.to, t);
        let basis = basis_from_normal(delta);
        match shot.stream {
            None => {
                let id = commands
                    .spawn((
                        Name::new("projectile stream"),
                        ParticleEffect::new("portal_projectile_stream").with_cp(0, head, basis).with_cp_vector(2, colour(shot.kind)),
                    ))
                    .id();
                shot.stream = Some(id);
            }
            Some(id) => {
                if let Ok(mut fx) = effects.get_mut(id) {
                    fx.set_cp(0, head, basis);
                    if t >= 1.0 {
                        fx.stop();
                    }
                }
            }
        }
        if t >= 1.0 {
            commands.entity(e).despawn();
        }
    }
}

/// Spawns the first-person top light on each gun and places it on the viewmodel pose.
fn sync_gun_top_light(
    settings: Res<FxSettings>,
    new_guns: Query<Entity, (With<PortalGun>, Without<GunFx>)>,
    fixed: Res<Time<Fixed>>,
    mut guns: Query<(&ViewmodelAttachments, &GunFx, &InterpolatedFeet, &Crouch, &ViewAngles)>,
    mut effects: Query<&mut ParticleEffect>,
    mut commands: Commands,
) {
    for gun in &new_guns {
        let top_light = (settings.particles && settings.top_light).then(|| {
            let mut fx = ParticleEffect::new("portalgun_top_light_firstperson");
            fx.render_layer = Some(LAYER_VIEWMODEL);
            fx.restart_when_done = true;
            fx.despawn_when_done = false;
            commands.spawn((Name::new("top light (first person)"), fx, FollowEye)).id()
        });
        commands.entity(gun).insert(GunFx { top_light, last_kind: PortalKind::Blue });
    }
    for (att, gfx, feet, crouch, view) in &mut guns {
        let Some(e) = gfx.top_light else { continue };
        let Ok(mut fx) = effects.get_mut(e) else { continue };
        match att.top_light {
            Some(pose) => {
                fx.visible = true;
                // Eye space: the attachment relative to the eye.
                let eye = eye_of(feet, crouch, view, fixed.overstep_fraction());
                let inv = eye.compute_affine().inverse();
                let local = |t: &Transform| (inv.transform_point3(t.translation), Mat3::from_quat(eye.rotation.inverse() * t.rotation));
                let (p, r) = local(&pose);
                fx.set_cp(0, p, r);
                fx.set_cp_vector(1, colour(gfx.last_kind));
                // CP3/CP4: the beam points (the core glow emits at and locks to CP3).
                for (i, t) in [(3, att.beam_point1), (4, att.beam_point5)] {
                    if let Some(t) = t {
                        let (p, r) = local(&t);
                        fx.set_cp(i, p, r);
                    }
                }
            }
            None => fx.visible = false,
        }
    }
}


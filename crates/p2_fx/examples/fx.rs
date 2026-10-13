//! Scripted run of the portal effects in the test chamber.
//!
//! Spawns a blue and an orange portal on the white east wall, sends the
//! portal messages on fixed frames (60 Hz virtual clock), and moves the camera
//! for each shot. Logs the particle counts per system.
//!
//! ```text
//! cargo run -p p2_fx --example fx -- \
//!   --size 960x540 --shot 50:DIR/blue_1m.png --shot 110:DIR/blue_3m.png \
//!   --shot 170:DIR/orange_1m.png --shot 200:DIR/badsurface.png \
//!   --shot 251:DIR/projectile.png --shot 312:DIR/close.png
//! ```
use std::f32::consts::FRAC_PI_2;
use std::time::Duration;

use avian3d::prelude::*;
use bevy::camera::visibility::RenderLayers;
use bevy::camera::{Camera, ClearColorConfig};
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use p2_assets::AssetsPlugin;
use p2_core::{
    spawn_player, GunFired, MainCamera, P2Settings, Portal, PortalClosed, PortalGun, PortalKind, PortalPlaced,
    PortalShotFailed, ShotFailReason, ViewmodelAttachments, LAYER_VIEWMODEL, PORTAL_HALF_SIZE,
};
use p2_fx::{ParticleStats, LAYER_PORTAL_MAIN_ONLY};
use p2_room::{HarnessArgs, HarnessPlugin, RoomPlugin};

/// East wall inner face (Source x = 512 u), world metres.
const EAST: f32 = p2_core::u(512.0);
const EYE: f32 = 1.6;

/// Marks the viewmodel camera (layer 26 only).
#[derive(Component)]
struct ViewCam;

#[derive(Resource)]
struct Rig {
    blue: Entity,
    orange: Entity,
    gun: Entity,
}

/// Wall frame on the east wall at `z`: +Z points out of the wall, into the room (-X).
fn wall_frame(z: f32) -> Transform {
    Transform::from_xyz(EAST, EYE, z).with_rotation(Quat::from_rotation_y(-FRAC_PI_2))
}

/// Camera pose for a frame of the script (the segments of the shot list).
fn pose(frame: u32) -> Transform {
    let look_east = Quat::from_rotation_y(-FRAC_PI_2);
    let at = |x: f32, y: f32, z: f32| Transform::from_xyz(x, y, z).with_rotation(look_east);
    match frame {
        0..=59 => at(EAST - 1.0, EYE, -2.0),
        60..=119 => at(EAST - 3.0, EYE, -2.0),
        120..=179 => at(EAST - 1.0, EYE, 2.0),
        180..=239 => at(EAST - 3.0, 1.4, 5.0),
        240..=299 => at(-6.0, EYE, 0.0),
        _ => at(EAST - 1.0, EYE, -2.0),
    }
}

fn setup(mut commands: Commands) {
    let blue = commands
        .spawn((Name::new("blue portal"), Portal { kind: PortalKind::Blue, half_size: PORTAL_HALF_SIZE, surfaces: vec![], placed_at: 0.0 }, wall_frame(-2.0)))
        .id();
    let orange = commands
        .spawn((Name::new("orange portal"), Portal { kind: PortalKind::Orange, half_size: PORTAL_HALF_SIZE, surfaces: vec![], placed_at: 0.0 }, wall_frame(2.0)))
        .id();
    let gun = spawn_player(&mut commands, Vec3::ZERO, 0.0);
    commands.insert_resource(Rig { blue, orange, gun });

    // Main view: world and portal rims (layer 25). Viewmodel camera draws on top.
    commands.spawn((
        Name::new("main camera"),
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection { fov: 59f32.to_radians(), ..default() }),
        RenderLayers::from_layers(&[0, LAYER_PORTAL_MAIN_ONLY]),
        MainCamera,
        pose(0),
    ));
    commands.spawn((
        Name::new("viewmodel camera"),
        Camera3d::default(),
        Camera { order: 1, clear_color: ClearColorConfig::None, ..default() },
        Projection::Perspective(PerspectiveProjection { fov: 59f32.to_radians(), ..default() }),
        RenderLayers::layer(LAYER_VIEWMODEL),
        ViewCam,
        pose(0),
    ));
}

/// `--cam X Y Z --target X Y Z` (metres): a fixed camera instead of the script's.
#[derive(Resource, Clone, Copy)]
struct FixedCam(Option<Transform>);

/// Moves both cameras along the script.
fn move_camera(fixed: Res<FixedCam>, mut frame: Local<u32>, mut cams: Query<&mut Transform, Or<(With<MainCamera>, With<ViewCam>)>>) {
    let t = fixed.0.unwrap_or_else(|| pose(*frame));
    for mut c in &mut cams {
        *c = t;
    }
    *frame += 1;
}

/// Puts the first-person gun attachments in front of the camera.
fn attach_viewmodel(cam: Query<&Transform, With<MainCamera>>, mut gun: Query<&mut ViewmodelAttachments, With<PortalGun>>) {
    let Ok(cam) = cam.single() else { return };
    let (f, r, u) = (cam.rotation * Vec3::NEG_Z, cam.rotation * Vec3::X, cam.rotation * Vec3::Y);
    for mut att in &mut gun {
        att.muzzle = Some(Transform::from_translation(cam.translation + f * 0.4 - u * 0.12 + r * 0.1).with_rotation(cam.rotation));
        att.top_light = Some(Transform::from_translation(cam.translation + f * 0.35 + u * 0.05 + r * 0.15).with_rotation(cam.rotation));
    }
}

/// Sends the scripted portal messages on fixed frames.
fn script(
    mut frame: Local<u32>,
    rig: Res<Rig>,
    mut placed: MessageWriter<PortalPlaced>,
    mut closed: MessageWriter<PortalClosed>,
    mut failed: MessageWriter<PortalShotFailed>,
    mut fired: MessageWriter<GunFired>,
) {
    *frame += 1;
    match *frame {
        2 => {
            placed.write(PortalPlaced { gun: rig.gun, portal: rig.blue, kind: PortalKind::Blue, transform: wall_frame(-2.0) });
            placed.write(PortalPlaced { gun: rig.gun, portal: rig.orange, kind: PortalKind::Orange, transform: wall_frame(2.0) });
        }
        190 => {
            failed.write(PortalShotFailed {
                gun: rig.gun,
                kind: PortalKind::Blue,
                reason: ShotFailReason::DoesNotFit,
                point: Some(Vec3::new(EAST, 1.0, 5.0)),
                normal: Some(Vec3::NEG_X),
            });
        }
        240 => {
            let eye = Vec3::new(-6.0, EYE, 0.0);
            fired.write(GunFired { gun: rig.gun, kind: PortalKind::Blue, from: eye, to: Vec3::new(EAST, EYE, 0.0) });
        }
        300 => {
            closed.write(PortalClosed { kind: PortalKind::Blue, transform: wall_frame(-2.0) });
        }
        _ => {}
    }
}

/// Prints the particle counts at the shot frames, to show which systems ran.
fn log_stats(mut frame: Local<u32>, stats: Res<ParticleStats>) {
    *frame += 1;
    if matches!(*frame, 50 | 170 | 251 | 312) {
        let mut emitted: Vec<_> = stats.emitted.iter().collect();
        emitted.sort();
        println!("frame {}: emitted so far {:?}", *frame, emitted);
        let mut alive: Vec<_> = stats.alive.iter().filter(|(_, n)| **n > 0).collect();
        alive.sort();
        println!("frame {}: alive now {:?}", *frame, alive);
    }
}

fn main() -> AppExit {
    let args = HarnessArgs::from_env();
    let vec = |flag: &str| {
        let i = args.rest.iter().position(|a| a == flag)?;
        let f = |k: usize| args.rest.get(i + k).and_then(|v| v.parse::<f32>().ok());
        Some(Vec3::new(f(1)?, f(2)?, f(3)?))
    };
    let fixed = FixedCam(vec("--cam").zip(vec("--target")).map(|(c, t)| Transform::from_translation(c).looking_at(t, Vec3::Y)));
    App::new()
        .insert_resource(fixed)
        .add_plugins(DefaultPlugins.set(args.window_plugin("p2_fx")))
        .insert_resource(args)
        // 60 Hz virtual clock so the scripted frames line up with the effect timings.
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(1.0 / 60.0)))
        .insert_resource(P2Settings { set_fixed_timestep: true })
        .add_plugins((
            AssetsPlugin,
            PhysicsPlugins::default(),
            p2_core::CorePlugin,
            RoomPlugin,
            p2_fx::Plugin,
            HarnessPlugin,
        ))
        .add_systems(Startup, setup)
        .add_systems(Update, (move_camera, attach_viewmodel, script, log_stats))
        .run()
}

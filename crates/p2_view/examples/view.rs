//! First-person camera, portal gun viewmodel and crosshair in the test chamber.
//!
//! cargo run -p p2_view --example view -- [--look YAW PITCH] [--fire blue|orange] [--fire-frame N]
//!     [--roll DEG] [--screenshot out.png --frames N] [--shot F:path] [--exit-after N] [--size WxH]
//!
//! * `--look YAW PITCH`: view angles in degrees (yaw 0 looks down -Z).
//! * `--fire blue|orange`: writes a `GunFired` message at frame N (default 30),
//!   so the fire sequence can be screenshotted.
//! * `--roll DEG`: starts the view rolled by DEG and logs the roll every frame
//!   against the 0.125 s half-life, then prints PASS/FAIL for the half-life check.
use avian3d::prelude::*;
use bevy::prelude::*;
use p2_assets::AssetsPlugin;
use p2_core::{Crouch, GunFired, InterpolatedFeet, PortalKind, Player, ViewAngles};
use p2_room::{HarnessArgs, HarnessPlugin, RoomPlugin};

/// Roll recovery half-life, seconds (portal-transit.md §1.7).
const ROLL_HALF_LIFE: f32 = 0.125;

#[derive(Resource, Default)]
struct Demo {
    player: Option<Entity>,
    look: (f32, f32),
    fire: Option<PortalKind>,
    fire_frame: u32,
    roll_deg: Option<f32>,
    frame: u32,
    /// Seconds since the roll was set.
    roll_t: f32,
    roll_started: bool,
    roll_failed: bool,
    roll_checked: bool,
    roll_done: bool,
}

fn main() -> AppExit {
    let args = HarnessArgs::from_env();
    let mut demo = Demo { fire_frame: 30, ..default() };
    let mut it = args.rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--look" => {
                let yaw = it.next().and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
                let pitch = it.next().and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
                demo.look = (yaw.to_radians(), pitch.to_radians());
            }
            "--fire" => {
                demo.fire = match it.next().map(String::as_str) {
                    Some("orange") => Some(PortalKind::Orange),
                    _ => Some(PortalKind::Blue),
                };
            }
            "--fire-frame" => demo.fire_frame = it.next().and_then(|v| v.parse().ok()).unwrap_or(30),
            "--roll" => demo.roll_deg = it.next().and_then(|v| v.parse().ok()),
            _ => {}
        }
    }

    App::new()
        .add_plugins(DefaultPlugins.set(args.window_plugin("p2_view")))
        .insert_resource(args)
        .insert_resource(demo)
        .insert_resource(p2_core::P2Settings { set_fixed_timestep: true })
        .add_plugins((
            PhysicsPlugins::default(),
            p2_core::CorePlugin,
            AssetsPlugin,
            RoomPlugin,
            HarnessPlugin,
            p2_view::Plugin,
        ))
        .add_systems(Startup, spawn_demo_player)
        .add_systems(Update, drive_demo)
        .add_systems(PostUpdate, check_roll.after(p2_core::P2Systems::View))
        .run()
}

fn spawn_demo_player(mut commands: Commands, mut demo: ResMut<Demo>) {
    // Feet at (0, 0, 6): clear of the pit (x -9..-3, z 3..9) and the pillar.
    let e = p2_core::spawn_player(&mut commands, Vec3::new(0.0, 0.0, 6.0), demo.look.0);
    commands.entity(e).insert(ViewAngles { yaw: demo.look.0, pitch: demo.look.1, roll: 0.0 });
    demo.player = Some(e);
}

fn drive_demo(
    mut demo: ResMut<Demo>,
    mut players: Query<(&InterpolatedFeet, &mut ViewAngles, &Crouch), With<Player>>,
    mut fired: MessageWriter<GunFired>,
) {
    demo.frame += 1;
    let Some(gun) = demo.player else { return };
    if demo.frame == 2 {
        if let Some(deg) = demo.roll_deg {
            if let Ok((_, mut a, _)) = players.get_mut(gun) {
                a.roll = deg.to_radians();
            }
            println!("roll test: start roll {deg:.2} deg, half-life {ROLL_HALF_LIFE} s");
            demo.roll_started = true;
        }
    }
    if demo.fire_frame == demo.frame {
        if let (Some(kind), Ok((feet, angles, crouch))) = (demo.fire, players.get(gun)) {
            let eye = feet.curr + Vec3::Y * crouch.eye_height;
            let to = eye + angles.forward() * 12.0;
            fired.write(GunFired { gun, kind, from: eye, to });
            println!("fire: {kind:?} at frame {}", demo.frame);
        }
    }
}

/// Numeric check of roll recovery: after one half-life the roll must be half
/// its start value (within a frame's worth of the 10 deg/s floor).
/// Runs after the view system in PostUpdate, so the roll shown already has this frame's decay.
fn check_roll(mut demo: ResMut<Demo>, time: Res<Time>, players: Query<&ViewAngles, With<Player>>) {
    let (Some(start), false) = (demo.roll_deg, demo.roll_done) else { return };
    let Some(angles) = demo.player.and_then(|e| players.get(e).ok()) else { return };
    if !demo.roll_started {
        return;
    }
    demo.roll_t += time.delta_secs();
    let roll = angles.roll.to_degrees();
    let expected = start * 0.5f32.powf(demo.roll_t / ROLL_HALF_LIFE);
    println!("roll t={:.4}s roll={roll:+.3} deg expected(half-life)={expected:+.3} deg", demo.roll_t);
    // Above the 10 deg/s floor (about 2 deg) the roll must follow the half-life curve.
    if roll.abs() > 2.0 && (roll - expected).abs() > 0.2 {
        demo.roll_failed = true;
    }
    if !demo.roll_checked && demo.roll_t >= ROLL_HALF_LIFE {
        demo.roll_checked = true;
        println!(
            "roll half-life check: {:.3} deg at t={:.4} s (curve {:.3} deg, start/2 at 0.125 s)",
            roll, demo.roll_t, expected
        );
    }
    if roll.abs() < 0.11 || demo.roll_t > 2.0 {
        demo.roll_done = true;
        println!("roll settled at t={:.4}s", demo.roll_t);
        println!(
            "roll half-life check: {}",
            if demo.roll_failed { "FAIL" } else { "PASS (within 0.2 deg of the curve above 2 deg)" }
        );
    }
}

//! Movement playground and numeric checks for the Portal 2 controller.
//!
//! ```text
//! cargo run -p p2_movement --example playground                          # play (click to grab, Esc releases)
//! cargo run -p p2_movement --example playground -- --script all          # headless-ish checks: PASS/FAIL per check
//! cargo run -p p2_movement --example playground -- --script jump --screenshot out.png
//! ```
//! Scripts: `ground stop jump crouch crouch_jump step fall force` (or `all`).
//! Scripted runs lock the input, step exactly one 60 Hz tick per frame, and
//! exit with a non-zero status if any check fails.
//!
//! Keys (play): WASD, mouse, Space jump, Ctrl duck, E use (unused here).
use std::f32::consts::PI;
use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy::transform::TransformSystems;
use p2_core::{
    spawn_player, CharacterVelocity, Crouch, ForceDuck, Grounded, InputLocked, InterpolatedFeet, MoveInput,
    P2Settings, P2Systems, Player, ViewAngles, U, u,
};
use p2_movement::{InputPlugin, Plugin as MovementPlugin};
use p2_room::{HarnessArgs, HarnessPlugin, RoomPlugin, RoomSpots};

/// The script checks, in run order.
const ALL: [&str; 9] = ["ground", "stop", "jump", "crouch", "crouch_jump", "step", "fall", "force", "push"];

/// The dynamic box the `push` script walks into.
#[derive(Component)]
struct PushBox;

#[derive(Component)]
struct PlayCamera;

#[derive(Resource)]
struct Script {
    order: Vec<&'static str>,
    idx: usize,
    t: u32,
    scripted: bool,
    max_speed: f32,
    start: Vec3,
    apex: f32,
    min_vy: f32,
    air_ducked: bool,
    /// (tick, eye in u, ducked, on ground) sampled at the start of each tick.
    samples: Vec<(u32, f32, bool, bool)>,
    results: Vec<(String, bool, String)>,
}

/// A check's verdict and the numbers behind it.
fn verdict(s: &mut Script, name: &str, pass: bool, detail: String) {
    println!("{} {name}: {detail}", if pass { "PASS" } else { "FAIL" });
    s.results.push((name.to_string(), pass, detail));
    s.idx += 1;
    s.t = 0;
    s.samples.clear();
    s.max_speed = 0.0;
    s.apex = f32::MIN;
    s.min_vy = 0.0;
    s.air_ducked = false;
}

fn main() {
    let harness = HarnessArgs::from_env();
    let mut script_arg: Option<String> = None;
    let mut it = harness.rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--script" => script_arg = it.next().cloned(),
            other => eprintln!("ignoring argument {other}"),
        }
    }
    let order: Vec<&'static str> = match script_arg.as_deref() {
        None => Vec::new(),
        Some("all") => ALL.to_vec(),
        Some(name) => match ALL.iter().find(|n| **n == name) {
            Some(n) => vec![*n],
            None => {
                eprintln!("unknown script '{name}'; use one of {ALL:?} or all");
                std::process::exit(2);
            }
        },
    };
    let scripted = !order.is_empty();

    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(harness.window_plugin("p2_movement playground")))
        .insert_resource(harness.clone())
        .insert_resource(P2Settings { set_fixed_timestep: true })
        .add_plugins((PhysicsPlugins::default(), p2_core::CorePlugin, RoomPlugin, HarnessPlugin, MovementPlugin))
        .add_plugins(InputPlugin)
        .insert_resource(Script {
            order,
            idx: 0,
            t: 0,
            scripted,
            max_speed: 0.0,
            start: Vec3::ZERO,
            apex: f32::MIN,
            min_vy: 0.0,
            air_ducked: false,
            samples: Vec::new(),
            results: Vec::new(),
        })
        .add_systems(Startup, (setup, spawn_test_geometry))
        .add_systems(FixedUpdate, run_script.before(P2Systems::Move))
        .add_systems(PostUpdate, place_camera.before(TransformSystems::Propagate));
    if scripted {
        app.insert_resource(InputLocked(true));
        // Deterministic: exactly one 60 Hz tick per frame.
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(1.0 / 60.0)));
    }
    if let AppExit::Error(code) = app.run() {
        std::process::exit(i32::from(code.get()));
    }
}

fn setup(mut commands: Commands, spots: Res<RoomSpots>) {
    spawn_player(&mut commands, spots.spawn, spots.spawn_yaw);
    commands.spawn((PlayCamera, Camera3d::default(), Transform::default()));
}

/// A point in Source units (y up) to world metres.
fn pt(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(u(x), u(y), u(z))
}

/// Extra test geometry for the step and fall checks.
fn spawn_test_geometry(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut mats: ResMut<Assets<StandardMaterial>>) {
    let mat = mats.add(StandardMaterial { base_color: Color::srgb(0.9, 0.5, 0.2), ..default() });
    // (min, max) corners, Source units converted to metres. Placed in an open
    // strip of the chamber (x -400..-340 u) clear of the room's walls and markers.
    let boxes = [
        // Platform, top 18 u: the step-up check climbs onto it.
        (pt(-400.0, 0.0, -340.0), pt(-340.0, 18.0, -300.0)),
        // Wall, top 40 u, right after the platform: 22 u above the platform, too high to climb.
        (pt(-400.0, 0.0, -300.0), pt(-340.0, 40.0, -292.0)),
        // A 3 m high block for the fall check.
        (pt(-400.0, 0.0, -160.0), Vec3::new(u(-340.0), 3.0, u(-100.0))),
    ];
    for (min, max) in boxes {
        let size = max - min;
        commands.spawn((
            Name::new("check step"),
            RigidBody::Static,
            Collider::cuboid(size.x, size.y, size.z),
            Mesh3d(meshes.add(Cuboid::new(size.x, size.y, size.z))),
            MeshMaterial3d(mat.clone()),
            Transform::from_translation((min + max) * 0.5),
        ));
    }
}

fn place_camera(
    fixed: Res<Time<Fixed>>,
    players: Query<(&InterpolatedFeet, &Crouch, &ViewAngles), With<Player>>,
    mut cams: Query<&mut Transform, With<PlayCamera>>,
) {
    let (Ok((feet, crouch, view)), Ok(mut cam)) = (players.single(), cams.single_mut()) else { return };
    cam.translation = feet.at(fixed.overstep_fraction()) + Vec3::Y * crouch.eye_height;
    cam.rotation = view.rotation();
}

/// Put the player at `feet` facing `yaw`, at rest, with no input.
fn teleport(feet: Vec3, yaw: f32, tf: &mut Transform, vel: &mut CharacterVelocity, view: &mut ViewAngles, interp: &mut InterpolatedFeet, input: &mut MoveInput) {
    tf.translation = feet;
    tf.rotation = Quat::from_rotation_y(yaw);
    vel.0 = Vec3::ZERO;
    *view = ViewAngles { yaw, pitch: 0.0, roll: 0.0 };
    *interp = InterpolatedFeet { prev: feet, curr: feet };
    *input = MoveInput::default();
}

#[allow(clippy::too_many_arguments)]
fn run_script(
    mut s: ResMut<Script>,
    mut forced: MessageWriter<ForceDuck>,
    mut exit: MessageWriter<AppExit>,
    mut commands: Commands,
    boxes: Query<&Transform, (With<PushBox>, Without<Player>)>,
    mut players: Query<
        (
            Entity,
            &mut Transform,
            &mut CharacterVelocity,
            &mut ViewAngles,
            &mut MoveInput,
            &mut InterpolatedFeet,
            &Crouch,
            &Grounded,
        ),
        With<Player>,
    >,
) {
    if !s.scripted {
        return;
    }
    let Ok((entity, mut tf, mut vel, mut view, mut input, mut interp, crouch, grounded)) = players.single_mut() else {
        return;
    };
    if s.idx >= s.order.len() {
        return;
    }
    let name = s.order[s.idx];
    let feet = tf.translation;
    let speed = Vec2::new(vel.x, vel.z).length() / U;
    let feet_u = feet.y / U;
    let t = s.t;
    let idx_before = s.idx;

    if t == 0 {
        s.start = feet;
        match name {
            "stop" => {} // continues from the ground check
            "ground" => teleport(pt(-250.0, 0.0, -480.0), PI, &mut tf, &mut vel, &mut view, &mut interp, &mut input),
            "step" => teleport(pt(-370.0, 0.0, -360.0), PI, &mut tf, &mut vel, &mut view, &mut interp, &mut input),
            "fall" => teleport(Vec3::new(u(-370.0), 3.0, u(-130.0)), PI, &mut tf, &mut vel, &mut view, &mut interp, &mut input),
            _ => teleport(pt(-250.0, 0.0, -100.0), PI, &mut tf, &mut vel, &mut view, &mut interp, &mut input),
        }
        if name == "force" {
            forced.write(ForceDuck { entity });
        }
        if name == "push" {
            // A 0.9 m dynamic box 1.2 m ahead (+Z at yaw pi), resting on the floor.
            commands.spawn((
                PushBox,
                RigidBody::Dynamic,
                Collider::cuboid(0.9, 0.9, 0.9),
                Transform::from_translation(tf.translation + Vec3::new(0.0, 0.45, 1.2)),
            ));
        }
        // Teleport already applied this tick: re-read the state the move will see.
        s.start = tf.translation;
    }

    let eye_u = crouch.eye_height / U;
    s.samples.push((t, eye_u, crouch.ducked, grounded.on_ground));

    match name {
        "ground" => {
            input.wish = Vec2::Y; // walk forward: +Z at yaw pi
            s.max_speed = s.max_speed.max(speed);
            if t == 150 {
                let ok = (s.max_speed - 175.0).abs() <= 0.5;
                let d = format!("max ground speed {:.2} u/s (spec 175)", s.max_speed);
                verdict(&mut s, name, ok, d);
            }
        }
        "stop" => {
            if t == 0 {
                input.wish = Vec2::ZERO;
            }
            if t > 0 && speed < 0.1 {
                let d = Vec2::new(feet.x - s.start.x, feet.z - s.start.z).length() / U;
                let ok = (d - 31.25).abs() <= 2.5;
                verdict(&mut s, name, ok, format!("stop distance {d:.2} u from 175 u/s (continuous analytic 31.25 u; 60 Hz discrete ~29 u), stopped after {t} ticks"));
            } else if t > 240 {
                verdict(&mut s, name, false, format!("did not stop within 240 ticks (speed {speed:.2} u/s)"));
            }
        }
        "jump" => {
            if t == 20 {
                input.jump = true;
            }
            if t >= 20 {
                s.apex = s.apex.max(feet_u);
            }
            if t == 100 {
                let landed = feet_u < 0.5;
                let ok = (s.apex - 45.0).abs() <= 1.0 && landed;
                let d = format!("jump apex {:.2} u (spec 45), landed {landed}", s.apex);
                verdict(&mut s, name, ok, d);
            }
        }
        "crouch" => {
            // Hold duck for 30 ticks, then release and watch the eye stand up.
            input.duck = t < 30;
            if t == 60 {
                let at = |k: u32| s.samples.iter().find(|x| x.0 == k).copied().unwrap();
                let (e12, e24, e42) = (at(12).1, at(24).1, at(42).1);
                let first_ducked = s.samples.iter().find(|x| x.2).map(|x| x.0).unwrap_or(999);
                let ok = (e12 - 46.0).abs() <= 1.0
                    && (e24 - 28.0).abs() <= 0.5
                    && (24..=26).contains(&first_ducked)
                    && (e42 - 64.0).abs() <= 0.5
                    && !at(42).2;
                verdict(
                    &mut s,
                    name,
                    ok,
                    format!(
                        "eye {e12:.2} u at 0.2 s (46), {e24:.2} u at 0.4 s (28), hull ducked at tick {first_ducked} (24-26), eye {e42:.2} u after unduck (64)"
                    ),
                );
            }
        }
        "crouch_jump" => {
            // Jump from a stand; hold duck in the air. No crouch-jump: the hull must not duck in the air.
            if t == 1 {
                input.jump = true;
            }
            if t >= 3 {
                input.duck = true;
            }
            if t >= 1 {
                s.apex = s.apex.max(feet_u);
                if !grounded.on_ground && crouch.ducked {
                    s.air_ducked = true;
                }
            }
            if t == 90 {
                let ok = !s.air_ducked && (s.apex - 45.0).abs() <= 1.0;
                let d = format!("apex {:.2} u (45), ducked in the air: {}", s.apex, s.air_ducked);
                verdict(&mut s, name, ok, d);
            }
        }
        "step" => {
            input.wish = Vec2::Y; // forward = +Z at yaw pi
            if t == 150 {
                let z = feet.z / U;
                let ok = (feet_u - 18.0).abs() <= 1.0 && (-325.0..=-310.0).contains(&z);
                verdict(
                    &mut s,
                    name,
                    ok,
                    format!("on the 18 u platform at {feet_u:.2} u (18), stopped by the 40 u wall at z {z:.1} u (about -316)"),
                );
            }
        }
        "fall" => {
            input.wish = Vec2::Y; // forward = +Z at yaw pi
            s.min_vy = s.min_vy.min(vel.y / U);
            if t == 90 {
                // Landed means feet within the 0.004 m skin of the floor (0.16 u).
                let landed = feet_u < 0.5 && grounded.on_ground;
                // Sampled at the start of each tick, so the impact speed lags by one tick (10 u/s).
                let ok = landed && (s.min_vy + 376.2).abs() <= 12.0;
                let d = format!("fell 3 m: landing speed {:.1} u/s (376 analytic), landed at {feet_u:.3} u, grounded {}", s.min_vy, grounded.on_ground);
                verdict(&mut s, name, ok, d);
            }
        }
        "force" => {
            // A forced crouch is not held: with room to stand, the player
            // stands up again within ~0.25 s (portal-transit §1.6).
            if t == 90 {
                let at = |k: u32| s.samples.iter().find(|x| x.0 == k).copied().unwrap();
                let e90 = at(90).1;
                let ok = at(1).2 && !at(15).2 && (e90 - 64.0).abs() <= 0.5;
                let d = format!("ducked at tick 1: {}, standing again by tick 15: {}, eye {e90:.2} u at end (64)", at(1).2, !at(15).2);
                verdict(&mut s, name, ok, d);
            }
        }
        "push" => {
            if t > 0 {
                input.wish = Vec2::Y;
            }
            if t == 150 {
                let moved = boxes.iter().next().map_or(0.0, |b| b.translation.z - (s.start.z + 1.2));
                let ok = moved > 0.5;
                verdict(&mut s, name, ok, format!("walking into a dynamic box pushed it {moved:.2} m (> 0.5)"));
            }
        }
        _ => {}
    }
    if s.idx == idx_before {
        s.t += 1;
    }

    if s.idx >= s.order.len() {
        let fails = s.results.iter().filter(|r| !r.1).count();
        println!("{} of {} checks passed", s.results.len() - fails, s.results.len());
        exit.write(if fails == 0 { AppExit::Success } else { AppExit::from_code(1) });
    }
}

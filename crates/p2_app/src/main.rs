//! Portal 2 Rust Engine demo: Portal 2's player, portal gun and portals
//! in a generated test chamber.
//!
//! Controls: WASD move, mouse look (click to grab, Esc to release), Space
//! jump, Ctrl crouch, left/right click fire blue/orange.
//! Harness flags (`--screenshot`, `--frames`, `--shot`, `--exit-after`,
//! `--size`) come from `p2_room::HarnessArgs`.
//!
//! `--demo <dir>`: scripted end-to-end check. Blue on the east wall, orange on
//! the west wall, look through blue (your own body is visible far away
//! through it), walk through, look back. Screenshots go to `<dir>`; the log
//! prints `DEMO` lines with placements and teleports.
//! * `--ceiling`: orange on the ceiling patch (a 90 deg transit); logs a
//!   per-tick `TICK` trace and the gun's offset around the transit.
//! * `--frames-around`: screenshot every frame from ~0.25 s before the transit
//!   to ~1 s after (`seq-NNN.png`), for frame-by-frame glitch checks.
//! * env `P2_PARK_X=<m>`: park the feet at that x in front of blue and take
//!   one screenshot (static repro of near-portal views).
use std::path::PathBuf;

use avian3d::prelude::*;
use bevy::prelude::*;
use p2_core::*;

fn main() {
    // Portal 2 is required: check before any window opens.
    let game = p2_assets::find_portal2().unwrap_or_else(|msg| {
        eprintln!("{msg}");
        std::process::exit(1);
    });
    let args = p2_room::HarnessArgs::from_env();
    let demo = args
        .rest
        .iter()
        .position(|a| a == "--demo")
        .map(|i| PathBuf::from(args.rest.get(i + 1).cloned().unwrap_or_else(|| ".".into())));
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(args.window_plugin("Portal 2 Rust Engine")))
        .insert_resource(P2Settings { set_fixed_timestep: true })
        .insert_resource(p2_assets::GameFsRes(std::sync::Arc::new(game)))
        .add_plugins(PhysicsPlugins::default())
        // The whole library: what a modder adds to their own game.
        .add_plugins(p2_portals::PortalPlugins)
        // The example world (not part of the library).
        .add_plugins(p2_room::RoomPlugin)
        .add_systems(Startup, spawn);
    if let Some(dir) = demo {
        let mut args = args;
        args.exit_after = None;
        app.insert_resource(args)
            .insert_resource(InputLocked(true))
            .insert_resource(Demo { dir, ceiling: args_has_ceiling(), every_frame: std::env::args().any(|a| a == "--frames-around"), fling: std::env::args().any(|a| a == "--fling"), ..default() })
            .add_systems(
                Update,
                (demo_script.run_if(|d: Res<Demo>| !d.fling), fling_script.run_if(|d: Res<Demo>| d.fling), demo_log)
                    .chain()
                    .after(P2Systems::Input)
                    .before(P2Systems::Fire),
            )
            .add_systems(PostUpdate, demo_gun_check.after(P2Systems::View))
            .add_systems(PostUpdate, demo_cam_trace.after(bevy::transform::TransformSystems::Propagate))
            .add_systems(FixedUpdate, demo_tick_trace.after(P2Systems::Transit));
    } else {
        app.insert_resource(args);
    }
    app.add_plugins(p2_room::HarnessPlugin).run();
}

/// Per-tick trace of the player for a few ticks around transits (`--ceiling`).
fn demo_tick_trace(
    demo: Res<Demo>,
    mut tp: MessageReader<PortalTeleported>,
    players: Query<(&Transform, &CharacterVelocity, &IgnoreColliders, &Grounded, &Crouch), With<Player>>,
    mut left: Local<u32>,
    mut tick: Local<u32>,
) {
    *tick += 1;
    if tp.read().count() > 0 {
        *left = 40;
    }
    if !(demo.ceiling || std::env::args().any(|a| a == "--high-exit")) || *left == 0 {
        return;
    }
    *left -= 1;
    if let Ok((tf, v, ign, g, c)) = players.single() {
        info!("TICK {} feet {:.3?} vel {:.2?} ignores {} ground {} ducked {}", *tick, tf.translation, v.0, ign.len(), g.on_ground, c.ducked);
    }
}

/// `--fling`: blue on the pit floor, orange on the east wall; the player is
/// dropped above the pit, falls 512 u into blue and flies out of orange.
/// Portal 2: exit speed = arrival speed (sqrt(2 g h) ~ 784 u/s here, under the
/// 1000 u/s cap), straight out of the exit's normal.
fn fling_script(
    time: Res<Time>,
    mut demo: ResMut<Demo>,
    mut commands: Commands,
    mut players: Query<(Entity, &mut Transform, &mut InterpolatedFeet, &mut ViewAngles, &mut MoveInput, &CharacterVelocity), With<Player>>,
    mut fire: MessageWriter<FirePortal>,
    mut tp: MessageReader<PortalTeleported>,
    mut exit: MessageWriter<AppExit>,
    mut last: Local<f32>,
) {
    let t = time.elapsed_secs();
    let prev = std::mem::replace(&mut *last, t);
    let at = |s: f32| prev < s && t >= s;
    let Ok((e, mut tf, mut feet, mut view, mut input, vel)) = players.single_mut() else { return };
    let eye_of = |f: Vec3| f + Vec3::Y * VIEW_STAND;
    let aim = |from: Vec3, to: Vec3| {
        let d = to - eye_of(from);
        ViewAngles { yaw: (-d.x).atan2(-d.z), pitch: d.y.atan2(Vec2::new(d.x, d.z).length()), roll: 0.0 }
    };
    let rim = Vec3::new(u(-304.0), 0.0, u(168.0));
    let pit_floor = Vec3::new(u(-304.0), u(-512.0), u(304.0));
    let east_wall = Vec3::new(u(512.0), u(72.0), u(-64.0));
    input.wish = Vec2::ZERO;
    if t < 3.5 {
        tf.translation = rim;
        *feet = InterpolatedFeet { prev: rim, curr: rim };
    }
    if t < 1.6 {
        *view = aim(rim, pit_floor);
    } else if t < 3.5 {
        *view = aim(rim, east_wall);
    }
    if at(1.0) {
        fire.write(FirePortal { gun: e, kind: PortalKind::Blue });
    }
    if at(2.5) {
        fire.write(FirePortal { gun: e, kind: PortalKind::Orange });
    }
    if at(3.5) {
        // Drop: feet 16 u above the rim height, straight over the floor portal.
        let p = Vec3::new(pit_floor.x, u(16.0), pit_floor.z);
        tf.translation = p;
        *feet = InterpolatedFeet { prev: p, curr: p };
        *view = ViewAngles { yaw: std::f32::consts::PI, pitch: -1.2, roll: 0.0 };
        demo.walking = true;
        info!("FLING dropped at {:.2?}", p);
    }
    if demo.every_frame && demo.walking && demo.seq < 140 && (demo.teleported_at.is_some() || tf.translation.y < u(-380.0)) {
        demo.seq += 1;
        let path = demo.dir.join(format!("seq-{:03}.png", demo.seq));
        commands.spawn(bevy::render::view::screenshot::Screenshot::primary_window()).observe(bevy::render::view::screenshot::save_to_disk(path));
    }
    for ev in tp.read() {
        if demo.teleported_at.is_none() {
            demo.teleported_at = Some(t);
        }
        info!("FLING teleport at t={:.3}: speed out {:.0} u/s, velocity {:.2?} m/s", t, ev.velocity_out.length() / U, ev.velocity_out);
    }
    if let Some(t0) = demo.teleported_at {
        if (t - t0 - 0.25).abs() < 0.01 || (t - t0 - 0.5).abs() < 0.01 {
            info!("FLING t+{:.2}: feet {:.2?} vel {:.2?} ({:.0} u/s)", t - t0, tf.translation, vel.0, vel.0.length() / U);
        }
        if t - t0 > 1.5 {
            exit.write(AppExit::Success);
        }
    }
    if t > 12.0 {
        warn!("FLING timed out");
        exit.write(AppExit::Success);
    }
}

/// `--frames-around`: per captured frame, where the main camera really is.
fn demo_cam_trace(
    demo: Res<Demo>,
    cams: Query<&GlobalTransform, With<MainCamera>>,
    players: Query<(&Transform, &InterpolatedFeet), With<Player>>,
    fixed: Res<Time<Fixed>>,
    mut last_seq: Local<u32>,
) {
    if !demo.every_frame || demo.seq == *last_seq {
        return;
    }
    *last_seq = demo.seq;
    let (Ok(cam), Ok((tf, feet))) = (cams.single(), players.single()) else { return };
    info!(
        "CAM seq {:03} cam {:.3?} feet {:.3?} prev {:.3?} curr {:.3?} overstep {:.2}",
        demo.seq, cam.translation(), tf.translation, feet.prev, feet.curr, fixed.overstep_fraction()
    );
}

fn args_has_ceiling() -> bool {
    std::env::args().any(|a| a == "--ceiling")
}

fn spawn(mut commands: Commands, spots: Res<p2_room::RoomSpots>) {
    spawn_player(&mut commands, spots.spawn, spots.spawn_yaw);
}

#[derive(Resource, Default)]
struct Demo {
    dir: PathBuf,
    /// `--ceiling`: orange on the ceiling patch, so the transit turns the view 90 deg.
    ceiling: bool,
    /// Frames left to screenshot right after the transit.
    burst: u32,
    /// `--fling`: floor portal in the pit, wall exit (the classic fling).
    fling: bool,
    /// `--frames-around`: screenshot every frame around the transit.
    every_frame: bool,
    seq: u32,
    /// Muzzle offset from the eye in view space, last frame (gun glitch check).
    last_muzzle: Option<Vec3>,
    shots: u32,
    walking: bool,
    teleported_at: Option<f32>,
    turned_at: Option<f32>,
    taken: Vec<&'static str>,
}

impl Demo {
    /// Take each named screenshot once.
    fn shot(&mut self, commands: &mut Commands, name: &'static str) {
        // One screenshot per frame: the every-frame sequence owns them.
        if self.every_frame || self.taken.contains(&name) {
            return;
        }
        self.taken.push(name);
        self.shots += 1;
        let path = self.dir.join(format!("{}-{name}.png", self.shots));
        info!("DEMO screenshot {}", path.display());
        commands.spawn(bevy::render::view::screenshot::Screenshot::primary_window())
            .observe(bevy::render::view::screenshot::save_to_disk(path));
    }
}

/// Time-driven (frame rate independent). East wall x = +512 u, west wall
/// x = -512 u; yaw -90 deg looks down +X.
fn demo_script(
    time: Res<Time>,
    mut demo: ResMut<Demo>,
    mut commands: Commands,
    mut players: Query<(Entity, &mut Transform, &mut InterpolatedFeet, &mut ViewAngles, &mut MoveInput), With<Player>>,
    mut fire: MessageWriter<FirePortal>,
    mut tp: MessageReader<PortalTeleported>,
    mut exit: MessageWriter<AppExit>,
    mut last: Local<f32>,
) {
    let t = time.elapsed_secs();
    let prev = std::mem::replace(&mut *last, t);
    let at = |s: f32| prev < s && t >= s;
    let Ok((e, mut tf, mut feet, mut view, mut input)) = players.single_mut() else { return };
    let (east, west) = (-std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2);
    if at(0.2) {
        // 300 u from the east wall, in the open strip between the recursion walls.
        let p = Vec3::new(u(212.0), 0.0, u(-64.0));
        tf.translation = p;
        *feet = InterpolatedFeet { prev: p, curr: p };
    }
    if t < 1.6 {
        *view = ViewAngles { yaw: east, pitch: 0.0, roll: 0.0 };
    }
    if at(1.0) {
        fire.write(FirePortal { gun: e, kind: PortalKind::Blue });
    }
    if at(1.12) {
        demo.shot(&mut commands, "projectile-a");
    }
    if at(1.17) {
        demo.shot(&mut commands, "projectile-b");
    }
    if at(1.5) {
        demo.shot(&mut commands, "blue-placed");
    }
    if (1.6..3.1).contains(&t) {
        *view = if std::env::args().any(|a| a == "--high-exit") {
            // West wall, ~1.4 m higher than blue: you come out with airtime.
            let d = Vec3::new(u(-724.0), u(56.0), 0.0);
            ViewAngles { yaw: (-d.x).atan2(-d.z), pitch: d.y.atan2(Vec2::new(d.x, d.z).length()), roll: 0.0 }
        } else if demo.ceiling {
            // Ceiling patch centre (0, 448, 64) u from the eye at (212, 64, -64) u.
            let d = Vec3::new(u(-212.0), u(384.0), u(128.0));
            ViewAngles { yaw: (-d.x).atan2(-d.z), pitch: d.y.atan2(Vec2::new(d.x, d.z).length()), roll: 0.0 }
        } else {
            ViewAngles { yaw: west, pitch: 0.0, roll: 0.0 }
        };
    }
    if at(2.5) {
        fire.write(FirePortal { gun: e, kind: PortalKind::Orange });
    }
    if at(3.0) {
        demo.shot(&mut commands, "orange-placed");
    }
    if (3.1..3.8).contains(&t) {
        *view = ViewAngles { yaw: east, pitch: 0.0, roll: 0.0 };
    }
    if at(3.6) {
        demo.shot(&mut commands, "through-blue");
    }
    // P2_PARK_X=<metres>: park the feet at that x instead of walking (repro).
    if let Some(x) = std::env::var("P2_PARK_X").ok().and_then(|v| v.parse::<f32>().ok()) {
        if t >= 3.8 {
            let p = Vec3::new(x, tf.translation.y, tf.translation.z);
            tf.translation = p;
            *feet = InterpolatedFeet { prev: p, curr: p };
            input.wish = Vec2::ZERO;
            if at(4.5) {
                demo.shot(&mut commands, "parked");
            }
            if t > 4.8 {
                exit.write(AppExit::Success);
            }
            return;
        }
    }
    if at(3.8) {
        demo.walking = true;
    }
    if demo.walking {
        input.wish = Vec2::new(0.0, 1.0);
        if demo.teleported_at.is_none() && tf.translation.x > u(440.0) {
            demo.shot(&mut commands, "approach");
        }
        if demo.teleported_at.is_none() && tf.translation.x > u(500.0) {
            demo.shot(&mut commands, "straddling");
        }
        // `--frames-around`: every frame from ~0.25 s before the transit on.
        if demo.every_frame && demo.teleported_at.is_none() && tf.translation.x > u(470.0) {
            demo.seq += 1;
            let path = demo.dir.join(format!("seq-{:03}.png", demo.seq));
            commands.spawn(bevy::render::view::screenshot::Screenshot::primary_window())
                .observe(bevy::render::view::screenshot::save_to_disk(path));
        }
    }
    if tp.read().next().is_some() && demo.teleported_at.is_none() {
        demo.teleported_at = Some(t);
        demo.shot(&mut commands, "just-teleported");
        demo.burst = 4;
    }
    // Every frame right after the transit, for the gun-glitch check.
    if demo.every_frame && demo.teleported_at.is_some() && demo.seq < 60 {
        demo.seq += 1;
        let path = demo.dir.join(format!("seq-{:03}.png", demo.seq));
        commands.spawn(bevy::render::view::screenshot::Screenshot::primary_window())
            .observe(bevy::render::view::screenshot::save_to_disk(path));
    }
    if demo.burst > 0 && demo.teleported_at != Some(t) {
        let n = demo.burst;
        demo.burst -= 1;
        demo.shot(&mut commands, ["", "post-4", "post-3", "post-2", "post-1"][n as usize]);
    }
    if let Some(t0) = demo.teleported_at {
        if t - t0 > 0.4 && demo.turned_at.is_none() {
            demo.shot(&mut commands, "after");
            demo.walking = false;
            input.wish = Vec2::ZERO;
            view.yaw += std::f32::consts::PI;
            demo.turned_at = Some(t);
        }
    }
    if let Some(t1) = demo.turned_at {
        if t - t1 > 0.6 {
            demo.shot(&mut commands, "look-back");
        }
        if t - t1 > 1.2 {
            exit.write(AppExit::Success);
        }
    }
    if t > 15.0 {
        warn!("DEMO timed out (teleported: {:?})", demo.teleported_at);
        exit.write(AppExit::Success);
    }
}

/// Logs the muzzle's offset from the eye in view space around a transit: a
/// smooth gun moves it by fractions of a unit per frame; a glitch jumps.
fn demo_gun_check(
    mut demo: ResMut<Demo>,
    players: Query<(&ViewmodelAttachments, &ViewAngles, &InterpolatedFeet, &Crouch), With<Player>>,
    fixed: Res<Time<Fixed>>,
    mut worst: Local<f32>,
) {
    let Ok((att, view, feet, crouch)) = players.single() else { return };
    let Some(m) = att.muzzle else { return };
    let eye = feet.at(fixed.overstep_fraction()) + Vec3::Y * crouch.eye_height;
    let local = view.rotation().inverse() * (m.translation - eye);
    if let Some(prev) = demo.last_muzzle {
        let jump = (local - prev).length() / U;
        if demo.teleported_at.is_some() && demo.turned_at.is_none() {
            *worst = worst.max(jump);
            info!("DEMO gun offset change {:.2} u (worst since transit {:.2} u)", jump, *worst);
        }
    }
    demo.last_muzzle = Some(local);
}

fn demo_log(
    mut placed: MessageReader<PortalPlaced>,
    mut failed: MessageReader<PortalShotFailed>,
    mut tp: MessageReader<PortalTeleported>,
    players: Query<(&Transform, &CharacterVelocity), With<Player>>,
    portals: Query<(Entity, &Portal, &PortalAnimation, Option<&PortalLink>, &Visibility, Option<&ViewVisibility>)>,
    stats: Option<Res<p2_portals::fx::ParticleStats>>,
    mut frame: Local<u32>,
) {
    *frame += 1;
    // (Teleports are logged here from a separate reader of PortalTeleported.)
    for p in placed.read() {
        info!("DEMO f{} placed {:?} at {:.2?} normal {:.2?}", *frame, p.kind, p.transform.translation, p.transform.rotation * Vec3::Z);
    }
    for p in failed.read() {
        info!("DEMO f{} shot failed {:?}: {:?} at {:.2?}", *frame, p.kind, p.reason, p.point);
    }
    for t in tp.read() {
        let feet = players.single().map(|(tf, _)| tf.translation).unwrap_or_default();
        info!("DEMO f{} TELEPORT {:?} -> {:?} velocity out {:.2?} feet now {:.2?}", *frame, t.from_portal, t.to_portal, t.velocity_out, feet);
    }
    if *frame % 120 == 0 {
        if let Some(st) = &stats {
            let mut v: Vec<_> = st.alive.iter().filter(|(_, n)| **n > 0).collect();
            v.sort();
            info!("DEMO f{} particles alive {:?}", *frame, v);
        }
    }
    if *frame % 60 == 0 {
        for (e, p, a, l, v, vv) in &portals {
            info!("DEMO f{} portal {e} {:?} open {:.2} static {:.2} link {:?} vis {:?} viewvis {:?}", *frame, p.kind, a.open_amount, a.static_amount, l, v, vv.map(|x| x.get()));
        }
    }
    if *frame % 20 == 0 {
        if let Ok((tf, v)) = players.single() {
            info!("DEMO f{} feet {:.2?} vel {:.2?}", *frame, tf.translation, v.0);
        }
    }
}

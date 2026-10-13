//! Portal transit check (`research/specs/portal-transit.md`).
//!
//! A stand-in mover (scripted walk or fall + gravity, kinematic, honouring
//! `IgnoreColliders`) drives the player into a portal pair. Every
//! `PortalTeleported` is logged, and each case checks the teleport first, then
//! the exit position, velocity, forced duck and collision.
//!
//! ```text
//! cargo run -p p2_transit --example transit -- --case wall --exit-after 400
//! cases: wall | fall | wallfloor | straddle | ceiling   (default: wall)
//! ```
//! The mover exists only for this example; the movement crate replaces it.
use avian3d::prelude::*;
use bevy::app::AppExit;
use bevy::prelude::*;
use p2_core::math::PortalFrame;
use p2_core::{
    spawn_player, u, CharacterVelocity, CorePlugin, Crouch, ForceDuck, Grounded, IgnoreColliders, InterpolatedFeet,
    P2Systems, Player, Portal, PortalKind, PortalLink, PortalTeleported, Portalable, ViewAngles, PORTAL_HALF_SIZE,
};
use p2_room::{HarnessArgs, HarnessPlugin, RoomPlugin};

const GRAVITY: f32 = 600.0 * p2_core::U; // sv_gravity, m/s^2
const TICK_DT: f32 = 1.0 / 60.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Case {
    /// Walk into a wall portal, out of the wall portal on the far side.
    Wall,
    /// Fall into a floor portal, fling out of a wall portal (speed capped at 1000 u/s).
    Fall,
    /// Walk into a wall portal, come out of a floor portal (300 u/s floor minimum).
    WallFloor,
    /// Stand with the hull straddling a floor portal's rim: no fall-through.
    Straddle,
    /// Walk into a wall portal, come out of a ceiling portal (forced duck).
    Ceiling,
}

impl Case {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "wall" => Case::Wall,
            "fall" => Case::Fall,
            "wallfloor" => Case::WallFloor,
            "straddle" => Case::Straddle,
            "ceiling" => Case::Ceiling,
            _ => return None,
        })
    }
}

/// One portal of the test pair: where it is, the way it faces, and its up hint.
#[derive(Clone, Copy)]
struct Mount {
    pos: Vec3,
    normal: Vec3,
    up: Vec3,
}

impl Mount {
    fn frame(&self) -> PortalFrame {
        PortalFrame::from_normal_up(self.pos, self.normal, self.up).expect("mount frame")
    }
}

/// A test scenario: the pair, the player start and the scripted motion.
#[derive(Resource, Clone)]
struct Setup {
    case: Case,
    entry: Mount,
    exit: Mount,
    feet: Vec3,
    yaw: f32,
    /// Scripted horizontal velocity while the script is active (m/s).
    walk: Vec3,
    /// Vertical velocity at spawn (m/s).
    init_vel: Vec3,
    /// Seconds the script runs for (or until the first teleport).
    duration: f32,
}

fn setup_for(case: Case) -> Setup {
    let wall_a = Mount { pos: uv(-512.0, 50.0, -64.0), normal: Vec3::X, up: Vec3::Y };
    let wall_b = Mount { pos: uv(512.0, 50.0, -64.0), normal: Vec3::NEG_X, up: Vec3::Y };
    let floor_f = Mount { pos: uv(0.0, 0.0, 0.0), normal: Vec3::Y, up: Vec3::NEG_Z };
    let ceiling_c = Mount { pos: uv(0.0, 448.0, 64.0), normal: Vec3::NEG_Y, up: Vec3::NEG_Z };
    let walk_fast = Vec3::new(-300.0 * p2_core::U, 0.0, 0.0);
    let walk_slow = Vec3::new(-200.0 * p2_core::U, 0.0, 0.0);
    let face_west = std::f32::consts::FRAC_PI_2; // looking down -X
    match case {
        Case::Wall => Setup {
            case,
            entry: wall_a,
            exit: wall_b,
            feet: uv(-256.0, 0.0, -64.0),
            yaw: face_west,
            walk: walk_fast,
            init_vel: Vec3::ZERO,
            duration: 1.2,
        },
        Case::Fall => Setup {
            case,
            entry: floor_f,
            exit: wall_b,
            feet: Vec3::new(0.0, u(315.0), 0.0),
            yaw: 0.0,
            walk: Vec3::ZERO,
            init_vel: Vec3::new(0.0, -25.0, 0.0),
            duration: 1.0,
        },
        Case::WallFloor => Setup {
            case,
            entry: wall_a,
            exit: floor_f,
            feet: uv(-256.0, 0.0, -64.0),
            yaw: face_west,
            walk: walk_slow,
            init_vel: Vec3::ZERO,
            duration: 1.0,
        },
        Case::Straddle => Setup {
            case,
            entry: floor_f,
            exit: wall_b,
            feet: uv(40.0, 0.0, 0.0),
            yaw: 0.0,
            walk: Vec3::ZERO,
            init_vel: Vec3::ZERO,
            duration: 3.0,
        },
        Case::Ceiling => Setup {
            case,
            entry: wall_a,
            exit: ceiling_c,
            feet: uv(-256.0, 0.0, -64.0),
            yaw: face_west,
            walk: walk_slow,
            init_vel: Vec3::ZERO,
            duration: 1.2,
        },
    }
}

fn uv(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(u(x), u(y), u(z))
}

/// The two spawned portals of the pair.
#[derive(Resource, Clone, Copy)]
struct Pair {
    entry: Entity,
    exit: Entity,
}

/// What the run has seen so far, for the checks.
#[derive(Resource, Default)]
struct Report {
    ticks: u32,
    /// Teleports: (from, to, velocity out, pre-teleport velocity).
    teleports: Vec<(Entity, Entity, Vec3, Vec3)>,
    /// Feet position right after the first teleport.
    first_after: Option<Vec3>,
    ducks: u32,
    min_feet_y: f32,
    max_step: f32,
    last_feet: Option<Vec3>,
}

/// The mover's velocity this tick before the transit (for the speed checks).
#[derive(Resource, Default)]
struct Script {
    active: bool,
    t: f32,
    pre_vel: Vec3,
}

fn main() {
    let args = HarnessArgs::from_env();
    let case_name = args.rest.windows(2).find(|w| w[0] == "--case").map(|w| w[1].clone()).unwrap_or("wall".into());
    let Some(case) = Case::parse(&case_name) else {
        eprintln!("unknown --case {case_name} (wall | fall | wallfloor | straddle | ceiling)");
        std::process::exit(2);
    };
    let setup = setup_for(case);
    let mut app = App::new();
    // Portal 2's 60 Hz tick is opt-in (CorePlugin reads this before it runs).
    app.insert_resource(p2_core::P2Settings { set_fixed_timestep: true })
        .insert_resource(args.clone())
        .insert_resource(setup.clone())
        .insert_resource(Script { active: true, ..default() })
        .insert_resource(Report { min_feet_y: f32::MAX, ..default() })
        .add_plugins(DefaultPlugins.set(args.window_plugin("p2_transit example")))
        .add_plugins((PhysicsPlugins::default(), CorePlugin, RoomPlugin, HarnessPlugin, p2_transit::Plugin))
        .add_systems(Startup, spawn_test_player)
        .add_systems(PostStartup, spawn_portals)
        .add_systems(FixedUpdate, drive.in_set(P2Systems::Move))
        .add_systems(FixedUpdate, report.after(P2Systems::Transit))
        .add_systems(Update, log_teleports)
        .run();
}

fn spawn_test_player(mut commands: Commands, setup: Res<Setup>) {
    let e = spawn_player(&mut commands, setup.feet, setup.yaw);
    commands.entity(e).insert(CharacterVelocity(setup.init_vel));
}

/// Surface colliders around a portal: the wall or floor the opening sits in.
fn surfaces_at(p: Vec3, n: Vec3, boxes: &Query<(Entity, &Collider, &Transform), With<Portalable>>) -> Vec<Entity> {
    let probes = [p - n * 0.02, p + n * 0.02];
    let mut out = Vec::new();
    for (e, col, tf) in boxes {
        let Some(c) = col.shape_scaled().as_cuboid() else { continue };
        let half: Vec3 = c.half_extents.into();
        let inside = probes.iter().any(|&q| {
            let l = tf.rotation.inverse() * (q - tf.translation);
            l.abs().cmple(half + Vec3::splat(1e-4)).all()
        });
        if inside {
            out.push(e);
        }
    }
    out
}

fn spawn_portals(
    mut commands: Commands,
    setup: Res<Setup>,
    boxes: Query<(Entity, &Collider, &Transform), With<Portalable>>,
) {
    let mut make = |m: Mount, kind: PortalKind| -> Entity {
        let frame = m.frame();
        let surfaces = surfaces_at(m.pos, m.normal, &boxes);
        if surfaces.is_empty() {
            warn!("no surface found behind the {kind:?} portal at {:?}", m.pos);
        }
        commands
            .spawn((
                Name::new(format!("{kind:?} portal")),
                Portal { kind, half_size: PORTAL_HALF_SIZE, surfaces, placed_at: 0.0 },
                Transform::from_translation(frame.position).with_rotation(frame.rotation),
            ))
            .id()
    };
    let entry = make(setup.entry, PortalKind::Blue);
    let exit = make(setup.exit, PortalKind::Orange);
    commands.entity(entry).insert(PortalLink(exit));
    commands.entity(exit).insert(PortalLink(entry));
    commands.insert_resource(Pair { entry, exit });
    info!("case {:?}: entry {entry:?} exit {exit:?}", setup.case);
}

/// Stand-in mover: scripted horizontal walk until the first teleport (or the
/// duration runs out), gravity, axis-separated kinematic collision that ignores
/// `IgnoreColliders`. Honours `ForceDuck`.
#[allow(clippy::type_complexity)]
fn drive(
    time: Res<Time>,
    setup: Res<Setup>,
    mut script: ResMut<Script>,
    mut players: Query<
        (Entity, &mut Transform, &mut InterpolatedFeet, &mut CharacterVelocity, &mut Grounded, &mut Crouch, &IgnoreColliders),
        With<Player>,
    >,
    mut ducks: MessageReader<ForceDuck>,
    spatial: SpatialQuery,
) {
    let dt = time.delta_secs();
    let forced: Vec<Entity> = ducks.read().map(|d| d.entity).collect();
    script.t += dt;
    if script.t >= setup.duration {
        script.active = false;
    }
    for (entity, mut tf, mut feet, mut vel, mut grounded, mut crouch, ignores) in &mut players {
        if forced.contains(&entity) {
            // Movement's job in the real game: duck instantly (air duck).
            crouch.ducked = true;
            crouch.wants_duck = true;
        }
        feet.prev = tf.translation;
        if script.active {
            vel.0.x = setup.walk.x;
            vel.0.z = setup.walk.z;
        }
        if !grounded.on_ground {
            vel.0.y -= GRAVITY * dt;
        } else if vel.0.y < 0.0 {
            vel.0.y = 0.0;
        }
        script.pre_vel = vel.0;
        let he = crouch.hull() * 0.5;
        let blocked = |p: Vec3| -> bool {
            let centre = p + crouch.hull_centre_offset();
            let half = he - Vec3::splat(0.01);
            let shape = Collider::cuboid(half.x * 2.0, half.y * 2.0, half.z * 2.0);
            spatial
                .shape_intersections(&shape, centre, Quat::IDENTITY, &SpatialQueryFilter::default())
                .into_iter()
                .any(|e| !ignores.contains(&e))
        };
        let mut p = tf.translation;
        let mut on_ground = false;
        for _ in 0..2 {
            let sub = dt / 2.0;
            let px = p + Vec3::X * vel.0.x * sub;
            if blocked(px) {
                vel.0.x = 0.0;
            } else {
                p = px;
            }
            let pz = p + Vec3::Z * vel.0.z * sub;
            if blocked(pz) {
                vel.0.z = 0.0;
            } else {
                p = pz;
            }
            let py = p + Vec3::Y * vel.0.y * sub;
            if blocked(py) {
                on_ground = vel.0.y <= 0.0;
                vel.0.y = 0.0;
            } else {
                p = py;
            }
        }
        grounded.on_ground = on_ground;
        tf.translation = p;
        feet.curr = p;
    }
}

fn report(
    mut report: ResMut<Report>,
    mut teleports: MessageReader<PortalTeleported>,
    mut ducks: MessageReader<ForceDuck>,
    mut script: ResMut<Script>,
    setup: Res<Setup>,
    pair: Res<Pair>,
    portals: Query<(&Portal, &Transform)>,
    players: Query<&Transform, With<Player>>,
    mut exit: MessageWriter<AppExit>,
) {
    report.ticks += 1;
    report.ducks += ducks.read().count() as u32;
    let Ok(tf) = players.single() else { return };
    let feet = tf.translation;
    if let Some(last) = report.last_feet {
        report.max_step = report.max_step.max((feet - last).length());
    }
    report.last_feet = Some(feet);
    report.min_feet_y = report.min_feet_y.min(feet.y);
    for t in teleports.read() {
        println!(
            "[transit] PortalTeleported entity={:?} {:?} -> {:?} v_out=({:.3}, {:.3}, {:.3}) m/s |v_out|={:.3} ({:.0} u/s)",
            t.entity,
            t.from_portal,
            t.to_portal,
            t.velocity_out.x,
            t.velocity_out.y,
            t.velocity_out.z,
            t.velocity_out.length(),
            t.velocity_out.length() / p2_core::U
        );
        if report.teleports.is_empty() {
            report.first_after = Some(feet);
        }
        report.teleports.push((t.from_portal, t.to_portal, t.velocity_out, script.pre_vel));
        script.active = false;
    }
    // Let the run settle for a few ticks after the script ends, then judge.
    let end_ticks = ((setup.duration + 0.5) / TICK_DT) as u32;
    if report.ticks >= end_ticks {
        let ok = evaluate(&setup, &report, &pair, &portals);
        println!("[transit] case {:?}: {}", setup.case, if ok { "PASS" } else { "FAIL" });
        exit.write(AppExit::Success);
    }
}

fn check(name: &str, ok: bool, detail: String) -> bool {
    println!("  [{}] {name}: {detail}", if ok { "ok" } else { "no" });
    ok
}

/// Per-case assertions. The teleport comes first: nothing else is judged unless it happened.
fn evaluate(
    setup: &Setup,
    report: &Report,
    pair: &Pair,
    portals: &Query<(&Portal, &Transform)>,
) -> bool {
    let exit_frame = portals
        .get(pair.exit)
        .map(|(_, t)| Portal::frame(t))
        .unwrap_or_else(|_| setup.exit.frame());
    let mut ok = true;
    let (expect_teleport, label) = match setup.case {
        Case::Straddle => (false, "no teleport"),
        _ => (true, "teleported"),
    };
    let n = report.teleports.len();
    ok &= check(label, (n > 0) == expect_teleport, format!("{n} teleport(s), {} forced duck(s)", report.ducks));
    if !expect_teleport {
        ok &= check(
            "no fall-through (feet stay on the floor)",
            report.min_feet_y > -0.03,
            format!("lowest feet y {:.3} m", report.min_feet_y),
        );
        ok &= check(
            "no pop (per-tick movement stays small)",
            report.max_step < 0.05,
            format!("largest per-tick step {:.3} m", report.max_step),
        );
        return ok;
    }
    if n == 0 {
        return false;
    }
    let (from, to, v_out, pre) = report.teleports[0];
    ok &= check(
        "through the right pair",
        from == pair.entry && to == pair.exit,
        format!("from {from:?} to {to:?}"),
    );
    if let Some(after) = report.first_after {
        // Judge the hull centre, not the feet: the centre is what crosses the plane.
        // A forced crouch happens in the teleport tick (§1.6), so the player
        // arrives with the crouched hull.
        let hull = if report.ducks > 0 { p2_core::HULL_DUCK } else { p2_core::HULL_STAND };
        let l = exit_frame.to_local(after + Vec3::Y * hull.y * 0.5);
        ok &= check(
            "arrives in front of the exit opening",
            l.z > -0.05 && l.x.abs() < PORTAL_HALF_SIZE.x + 0.5 && l.y.abs() < PORTAL_HALF_SIZE.y + 1.0,
            format!("feet local to exit ({:.2}, {:.2}, {:.2}) m", l.x, l.y, l.z),
        );
    }
    let speed = v_out.length();
    let pre_speed = pre.length();
    match setup.case {
        Case::Wall => {
            ok &= check(
                "wall to wall keeps the speed and height",
                (v_out.x + 300.0 * p2_core::U).abs() < 0.05 && v_out.y.abs() < 0.05,
                format!("v_out {v_out:.3?} m/s, expect (-7.620, ~0, 0)"),
            );
        }
        Case::Fall => {
            let expect = pre_speed.min(1000.0 * p2_core::U);
            ok &= check(
                "exit speed is the entry speed, capped at 1000 u/s",
                (speed - expect).abs() < 0.05,
                format!("|v_out| {speed:.3} m/s, |v_in| {pre_speed:.3} m/s, expect {expect:.3}"),
            );
            let out_dir = v_out.normalize_or_zero().dot(exit_frame.normal());
            ok &= check("leaves through the exit face", out_dir > 0.99, format!("cos to exit normal {out_dir:.4}"));
        }
        Case::WallFloor => {
            let want = 300.0 * p2_core::U;
            ok &= check(
                "floor exit raised to 300 u/s",
                (v_out.dot(exit_frame.normal()) - want).abs() < 0.02,
                format!("out along exit normal {:.3} m/s, expect {want:.3}", v_out.dot(exit_frame.normal())),
            );
            ok &= check("forced duck on wall to floor", report.ducks >= 1, format!("{} forced duck(s)", report.ducks));
        }
        Case::Ceiling => {
            ok &= check(
                "walks out of the ceiling at the arrival speed",
                (v_out.y + 200.0 * p2_core::U).abs() < 0.3,
                format!("v_out.y {:.3} m/s, expect -5.08 plus the spec's gravity re-add (about -0.26)", v_out.y),
            );
            ok &= check("forced duck on wall to ceiling", report.ducks >= 1, format!("{} forced duck(s)", report.ducks));
        }
        Case::Straddle => {}
    }
    ok
}

fn log_teleports(mut teleports: MessageReader<PortalTeleported>, players: Query<&ViewAngles, With<Player>>) {
    for _ in teleports.read() {
        if let Ok(v) = players.single() {
            println!("[transit] view after teleport: yaw {:.3} pitch {:.3} roll {:.3}", v.yaw, v.pitch, v.roll);
        }
    }
}



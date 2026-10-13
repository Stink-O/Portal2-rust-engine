//! Portal gun placement checks in the greybox chamber.
//!
//! `cargo run -p p2_gun --example gun -- --script`
//!
//! `--script` aims the gun from the named spots in [`RoomSpots`] (feet, yaw and
//! pitch from `p2_room`), checks each result against the spec, prints PASS/FAIL
//! and exits non-zero on any failure. Without `--script` the window shows the
//! room with the mouse-input plugin on (it fires only while the cursor is
//! grabbed, which this example does not do).
use avian3d::prelude::*;
use bevy::prelude::*;
use p2_core::{
    spawn_player, Crouch, FirePortal, GunFired, InputLocked, InterpolatedFeet, P2Settings, P2Systems, Player, Portal,
    PortalClosed, PortalGun, PortalKind, PortalLink, PortalPlaced, PortalShotFailed, ShotFailReason, ViewAngles, VIEW_STAND,
    u,
};
use p2_room::{HarnessArgs, HarnessPlugin, RoomPlugin, RoomSpots};
use std::f32::consts::FRAC_PI_4;

/// Where the centre of a placed portal must be.
#[derive(Clone, Copy)]
enum Centre {
    /// Within 2 m of this point (where the view ray meets the surface).
    Near(Vec3),
    /// The whole 64 x 112 u opening is on the white side of the edge at z = 0:
    /// the centre's z is at or below this value.
    WhiteBelowZ(f32),
    /// Not checked.
    Any,
}

/// Expected outcome of one shot.
#[derive(Clone, Copy)]
enum Expect {
    /// The portal opens with its normal `normal` and its centre per `centre`.
    Placed { centre: Centre, normal: Vec3 },
    /// The shot fails with one of `reasons` and the gun's portal of that colour is unchanged.
    Failed(&'static [ShotFailReason]),
}

/// One scripted shot: stand at `feet`, look along `yaw`/`pitch`, fire `kind`.
#[derive(Clone, Copy)]
struct Step {
    label: &'static str,
    kind: PortalKind,
    feet: Vec3,
    yaw: f32,
    pitch: f32,
    expect: Expect,
}

/// Eye position for a spot's feet.
fn eye(feet: Vec3) -> Vec3 {
    feet + Vec3::Y * VIEW_STAND
}

/// Unit view direction for a yaw and pitch.
fn fwd(yaw: f32, pitch: f32) -> Vec3 {
    ViewAngles { yaw, pitch, roll: 0.0 }.forward()
}

/// Where a ray from `origin` along `dir` meets the plane x = `x`.
fn hit_x(origin: Vec3, dir: Vec3, x: f32) -> Vec3 {
    origin + dir * ((x - origin.x) / dir.x)
}

fn steps(spots: &RoomSpots) -> Vec<Step> {
    // Recursion spot: the east wall (x = 512 u) is straight ahead.
    let rec = spots.recursion;
    let rec_dir = fwd(rec.yaw, rec.pitch);
    let rec_hit = hit_x(eye(rec.feet), rec_dir, u(512.0));
    // Orange aimed 0.6 m along +Z from the same wall point.
    let (beside_yaw, beside_pitch) = look_angles(eye(rec.feet), rec_hit + Vec3::Z * 0.6);

    // Small panel: the spot's own pitch misses the 48 x 96 u insert (the ray
    // passes under it), so aim at the insert's centre (x -72, y 160, z 512 u).
    let panel = spots.small_panel;
    let (panel_yaw, panel_pitch) = look_angles(eye(panel.feet), Vec3::new(u(-72.0), u(160.0), u(512.0)));

    // Pit rim: aim at the pit floor centre (x -304, y -512, z 304 u).
    let pit = spots.pit_rim;
    let (pit_yaw, pit_pitch) = look_angles(eye(pit.feet), Vec3::new(u(-304.0), u(-512.0), u(304.0)));

    let edge = spots.edge_bump;
    // Aim 8 u from the white/black edge (z = 0) so the opening must bump >= 24 u.
    let (edge_yaw, edge_pitch) = look_angles(eye(edge.feet), Vec3::new(u(512.0), eye(edge.feet).y, u(-8.0)));
    let ang = spots.angled_panel;
    let ceil = spots.ceiling;
    let black = spots.black_wall;
    vec![
        Step {
            label: "blue on the east wall (recursion spot)",
            kind: PortalKind::Blue,
            feet: rec.feet,
            yaw: rec.yaw,
            pitch: rec.pitch,
            expect: Expect::Placed { centre: Centre::Near(rec_hit), normal: -rec_dir },
        },
        Step {
            label: "orange 0.6 m beside blue on the same wall (overlap)",
            kind: PortalKind::Orange,
            feet: rec.feet,
            yaw: beside_yaw,
            pitch: beside_pitch,
            expect: Expect::Failed(&[ShotFailReason::Overlap]),
        },
        Step {
            label: "blue at the white/black edge (must bump onto the white)",
            kind: PortalKind::Blue,
            feet: edge.feet,
            yaw: edge_yaw,
            pitch: edge_pitch,
            // 1/512 u placement snap: allow a hair over 32 u.
            expect: Expect::Placed { centre: Centre::WhiteBelowZ(-u(32.0) + 1e-4), normal: Vec3::NEG_X },
        },
        Step {
            label: "orange on the small white insert (too small)",
            kind: PortalKind::Orange,
            feet: panel.feet,
            yaw: panel_yaw,
            pitch: panel_pitch,
            expect: Expect::Failed(&[ShotFailReason::DoesNotFit, ShotFailReason::NotPortalable]),
        },
        Step {
            label: "blue on the black north wall (not portalable)",
            kind: PortalKind::Blue,
            feet: black.feet,
            yaw: black.yaw,
            pitch: black.pitch,
            expect: Expect::Failed(&[ShotFailReason::NotPortalable]),
        },
        Step {
            label: "orange on the pit floor (links the pair)",
            kind: PortalKind::Orange,
            feet: pit.feet,
            yaw: pit_yaw,
            pitch: pit_pitch,
            expect: Expect::Placed { centre: Centre::Any, normal: Vec3::Y },
        },
        Step {
            label: "orange on the ceiling patch",
            kind: PortalKind::Orange,
            feet: ceil.feet,
            yaw: ceil.yaw,
            pitch: ceil.pitch,
            expect: Expect::Placed { centre: Centre::Any, normal: Vec3::NEG_Y },
        },
        Step {
            label: "blue on the 45 degree angled panel",
            kind: PortalKind::Blue,
            feet: ang.feet,
            yaw: ang.yaw,
            pitch: ang.pitch,
            expect: Expect::Placed { centre: Centre::Any, normal: Quat::from_rotation_x(FRAC_PI_4) * Vec3::Y },
        },
        Step {
            label: "blue again from the recursion spot (replaces own blue)",
            kind: PortalKind::Blue,
            feet: rec.feet,
            yaw: rec.yaw,
            pitch: rec.pitch,
            expect: Expect::Placed { centre: Centre::Near(rec_hit), normal: -rec_dir },
        },
    ]
}

/// Script state: which step runs and what the last shot produced.
#[derive(Resource, Default)]
struct Script {
    on: bool,
    done: bool,
    index: usize,
    phase: Phase,
    /// Time the current phase started (seconds).
    t0: f32,
    frames: u32,
    /// Gun portals before the current shot, indexed by colour.
    before: [Option<Entity>; 2],
    placed: Vec<PortalPlaced>,
    failed: Vec<PortalShotFailed>,
    closed: Vec<PortalClosed>,
    fired: Vec<GunFired>,
    passes: u32,
    fails: u32,
}

#[derive(Clone, Copy, Default, PartialEq)]
enum Phase {
    /// Teleport the player to the step and let the colliders settle.
    #[default]
    Settle,
    /// Send the `FirePortal`.
    Fire,
    /// Wait for the outcome messages, then check.
    Wait,
}

fn main() {
    let args = HarnessArgs::from_env();
    let script = args.rest.iter().any(|a| a == "--script");
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(args.window_plugin("p2_gun")))
        .insert_resource(args)
        .insert_resource(P2Settings { set_fixed_timestep: true })
        .add_plugins((
            PhysicsPlugins::default(),
            p2_core::CorePlugin,
            RoomPlugin,
            HarnessPlugin,
            p2_gun::Plugin,
        ))
        .insert_resource(Script { on: script, ..default() })
        .add_systems(Startup, setup)
        .add_systems(Update, (drive_script.after(P2Systems::Fire), draw_portals, follow_camera));
    if script {
        app.insert_resource(InputLocked(true));
    } else {
        app.add_plugins(p2_gun::MouseInputPlugin);
    }
    app.run();
}

fn setup(mut commands: Commands, spots: Res<RoomSpots>) {
    spawn_player(&mut commands, spots.spawn, spots.spawn_yaw);
    commands.spawn((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection { fov: 59f32.to_radians(), ..default() }),
        Transform::default(),
    ));
}

fn v(p: Vec3) -> String {
    format!("({:.3}, {:.3}, {:.3})", p.x, p.y, p.z)
}

/// Yaw and pitch that look from `eye` at `aim` (yaw 0 looks down -Z, positive pitch looks up).
fn look_angles(eye: Vec3, aim: Vec3) -> (f32, f32) {
    let d = aim - eye;
    let yaw = (-d.x).atan2(-d.z);
    let pitch = (d.y / d.length()).asin();
    (yaw, pitch)
}

fn drive_script(
    mut st: ResMut<Script>,
    time: Res<Time>,
    spots: Res<RoomSpots>,
    mut fire: MessageWriter<FirePortal>,
    mut exit: MessageWriter<AppExit>,
    mut placed_r: MessageReader<PortalPlaced>,
    mut failed_r: MessageReader<PortalShotFailed>,
    mut closed_r: MessageReader<PortalClosed>,
    mut fired_r: MessageReader<GunFired>,
    mut player: Query<(Entity, &mut Transform, &mut ViewAngles, &mut InterpolatedFeet, &PortalGun), With<Player>>,
    portals: Query<(&Transform, &Portal, Option<&PortalLink>), Without<Player>>,
) {
    // Messages are collected every frame, whatever the phase.
    st.placed.extend(placed_r.read().copied());
    st.failed.extend(failed_r.read().copied());
    st.closed.extend(closed_r.read().copied());
    st.fired.extend(fired_r.read().copied());
    if !st.on || st.done {
        return;
    }
    let now = time.elapsed_secs();
    let all = steps(&spots);
    let Some(step) = all.get(st.index).copied() else {
        println!("{} passed, {} failed", st.passes, st.fails);
        st.done = true;
        exit.write(if st.fails == 0 { AppExit::Success } else { AppExit::from_code(st.fails.min(255) as u8) });
        return;
    };
    let Ok((gun_e, mut feet, mut view, mut interp, gun)) = player.single_mut() else { return };

    match st.phase {
        Phase::Settle => {
            // Put the player on the spot, look along its yaw and pitch, and hold
            // it there while the physics settles.
            feet.translation = step.feet;
            feet.rotation = Quat::from_rotation_y(step.yaw);
            *view = ViewAngles { yaw: step.yaw, pitch: step.pitch, roll: 0.0 };
            interp.prev = step.feet;
            interp.curr = step.feet;
            st.frames += 1;
            if st.frames >= 10 && now - st.t0 >= 0.45 {
                st.phase = Phase::Fire;
                st.t0 = now;
                st.frames = 0;
            }
        }
        Phase::Fire => {
            st.placed.clear();
            st.failed.clear();
            st.closed.clear();
            st.fired.clear();
            st.before = [gun.portal(PortalKind::Blue), gun.portal(PortalKind::Orange)];
            fire.write(FirePortal { gun: gun_e, kind: step.kind });
            st.phase = Phase::Wait;
            st.t0 = now;
            st.frames = 0;
        }
        Phase::Wait => {
            st.frames += 1;
            if !(st.frames >= 4 && now - st.t0 >= 0.25) {
                return;
            }
            let problems = check(&step, &st, gun, &portals);
            let label = step.label;
            if problems.is_empty() {
                st.passes += 1;
                println!("PASS  {label}: {}", outcome_text(&st, &step));
            } else {
                st.fails += 1;
                println!("FAIL  {label}: {}", problems.join("; "));
            }
            st.index += 1;
            st.phase = Phase::Settle;
            st.t0 = now;
            st.frames = 0;
        }
    }
}

fn outcome_text(st: &Script, step: &Step) -> String {
    match step.expect {
        Expect::Placed { .. } => {
            let p = st.placed.iter().find(|p| p.kind == step.kind).expect("checked");
            format!("portal at {} from shot {}", v(p.transform.translation), v(st.fired[0].to))
        }
        Expect::Failed(_) => {
            let f = st.failed.iter().find(|f| f.kind == step.kind).expect("checked");
            format!("{:?} at {}", f.reason, f.point.map(v).unwrap_or_default())
        }
    }
}

/// Every mismatch for this step (empty = PASS).
fn check(
    step: &Step,
    st: &Script,
    gun: &PortalGun,
    portals: &Query<(&Transform, &Portal, Option<&PortalLink>), Without<Player>>,
) -> Vec<String> {
    let mut problems = Vec::new();
    if st.fired.len() != 1 {
        problems.push(format!("expected one GunFired, got {}", st.fired.len()));
    }
    // The Portal entities in the world must match what the gun reports.
    for (kind, name) in [(PortalKind::Blue, "blue"), (PortalKind::Orange, "orange")] {
        let live = portals.iter().filter(|(_, p, _)| p.kind == kind).count();
        let want = gun.portal(kind).is_some() as usize;
        if live != want {
            problems.push(format!("{name}: {live} Portal entities in the world, gun expects {want}"));
        }
    }
    match step.expect {
        Expect::Placed { centre, normal } => {
            let Some(p) = st.placed.iter().find(|p| p.kind == step.kind).copied() else {
                let why = st.failed.first().map(|f| format!("{:?}", f.reason)).unwrap_or_else(|| "nothing".into());
                problems.push(format!("no portal opened (got {why})"));
                return problems;
            };
            let Ok((tf, portal, link)) = portals.get(p.portal) else {
                problems.push("PortalPlaced named an entity that does not exist".into());
                return problems;
            };
            let frame = Portal::frame(tf);
            match centre {
                Centre::Near(c) => {
                    if (tf.translation - c).length() > 2.0 {
                        problems.push(format!("centre {} is over 2 m from the view ray hit {}", v(tf.translation), v(c)));
                    }
                }
                Centre::WhiteBelowZ(z) => {
                    if tf.translation.z > z {
                        problems.push(format!("centre {} is within 32 u of the black edge", v(tf.translation)));
                    }
                }
                Centre::Any => {}
            }
            if frame.normal().dot(normal) < 0.999 {
                problems.push(format!("normal {} != expected {}", v(frame.normal()), v(normal)));
            }
            if portal.kind != step.kind {
                problems.push("portal has the wrong colour".into());
            }
            if gun.portal(step.kind) != Some(p.portal) {
                problems.push("gun does not point at the new portal".into());
            }
            // The pair is linked exactly when the other colour is open.
            let partner = gun.portal(step.kind.other());
            match (partner, link) {
                (Some(q), Some(l)) if l.0 == q => {
                    let back = portals.get(q).ok().and_then(|(_, _, l2)| l2.map(|l| l.0));
                    if back != Some(p.portal) {
                        problems.push("partner does not link back".into());
                    }
                }
                (Some(_), _) => problems.push("expected the pair to be linked".into()),
                (None, Some(_)) => problems.push("unexpected link on a lone portal".into()),
                (None, None) => {}
            }
            // A shot replaces the gun's old portal of the same colour, if any.
            let replaces = st.before[step.kind.index()].is_some();
            let closed = st.closed.iter().any(|c| c.kind == step.kind);
            if replaces != closed {
                problems.push(format!("replacement close message: expected {replaces}, got {closed}"));
            }
            if replaces {
                if let Some(old) = st.before[step.kind.index()] {
                    if portals.get(old).is_ok() {
                        problems.push("old same-colour portal was not despawned".into());
                    }
                }
            }
        }
        Expect::Failed(reasons) => {
            let Some(f) = st.failed.iter().find(|f| f.kind == step.kind) else {
                problems.push(format!("no failure reported (portal opened: {})", st.placed.len()));
                return problems;
            };
            if !reasons.contains(&f.reason) {
                problems.push(format!("reason {:?} not in expected {:?}", f.reason, reasons));
            }
            if gun.portal(step.kind) != st.before[step.kind.index()] {
                problems.push("a failed shot changed the gun's portal".into());
            }
        }
    }
    problems
}

/// Gizmo ellipses for each placed portal: bright when linked, dim when lone.
fn draw_portals(mut gizmos: Gizmos, portals: Query<(&Portal, &Transform, Option<&PortalLink>)>) {
    for (portal, tf, link) in &portals {
        let color = match portal.kind {
            PortalKind::Blue => Color::srgb(0.0, 0.25, 1.0),
            PortalKind::Orange => Color::srgb(0.95, 0.31, 0.01),
        };
        let alpha = if link.is_some() { 1.0 } else { 0.4 };
        let frame = Portal::frame(tf);
        let centre = frame.position + frame.normal() * 0.01;
        gizmos.ellipse(Isometry3d::new(centre, tf.rotation), portal.half_size, color.with_alpha(alpha));
    }
}

/// Keeps the camera at the player's eye, looking where the player looks.
fn follow_camera(
    mut cams: Query<&mut Transform, (With<Camera3d>, Without<Player>)>,
    players: Query<(&Transform, &ViewAngles, &Crouch), (With<Player>, Without<Camera3d>)>,
) {
    let Ok((feet, view, crouch)) = players.single() else { return };
    let Ok(mut cam) = cams.single_mut() else { return };
    cam.translation = feet.translation + Vec3::Y * crouch.eye_height;
    cam.rotation = view.rotation();
}

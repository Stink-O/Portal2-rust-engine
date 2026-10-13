//! Portal views and surfaces in the greybox test room.
//!
//! ```text
//! cargo run -p p2_render --example render -- [--preset facing|floor] [--pos X Y Z] [--look YAW PITCH]
//!     [--near DIST ANGLE] [--age SECS] [--unlinked] [--depth N] [--screenshot out.png] [--frames N]
//! ```
//!
//! * `--preset facing`: blue on the west wall, orange on the east wall, facing
//!   each other (recursion). `floor`: blue on the west wall, orange on the floor.
//! * `--look YAW PITCH` (degrees): yaw 0 looks down -Z, +90 looks west (-X).
//! * `--near DIST ANGLE`: put the camera `DIST` metres in front of blue, looking
//!   at its centre at `ANGLE` degrees to the plane (90 = straight on).
//! * `--age SECS`: pretend the portals were placed `SECS` ago (open animation).
//! * `--unlinked`: do not link the pair.
//! * `--depth N`: recursion depth (default 2).
//!
//! The harness options (`--screenshot`, `--frames`, `--shot`, `--exit-after`,
//! `--size`) come from `p2_room`.
use avian3d::prelude::*;
use bevy::camera::Hdr;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::prelude::*;
use p2_core::math::PortalFrame;
use p2_core::{spawn_player, CorePlugin, MainCamera, P2Settings, Portal, PortalKind, PortalLink, PortalViewCamera, PORTAL_HALF_SIZE};
use p2_render::PortalRenderConfig;
use p2_room::{HarnessArgs, HarnessPlugin, RoomPlugin};

/// Blue's centre on the west wall, facing +X.
const BLUE_WALL: Vec3 = Vec3::new(-11.99, 1.5, 0.0);
/// Orange's centre on the east wall, facing -X.
const ORANGE_WALL: Vec3 = Vec3::new(11.99, 1.5, 0.0);
/// Orange's centre on the floor, facing +Y.
const ORANGE_FLOOR: Vec3 = Vec3::new(6.0, 0.0, 0.0);

#[derive(Resource, Clone)]
struct Setup {
    preset: String,
    pos: Vec3,
    yaw: f32,
    pitch: f32,
    near: Option<(f32, f32)>,
    age: f32,
    unlinked: bool,
    single: bool,
}

fn parse(rest: &[String]) -> Setup {
    let mut s = Setup {
        preset: "facing".into(),
        pos: Vec3::new(2.0, 1.6, 0.0),
        yaw: 90f32.to_radians(),
        pitch: 0.0,
        near: None,
        age: 0.0,
        unlinked: false,
        single: false,
    };
    let mut i = 0;
    let nums = |i: usize, n: usize| -> Vec<f32> {
        (1..=n).filter_map(|k| rest.get(i + k).and_then(|x| x.parse().ok())).collect()
    };
    while i < rest.len() {
        match rest[i].as_str() {
            "--preset" => s.preset = rest.get(i + 1).cloned().unwrap_or_default(),
            "--pos" => {
                let v = nums(i, 3);
                if v.len() == 3 {
                    s.pos = Vec3::new(v[0], v[1], v[2]);
                }
            }
            "--look" => {
                let v = nums(i, 2);
                if v.len() == 2 {
                    s.yaw = v[0].to_radians();
                    s.pitch = v[1].to_radians();
                }
            }
            "--near" => {
                let v = nums(i, 2);
                if v.len() == 2 {
                    s.near = Some((v[0], v[1]));
                }
            }
            "--age" => s.age = nums(i, 1).first().copied().unwrap_or(0.0),
            "--unlinked" => s.unlinked = true,
            "--single" => s.single = true,
            _ => {}
        }
        i += 1;
    }
    s
}

fn main() -> AppExit {
    let args = HarnessArgs::from_env();
    let setup = parse(&args.rest);
    let mut config = PortalRenderConfig::default();
    let mut it = args.rest.iter();
    while let Some(a) = it.next() {
        if a == "--depth" {
            config.recursion_depth = it.next().and_then(|v| v.parse().ok()).unwrap_or(2);
        }
    }

    App::new()
        .add_plugins(DefaultPlugins.set(args.window_plugin("p2_render")))
        .insert_resource(args)
        .insert_resource(setup.clone())
        // Portal 2's 60 Hz tick, opted in before CorePlugin reads it.
        .insert_resource(P2Settings { set_fixed_timestep: true })
        .add_plugins((
            PhysicsPlugins::default(),
            CorePlugin,
            RoomPlugin,
            HarnessPlugin,
            p2_assets::AssetsPlugin,
            p2_render::Plugin { config },
        ))
        .add_systems(Startup, spawn_scene)
        .add_systems(Update, log_state.run_if(|mut n: Local<u32>| { *n += 1; *n == 5 }))
        .run()
}

fn spawn_scene(mut commands: Commands, time: Res<Time>, setup: Res<Setup>) {
    let now = time.elapsed_secs();
    let (blue_pos, blue_n, orange_pos, orange_n) = match setup.preset.as_str() {
        "floor" => (BLUE_WALL, Vec3::X, ORANGE_FLOOR, Vec3::Y),
        _ => (BLUE_WALL, Vec3::X, ORANGE_WALL, Vec3::NEG_X),
    };
    let up = Vec3::Y;
    let blue = PortalFrame::from_normal_up(blue_pos, blue_n, up).expect("frame");
    let orange = PortalFrame::from_normal_up(orange_pos, orange_n, if orange_n == Vec3::Y { Vec3::NEG_Z } else { up })
        .expect("frame");

    let spawn_portal = |commands: &mut Commands, kind: PortalKind, f: &PortalFrame| {
        commands
            .spawn((
                Name::new(format!("{kind:?} portal")),
                Portal { kind, half_size: PORTAL_HALF_SIZE, surfaces: vec![], placed_at: now - setup.age },
                Transform::from_translation(f.position).with_rotation(f.rotation),
            ))
            .id()
    };
    let b = spawn_portal(&mut commands, PortalKind::Blue, &blue);
    if !setup.single {
        let o = spawn_portal(&mut commands, PortalKind::Orange, &orange);
        if !setup.unlinked {
            commands.entity(b).insert(PortalLink(o));
            commands.entity(o).insert(PortalLink(b));
        }
    }

    // Camera: either a point in front of blue, or the given pos and look.
    let (pos, rot) = match setup.near {
        Some((dist, angle_deg)) => {
            let n = blue.normal();
            let offset_t = dist / angle_deg.to_radians().tan();
            let tangent = Vec3::Z * if angle_deg < 90.0 { 1.0 } else { 0.0 };
            let pos = blue_pos + n * dist + tangent * offset_t;
            let rot = Transform::from_translation(pos).looking_at(blue_pos, Vec3::Y).rotation;
            (pos, rot)
        }
        None => (setup.pos, Quat::from_euler(EulerRot::YXZ, setup.yaw, setup.pitch, 0.0)),
    };
    spawn_player(&mut commands, Vec3::new(pos.x, 0.0, pos.z), setup.yaw);
    commands.spawn((
        Name::new("main camera"),
        Camera3d::default(),
        Hdr,
        Tonemapping::default(),
        Projection::Perspective(PerspectiveProjection { fov: 59f32.to_radians(), near: 0.1, ..default() }),
        Transform::from_translation(pos).with_rotation(rot),
        MainCamera,
    ));
}

fn log_state(
    portals: Query<(&Portal, &p2_core::PortalAnimation, Option<&PortalLink>)>,
    views: Query<&PortalViewCamera>,
    tree: Res<p2_render::ViewTree>,
) {
    for (p, a, link) in &portals {
        info!("{:?} open {:.3} static {:.3} linked {}", p.kind, a.open_amount, a.static_amount, link.is_some());
    }
    info!("view cameras in tree: {} (components on cams: {})", tree.active_views(), views.iter().count());
}


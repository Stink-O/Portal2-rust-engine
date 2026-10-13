//! Chell's body and its ghosts across a wall portal and a floor portal.
//!
//! The rendering crate is not ready, so this uses a debug third-person camera
//! that draws layer 0 and the body layer. Portals are static pairs spawned
//! here (blue/orange translucent quads show the openings).
//!
//! ```text
//! cargo run -p p2_body --example body -- --case idle  --screenshot idle.png
//! cargo run -p p2_body --example body -- --case run   --screenshot run.png
//! cargo run -p p2_body --example body -- --case wall  --cam 9 1.4 3 --target 11.9 1.2 0 --screenshot wall_exit.png
//! cargo run -p p2_body --example body -- --case wall  --cam -7 1.4 2.5 --target -13 1.2 0 --screenshot wall_entry.png
//! cargo run -p p2_body --example body -- --case wall --portal-view --cam 9 1.4 1.5 --target 13 1.1 0 --screenshot seam.png
//! cargo run -p p2_body --example body -- --case floor --cam 4 2.5 0 --target 4 0.8 -7 --screenshot floor.png
//! ```
//! Extra flags: `--cam X Y Z`,
//! `--target X Y Z`, `--pitch DEG` (view pitch, + up), `--crouch`,
//! `--feet X Y Z` and `--yaw DEG` (override the case's player placement),
//! `--walk X Y Z` (move the player at this velocity, m/s, every frame),
//! `--side` (the wall pair side by side on the west wall, both facing +X:
//! blue at z = -1.5, orange at z = +1.5), `--corner` (blue on the west wall
//! at z = -5, orange on the north wall at x = -5, facing +Z),
//! `--first-person` (with `--portal-view`: the camera rides at the player's
//! eye, through a portal like `p2_view`, and `p2_render` applies the
//! first-person body and ghost rules; without it, third person: all drawn). The harness flags (`--screenshot`, `--frames`, `--shot`,
//! `--exit-after`, `--size`) are shared with the other p2 examples.
use avian3d::prelude::PhysicsPlugins;
use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use p2_body::{BodyConfig, Ghostable};
use p2_core::{
    spawn_player, CharacterVelocity, InterpolatedFeet, P2Settings, Portal, PortalFrame, PortalKind, PortalLink,
    ViewAngles, LAYER_PLAYER_BODY, PORTAL_HALF_SIZE,
};
use p2_room::{HarnessArgs, HarnessPlugin, RoomPlugin};

/// What the player does in each case: feet, yaw, world velocity.
struct Case {
    feet: Vec3,
    yaw: f32,
    velocity: Vec3,
    /// Whether a ghost is expected (checked by the scripted asserts).
    expect_ghost: bool,
}

fn case_named(name: &str) -> Case {
    match name {
        // Yaw pi/2 faces -X: towards the west wall portal.
        "wall" => Case { feet: Vec3::new(-12.9, 0.0, 0.0), yaw: std::f32::consts::FRAC_PI_2, velocity: Vec3::ZERO, expect_ghost: true },
        // Feet 0.4 m below the floor portal: the legs are passing through it.
        "floor" => Case { feet: Vec3::new(4.0, -0.4, -4.0), yaw: 0.0, velocity: Vec3::ZERO, expect_ghost: true },
        "run" => Case { feet: Vec3::new(-3.0, 0.0, 2.0), yaw: 0.0, velocity: Vec3::new(0.0, 0.0, -4.0), expect_ghost: false },
        _ => Case { feet: Vec3::new(-3.0, 0.0, 0.0), yaw: 0.0, velocity: Vec3::ZERO, expect_ghost: false },
    }
}

#[derive(Resource)]
struct Setup {
    case: Case,
    name: String,
    pitch: f32,
    crouch: bool,
}

fn main() -> AppExit {
    let args = HarnessArgs::from_env();
    let mut name = "idle".to_string();
    let mut cam = Vec3::new(-6.0, 1.6, 4.0);
    let mut target = Vec3::new(-3.0, 1.0, 0.0);
    let mut portal_view = false;
    let mut side = "";
    let mut pitch = 0.0;
    let mut crouch = false;
    let mut feet = None;
    let mut first_person = false;
    let mut walk = None;
    let mut yaw = None;
    let mut it = args.rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--case" => name = it.next().cloned().unwrap_or_default(),
            "--cam" => cam = read_vec3(&mut it),
            "--target" => target = read_vec3(&mut it),
            "--portal-view" => portal_view = true,
            "--side" => side = "side",
            "--corner" => side = "corner",
            "--pitch" => pitch = it.next().and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0),
            "--crouch" => crouch = true,
            "--first-person" => first_person = true,
            "--walk" => walk = Some(read_vec3(&mut it)),
            "--feet" => feet = Some(read_vec3(&mut it)),
            "--yaw" => yaw = it.next().and_then(|v| v.parse::<f32>().ok()).map(f32::to_radians),
            _ => {}
        }
    }
    let mut case = case_named(&name);
    if let Some(f) = feet {
        case.feet = f;
        // A free placement has no scripted expectation.
        name = format!("{name} (free)");
    }
    if let Some(y) = yaw {
        case.yaw = y;
    }
    if let Some(v) = walk {
        case.velocity = v;
        name = format!("{name} (free)");
    }
    App::new()
        .add_plugins(DefaultPlugins.set(args.window_plugin("p2_body")))
        .insert_resource(args)
        .insert_resource(BodyConfig::default())
        .insert_resource(P2Settings { set_fixed_timestep: true })
        .add_plugins((
            PhysicsPlugins::default(),
            p2_core::CorePlugin,
            p2_assets::AssetsPlugin,
            RoomPlugin,
            HarnessPlugin,
            p2_body::Plugin,
        ))
        .insert_resource(Setup { case, name, pitch, crouch })
        .insert_resource(PortalView(portal_view))
        .insert_resource(SideBySide(side))
        .add_systems(Startup, (spawn_portals, spawn_player_for_case, move |mut c: Commands, pv: Res<PortalView>| spawn_camera(&mut c, cam, target, pv.0, first_person)))
        .add_systems(Update, check_ghosts)
        .add_systems(Update, walk_player.run_if(move || walk.is_some()))
        .add_systems(Update, follow_eye.after(walk_player).run_if(move || first_person))
        .add_plugins(p2_render::Plugin::default())
        .run()
}

fn read_vec3<'a>(it: &mut impl Iterator<Item = &'a String>) -> Vec3 {
    let mut f = || it.next().and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
    Vec3::new(f(), f(), f())
}

/// Two linked pairs: a wall pair (west wall to east wall) and a floor pair
/// (floor pit to the south wall). Blue is the entry side of each pair.
fn spawn_portals(
    pv: Res<PortalView>,
    side: Res<SideBySide>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let wall_a = PortalFrame::from_normal_up(Vec3::new(-12.9, 1.5, 0.0), Vec3::X, Vec3::Y).unwrap();
    let wall_b = PortalFrame::from_normal_up(Vec3::new(12.9, 1.5, 0.0), Vec3::NEG_X, Vec3::Y).unwrap();
    // `--side`: both on the west wall facing +X, blue at z = -1.5, orange at z = +1.5.
    // `--corner`: blue on the west wall at z = -5, orange on the north wall at x = -5.
    let west = |z: f32| PortalFrame::from_normal_up(Vec3::new(-12.9, 1.5, z), Vec3::X, Vec3::Y).unwrap();
    let (wall_a, wall_b) = match side.0 {
        "side" => (west(-1.5), west(1.5)),
        "corner" => (west(-5.0), PortalFrame::from_normal_up(Vec3::new(-5.0, 1.5, -12.9), Vec3::Z, Vec3::Y).unwrap()),
        _ => (wall_a, wall_b),
    };
    let floor_f = PortalFrame::from_normal_up(Vec3::new(4.0, 0.0, -4.0), Vec3::Y, Vec3::NEG_Z).unwrap();
    let floor_w = PortalFrame::from_normal_up(Vec3::new(4.0, 1.5, -12.0), Vec3::Z, Vec3::Y).unwrap();
    // The portal renderer handles one pair (Portal 2 single player), so the
    // portal-view mode spawns only the wall pair.
    let pairs: &[(PortalFrame, PortalFrame)] = if pv.0 { &[(wall_a, wall_b)] } else { &[(wall_a, wall_b), (floor_f, floor_w)] };
    for &(a, b) in pairs {
        let (ea, eb) = (
            spawn_portal(&mut commands, &mut meshes, &mut materials, PortalKind::Blue, a, pv.0),
            spawn_portal(&mut commands, &mut meshes, &mut materials, PortalKind::Orange, b, pv.0),
        );
        commands.entity(ea).insert(PortalLink(eb));
        commands.entity(eb).insert(PortalLink(ea));
    }
}

fn spawn_portal(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    kind: PortalKind,
    frame: PortalFrame,
    portal_view: bool,
) -> Entity {
    let rgb = kind.rgb255() / 255.0;
    let quad = materials.add(StandardMaterial {
        base_color: Color::srgba(rgb.x, rgb.y, rgb.z, 0.3),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        cull_mode: None,
        ..default()
    });
    let mut e = commands.spawn((
        Name::new(format!("portal {kind:?}")),
        Portal { kind, half_size: PORTAL_HALF_SIZE, surfaces: Vec::new(), placed_at: 0.0 },
        Transform::from_translation(frame.position).with_rotation(frame.rotation),
    ));
    // Without the portal renderer, a translucent quad marks the opening.
    if !portal_view {
        e.insert((Mesh3d(meshes.add(Mesh::from(Plane3d::new(Vec3::Z, PORTAL_HALF_SIZE)))), MeshMaterial3d(quad)));
    }
    e.id()
}

fn spawn_player_for_case(mut commands: Commands, setup: Res<Setup>) {
    let c = &setup.case;
    let e = spawn_player(&mut commands, c.feet, c.yaw);
    let view = ViewAngles { yaw: c.yaw, pitch: setup.pitch.to_radians(), roll: 0.0 };
    // No movement plugin here: say she stands on the floor, or she would play
    // the in-air animation (the floor case is half through a floor portal).
    let grounded = p2_core::Grounded { on_ground: setup.name != "floor", normal: Vec3::Y, entity: None };
    commands.entity(e).insert((CharacterVelocity(c.velocity), InterpolatedFeet { prev: c.feet, curr: c.feet }, view, grounded));
    if setup.crouch {
        commands.entity(e).insert(p2_core::Crouch { ducked: true, wants_duck: true, eye_height: p2_core::VIEW_DUCK });
    }
}

/// `--walk`: moves the player (feet, interpolation, transform) at its
/// velocity, one 60 Hz step per frame (so screenshots by frame are repeatable).
fn walk_player(mut q: Query<(&mut Transform, &mut InterpolatedFeet, &CharacterVelocity), With<p2_core::Player>>) {
    for (mut tf, mut feet, vel) in &mut q {
        feet.prev = feet.curr;
        feet.curr += vel.0 / 60.0;
        tf.translation = feet.curr;
    }
}

/// Debug camera: layer 0 (the room) and the body layer, which the main
/// first-person camera never sees.
/// `--portal-view`: the camera is also the portal renderer's main camera, so
/// portal views (which draw the body layer) show the other half of the body.
#[derive(Resource)]
struct PortalView(bool);

/// `--side` / `--corner`: where the wall pair sits (empty: facing walls).
#[derive(Resource)]
struct SideBySide(&'static str);

#[derive(Component)]
struct DebugCamera;

/// `--first-person`: the camera rides at the player's standing eye (64 u above
/// the feet), keeping the direction from `--cam` to `--target`.
/// Like `p2_view`, an eye just behind a portal plane renders from the mapped
/// pose and sets [`p2_core::LocalEye::through`].
fn follow_eye(
    players: Query<&Transform, With<p2_core::Player>>,
    mut cams: Query<&mut Transform, (With<DebugCamera>, Without<p2_core::Player>, Without<Portal>)>,
    portals: Query<(&Transform, &Portal, &PortalLink)>,
    exits: Query<&Transform, With<Portal>>,
    look: Res<EyeLook>,
    mut local_eye: ResMut<p2_core::LocalEye>,
) {
    let Some(p) = players.iter().next() else { return };
    let eye = Transform::from_translation(p.translation + Vec3::Y * p2_core::VIEW_STAND).with_rotation(look.0);
    let linked = portals.iter().filter_map(|(tf, portal, l)| exits.get(l.0).ok().map(|x| (tf, portal, l.0, x)));
    let through = p2_core::eye_through_portal(eye, linked);
    local_eye.through = through.map(|(_, exit)| exit);
    for mut tf in &mut cams {
        *tf = through.map_or(eye, |(t, _)| t);
    }
}

/// The `--cam`/`--target` look direction the first-person eye keeps.
#[derive(Resource)]
struct EyeLook(Quat);

fn spawn_camera(commands: &mut Commands, cam: Vec3, target: Vec3, portal_view: bool, first_person: bool) {
    let layers = if first_person {
        RenderLayers::layer(0)
    } else {
        RenderLayers::from_layers(&[0, LAYER_PLAYER_BODY, p2_core::LAYER_PLAYER_GHOST, p2_core::LAYER_PLAYER_GHOST_ORANGE])
    };
    let look = Transform::from_translation(cam).looking_at(target, Vec3::Y);
    commands.insert_resource(EyeLook(look.rotation));
    commands.insert_resource(p2_core::LocalEye { third_person: !first_person, through: None });
    let mut e = commands.spawn((
        Name::new("debug camera"),
        DebugCamera,
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection { fov: 59f32.to_radians(), ..default() }),
        layers,
        Transform::from_translation(cam).looking_at(target, Vec3::Y),
    ));
    if portal_view {
        e.insert((p2_core::MainCamera, bevy::camera::Hdr, bevy::core_pipeline::tonemapping::Tonemapping::None));
    }
}

/// Scripted checks, once the body and its ghosts have been built (frame 30).
/// A failed check panics: the run is not a pass.
fn check_ghosts(
    mut frame: Local<u32>,
    setup: Res<Setup>,
    ghosts: Query<(&Name, &Transform)>,
    bodies: Query<(&Name, &Transform), With<Ghostable>>,
) {
    *frame += 1;
    let free = setup.name.ends_with("(free)");
    if *frame != 30 && !(free && *frame % 10 == 0) {
        return;
    }
    let ghost_roots: Vec<Vec3> = ghosts.iter().filter(|(n, _)| n.as_str() == "ghost root").map(|(_, t)| t.translation).collect();
    let body = bodies.iter().next().map(|(_, t)| t.translation);
    info!("case {} frame {}: body {:?}, ghost roots {:?}", setup.name, *frame, body, ghost_roots);
    if free {
        return;
    }
    if setup.case.expect_ghost {
        assert_eq!(ghost_roots.len(), 1, "case {}: expected one ghost", setup.name);
    } else {
        assert!(ghost_roots.is_empty(), "case {}: expected no ghost, got {:?}", setup.name, ghost_roots);
    }
    // The ghost sits at its exit portal: wall exit at x = 13 (the east wall,
    // room side is -X), floor exit at z = -12 (room side is +Z).
    match setup.name.as_str() {
        "wall" => assert!(ghost_roots.iter().all(|g| g.x > 11.0 && g.x < 13.5), "wall ghost {ghost_roots:?}"),
        "floor" => assert!(ghost_roots.iter().all(|g| g.z > -12.5 && g.z < -10.0), "floor ghost {ghost_roots:?}"),
        _ => {}
    }
}


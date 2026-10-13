//! Fly around a Portal 2 map.
//!
//! cargo run -p source_assets --example view -- sp_a1_intro3
//!   [--screenshot out.png] [--frames N] [--cam x,y,z,yaw,pitch] [--light-scale S]
//!   [--collision] [--offset x,y,z]
//!
//! WASD + mouse (hold right button or press Tab to capture), Space/Ctrl up/down,
//! Shift fast. `--cam` is world metres and degrees relative to the map origin.
//! `--collision` tints portalable colliders blue and the rest red (debug gizmos).

use avian3d::prelude::*;
use bevy::core_pipeline::Skybox;
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use bevy::window::{CursorGrabMode, CursorOptions};
use source_assets::{spawn_map, SourceCollider, SourceMapAssets, SourceMapPlugin, SourceRenderSettings};

#[derive(Resource, Clone)]
struct Args {
    map: String,
    screenshot: Option<String>,
    frames: u32,
    cam: Option<[f32; 5]>,
    /// Source-space pose like the game's `setpos x y z; setang pitch yaw`:
    /// player origin (feet); the eye is 64 units above it.
    setpos: Option<[f32; 5]>,
    light_scale: Option<f32>,
    collision: bool,
    offset: Vec3,
}

fn parse_args() -> Args {
    let mut a = Args {
        map: "sp_a1_intro3".into(),
        screenshot: None,
        frames: 30,
        cam: None,
        setpos: None,
        light_scale: None,
        collision: false,
        offset: Vec3::ZERO,
    };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    let floats = |s: &str| -> Vec<f32> { s.split(',').filter_map(|v| v.trim().parse().ok()).collect() };
    while i < argv.len() {
        match argv[i].as_str() {
            "--screenshot" => {
                i += 1;
                a.screenshot = argv.get(i).cloned();
            }
            "--frames" => {
                i += 1;
                a.frames = argv.get(i).and_then(|s| s.parse().ok()).unwrap_or(30);
            }
            "--cam" => {
                i += 1;
                let v = floats(argv.get(i).map(String::as_str).unwrap_or(""));
                if v.len() >= 3 {
                    a.cam = Some([v[0], v[1], v[2], *v.get(3).unwrap_or(&0.0), *v.get(4).unwrap_or(&0.0)]);
                }
            }
            "--setpos" => {
                i += 1;
                let v: Vec<f32> = argv
                    .get(i)
                    .map(String::as_str)
                    .unwrap_or("")
                    .split([',', ' '])
                    .filter_map(|v| v.trim().parse().ok())
                    .collect();
                if v.len() >= 5 {
                    a.setpos = Some([v[0], v[1], v[2], v[3], v[4]]);
                }
            }
            "--light-scale" => {
                i += 1;
                a.light_scale = argv.get(i).and_then(|s| s.parse().ok());
            }
            "--offset" => {
                i += 1;
                let v = floats(argv.get(i).map(String::as_str).unwrap_or(""));
                if v.len() == 3 {
                    a.offset = Vec3::new(v[0], v[1], v[2]);
                }
            }
            "--collision" => a.collision = true,
            s if !s.starts_with("--") => a.map = s.to_string(),
            s => eprintln!("unknown argument {s}"),
        }
        i += 1;
    }
    a
}

#[derive(Component)]
struct FlyCam {
    yaw: f32,
    pitch: f32,
}

fn main() {
    let args = parse_args();
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: format!("source_assets: {}", args.map),
            resolution: (1600, 900).into(),
            ..default()
        }),
        ..default()
    }))
    .add_plugins(PhysicsPlugins::default())
    .add_plugins(SourceMapPlugin::default())
    .insert_resource(args.clone())
    .insert_resource(ClearColor(Color::BLACK))
    .add_systems(Startup, setup)
    .add_systems(Update, (fly, grab, screenshot));
    app.insert_resource(SourceRenderSettings {
        light_scale: args.light_scale.unwrap_or(source_assets::DEFAULT_LIGHT_SCALE),
        fullbright: std::env::var_os("SOURCE_FULLBRIGHT").is_some(),
        valve_shaders: std::env::var_os("SOURCE_NO_VALVE").is_none(),
    });
    app.insert_resource(source_assets::valve_render::ValveExposure(
        args.light_scale.unwrap_or(source_assets::DEFAULT_LIGHT_SCALE),
    ));
    if args.collision {
        app.add_plugins(PhysicsDebugPlugin)
            .add_systems(PostStartup, tint_colliders);
    }
    app.run();
}

fn setup(mut commands: Commands, mut assets: SourceMapAssets, args: Res<Args>, mut physics_time: ResMut<Time<Physics>>) {
    physics_time.pause();
    let info = match spawn_map(&mut commands, &mut assets, &args.map, args.offset) {
        Ok(i) => i,
        Err(e) => {
            error!("{e}");
            std::process::exit(1);
        }
    };
    println!(
        "{}: converted in {:.2}s; {} entities; player start {:?}; bounds {:?}; sky {:?}",
        info.name,
        info.convert_seconds,
        info.entities.len(),
        info.player_start.translation,
        info.bounds,
        info.skyname
    );
    let (pos, yaw, pitch) = match (args.setpos, args.cam) {
        (Some(p), _) => {
            let eye = shared::source_to_world(Vec3::new(p[0], p[1], p[2] + 64.0)) + args.offset;
            let rot = source_assets::entities::source_view_to_world(Vec3::new(p[3], p[4], 0.0));
            let (yaw, pitch, _) = rot.to_euler(EulerRot::YXZ);
            (eye, yaw, pitch)
        }
        (None, cam) => match cam {
        Some(c) => (Vec3::new(c[0], c[1], c[2]) + args.offset, c[3].to_radians(), c[4].to_radians()),
        None => {
            let (yaw, pitch, _) = info.arrival.rotation.to_euler(EulerRot::YXZ);
            (info.arrival.translation + Vec3::Y * 64.0 * shared::SOURCE_UNIT, yaw, pitch)
        }
        },
    };
    let mut cam = commands.spawn((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            // Portal 2: fov_desired defaults to 75, defined for 4:3 and
            // widened for 16:9, i.e. vertical FOV = 2*atan(tan(37.5 deg) * 3/4).
            fov: 2.0 * (37.5f32.to_radians().tan() * 0.75).atan(),
            near: 0.02,
            ..default()
        }),
        Transform::from_translation(pos).with_rotation(Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0)),
        FlyCam { yaw, pitch },
    ));
    // Portal 2 fogs its (white) 2D sky into the fog colour; with fog on,
    // the clear colour stands in for the sky.
    match (&info.fog, info.skybox.clone()) {
        (Some(fog), _) => {
            cam.insert(fog.distance_fog());
            commands.insert_resource(ClearColor(fog.color));
        }
        (None, Some(sky)) => {
            cam.insert(Skybox { image: Some(sky), brightness: 1000.0, ..default() });
        }
        _ => {}
    }
}

fn tint_colliders(mut commands: Commands, q: Query<(Entity, &SourceCollider, Has<shared::Portalable>)>) {
    for (e, _, portalable) in &q {
        let c = if portalable { Color::srgb(0.2, 0.5, 1.0) } else { Color::srgb(1.0, 0.25, 0.2) };
        commands.entity(e).insert(DebugRender::default().with_collider_color(c));
    }
}

fn grab(
    mut cursor: Single<&mut CursorOptions>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
) {
    if keys.just_pressed(KeyCode::Tab) || mouse.just_pressed(MouseButton::Right) {
        let grabbed = cursor.grab_mode != CursorGrabMode::None;
        cursor.grab_mode = if grabbed { CursorGrabMode::None } else { CursorGrabMode::Locked };
        cursor.visible = grabbed;
    }
    if keys.just_pressed(KeyCode::Escape) {
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
    }
}

fn fly(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    motion: Res<AccumulatedMouseMotion>,
    cursor: Single<&CursorOptions>,
    mut q: Query<(&mut Transform, &mut FlyCam)>,
) {
    let Ok((mut t, mut fc)) = q.single_mut() else { return };
    if cursor.grab_mode != CursorGrabMode::None {
        fc.yaw -= motion.delta.x * 0.002;
        fc.pitch = (fc.pitch - motion.delta.y * 0.002).clamp(-1.55, 1.55);
    }
    t.rotation = Quat::from_euler(EulerRot::YXZ, fc.yaw, fc.pitch, 0.0);
    let mut dir = Vec3::ZERO;
    let fwd = t.forward().as_vec3();
    let right = t.right().as_vec3();
    if keys.pressed(KeyCode::KeyW) {
        dir += fwd;
    }
    if keys.pressed(KeyCode::KeyS) {
        dir -= fwd;
    }
    if keys.pressed(KeyCode::KeyD) {
        dir += right;
    }
    if keys.pressed(KeyCode::KeyA) {
        dir -= right;
    }
    if keys.pressed(KeyCode::Space) {
        dir += Vec3::Y;
    }
    if keys.pressed(KeyCode::ControlLeft) {
        dir -= Vec3::Y;
    }
    let speed = if keys.pressed(KeyCode::ShiftLeft) { 20.0 } else { 5.0 };
    t.translation += dir.normalize_or_zero() * speed * time.delta_secs();
}

fn screenshot(mut commands: Commands, args: Res<Args>, mut frame: Local<u32>, mut exit: MessageWriter<AppExit>) {
    let Some(path) = &args.screenshot else { return };
    *frame += 1;
    if *frame == args.frames {
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path.clone()));
    }
    if *frame == args.frames + 5 {
        println!("saved {path}");
        exit.write(AppExit::Success);
    }
}

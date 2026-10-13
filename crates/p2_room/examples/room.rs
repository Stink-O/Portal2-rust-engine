//! Look at the test chamber from a fixed pose.
//!
//! `cargo run -p p2_room --example room -- [--spot NAME] [--pos X Y Z] [--look YAW_DEG PITCH_DEG] [--screenshot out.png] [--frames N]`
//!
//! `--spot` takes a named place from `RoomSpots` (spawn, recursion, pit_rim,
//! ledge, angled_panel, ceiling, small_panel, edge_bump, black_wall, corridor)
//! and puts the eye 64 u above its feet. `--pos` / `--look` (world metres,
//! degrees) override the pose. `--pos` and `--look` are read after `--spot`.
use avian3d::prelude::*;
use bevy::prelude::*;
use p2_core::VIEW_STAND;
use p2_room::{HarnessArgs, HarnessPlugin, RoomPlugin, RoomSpots};

fn main() {
    let args = HarnessArgs::from_env();
    let mut spot_name: Option<String> = None;
    let mut pos: Option<Vec3> = None;
    let (mut yaw, mut pitch) = (None, None);
    let mut it = args.rest.iter();
    while let Some(a) = it.next() {
        let mut f = || it.next().and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
        match a.as_str() {
            "--spot" => spot_name = it.next().cloned(),
            "--pos" => pos = Some(Vec3::new(f(), f(), f())),
            "--look" => {
                yaw = Some(f().to_radians());
                pitch = Some(f().to_radians());
            }
            _ => {}
        }
    }
    App::new()
        .add_plugins(DefaultPlugins.set(args.window_plugin("p2_room")))
        .insert_resource(args)
        // The example owns the fixed timestep: CorePlugin no longer forces 60 Hz.
        .insert_resource(p2_core::P2Settings { set_fixed_timestep: true })
        .add_plugins((PhysicsPlugins::default(), p2_core::CorePlugin, RoomPlugin, HarnessPlugin))
        .add_systems(Startup, move |mut c: Commands, spots: Res<RoomSpots>| {
            let spot = match spot_name.as_deref() {
                None => None,
                Some("spawn") => Some(p2_room::RoomSpot { feet: spots.spawn, yaw: spots.spawn_yaw, pitch: 0.0 }),
                Some("recursion") => Some(spots.recursion),
                Some("pit_rim") => Some(spots.pit_rim),
                Some("ledge") => Some(spots.ledge),
                Some("angled_panel") => Some(spots.angled_panel),
                Some("ceiling") => Some(spots.ceiling),
                Some("small_panel") => Some(spots.small_panel),
                Some("edge_bump") => Some(spots.edge_bump),
                Some("black_wall") => Some(spots.black_wall),
                Some("corridor") => Some(spots.corridor),
                Some(other) => {
                    warn!("unknown --spot {other}");
                    None
                }
            };
            let (mut eye, mut y, mut p) = (Vec3::new(0.0, 1.63, 2.0), 0.0, 0.0);
            if let Some(s) = spot {
                eye = s.feet + Vec3::Y * VIEW_STAND;
                (y, p) = (s.yaw, s.pitch);
            }
            if let Some(v) = pos {
                eye = v;
            }
            y = yaw.unwrap_or(y);
            p = pitch.unwrap_or(p);
            c.spawn((
                Camera3d::default(),
                Projection::Perspective(PerspectiveProjection { fov: 59f32.to_radians(), ..default() }),
                Transform::from_translation(eye).with_rotation(Quat::from_euler(EulerRot::YXZ, y, p, 0.0)),
            ));
        })
        .run();
}

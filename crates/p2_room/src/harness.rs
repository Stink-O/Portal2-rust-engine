//! Shared harness for every `p2_*` example:
//!
//! * `--screenshot <png>`: save the window at frame `--frames N` (default 90), then exit.
//! * `--shot <frame>:<png>` (repeatable): extra screenshots at given frames.
//! * `--exit-after <frames>`: exit after that many frames even with no screenshot.
//! * `--size WxH`: window size (default 1280x720).
//!
//! Unknown arguments are kept in [`HarnessArgs::rest`] for the example to parse.
//! When any screenshot or exit is requested, [`p2_core::InputLocked`] is set so
//! the user's keyboard/mouse don't disturb the run.
use std::path::PathBuf;

use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot, ScreenshotCaptured};

#[derive(Resource, Clone, Debug, Default)]
pub struct HarnessArgs {
    pub shots: Vec<(u32, PathBuf)>,
    pub exit_after: Option<u32>,
    pub size: Option<UVec2>,
    /// Arguments the harness did not understand, in order.
    pub rest: Vec<String>,
}

impl HarnessArgs {
    pub fn from_env() -> Self {
        let mut a = HarnessArgs::default();
        let mut screenshot = None;
        let mut frames = 90u32;
        let mut it = std::env::args().skip(1);
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--screenshot" => screenshot = it.next().map(PathBuf::from),
                "--frames" => frames = it.next().and_then(|v| v.parse().ok()).unwrap_or(frames),
                "--shot" => {
                    if let Some((f, p)) = it.next().as_deref().and_then(|s| s.split_once(':')) {
                        if let Ok(f) = f.parse() {
                            a.shots.push((f, PathBuf::from(p)));
                        }
                    }
                }
                "--exit-after" => a.exit_after = it.next().and_then(|v| v.parse().ok()),
                "--size" => {
                    a.size = it.next().and_then(|s| {
                        let (w, h) = s.split_once('x')?;
                        Some(UVec2::new(w.parse().ok()?, h.parse().ok()?))
                    })
                }
                _ => a.rest.push(arg),
            }
        }
        if let Some(p) = screenshot {
            a.shots.push((frames, p));
        }
        a
    }

    /// True when the run is scripted (screenshots or a fixed length).
    pub fn automated(&self) -> bool {
        !self.shots.is_empty() || self.exit_after.is_some()
    }

    /// A `WindowPlugin` sized by `--size` (default 1280x720).
    pub fn window_plugin(&self, title: &str) -> WindowPlugin {
        let size = self.size.unwrap_or(UVec2::new(1280, 720));
        WindowPlugin {
            primary_window: Some(Window {
                title: title.into(),
                resolution: (size.x, size.y).into(),
                ..default()
            }),
            ..default()
        }
    }
}

/// Add after `DefaultPlugins`. Reads the command line itself if no
/// [`HarnessArgs`] resource was inserted.
pub struct HarnessPlugin;

impl Plugin for HarnessPlugin {
    fn build(&self, app: &mut App) {
        if !app.world().contains_resource::<HarnessArgs>() {
            app.insert_resource(HarnessArgs::from_env());
        }
        let automated = app.world().resource::<HarnessArgs>().automated();
        if automated {
            app.insert_resource(p2_core::InputLocked(true));
        }
        app.init_resource::<ShotCounter>().add_systems(Last, run_harness);
    }
}

#[derive(Resource, Default)]
struct ShotCounter {
    requested: u32,
    saved: u32,
}

fn run_harness(
    mut frame: Local<u32>,
    args: Res<HarnessArgs>,
    mut commands: Commands,
    mut shots: ResMut<ShotCounter>,
    mut exit: MessageWriter<AppExit>,
) {
    *frame += 1;
    for (f, path) in &args.shots {
        if *f == *frame {
            shots.requested += 1;
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk(path.clone()))
                .observe(|_: On<ScreenshotCaptured>, mut s: ResMut<ShotCounter>| s.saved += 1);
        }
    }
    let last_shot = args.shots.iter().map(|(f, _)| *f).max();
    let done_shots = last_shot.is_some_and(|l| *frame > l && shots.saved >= shots.requested);
    let done_exit = args.exit_after.is_some_and(|n| *frame >= n);
    let timeout = last_shot.is_some_and(|l| *frame > l + 300);
    if (done_shots && args.exit_after.is_none_or(|n| *frame >= n)) || done_exit || timeout {
        exit.write(AppExit::Success);
    }
}

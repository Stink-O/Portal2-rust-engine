//! Shared Portal 2 asset helpers for the `p2_*` crates.
//!
//! Portal 2 is required: nothing from the game is shipped, and without the
//! player's own install [`AssetsPlugin`] logs [`PORTAL2_REQUIRED`] and the app
//! exits with an error before any schedule runs.
//!
//! * [`GameFsRes`]: the mounted Portal 2 filesystem.
//! * [`find_portal2`]: the install check (for a binary's `main`).
//! * [`portal_textures`]: portal surface textures (noise, colour ramps...).
//! * [`anim`]: MDL sequence sampling (any frame, sectioned animations,
//!   attachments) for skinned models (viewmodel, Chell).
use std::sync::Arc;

use bevy::prelude::*;

pub mod anim;
pub mod portal_textures;

pub use source_assets;

/// What a player without Portal 2 is told.
pub const PORTAL2_REQUIRED: &str = "Portal 2 is required. These crates load Portal 2's own models, textures, \
particles and sounds from your copy of the game; nothing from the game is shipped. Install Portal 2 through \
Steam, or set PORTAL2_DIR to the folder that contains `portal2/`.";

/// The user's Portal 2 install, mounted like the engine does. Always present
/// once [`AssetsPlugin`] is in the app (it exits the app when the game is
/// missing); a host may insert its own before adding the plugin.
#[derive(Resource, Clone)]
pub struct GameFsRes(pub Arc<source_assets::GameFs>);

impl GameFsRes {
    pub fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.0.read(path)
    }
}

/// Finds and mounts Portal 2: `PORTAL2_DIR` if set (and it must be right),
/// else every Steam library on this machine (Steam's `libraryfolders.vdf`,
/// native and Flatpak Steam on Linux, the default Steam folders on Windows
/// and macOS). The error is the full message for the player:
/// [`PORTAL2_REQUIRED`] plus where it looked.
pub fn find_portal2() -> Result<source_assets::GameFs, String> {
    let root = match std::env::var_os(source_assets::PORTAL2_DIR_ENV) {
        Some(dir) => {
            let dir = std::path::PathBuf::from(dir);
            if !is_portal2(&dir) {
                return Err(format!(
                    "{PORTAL2_REQUIRED}\n(PORTAL2_DIR={} has no portal2/pak01_dir.vpk)",
                    dir.display()
                ));
            }
            dir
        }
        None => {
            let libraries = steam_libraries();
            match libraries.iter().map(|l| install_dir(l)).find(|d| is_portal2(d)) {
                Some(dir) => dir,
                None => {
                    let looked: Vec<String> = libraries.iter().map(|l| l.display().to_string()).collect();
                    let looked = if looked.is_empty() { "no Steam install found".to_string() } else { looked.join(", ") };
                    return Err(format!("{PORTAL2_REQUIRED}\n(searched Steam libraries: {looked})"));
                }
            }
        }
    };
    let config = source_assets::config::GameConfig::from_root(&root);
    source_assets::GameFs::mount(config).map_err(|e| format!("{PORTAL2_REQUIRED}\n({e})"))
}

/// Portal 2 is Steam app 620.
const PORTAL2_APP_ID: &str = "620";

fn is_portal2(root: &std::path::Path) -> bool {
    root.join("portal2").join("pak01_dir.vpk").is_file()
}

/// The install folder of Portal 2 inside a Steam library: the manifest's
/// `installdir`, else "Portal 2".
fn install_dir(library: &std::path::Path) -> std::path::PathBuf {
    let manifest = library.join("steamapps").join(format!("appmanifest_{PORTAL2_APP_ID}.acf"));
    let name = std::fs::read_to_string(manifest)
        .ok()
        .and_then(|text| vdf_values(&text, "installdir").into_iter().next())
        .unwrap_or_else(|| "Portal 2".to_string());
    library.join("steamapps").join("common").join(name)
}

/// Every Steam library folder on this machine, de-duplicated.
fn steam_libraries() -> Vec<std::path::PathBuf> {
    use std::path::PathBuf;
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from);
    let mut steam_roots: Vec<PathBuf> = Vec::new();
    if let Some(h) = &home {
        for rel in [".steam/steam", ".local/share/Steam", ".var/app/com.valvesoftware.Steam/.local/share/Steam", "Library/Application Support/Steam"] {
            steam_roots.push(h.join(rel));
        }
    }
    for var in ["ProgramFiles(x86)", "ProgramFiles"] {
        if let Some(p) = std::env::var_os(var) {
            steam_roots.push(PathBuf::from(p).join("Steam"));
        }
    }
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |p: PathBuf| {
        if let Ok(p) = p.canonicalize() {
            if !out.contains(&p) {
                out.push(p);
            }
        }
    };
    for root in steam_roots {
        let vdf = root.join("steamapps").join("libraryfolders.vdf");
        if let Ok(text) = std::fs::read_to_string(vdf) {
            for path in vdf_values(&text, "path") {
                push(PathBuf::from(path.replace("\\\\", "\\")));
            }
        }
        push(root);
    }
    out
}

/// Values of every `"key" "value"` pair named `key` in a Valve KeyValues text.
fn vdf_values(text: &str, key: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split('"').skip(1).step_by(2);
            let (k, v) = (parts.next()?, parts.next()?);
            k.eq_ignore_ascii_case(key).then(|| v.to_string())
        })
        .collect()
}

/// Mounts Portal 2 into [`GameFsRes`] (unless the host inserted one) and loads
/// the portal textures.
///
/// Without the game it replaces the app's runner: the app logs
/// [`PORTAL2_REQUIRED`] and `App::run` returns `AppExit::error()` without
/// running a single schedule, so no window opens and no other p2 system runs.
/// Return that from `main` (`fn main() -> AppExit`) for exit status 1. Add
/// the plugin after `DefaultPlugins` (whose winit runner would replace ours);
/// if it is added before, a `PreStartup` system logs and exits instead.
pub struct AssetsPlugin;

impl Plugin for AssetsPlugin {
    fn build(&self, app: &mut App) {
        if !app.world().contains_resource::<GameFsRes>() {
            match find_portal2() {
                Ok(fs) => {
                    app.insert_resource(GameFsRes(Arc::new(fs)));
                }
                Err(msg) => {
                    let m = msg.clone();
                    app.set_runner(move |_| {
                        error!("{m}");
                        AppExit::error()
                    });
                    app.add_systems(PreStartup, move |mut exit: MessageWriter<AppExit>| {
                        error!("{msg}");
                        exit.write(AppExit::error());
                    });
                    return;
                }
            }
        }
        app.add_plugins(portal_textures::plugin);
    }
}

#[cfg(test)]
mod steam_tests {
    use super::*;

    #[test]
    fn reads_library_paths_and_installdir() {
        let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"/home/a/.local/share/Steam\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n\t\t\"apps\"\n\t\t{\n\t\t\t\"620\"\t\t\"123\"\n\t\t}\n\t}\n}\n";
        assert_eq!(vdf_values(vdf, "path"), vec!["/home/a/.local/share/Steam".to_string(), "D:\\\\SteamLibrary".to_string()]);
        assert_eq!(vdf_values("\"AppState\"\n{\n\t\"installdir\"\t\t\"Portal 2\"\n}", "installdir"), vec!["Portal 2".to_string()]);
    }
}

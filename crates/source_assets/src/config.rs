//! Where the game lives. The only place install paths are configured.

use std::path::{Path, PathBuf};

/// Fallback when `PORTAL2_DIR` is unset: a `Portal 2` folder in the working
/// directory. Set `PORTAL2_DIR`, or use `p2_assets::find_portal2`, which also
/// searches every Steam library on the machine.
pub const DEFAULT_PORTAL2_DIR: &str = "Portal 2";

/// Environment variable that overrides [`DEFAULT_PORTAL2_DIR`]. It may also
/// point at a Portal 1 install (`.../common/Portal`) as a fallback.
pub const PORTAL2_DIR_ENV: &str = "PORTAL2_DIR";

/// Which Source game an install directory holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameKind {
    Portal2,
    Portal1,
}

/// Install location plus the ordered list of search-path directories,
/// highest priority first, as the engine would mount them.
#[derive(Clone, Debug)]
pub struct GameConfig {
    pub root: PathBuf,
    pub kind: GameKind,
    /// Mount directories, highest priority first. Each is mounted as its
    /// `*_dir.vpk` archives followed by its loose files.
    pub search_paths: Vec<PathBuf>,
}

impl GameConfig {
    /// `PORTAL2_DIR` if set, else [`DEFAULT_PORTAL2_DIR`].
    pub fn from_env() -> Self {
        let root = std::env::var_os(PORTAL2_DIR_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_PORTAL2_DIR));
        Self::from_root(root)
    }

    /// Build the search paths for an install root.
    ///
    /// Portal 2's gameinfo.txt only lists `|gameinfo_path|.`; the engine
    /// itself mounts `update/` above everything and every `portal2_dlcN/`
    /// (highest N first) above `portal2/`. `platform/` comes last. Portal 1
    /// (orange box layout) is `portal/` then `hl2/` then `platform/`.
    pub fn from_root(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref().to_path_buf();
        let kind = if root.join("portal2").is_dir() {
            GameKind::Portal2
        } else {
            GameKind::Portal1
        };
        let mut search_paths = Vec::new();
        match kind {
            GameKind::Portal2 => {
                search_paths.push(root.join("update"));
                let mut dlcs: Vec<(u32, PathBuf)> = std::fs::read_dir(&root)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().to_string();
                        let n = name.strip_prefix("portal2_dlc")?.parse::<u32>().ok()?;
                        Some((n, e.path()))
                    })
                    .collect();
                dlcs.sort_by(|a, b| b.0.cmp(&a.0));
                search_paths.extend(dlcs.into_iter().map(|(_, p)| p));
                search_paths.push(root.join("portal2"));
            }
            GameKind::Portal1 => {
                search_paths.push(root.join("portal"));
                search_paths.push(root.join("hl2"));
            }
        }
        search_paths.push(root.join("platform"));
        search_paths.retain(|p| p.is_dir());
        GameConfig {
            root,
            kind,
            search_paths,
        }
    }

    /// Directory where derived data may be cached (`<workspace>/cache/source_assets`).
    pub fn cache_dir() -> PathBuf {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        manifest
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.join("cache").join("source_assets"))
            .unwrap_or_else(|| PathBuf::from("cache/source_assets"))
    }
}

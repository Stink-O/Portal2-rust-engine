//! Source filesystem: the game's search paths (VPK archives and loose files)
//! with an optional BSP pakfile layered on top. All lookups are
//! case-insensitive and accept `\` or `/` separators.

use crate::config::GameConfig;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Normalise a Source path: lowercase, forward slashes, no leading slash,
/// no doubled slashes.
pub fn normalize_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut last_slash = true;
    for c in path.chars() {
        let c = if c == '\\' { '/' } else { c.to_ascii_lowercase() };
        if c == '/' {
            if last_slash {
                continue;
            }
            last_slash = true;
        } else {
            last_slash = false;
        }
        out.push(c);
    }
    out
}

struct VpkEntry {
    archive: Option<u16>,
    offset: u64,
    length: u32,
    preload: Vec<u8>,
}

struct VpkMount {
    dir_path: PathBuf,
    entries: HashMap<String, VpkEntry>,
    archives: HashMap<u16, PathBuf>,
    /// One open handle per archive; `read_at` is thread safe.
    handles: Mutex<HashMap<u16, Arc<File>>>,
}

impl VpkMount {
    fn open(path: &Path) -> anyhow::Result<Self> {
        let vpk = vpk::from_path(path)?;
        let mut archives = HashMap::new();
        let mut entries = HashMap::with_capacity(vpk.tree.len());
        for (name, entry) in vpk.tree {
            let archive = entry.archive_path.as_ref().map(|p| {
                let idx = entry.dir_entry.archive_index;
                archives.entry(idx).or_insert_with(|| p.as_ref().clone());
                idx
            });
            entries.insert(
                normalize_path(&name),
                VpkEntry {
                    archive,
                    offset: entry.dir_entry.archive_offset as u64,
                    length: entry.dir_entry.file_length,
                    preload: entry.preload_data,
                },
            );
        }
        Ok(VpkMount {
            dir_path: path.to_path_buf(),
            entries,
            archives,
            handles: Mutex::new(HashMap::new()),
        })
    }

    fn handle(&self, idx: u16) -> Option<Arc<File>> {
        let mut handles = self.handles.lock().unwrap();
        if let Some(h) = handles.get(&idx) {
            return Some(h.clone());
        }
        let file = Arc::new(File::open(self.archives.get(&idx)?).ok()?);
        handles.insert(idx, file.clone());
        Some(file)
    }

    fn read(&self, path: &str) -> Option<Vec<u8>> {
        let entry = self.entries.get(path)?;
        let mut data = Vec::with_capacity(entry.preload.len() + entry.length as usize);
        data.extend_from_slice(&entry.preload);
        if let Some(idx) = entry.archive {
            if entry.length > 0 {
                let file = self.handle(idx)?;
                let start = data.len();
                data.resize(start + entry.length as usize, 0);
                read_exact_at(&file, &mut data[start..], entry.offset).ok()?;
            }
        }
        Some(data)
    }
}

/// Positional read that leaves the file cursor alone, so threads can share one handle.
#[cfg(unix)]
fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(file, buf, offset)
}

/// Positional read; Windows has no `read_exact_at`, so loop over `seek_read`.
#[cfg(windows)]
fn read_exact_at(file: &File, mut buf: &mut [u8], mut offset: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        match file.seek_read(buf, offset)? {
            0 => return Err(std::io::ErrorKind::UnexpectedEof.into()),
            n => {
                buf = &mut buf[n..];
                offset += n as u64;
            }
        }
    }
    Ok(())
}

struct LooseMount {
    root: PathBuf,
    /// normalised relative path -> real path
    index: HashMap<String, PathBuf>,
}

impl LooseMount {
    fn scan(root: &Path) -> Self {
        let mut index = HashMap::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                let Ok(ft) = e.file_type() else { continue };
                if ft.is_dir() {
                    // Skip directories that can never hold assets we read.
                    let name = e.file_name().to_string_lossy().to_ascii_lowercase();
                    if matches!(name.as_str(), "sound" | "media" | "save" | "bin" | "soundtrack") {
                        continue;
                    }
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("vpk")) {
                    continue;
                } else if let Ok(rel) = p.strip_prefix(root) {
                    index.insert(normalize_path(&rel.to_string_lossy()), p);
                }
            }
        }
        LooseMount {
            root: root.to_path_buf(),
            index,
        }
    }
}

enum Mount {
    Vpk(VpkMount),
    Loose(LooseMount),
}

/// The game's mounted search paths. Cheap to share behind an `Arc`.
pub struct GameFs {
    pub config: GameConfig,
    mounts: Vec<Mount>,
}

impl GameFs {
    /// Mount everything the config lists. Takes ~0.1 s for Portal 2.
    pub fn mount(config: GameConfig) -> anyhow::Result<Self> {
        let mut mounts = Vec::new();
        for dir in &config.search_paths {
            // VPKs in a search path take priority over its loose files.
            let mut vpks: Vec<PathBuf> = std::fs::read_dir(dir)?
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.to_ascii_lowercase().ends_with("_dir.vpk"))
                })
                .collect();
            vpks.sort();
            for vpk in vpks {
                match VpkMount::open(&vpk) {
                    Ok(m) => mounts.push(Mount::Vpk(m)),
                    Err(e) => bevy::log::warn!("failed to mount {}: {e}", vpk.display()),
                }
            }
            mounts.push(Mount::Loose(LooseMount::scan(dir)));
        }
        if mounts.is_empty() {
            anyhow::bail!(
                "no Source search paths found under {} (set {})",
                config.root.display(),
                crate::config::PORTAL2_DIR_ENV
            );
        }
        Ok(GameFs { config, mounts })
    }

    /// Mount the install named by `PORTAL2_DIR` / the default path.
    pub fn from_env() -> anyhow::Result<Self> {
        Self::mount(GameConfig::from_env())
    }

    /// Read a file by Source path, highest-priority mount first.
    pub fn read(&self, path: &str) -> Option<Vec<u8>> {
        let path = normalize_path(path);
        self.read_normalized(&path)
    }

    fn read_normalized(&self, path: &str) -> Option<Vec<u8>> {
        for m in &self.mounts {
            match m {
                Mount::Vpk(v) => {
                    if let Some(d) = v.read(path) {
                        return Some(d);
                    }
                }
                Mount::Loose(l) => {
                    if let Some(p) = l.index.get(path) {
                        if let Ok(mut f) = File::open(p) {
                            let mut buf = Vec::new();
                            if f.read_to_end(&mut buf).is_ok() {
                                return Some(buf);
                            }
                        }
                    }
                }
            }
        }
        None
    }

    pub fn exists(&self, path: &str) -> bool {
        let path = normalize_path(path);
        self.mounts.iter().any(|m| match m {
            Mount::Vpk(v) => v.entries.contains_key(&path),
            Mount::Loose(l) => l.index.contains_key(&path),
        })
    }

    /// All files under a directory prefix (normalised), across mounts, deduplicated.
    pub fn list(&self, prefix: &str) -> Vec<String> {
        let prefix = normalize_path(prefix);
        let mut out: Vec<String> = self
            .mounts
            .iter()
            .flat_map(|m| -> Box<dyn Iterator<Item = &String>> {
                match m {
                    Mount::Vpk(v) => Box::new(v.entries.keys()),
                    Mount::Loose(l) => Box::new(l.index.keys()),
                }
            })
            .filter(|k| k.starts_with(&prefix))
            .cloned()
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// Human-readable list of mounts, highest priority first.
    pub fn describe_mounts(&self) -> Vec<String> {
        self.mounts
            .iter()
            .map(|m| match m {
                Mount::Vpk(v) => format!("vpk   {} ({} files)", v.dir_path.display(), v.entries.len()),
                Mount::Loose(l) => format!("loose {} ({} files)", l.root.display(), l.index.len()),
            })
            .collect()
    }
}

/// A BSP's embedded pakfile (LUMP_PAKFILE, a zip archive).
pub struct PakFile {
    files: HashMap<String, Vec<u8>>,
}

impl PakFile {
    /// Extract every file of the pakfile up front (they are small and a map
    /// needs most of them: cubemap patch materials, static prop lighting).
    pub fn from_vbsp(pack: &vbsp::Packfile) -> Self {
        let zip = pack.clone().into_zip();
        let mut zip = zip.into_inner().unwrap();
        let mut files = HashMap::new();
        for i in 0..zip.len() {
            let Ok(mut f) = zip.by_index(i) else { continue };
            if f.is_dir() {
                continue;
            }
            let name = normalize_path(f.name());
            let mut buf = Vec::with_capacity(f.size() as usize);
            if f.read_to_end(&mut buf).is_ok() {
                files.insert(name, buf);
            }
        }
        PakFile { files }
    }

    pub fn empty() -> Self {
        PakFile {
            files: HashMap::new(),
        }
    }

    pub fn get(&self, path: &str) -> Option<&[u8]> {
        self.files.get(path).map(|v| v.as_slice())
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(|s| s.as_str())
    }
}

/// The game filesystem with one map's pakfile mounted on top.
pub struct MapFs<'a> {
    pub game: &'a GameFs,
    pub pak: &'a PakFile,
}

impl MapFs<'_> {
    pub fn read(&self, path: &str) -> Option<Vec<u8>> {
        let path = normalize_path(path);
        if let Some(d) = self.pak.get(&path) {
            return Some(d.to_vec());
        }
        self.game.read_normalized(&path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_paths() {
        assert_eq!(normalize_path("Materials\\Metal//Foo.VMT"), "materials/metal/foo.vmt");
        assert_eq!(normalize_path("/models/props/x.mdl"), "models/props/x.mdl");
    }

    #[test]
    fn portal2_search_paths_in_engine_order() {
        let cfg = GameConfig::from_env();
        if !cfg.root.join("portal2").is_dir() {
            eprintln!("Portal 2 not installed, skipping");
            return;
        }
        let names: Vec<String> = cfg
            .search_paths
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names.first().map(String::as_str), Some("update"));
        let p2 = names.iter().position(|n| n == "portal2").unwrap();
        let dlc1 = names.iter().position(|n| n == "portal2_dlc1").unwrap();
        let dlc2 = names.iter().position(|n| n == "portal2_dlc2").unwrap();
        assert!(dlc2 < dlc1 && dlc1 < p2);
    }

    #[test]
    fn reads_from_vpk_and_loose() {
        let cfg = GameConfig::from_env();
        if !cfg.root.join("portal2").is_dir() {
            return;
        }
        let fs = GameFs::mount(cfg).unwrap();
        let vmt = fs.read("materials/metal/black_wall_metal_002b.vmt").unwrap();
        assert!(String::from_utf8_lossy(&vmt).to_ascii_lowercase().contains("basetexture"));
        // case-insensitive, backslashes
        assert!(fs.read("MATERIALS\\Metal\\black_wall_metal_002b.VMT").is_some());
        // loose file
        assert!(fs.read("maps/sp_a1_intro3.bsp").is_some());
    }
}

//! `scripts/surfaceproperties*.txt`: surfaceprop name -> game material
//! character, following `base` inheritance. Portal placement rejects
//! surfaces whose game material is glass (`Y`).

use crate::fs::GameFs;
use crate::kv;
use std::collections::HashMap;

#[derive(Clone, Debug, Default)]
pub struct SurfaceProps {
    entries: HashMap<String, (Option<String>, Option<char>)>,
}

pub const GAMEMATERIAL_GLASS: char = 'Y';

impl SurfaceProps {
    pub fn load(fs: &GameFs) -> Self {
        let mut sp = SurfaceProps::default();
        let files: Vec<String> = fs
            .read("scripts/surfaceproperties_manifest.txt")
            .map(|d| {
                let kv = kv::parse(&String::from_utf8_lossy(&d));
                kv.root()
                    .map(|(_, b)| {
                        b.entries
                            .iter()
                            .filter(|(k, _)| k == "file")
                            .filter_map(|(_, v)| match v {
                                kv::KvValue::Str(s) => Some(s.clone()),
                                _ => None,
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .unwrap_or_else(|| vec!["scripts/surfaceproperties.txt".into()]);
        for f in files {
            if let Some(d) = fs.read(&f) {
                sp.add_text(&String::from_utf8_lossy(&d));
            }
        }
        sp
    }

    pub fn add_text(&mut self, text: &str) {
        let kv = kv::parse(text);
        for (name, v) in &kv.entries {
            if let kv::KvValue::Block(b) = v {
                let base = b.get("base").map(|s| s.to_ascii_lowercase());
                let gm = b.get("gamematerial").and_then(|s| s.trim().chars().next());
                self.entries.insert(name.clone(), (base, gm));
            }
        }
    }

    /// Game material character for a surfaceprop, following `base` (default 'C' concrete).
    pub fn game_material(&self, name: &str) -> char {
        let mut cur = name.to_ascii_lowercase();
        for _ in 0..16 {
            match self.entries.get(&cur) {
                Some((_, Some(gm))) => return gm.to_ascii_uppercase(),
                Some((Some(base), None)) => cur = base.clone(),
                _ => break,
            }
        }
        'C'
    }

    pub fn is_glass(&self, name: &str) -> bool {
        self.game_material(name) == GAMEMATERIAL_GLASS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inherits_game_material() {
        let mut sp = SurfaceProps::default();
        sp.add_text(r#""glass" { "gamematerial" "Y" } "glassbottle" { "base" "glass" } "metal" { "gamematerial" "M" }"#);
        assert!(sp.is_glass("GlassBottle"));
        assert!(!sp.is_glass("metal"));
        assert_eq!(sp.game_material("unknown"), 'C');
    }
}

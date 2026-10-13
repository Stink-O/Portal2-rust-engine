//! Compiled Source shaders (`shaders/fxc/*.vcs`, version 6 as shipped with
//! Portal 2): combo directory, LZMA-compressed dynamic-combo blocks, and the
//! D3D9 bytecode's constant table (CTAB).
//!
//! Layout (all little endian):
//! - header: `version, total_combos, dynamic_combos, flags, centroid_mask,
//!   static_count, crc` (7 x i32)
//! - `static_count` x `(static_id, file_offset)`; the last entry is a
//!   sentinel `(0xffffffff, end_of_data)`
//! - `u32 duplicate_count`, then `(static_id, same_as_static_id)` pairs
//! - per static combo, at its offset: chunks of `u32 (kind << 30 | size)`
//!   followed by `size` bytes (kind 1 = Valve LZMA, 2 = stored), ending with
//!   `0xffffffff`. Each chunk decodes to `(dynamic_id, byte_len, bytecode)`
//!   records.
//!
//! Full combo id = `static_id * dynamic_combos + dynamic_id`.

use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct VcsFile {
    pub version: i32,
    pub total_combos: u32,
    pub dynamic_combos: u32,
    pub flags: u32,
    pub centroid_mask: u32,
    pub crc: u32,
    /// static id -> byte offset of its chunk list
    statics: Vec<(u32, u32)>,
    /// static id -> static id it duplicates
    duplicates: HashMap<u32, u32>,
    data: Vec<u8>,
}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

/// Decode Valve's LZMA wrapper: `"LZMA", actual_size, lzma_size, props[5], data`.
pub fn valve_lzma(buf: &[u8]) -> anyhow::Result<Vec<u8>> {
    if buf.get(0..4) != Some(b"LZMA") {
        anyhow::bail!("missing LZMA header");
    }
    let actual = u32_at(buf, 4).unwrap_or(0) as u64;
    let comp = u32_at(buf, 8).unwrap_or(0) as usize;
    let props = buf.get(12..17).ok_or_else(|| anyhow::anyhow!("short LZMA header"))?;
    let body = buf.get(17..17 + comp).ok_or_else(|| anyhow::anyhow!("short LZMA body"))?;
    // Rebuild a classic .lzma header: props(5) + unpacked size(8).
    let mut stream = Vec::with_capacity(13 + body.len());
    stream.extend_from_slice(props);
    stream.extend_from_slice(&actual.to_le_bytes());
    stream.extend_from_slice(body);
    let mut out = Vec::with_capacity(actual as usize);
    lzma_rs::lzma_decompress(&mut std::io::Cursor::new(stream), &mut out)
        .map_err(|e| anyhow::anyhow!("lzma: {e:?}"))?;
    Ok(out)
}

impl VcsFile {
    pub fn parse(data: Vec<u8>) -> anyhow::Result<Self> {
        let rd = |o: usize| u32_at(&data, o).ok_or_else(|| anyhow::anyhow!("truncated VCS"));
        let version = rd(0)? as i32;
        if version != 6 {
            anyhow::bail!("unsupported VCS version {version}");
        }
        let total_combos = rd(4)?;
        let dynamic_combos = rd(8)?.max(1);
        let flags = rd(12)?;
        let centroid_mask = rd(16)?;
        let static_count = rd(20)? as usize;
        let crc = rd(24)?;
        let mut statics = Vec::with_capacity(static_count);
        for i in 0..static_count {
            statics.push((rd(28 + i * 8)?, rd(32 + i * 8)?));
        }
        let mut o = 28 + static_count * 8;
        let dup_count = rd(o)? as usize;
        o += 4;
        let mut duplicates = HashMap::with_capacity(dup_count);
        for i in 0..dup_count {
            duplicates.insert(rd(o + i * 8)?, rd(o + i * 8 + 4)?);
        }
        Ok(VcsFile {
            version,
            total_combos,
            dynamic_combos,
            flags,
            centroid_mask,
            crc,
            statics,
            duplicates,
            data,
        })
    }

    pub fn static_combos(&self) -> u32 {
        self.total_combos / self.dynamic_combos
    }

    /// Static ids with their own code (sentinel excluded).
    pub fn static_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.statics.iter().map(|s| s.0).filter(|&s| s != u32::MAX)
    }

    /// Resolve duplicates to the static id that owns the code.
    pub fn canonical_static(&self, id: u32) -> Option<u32> {
        let mut id = id;
        for _ in 0..8 {
            if self.statics.binary_search_by_key(&id, |s| s.0).is_ok() {
                return Some(id);
            }
            id = *self.duplicates.get(&id)?;
        }
        None
    }

    /// All dynamic combos of a static combo: `dynamic_id -> bytecode`.
    pub fn static_block(&self, static_id: u32) -> anyhow::Result<Vec<(u32, Vec<u8>)>> {
        let id = self
            .canonical_static(static_id)
            .ok_or_else(|| anyhow::anyhow!("static combo {static_id} not present"))?;
        let idx = self.statics.binary_search_by_key(&id, |s| s.0).unwrap();
        let start = self.statics[idx].1 as usize;
        let end = self
            .statics
            .get(idx + 1)
            .map(|s| s.1 as usize)
            .unwrap_or(self.data.len());
        let mut out = Vec::new();
        let mut p = start;
        while p + 4 <= end {
            let w = u32_at(&self.data, p).unwrap();
            if w == u32::MAX {
                break;
            }
            let size = (w & 0x3fff_ffff) as usize;
            let kind = w >> 30;
            let chunk = self
                .data
                .get(p + 4..p + 4 + size)
                .ok_or_else(|| anyhow::anyhow!("chunk out of range"))?;
            let raw = if kind == 1 { valve_lzma(chunk)? } else { chunk.to_vec() };
            let mut q = 0;
            while q + 8 <= raw.len() {
                let dynamic = u32_at(&raw, q).unwrap();
                let len = u32_at(&raw, q + 4).unwrap() as usize;
                let code = raw
                    .get(q + 8..q + 8 + len)
                    .ok_or_else(|| anyhow::anyhow!("combo out of range"))?;
                out.push((dynamic, code.to_vec()));
                q += 8 + len;
            }
            p += 4 + size;
        }
        Ok(out)
    }

    /// Bytecode for one full combo.
    pub fn combo(&self, static_id: u32, dynamic_id: u32) -> Option<Vec<u8>> {
        self.static_block(static_id)
            .ok()?
            .into_iter()
            .find(|(d, _)| *d == dynamic_id)
            .map(|(_, c)| c)
    }
}

/// D3DXREGISTER_SET
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegisterSet {
    Bool,
    Int4,
    Float4,
    Sampler,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CtabConstant {
    pub name: String,
    pub set: RegisterSet,
    pub register: u16,
    pub count: u16,
}

/// Shader version token, e.g. `0xffff0201` = ps_2_b (ps_2_x), `0xfffe0200` = vs_2_0.
pub fn shader_version(code: &[u8]) -> Option<(bool, u8, u8)> {
    let v = u32_at(code, 0)?;
    let pixel = v >> 16 == 0xffff;
    Some((pixel, ((v >> 8) & 0xff) as u8, (v & 0xff) as u8))
}

/// Parse the CTAB comment block of D3D9 shader bytecode.
pub fn parse_ctab(code: &[u8]) -> Vec<CtabConstant> {
    let mut out = Vec::new();
    let mut p = 4;
    while p + 8 <= code.len() {
        let tok = u32_at(code, p).unwrap();
        if tok & 0xffff != 0xfffe {
            break; // comments come first; stop at the first instruction
        }
        let len = ((tok >> 16) & 0x7fff) as usize * 4;
        let body = &code[(p + 4).min(code.len())..(p + 4 + len).min(code.len())];
        if body.get(0..4) == Some(b"CTAB") {
            let t = &body[4..];
            let n = u32_at(t, 12).unwrap_or(0) as usize;
            let info = u32_at(t, 16).unwrap_or(0) as usize;
            for i in 0..n {
                let e = info + i * 20;
                let (Some(name_off), Some(set_reg), Some(count)) = (
                    u32_at(t, e),
                    u32_at(t, e + 4),
                    t.get(e + 8..e + 10).map(|b| u16::from_le_bytes([b[0], b[1]])),
                ) else {
                    break;
                };
                let name_off = name_off as usize;
                let name_end = t[name_off.min(t.len())..].iter().position(|&c| c == 0).unwrap_or(0);
                let name = String::from_utf8_lossy(&t[name_off.min(t.len())..name_off.min(t.len()) + name_end]).to_string();
                let set = match set_reg & 0xffff {
                    0 => RegisterSet::Bool,
                    1 => RegisterSet::Int4,
                    2 => RegisterSet::Float4,
                    _ => RegisterSet::Sampler,
                };
                out.push(CtabConstant {
                    name,
                    set,
                    register: (set_reg >> 16) as u16,
                    count,
                });
            }
        }
        p += 4 + len;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_lightmappedgeneric_ps20b() {
        let cfg = crate::config::GameConfig::from_env();
        if !cfg.root.join("portal2").is_dir() {
            return;
        }
        let fs = crate::fs::GameFs::mount(cfg).unwrap();
        let data = fs.read("shaders/fxc/lightmappedgeneric_ps20b.vcs").unwrap();
        let vcs = VcsFile::parse(data).unwrap();
        assert_eq!(vcs.dynamic_combos, 32);
        assert!(vcs.static_ids().count() > 1000);
        let block = vcs.static_block(0).unwrap();
        assert!(!block.is_empty());
        let (pixel, major, _) = shader_version(&block[0].1).unwrap();
        assert!(pixel && major == 2);
        let ctab = parse_ctab(&block[0].1);
        assert!(ctab.iter().any(|c| c.name == "BaseTextureSampler" && c.set == RegisterSet::Sampler && c.register == 0));
        assert!(ctab.iter().any(|c| c.name == "LightmapSampler" && c.register == 1));
    }
}

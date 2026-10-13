//! Direct lump access for what `vbsp` does not expose (or gets wrong):
//! HDR lighting, the HDR face lump, leaves in file order (vbsp sorts them by
//! cluster, breaking node -> leaf indices) and leaf ambient lighting.

use bevy::math::Vec3;

pub const LUMP_LIGHTING: usize = 8;
pub const LUMP_LEAVES: usize = 10;
pub const LUMP_FACES: usize = 7;
pub const LUMP_LEAF_AMBIENT_INDEX_HDR: usize = 51;
pub const LUMP_LEAF_AMBIENT_INDEX: usize = 52;
pub const LUMP_LIGHTING_HDR: usize = 53;
pub const LUMP_LEAF_AMBIENT_LIGHTING_HDR: usize = 55;
pub const LUMP_LEAF_AMBIENT_LIGHTING: usize = 56;
pub const LUMP_FACES_HDR: usize = 58;

#[derive(Clone, Copy, Debug, Default)]
pub struct LumpInfo {
    pub offset: u32,
    pub length: u32,
    pub version: u32,
    pub ident: u32,
}

pub struct RawBsp<'a> {
    pub data: &'a [u8],
    pub lumps: [LumpInfo; 64],
}

fn u32_at(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}
fn i32_at(d: &[u8], o: usize) -> i32 {
    i32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}
fn i16_at(d: &[u8], o: usize) -> i16 {
    i16::from_le_bytes([d[o], d[o + 1]])
}
fn u16_at(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}

impl<'a> RawBsp<'a> {
    pub fn new(data: &'a [u8]) -> anyhow::Result<Self> {
        if data.len() < 8 + 64 * 16 || &data[0..4] != b"VBSP" {
            anyhow::bail!("not a VBSP file");
        }
        let mut lumps = [LumpInfo::default(); 64];
        for (i, l) in lumps.iter_mut().enumerate() {
            let o = 8 + i * 16;
            *l = LumpInfo {
                offset: u32_at(data, o),
                length: u32_at(data, o + 4),
                version: u32_at(data, o + 8),
                ident: u32_at(data, o + 12),
            };
        }
        // L4D2-style {version, offset, length, ident} ordering (same heuristic as vbsp).
        let l4d2 = lumps.iter().all(|l| l.offset <= 20)
            || lumps.iter().any(|l| l.length as usize > data.len());
        if l4d2 {
            for l in lumps.iter_mut() {
                *l = LumpInfo {
                    offset: l.length,
                    length: l.version,
                    version: l.offset,
                    ident: l.ident,
                };
            }
        }
        Ok(RawBsp { data, lumps })
    }

    /// Lump bytes, or `None` if empty, out of range or LZMA-compressed.
    pub fn lump(&self, index: usize) -> Option<&'a [u8]> {
        let l = self.lumps[index];
        if l.length == 0 || l.ident != 0 {
            return None;
        }
        self.data.get(l.offset as usize..(l.offset + l.length) as usize)
    }
}

/// The lighting-relevant fields of a `dface_t`.
#[derive(Clone, Copy, Debug)]
pub struct FaceLight {
    pub styles: [u8; 4],
    pub light_offset: i32,
    pub mins: [i32; 2],
    pub size: [i32; 2],
}

pub fn parse_face_lights(lump: &[u8]) -> Vec<FaceLight> {
    lump.chunks_exact(56)
        .map(|f| FaceLight {
            styles: [f[16], f[17], f[18], f[19]],
            light_offset: i32_at(f, 20),
            mins: [i32_at(f, 28), i32_at(f, 32)],
            size: [i32_at(f, 36), i32_at(f, 40)],
        })
        .collect()
}

#[derive(Clone, Copy, Debug)]
pub struct RawLeaf {
    pub contents: i32,
    pub cluster: i16,
    pub first_leaf_brush: u16,
    pub leaf_brush_count: u16,
}

pub fn parse_leaves(bsp: &RawBsp) -> Vec<RawLeaf> {
    let Some(lump) = bsp.lump(LUMP_LEAVES) else {
        return Vec::new();
    };
    let size = if bsp.lumps[LUMP_LEAVES].version == 0 { 56 } else { 32 };
    lump.chunks_exact(size)
        .map(|l| RawLeaf {
            contents: i32_at(l, 0),
            cluster: i16_at(l, 4),
            first_leaf_brush: u16_at(l, 24),
            leaf_brush_count: u16_at(l, 26),
        })
        .collect()
}

/// `ColorRGBExp32` to linear RGB.
#[inline]
pub fn rgbexp_to_linear(c: &[u8]) -> Vec3 {
    let e = (c[3] as i8) as i32;
    let s = (2.0f32).powi(e) / 255.0;
    Vec3::new(c[0] as f32 * s, c[1] as f32 * s, c[2] as f32 * s)
}

/// Six-sided ambient light cube (+X, -X, +Y, -Y, +Z, -Z in Source axes).
#[derive(Clone, Copy, Debug, Default)]
pub struct AmbientCube {
    pub sides: [Vec3; 6],
    /// Sample position in Source units.
    pub pos: Vec3,
}

impl AmbientCube {
    /// Irradiance for a Source-space normal.
    pub fn eval(&self, n: Vec3) -> Vec3 {
        let sq = n * n;
        let x = if n.x >= 0.0 { self.sides[0] } else { self.sides[1] };
        let y = if n.y >= 0.0 { self.sides[2] } else { self.sides[3] };
        let z = if n.z >= 0.0 { self.sides[4] } else { self.sides[5] };
        x * sq.x + y * sq.y + z * sq.z
    }

    pub fn average(&self) -> Vec3 {
        self.sides.iter().copied().sum::<Vec3>() / 6.0
    }
}

/// Per-leaf ambient samples (v1 lumps, Portal 2 / L4D2 era).
pub struct LeafAmbient {
    /// (first, count) per leaf
    pub index: Vec<(u16, u16)>,
    /// cube + fractional position (0..1) in the leaf bounds
    pub samples: Vec<(AmbientCube, [u8; 3])>,
}

pub fn parse_leaf_ambient(bsp: &RawBsp) -> Option<LeafAmbient> {
    let (idx, samp) = match (
        bsp.lump(LUMP_LEAF_AMBIENT_INDEX_HDR),
        bsp.lump(LUMP_LEAF_AMBIENT_LIGHTING_HDR),
    ) {
        (Some(i), Some(s)) => (i, s),
        _ => (
            bsp.lump(LUMP_LEAF_AMBIENT_INDEX)?,
            bsp.lump(LUMP_LEAF_AMBIENT_LIGHTING)?,
        ),
    };
    let index = idx
        .chunks_exact(4)
        .map(|c| (u16_at(c, 2), u16_at(c, 0)))
        .collect();
    let samples = samp
        .chunks_exact(28)
        .map(|c| {
            let mut cube = AmbientCube::default();
            // Ambient cubes are stored in 0..1 units, lightmaps in 0..255
            // units: scale to the lightmap convention (exponent histograms of
            // the two lumps sit ~8 stops apart).
            for s in 0..6 {
                cube.sides[s] = rgbexp_to_linear(&c[s * 4..s * 4 + 4]) * 255.0;
            }
            (cube, [c[24], c[25], c[26]])
        })
        .collect();
    Some(LeafAmbient { index, samples })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgbexp() {
        let v = rgbexp_to_linear(&[255, 128, 0, 1]);
        assert!((v.x - 2.0).abs() < 1e-5 && (v.y - 256.0 / 255.0).abs() < 1e-5 && v.z == 0.0);
        let v = rgbexp_to_linear(&[255, 0, 0, (-2i8) as u8]);
        assert!((v.x - 0.25).abs() < 1e-5);
    }

    #[test]
    fn ambient_cube_eval() {
        let mut c = AmbientCube::default();
        c.sides[4] = Vec3::ONE; // +Z
        assert_eq!(c.eval(Vec3::Z), Vec3::ONE);
        assert_eq!(c.eval(-Vec3::Z), Vec3::ZERO);
    }
}


pub const LUMP_OVERLAYS: usize = 45;

/// One `doverlay_t` (an `info_overlay` baked into the map).
#[derive(Clone, Debug)]
pub struct Overlay {
    pub texinfo: i16,
    pub faces: Vec<usize>,
    pub render_order: u16,
    pub u: [f32; 2],
    pub v: [f32; 2],
    /// Corner positions in the (basis_u, basis_v) plane.
    pub points: [Vec3; 4],
    pub origin: Vec3,
    pub normal: Vec3,
    /// Stored in the z of the first three UV points.
    pub basis_u: Vec3,
}

pub fn parse_overlays(bsp: &RawBsp) -> Vec<Overlay> {
    let Some(lump) = bsp.lump(LUMP_OVERLAYS) else { return Vec::new() };
    let f = |d: &[u8], o: usize| f32::from_le_bytes(d[o..o + 4].try_into().unwrap());
    let v3 = |d: &[u8], o: usize| Vec3::new(f(d, o), f(d, o + 4), f(d, o + 8));
    lump.chunks_exact(352)
        .map(|d| {
            let fc = u16_at(d, 6);
            let count = (fc & 0x3fff) as usize;
            let faces = (0..count.min(64)).map(|k| i32_at(d, 8 + k * 4).max(0) as usize).collect();
            let pts: Vec<Vec3> = (0..4).map(|k| v3(d, 280 + k * 12)).collect();
            Overlay {
                texinfo: i16_at(d, 4),
                faces,
                render_order: fc >> 14,
                u: [f(d, 264), f(d, 268)],
                v: [f(d, 272), f(d, 276)],
                points: [pts[0], pts[1], pts[2], pts[3]],
                origin: v3(d, 328),
                normal: v3(d, 340),
                basis_u: Vec3::new(pts[0].z, pts[1].z, pts[2].z),
            }
        })
        .collect()
}

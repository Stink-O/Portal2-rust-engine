//! Brush faces and displacements to triangle meshes with lightmap UVs, and
//! the lightmap atlas built from the BSP lighting lumps.

use crate::bsp_raw::{self, FaceLight, RawBsp};
use bevy::math::{Vec2, Vec3};
use shared::{source_dir_to_world, source_to_world};
use std::collections::HashMap;

/// A mesh in world axes (Y-up, metres), relative to its brush model origin.
#[derive(Clone, Debug, Default)]
pub struct MeshData {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uv0: Vec<[f32; 2]>,
    pub uv1: Vec<[f32; 2]>,
    /// RGBA; alpha is the WorldVertexTransition blend on displacements.
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
    /// Source-space data for the Valve-shader path (relative to the model origin).
    pub source_pos: Vec<[f32; 3]>,
    pub source_normal: Vec<[f32; 3]>,
    /// Tangent (texture s) and binormal, Source space, per the vrad bump basis.
    pub tangent_s: Vec<[f32; 3]>,
    pub tangent_t: Vec<[f32; 3]>,
    /// Offset from one bumped lightmap block to the next, in atlas UV.
    pub lm_offset: Vec<[f32; 2]>,
}

/// Per-face vertex attributes beyond position/normal/uv.
#[derive(Clone, Copy, Default)]
pub struct FaceFrame {
    pub tangent_s: Vec3,
    pub tangent_t: Vec3,
    pub lm_offset: Vec2,
}

impl MeshData {
    #[cfg(test)]
    fn push_vertex(&mut self, p: Vec3, n: Vec3, uv: Vec2, lm: Vec2, alpha: f32) -> u32 {
        self.push_vertex_ex(p, n, uv, lm, alpha, &FaceFrame::default())
    }

    fn push_vertex_ex(&mut self, p: Vec3, n: Vec3, uv: Vec2, lm: Vec2, alpha: f32, f: &FaceFrame) -> u32 {
        let i = self.positions.len() as u32;
        self.positions.push(source_to_world(p).to_array());
        self.normals.push(source_dir_to_world(n).normalize_or_zero().to_array());
        self.uv0.push(uv.to_array());
        self.uv1.push(lm.to_array());
        self.colors.push([1.0, 1.0, 1.0, alpha]);
        self.source_pos.push(p.to_array());
        self.source_normal.push(n.normalize_or_zero().to_array());
        self.tangent_s.push(f.tangent_s.to_array());
        self.tangent_t.push(f.tangent_t.to_array());
        self.lm_offset.push(f.lm_offset.to_array());
        i
    }

    /// Push a triangle, oriented so its front face (CCW in Bevy) faces `n` (Source space).
    fn push_tri(&mut self, a: u32, b: u32, c: u32, source_pos: [Vec3; 3], n: Vec3) {
        let cross = (source_pos[1] - source_pos[0]).cross(source_pos[2] - source_pos[0]);
        if cross.length_squared() < 1e-12 {
            return;
        }
        // The Source->world axis swap is a proper rotation, so orientation
        // relative to the normal is preserved.
        if cross.dot(n) >= 0.0 {
            self.indices.extend_from_slice(&[a, b, c]);
        } else {
            self.indices.extend_from_slice(&[a, c, b]);
        }
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

/// HDR (or LDR) lightmap atlas, linear RGB, packed as RGB9E5.
#[derive(Clone, Debug)]
pub struct LightmapAtlas {
    pub width: u32,
    pub height: u32,
    /// `Rgb9e5Ufloat` texels
    pub texels: Vec<u32>,
    pub hdr: bool,
}

/// Pack linear RGB into the shared-exponent RGB9E5 format.
pub fn pack_rgb9e5(c: Vec3) -> u32 {
    const MAX: f32 = 65408.0; // (2^9-1)/2^9 * 2^15
    let r = c.x.clamp(0.0, MAX);
    let g = c.y.clamp(0.0, MAX);
    let b = c.z.clamp(0.0, MAX);
    let maxc = r.max(g).max(b);
    if maxc <= 0.0 {
        return 0;
    }
    let mut exp_shared = (maxc.log2().floor() as i32).max(-16) + 1 + 15;
    let mut denom = 2f32.powi(exp_shared - 15 - 9);
    let maxm = (maxc / denom + 0.5).floor() as i32;
    if maxm == 512 {
        denom *= 2.0;
        exp_shared += 1;
    }
    let exp_shared = exp_shared.clamp(0, 31) as u32;
    let rm = ((r / denom + 0.5).floor() as u32).min(511);
    let gm = ((g / denom + 0.5).floor() as u32).min(511);
    let bm = ((b / denom + 0.5).floor() as u32).min(511);
    rm | (gm << 9) | (bm << 18) | (exp_shared << 27)
}

pub fn unpack_rgb9e5(v: u32) -> Vec3 {
    let e = (v >> 27) as i32 - 15 - 9;
    let s = 2f32.powi(e);
    Vec3::new(
        (v & 511) as f32 * s,
        ((v >> 9) & 511) as f32 * s,
        ((v >> 18) & 511) as f32 * s,
    )
}

/// Shelf packer for lightmap rectangles.
struct Packer {
    width: u32,
    x: u32,
    y: u32,
    shelf_h: u32,
}

impl Packer {
    fn alloc(&mut self, w: u32, h: u32) -> (u32, u32) {
        if self.x + w > self.width {
            self.x = 0;
            self.y += self.shelf_h;
            self.shelf_h = 0;
        }
        let r = (self.x, self.y);
        self.x += w;
        self.shelf_h = self.shelf_h.max(h);
        r
    }
}

/// Everything needed to build face geometry, borrowed from the parsed BSP.
pub struct FaceBuilder<'a> {
    pub bsp: &'a vbsp::Bsp,
    pub face_lights: Vec<FaceLight>,
    pub lighting: Option<&'a [u8]>,
    pub hdr: bool,
    /// Where each lit face's lightmap landed in the atlas: face index -> (x, y).
    rects: HashMap<usize, (u32, u32)>,
    pub atlas: LightmapAtlas,
}

pub const SURF_LIGHT: u32 = 0x1;
pub const SURF_SKY2D: u32 = 0x2;
pub const SURF_SKY: u32 = 0x4;
pub const SURF_WARP: u32 = 0x8;
pub const SURF_TRANS: u32 = 0x10;
pub const SURF_NOPORTAL: u32 = 0x20;
pub const SURF_TRIGGER: u32 = 0x40;
pub const SURF_NODRAW: u32 = 0x80;
pub const SURF_HINT: u32 = 0x100;
pub const SURF_SKIP: u32 = 0x200;
pub const SURF_NOLIGHT: u32 = 0x400;
pub const SURF_BUMPLIGHT: u32 = 0x800;

pub fn texinfo_flags(ti: &vbsp::TextureInfo) -> u32 {
    ti.flags.bits()
}

impl<'a> FaceBuilder<'a> {
    /// Prepare lighting data and pack the lightmap atlas for the given faces.
    pub fn new(bsp: &'a vbsp::Bsp, raw: &RawBsp<'a>, faces: &[usize]) -> Self {
        let (lighting, face_lump, hdr) = match (
            raw.lump(bsp_raw::LUMP_LIGHTING_HDR),
            raw.lump(bsp_raw::LUMP_FACES_HDR),
        ) {
            (Some(l), Some(f)) => (Some(l), Some(f), true),
            (Some(l), None) if raw.lump(bsp_raw::LUMP_LIGHTING).is_none() => {
                (Some(l), raw.lump(bsp_raw::LUMP_FACES), true)
            }
            _ => (
                raw.lump(bsp_raw::LUMP_LIGHTING),
                raw.lump(bsp_raw::LUMP_FACES),
                false,
            ),
        };
        let mut face_lights = face_lump.map(bsp_raw::parse_face_lights).unwrap_or_default();
        if face_lights.len() != bsp.faces.len() {
            face_lights = bsp
                .faces
                .iter()
                .map(|f| FaceLight {
                    styles: f.styles,
                    light_offset: f.light_offset,
                    mins: f.light_map_texture_min,
                    size: f.light_map_texture_size,
                })
                .collect();
        }

        let mut fb = FaceBuilder {
            bsp,
            face_lights,
            lighting,
            hdr,
            rects: HashMap::new(),
            atlas: LightmapAtlas {
                width: 0,
                height: 0,
                texels: Vec::new(),
                hdr,
            },
        };
        fb.pack(faces);
        fb
    }

    fn face_has_lightmap(&self, fi: usize) -> bool {
        let face = &self.bsp.faces[fi];
        let Some(lighting) = self.lighting else { return false };
        let fl = &self.face_lights[fi];
        let Some(ti) = self.bsp.textures_info.get(face.texture_info as usize) else {
            return false;
        };
        if texinfo_flags(ti) & SURF_NOLIGHT != 0 || fl.light_offset < 0 || fl.styles[0] == 255 {
            return false;
        }
        let n = ((fl.size[0] + 1) * (fl.size[1] + 1)) as usize;
        (fl.light_offset as usize + n * 4) <= lighting.len()
    }

    fn pack(&mut self, faces: &[usize]) {
        let mut lit: Vec<(usize, u32, u32)> = faces
            .iter()
            .copied()
            .filter(|&f| self.face_has_lightmap(f))
            .map(|f| {
                let fl = &self.face_lights[f];
                // 1 texel of padding on each side; bumped faces store four
                // lightmaps (flat + 3 basis directions) side by side.
                let blocks = if self.face_bumped(f) { 4 } else { 1 };
                (f, (fl.size[0] as u32 + 3) * blocks, fl.size[1] as u32 + 3)
            })
            .collect();
        lit.sort_by(|a, b| b.2.cmp(&a.2).then(b.1.cmp(&a.1)));
        let area: u64 = lit.iter().map(|r| (r.1 * r.2) as u64).sum::<u64>() + 16;
        let mut width = 256u32;
        while (width as u64 * width as u64) < area * 5 / 4 && width < 8192 {
            width *= 2;
        }
        let mut packer = Packer {
            width,
            x: 4, // (0..4, 0..4) reserved: a white block for unlit faces
            y: 0,
            shelf_h: 4,
        };
        for &(f, w, h) in &lit {
            let (x, y) = packer.alloc(w, h);
            self.rects.insert(f, (x + 1, y + 1));
        }
        let height = (packer.y + packer.shelf_h).max(4).next_power_of_two();
        let mut texels = vec![0u32; (width * height) as usize];
        let white = pack_rgb9e5(Vec3::ONE);
        for y in 0..4 {
            for x in 0..4 {
                texels[(y * width + x) as usize] = white;
            }
        }
        let lighting = self.lighting.unwrap_or(&[]);
        for &(f, _, _) in &lit {
            let (x0, y0) = self.rects[&f];
            let fl = self.face_lights[f];
            let (w, h) = (fl.size[0] as u32 + 1, fl.size[1] as u32 + 1);
            let blocks = if self.face_bumped(f) { 4 } else { 1 };
            for block in 0..blocks {
                let base = fl.light_offset as usize + (block * w * h * 4) as usize;
                if base + (w * h * 4) as usize > lighting.len() {
                    break;
                }
                let get = |x: u32, y: u32| {
                    let o = base + ((y * w + x) * 4) as usize;
                    pack_rgb9e5(bsp_raw::rgbexp_to_linear(&lighting[o..o + 4]))
                };
                let bx = x0 + block * (w + 2);
                // copy with a 1-texel clamped border
                for yy in 0..h + 2 {
                    for xx in 0..w + 2 {
                        let sx = (xx as i32 - 1).clamp(0, w as i32 - 1) as u32;
                        let sy = (yy as i32 - 1).clamp(0, h as i32 - 1) as u32;
                        let tx = bx - 1 + xx;
                        let ty = y0 - 1 + yy;
                        texels[(ty * width + tx) as usize] = get(sx, sy);
                    }
                }
            }
        }
        self.atlas = LightmapAtlas {
            width,
            height,
            texels,
            hdr: self.hdr,
        };
    }

    fn face_bumped(&self, fi: usize) -> bool {
        let face = &self.bsp.faces[fi];
        self.bsp
            .textures_info
            .get(face.texture_info as usize)
            .is_some_and(|ti| texinfo_flags(ti) & SURF_BUMPLIGHT != 0)
    }

    /// Tangent frame (vrad's bump basis convention) and bump block offset for a face.
    fn face_frame(&self, fi: usize, normal: Vec3) -> FaceFrame {
        let bsp = self.bsp;
        let face = &bsp.faces[fi];
        let Some(ti) = bsp.textures_info.get(face.texture_info as usize) else {
            return FaceFrame::default();
        };
        let s_axis = Vec3::new(ti.texture_transforms_u[0], ti.texture_transforms_u[1], ti.texture_transforms_u[2]);
        let t_axis = Vec3::new(ti.texture_transforms_v[0], ti.texture_transforms_v[1], ti.texture_transforms_v[2]);
        let left_handed = s_axis.cross(t_axis).dot(normal) < 0.0;
        let mut binormal = normal.cross(s_axis).normalize_or_zero();
        let tangent = binormal.cross(normal).normalize_or_zero();
        if left_handed {
            binormal = -binormal;
        }
        let lm_offset = match (self.face_bumped(fi), self.rects.get(&fi)) {
            (true, Some(_)) => {
                let fl = &self.face_lights[fi];
                Vec2::new((fl.size[0] as f32 + 3.0) / self.atlas.width.max(1) as f32, 0.0)
            }
            _ => Vec2::ZERO,
        };
        FaceFrame {
            tangent_s: tangent,
            tangent_t: binormal,
            lm_offset,
        }
    }

    fn lightmap_uv(&self, fi: usize, s: f32, t: f32) -> Vec2 {
        let w = self.atlas.width as f32;
        let h = self.atlas.height as f32;
        match self.rects.get(&fi) {
            Some(&(x, y)) => {
                let fl = &self.face_lights[fi];
                let s = (s - fl.mins[0] as f32).clamp(0.0, fl.size[0] as f32);
                let t = (t - fl.mins[1] as f32).clamp(0.0, fl.size[1] as f32);
                Vec2::new((x as f32 + s + 0.5) / w, (y as f32 + t + 0.5) / h)
            }
            None => Vec2::new(2.0 / w, 2.0 / h),
        }
    }

    fn face_polygon(&self, fi: usize) -> Vec<Vec3> {
        let face = &self.bsp.faces[fi];
        let bsp = self.bsp;
        (face.first_edge..face.first_edge + face.num_edges as i32)
            .filter_map(|se| bsp.surface_edges.get(se as usize))
            .filter_map(|se| {
                let e = bsp.edges.get(se.edge_index() as usize)?;
                let vi = match se.direction() {
                    vbsp::EdgeDirection::FirstToLast => e.start_index,
                    vbsp::EdgeDirection::LastToFirst => e.end_index,
                };
                bsp.vertices.get(vi as usize).map(|v| v3(v.position))
            })
            .collect()
    }

    /// An `info_overlay` quad, lit by the lightmap of the first face it covers.
    /// Returns the texinfo and the quad (pushed slightly off the surface).
    pub fn build_overlay(&self, ov: &crate::bsp_raw::Overlay) -> Option<(usize, MeshData)> {
        let face = *ov.faces.first()?;
        if face >= self.bsp.faces.len() {
            return None;
        }
        let normal = ov.normal.normalize_or_zero();
        if normal == Vec3::ZERO {
            return None;
        }
        let basis_u = ov.basis_u.normalize_or_zero();
        let basis_v = normal.cross(basis_u).normalize_or_zero();
        let f = &self.bsp.faces[face];
        let ti = self.bsp.textures_info.get(f.texture_info as usize)?;
        let lu = ti.light_map_scale;
        let lv = ti.light_map_transform;
        let frame = self.face_frame(face, normal);
        let uvs = [
            Vec2::new(ov.u[0], ov.v[0]),
            Vec2::new(ov.u[0], ov.v[1]),
            Vec2::new(ov.u[1], ov.v[1]),
            Vec2::new(ov.u[1], ov.v[0]),
        ];
        let mut mesh = MeshData::default();
        let mut pts = [Vec3::ZERO; 4];
        for i in 0..4 {
            let p = ov.origin + basis_u * ov.points[i].x + basis_v * ov.points[i].y + normal * 0.1;
            pts[i] = p;
            let s = p.dot(Vec3::new(lu[0], lu[1], lu[2])) + lu[3];
            let t = p.dot(Vec3::new(lv[0], lv[1], lv[2])) + lv[3];
            mesh.push_vertex_ex(p, normal, uvs[i], self.lightmap_uv(face, s, t), 0.0, &frame);
        }
        mesh.push_tri(0, 1, 2, [pts[0], pts[1], pts[2]], normal);
        mesh.push_tri(0, 2, 3, [pts[0], pts[2], pts[3]], normal);
        Some((ov.texinfo as usize, mesh))
    }

    /// Append the triangles of face `fi` to `mesh`, relative to `origin` (Source units).
    pub fn build_face(&self, fi: usize, mesh: &mut MeshData, origin: Vec3) {
        let bsp = self.bsp;
        let face = &bsp.faces[fi];
        let Some(ti) = bsp.textures_info.get(face.texture_info as usize) else { return };
        let Some(td) = bsp.textures_data.get(ti.texture_data_index as usize) else { return };
        let tw = td.width.max(1) as f32;
        let th = td.height.max(1) as f32;
        let tu = ti.texture_transforms_u;
        let tv = ti.texture_transforms_v;
        let lu = ti.light_map_scale; // vbsp names: lightmapVecs[0]
        let lv = ti.light_map_transform; // lightmapVecs[1]
        let tex_uv = |p: Vec3| {
            Vec2::new(
                (p.dot(Vec3::new(tu[0], tu[1], tu[2])) + tu[3]) / tw,
                (p.dot(Vec3::new(tv[0], tv[1], tv[2])) + tv[3]) / th,
            )
        };
        let lm_st = |p: Vec3| {
            (
                p.dot(Vec3::new(lu[0], lu[1], lu[2])) + lu[3],
                p.dot(Vec3::new(lv[0], lv[1], lv[2])) + lv[3],
            )
        };
        let Some(plane) = bsp.planes.get(face.plane_num as usize) else { return };
        // `planenum` already names the face-oriented plane; `side` is only
        // relative to the node's plane (verified: windings agree with the
        // unflipped normal on every face).
        let normal = v3(plane.normal);
        let poly = self.face_polygon(fi);
        if poly.len() < 3 {
            return;
        }

        if face.displacement_info >= 0 {
            self.build_displacement(fi, face.displacement_info as usize, &poly, mesh, origin, &tex_uv, &lm_st, normal);
            return;
        }

        let frame = self.face_frame(fi, normal);
        let base: Vec<u32> = poly
            .iter()
            .map(|&p| {
                let (s, t) = lm_st(p);
                mesh.push_vertex_ex(p - origin, normal, tex_uv(p), self.lightmap_uv(fi, s, t), 0.0, &frame)
            })
            .collect();
        for i in 1..poly.len() - 1 {
            mesh.push_tri(base[0], base[i], base[i + 1], [poly[0], poly[i], poly[i + 1]], normal);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_displacement(
        &self,
        fi: usize,
        di: usize,
        poly: &[Vec3],
        mesh: &mut MeshData,
        origin: Vec3,
        tex_uv: &dyn Fn(Vec3) -> Vec2,
        lm_st: &dyn Fn(Vec3) -> (f32, f32),
        normal: Vec3,
    ) {
        let bsp = self.bsp;
        let Some(disp) = bsp.displacements.get(di) else { return };
        if poly.len() != 4 {
            return;
        }
        let start = v3(disp.start_position);
        let first = (0..4)
            .min_by(|&a, &b| {
                (poly[a] - start)
                    .length_squared()
                    .total_cmp(&(poly[b] - start).length_squared())
            })
            .unwrap();
        let c: Vec<Vec3> = (0..4).map(|i| poly[(first + i) % 4]).collect();
        let n = (1usize << disp.power) + 1;
        let verts = bsp
            .displacement_vertices
            .get(disp.displacement_vertex_start as usize..disp.displacement_vertex_start as usize + n * n);
        let Some(verts) = verts else { return };

        // Lightmap coordinates of the corners, interpolated like positions.
        let lmc: Vec<Vec2> = c
            .iter()
            .map(|&p| {
                let (s, t) = lm_st(p);
                Vec2::new(s, t)
            })
            .collect();

        let frame = self.face_frame(fi, normal);
        let mut pos = Vec::with_capacity(n * n);
        let mut idx = Vec::with_capacity(n * n);
        let step = 1.0 / (n - 1) as f32;
        for i in 0..n {
            let fi_ = i as f32 * step;
            let e0 = c[0].lerp(c[1], fi_);
            let e1 = c[3].lerp(c[2], fi_);
            let l0 = lmc[0].lerp(lmc[1], fi_);
            let l1 = lmc[3].lerp(lmc[2], fi_);
            for j in 0..n {
                let fj = j as f32 * step;
                let basep = e0.lerp(e1, fj);
                let dv = &verts[i * n + j];
                let p = basep + v3(dv.vector) * dv.distance;
                let l = l0.lerp(l1, fj);
                let alpha = (dv.alpha / 255.0).clamp(0.0, 1.0);
                idx.push(mesh.push_vertex_ex(p - origin, normal, tex_uv(basep), self.lightmap_uv(fi, l.x, l.y), alpha, &frame));
                pos.push(p);
            }
        }
        // Smooth normals from the displaced grid.
        let mut normals = vec![Vec3::ZERO; n * n];
        let mut tris = Vec::new();
        for i in 0..n - 1 {
            for j in 0..n - 1 {
                let a = i * n + j;
                let b = (i + 1) * n + j;
                let cc = (i + 1) * n + j + 1;
                let d = i * n + j + 1;
                let (t1, t2) = if (i + j) % 2 == 0 { ([a, b, cc], [a, cc, d]) } else { ([a, b, d], [b, cc, d]) };
                for t in [t1, t2] {
                    let mut fnorm = (pos[t[1]] - pos[t[0]]).cross(pos[t[2]] - pos[t[0]]);
                    if fnorm.dot(normal) < 0.0 {
                        fnorm = -fnorm;
                    }
                    for &k in &t {
                        normals[k] += fnorm;
                    }
                    tris.push(t);
                }
            }
        }
        for t in tris {
            mesh.push_tri(idx[t[0]], idx[t[1]], idx[t[2]], [pos[t[0]], pos[t[1]], pos[t[2]]], normal);
        }
        for (k, nrm) in normals.iter().enumerate() {
            mesh.normals[idx[k] as usize] = source_dir_to_world(nrm.normalize_or(normal)).to_array();
            mesh.source_normal[idx[k] as usize] = nrm.normalize_or(normal).to_array();
        }
    }
}

pub fn v3(v: vbsp::Vector) -> Vec3 {
    Vec3::new(v.x, v.y, v.z)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb9e5_roundtrip() {
        for c in [Vec3::new(1.0, 0.5, 0.25), Vec3::new(10.0, 0.01, 3.0), Vec3::splat(0.001)] {
            let r = unpack_rgb9e5(pack_rgb9e5(c));
            assert!((r - c).abs().max_element() <= c.max_element() / 256.0 + 1e-6, "{c} -> {r}");
        }
        assert_eq!(pack_rgb9e5(Vec3::ZERO), 0);
    }

    #[test]
    fn triangles_face_their_normal() {
        let mut m = MeshData::default();
        let p = [Vec3::ZERO, Vec3::X, Vec3::Y];
        let a = m.push_vertex(p[0], Vec3::Z, Vec2::ZERO, Vec2::ZERO, 0.0);
        let b = m.push_vertex(p[1], Vec3::Z, Vec2::ZERO, Vec2::ZERO, 0.0);
        let c = m.push_vertex(p[2], Vec3::Z, Vec2::ZERO, Vec2::ZERO, 0.0);
        // Source +Z normal is world +Y; the emitted triangle must be CCW seen from +Y.
        m.push_tri(a, c, b, p_perm(p, [0, 2, 1]), Vec3::Z);
        let w: Vec<Vec3> = m.indices.iter().map(|&i| Vec3::from(m.positions[i as usize])).collect();
        let n = (w[1] - w[0]).cross(w[2] - w[0]);
        assert!(n.y > 0.0, "{n}");
    }

    fn p_perm(p: [Vec3; 3], o: [usize; 3]) -> [Vec3; 3] {
        [p[o[0]], p[o[1]], p[o[2]]]
    }
}

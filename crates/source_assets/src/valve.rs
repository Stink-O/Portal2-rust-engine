//! Portal 2's own compiled shaders applied to our geometry.
//!
//! For each material we pick the static/dynamic combo the Source shader DLL
//! would (layouts inferred from the shipped `.vcs` files, see
//! `docs` in the README), translate the VS/PS bytecode pair to WGSL
//! ([`crate::sm`]) and prepare the constant registers and sampler bindings
//! the way the engine does (register meanings from the shipped CTABs and the
//! public Source SDK 2013 shader sources). The result is a [`ValveSurface`]
//! component drawn by [`crate::valve_render`].

use crate::fs::GameFs;
use crate::geometry::MeshData;
use crate::sm::{translate_pass, TranslatedPass, VertexSlot};
use crate::vcs::VcsFile;
use crate::vmt::{BlendKind, MaterialInfo};
use bevy::asset::{Assets, Handle};
use bevy::image::Image;
use bevy::prelude::{Component, Resource};
use bevy::shader::Shader;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Which combo of which shader files.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PassKey {
    pub vs: &'static str,
    pub vs_static: u32,
    pub vs_dynamic: u32,
    pub ps: &'static str,
    pub ps_static: u32,
    pub ps_dynamic: u32,
    /// Fixed-function alpha test (`>= ref/255`).
    pub alpha_ref: Option<u8>,
}

/// A translated pass registered as a Bevy shader asset.
pub struct LibraryPass {
    pub id: usize,
    pub key: PassKey,
    pub pass: TranslatedPass,
    pub shader: Handle<Shader>,
    pub fragment_entry: String,
}

#[derive(Default)]
struct LibraryInner {
    vcs: HashMap<&'static str, Option<Arc<VcsFile>>>,
    passes: Vec<Arc<LibraryPass>>,
    index: HashMap<PassKey, Result<usize, String>>,
}

/// Translated passes, shared by every map. Cheap to clone.
#[derive(Resource, Clone, Default)]
pub struct ValveShaderLibrary(Arc<RwLock<LibraryInner>>);

impl ValveShaderLibrary {
    fn vcs(inner: &mut LibraryInner, fs: &GameFs, name: &'static str) -> Option<Arc<VcsFile>> {
        inner
            .vcs
            .entry(name)
            .or_insert_with(|| {
                let data = fs.read(&format!("shaders/fxc/{name}.vcs"))?;
                VcsFile::parse(data).ok().map(Arc::new)
            })
            .clone()
    }

    /// Translate (once) and return a pass, or why it cannot be used.
    pub fn get(&self, fs: &GameFs, shaders: &mut Assets<Shader>, key: PassKey) -> Result<Arc<LibraryPass>, String> {
        let mut inner = self.0.write().unwrap();
        if let Some(r) = inner.index.get(&key) {
            return r.clone().map(|i| inner.passes[i].clone());
        }
        let result = (|| {
            let vs = Self::vcs(&mut inner, fs, key.vs).ok_or(format!("no {}", key.vs))?;
            let ps = Self::vcs(&mut inner, fs, key.ps).ok_or(format!("no {}", key.ps))?;
            let vcode = vs
                .combo(key.vs_static, key.vs_dynamic)
                .ok_or(format!("{} combo {}/{} missing", key.vs, key.vs_static, key.vs_dynamic))?;
            let pcode = ps
                .combo(key.ps_static, key.ps_dynamic)
                .ok_or(format!("{} combo {}/{} missing", key.ps, key.ps_static, key.ps_dynamic))?;
            let alpha = key.alpha_ref.map(|r| d3d9_sm3::AlphaTest {
                func: d3d9_sm3::CompareFunc::GreaterEqual,
                reference: r,
            });
            translate_pass(&vcode, &pcode, alpha).map_err(|e| e.to_string())
        })();
        let r = result.map(|pass| {
            let id = inner.passes.len();
            let fragment_entry = if key.alpha_ref.is_some() {
                d3d9_sm3::pass_fragment_alpha_test_entry(0)
            } else {
                d3d9_sm3::PASS_FRAGMENT_ENTRY.to_string()
            };
            let shader = shaders.add(Shader::from_wgsl(
                pass.wgsl.source.clone(),
                format!("source_assets/valve/{}_{}_{}__{}_{}_{}.wgsl", key.vs, key.vs_static, key.vs_dynamic, key.ps, key.ps_static, key.ps_dynamic),
            ));
            inner.passes.push(Arc::new(LibraryPass {
                id,
                key,
                pass,
                shader,
                fragment_entry,
            }));
            id
        });
        inner.index.insert(key, r.clone());
        r.map(|i| inner.passes[i].clone())
    }
}

/// How a texture is sampled (D3D sampler state).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct SamplerKey {
    pub clamp_s: bool,
    pub clamp_t: bool,
    pub point: bool,
}

impl SamplerKey {
    pub fn index(self) -> u16 {
        self.clamp_s as u16 | (self.clamp_t as u16) << 1 | (self.point as u16) << 2
    }
    pub fn from_index(i: u16) -> Self {
        SamplerKey {
            clamp_s: i & 1 != 0,
            clamp_t: i & 2 != 0,
            point: i & 4 != 0,
        }
    }
    pub fn from_vtf_flags(flags: u32) -> Self {
        SamplerKey {
            clamp_s: flags & crate::vtf::FLAG_CLAMPS != 0,
            clamp_t: flags & crate::vtf::FLAG_CLAMPT != 0,
            point: flags & crate::vtf::FLAG_POINTSAMPLE != 0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TexBinding {
    /// Sampler register (`s0`, `s1`, ...).
    pub register: u16,
    pub image: Handle<Image>,
    pub cube: bool,
    pub sampler: SamplerKey,
}

/// Fat vertex: every attribute our shaders may read, at the locations of
/// [`VertexSlot::location`]. Colour is BGRA bytes (D3DCOLOR).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct FatVertex {
    pub position: [f32; 4],
    pub normal: [f32; 4],
    pub color: [u8; 4],
    pub texcoord: [[f32; 4]; 4],
    pub tangent: [f32; 4],
    pub binormal: [f32; 4],
    /// COLOR1 (BGRA), static-prop vertex lighting.
    pub color1: [u8; 4],
}

impl FatVertex {
    pub const STRIDE: u64 = std::mem::size_of::<FatVertex>() as u64;

    /// Byte offset of an attribute slot, `None` for slots we do not carry.
    pub fn offset(slot: VertexSlot) -> Option<u64> {
        Some(match slot {
            VertexSlot::Position => 0,
            VertexSlot::Normal => 16,
            VertexSlot::Color => 32,
            VertexSlot::TexCoord(i) if i < 4 => 36 + 16 * i as u64,
            VertexSlot::Tangent => 100,
            VertexSlot::Binormal => 116,
            VertexSlot::Color1 => 132,
            _ => return None,
        })
    }
}

/// Everything needed to draw one mesh with a Valve shader.
pub struct SurfaceDraw {
    pub pass: Arc<LibraryPass>,
    pub vertices: Arc<Vec<FatVertex>>,
    pub indices: Arc<Vec<u32>>,
    /// Static vertex / pixel constants (register, value).
    pub vs_consts: Vec<(u16, [f32; 4])>,
    pub ps_consts: Vec<(u16, [f32; 4])>,
    pub textures: Vec<TexBinding>,
    pub blend: BlendKind,
    pub double_sided: bool,
    pub decal: bool,
    /// Map fog (VS `cFogParams`) and linear fog colour.
    pub fog: [f32; 4],
    pub fog_color: [f32; 3],
    /// `.w` of the eye-position pixel constant (VertexLitGeneric: 1 when the
    /// base texture's alpha is translucency).
    pub eye_w: f32,
    /// Material name, for diagnostics.
    pub material: String,
}

/// Draw this entity's mesh with Portal 2's shaders. Vertex positions are in
/// Source units relative to the entity; its `GlobalTransform` places it.
#[derive(Component, Clone)]
pub struct ValveSurface(pub Arc<SurfaceDraw>);

/// Map-wide inputs for building surfaces.
pub struct MapShaderEnv<'a> {
    pub fs: &'a GameFs,
    pub fog: [f32; 4],
    pub fog_color: [f32; 3],
    pub lightmap: Handle<Image>,
    /// Texture lookup: (name, srgb) -> (handle, sampler, is cube).
    pub texture: &'a mut dyn FnMut(&str, bool) -> Option<(Handle<Image>, SamplerKey, bool)>,
    pub white: Handle<Image>,
    pub black: Handle<Image>,
    pub grey: Handle<Image>,
    pub flat_normal: Handle<Image>,
}

/// VS `cFogParams` for linear fog from `start` to `end` (Source units) capped at `max_density`.
pub fn fog_params(start: f32, end: f32, max_density: f32) -> [f32; 4] {
    let range = (end - start).max(1.0);
    [-start / range, 1.0, max_density, 1.0 / range]
}

fn splat(v: f32) -> [f32; 4] {
    [v; 4]
}

fn param_vec3(m: &MaterialInfo, key: &str, default: [f32; 3]) -> [f32; 3] {
    m.params
        .get(key)
        .and_then(crate::kv::parse_color)
        .unwrap_or(default)
}

fn param_f32(m: &MaterialInfo, key: &str, default: f32) -> f32 {
    m.params.get_f32(key).unwrap_or(default)
}

/// Build fat vertices from our brush mesh data.
pub fn brush_vertices(mesh: &MeshData) -> Vec<FatVertex> {
    (0..mesh.source_pos.len())
        .map(|i| {
            let p = mesh.source_pos[i];
            let n = mesh.source_normal[i];
            let t = mesh.tangent_s[i];
            let b = mesh.tangent_t[i];
            let uv = mesh.uv0[i];
            let lm = mesh.uv1[i];
            let off = mesh.lm_offset[i];
            let a = (mesh.colors[i][3].clamp(0.0, 1.0) * 255.0).round() as u8;
            FatVertex {
                position: [p[0], p[1], p[2], 1.0],
                normal: [n[0], n[1], n[2], 1.0],
                color: [255, 255, 255, a],
                texcoord: [
                    [uv[0], uv[1], 0.0, 1.0],
                    [lm[0], lm[1], 0.0, 1.0],
                    [off[0], off[1], 0.0, 1.0],
                    [0.0, 0.0, 0.0, 1.0],
                ],
                tangent: [t[0], t[1], t[2], 1.0],
                binormal: [b[0], b[1], b[2], 1.0],
                color1: [0, 0, 0, 255],
            }
        })
        .collect()
}

/// LightmappedGeneric / WorldVertexTransition on brush geometry.
///
/// `lightmappedgeneric_ps20b` static layout (from the combo directory):
/// `BASETEXTURE2 x1 (2), BUMPMAP x2 (3), BUMPMAP2 x6 (2), CUBEMAP x12 (3),
/// ENVMAPMASK x36, BASEALPHAENVMAPMASK x72, SELFILLUM x144,
/// NORMALMAPALPHAENVMAPMASK x288, FANCY_BLENDING x576, (unknown) x1152,
/// DETAIL x2304 (13: blend mode 0..11, 12 = no detail), (flashlight etc.) x29952`.
/// Dynamic: 0 (general path, range fog, no dest-alpha writes).
///
/// `lightmappedgeneric_vs20` static bits: `ENVMAP_MASK 1, TANGENTSPACE 2,
/// BUMPMAP 4, (bump transform) 8, VERTEXCOLOR 16, VERTEXALPHATEXBLENDFACTOR 32,
/// SEAMLESS 64, FANCY_BLENDING 128, ...`; dynamic 0.
pub fn lightmapped_generic(
    lib: &ValveShaderLibrary,
    shaders: &mut Assets<Shader>,
    env: &mut MapShaderEnv,
    m: &MaterialInfo,
    mesh: &MeshData,
) -> Result<SurfaceDraw, String> {
    let p = &m.params;
    let tex = |name: &str, srgb: bool, env: &mut MapShaderEnv| (env.texture)(name, srgb);
    let base = m.base_texture.as_deref().and_then(|t| tex(t, true, env));
    let base2 = m.base_texture2.as_deref().and_then(|t| tex(t, true, env));
    let bump = crate::vmt::texture_param(p, "$bumpmap").and_then(|t| tex(&t, false, env));
    let ssbump = p.get_bool("$ssbump");
    let bump2 = crate::vmt::texture_param(p, "$bumpmap2").and_then(|t| tex(&t, false, env));
    let envmap = crate::vmt::texture_param(p, "$envmap")
        .filter(|e| e != "env_cubemap")
        .and_then(|t| tex(&t, false, env))
        .filter(|t| t.2);
    let envmask = crate::vmt::texture_param(p, "$envmapmask").and_then(|t| tex(&t, false, env));
    let blendmod = crate::vmt::texture_param(p, "$blendmodulatetexture").and_then(|t| tex(&t, false, env));
    let detail_mode = p.get_f32("$detailblendmode").unwrap_or(0.0) as u32;
    let detail = crate::vmt::texture_param(p, "$detail").and_then(|t| tex(&t, detail_mode == 1, env));
    let selfillum = p.get_bool("$selfillum");
    let bamask = p.get_bool("$basealphaenvmapmask") && envmap.is_some();
    let nmamask = p.get_bool("$normalmapalphaenvmapmask") && envmap.is_some() && bump.is_some();

    let b2 = base2.is_some() as u32;
    let bumpv = match (&bump, ssbump) {
        (Some(_), true) => 2,
        (Some(_), false) => 1,
        (None, _) => 0,
    };
    let bump2v = (bump2.is_some() && bump.is_some()) as u32;
    let cube = envmap.is_some() as u32;
    let mask = (envmask.is_some() && envmap.is_some()) as u32;
    let fancy = (blendmod.is_some() && base2.is_some()) as u32;
    let detail_slot = match &detail {
        Some(_) if detail_mode <= 11 => detail_mode,
        _ => 12,
    };
    let ps_static = b2
        + bumpv * 2
        + bump2v * 6
        + cube * 12
        + mask * 36
        + (bamask as u32) * 72
        + (selfillum as u32) * 144
        + (nmamask as u32) * 288
        + fancy * 576
        + detail_slot * 2304;
    let vs_static = mask
        | ((bumpv > 0) as u32) * 2
        | ((bumpv > 0) as u32) * 4
        | ((b2 > 0 || bump2v > 0) as u32) * 32
        | fancy * 128;
    let alpha_ref = (m.blend == BlendKind::Mask).then(|| (m.alpha_cutoff.clamp(0.0, 1.0) * 255.0).round() as u8);
    let key = PassKey {
        vs: "lightmappedgeneric_vs20",
        vs_static,
        vs_dynamic: 0,
        ps: "lightmappedgeneric_ps20b",
        ps_static,
        ps_dynamic: 0,
        alpha_ref,
    };
    if std::env::var_os("SOURCE_VALVE_DEBUG").is_some() {
        bevy::log::info!(
            "valve {}: ps {} (b2 {b2} bump {bumpv} bump2 {bump2v} cube {cube} mask {mask} bamask {bamask} si {selfillum} nma {nmamask} fancy {fancy} detail {detail_slot}) vs {vs_static}",
            m.name, ps_static
        );
    }
    let pass = lib.get(env.fs, shaders, key)?;

    // ---- constants ----
    let color = m.color;
    let alpha = m.alpha;
    let detail_scale = p
        .get("$detailscale")
        .map(|s| {
            let v: Vec<f32> = s
                .trim_matches(|c| c == '[' || c == ']')
                .split_whitespace()
                .filter_map(crate::kv::parse_f32)
                .collect();
            match v.len() {
                0 => [4.0, 4.0],
                1 => [v[0], v[0]],
                _ => [v[0], v[1]],
            }
        })
        .unwrap_or([4.0, 4.0]);
    let mut vs = vec![
        (0, [0.0, 1.0, 2.0, 0.5]),
        (1, [1.0 / 2.2, 2.0, 1.0 / 3.0, 0.5]),
        (14, [1.0, 0.0, 0.0, 0.0]),
        (15, [0.0, 1.0, 0.0, 0.0]),
        (47, [color[0], color[1], color[2], alpha]),
        (48, [1.0, 0.0, 0.0, 0.0]),
        (49, [0.0, 1.0, 0.0, 0.0]),
        (52, [1.0, 0.0, 0.0, 0.0]),
        (53, [0.0, 1.0, 0.0, 0.0]),
    ];
    if detail.is_some() {
        vs.push((50, [detail_scale[0], 0.0, 0.0, 0.0]));
        vs.push((51, [0.0, detail_scale[1], 0.0, 0.0]));
    } else {
        vs.push((50, [1.0, 0.0, 0.0, 0.0]));
        vs.push((51, [0.0, 1.0, 0.0, 0.0]));
    }
    if bump2v > 0 && mask == 0 {
        vs.push((52, [1.0, 0.0, 0.0, 0.0]));
        vs.push((53, [0.0, 1.0, 0.0, 0.0]));
    }
    let envtint = param_vec3(m, "$envmaptint", [1.0; 3]);
    let fresnel = param_f32(m, "$fresnelreflection", 1.0);
    let selftint = param_vec3(m, "$selfillumtint", [1.0; 3]);
    let dtint = param_vec3(m, "$detailtint", [1.0; 3]);
    let dblend = param_f32(m, "$detailblendfactor", 1.0);
    let ps = vec![
        (0, [envtint[0], envtint[1], envtint[2], 1.0]),
        (2, splat(param_f32(m, "$envmapcontrast", 0.0))),
        (3, splat(param_f32(m, "$envmapsaturation", 1.0))),
        (4, [0.0, 0.0, 1.0 - fresnel, fresnel]),
        (7, [selftint[0], selftint[1], selftint[2], 1.0]),
        (8, [dtint[0], dtint[1], dtint[2], if detail.is_some() { dblend } else { 0.0 }]),
        (12, [color[0], color[1], color[2], alpha]),
        (20, [0.0; 4]),
        (31, [0.0; 4]),
    ];

    // ---- samplers ----
    let default_sampler = SamplerKey::default();
    let mut textures = Vec::new();
    let mut bind = |register: u16, t: Option<(Handle<Image>, SamplerKey, bool)>, fallback: &Handle<Image>, cube: bool| {
        let (image, sampler, is_cube) = t.unwrap_or((fallback.clone(), default_sampler, cube));
        textures.push(TexBinding {
            register,
            image,
            cube: is_cube,
            sampler,
        });
    };
    let base_fallback = if envmap.is_some() { env.black.clone() } else { env.white.clone() };
    bind(0, base, &base_fallback, false);
    bind(1, Some((env.lightmap.clone(), SamplerKey { clamp_s: true, clamp_t: true, point: false }, false)), &env.white, false);
    if let Some(e) = envmap {
        bind(2, Some(e), &env.black, true);
    }
    if let Some(b) = blendmod {
        bind(3, Some(b), &env.grey, false);
    }
    if bump.is_some() {
        bind(4, bump, &env.flat_normal, false);
    }
    if bump2v > 0 {
        bind(5, bump2, &env.flat_normal, false);
    } else if mask > 0 {
        bind(5, envmask, &env.white, false);
    }
    if base2.is_some() {
        bind(7, base2, &env.white, false);
    }
    bind(12, detail, &env.grey, false);
    // Only keep samplers the pass declares.
    textures.retain(|t| pass.pass.samplers.iter().any(|(r, _)| *r == t.register));

    Ok(SurfaceDraw {
        pass,
        vertices: Arc::new(brush_vertices(mesh)),
        indices: Arc::new(mesh.indices.clone()),
        vs_consts: vs,
        ps_consts: ps,
        textures,
        blend: m.blend,
        double_sided: m.no_cull,
        decal: m.decal,
        fog: env.fog,
        fog_color: env.fog_color,
        eye_w: 0.0,
        material: m.name.clone(),
    })
}

/// Source world space (Z-up inches) -> Bevy world space (Y-up metres), as a matrix.
pub fn source_to_world_matrix() -> bevy::math::Mat4 {
    let s = shared::SOURCE_UNIT;
    bevy::math::Mat4::from_cols(
        bevy::math::Vec4::new(s, 0.0, 0.0, 0.0),
        bevy::math::Vec4::new(0.0, 0.0, -s, 0.0),
        bevy::math::Vec4::new(0.0, s, 0.0, 0.0),
        bevy::math::Vec4::new(0.0, 0.0, 0.0, 1.0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::Vec3;

    #[test]
    fn source_matrix_matches_shared_conversion() {
        let v = Vec3::new(1.0, 2.0, 3.0);
        let m = source_to_world_matrix();
        assert!((m.transform_point3(v) - shared::source_to_world(v)).length() < 1e-6);
    }
}


/// Encode linear light `l` into the COLOR1 byte the VertexLitGeneric VS
/// decodes as `(2 * byte / 255) ^ 2.2`.
pub fn encode_static_light(l: bevy::math::Vec3) -> [u8; 4] {
    let e = |v: f32| ((v.max(0.0).powf(1.0 / 2.2) / 2.0) * 255.0).round().clamp(0.0, 255.0) as u8;
    [e(l.z), e(l.y), e(l.x), 255]
}

/// Model mesh vertices for the Valve path (Source units, model space, posed).
pub fn model_vertices(mesh: &crate::models::ModelMesh, light: &dyn Fn(usize) -> [u8; 4]) -> Vec<FatVertex> {
    (0..mesh.positions.len())
        .map(|i| {
            let w = bevy::math::Vec3::from(mesh.positions[i]) / shared::SOURCE_UNIT;
            let p = [w.x, -w.z, w.y];
            let n = mesh.source_normals[i];
            let uv = mesh.uvs[i];
            FatVertex {
                position: [p[0], p[1], p[2], 1.0],
                normal: [n.x, n.y, n.z, 1.0],
                color: [255, 255, 255, 255],
                texcoord: [[uv[0], uv[1], 0.0, 1.0], [0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 0.0, 1.0]],
                tangent: [1.0, 0.0, 0.0, 1.0],
                binormal: [0.0, 1.0, 0.0, 1.0],
                color1: light(i),
            }
        })
        .collect()
}

/// VertexLitGeneric / UnlitGeneric on studio models.
///
/// `vertexlit_and_unlit_generic_ps20b` static digits used: `DETAILTEXTURE x1,
/// CUBEMAP x2, DIFFUSELIGHTING x4, ENVMAPMASK x8, SELFILLUM x32 (samples a
/// mask at s11)`. VS static: 0 lit, 16 unlit. VS dynamic 1: static
/// per-vertex light from COLOR1 (the ambient-cube light of dynamic props is
/// baked into COLOR1 too). PS dynamic 0.
pub fn vertex_lit_generic(
    lib: &ValveShaderLibrary,
    shaders: &mut Assets<Shader>,
    env: &mut MapShaderEnv,
    m: &MaterialInfo,
    vertices: Vec<FatVertex>,
    indices: Vec<u32>,
) -> Result<SurfaceDraw, String> {
    let p = &m.params;
    let tex = |name: &str, srgb: bool, env: &mut MapShaderEnv| (env.texture)(name, srgb);
    let base = m.base_texture.as_deref().and_then(|t| tex(t, true, env));
    let envmap = crate::vmt::texture_param(p, "$envmap")
        .filter(|e| e != "env_cubemap")
        .and_then(|t| tex(&t, false, env))
        .filter(|t| t.2);
    let envmask = crate::vmt::texture_param(p, "$envmapmask").and_then(|t| tex(&t, false, env));
    let detail_mode = p.get_f32("$detailblendmode").unwrap_or(0.0) as u32;
    let detail = crate::vmt::texture_param(p, "$detail").and_then(|t| tex(&t, detail_mode == 1, env));
    let selfillum = p.get_bool("$selfillum");
    let simask = crate::vmt::texture_param(p, "$selfillummask").and_then(|t| tex(&t, false, env));
    let lit = m.shader != "unlitgeneric";

    let ps_static = detail.is_some() as u32
        + envmap.is_some() as u32 * 2
        + lit as u32 * 4
        + (envmask.is_some() && envmap.is_some()) as u32 * 8
        + selfillum as u32 * 32;
    let vs_static = if lit { 0 } else { 16 };
    let alpha_ref = (m.blend == BlendKind::Mask).then(|| (m.alpha_cutoff.clamp(0.0, 1.0) * 255.0).round() as u8);
    let key = PassKey {
        vs: "vertexlit_and_unlit_generic_vs20",
        vs_static,
        vs_dynamic: if lit { 1 } else { 0 },
        ps: "vertexlit_and_unlit_generic_ps20b",
        ps_static,
        ps_dynamic: 0,
        alpha_ref,
    };
    if std::env::var_os("SOURCE_VALVE_DEBUG").is_some() {
        bevy::log::info!("valve model {}: vs {vs_static} ps {ps_static}", m.name);
    }
    let pass = lib.get(env.fs, shaders, key)?;
    let color = m.color;
    let alpha = m.alpha;
    let envtint = param_vec3(m, "$envmaptint", [1.0; 3]);
    let selftint = param_vec3(m, "$selfillumtint", [1.0; 3]);
    let dtint = param_vec3(m, "$detailtint", [1.0; 3]);
    let dscale = param_f32(m, "$detailscale", 4.0);
    let vs = vec![
        (0, [0.0, 1.0, 2.0, 0.5]),
        (13, [0.0; 4]),
        (48, [1.0, 0.0, 0.0, 0.0]),
        (49, [0.0, 1.0, 0.0, 0.0]),
        (50, [1.0, 0.0, 0.0, 0.0]),
        (52, [dscale, 0.0, 0.0, 0.0]),
        (53, [0.0, dscale, 0.0, 0.0]),
    ];
    let lin = |c: [f32; 3]| [c[0].powf(2.2), c[1].powf(2.2), c[2].powf(2.2)];
    let ct = lin(color);
    let et = lin(envtint);
    let dt = lin(dtint);
    let ps = vec![
        (0, [et[0], et[1], et[2], 0.0]),
        (1, [ct[0], ct[1], ct[2], alpha]),
        (2, [param_f32(m, "$envmapcontrast", 0.0), 0.0, 0.0, 0.0]),
        (3, [1.0, 1.0, 1.0, simask.is_some() as u32 as f32]),
        (4, [selftint[0], selftint[1], selftint[2], param_f32(m, "$detailblendfactor", 1.0)]),
        (10, [dt[0], dt[1], dt[2], 1.0]),
        (11, [1.0, 0.0, 0.0, 0.0]),
        (12, [0.0; 4]),
    ];
    let default_sampler = SamplerKey::default();
    let mut textures = Vec::new();
    let mut bind = |register: u16, t: Option<(Handle<Image>, SamplerKey, bool)>, fallback: &Handle<Image>, cube: bool| {
        let (image, sampler, is_cube) = t.unwrap_or((fallback.clone(), default_sampler, cube));
        textures.push(TexBinding { register, image, cube: is_cube, sampler });
    };
    let base_or_white = base.clone();
    bind(0, base, &env.white, false);
    if let Some(e) = envmap {
        bind(1, Some(e), &env.black, true);
    }
    bind(2, detail, &env.grey, false);
    bind(4, envmask, &env.white, false);
    // no mask: Source uses the base texture's alpha
    bind(11, simask.or(base_or_white), &env.white, false);
    textures.retain(|t| pass.pass.samplers.iter().any(|(r, _)| *r == t.register));
    Ok(SurfaceDraw {
        pass,
        vertices: Arc::new(vertices),
        indices: Arc::new(indices),
        vs_consts: vs,
        ps_consts: ps,
        textures,
        blend: m.blend,
        double_sided: m.no_cull,
        decal: false,
        fog: env.fog,
        fog_color: env.fog_color,
        eye_w: if m.blend == BlendKind::Opaque { 0.0 } else { 1.0 },
        material: m.name.clone(),
    })
}

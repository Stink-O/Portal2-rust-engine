//! Portal 2's compiled D3D9 shaders (from the `.vcs` files) translated to
//! WGSL with the vendored `d3d9_sm3` crate.
//!
//! This module links a vertex/pixel program pair into one WGSL module:
//! vertex inputs come from `dcl` tokens and are given fixed attribute
//! locations ([`VertexSlot`]), SM2 outputs are linked by register
//! (`oTn` -> `tn`, `oDn` -> `vn`), SM3 by semantic.
//!
//! The emitted module's bindings (see `d3d9_sm3::lower_pass_to_wgsl`):
//! `@group(0) @binding(0)` a storage buffer of `vec4<f32>` holding, per draw
//! at base `instance_index`: vertex constants `c0..cN`, then pixel constants,
//! then packed texture/sampler slot words; `@group(1)` binding arrays of
//! 2D / cube textures and samplers.

use d3d9_decl::{DeclType, Semantic};
use d3d9_sm3::{
    decode_sm3_program, lower_pass_to_wgsl, ConstantSlot, PassLoweringAbi, PassWgsl, SamplerSlot,
    SamplerTextureDimension, ShaderStage, Sm3InstructionBody, Sm3Opcode, Sm3ProgramIr, Sm3Register,
    VaryingLink, VertexInput,
};
use d3d9_sm3::ir::Sm3RegisterFile;
use std::collections::BTreeMap;

pub const USAGE_POSITION: u8 = 0;
pub const USAGE_BLENDWEIGHT: u8 = 1;
pub const USAGE_BLENDINDICES: u8 = 2;
pub const USAGE_NORMAL: u8 = 3;
pub const USAGE_TEXCOORD: u8 = 5;
pub const USAGE_TANGENT: u8 = 6;
pub const USAGE_BINORMAL: u8 = 7;
pub const USAGE_COLOR: u8 = 10;
pub const USAGE_FOG: u8 = 11;

/// The vertex attributes we can feed, with their fixed locations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum VertexSlot {
    Position,
    Normal,
    Color,
    /// COLOR1: static-prop per-vertex lighting stream.
    Color1,
    TexCoord(u8),
    Tangent,
    Binormal,
    BlendWeight,
    BlendIndices,
}

impl VertexSlot {
    pub fn from_semantic(s: Semantic) -> Option<Self> {
        Some(match (s.usage, s.usage_index) {
            (USAGE_POSITION, 0) => VertexSlot::Position,
            (USAGE_NORMAL, 0) => VertexSlot::Normal,
            (USAGE_COLOR, 0) => VertexSlot::Color,
            (USAGE_COLOR, 1) => VertexSlot::Color1,
            (USAGE_TEXCOORD, i) if i < 8 => VertexSlot::TexCoord(i),
            (USAGE_TANGENT, 0) => VertexSlot::Tangent,
            (USAGE_BINORMAL, 0) => VertexSlot::Binormal,
            (USAGE_BLENDWEIGHT, 0) => VertexSlot::BlendWeight,
            (USAGE_BLENDINDICES, 0) => VertexSlot::BlendIndices,
            _ => return None,
        })
    }

    /// Attribute location in our vertex buffers.
    pub fn location(self) -> u32 {
        match self {
            VertexSlot::Position => 0,
            VertexSlot::Normal => 1,
            VertexSlot::Color => 2,
            VertexSlot::TexCoord(i) => 3 + i as u32,
            VertexSlot::Tangent => 11,
            VertexSlot::Binormal => 12,
            VertexSlot::BlendWeight => 13,
            VertexSlot::BlendIndices => 14,
            VertexSlot::Color1 => 15,
        }
    }

    /// Everything is fed as float4 except colour (unorm bytes).
    pub fn decl_type(self) -> DeclType {
        match self {
            VertexSlot::Color | VertexSlot::Color1 | VertexSlot::BlendIndices => DeclType::D3dColor,
            _ => DeclType::Float4,
        }
    }
}

fn decl_semantic(usage_token: u32) -> Semantic {
    Semantic::new((usage_token & 0x1f) as u8, ((usage_token >> 16) & 0xf) as u8)
}

/// Program facts gathered from the token stream.
#[derive(Debug, Default)]
pub struct ProgramInfo {
    pub inputs: Vec<(Sm3Register, Semantic)>,
    /// Declared outputs (SM3) or implicitly written ones (SM2), with semantics.
    pub outputs: Vec<(Sm3Register, Semantic)>,
    pub samplers: Vec<(u16, SamplerTextureDimension)>,
    pub float_constants: Vec<u16>,
    pub defined_constants: Vec<u16>,
    pub opcodes: BTreeMap<String, usize>,
    pub major: u8,
}

pub fn program_info(ir: &Sm3ProgramIr, major: u8) -> ProgramInfo {
    let mut info = ProgramInfo {
        major,
        ..Default::default()
    };
    let mut used_consts = std::collections::BTreeSet::new();
    let mut written = std::collections::BTreeSet::new();
    for ins in &ir.instructions {
        *info.opcodes.entry(format!("{:?}", ins.opcode)).or_default() += 1;
        match &ins.body {
            Sm3InstructionBody::Declaration { usage_token, destination } => {
                let reg = destination.register;
                match reg.file {
                    Sm3RegisterFile::Sampler => {
                        let ty = usage_token.map(|t| (t >> 27) & 0xf).unwrap_or(2);
                        let dim = match ty {
                            3 => SamplerTextureDimension::Cube,
                            4 => SamplerTextureDimension::D3,
                            _ => SamplerTextureDimension::D2,
                        };
                        info.samplers.push((reg.index, dim));
                    }
                    Sm3RegisterFile::Input => {
                        let sem = usage_token.map(decl_semantic).unwrap_or(Semantic::new(USAGE_COLOR, reg.index as u8));
                        info.inputs.push((reg, sem));
                    }
                    Sm3RegisterFile::Texture => {
                        info.inputs.push((reg, Semantic::texcoord(reg.index as u8)));
                    }
                    Sm3RegisterFile::Output => {
                        if let Some(t) = usage_token {
                            info.outputs.push((reg, decl_semantic(*t)));
                        }
                    }
                    _ => {}
                }
            }
            Sm3InstructionBody::FloatDefinition { destination, .. } => {
                info.defined_constants.push(destination.register.index);
            }
            Sm3InstructionBody::Operation { destination, sources } => {
                for s in sources {
                    if s.register.file == Sm3RegisterFile::FloatConstant {
                        if s.relative.is_some() {
                            // relative addressing: the whole bank may be read
                            used_consts.insert(u16::MAX);
                        } else {
                            used_consts.insert(s.register.index);
                        }
                    }
                }
                if let Some(d) = destination {
                    written.insert(d.register);
                }
            }
            _ => {}
        }
    }
    // SM2 vertex outputs are implicit.
    if major < 3 {
        for r in written {
            let sem = match r.file {
                Sm3RegisterFile::RasterOutput if r.index == 0 => Semantic::position(0),
                Sm3RegisterFile::RasterOutput if r.index == 1 => Semantic::new(USAGE_FOG, 0),
                Sm3RegisterFile::AttributeOutput => Semantic::new(USAGE_COLOR, r.index as u8),
                // vs_2_0 oTn share the Output file number
                Sm3RegisterFile::Output => Semantic::texcoord(r.index as u8),
                _ => continue,
            };
            info.outputs.push((r, sem));
        }
    }
    info.float_constants = used_consts.into_iter().collect();
    info
}

/// Why a pair could not be translated.
#[derive(Debug)]
pub enum TranslateError {
    Decode(String),
    Unsupported(String),
    Lower(String),
    Validate(String),
}

impl std::fmt::Display for TranslateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

/// A translated pass, ready to become a render pipeline.
#[derive(Clone, Debug)]
pub struct TranslatedPass {
    pub wgsl: PassWgsl,
    /// Vertex attributes the VS reads.
    pub vertex_slots: Vec<VertexSlot>,
    /// Pixel samplers in slot order (slot word index) with dimension.
    pub samplers: Vec<(u16, SamplerTextureDimension)>,
    /// Number of vec4 vertex / pixel constants in the per-draw arena.
    pub vertex_constants: usize,
    pub pixel_constants: usize,
}

fn stage_of(code: &[u8]) -> Option<(ShaderStage, u8)> {
    let (pixel, major, _) = crate::vcs::shader_version(code)?;
    Some((if pixel { ShaderStage::Pixel } else { ShaderStage::Vertex }, major))
}

/// Translate a VS + PS bytecode pair into one WGSL module.
pub fn translate_pass(vs: &[u8], ps: &[u8], alpha_test: Option<d3d9_sm3::AlphaTest>) -> Result<TranslatedPass, TranslateError> {
    let (vstage, vmajor) = stage_of(vs).ok_or_else(|| TranslateError::Decode("vs version".into()))?;
    let (pstage, pmajor) = stage_of(ps).ok_or_else(|| TranslateError::Decode("ps version".into()))?;
    if vstage != ShaderStage::Vertex || pstage != ShaderStage::Pixel {
        return Err(TranslateError::Decode("stage mismatch".into()));
    }
    let vir = decode_sm3_program(vs, ShaderStage::Vertex).map_err(|e| TranslateError::Decode(format!("vs {e:?}")))?;
    let pir = decode_sm3_program(ps, ShaderStage::Pixel).map_err(|e| TranslateError::Decode(format!("ps {e:?}")))?;
    let vi = program_info(&vir, vmajor);
    let pi = program_info(&pir, pmajor);

    let mut vertex_inputs = Vec::new();
    let mut vertex_slots = Vec::new();
    for (reg, sem) in &vi.inputs {
        let slot = VertexSlot::from_semantic(*sem);
        if let Some(s) = slot {
            vertex_slots.push(s);
        }
        vertex_inputs.push(VertexInput {
            register: *reg,
            location: slot.map(|s| s.location()),
            decl_type: slot.map(|s| s.decl_type()),
            semantic: *sem,
        });
    }

    // Link VS outputs to PS inputs.
    let mut position = None;
    let mut varyings = Vec::new();
    for (reg, sem) in &vi.outputs {
        if sem.usage == USAGE_POSITION {
            position = Some(VaryingLink { semantic: *sem, vertex_register: *reg, pixel_register: None, location: 0 });
            continue;
        }
        let pixel_register = pi.inputs.iter().find(|(pr, psem)| {
            if pmajor >= 3 {
                psem == sem
            } else {
                match (reg.file, pr.file) {
                    (Sm3RegisterFile::Output, Sm3RegisterFile::Texture) => pr.index == reg.index,
                    (Sm3RegisterFile::AttributeOutput, Sm3RegisterFile::Input) => pr.index == reg.index,
                    _ => false,
                }
            }
        });
        let location = varyings.len() as u32;
        varyings.push(VaryingLink {
            semantic: *sem,
            vertex_register: *reg,
            pixel_register: pixel_register.map(|p| p.0),
            location,
        });
    }
    let position = position.ok_or_else(|| TranslateError::Unsupported("vs writes no position".into()))?;
    // Texcoords beyond 3 are not in our vertex format.
    if vertex_slots.iter().any(|s| matches!(s, VertexSlot::TexCoord(i) if *i > 3)) {
        return Err(TranslateError::Unsupported("vs reads texcoord > 3".into()));
    }
    // PS inputs the VS never writes: refuse (the engine would read garbage too).
    for (pr, _) in &pi.inputs {
        if !varyings.iter().any(|v| v.pixel_register == Some(*pr)) {
            return Err(TranslateError::Unsupported(format!("ps input {pr:?} not written by vs")));
        }
    }

    let consts = |info: &ProgramInfo| -> Vec<ConstantSlot> {
        let max = if info.float_constants.contains(&u16::MAX) {
            255
        } else {
            info.float_constants.iter().copied().max().unwrap_or(0).max(info.defined_constants.iter().copied().max().unwrap_or(0))
        };
        (0..=max)
            .map(|r| ConstantSlot { register: r, program_defined: info.defined_constants.contains(&r) })
            .collect()
    };
    let abi = PassLoweringAbi {
        vertex_inputs,
        position,
        varyings,
        vertex_constants: consts(&vi),
        pixel_constants: consts(&pi),
        samplers: pi
            .samplers
            .iter()
            .map(|(r, d)| SamplerSlot { register: *r, dimension: *d })
            .collect(),
        alpha_tests: alpha_test.into_iter().collect(),
    };
    let wgsl = lower_pass_to_wgsl(&abi, &vir, &pir).map_err(|e| TranslateError::Lower(format!("{e:?}")))?;
    validate_wgsl(&wgsl.source)?;
    Ok(TranslatedPass {
        vertex_constants: wgsl.vertex_constant_len,
        pixel_constants: wgsl.pixel_constant_len,
        wgsl,
        vertex_slots,
        samplers: pi.samplers.clone(),
    })
}

/// Parse and validate WGSL with naga so a bad translation falls back
/// instead of failing pipeline creation.
pub fn validate_wgsl(src: &str) -> Result<(), TranslateError> {
    let module = naga::front::wgsl::parse_str(src).map_err(|e| TranslateError::Validate(e.emit_to_string(src)))?;
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
        .validate(&module)
        .map_err(|e| TranslateError::Validate(format!("{e:?}")))?;
    Ok(())
}

/// Opcode histogram of a program (diagnostics).
pub fn opcode_names(code: &[u8]) -> Vec<String> {
    let Some((stage, major)) = stage_of(code) else { return Vec::new() };
    let Ok(ir) = decode_sm3_program(code, stage) else { return vec!["<decode error>".into()] };
    let _ = Sm3Opcode::Nop;
    program_info(&ir, major).opcodes.keys().cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcs::VcsFile;

    fn vcs(fs: &crate::fs::GameFs, name: &str) -> VcsFile {
        VcsFile::parse(fs.read(&format!("shaders/fxc/{name}.vcs")).unwrap()).unwrap()
    }

    #[test]
    fn translates_lightmappedgeneric_base_combo() {
        let cfg = crate::config::GameConfig::from_env();
        if !cfg.root.join("portal2").is_dir() {
            return;
        }
        let fs = crate::fs::GameFs::mount(cfg).unwrap();
        let vs = vcs(&fs, "lightmappedgeneric_vs20");
        let ps = vcs(&fs, "lightmappedgeneric_ps20b");
        let v = vs.static_block(0).unwrap();
        let p = ps.static_block(0).unwrap();
        let r = translate_pass(&v[0].1, &p[0].1, None);
        match &r {
            Ok(t) => {
                eprintln!("ok: slots {:?} samplers {:?} vc {} pc {}", t.vertex_slots, t.samplers, t.vertex_constants, t.pixel_constants);
                std::fs::write(crate::config::GameConfig::cache_dir().join("lmg_base.wgsl"), &t.wgsl.source).ok();
            }
            Err(e) => eprintln!("err: {e}"),
        }
        r.unwrap();
    }
}

/// Samplers actually read by `texld` instructions (diagnostics).
pub fn texld_samplers(code: &[u8]) -> Vec<u16> {
    let Some((stage, _)) = stage_of(code) else { return Vec::new() };
    let Ok(ir) = decode_sm3_program(code, stage) else { return Vec::new() };
    let mut out: Vec<u16> = ir
        .instructions
        .iter()
        .filter(|i| matches!(i.opcode, Sm3Opcode::TexLd | Sm3Opcode::TexLdD | Sm3Opcode::TexLdL))
        .filter_map(|i| match &i.body {
            Sm3InstructionBody::Operation { sources, .. } => sources
                .iter()
                .find(|s| s.register.file == Sm3RegisterFile::Sampler)
                .map(|s| s.register.index),
            _ => None,
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Inputs and outputs of a program as short strings (diagnostics).
pub fn io_summary(code: &[u8]) -> String {
    let Some((stage, major)) = stage_of(code) else { return String::new() };
    let Ok(ir) = decode_sm3_program(code, stage) else { return "<decode error>".into() };
    let info = program_info(&ir, major);
    let sem = |s: &Semantic| format!("{}.{}", s.usage, s.usage_index);
    format!(
        "in[{}] out[{}]",
        info.inputs.iter().map(|(r, s)| format!("{:?}{}={}", r.file, r.index, sem(s))).collect::<Vec<_>>().join(" "),
        info.outputs.iter().map(|(r, s)| format!("{:?}{}={}", r.file, r.index, sem(s))).collect::<Vec<_>>().join(" ")
    )
}

#[cfg(test)]
mod dump_tests {
    /// Writes the WGSL of chosen combo pairs to cache/source_assets for inspection.
    #[test]
    fn dump_vertexlit_pair() {
        let cfg = crate::config::GameConfig::from_env();
        if !cfg.root.join("portal2").is_dir() {
            return;
        }
        let fs = crate::fs::GameFs::mount(cfg).unwrap();
        let v = crate::vcs::VcsFile::parse(fs.read("shaders/fxc/vertexlit_and_unlit_generic_vs20.vcs").unwrap()).unwrap();
        let p = crate::vcs::VcsFile::parse(fs.read("shaders/fxc/vertexlit_and_unlit_generic_ps20b.vcs").unwrap()).unwrap();
        let vc = v.combo(0, 1).unwrap();
        let pc = p.combo(4, 0).unwrap();
        match super::translate_pass(&vc, &pc, None) {
            Ok(t) => {
                std::fs::create_dir_all(crate::config::GameConfig::cache_dir()).ok();
                std::fs::write(crate::config::GameConfig::cache_dir().join("vlg.wgsl"), &t.wgsl.source).unwrap();
            }
            Err(e) => panic!("{e}"),
        }
    }
}

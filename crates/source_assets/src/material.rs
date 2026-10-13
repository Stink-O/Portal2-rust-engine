//! The Bevy material used for all Source surfaces: base texture times baked
//! light (lightmap for brushes, per-vertex for props), unaffected by Bevy
//! lights so a chamber keeps its own lighting wherever it is placed.

use bevy::asset::embedded_asset;
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
};
use bevy::shader::ShaderRef;

pub const FLAG_LIGHTMAP: u32 = 1;
pub const FLAG_VERTEX_LIGHT: u32 = 2;
pub const FLAG_UNLIT: u32 = 4;
pub const FLAG_BLEND2: u32 = 8;
pub const FLAG_ALPHA_TEST: u32 = 16;
pub const FLAG_IGNORE_TEX_ALPHA: u32 = 32;
/// Debug: output only the baked light.
pub const FLAG_LIGHT_ONLY: u32 = 64;

#[derive(Clone, Copy, Debug, ShaderType)]
pub struct SourceMaterialParams {
    /// RGB tint and overall alpha.
    pub color: Vec4,
    /// Multiplier on baked light (lightmap or vertex light).
    pub light_scale: f32,
    pub alpha_cutoff: f32,
    pub flags: u32,
    /// `StandardMaterialFlags` bits (fog enable, alpha mode) for Bevy's post-lighting step.
    pub pbr_flags: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SourceMaterialKey {
    pub double_sided: bool,
}

impl From<&SourceMaterial> for SourceMaterialKey {
    fn from(m: &SourceMaterial) -> Self {
        SourceMaterialKey {
            double_sided: m.double_sided,
        }
    }
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
#[bind_group_data(SourceMaterialKey)]
pub struct SourceMaterial {
    #[uniform(0)]
    pub params: SourceMaterialParams,
    #[texture(1)]
    #[sampler(2)]
    pub base_texture: Option<Handle<Image>>,
    #[texture(3)]
    #[sampler(4)]
    pub lightmap: Option<Handle<Image>>,
    #[texture(5)]
    #[sampler(6)]
    pub base_texture2: Option<Handle<Image>>,
    pub alpha_mode: AlphaMode,
    pub double_sided: bool,
}

impl Material for SourceMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://source_assets/shaders/source_material.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        self.alpha_mode
    }

    // The default prepass cannot alpha-test our textures; opting out keeps
    // grates and foliage from writing solid depth.
    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        if key.bind_group_data.double_sided {
            descriptor.primitive.cull_mode = None;
        }
        Ok(())
    }
}

pub(crate) fn register(app: &mut App) {
    embedded_asset!(app, "shaders/source_material.wgsl");
    app.add_plugins(MaterialPlugin::<SourceMaterial>::default());
}

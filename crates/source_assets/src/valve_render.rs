//! Render-world side of the Valve-shader path: draws every [`ValveSurface`]
//! into each 3D view right after Bevy's opaque pass, sharing its colour and
//! depth targets.
//!
//! Bindings match what `d3d9_sm3::lower_pass_to_wgsl` emits: group 0 is one
//! storage buffer of `vec4<f32>` constants (each draw finds its block via
//! `instance_index` = first instance), group 1 holds binding arrays of 2D
//! and cube textures plus a small sampler table.

use crate::valve::{FatVertex, SamplerKey, SurfaceDraw, ValveSurface};
use crate::vmt::BlendKind;
use bevy::core_pipeline::core_3d::{main_opaque_pass_3d, main_transparent_pass_3d, CORE_3D_DEPTH_FORMAT};
use bevy::core_pipeline::schedule::Core3d;
use bevy::core_pipeline::Core3dSystems;
use bevy::image::Image;
use bevy::mesh::VertexBufferLayout;
use bevy::math::{Mat4, Vec3};
use bevy::prelude::*;
use bevy::render::camera::ExtractedCamera;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::*;
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::texture::GpuImage;
use bevy::render::view::{ExtractedView, ViewDepthTexture, ViewTarget};
use bevy::render::{Extract, ExtractSchedule, Render, RenderApp, RenderSystems};
use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::Arc;

/// Global exposure applied as Source's tonemap scale (`cLightScale.x`).
#[derive(Resource, Clone, Copy, Debug)]
pub struct ValveExposure(pub f32);

impl Default for ValveExposure {
    fn default() -> Self {
        ValveExposure(crate::plugin::DEFAULT_LIGHT_SCALE)
    }
}

pub struct ValveRenderPlugin;

impl Plugin for ValveRenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ValveExposure>();
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app
            .init_resource::<ExtractedSurfaces>()
            .init_resource::<PreparedSurfaces>()
            .add_systems(ExtractSchedule, extract_surfaces)
            .add_systems(Render, prepare_surfaces.in_set(RenderSystems::PrepareBindGroups))
            .add_systems(
                Core3d,
                valve_main_pass
                    .in_set(Core3dSystems::MainPass)
                    .after(main_opaque_pass_3d)
                    .before(main_transparent_pass_3d),
            );
    }
}

#[derive(Resource, Default)]
struct ExtractedSurfaces {
    draws: Vec<(Arc<SurfaceDraw>, Mat4)>,
    exposure: f32,
}

fn extract_surfaces(
    mut out: ResMut<ExtractedSurfaces>,
    q: Extract<Query<(&ValveSurface, &GlobalTransform, &InheritedVisibility)>>,
    exposure: Extract<Option<Res<ValveExposure>>>,
) {
    out.draws.clear();
    for (s, t, v) in &q {
        if v.get() {
            out.draws.push((s.0.clone(), t.to_matrix()));
        }
    }
    out.exposure = exposure.as_ref().map(|e| e.0).unwrap_or(crate::plugin::DEFAULT_LIGHT_SCALE);
}

const MAX_2D: u32 = 2048;
const MAX_CUBE: u32 = 64;
const SAMPLERS: u32 = 8;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct PipelineKey {
    pass: usize,
    blend: u8,
    double_sided: bool,
    decal: bool,
    format: TextureFormat,
    samples: u32,
}

struct GpuMesh {
    vertex: Buffer,
    index: Buffer,
    count: u32,
}

struct PreparedDraw {
    pipeline: CachedRenderPipelineId,
    mesh: usize,
    base: u32,
}

#[derive(Default)]
struct ViewDraws {
    opaque: Vec<PreparedDraw>,
    translucent: Vec<PreparedDraw>,
}

#[derive(Resource, Default)]
struct PreparedSurfaces {
    constants_layout: Option<BindGroupLayoutDescriptor>,
    textures_layout: Option<BindGroupLayoutDescriptor>,
    pipelines: HashMap<PipelineKey, CachedRenderPipelineId>,
    meshes: Vec<GpuMesh>,
    mesh_index: HashMap<(usize, usize), usize>,
    constants: Option<Buffer>,
    constants_bind: Option<BindGroup>,
    texture_bind: Option<BindGroup>,
    texture_set: Vec<AssetId<Image>>,
    samplers: Vec<Sampler>,
    fallback_2d: Option<(Texture, TextureView)>,
    fallback_cube: Option<(Texture, TextureView)>,
    views: HashMap<Entity, ViewDraws>,
}

fn layouts() -> (BindGroupLayoutDescriptor, BindGroupLayoutDescriptor) {
    let constants = BindGroupLayoutDescriptor::new(
        "valve_constants",
        &[BindGroupLayoutEntry {
            binding: 0,
            visibility: ShaderStages::VERTEX_FRAGMENT,
            ty: BindingType::Buffer {
                ty: BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    );
    let textures = BindGroupLayoutDescriptor::new(
        "valve_textures",
        &[
            BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::VERTEX_FRAGMENT,
                ty: BindingType::Texture {
                    sample_type: TextureSampleType::Float { filterable: true },
                    view_dimension: TextureViewDimension::D2,
                    multisampled: false,
                },
                count: NonZeroU32::new(MAX_2D),
            },
            BindGroupLayoutEntry {
                binding: 1,
                visibility: ShaderStages::VERTEX_FRAGMENT,
                ty: BindingType::Texture {
                    sample_type: TextureSampleType::Float { filterable: true },
                    view_dimension: TextureViewDimension::Cube,
                    multisampled: false,
                },
                count: NonZeroU32::new(MAX_CUBE),
            },
            BindGroupLayoutEntry {
                binding: 3,
                visibility: ShaderStages::VERTEX_FRAGMENT,
                ty: BindingType::Sampler(SamplerBindingType::Filtering),
                count: NonZeroU32::new(SAMPLERS),
            },
        ],
    );
    (constants, textures)
}

fn blend_code(b: BlendKind) -> u8 {
    match b {
        BlendKind::Opaque => 0,
        BlendKind::Mask => 1,
        BlendKind::Blend => 2,
        BlendKind::Additive => 3,
    }
}

fn pipeline_descriptor(
    draw: &SurfaceDraw,
    key: PipelineKey,
    constants: &BindGroupLayoutDescriptor,
    textures: &BindGroupLayoutDescriptor,
) -> RenderPipelineDescriptor {
    let pass = &draw.pass;
    let attributes: Vec<VertexAttribute> = pass
        .pass
        .vertex_slots
        .iter()
        .filter_map(|&slot| {
            Some(VertexAttribute {
                format: if matches!(slot.decl_type(), d3d9_decl::DeclType::D3dColor) {
                    VertexFormat::Unorm8x4
                } else {
                    VertexFormat::Float32x4
                },
                offset: FatVertex::offset(slot)?,
                shader_location: slot.location(),
            })
        })
        .collect();
    let blend = match draw.blend {
        BlendKind::Opaque | BlendKind::Mask => None,
        BlendKind::Blend => Some(BlendState::ALPHA_BLENDING),
        BlendKind::Additive => Some(BlendState {
            color: BlendComponent {
                src_factor: BlendFactor::SrcAlpha,
                dst_factor: BlendFactor::One,
                operation: BlendOperation::Add,
            },
            alpha: BlendComponent::OVER,
        }),
    };
    let translucent = matches!(draw.blend, BlendKind::Blend | BlendKind::Additive);
    RenderPipelineDescriptor {
        label: Some(format!("valve {}", draw.material).into()),
        layout: vec![constants.clone(), textures.clone()],
        immediate_size: 0,
        vertex: VertexState {
            shader: pass.shader.clone(),
            shader_defs: vec![],
            entry_point: Some(d3d9_sm3::PASS_VERTEX_ENTRY.into()),
            buffers: vec![VertexBufferLayout {
                array_stride: FatVertex::STRIDE,
                step_mode: VertexStepMode::Vertex,
                attributes,
            }],
        },
        primitive: PrimitiveState {
            // our triangles are counter-clockwise seen from the front in a
            // right-handed world, like Bevy's
            front_face: FrontFace::Ccw,
            cull_mode: if key.double_sided { None } else { Some(Face::Back) },
            ..default()
        },
        depth_stencil: Some(DepthStencilState {
            format: CORE_3D_DEPTH_FORMAT,
            depth_write_enabled: Some(!translucent),
            depth_compare: Some(CompareFunction::GreaterEqual),
            stencil: default(),
            bias: if key.decal {
                DepthBiasState {
                    constant: 4,
                    slope_scale: 1.0,
                    clamp: 0.0,
                }
            } else {
                default()
            },
        }),
        multisample: MultisampleState {
            count: key.samples,
            ..default()
        },
        fragment: Some(FragmentState {
            shader: pass.shader.clone(),
            shader_defs: vec![],
            entry_point: Some(pass.fragment_entry.clone().into()),
            targets: vec![Some(ColorTargetState {
                format: key.format,
                blend,
                write_mask: ColorWrites::ALL,
            })],
        }),
        zero_initialize_workgroup_memory: true,
    }
}

fn make_fallbacks(device: &RenderDevice, queue: &RenderQueue) -> ((Texture, TextureView), (Texture, TextureView)) {
    let tex2d = device.create_texture(&TextureDescriptor {
        label: Some("valve fallback 2d"),
        size: Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        tex2d.as_image_copy(),
        &[255, 255, 255, 255],
        TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4), rows_per_image: None },
        Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
    );
    let view2d = tex2d.create_view(&TextureViewDescriptor::default());
    let cube = device.create_texture(&TextureDescriptor {
        label: Some("valve fallback cube"),
        size: Extent3d { width: 1, height: 1, depth_or_array_layers: 6 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        cube.as_image_copy(),
        &[0u8; 24],
        TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4), rows_per_image: Some(1) },
        Extent3d { width: 1, height: 1, depth_or_array_layers: 6 },
    );
    let cube_view = cube.create_view(&TextureViewDescriptor {
        dimension: Some(TextureViewDimension::Cube),
        ..default()
    });
    ((tex2d, view2d), (cube, cube_view))
}

#[allow(clippy::too_many_arguments)]
fn prepare_surfaces(
    extracted: Res<ExtractedSurfaces>,
    mut prepared: ResMut<PreparedSurfaces>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    pipeline_cache: Res<PipelineCache>,
    images: Res<RenderAssets<GpuImage>>,
    views: Query<(Entity, &ExtractedView, &ViewTarget, &Msaa)>,
) {
    let prepared = &mut *prepared;
    prepared.views.clear();
    if extracted.draws.is_empty() {
        return;
    }
    if prepared.constants_layout.is_none() {
        let (c, t) = layouts();
        prepared.constants_layout = Some(c);
        prepared.textures_layout = Some(t);
        prepared.samplers = (0..SAMPLERS as u16)
            .map(|i| {
                let k = SamplerKey::from_index(i);
                let addr = |clamp: bool| if clamp { AddressMode::ClampToEdge } else { AddressMode::Repeat };
                let filter = if k.point { FilterMode::Nearest } else { FilterMode::Linear };
                device.create_sampler(&SamplerDescriptor {
                    label: Some("valve sampler"),
                    address_mode_u: addr(k.clamp_s),
                    address_mode_v: addr(k.clamp_t),
                    address_mode_w: addr(k.clamp_t),
                    mag_filter: filter,
                    min_filter: filter,
                    mipmap_filter: if k.point { MipmapFilterMode::Nearest } else { MipmapFilterMode::Linear },
                    anisotropy_clamp: if k.point { 1 } else { 16 },
                    ..default()
                })
            })
            .collect();
        let (a, b) = make_fallbacks(&device, &queue);
        prepared.fallback_2d = Some(a);
        prepared.fallback_cube = Some(b);
    }
    let constants_layout = prepared.constants_layout.clone().unwrap();
    let textures_layout = prepared.textures_layout.clone().unwrap();

    // ---- texture table ----
    let mut table2d: Vec<AssetId<Image>> = Vec::new();
    let mut tablecube: Vec<AssetId<Image>> = Vec::new();
    let mut slot_of: HashMap<AssetId<Image>, u16> = HashMap::new();
    for (d, _) in &extracted.draws {
        for t in &d.textures {
            let id = t.image.id();
            if slot_of.contains_key(&id) || images.get(id).is_none() {
                continue;
            }
            let table = if t.cube { &mut tablecube } else { &mut table2d };
            let cap = if t.cube { MAX_CUBE } else { MAX_2D } as usize;
            if table.len() < cap {
                slot_of.insert(id, table.len() as u16);
                table.push(id);
            }
        }
    }
    let set: Vec<AssetId<Image>> = table2d.iter().chain(tablecube.iter()).copied().collect();
    if prepared.texture_bind.is_none() || prepared.texture_set != set {
        let fb2d = &prepared.fallback_2d.as_ref().unwrap().1;
        let fbcube = &prepared.fallback_cube.as_ref().unwrap().1;
        let mut views2d: Vec<&WgpuTextureView> = table2d.iter().map(|id| &*images.get(*id).unwrap().texture_view).collect();
        views2d.resize(MAX_2D as usize, &**fb2d);
        let mut viewscube: Vec<&WgpuTextureView> = tablecube.iter().map(|id| &*images.get(*id).unwrap().texture_view).collect();
        viewscube.resize(MAX_CUBE as usize, &**fbcube);
        let samplers: Vec<&WgpuSampler> = prepared.samplers.iter().map(|s| &**s).collect();
        let layout = pipeline_cache.get_bind_group_layout(&textures_layout);
        let bind = device.create_bind_group(
            "valve textures",
            &layout,
            &[
                BindGroupEntry { binding: 0, resource: BindingResource::TextureViewArray(&views2d) },
                BindGroupEntry { binding: 1, resource: BindingResource::TextureViewArray(&viewscube) },
                BindGroupEntry { binding: 3, resource: BindingResource::SamplerArray(&samplers) },
            ],
        );
        prepared.texture_bind = Some(bind);
        prepared.texture_set = set;
    }

    // ---- meshes ----
    for (d, _) in &extracted.draws {
        let key = (Arc::as_ptr(&d.vertices) as usize, Arc::as_ptr(&d.indices) as usize);
        if prepared.mesh_index.contains_key(&key) {
            continue;
        }
        let vertex = device.create_buffer_with_data(&BufferInitDescriptor {
            label: Some("valve vertices"),
            contents: bytemuck::cast_slice(&d.vertices),
            usage: BufferUsages::VERTEX,
        });
        let index = device.create_buffer_with_data(&BufferInitDescriptor {
            label: Some("valve indices"),
            contents: bytemuck::cast_slice(&d.indices),
            usage: BufferUsages::INDEX,
        });
        prepared.mesh_index.insert(key, prepared.meshes.len());
        prepared.meshes.push(GpuMesh {
            vertex,
            index,
            count: d.indices.len() as u32,
        });
    }

    // ---- per view constants ----
    let s2w = crate::valve::source_to_world_matrix();
    let w2s = s2w.inverse();
    let mut arena: Vec<[f32; 4]> = Vec::new();
    for (view_entity, view, target, msaa) in &views {
        let clip_from_world = view
            .clip_from_world
            .unwrap_or_else(|| view.clip_from_view * view.world_from_view.to_matrix().inverse());
        let view_proj = clip_from_world * s2w; // Source world -> clip
        let eye = w2s.transform_point3(view.world_from_view.translation());
        let mut vd = ViewDraws::default();
        let mut translucent: Vec<(f32, PreparedDraw)> = Vec::new();
        for (d, model) in &extracted.draws {
            let pass = &d.pass.pass;
            let key = PipelineKey {
                pass: d.pass.id,
                blend: blend_code(d.blend),
                double_sided: d.double_sided,
                decal: d.decal,
                format: target.main_texture_format(),
                samples: msaa.samples(),
            };
            let pipeline = *prepared
                .pipelines
                .entry(key)
                .or_insert_with(|| pipeline_cache.queue_render_pipeline(pipeline_descriptor(d, key, &constants_layout, &textures_layout)));
            let base = arena.len() as u32;
            let vc = pass.vertex_constants;
            let pc = pass.pixel_constants;
            let slot_rows = pass.samplers.len().div_ceil(4);
            arena.resize(arena.len() + vc + pc + slot_rows, [0.0; 4]);
            let b = base as usize;
            let setv = |r: usize, v: [f32; 4], arena: &mut Vec<[f32; 4]>| {
                if r < vc {
                    arena[b + r] = v;
                }
            };
            for &(r, v) in &d.vs_consts {
                setv(r as usize, v, &mut arena);
            }
            // cModel: Source local -> Source world, as three rows
            let model_src = w2s * *model * s2w;
            let rows = model_src.transpose();
            setv(58, rows.x_axis.to_array(), &mut arena);
            setv(59, rows.y_axis.to_array(), &mut arena);
            setv(60, rows.z_axis.to_array(), &mut arena);
            let vp = view_proj.transpose();
            setv(8, vp.x_axis.to_array(), &mut arena);
            setv(9, vp.y_axis.to_array(), &mut arena);
            setv(10, vp.z_axis.to_array(), &mut arena);
            setv(11, vp.w_axis.to_array(), &mut arena);
            setv(2, [eye.x, eye.y, eye.z, 0.0], &mut arena);
            let fwd = w2s.transform_vector3(view.world_from_view.forward().as_vec3()).normalize_or_zero();
            setv(12, [fwd.x, fwd.y, fwd.z, 0.0], &mut arena);
            setv(16, d.fog, &mut arena);
            let pbase = b + vc;
            let setp = |r: usize, v: [f32; 4], arena: &mut Vec<[f32; 4]>| {
                if r < pc {
                    arena[pbase + r] = v;
                }
            };
            for &(r, v) in &d.ps_consts {
                setp(r as usize, v, &mut arena);
            }
            let e = extracted.exposure;
            if d.pass.key.ps.starts_with("vertexlit") {
                setp(20, [eye.x, eye.y, eye.z, d.eye_w], &mut arena);
            } else {
                setp(10, [eye.x, eye.y, eye.z, 0.0], &mut arena);
            }
            setp(29, [d.fog_color[0] * e, d.fog_color[1] * e, d.fog_color[2] * e, 1.0], &mut arena);
            setp(30, [e, 1.0, 1.0, e], &mut arena);
            // texture slot words, in the pass's sampler order
            let sbase = pbase + pc;
            for (i, (reg, dim)) in pass.samplers.iter().enumerate() {
                let bound = d.textures.iter().find(|t| t.register == *reg);
                let (tex, samp) = match bound {
                    Some(t) => (
                        slot_of.get(&t.image.id()).copied().unwrap_or(0),
                        t.sampler.index(),
                    ),
                    None => (0, 0),
                };
                let _ = dim;
                let word = d3d9_sm3::texture_slot_word(tex, samp);
                arena[sbase + i / 4][i % 4] = f32::from_bits(word);
            }
            let key_mesh = (Arc::as_ptr(&d.vertices) as usize, Arc::as_ptr(&d.indices) as usize);
            let pd = PreparedDraw {
                pipeline,
                mesh: prepared.mesh_index[&key_mesh],
                base,
            };
            if matches!(d.blend, BlendKind::Blend | BlendKind::Additive) {
                let c = model.transform_point3(Vec3::ZERO);
                let dist = c.distance_squared(view.world_from_view.translation());
                translucent.push((dist, pd));
            } else {
                vd.opaque.push(pd);
            }
        }
        translucent.sort_by(|a, b| b.0.total_cmp(&a.0));
        vd.translucent = translucent.into_iter().map(|t| t.1).collect();
        vd.opaque.sort_by_key(|d| d.pipeline.id());
        prepared.views.insert(view_entity, vd);
    }
    if arena.is_empty() {
        return;
    }
    let bytes: &[u8] = bytemuck::cast_slice(&arena);
    let needed = bytes.len() as u64;
    if prepared.constants.as_ref().is_none_or(|b| b.size() < needed) {
        let size = needed.next_power_of_two().max(1 << 16);
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some("valve constants"),
            size,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = pipeline_cache.get_bind_group_layout(&constants_layout);
        prepared.constants_bind = Some(device.create_bind_group(
            "valve constants",
            &layout,
            &[BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }],
        ));
        prepared.constants = Some(buffer);
    }
    queue.write_buffer(prepared.constants.as_ref().unwrap(), 0, bytes);
}

fn valve_main_pass(
    view: ViewQuery<(&ExtractedCamera, &ViewTarget, &ViewDepthTexture)>,
    prepared: Res<PreparedSurfaces>,
    pipeline_cache: Res<PipelineCache>,
    mut ctx: RenderContext,
) {
    let view_entity = view.entity();
    let (camera, target, depth) = view.into_inner();
    let Some(draws) = prepared.views.get(&view_entity) else { return };
    let (Some(cb), Some(tb)) = (&prepared.constants_bind, &prepared.texture_bind) else { return };
    if draws.opaque.is_empty() && draws.translucent.is_empty() {
        return;
    }
    let color_attachments = [Some(target.get_color_attachment())];
    let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
        label: Some("valve_surfaces"),
        color_attachments: &color_attachments,
        depth_stencil_attachment: Some(depth.get_attachment(StoreOp::Store)),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    if let Some(viewport) = camera.viewport.as_ref() {
        pass.set_camera_viewport(viewport);
    }
    pass.set_bind_group(0, cb, &[]);
    pass.set_bind_group(1, tb, &[]);
    for d in draws.opaque.iter().chain(draws.translucent.iter()) {
        let Some(pipeline) = pipeline_cache.get_render_pipeline(d.pipeline) else { continue };
        let mesh = &prepared.meshes[d.mesh];
        pass.set_render_pipeline(pipeline);
        pass.set_vertex_buffer(0, mesh.vertex.slice(..));
        pass.set_index_buffer(mesh.index.slice(..), IndexFormat::Uint32);
        pass.draw_indexed(0..mesh.count, 0, d.base..d.base + 1);
    }
}


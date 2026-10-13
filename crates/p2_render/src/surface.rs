//! The portal surface: the material (a port of PortalRefract), the quad on
//! the wall, and the near-plane cap that stands in for it when the eye is
//! close.
//!
//! Each portal has one quad and one cap per node level 0..=2 (7 node codes
//! in total: root, the two level-1 views and the four level-2 views). The quad
//! for node `N` sits on layer `node_layer(N)`, so only the camera of `N`
//! draws it. It shows the image of `N`'s child through this portal.

use bevy::asset::embedded_asset;
use bevy::camera::visibility::RenderLayers;
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::math::Vec4Swizzles;
use bevy::prelude::*;
use bevy::asset::RenderAssetUsages;
use bevy::render::render_resource::{
    AsBindGroup, ColorWrites, CompareFunction, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
};
use bevy::shader::ShaderRef;
use bevy::mesh::MeshVertexBufferLayoutRef;

use p2_assets::portal_textures::PortalGameAssets;
use p2_core::{Portal, PortalKind, PORTAL_HALF_SIZE};

use crate::views::{child_code, NodeImages, PortalInfos, ViewTree};
use crate::{node_layer, PortalRenderConfig};

/// Most surface nodes any config can need (recursion depth 3 plus the
/// doubler level: codes 1..=31).
pub const MAX_SURFACE_NODES: usize = 31;

pub const FLAG_HAS_VIEW: u32 = 1;
pub const FLAG_CAP: u32 = 2;
pub const FLAG_WRITEZ: u32 = 4;
/// The view texture is last frame's image re-mapped by `uv_remap`.
pub const FLAG_DOUBLER: u32 = 8;

/// Shader inputs. `portal_active` is `1 - $PortalStatic`.
#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct PortalSurfaceParams {
    pub open_amount: f32,
    pub portal_active: f32,
    pub color_scale: f32,
    pub flags: u32,
    /// Depth doubler: view UV = screen UV * xy + zw (identity otherwise).
    pub uv_remap: Vec4,
}

/// Pipeline state that differs between the three uses of the material.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SurfaceKey {
    /// Depth only (unlinked and opening).
    pub writez: bool,
    /// Depth test always passes (the near-plane cap).
    pub cap: bool,
}

/// The portal surface material. One asset per (node, portal, use).
#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
#[bind_group_data(SurfaceKey)]
pub struct PortalSurfaceMaterial {
    #[uniform(0)]
    pub params: PortalSurfaceParams,
    /// The view through the portal, rendered by the child node's camera.
    #[texture(1)]
    #[sampler(2)]
    pub view: Option<Handle<Image>>,
    /// `$PortalMaskTexture` (noise-blur-256x256).
    #[texture(3)]
    #[sampler(4)]
    pub noise: Option<Handle<Image>>,
    /// `$PortalColorTexture` (1-D colour ramp).
    #[texture(5)]
    #[sampler(6)]
    pub ramp: Option<Handle<Image>>,
    pub writez: bool,
    pub cap: bool,
}

impl From<&PortalSurfaceMaterial> for SurfaceKey {
    fn from(m: &PortalSurfaceMaterial) -> Self {
        SurfaceKey { writez: m.writez, cap: m.cap }
    }
}

impl Material for PortalSurfaceMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://p2_render/portal_surface.wgsl".into()
    }
    fn alpha_mode(&self) -> AlphaMode {
        // The live surface composites its rim over the scene itself (it reads
        // a copy of the opaque pass, like PortalRefract reads the frame
        // buffer), so it is drawn opaque; only the near-plane cap blends.
        if self.cap {
            AlphaMode::Blend
        } else {
            AlphaMode::Opaque
        }
    }
    fn reads_view_transmission_texture(&self) -> bool {
        !self.writez && !self.cap
    }
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
        if key.bind_group_data.writez {
            // Depth only: no colour, depth written.
            if let Some(ds) = descriptor.depth_stencil.as_mut() {
                ds.depth_write_enabled = Some(true);
            }
            if let Some(frag) = descriptor.fragment.as_mut() {
                for target in frag.targets.iter_mut().flatten() {
                    target.blend = None;
                    target.write_mask = ColorWrites::empty();
                }
            }
        }
        if key.bind_group_data.cap {
            // Drawn over the wall: depth neither tested nor written.
            if let Some(ds) = descriptor.depth_stencil.as_mut() {
                ds.depth_write_enabled = Some(false);
                ds.depth_compare = Some(CompareFunction::Always);
            }
        }
        Ok(())
    }
}

/// Meshes shared by every portal of a given size.
#[derive(Resource, Default)]
pub(crate) struct SurfaceMeshes {
    quad: Option<Handle<Mesh>>,
    cap: Option<Handle<Mesh>>,
}

/// The entities and materials of one portal's surfaces, indexed by node code - 1.
/// Only the first `PortalRenderConfig::surface_nodes()` entries are used.
#[derive(Component)]
pub(crate) struct PortalSurfaces {
    pub quads: [Entity; MAX_SURFACE_NODES],
    pub caps: [Entity; MAX_SURFACE_NODES],
    pub live: [Handle<PortalSurfaceMaterial>; MAX_SURFACE_NODES],
    pub writez: [Handle<PortalSurfaceMaterial>; MAX_SURFACE_NODES],
    pub cap_mat: [Handle<PortalSurfaceMaterial>; MAX_SURFACE_NODES],
}

/// Marks a surface quad (for queries).
#[derive(Component, Clone, Copy, Debug)]
pub struct PortalSurfaceQuad {
    pub node: u32,
    pub kind: PortalKind,
}

/// Marks a near-plane cap.
#[derive(Component, Clone, Copy, Debug)]
pub struct PortalSurfaceCap {
    pub node: u32,
    pub kind: PortalKind,
}

/// Frames left in which a new cap is drawn microscopically small, so its
/// pipeline is compiled before it is first needed. Bevy compiles pipelines
/// lazily and skips the draw until they are ready; the cap is needed for
/// only a frame or two as the eye reaches a portal, so without this the
/// first transit showed the void behind the wall for a frame.
#[derive(Component, Clone, Copy, Debug)]
pub struct CapWarmup(pub u32);

pub(crate) fn plugin(app: &mut App) {
    embedded_asset!(app, "portal_surface.wgsl");
    app.add_plugins(MaterialPlugin::<PortalSurfaceMaterial>::default())
        .init_resource::<SurfaceMeshes>()
        .add_observer(cleanup_surfaces);
}

fn quad_mesh(half: Vec2) -> Mesh {
    Rectangle::new(half.x * 2.0, half.y * 2.0).mesh().build()
}

/// An open box behind the portal plane (inside faces only, toward the
/// viewer standing in front). It covers the opening exactly, so a camera
/// closer than the near plane still sees the view where the quad was clipped.
fn cap_mesh(half: Vec2, depth: f32) -> Mesh {
    let (x, y, z) = (half.x, half.y, -depth);
    let quads: [[Vec3; 4]; 5] = [
        [Vec3::new(-x, -y, z), Vec3::new(x, -y, z), Vec3::new(x, y, z), Vec3::new(-x, y, z)],
        [Vec3::new(-x, -y, 0.0), Vec3::new(-x, -y, z), Vec3::new(-x, y, z), Vec3::new(-x, y, 0.0)],
        [Vec3::new(x, -y, z), Vec3::new(x, -y, 0.0), Vec3::new(x, y, 0.0), Vec3::new(x, y, z)],
        [Vec3::new(-x, -y, 0.0), Vec3::new(x, -y, 0.0), Vec3::new(x, -y, z), Vec3::new(-x, -y, z)],
        [Vec3::new(-x, y, z), Vec3::new(x, y, z), Vec3::new(x, y, 0.0), Vec3::new(-x, y, 0.0)],
    ];
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    let mut indices = Vec::new();
    for q in quads {
        let n = (q[1] - q[0]).cross(q[2] - q[0]).normalize();
        let base = positions.len() as u32;
        for p in q {
            positions.push(p.to_array());
            normals.push(n.to_array());
            uvs.push([0.5, 0.5]);
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        .with_inserted_indices(Indices::U32(indices))
}

/// Gives every newly placed portal its surfaces.
pub(crate) fn ensure_surfaces(
    mut commands: Commands,
    new: Query<Entity, (With<Portal>, Without<PortalSurfaces>)>,
    mut materials: ResMut<Assets<PortalSurfaceMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut cache: ResMut<SurfaceMeshes>,
    config: Res<PortalRenderConfig>,
    game: Option<Res<PortalGameAssets>>,
    portals: Query<&Portal>,
) {
    // Without Portal 2 (no `p2_assets::AssetsPlugin` textures) nothing is drawn.
    let Some(game) = game else { return };
    if new.is_empty() {
        return;
    }
    for entity in &new {
        let Ok(portal) = portals.get(entity) else { continue };
        let kind = portal.kind;
        let n = config.doubler_nodes();
        let quad_mesh_h = cache
            .quad
            .get_or_insert_with(|| meshes.add(quad_mesh(PORTAL_HALF_SIZE)))
            .clone();
        let cap_mesh_h = cache
            .cap
            .get_or_insert_with(|| meshes.add(cap_mesh(PORTAL_HALF_SIZE, config.cap_depth)))
            .clone();
        let (noise, ramp, color_scale) = (Some(game.noise.clone()), Some(game.ramp(kind)), game.color_scale);
        let base = PortalSurfaceParams {
            open_amount: 0.0,
            portal_active: 0.0,
            color_scale,
            flags: 0,
            uv_remap: Vec4::new(1.0, 1.0, 0.0, 0.0),
        };
        let mk = |params: PortalSurfaceParams, writez: bool, cap: bool| PortalSurfaceMaterial {
            params,
            view: None,
            noise: noise.clone(),
            ramp: ramp.clone(),
            writez,
            cap,
        };

        let mut quads = [Entity::PLACEHOLDER; MAX_SURFACE_NODES];
        let mut caps = [Entity::PLACEHOLDER; MAX_SURFACE_NODES];
        let mut live: [Handle<PortalSurfaceMaterial>; MAX_SURFACE_NODES] = Default::default();
        let mut writez: [Handle<PortalSurfaceMaterial>; MAX_SURFACE_NODES] = Default::default();
        let mut cap_mat: [Handle<PortalSurfaceMaterial>; MAX_SURFACE_NODES] = Default::default();
        for i in 0..n {
            let node = i as u32 + 1;
            live[i] = materials.add(mk(base, false, false));
            writez[i] = materials.add(mk(PortalSurfaceParams { flags: FLAG_WRITEZ, ..base }, true, false));
            cap_mat[i] = materials.add(mk(PortalSurfaceParams { flags: FLAG_CAP, ..base }, false, true));
            quads[i] = commands
                .spawn((
                    Name::new(format!("Portal surface node {node}")),
                    PortalSurfaceQuad { node, kind },
                    Mesh3d(quad_mesh_h.clone()),
                    MeshMaterial3d(live[i].clone()),
                    // Just in front of the wall, so it never z-fights with it.
                    Transform::from_xyz(0.0, 0.0, config.quad_standoff),
                    RenderLayers::layer(node_layer(node)),
                    NotShadowCaster,
                    NotShadowReceiver,
                    ChildOf(entity),
                ))
                .id();
            caps[i] = commands
                .spawn((
                    Name::new(format!("Portal near-plane cap node {node}")),
                    PortalSurfaceCap { node, kind },
                    CapWarmup(30),
                    Mesh3d(cap_mesh_h.clone()),
                    MeshMaterial3d(cap_mat[i].clone()),
                    Transform::from_scale(Vec3::splat(1e-4)),
                    Visibility::Inherited,
                    RenderLayers::layer(node_layer(node)),
                    NotShadowCaster,
                    NotShadowReceiver,
                    ChildOf(entity),
                ))
                .id();
        }
        commands.entity(entity).insert(PortalSurfaces { quads, caps, live, writez, cap_mat });
    }
}

/// Frees the materials of a portal that was despawned.
fn cleanup_surfaces(
    remove: On<Remove, PortalSurfaces>,
    surfaces: Query<&PortalSurfaces>,
    mut materials: ResMut<Assets<PortalSurfaceMaterial>>,
) {
    let Ok(s) = surfaces.get(remove.entity) else { return };
    for h in s.live.iter().chain(&s.writez).chain(&s.cap_mat) {
        materials.remove(h.id());
    }
}

/// Pushes this frame's view images, open and static amounts, and the cap
/// visibility into every surface.
#[allow(clippy::too_many_arguments)]
pub(crate) fn sync_surfaces(
    infos: Res<PortalInfos>,
    tree: Res<ViewTree>,
    node_images: Res<NodeImages>,
    game: Option<Res<PortalGameAssets>>,
    main_near: Res<crate::views::MainView>,
    config: Res<PortalRenderConfig>,
    surfaces: Query<&PortalSurfaces>,
    mut quad_mats: Query<&mut MeshMaterial3d<PortalSurfaceMaterial>, With<PortalSurfaceQuad>>,
    mut cap_vis: Query<(&mut Visibility, &mut Transform, &mut CapWarmup), With<PortalSurfaceCap>>,
    mut materials: ResMut<Assets<PortalSurfaceMaterial>>,
) {
    let _ = game;
    let near_limit = main_near.near + config.cap_margin;
    let n = config.doubler_nodes();
    let plain_level = config.recursion_depth + 1;
    for kind in [PortalKind::Blue, PortalKind::Orange] {
        let Some(info) = infos.get(kind) else { continue };
        let Ok(s) = surfaces.get(info.entity) else { continue };
        let linked = info.partner.is_some();
        let open = info.open;
        let active_static = 1.0 - info.static_amount;
        // Unlinked and opening: depth only, inside the hole (PortalRefract's anims material).
        let writez_now = !linked && open > 0.0 && open < 1.0;

        for i in 0..n {
            let node = i as u32 + 1;
            let view_node = tree.nodes.get(&node);
            let child = child_code_of(node, kind);
            // The view through this portal from this node, if the child camera is live.
            let mut uv_remap = Vec4::new(1.0, 1.0, 0.0, 0.0);
            let mut doubler = false;
            let view = if crate::views::node_level(node) == plain_level {
                // Depth doubler: this node has no child camera. Show last
                // frame's image of this node, scaled from where this node's own
                // opening sits on screen (src) into this portal's rectangle (dst).
                match (linked, view_node) {
                    (true, Some(vn)) => {
                        let dst = crate::views::screen_rect(crate::views::node_clip_from_world(vn), &info.frame, info.half);
                        if let (Some(src), Some(dst)) = (vn.via_rect, dst) {
                            let scale = (src.zw() - src.xy()) / (dst.zw() - dst.xy());
                            let offset = src.xy() - dst.xy() * scale;
                            uv_remap = Vec4::new(scale.x, scale.y, offset.x, offset.y);
                            doubler = true;
                        }
                        node_images.prev.get(&node).cloned()
                    }
                    _ => None,
                }
            } else {
                match (linked, tree.nodes.get(&child)) {
                    (true, Some(_)) => node_images.images.get(&child).cloned(),
                    _ => None,
                }
            };
            let has_view = view.is_some();
            let flags = if has_view { FLAG_HAS_VIEW } else { 0 } | if doubler { FLAG_DOUBLER } else { 0 };
            let want_params = PortalSurfaceParams {
                open_amount: open,
                portal_active: active_static,
                color_scale: game.as_ref().map_or(4.0, |g| g.color_scale),
                flags,
                uv_remap,
            };
            set_material(&mut materials, &s.live[i], want_params, view.clone());
            set_material(
                &mut materials,
                &s.writez[i],
                PortalSurfaceParams { flags: flags | FLAG_WRITEZ, ..want_params },
                view.clone(),
            );
            set_material(
                &mut materials,
                &s.cap_mat[i],
                PortalSurfaceParams { flags: flags | FLAG_CAP, ..want_params },
                view.clone(),
            );

            // The quad always draws the visible surface. Portal 2's depth-only
            // pass for an unlinked, opening portal is drawn IN ADDITION to the
            // growing flame ring, never instead of it (a lone portal is visible
            // while it opens).
            let _ = writez_now;
            let want = &s.live[i];
            if let Ok(mut m) = quad_mats.get_mut(s.quads[i]) {
                if m.0.id() != want.id() {
                    m.0 = want.clone();
                }
            }

            // The cap is shown only when this node's camera is just in front
            // of the opening (within the near plane) and the view exists.
            let cap_on = match view_node {
                Some(n) if has_view => {
                    let d = info.frame.plane_distance(Vec3::from(n.pose.translation));
                    d > 0.0 && d < near_limit
                }
                _ => false,
            };
            if let Ok((mut v, mut t, mut warm)) = cap_vis.get_mut(s.caps[i]) {
                if warm.0 > 0 {
                    warm.0 -= 1;
                    if warm.0 == 0 {
                        t.scale = Vec3::ONE;
                    } else {
                        continue;
                    }
                }
                let want_v = if cap_on { Visibility::Inherited } else { Visibility::Hidden };
                if *v != want_v {
                    *v = want_v;
                }
            }
        }
    }
}

fn child_code_of(node: u32, kind: PortalKind) -> u32 {
    child_code(node, kind)
}

fn set_material(
    materials: &mut Assets<PortalSurfaceMaterial>,
    handle: &Handle<PortalSurfaceMaterial>,
    params: PortalSurfaceParams,
    view: Option<Handle<Image>>,
) {
    let Some(mut m) = materials.get_mut(handle) else { return };
    let view_same = match (&m.view, &view) {
        (Some(a), Some(b)) => a.id() == b.id(),
        (None, None) => true,
        _ => false,
    };
    let params_same = m.params.open_amount == params.open_amount
        && m.params.portal_active == params.portal_active
        && m.params.color_scale == params.color_scale
        && m.params.flags == params.flags;
    if !view_same {
        m.view = view;
    }
    if !params_same {
        m.params = params;
    }
}

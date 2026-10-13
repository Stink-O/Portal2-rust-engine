//! Bevy side: the plugin, spawning a converted map at any world offset, and
//! the components other crates query.

use crate::collision::{ColliderData, SideInfo};
use crate::config::GameConfig;
use crate::entities::{source_angles_to_world, source_view_to_world, SourceEntity};
use crate::fs::GameFs;
use crate::geometry::MeshData;
use crate::map::{self, LoadedMap};
use crate::material::{self, SourceMaterial, SourceMaterialParams};
use crate::models::ModelData;
use crate::surfaceprops::SurfaceProps;
use crate::vmt::{BlendKind, MaterialInfo};
use avian3d::prelude::*;
use bevy::asset::RenderAssetUsages;
use bevy::ecs::system::SystemParam;
use bevy::image::{CompressedImageFormatSupport, CompressedImageFormats, ImageSampler, ImageSamplerDescriptor};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureViewDescriptor, TextureViewDimension};
use shared::{source_dir_to_world, source_to_world, Portalable};
use std::collections::HashMap;
use std::sync::Arc;

/// Mounts the game and registers the Source material.
///
/// If the game cannot be mounted the plugin logs an error and
/// [`spawn_map`] returns an error; the app keeps running.
#[derive(Default)]
pub struct SourceMapPlugin {
    /// Override the install; `None` = `PORTAL2_DIR` or the default path.
    pub config: Option<GameConfig>,
}

impl Plugin for SourceMapPlugin {
    fn build(&self, app: &mut App) {
        material::register(app);
        let config = self.config.clone().unwrap_or_else(GameConfig::from_env);
        let game = match GameFs::mount(config) {
            Ok(fs) => {
                let surfprops = SurfaceProps::load(&fs);
                info!("source_assets: mounted {}", fs.config.root.display());
                Some((Arc::new(fs), Arc::new(surfprops)))
            }
            Err(e) => {
                error!("source_assets: {e}");
                None
            }
        };
        app.insert_resource(SourceGame {
            fs: game.as_ref().map(|g| g.0.clone()),
            surfprops: game.map(|g| g.1).unwrap_or_default(),
        })
        .init_resource::<SourceCache>()
        .init_resource::<SourceRenderSettings>()
        .init_resource::<crate::valve::ValveShaderLibrary>()
        .add_plugins(crate::valve_render::ValveRenderPlugin);
    }
}

/// The mounted game filesystem.
#[derive(Resource, Clone)]
pub struct SourceGame {
    pub fs: Option<Arc<GameFs>>,
    pub surfprops: Arc<SurfaceProps>,
}

/// Default [`SourceRenderSettings::light_scale`]: Portal 2 maps set their
/// auto-exposure range to roughly 0.5..3 (`env_tonemap_controller`), so a
/// settled exposure of ~3 reproduces the in-game brightness.
pub const DEFAULT_LIGHT_SCALE: f32 = 3.0;

/// How baked lighting is turned into colour.
#[derive(Resource, Clone, Debug)]
pub struct SourceRenderSettings {
    /// Multiplier on lightmaps and prop vertex light.
    pub light_scale: f32,
    /// Ignore baked light (debugging textures and geometry).
    pub fullbright: bool,
    /// Draw surfaces with Portal 2's own shaders where translated
    /// (`SourceMaterial` stays the fallback).
    pub valve_shaders: bool,
}

impl Default for SourceRenderSettings {
    fn default() -> Self {
        SourceRenderSettings {
            light_scale: DEFAULT_LIGHT_SCALE,
            fullbright: false,
            valve_shaders: true,
        }
    }
}

/// Converted maps and uploaded images, shared between spawns.
#[derive(Resource, Default)]
pub struct SourceCache {
    pub maps: HashMap<String, Arc<LoadedMap>>,
    images: HashMap<String, Handle<Image>>,
    /// Linear (non-sRGB) uploads, for normal maps and masks.
    linear_images: HashMap<String, Handle<Image>>,
    fallback_images: HashMap<&'static str, Handle<Image>>,
    /// Per map: materials drawn by the fallback instead of Valve shaders, with why.
    pub fallback_materials: HashMap<String, Vec<(String, String)>>,
    prop_materials: HashMap<(String, bool), Handle<SourceMaterial>>,
    white: Option<Handle<Image>>,
}

/// Everything [`spawn_map`] needs, as one system parameter.
#[derive(SystemParam)]
pub struct SourceMapAssets<'w> {
    pub game: Res<'w, SourceGame>,
    pub cache: ResMut<'w, SourceCache>,
    pub settings: Res<'w, SourceRenderSettings>,
    pub meshes: ResMut<'w, Assets<Mesh>>,
    pub materials: ResMut<'w, Assets<SourceMaterial>>,
    pub images: ResMut<'w, Assets<Image>>,
    pub compressed: Option<Res<'w, CompressedImageFormatSupport>>,
    pub shaders: ResMut<'w, Assets<bevy::shader::Shader>>,
    pub valve: Res<'w, crate::valve::ValveShaderLibrary>,
}

// ---------------------------------------------------------------- components

/// Root of a spawned map. Children: geometry, colliders, props.
#[derive(Component, Clone, Debug)]
pub struct SourceMapRoot {
    pub name: String,
}

/// On every brush collider: what the portal gun needs to judge a hit.
#[derive(Component, Clone, Debug)]
pub struct SourceCollider {
    /// Brush contents flags (`CONTENTS_*`).
    pub contents: u32,
    pub brush: u32,
    /// 0 = world, N = brush entity `*N`.
    pub model: u32,
    /// Outward faces covered by this collider, with material and surfaceprop.
    pub sides: Vec<SideInfo>,
}

impl SourceCollider {
    /// The side whose outward normal best matches a hit normal (world space).
    pub fn side_for_normal(&self, normal: Vec3) -> Option<&SideInfo> {
        self.sides
            .iter()
            .max_by(|a, b| a.normal.dot(normal).total_cmp(&b.normal.dot(normal)))
    }
}

/// Collider the portal gun's shot passes through (grates, player clips).
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct PortalShotPassthrough;

/// A rendered entity from the map's entity lump or static prop lump.
#[derive(Component, Clone, Debug)]
pub struct SourceEntityRef {
    pub kind: MapEntityKind,
    /// Index into [`MapInfo::entities`].
    pub index: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MapEntityKind {
    PlayerStart,
    StaticProp,
    WeightedCube,
    DynamicProp,
    PhysicsProp,
    Door,
    Button,
    /// Any other brush entity with a model (`func_brush`, `func_door`...).
    BrushEntity,
    LightEnvironment,
    /// Laser emitters/catchers, turrets, cameras and other `prop_*` classes.
    OtherProp,
    Other,
}

/// One entity of interest, in world space (map offset applied).
#[derive(Clone, Debug)]
pub struct MapEntity {
    pub kind: MapEntityKind,
    pub classname: String,
    pub targetname: Option<String>,
    pub transform: Transform,
    /// `.mdl` path for studio models, `*N` brush model index as `Some` in `brush_model`.
    pub model: Option<String>,
    pub brush_model: Option<usize>,
    pub skin: i32,
    /// The spawned render entity, if any.
    pub entity: Option<Entity>,
    /// Model-space bounds in world axes (metres), when a model is known.
    pub bounds: Option<(Vec3, Vec3)>,
    pub keyvalues: Vec<(String, String)>,
}

/// `light_environment` (sun) in world space.
#[derive(Clone, Debug)]
pub struct LightEnvironment {
    /// Direction light travels, world space.
    pub direction: Vec3,
    /// Linear colour and brightness from `_light` (RGB 0..1, intensity).
    pub color: Vec3,
    pub brightness: f32,
    pub ambient_color: Vec3,
    pub ambient_brightness: f32,
}

/// `env_fog_controller` settings (the first enabled one), world units.
#[derive(Clone, Debug)]
pub struct MapFog {
    /// Linear colour.
    pub color: Color,
    /// Metres.
    pub start: f32,
    pub end: f32,
    pub max_density: f32,
}

impl MapFog {
    /// The same fog as a Bevy [`DistanceFog`] for a camera.
    pub fn distance_fog(&self) -> DistanceFog {
        DistanceFog {
            color: self.color.with_alpha(self.max_density),
            falloff: FogFalloff::Linear { start: self.start, end: self.end },
            ..default()
        }
    }
}

/// What [`spawn_map`] returns.
#[derive(Clone, Debug)]
pub struct MapInfo {
    pub name: String,
    pub root: Entity,
    pub offset: Vec3,
    /// `info_player_start` in world space: feet position, yaw/pitch rotation (-Z forward).
    /// In most Portal 2 maps this is a hidden room; the game immediately
    /// teleports the player into the arrival elevator (see `arrival`).
    pub player_start: Transform,
    /// Where the player actually begins: the `@arrival_teleport`
    /// point_teleport (inside the arrival elevator) when the map has one,
    /// else `player_start`.
    pub arrival: Transform,
    pub entities: Vec<MapEntity>,
    pub light_environment: Option<LightEnvironment>,
    pub fog: Option<MapFog>,
    /// Cubemap from the 2D skybox (`skyname`), for `bevy::core_pipeline::Skybox`.
    pub skybox: Option<Handle<Image>>,
    pub skyname: Option<String>,
    /// World-space bounds of the brush geometry.
    pub bounds: (Vec3, Vec3),
    /// Seconds spent converting (0 when the map came from the cache).
    pub convert_seconds: f32,
}

impl MapInfo {
    pub fn entities_of(&self, kind: MapEntityKind) -> impl Iterator<Item = &MapEntity> {
        self.entities.iter().filter(move |e| e.kind == kind)
    }
}

// ---------------------------------------------------------------- spawning

/// Convert (or reuse) a map without touching the world. Useful on a
/// background thread: `load_map` then [`spawn_loaded_map`].
pub fn load_map(game: &SourceGame, name: &str) -> anyhow::Result<LoadedMap> {
    let fs = game
        .fs
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Portal 2 is not mounted (set {})", crate::config::PORTAL2_DIR_ENV))?;
    map::load_map(fs, &game.surfprops, name)
}

/// Load `name` (e.g. `"sp_a1_intro3"`) and spawn it with its origin at `offset`.
pub fn spawn_map(
    commands: &mut Commands,
    assets: &mut SourceMapAssets,
    name: &str,
    offset: Vec3,
) -> anyhow::Result<MapInfo> {
    let key = name.to_ascii_lowercase();
    let (map, secs) = match assets.cache.maps.get(&key) {
        Some(m) => (m.clone(), 0.0),
        None => {
            let t = std::time::Instant::now();
            let m = Arc::new(load_map(&assets.game, name)?);
            let secs = t.elapsed().as_secs_f32();
            info!("source_assets: converted {name} in {secs:.2}s {:?}", m.timings);
            assets.cache.maps.insert(key, m.clone());
            (m, secs)
        }
    };
    let mut info = spawn_loaded_map(commands, assets, &map, offset);
    info.convert_seconds = secs;
    Ok(info)
}

fn white_image(assets: &mut SourceMapAssets) -> Handle<Image> {
    if let Some(h) = &assets.cache.white {
        return h.clone();
    }
    let img = Image::new(
        Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        TextureDimension::D2,
        vec![255, 255, 255, 255],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    let h = assets.images.add(img);
    assets.cache.white = Some(h.clone());
    h
}

fn bc_supported(assets: &SourceMapAssets) -> bool {
    assets
        .compressed
        .as_ref()
        .is_some_and(|c| c.0.contains(CompressedImageFormats::BC))
}

fn texture_handle(assets: &mut SourceMapAssets, map: &LoadedMap, name: &str) -> Option<Handle<Image>> {
    if let Some(h) = assets.cache.images.get(name) {
        return Some(h.clone());
    }
    let tex = map.textures.get(name)?.clone();
    let bc = bc_supported(assets);
    let h = assets.images.add(tex.into_image(bc, true));
    assets.cache.images.insert(name.to_string(), h.clone());
    Some(h)
}

/// Per-map inputs for model surfaces on Valve shaders.
struct ValveCtx {
    fog: [f32; 4],
    fog_color: [f32; 3],
    /// Scale from our decoded vertex light to what the engine's VS decodes.
    light_scale: f32,
    fallbacks: std::sync::Mutex<std::collections::BTreeMap<String, String>>,
}

/// A texture for the Valve path: (handle, sampler, is cube).
fn valve_texture(
    images: &mut Assets<Image>,
    cache: &mut SourceCache,
    bc: bool,
    map: &LoadedMap,
    name: &str,
    srgb: bool,
) -> Option<(Handle<Image>, crate::valve::SamplerKey, bool)> {
    let tex = map.textures.get(name)?;
    let sampler = crate::valve::SamplerKey::from_vtf_flags(tex.flags);
    let cube = tex.faces == 6;
    let table = if srgb { &mut cache.images } else { &mut cache.linear_images };
    if let Some(h) = table.get(name) {
        return Some((h.clone(), sampler, cube));
    }
    let h = images.add(tex.clone().into_image(bc, srgb));
    table.insert(name.to_string(), h.clone());
    Some((h, sampler, cube))
}

fn fallback_image(images: &mut Assets<Image>, cache: &mut SourceCache, name: &'static str, rgba: [u8; 4], srgb: bool) -> Handle<Image> {
    cache
        .fallback_images
        .entry(name)
        .or_insert_with(|| {
            images.add(Image::new(
                Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                TextureDimension::D2,
                rgba.to_vec(),
                if srgb { TextureFormat::Rgba8UnormSrgb } else { TextureFormat::Rgba8Unorm },
                RenderAssetUsages::RENDER_WORLD,
            ))
        })
        .clone()
}

fn material_params(m: &MaterialInfo, flags: u32, light_scale: f32) -> (SourceMaterialParams, AlphaMode) {
    let mut flags = flags;
    let mut color = Vec4::new(m.color[0], m.color[1], m.color[2], m.alpha);
    let alpha_mode = match m.blend {
        BlendKind::Opaque => AlphaMode::Opaque,
        BlendKind::Mask => {
            flags |= material::FLAG_ALPHA_TEST;
            AlphaMode::Mask(m.alpha_cutoff)
        }
        BlendKind::Blend => AlphaMode::Blend,
        BlendKind::Additive => AlphaMode::Add,
    };
    if m.unlit {
        flags |= material::FLAG_UNLIT;
    }
    if m.base_texture2.is_some() {
        flags |= material::FLAG_BLEND2;
    }
    if m.is_glassy && m.base_texture.is_none() {
        // refraction/water without a base texture: faint tinted pane
        color = Vec4::new(0.6, 0.75, 0.8, 0.15);
        flags |= material::FLAG_IGNORE_TEX_ALPHA;
    }
    (
        SourceMaterialParams {
            color,
            light_scale,
            alpha_cutoff: m.alpha_cutoff,
            flags,
            pbr_flags: pbr_flags(alpha_mode),
        },
        alpha_mode,
    )
}

fn pbr_flags(alpha_mode: AlphaMode) -> u32 {
    use bevy::pbr::StandardMaterialFlags as F;
    let mode = match alpha_mode {
        AlphaMode::Opaque => F::ALPHA_MODE_OPAQUE,
        AlphaMode::Mask(_) => F::ALPHA_MODE_MASK,
        AlphaMode::Blend => F::ALPHA_MODE_BLEND,
        AlphaMode::Premultiplied => F::ALPHA_MODE_PREMULTIPLIED,
        AlphaMode::Add => F::ALPHA_MODE_ADD,
        AlphaMode::Multiply => F::ALPHA_MODE_MULTIPLY,
        _ => F::ALPHA_MODE_OPAQUE,
    };
    (F::FOG_ENABLED | mode).bits()
}

fn make_material(
    assets: &mut SourceMapAssets,
    map: &LoadedMap,
    m: &MaterialInfo,
    flags: u32,
    lightmap: Option<Handle<Image>>,
) -> SourceMaterial {
    let light_scale = assets.settings.light_scale;
    let mut flags = if assets.settings.fullbright { flags | material::FLAG_UNLIT } else { flags };
    if std::env::var_os("SOURCE_LIGHT_ONLY").is_some() {
        flags |= material::FLAG_LIGHT_ONLY;
    }
    let (params, alpha_mode) = material_params(m, flags, light_scale);
    let base = m
        .base_texture
        .as_deref()
        .and_then(|t| texture_handle(assets, map, t))
        .or_else(|| Some(white_image(assets)));
    let base2 = m.base_texture2.as_deref().and_then(|t| texture_handle(assets, map, t));
    SourceMaterial {
        params,
        base_texture: base,
        lightmap,
        base_texture2: base2,
        alpha_mode,
        double_sided: m.no_cull || std::env::var_os("SOURCE_NOCULL").is_some(),
    }
}

fn mesh_from_brush(d: &MeshData) -> Mesh {
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, d.positions.clone())
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, d.normals.clone())
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, d.uv0.clone())
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_1, d.uv1.clone())
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, d.colors.clone())
        .with_inserted_indices(Indices::U32(d.indices.clone()))
}

fn lightmap_image(map: &LoadedMap) -> Image {
    let lm = &map.lightmap;
    let data: Vec<u8> = bytemuck::cast_slice(&lm.texels).to_vec();
    let mut img = Image::new(
        Extent3d { width: lm.width, height: lm.height, depth_or_array_layers: 1 },
        TextureDimension::D2,
        data,
        TextureFormat::Rgb9e5Ufloat,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor::linear());
    img
}

/// Build a cubemap from the six sky faces (Source order rt, lf, bk, ft, up, dn).
fn skybox_image(assets: &mut SourceMapAssets, map: &LoadedMap) -> Option<Handle<Image>> {
    let faces = map.sky_faces.as_ref()?;
    let mut decoded = Vec::new();
    for f in faces {
        let t = map.textures.get(f)?;
        let mut t = t.clone();
        t.mips.truncate(1);
        t.decompress();
        decoded.push(t);
    }
    // Every face must match the first in size and format.
    let (w, h, fmt) = (decoded[0].width, decoded[0].height, decoded[0].format);
    let size = w.max(h);
    if decoded.iter().any(|t| t.format != fmt) {
        return None;
    }
    let bpp = if fmt == crate::vtf::PixelFormat::Rgba16F { 8 } else { 4 };
    let resize = |t: &crate::vtf::VtfTexture| -> Vec<u8> {
        // nearest-neighbour resize to size x size (Portal 2 skies have half-height side faces)
        let mut out = vec![0u8; (size * size) as usize * bpp];
        for y in 0..size {
            let sy = (y * t.height / size).min(t.height - 1);
            for x in 0..size {
                let sx = (x * t.width / size).min(t.width - 1);
                let s = ((sy * t.width + sx) as usize) * bpp;
                let d = ((y * size + x) as usize) * bpp;
                out[d..d + bpp].copy_from_slice(&t.mips[0][s..s + bpp]);
            }
        }
        out
    };
    let rot = |px: Vec<u8>, quarter_turns: u32| -> Vec<u8> {
        let mut cur = px;
        for _ in 0..quarter_turns % 4 {
            let mut out = vec![0u8; cur.len()];
            for y in 0..size {
                for x in 0..size {
                    // rotate 90 degrees clockwise
                    let s = ((y * size + x) as usize) * bpp;
                    let d = ((x * size + (size - 1 - y)) as usize) * bpp;
                    out[d..d + bpp].copy_from_slice(&cur[s..s + bpp]);
                }
            }
            cur = out;
        }
        cur
    };
    let [rt, lf, bk, ft, up, dn] = [0, 1, 2, 3, 4, 5].map(|i| resize(&decoded[i]));
    // Source: ft looks down +X, bk -X, lf +Y, rt -Y, up +Z, dn -Z.
    // World: +X = Source +X, +Y = Source +Z, +Z = Source -Y.
    // Cubemap layer order: +X, -X, +Y, -Y, +Z, -Z.
    let layers = [ft, bk, rot(up, 1), rot(dn, 3), rt, lf];
    let data: Vec<u8> = layers.concat();
    let format = if bpp == 8 { TextureFormat::Rgba16Float } else { TextureFormat::Rgba8UnormSrgb };
    let mut img = Image::new(
        Extent3d { width: size, height: size, depth_or_array_layers: 6 },
        TextureDimension::D2,
        data,
        format,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::Cube),
        ..default()
    });
    Some(assets.images.add(img))
}

fn classify(e: &SourceEntity) -> MapEntityKind {
    match e.classname.as_str() {
        "info_player_start" => MapEntityKind::PlayerStart,
        "prop_weighted_cube" | "prop_monster_box" => MapEntityKind::WeightedCube,
        "prop_dynamic" | "prop_dynamic_override" | "prop_dynamic_ornament" => MapEntityKind::DynamicProp,
        "prop_physics" | "prop_physics_override" | "prop_physics_multiplayer" => MapEntityKind::PhysicsProp,
        "prop_testchamber_door" | "func_door" | "func_door_rotating" | "prop_door_rotating" => MapEntityKind::Door,
        "prop_floor_button" | "prop_floor_cube_button" | "prop_floor_ball_button" | "prop_button"
        | "prop_under_button" | "prop_under_floor_button" | "func_button" => MapEntityKind::Button,
        "light_environment" => MapEntityKind::LightEnvironment,
        c if c.starts_with("prop_") => MapEntityKind::OtherProp,
        _ if e.brush_model().is_some() => MapEntityKind::BrushEntity,
        _ => MapEntityKind::Other,
    }
}

fn parse_light(s: Option<&str>) -> (Vec3, f32) {
    let v: Vec<f32> = s
        .unwrap_or("255 255 255 200")
        .split_whitespace()
        .filter_map(crate::kv::parse_f32)
        .collect();
    let c = if v.len() >= 3 { Vec3::new(v[0], v[1], v[2]) / 255.0 } else { Vec3::ONE };
    let b = v.get(3).copied().unwrap_or(200.0);
    // Source colours are gamma space.
    (c.powf(2.2), b)
}

/// Spawn a converted map. Returns entity/player-start info in world space.
pub fn spawn_loaded_map(
    commands: &mut Commands,
    assets: &mut SourceMapAssets,
    map: &LoadedMap,
    offset: Vec3,
) -> MapInfo {
    let root = commands
        .spawn((
            Name::new(format!("source map {}", map.name)),
            SourceMapRoot { name: map.name.clone() },
            Transform::from_translation(offset),
            Visibility::default(),
            RigidBody::Static,
        ))
        .id();

    let lightmap = assets.images.add(lightmap_image(map));

    // Parent entity per brush model (0 = world at the root itself).
    let mut model_parent: HashMap<usize, Entity> = HashMap::new();
    model_parent.insert(0, root);
    let mut model_entities = Vec::new();
    for (&m, &ei) in &map.brush_model_entity {
        let e = &map.entities[ei];
        let child = commands
            .spawn((
                Name::new(format!("{} *{m}", e.classname)),
                Transform::from_translation(source_to_world(e.origin)).with_rotation(source_angles_to_world(e.angles)),
                Visibility::default(),
                ChildOf(root),
            ))
            .id();
        model_parent.insert(m, child);
        model_entities.push((m, ei, child));
    }

    // Map fog for the Valve shaders (range fog in Source units, linear colour).
    let (valve_fog, valve_fog_color) = map
        .entities
        .iter()
        .find(|e| e.classname == "env_fog_controller" && e.get("fogenable") == Some("1"))
        .map(|e| {
            let c = (e.get_vec3("fogcolor").unwrap_or(Vec3::splat(128.0)) / 255.0).powf(2.2);
            (
                crate::valve::fog_params(
                    e.get_f32("fogstart").unwrap_or(0.0),
                    e.get_f32("fogend").unwrap_or(10000.0),
                    e.get_f32("fogmaxdensity").unwrap_or(1.0).clamp(0.0, 1.0),
                ),
                [c.x, c.y, c.z],
            )
        })
        .unwrap_or(([0.0, 1.0, 0.0, 0.0], [0.0; 3]));
    let bc = bc_supported(assets);
    let use_valve = assets.settings.valve_shaders && assets.game.fs.is_some();
    let mut valve_fallbacks: Vec<(String, String)> = Vec::new();
    let mut valve_done: HashMap<usize, Result<(), String>> = HashMap::new();

    // Brush meshes: one material per texdata for this map.
    let mut mats: HashMap<usize, Handle<SourceMaterial>> = HashMap::new();
    let mut bounds = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for bm in map.brush_meshes.iter().chain(map.overlays.iter()) {
        let parent = model_parent.get(&bm.model).copied().unwrap_or(root);
        if bm.model == 0 {
            for p in &bm.mesh.positions {
                let p = Vec3::from(*p) + offset;
                bounds.0 = bounds.0.min(p);
                bounds.1 = bounds.1.max(p);
            }
        }
        let m = &map.materials[bm.material];
        if use_valve && matches!(m.shader.as_str(), "lightmappedgeneric" | "worldvertextransition" | "unlitgeneric") && !m.is_glassy {
            let fs = assets.game.fs.clone().unwrap();
            let white = fallback_image(&mut assets.images, &mut assets.cache, "white", [255; 4], true);
            let black = fallback_image(&mut assets.images, &mut assets.cache, "black", [0, 0, 0, 255], true);
            let grey = fallback_image(&mut assets.images, &mut assets.cache, "grey", [128, 128, 128, 255], false);
            let flat = fallback_image(&mut assets.images, &mut assets.cache, "flat_normal", [128, 128, 255, 255], false);
            let images = &mut assets.images;
            let cache = &mut *assets.cache;
            let mut texture = |name: &str, srgb: bool| valve_texture(images, cache, bc, map, name, srgb);
            let mut env = crate::valve::MapShaderEnv {
                fs: &fs,
                fog: valve_fog,
                fog_color: valve_fog_color,
                lightmap: lightmap.clone(),
                texture: &mut texture,
                white,
                black,
                grey,
                flat_normal: flat,
            };
            let built = if m.shader == "unlitgeneric" {
                crate::valve::vertex_lit_generic(
                    &assets.valve,
                    &mut assets.shaders,
                    &mut env,
                    m,
                    crate::valve::brush_vertices(&bm.mesh),
                    bm.mesh.indices.clone(),
                )
            } else {
                crate::valve::lightmapped_generic(&assets.valve, &mut assets.shaders, &mut env, m, &bm.mesh)
            };
            match built {
                Ok(draw) => {
                    commands.spawn((
                        Name::new(format!("valve brush {}", m.name)),
                        crate::valve::ValveSurface(Arc::new(draw)),
                        Transform::IDENTITY,
                        Visibility::default(),
                        ChildOf(parent),
                    ));
                    valve_done.entry(bm.material).or_insert(Ok(()));
                    continue;
                }
                Err(e) => {
                    valve_done.entry(bm.material).or_insert(Err(e));
                }
            }
        } else if use_valve {
            valve_done.entry(bm.material).or_insert(Err(format!("shader {} not translated yet", m.shader)));
        }
        let handle = match mats.get(&bm.material) {
            Some(h) => h.clone(),
            None => {
                let m = &map.materials[bm.material];
                let mat = make_material(assets, map, m, material::FLAG_LIGHTMAP, Some(lightmap.clone()));
                let h = assets.materials.add(mat);
                mats.insert(bm.material, h.clone());
                h
            }
        };
        commands.spawn((
            Name::new(format!("brush {}", map.materials[bm.material].name)),
            Mesh3d(assets.meshes.add(mesh_from_brush(&bm.mesh))),
            MeshMaterial3d(handle),
            Transform::IDENTITY,
            ChildOf(parent),
        ));
    }

    for (mi, r) in &valve_done {
        if let Err(e) = r {
            valve_fallbacks.push((map.materials[*mi].name.clone(), e.clone()));
        }
    }
    valve_fallbacks.sort();
    if use_valve {
        info!(
            "source_assets: {}: {} brush materials on Valve shaders, {} on the fallback: {:?}",
            map.name,
            valve_done.values().filter(|r| r.is_ok()).count(),
            valve_fallbacks.len(),
            valve_fallbacks
        );
    }
    assets.cache.fallback_materials.insert(map.name.clone(), valve_fallbacks);

    // Colliders.
    for c in &map.colliders {
        spawn_collider(commands, c, model_parent.get(&(c.model as usize)).copied().unwrap_or(root));
    }

    // Model surfaces on Valve shaders share these per-map inputs.
    let vhv_ctx = use_valve.then(|| ValveCtx {
        fog: valve_fog,
        fog_color: valve_fog_color,
        // our .vhv decode is 2*(b/255)^2.2; the engine's VS decodes (2b/255)^2.2
        light_scale: 2f32.powf(2.2) / 2.0,
        fallbacks: Default::default(),
    });
    let amb_ctx = use_valve.then(|| ValveCtx {
        fog: valve_fog,
        fog_color: valve_fog_color,
        light_scale: 1.0,
        fallbacks: Default::default(),
    });

    // Static props.
    let mut entities_out: Vec<MapEntity> = Vec::new();
    for sp in &map.static_props {
        let Some(model) = map.models.get(&sp.model) else { continue };
        let transform = Transform::from_translation(source_to_world(sp.origin))
            .with_rotation(source_angles_to_world(sp.angles));
        let e = spawn_model(
            commands,
            assets,
            map,
            model,
            sp.skin,
            transform,
            root,
            &|src_n: Vec3, vvd: u32| match &sp.vertex_light {
                Some(vl) => vl.get(vvd as usize).copied().unwrap_or(Vec3::ZERO),
                None => {
                    // Rotate the normal into Source world space for the ambient cube.
                    let q = source_angles_to_world(sp.angles);
                    let wn = q * source_dir_to_world(src_n);
                    sp.ambient.eval(Vec3::new(wn.x, -wn.z, wn.y))
                }
            },
            Vec3::from_slice(&sp.tint[..3]),
            None,
            if sp.vertex_light.is_some() { vhv_ctx.as_ref() } else { amb_ctx.as_ref() },
        );
        if sp.solid != 0 {
            spawn_prop_collider(commands, model, e);
        }
        let idx = entities_out.len();
        commands.entity(e).insert(SourceEntityRef { kind: MapEntityKind::StaticProp, index: idx });
        entities_out.push(MapEntity {
            kind: MapEntityKind::StaticProp,
            classname: "prop_static".into(),
            targetname: None,
            transform: Transform { translation: transform.translation + offset, ..transform },
            model: Some(sp.model.clone()),
            brush_model: None,
            skin: sp.skin,
            entity: Some(e),
            bounds: Some(model_bounds(model)),
            keyvalues: Vec::new(),
        });
    }

    // Entity lump.
    let mut player_start = None;
    let mut light_env = None;
    for (ei, e) in map.entities.iter().enumerate() {
        let kind = classify(e);
        let transform = Transform::from_translation(source_to_world(e.origin) + offset)
            .with_rotation(source_angles_to_world(e.angles));
        let mut spawned = None;
        let mut bounds_e = None;
        let model_path = map::entity_model(e);
        match kind {
            MapEntityKind::PlayerStart => {
                if player_start.is_none() || e.get("spawnflags").is_some_and(|f| f == "1") {
                    player_start = Some(
                        Transform::from_translation(source_to_world(e.origin) + offset)
                            .with_rotation(source_view_to_world(e.angles)),
                    );
                }
            }
            MapEntityKind::LightEnvironment => {
                let (color, brightness) = parse_light(e.get("_light"));
                let (ambient_color, ambient_brightness) = parse_light(e.get("_ambient"));
                let mut ang = e.angles;
                if let Some(p) = e.get_f32("pitch") {
                    ang.x = p;
                }
                light_env = Some(LightEnvironment {
                    direction: source_angles_to_world(ang) * Vec3::X,
                    color,
                    brightness,
                    ambient_color,
                    ambient_brightness,
                });
            }
            _ => {}
        }
        if let Some(path) = &model_path {
            if let Some(model) = map.models.get(path) {
                let skin = e.get("skin").and_then(|s| s.parse().ok()).unwrap_or(0);
                let scale = e.get_f32("modelscale").filter(|s| *s > 0.0).unwrap_or(1.0);
                let local = Transform::from_translation(source_to_world(e.origin))
                    .with_rotation(source_angles_to_world(e.angles))
                    .with_scale(Vec3::splat(scale));
                let rot = source_angles_to_world(e.angles);
                // Source lights models at their illumposition, rotated with the entity.
                let q_src = Quat::from_rotation_z(e.angles.y.to_radians())
                    * Quat::from_rotation_y(e.angles.x.to_radians())
                    * Quat::from_rotation_x(e.angles.z.to_radians());
                let amb = map.ambient_at(e.origin + q_src * model.illum_position * scale);
                let tint = e
                    .get_vec3("rendercolor")
                    .map(|c| (c / 255.0).powf(2.2))
                    .unwrap_or(Vec3::ONE);
                let ent = spawn_model(
                    commands,
                    assets,
                    map,
                    model,
                    skin,
                    local,
                    root,
                    &|src_n: Vec3, _| {
                        let wn = rot * source_dir_to_world(src_n);
                        amb.eval(Vec3::new(wn.x, -wn.z, wn.y))
                    },
                    tint,
                    Some(
                        e.get("defaultanim")
                            .filter(|s| !s.is_empty())
                            .or_else(|| map::class_default_sequence(&e.classname)),
                    ),
                    amb_ctx.as_ref(),
                );
                if e.get("startdisabled") == Some("1") || e.get("rendermode") == Some("10") {
                    commands.entity(ent).insert(Visibility::Hidden);
                }
                if let Some(tn) = &e.targetname {
                    commands.entity(ent).insert(Name::new(format!("{} {tn}", e.classname)));
                }
                spawned = Some(ent);
                bounds_e = Some(model_bounds(model));
            }
        }
        if spawned.is_none() {
            if let Some(m) = e.brush_model() {
                spawned = model_entities.iter().find(|x| x.0 == m).map(|x| x.2);
            }
        }
        if kind == MapEntityKind::Other && spawned.is_none() {
            continue;
        }
        let idx = entities_out.len();
        if let Some(s) = spawned {
            commands.entity(s).insert(SourceEntityRef { kind, index: idx });
        }
        entities_out.push(MapEntity {
            kind,
            classname: e.classname.clone(),
            targetname: e.targetname.clone(),
            transform,
            model: model_path,
            brush_model: e.brush_model(),
            skin: e.get("skin").and_then(|s| s.parse().ok()).unwrap_or(0),
            entity: spawned,
            bounds: bounds_e,
            keyvalues: e.keyvalues.clone(),
        });
        let _ = ei;
    }

    let arrival = map
        .entities
        .iter()
        .find(|e| {
            e.classname == "point_teleport"
                && e.targetname.as_deref().is_some_and(|t| t.to_ascii_lowercase().contains("arrival_teleport"))
        })
        .map(|e| {
            Transform::from_translation(source_to_world(e.origin) + offset)
                .with_rotation(source_view_to_world(e.angles))
        });
    let fog = map
        .entities
        .iter()
        .find(|e| e.classname == "env_fog_controller" && e.get("fogenable") == Some("1"))
        .map(|e| {
            let c = e.get_vec3("fogcolor").unwrap_or(Vec3::splat(128.0)) / 255.0;
            let c = c.powf(2.2);
            MapFog {
                color: Color::linear_rgb(c.x, c.y, c.z),
                start: e.get_f32("fogstart").unwrap_or(0.0) * shared::SOURCE_UNIT,
                end: e.get_f32("fogend").unwrap_or(10000.0) * shared::SOURCE_UNIT,
                max_density: e.get_f32("fogmaxdensity").unwrap_or(1.0).clamp(0.0, 1.0),
            }
        });
    if use_valve {
        let mut fb: std::collections::BTreeMap<String, String> = Default::default();
        for c in [&vhv_ctx, &amb_ctx].into_iter().flatten() {
            fb.extend(c.fallbacks.lock().unwrap().clone());
        }
        info!("source_assets: {}: {} model materials on the fallback: {:?}", map.name, fb.len(), fb);
        if let Some(list) = assets.cache.fallback_materials.get_mut(&map.name) {
            list.extend(fb.into_iter());
        }
    }
    let skybox = skybox_image(assets, map);
    if bounds.0.x > bounds.1.x {
        bounds = (offset, offset);
    }
    MapInfo {
        name: map.name.clone(),
        root,
        offset,
        player_start: player_start.unwrap_or(Transform::from_translation(offset)),
        arrival: arrival
            .or(player_start)
            .unwrap_or(Transform::from_translation(offset)),
        entities: entities_out,
        light_environment: light_env,
        fog,
        skybox,
        skyname: map.skyname.clone(),
        bounds,
        convert_seconds: 0.0,
    }
}

fn model_bounds(model: &ModelData) -> (Vec3, Vec3) {
    let a = source_to_world(model.mins);
    let b = source_to_world(model.maxs);
    (a.min(b), a.max(b))
}

fn spawn_collider(commands: &mut Commands, c: &ColliderData, parent: Entity) {
    let collider = if c.pieces.len() == 1 {
        Collider::convex_hull(c.pieces[0].clone())
    } else {
        let parts: Vec<(Vec3, Quat, Collider)> = c
            .pieces
            .iter()
            .filter_map(|p| Collider::convex_hull(p.clone()).map(|col| (Vec3::ZERO, Quat::IDENTITY, col)))
            .collect();
        (!parts.is_empty()).then(|| Collider::compound(parts))
    };
    let Some(collider) = collider else { return };
    let mut ec = commands.spawn((
        Name::new(if c.portalable { "portalable brush" } else { "brush" }),
        collider,
        Transform::IDENTITY,
        SourceCollider {
            contents: c.contents,
            brush: c.brush,
            model: c.model,
            sides: c.sides.clone(),
        },
        ChildOf(parent),
    ));
    if c.portalable {
        ec.insert(Portalable);
    }
    if c.shot_passthrough() {
        ec.insert(PortalShotPassthrough);
    }
}

/// Prop collision: the model's `.phy` convex pieces, or its bounding box.
fn spawn_prop_collider(commands: &mut Commands, model: &ModelData, parent: Entity) {
    let collider = if !model.physics.is_empty() {
        let parts: Vec<(Vec3, Quat, Collider)> = model
            .physics
            .iter()
            .filter_map(|p| Collider::convex_hull(p.clone()).map(|c| (Vec3::ZERO, Quat::IDENTITY, c)))
            .collect();
        match parts.len() {
            0 => None,
            1 => parts.into_iter().next().map(|p| p.2),
            _ => Some(Collider::compound(parts)),
        }
    } else {
        let (lo, hi) = model_bounds(model);
        let size = hi - lo;
        (size.min_element() > 0.01).then(|| {
            Collider::compound(vec![((lo + hi) * 0.5, Quat::IDENTITY, Collider::cuboid(size.x, size.y, size.z))])
        })
    };
    if let Some(c) = collider {
        commands.spawn((Name::new("prop collider"), c, Transform::IDENTITY, ChildOf(parent)));
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_model(
    commands: &mut Commands,
    assets: &mut SourceMapAssets,
    map: &LoadedMap,
    model: &ModelData,
    skin: i32,
    transform: Transform,
    parent: Entity,
    light: &dyn Fn(Vec3, u32) -> Vec3,
    tint: Vec3,
    // Some(sequence name or None for sequence 0) to draw the default pose;
    // None to draw the stored (reference / static prop) pose.
    pose: Option<Option<&str>>,
    valve: Option<&ValveCtx>,
) -> Entity {
    let e = commands
        .spawn((
            Name::new(model.path.clone()),
            transform,
            Visibility::default(),
            ChildOf(parent),
        ))
        .id();
    let meshes = match pose {
        Some(seq) => model.posed_meshes(seq),
        None => std::borrow::Cow::Borrowed(&model.meshes[..]),
    };
    for mesh in meshes.iter() {
        let Some(mat_name) = model.material_for(skin, mesh.material_index) else { continue };
        let Some(mi) = map.material_id(mat_name) else { continue };
        let minfo = &map.materials[mi];
        if !minfo.is_drawn() {
            continue;
        }
        if let Some(v) = valve {
            if matches!(minfo.shader.as_str(), "vertexlitgeneric" | "unlitgeneric") {
                let verts = crate::valve::model_vertices(mesh, &|i| {
                    let l = light(mesh.source_normals[i], mesh.vvd_index[i]) * tint * v.light_scale;
                    crate::valve::encode_static_light(l)
                });
                let fs = assets.game.fs.clone().unwrap();
                let bc = bc_supported(assets);
                let white = fallback_image(&mut assets.images, &mut assets.cache, "white", [255; 4], true);
                let black = fallback_image(&mut assets.images, &mut assets.cache, "black", [0, 0, 0, 255], true);
                let grey = fallback_image(&mut assets.images, &mut assets.cache, "grey", [128, 128, 128, 255], false);
                let flat = fallback_image(&mut assets.images, &mut assets.cache, "flat_normal", [128, 128, 255, 255], false);
                let images = &mut assets.images;
                let cache = &mut *assets.cache;
                let mut texture = |name: &str, srgb: bool| valve_texture(images, cache, bc, map, name, srgb);
                let mut env = crate::valve::MapShaderEnv {
                    fs: &fs,
                    fog: v.fog,
                    fog_color: v.fog_color,
                    lightmap: white.clone(),
                    texture: &mut texture,
                    white,
                    black,
                    grey,
                    flat_normal: flat,
                };
                match crate::valve::vertex_lit_generic(&assets.valve, &mut assets.shaders, &mut env, minfo, verts, mesh.indices.clone()) {
                    Ok(draw) => {
                        commands.spawn((
                            crate::valve::ValveSurface(Arc::new(draw)),
                            Transform::IDENTITY,
                            Visibility::default(),
                            ChildOf(e),
                        ));
                        continue;
                    }
                    Err(err) => {
                        v.fallbacks.lock().unwrap().insert(minfo.name.clone(), err);
                    }
                }
            } else {
                v.fallbacks
                    .lock()
                    .unwrap()
                    .insert(minfo.name.clone(), format!("shader {} not translated yet", minfo.shader));
            }
        }
        let key = (minfo.name.clone(), true);
        let mat = match assets.cache.prop_materials.get(&key) {
            Some(h) => h.clone(),
            None => {
                let m = make_material(assets, map, minfo, material::FLAG_VERTEX_LIGHT, None);
                let h = assets.materials.add(m);
                assets.cache.prop_materials.insert(key, h.clone());
                h
            }
        };
        let colors: Vec<[f32; 4]> = mesh
            .source_normals
            .iter()
            .zip(&mesh.vvd_index)
            .map(|(&n, &vi)| {
                let l = light(n, vi) * tint;
                [l.x, l.y, l.z, 1.0]
            })
            .collect();
        let m = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, mesh.positions.clone())
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, mesh.normals.clone())
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, mesh.uvs.clone())
            .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
            .with_inserted_indices(Indices::U32(mesh.indices.clone()));
        commands.spawn((
            Mesh3d(assets.meshes.add(m)),
            MeshMaterial3d(mat),
            Transform::IDENTITY,
            ChildOf(e),
        ));
    }
    e
}

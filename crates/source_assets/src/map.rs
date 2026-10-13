//! CPU-side map conversion: everything a chamber needs, decoded and in
//! world axes, with no Bevy world access. [`crate::plugin`] spawns it.

use crate::bsp_raw::{self, AmbientCube, RawBsp};
use crate::collision::{self, ColliderData};
use crate::entities::{self, SourceEntity};
use crate::fs::{GameFs, MapFs, PakFile};
use crate::geometry::{self, FaceBuilder, LightmapAtlas, MeshData};
use crate::models::{self, ModelData};
use crate::surfaceprops::SurfaceProps;
use crate::vmt::{self, BlendKind, MaterialInfo, ToolKind};
use crate::vtf::VtfTexture;
use bevy::math::{DVec3, Vec3};
use rayon::prelude::*;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

/// Brush geometry for one (brush model, material) pair.
#[derive(Clone, Debug)]
pub struct BrushMesh {
    /// 0 = world; N = the brush entity whose model is `*N`.
    pub model: usize,
    /// Index into [`LoadedMap::materials`].
    pub material: usize,
    pub mesh: MeshData,
}

/// A `prop_static` from the static prop game lump.
#[derive(Clone, Debug)]
pub struct StaticProp {
    pub model: String,
    /// Source units.
    pub origin: Vec3,
    pub angles: Vec3,
    pub skin: i32,
    /// SOLID_* (0 none, 2 bbox, 6 vphysics).
    pub solid: u8,
    pub flags: u8,
    pub tint: [f32; 4],
    /// Per-vertex baked light from `sp_hdr_N.vhv`, indexed by global VVD vertex.
    pub vertex_light: Option<Arc<Vec<Vec3>>>,
    /// Fallback ambient light at the prop's lighting origin.
    pub ambient: AmbientCube,
}

/// A converted map, ready to spawn any number of times.
pub struct LoadedMap {
    pub name: String,
    /// Material per BSP texdata index (then props' materials appended).
    pub materials: Vec<MaterialInfo>,
    material_index: HashMap<String, usize>,
    pub brush_meshes: Vec<BrushMesh>,
    /// `info_overlay` decals, drawn after the brushes.
    pub overlays: Vec<BrushMesh>,
    pub lightmap: LightmapAtlas,
    pub colliders: Vec<ColliderData>,
    pub entities: Vec<SourceEntity>,
    /// Brush model index -> entity index using it.
    pub brush_model_entity: HashMap<usize, usize>,
    pub static_props: Vec<StaticProp>,
    pub models: HashMap<String, Arc<ModelData>>,
    /// Decoded base textures by canonical name.
    pub textures: HashMap<String, VtfTexture>,
    pub skyname: Option<String>,
    /// Sky faces in order rt, lf, bk, ft, up, dn (texture names).
    pub sky_faces: Option<[String; 6]>,
    ambient: Option<AmbientLookup>,
    pub timings: Vec<(&'static str, f32)>,
}

struct AmbientLookup {
    nodes: Vec<(usize, [i32; 2])>,
    planes: Vec<(Vec3, f32)>,
    leaf_samples: Vec<Vec<AmbientCube>>,
    leaf_solid: Vec<bool>,
}

impl AmbientLookup {
    fn sample(&self, p: Vec3) -> Option<AmbientCube> {
        if self.nodes.is_empty() {
            return None;
        }
        let mut node = 0i32;
        let mut guard = 0;
        while node >= 0 && guard < 4096 {
            let (plane, children) = self.nodes[node as usize];
            let (n, d) = self.planes[plane];
            node = if p.dot(n) - d >= 0.0 { children[0] } else { children[1] };
            guard += 1;
        }
        let leaf = (-1 - node) as usize;
        if self.leaf_solid.get(leaf).copied().unwrap_or(false) {
            return None;
        }
        let samples = self.leaf_samples.get(leaf)?;
        if samples.is_empty() {
            return None;
        }
        let mut avg = AmbientCube::default();
        for s in samples {
            for i in 0..6 {
                avg.sides[i] += s.sides[i] / samples.len() as f32;
            }
        }
        avg.pos = p;
        Some(avg)
    }
}

impl LoadedMap {
    pub fn material(&self, name: &str) -> Option<&MaterialInfo> {
        self.material_index.get(&vmt::canonical_name(name)).map(|&i| &self.materials[i])
    }

    pub fn material_id(&self, name: &str) -> Option<usize> {
        self.material_index.get(&vmt::canonical_name(name)).copied()
    }

    /// Baked ambient light at a Source-space point (for dynamic models).
    /// Points inside solid leaves (a door origin sunk into the floor) are
    /// nudged to the nearest empty leaf within 48 units.
    pub fn ambient_at(&self, p: Vec3) -> AmbientCube {
        self.ambient
            .as_ref()
            .and_then(|a| {
                a.sample(p).or_else(|| {
                    [8.0f32, 16.0, 32.0, 48.0].iter().find_map(|&r| {
                        [Vec3::Z, Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y, -Vec3::Z]
                            .iter()
                            .find_map(|d| a.sample(p + *d * r))
                    })
                })
            })
            .unwrap_or(AmbientCube {
                sides: [Vec3::splat(0.05); 6],
                pos: p,
            })
    }

    pub fn worldspawn(&self) -> Option<&SourceEntity> {
        self.entities.iter().find(|e| e.classname == "worldspawn")
    }
}

/// Brush entity classes that are solid to the player.
fn brush_entity_solid(e: &SourceEntity) -> bool {
    let c = e.classname.as_str();
    match c {
        "func_brush" => e.get("solidity") != Some("1"),
        "func_door" | "func_door_rotating" | "func_movelinear" | "func_wall" | "func_wall_toggle"
        | "func_breakable" | "func_tracktrain" | "func_train" | "func_rotating" | "func_platrot"
        | "func_physbox" | "func_button" | "func_rot_button" | "func_detail" | "func_lod"
        | "func_breakable_surf" | "func_reflective_glass" => true,
        _ => false,
    }
}

/// Brush entity classes whose faces are drawn.
fn brush_entity_drawn(e: &SourceEntity) -> bool {
    let c = e.classname.as_str();
    if c.starts_with("trigger_")
        || matches!(
            c,
            "func_noportal_volume" | "func_portal_bumper" | "func_portal_detector" | "func_areaportal"
                | "func_areaportalwindow" | "func_occluder" | "func_clip_vphysics" | "func_placement_clip"
                | "func_portal_orientation" | "func_precipitation" | "func_viscluster" | "func_vehicleclip"
        )
    {
        return false;
    }
    if e.get("rendermode") == Some("10") {
        return false;
    }
    true
}

fn starts_disabled(e: &SourceEntity) -> bool {
    e.get("startdisabled") == Some("1")
}

fn texdata_name(bsp: &vbsp::Bsp, td: usize) -> String {
    let Some(t) = bsp.textures_data.get(td) else { return String::new() };
    let Some(&off) = bsp.texture_string_tables.get(t.name_string_table_id as usize) else {
        return String::new();
    };
    let s = &bsp.texture_string_data[off as usize..];
    let end = s.find('\0').unwrap_or(s.len());
    vmt::canonical_name(&s[..end])
}

/// Parse the Portal 2 (v9..v11) / older static prop game lump directly;
/// vbsp's reader misreads version 9's layout.
fn parse_static_props(raw: &RawBsp, data: &[u8]) -> Vec<(String, Vec3, Vec3, u16, u8, u8, i32, [u8; 4], Vec3)> {
    let Some(gl) = raw.lump(35) else { return Vec::new() };
    let u32a = |d: &[u8], o: usize| u32::from_le_bytes(d[o..o + 4].try_into().unwrap());
    let f32a = |d: &[u8], o: usize| f32::from_le_bytes(d[o..o + 4].try_into().unwrap());
    let n = u32a(gl, 0) as usize;
    for i in 0..n {
        let o = 4 + i * 16;
        if o + 16 > gl.len() {
            break;
        }
        if &gl[o..o + 4] != b"prps" {
            continue;
        }
        let flags = u16::from_le_bytes([gl[o + 4], gl[o + 5]]);
        let version = u16::from_le_bytes([gl[o + 6], gl[o + 7]]);
        let off = u32a(gl, o + 8) as usize;
        let len = u32a(gl, o + 12) as usize;
        if flags & 1 != 0 || off + len > data.len() {
            bevy::log::warn!("compressed/out of range static prop lump, skipping props");
            return Vec::new();
        }
        let d = &data[off..off + len];
        let dn = u32a(d, 0) as usize;
        let names: Vec<String> = (0..dn)
            .map(|k| {
                let s = &d[4 + k * 128..4 + (k + 1) * 128];
                let end = s.iter().position(|&c| c == 0).unwrap_or(128);
                crate::fs::normalize_path(&String::from_utf8_lossy(&s[..end]))
            })
            .collect();
        let mut p = 4 + dn * 128;
        let ln = u32a(d, p) as usize;
        p += 4 + ln * 2;
        let pn = u32a(d, p) as usize;
        p += 4;
        if pn == 0 {
            return Vec::new();
        }
        let size = (d.len() - p) / pn;
        let _ = version;
        let mut out = Vec::with_capacity(pn);
        for k in 0..pn {
            let s = &d[p + k * size..p + (k + 1) * size];
            let origin = Vec3::new(f32a(s, 0), f32a(s, 4), f32a(s, 8));
            let angles = Vec3::new(f32a(s, 12), f32a(s, 16), f32a(s, 20));
            let ptype = u16::from_le_bytes([s[24], s[25]]);
            let solid = s[30];
            let pflags = s[31];
            let skin = i32::from_le_bytes(s[32..36].try_into().unwrap());
            let lighting_origin = Vec3::new(f32a(s, 44), f32a(s, 48), f32a(s, 52));
            // v7+: diffuse modulation sits at 64 (v9/v10: after cpu/gpu levels).
            let tint = if size >= 68 && version >= 7 { [s[64], s[65], s[66], s[67]] } else { [255; 4] };
            let Some(name) = names.get(ptype as usize) else { continue };
            out.push((name.clone(), origin, angles, ptype, solid, pflags, skin, tint, lighting_origin));
        }
        return out;
    }
    Vec::new()
}

/// Scale applied to decoded `.vhv` vertex light.
pub const VHV_SCALE: f32 = 2.0;

/// Parse `sp_hdr_N.vhv` into a global-VVD-indexed light array.
fn parse_vhv(data: &[u8], model: &ModelData) -> Option<Vec<Vec3>> {
    let u = |o: usize| -> Option<u32> { Some(u32::from_le_bytes(data.get(o..o + 4)?.try_into().ok()?)) };
    let version = u(0)?;
    if version != 2 {
        return None;
    }
    let vsize = u(12)? as usize;
    let nmeshes = u(20)? as usize;
    if vsize != 4 {
        return None;
    }
    let mut out = vec![Vec3::ZERO; model.vertex_count];
    let mut lod0 = 0;
    for m in 0..nmeshes {
        let h = 40 + m * 28;
        let lod = u(h)?;
        let count = u(h + 4)? as usize;
        let off = u(h + 8)? as usize;
        if lod != 0 {
            continue;
        }
        let order = model.lod0_vertex_order.get(lod0)?;
        lod0 += 1;
        for (k, &gi) in order.iter().enumerate().take(count) {
            let c = data.get(off + k * 4..off + k * 4 + 4)?;
            // BGRA8 in gamma space (see README: calibrated against lightmaps)
            let g = |b: u8| (b as f32 / 255.0).powf(2.2) * VHV_SCALE;
            let v = Vec3::new(g(c[2]), g(c[1]), g(c[0]));
            if let Some(slot) = out.get_mut(gi as usize) {
                *slot = v;
            }
        }
    }
    Some(out)
}

/// Studio models an entity class renders when it has no `model` key.
pub fn default_entity_model(e: &SourceEntity) -> Option<String> {
    let m = match e.classname.as_str() {
        "prop_weighted_cube" => match e.get("cubetype").unwrap_or("0") {
            "1" => "models/props/metal_box.mdl",
            "2" => "models/props/reflection_cube.mdl",
            "3" => "models/props_gameplay/mp_ball.mdl",
            "4" => "models/props_underground/underground_weighted_cube.mdl",
            _ => "models/props/metal_box.mdl",
        },
        "prop_testchamber_door" => "models/props/portal_door_combined.mdl",
        "prop_floor_button" => "models/props/portal_button.mdl",
        "prop_floor_cube_button" => "models/props/box_socket.mdl",
        "prop_floor_ball_button" => "models/props/ball_button.mdl",
        "prop_button" => "models/props/switch001.mdl",
        "prop_under_button" => "models/props_underground/underground_testchamber_button.mdl",
        "prop_under_floor_button" => "models/props_underground/underground_floor_button.mdl",
        "prop_laser_catcher" => "models/props/laser_catcher.mdl",
        "prop_laser_relay" => "models/props/laser_receptacle.mdl",
        "env_portal_laser" => "models/props/laser_emitter.mdl",
        "prop_monster_box" => "models/npcs/monsters/monster_a.mdl",
        _ => return None,
    };
    Some(m.to_string())
}

/// The sequence the game's entity code puts a class's model in at spawn,
/// when the map gives no `DefaultAnim`.
pub fn class_default_sequence(classname: &str) -> Option<&'static str> {
    match classname {
        // spawns closed; sequence 0 ("close") starts from open
        "prop_testchamber_door" => Some("idleclose"),
        _ => None,
    }
}

/// Entity classes whose studio model we render.
pub fn entity_renders_model(e: &SourceEntity) -> bool {
    let c = e.classname.as_str();
    c.starts_with("prop_") && c != "prop_portal" && c != "prop_static"
        || matches!(c, "env_portal_laser" | "npc_security_camera" | "npc_portal_turret_floor")
}

pub fn entity_model(e: &SourceEntity) -> Option<String> {
    if !entity_renders_model(e) {
        return None;
    }
    // Portal 2 entities with a fixed model ignore a missing `model` key.
    match e.classname.as_str() {
        "prop_weighted_cube" | "prop_testchamber_door" | "prop_floor_button" | "prop_floor_cube_button"
        | "prop_floor_ball_button" | "prop_under_floor_button" => {
            return e.studio_model().or_else(|| default_entity_model(e));
        }
        _ => {}
    }
    e.studio_model().or_else(|| default_entity_model(e))
}

/// Convert a map. `name` is a map name (`sp_a1_intro3`) or a path to a `.bsp`.
pub fn load_map(fs: &GameFs, surfprops: &SurfaceProps, name: &str) -> anyhow::Result<LoadedMap> {
    let t0 = Instant::now();
    let mut timings = Vec::new();
    let mut lap = |label: &'static str, t: &mut Instant| {
        timings.push((label, t.elapsed().as_secs_f32()));
        *t = Instant::now();
    };
    let mut t = Instant::now();

    let data = if name.ends_with(".bsp") && std::path::Path::new(name).is_file() {
        std::fs::read(name)?
    } else {
        fs.read(&format!("maps/{name}.bsp"))
            .ok_or_else(|| anyhow::anyhow!("map {name} not found in {}", fs.config.root.display()))?
    };
    let map_name = std::path::Path::new(name)
        .file_stem()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_else(|| name.to_string());
    let bsp = vbsp::Bsp::read(&data).map_err(|e| anyhow::anyhow!("bsp {name}: {e}"))?;
    let raw = RawBsp::new(&data)?;
    let pak = PakFile::from_vbsp(&bsp.pack);
    lap("parse bsp", &mut t);

    let mapfs = MapFs { game: fs, pak: &pak };
    let read = |p: &str| mapfs.read(p);

    let entities = entities::parse_entities(&bsp.entities);
    let mut brush_model_entity = HashMap::new();
    for (i, e) in entities.iter().enumerate() {
        if let Some(m) = e.brush_model() {
            brush_model_entity.insert(m, i);
        }
    }

    // --- materials (one per texdata) ---
    let td_names: Vec<String> = (0..bsp.textures_data.len()).map(|i| texdata_name(&bsp, i)).collect();
    let mut materials: Vec<MaterialInfo> = td_names.par_iter().map(|n| vmt::resolve(&read, n)).collect();
    lap("materials", &mut t);

    // --- which models are drawn ---
    let mut drawn_models = vec![0usize];
    for (&m, &ei) in &brush_model_entity {
        let e = &entities[ei];
        if m < bsp.models.len() && brush_entity_drawn(e) && !starts_disabled(e) {
            drawn_models.push(m);
        }
    }
    drawn_models.sort_unstable();

    let face_material = |fi: usize| -> Option<usize> {
        let f = &bsp.faces[fi];
        let ti = bsp.textures_info.get(f.texture_info as usize)?;
        let flags = ti.flags.bits();
        if flags
            & (geometry::SURF_NODRAW
                | geometry::SURF_SKY
                | geometry::SURF_SKY2D
                | geometry::SURF_TRIGGER
                | geometry::SURF_HINT
                | geometry::SURF_SKIP)
            != 0
        {
            return None;
        }
        let td = ti.texture_data_index as usize;
        materials.get(td).filter(|m| m.is_drawn())?;
        Some(td)
    };

    let mut faces_by_model: Vec<(usize, Vec<usize>)> = Vec::new();
    let mut all_faces = Vec::new();
    for &m in &drawn_models {
        let model = &bsp.models[m];
        let faces: Vec<usize> = (model.first_face..model.first_face + model.face_count)
            .map(|f| f as usize)
            .filter(|&f| f < bsp.faces.len() && face_material(f).is_some())
            .collect();
        all_faces.extend_from_slice(&faces);
        faces_by_model.push((m, faces));
    }

    // --- geometry + lightmaps ---
    let fb = FaceBuilder::new(&bsp, &raw, &all_faces);
    lap("lightmap atlas", &mut t);
    let mut brush_meshes: Vec<BrushMesh> = faces_by_model
        .par_iter()
        .flat_map_iter(|(m, faces)| {
            let mut by_mat: HashMap<usize, MeshData> = HashMap::new();
            for &f in faces {
                let mat = face_material(f).unwrap();
                fb.build_face(f, by_mat.entry(mat).or_default(), Vec3::ZERO);
            }
            by_mat
                .into_iter()
                .filter(|(_, mesh)| !mesh.indices.is_empty())
                .map(|(material, mesh)| BrushMesh { model: *m, material, mesh })
                .collect::<Vec<_>>()
        })
        .collect();
    brush_meshes.sort_by_key(|b| (b.model, b.material));
    // info_overlays (signs, stains): quads lit by the face they sit on.
    let mut overlays: Vec<BrushMesh> = Vec::new();
    for ov in bsp_raw::parse_overlays(&raw) {
        let Some((ti, mesh)) = fb.build_overlay(&ov) else { continue };
        let Some(t) = bsp.textures_info.get(ti) else { continue };
        let td = t.texture_data_index as usize;
        if materials.get(td).is_some_and(|m| m.is_drawn()) {
            overlays.push(BrushMesh { model: 0, material: td, mesh });
        }
    }
    let lightmap = fb.atlas.clone();
    lap("brush meshes", &mut t);

    // --- collision ---
    let leaves = bsp_raw::parse_leaves(&raw);
    let noportal_volumes: Vec<(DVec3, DVec3)> = entities
        .iter()
        .filter(|e| e.classname == "func_noportal_volume" && !starts_disabled(e))
        .filter_map(|e| {
            let m = bsp.models.get(e.brush_model()?)?;
            let o = e.origin.as_dvec3();
            let mins = DVec3::new(m.mins.x as f64, m.mins.y as f64, m.mins.z as f64) + o;
            let maxs = DVec3::new(m.maxs.x as f64, m.maxs.y as f64, m.maxs.z as f64) + o;
            Some((mins - DVec3::splat(1.0), maxs + DVec3::splat(1.0)))
        })
        .collect();

    // brushes per model via each model's node tree
    let model_brushes = |m: usize| -> BTreeSet<usize> {
        let mut out = BTreeSet::new();
        let Some(model) = bsp.models.get(m) else { return out };
        let mut stack = vec![model.head_node];
        let mut seen = HashSet::new();
        while let Some(n) = stack.pop() {
            if n < 0 {
                let li = (-1 - n) as usize;
                if let Some(l) = leaves.get(li) {
                    for k in 0..l.leaf_brush_count as usize {
                        if let Some(b) = bsp.leaf_brushes.get(l.first_leaf_brush as usize + k) {
                            out.insert(b.brush as usize);
                        }
                    }
                }
            } else if seen.insert(n) {
                if let Some(node) = bsp.nodes.get(n as usize) {
                    stack.extend_from_slice(&node.children);
                }
            }
        }
        out
    };

    let mut collide_models: Vec<(usize, DVec3)> = vec![(0, DVec3::ZERO)];
    for (&m, &ei) in &brush_model_entity {
        let e = &entities[ei];
        if m > 0 && m < bsp.models.len() && brush_entity_solid(e) && !starts_disabled(e) {
            collide_models.push((m, DVec3::ZERO));
        }
    }
    collide_models.sort_by_key(|c| c.0);
    let texinfo_material = |ti: i16| -> Option<usize> {
        if ti < 0 {
            return None;
        }
        bsp.textures_info.get(ti as usize).map(|t| t.texture_data_index as usize)
    };
    let side_portalable = |contents: u32| {
        let materials = &materials;
        let noportal_volumes = &noportal_volumes;
        move |s: &collision::BrushSide, centroid: DVec3| -> bool {
            if contents & (collision::CONTENTS_SOLID | collision::CONTENTS_MOVEABLE) == 0
                || contents & (collision::CONTENTS_GRATE | collision::CONTENTS_WINDOW) != 0
            {
                return false;
            }
            if s.texinfo_flags
                & (geometry::SURF_NOPORTAL
                    | geometry::SURF_SKY
                    | geometry::SURF_SKY2D
                    | geometry::SURF_NODRAW
                    | geometry::SURF_TRIGGER
                    | geometry::SURF_TRANS)
                != 0
            {
                return false;
            }
            let Some(m) = s.material.and_then(|i| materials.get(i)) else { return false };
            if m.tool != ToolKind::None
                || m.no_portal
                || m.is_glassy
                || matches!(m.blend, BlendKind::Blend | BlendKind::Additive)
            {
                return false;
            }
            if m.surface_prop.as_deref().is_some_and(|sp| surfprops.is_glass(sp)) {
                return false;
            }
            !noportal_volumes
                .iter()
                .any(|(lo, hi)| centroid.cmpge(*lo).all() && centroid.cmple(*hi).all())
        }
    };
    let side_info = |s: &collision::BrushSide| -> (String, Option<String>) {
        match s.material.and_then(|i| materials.get(i)) {
            Some(m) => (m.name.clone(), m.surface_prop.clone()),
            None => (String::new(), None),
        }
    };
    let colliders: Vec<ColliderData> = collide_models
        .par_iter()
        .flat_map_iter(|&(m, origin)| {
            let brushes = model_brushes(m);
            let mut out = Vec::new();
            for b in brushes {
                let Some(brush) = bsp.brushes.get(b) else { continue };
                let contents = brush.flags.bits();
                if contents & collision::MASK_PLAYERSOLID == 0 {
                    continue;
                }
                let sides = collision::brush_sides(&bsp, b, &texinfo_material);
                // Brushes made only of non-solid tool sides (triggers) are skipped.
                let all_nonsolid = sides.iter().all(|s| {
                    s.material
                        .and_then(|i| materials.get(i))
                        .is_some_and(|mm| mm.tool == ToolKind::NonSolid)
                });
                if all_nonsolid && contents & collision::CONTENTS_SOLID == 0 {
                    continue;
                }
                out.extend(collision::brush_colliders(
                    &sides,
                    contents,
                    b as u32,
                    m as u32,
                    origin,
                    &side_portalable(contents),
                    &side_info,
                ));
            }
            out
        })
        .collect();
    lap("collision", &mut t);

    // --- ambient lighting lookup for props ---
    let ambient = bsp_raw::parse_leaf_ambient(&raw).map(|la| {
        let leaf_samples = la
            .index
            .iter()
            .map(|&(first, count)| {
                la.samples
                    .get(first as usize..(first as usize + count as usize))
                    .map(|s| s.iter().map(|(c, _)| *c).collect())
                    .unwrap_or_default()
            })
            .collect();
        AmbientLookup {
            nodes: bsp.nodes.iter().map(|n| (n.plane_index as usize, n.children)).collect(),
            planes: bsp
                .planes
                .iter()
                .map(|p| (Vec3::new(p.normal.x, p.normal.y, p.normal.z), p.dist))
                .collect(),
            leaf_samples,
            leaf_solid: leaves.iter().map(|l| l.contents & 1 != 0).collect(),
        }
    });

    // --- models (static props + entity props) ---
    let raw_props = parse_static_props(&raw, &data);
    let mut model_names: BTreeSet<String> = raw_props
        .iter()
        .filter(|p| p.5 & 0x4 == 0)
        .map(|p| p.0.clone())
        .collect();
    for e in &entities {
        if let Some(m) = entity_model(e) {
            model_names.insert(m);
        }
    }
    let material_exists = |m: &str| mapfs.read(&format!("materials/{m}.vmt")).is_some();
    let models: HashMap<String, Arc<ModelData>> = model_names
        .par_iter()
        .filter_map(|m| match models::load_model(m, &read, &material_exists) {
            Ok(md) => Some((m.clone(), Arc::new(md))),
            Err(e) => {
                bevy::log::warn!("model {m}: {e}");
                None
            }
        })
        .collect();
    lap("models", &mut t);

    let static_props: Vec<StaticProp> = raw_props
        .par_iter()
        .enumerate()
        .filter(|(_, p)| p.5 & 0x4 == 0)
        .map(|(i, p)| {
            let (model, origin, angles, _pt, solid, flags, skin, tint, lorigin) = p.clone();
            let vertex_light = models.get(&model).and_then(|md| {
                let d = pak.get(&format!("sp_hdr_{i}.vhv")).or_else(|| pak.get(&format!("sp_{i}.vhv")))?;
                parse_vhv(d, md).map(Arc::new)
            });
            let light_pos = if flags & 0x2 != 0 { lorigin } else { origin + Vec3::Z * 4.0 };
            let ambient_cube = ambient
                .as_ref()
                .and_then(|a| {
                    a.sample(light_pos).or_else(|| {
                        [8.0f32, 16.0, 32.0].iter().find_map(|&r| a.sample(light_pos + Vec3::Z * r))
                    })
                })
                .unwrap_or(AmbientCube { sides: [Vec3::splat(0.05); 6], pos: light_pos });
            StaticProp {
                model,
                origin,
                angles,
                skin,
                solid,
                flags,
                tint: tint.map(|c| c as f32 / 255.0),
                vertex_light,
                ambient: ambient_cube,
            }
        })
        .collect();
    lap("prop lighting", &mut t);

    // --- prop materials: append to the table ---
    let mut material_index: HashMap<String, usize> = HashMap::new();
    for (i, m) in materials.iter().enumerate() {
        material_index.entry(m.name.clone()).or_insert(i);
    }
    let prop_mats: BTreeSet<String> = models
        .values()
        .flat_map(|m| m.all_materials().map(|s| s.to_string()).collect::<Vec<_>>())
        .filter(|n| !material_index.contains_key(n))
        .collect();
    let resolved: Vec<MaterialInfo> = prop_mats.par_iter().map(|n| vmt::resolve(&read, n)).collect();
    for m in resolved {
        material_index.insert(m.name.clone(), materials.len());
        materials.push(m);
    }

    // --- sky ---
    let skyname = entities
        .iter()
        .find(|e| e.classname == "worldspawn")
        .and_then(|w| w.get("skyname"))
        .map(|s| s.to_ascii_lowercase());
    let mut sky_faces = None;
    let mut sky_textures: Vec<String> = Vec::new();
    if let Some(sky) = &skyname {
        let faces = ["rt", "lf", "bk", "ft", "up", "dn"].map(|s| {
            // prefer the HDR variant when present
            let hdr = format!("skybox/{sky}_hdr{s}");
            let ldr = format!("skybox/{sky}{s}");
            let pick = if mapfs.read(&format!("materials/{hdr}.vmt")).is_some() { hdr } else { ldr };
            vmt::resolve(&read, &pick).base_texture.unwrap_or(pick)
        });
        if faces.iter().all(|f| mapfs.read(&format!("materials/{f}.vtf")).is_some()) {
            sky_textures.extend(faces.iter().cloned());
            sky_faces = Some(faces);
        }
    }

    // --- textures ---
    let used: HashSet<usize> = brush_meshes.iter().chain(overlays.iter()).map(|b| b.material).collect();
    let mut tex_names: BTreeSet<String> = BTreeSet::new();
    for (i, m) in materials.iter().enumerate() {
        let is_prop = i >= bsp.textures_data.len();
        if (is_prop || used.contains(&i)) && m.is_drawn() {
            tex_names.extend(m.base_texture.iter().cloned());
            tex_names.extend(m.base_texture2.iter().cloned());
            // textures the Valve shaders sample too
            for key in ["$bumpmap", "$bumpmap2", "$envmapmask", "$detail", "$blendmodulatetexture", "$envmap"] {
                if let Some(t) = vmt::texture_param(&m.params, key) {
                    if t != "env_cubemap" {
                        tex_names.insert(t);
                    }
                }
            }
        }
    }
    tex_names.extend(sky_textures);
    let textures: HashMap<String, VtfTexture> = tex_names
        .par_iter()
        .filter_map(|n| {
            let data = mapfs.read(&format!("materials/{n}.vtf"))?;
            match VtfTexture::parse(&data) {
                Ok(t) => Some((n.clone(), t)),
                Err(e) => {
                    bevy::log::warn!("texture {n}: {e}");
                    None
                }
            }
        })
        .collect();
    lap("textures", &mut t);
    timings.push(("total", t0.elapsed().as_secs_f32()));

    Ok(LoadedMap {
        name: map_name,
        materials,
        material_index,
        brush_meshes,
        overlays,
        lightmap,
        colliders,
        entities,
        brush_model_entity,
        static_props,
        models,
        textures,
        skyname,
        sky_faces,
        ambient,
        timings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fs() -> Option<(GameFs, SurfaceProps)> {
        let cfg = crate::config::GameConfig::from_env();
        if !cfg.root.join("portal2").is_dir() {
            return None;
        }
        let fs = GameFs::mount(cfg).unwrap();
        let sp = SurfaceProps::load(&fs);
        Some((fs, sp))
    }

    #[test]
    fn converts_sp_a1_intro3() {
        let Some((fs, sp)) = fs() else { return };
        let map = load_map(&fs, &sp, "sp_a1_intro3").unwrap();
        eprintln!("timings: {:?}", map.timings);
        let tris: usize = map.brush_meshes.iter().map(|b| b.mesh.triangle_count()).sum();
        eprintln!(
            "{} brush meshes, {tris} tris, lightmap {}x{} hdr={}, {} colliders ({} portalable), {} props, {} models, {} textures, sky {:?}",
            map.brush_meshes.len(),
            map.lightmap.width,
            map.lightmap.height,
            map.lightmap.hdr,
            map.colliders.len(),
            map.colliders.iter().filter(|c| c.portalable).count(),
            map.static_props.len(),
            map.models.len(),
            map.textures.len(),
            map.sky_faces
        );
        assert!(tris > 1000);
        assert!(map.lightmap.hdr);
        assert!(map.colliders.iter().any(|c| c.portalable));
        assert!(map.colliders.iter().any(|c| !c.portalable));
        assert!(map.entities.iter().any(|e| e.classname == "info_player_start"));
        assert!(map.static_props.len() > 100);
        assert!(map.static_props.iter().filter(|p| p.vertex_light.is_some()).count() > 100);
        let total = map.timings.last().unwrap().1;
        assert!(total < 20.0, "conversion took {total}s");
        // Every drawn material with a base texture found its texture.
        let missing: Vec<_> = map
            .materials
            .iter()
            .filter(|m| m.is_drawn() && m.shader == "missing")
            .map(|m| m.name.clone())
            .collect();
        eprintln!("missing materials: {missing:?}");
    }
}


#[cfg(test)]
mod portal_tests {
    /// White tiles are portalable, black metal and tool textures are not,
    /// and every collider's class matches all of its sides.
    #[test]
    fn portalability_follows_materials() {
        let cfg = crate::config::GameConfig::from_env();
        if !cfg.root.join("portal2").is_dir() {
            return;
        }
        let fs = crate::fs::GameFs::mount(cfg).unwrap();
        let sp = crate::surfaceprops::SurfaceProps::load(&fs);
        let map = super::load_map(&fs, &sp, "sp_a2_bridge_intro").unwrap();
        let mut white = (0, 0);
        for c in &map.colliders {
            for s in &c.sides {
                assert_eq!(s.portalable, c.portalable, "collider class must match its sides");
                if s.material.starts_with("metal/black_") || s.material.starts_with("tools/") {
                    assert!(!s.portalable, "{} should not be portalable", s.material);
                }
                if s.material.starts_with("tile/white_wall_tile") {
                    if s.portalable { white.0 += 1 } else { white.1 += 1 }
                }
            }
        }
        // most white wall tiles are portalable (the rest sit in func_noportal_volumes)
        assert!(white.0 > white.1 * 2, "{white:?}");
    }
}

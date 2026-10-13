//! MDL/VTX/VVD studio models via `vmdl`, flattened into per-material
//! triangle lists in world axes (metres, model space).

use bevy::math::Vec3;
use shared::{source_dir_to_world, source_to_world};

#[derive(Clone, Debug, Default)]
pub struct ModelMesh {
    /// Index into [`ModelData::materials`] for skin 0; use [`ModelData::material_for`] for other skins.
    pub material_index: i32,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// Source-space normals, for sampling baked ambient light.
    pub source_normals: Vec<Vec3>,
    /// Global VVD vertex index per vertex (for per-vertex prop lighting).
    pub vvd_index: Vec<u32>,
    /// Bind-pose Source-space positions (for re-posing).
    pub source_positions: Vec<Vec3>,
    /// Up to three (bone, weight) pairs per vertex.
    pub bone_weights: Vec<[(u8, f32); 3]>,
    pub indices: Vec<u32>,
    /// For an eyeball mesh (`EyeRefract`/`Eyes` material), the index of its
    /// eyeball in [`ModelData::eyeballs`].
    pub eyeball: Option<usize>,
}

/// One eyeball of a studio model (MDL `eyeball` block). Vectors are in the
/// space of `bone`, Source units and axes.
#[derive(Clone, Copy, Debug, Default)]
pub struct Eyeball {
    /// The bone the eye is attached to (normally the head).
    pub bone: usize,
    /// Eye centre.
    pub org: Vec3,
    /// Tangent of the iris's outward yaw (the QC eyeball "angle"); positive
    /// turns the iris towards the eye's right.
    pub zoffset: f32,
    pub radius: f32,
    /// The eye's rest up and forward directions.
    pub up: Vec3,
    pub forward: Vec3,
    /// Iris projection scale: cornea texture coordinates per unit, i.e. the
    /// cornea UV of a surface point is `0.5 + offset * iris_scale`.
    pub iris_scale: f32,
}

#[derive(Clone, Debug, Default)]
pub struct ModelData {
    pub path: String,
    pub meshes: Vec<ModelMesh>,
    /// Material name per skin-table entry for each skin: `skins[skin][material_index]`.
    pub skins: Vec<Vec<String>>,
    pub surface_prop: String,
    /// Source-space bounds.
    pub mins: Vec3,
    pub maxs: Vec3,
    pub vertex_count: usize,
    /// For each LOD-0 mesh in file order, the global VVD index of every VTX
    /// strip-group vertex in order: the layout of `.vhv` prop lighting.
    pub lod0_vertex_order: Vec<Vec<u32>>,
    /// Convex collision pieces from the `.phy` (world axes, metres, model space).
    pub physics: Vec<Vec<Vec3>>,
    /// Bones, animations and sequences (None for unreadable headers).
    pub skeleton: Option<std::sync::Arc<crate::skeleton::Skeleton>>,
    /// Compiled with `$staticprop` (bones collapsed; never posed).
    pub is_static_prop: bool,
    /// `illumposition`: where the engine samples lighting, Source model space.
    pub illum_position: Vec3,
    /// Eyeballs of every body-part model, in file order.
    pub eyeballs: Vec<Eyeball>,
}

impl ModelData {
    /// Meshes posed at frame 0 of a sequence (`DefaultAnim` name, else
    /// sequence 0), as Portal 2 draws dynamic props. Static-prop models and
    /// models without a skeleton come back unchanged.
    pub fn posed_meshes(&self, sequence: Option<&str>) -> std::borrow::Cow<'_, [ModelMesh]> {
        let Some(sk) = self.skeleton.as_ref().filter(|_| !self.is_static_prop) else {
            return std::borrow::Cow::Borrowed(&self.meshes);
        };
        let seq = sequence.and_then(|n| sk.sequence_index(n)).or(Some(0));
        let pose = sk.pose(seq);
        let meshes = self
            .meshes
            .iter()
            .map(|m| {
                let mut m = m.clone();
                for i in 0..m.source_positions.len() {
                    let (mut p, mut n) = (Vec3::ZERO, Vec3::ZERO);
                    let mut total = 0.0;
                    for &(b, w) in &m.bone_weights[i] {
                        if w <= 0.0 {
                            continue;
                        }
                        let Some(mat) = pose.get(b as usize) else { continue };
                        p += mat.transform_point3(m.source_positions[i]) * w;
                        n += mat.transform_vector3(m.source_normals[i]) * w;
                        total += w;
                    }
                    if total <= 0.0 {
                        continue;
                    }
                    p /= total;
                    let n = n.normalize_or_zero();
                    m.positions[i] = source_to_world(p).to_array();
                    m.normals[i] = source_dir_to_world(n).to_array();
                    m.source_normals[i] = n;
                }
                // Posing can mirror nothing, but re-check winding against normals.
                for t in m.indices.chunks_exact_mut(3) {
                    let p: Vec<Vec3> = t.iter().map(|&i| Vec3::from(m.positions[i as usize])).collect();
                    let n: Vec3 = t.iter().map(|&i| Vec3::from(m.normals[i as usize])).sum();
                    if (p[1] - p[0]).cross(p[2] - p[0]).dot(n) < 0.0 {
                        t.swap(1, 2);
                    }
                }
                m
            })
            .collect();
        std::borrow::Cow::Owned(meshes)
    }

    pub fn material_for(&self, skin: i32, material_index: i32) -> Option<&str> {
        let table = self.skins.get(skin.max(0) as usize).or_else(|| self.skins.first())?;
        table.get(material_index as usize).map(String::as_str)
    }

    /// All material names referenced by any skin.
    pub fn all_materials(&self) -> impl Iterator<Item = &str> {
        self.skins.iter().flatten().map(String::as_str)
    }
}

fn mdl_vec(v: vmdl::Vector) -> Vec3 {
    Vec3::new(v.x, v.y, v.z)
}

/// Load a model. `read` reads game files; `material_exists` resolves the
/// `$cdmaterials` search paths to the first directory holding the VMT.
/// Never panics: a model that trips the parser is reported as an error.
pub fn load_model(
    path: &str,
    read: &dyn Fn(&str) -> Option<Vec<u8>>,
    material_exists: &dyn Fn(&str) -> bool,
) -> anyhow::Result<ModelData> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| load_model_inner(path, read, material_exists))) {
        Ok(r) => r,
        Err(p) => {
            let msg = p
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            Err(anyhow::anyhow!("{path}: parser panicked: {msg}"))
        }
    }
}

fn load_model_inner(
    path: &str,
    read: &dyn Fn(&str) -> Option<Vec<u8>>,
    material_exists: &dyn Fn(&str) -> bool,
) -> anyhow::Result<ModelData> {
    let path = crate::fs::normalize_path(path);
    let stem = path.strip_suffix(".mdl").unwrap_or(&path);
    let mdl_data = read(&path).ok_or_else(|| anyhow::anyhow!("missing {path}"))?;
    let vtx_data = read(&format!("{stem}.dx90.vtx"))
        .or_else(|| read(&format!("{stem}.vtx")))
        .ok_or_else(|| anyhow::anyhow!("missing vtx for {path}"))?;
    let vvd_data = read(&format!("{stem}.vvd")).ok_or_else(|| anyhow::anyhow!("missing vvd for {path}"))?;
    let skeleton = crate::skeleton::Skeleton::parse(std::sync::Arc::new(mdl_data.clone())).map(std::sync::Arc::new);
    let is_static_prop = i32::from_le_bytes(mdl_data.get(152..156).and_then(|b| b.try_into().ok()).unwrap_or([0; 4])) & 0x10 != 0;
    // vmdl's animation decoding panics on some Portal 2 models; animations
    // are read by `skeleton.rs`, so hide them (and sequences) from vmdl.
    let mut mdl_for_vmdl = mdl_data.clone();
    for off in [180usize, 188] {
        if let Some(b) = mdl_for_vmdl.get_mut(off..off + 4) {
            b.copy_from_slice(&0i32.to_le_bytes());
        }
    }
    let mdl = vmdl::Mdl::read(&mdl_for_vmdl).map_err(|e| anyhow::anyhow!("{path}: mdl: {e}"))?;
    let vtx = vmdl::Vtx::read(&vtx_data).map_err(|e| anyhow::anyhow!("{path}: vtx: {e}"))?;
    let vvd = vmdl::Vvd::read(&vvd_data).map_err(|e| anyhow::anyhow!("{path}: vvd: {e}"))?;
    // Walk LOD 0 of the VTX from the raw bytes: vmdl assumes the SDK 2013
    // header sizes, but Portal 2 (MDL v49) strip and strip-group headers are
    // 8 bytes longer, so vmdl misreads every strip after a group's first.
    let mdl_version = i32::from_le_bytes(mdl_data.get(4..8).and_then(|b| b.try_into().ok()).unwrap_or([0; 4]));
    let vtx_lod0 = read_vtx_lod0(&vtx_data, mdl_version >= 49)
        .or_else(|| read_vtx_lod0(&vtx_data, mdl_version < 49))
        .ok_or_else(|| anyhow::anyhow!("{path}: unreadable vtx"))?;
    let raw_eyes = read_eyeballs(&mdl_data);
    let mut lod0_vertex_order = Vec::new();
    // (material index, eyeball, triangle list of global VVD indices) per mesh
    let mut raw_meshes: Vec<(i32, Option<usize>, Vec<u32>)> = Vec::new();
    let mut eyeballs = Vec::new();
    for (bpi, (bp_mdl, bp_vtx)) in mdl.body_parts.iter().zip(&vtx_lod0).enumerate() {
        for (mi, (m_mdl, m_vtx)) in bp_mdl.models.iter().zip(bp_vtx).enumerate() {
            let (eye_base, mesh_eyes) = match raw_eyes.get(bpi).and_then(|b| b.get(mi)) {
                Some((eyes, mesh_eyes)) => {
                    let base = eyeballs.len();
                    eyeballs.extend_from_slice(eyes);
                    (base, mesh_eyes.as_slice())
                }
                None => (0, &[][..]),
            };
            for (k, (mesh_mdl, mesh_vtx)) in m_mdl.meshes.iter().zip(m_vtx).enumerate() {
                let base = (m_mdl.vertex_offset + mesh_mdl.vertex_offset) as u32;
                lod0_vertex_order.push(mesh_vtx.order.iter().map(|&v| base + v as u32).collect());
                let tris = mesh_vtx.tris.iter().map(|&v| base + v as u32).collect();
                let eye = mesh_eyes.get(k).copied().flatten().map(|e| eye_base + e);
                raw_meshes.push((mesh_mdl.material, eye, tris));
            }
        }
    }
    let model = vmdl::Model::from_parts(mdl, vtx, vvd);

    // Material names per skin.
    let dirs: Vec<String> = model
        .texture_directories()
        .iter()
        .map(|d| crate::fs::normalize_path(d))
        .collect();
    let resolve = |name: &str| -> String {
        let name = crate::fs::normalize_path(name);
        for d in &dirs {
            let d = if d.is_empty() || d.ends_with('/') { d.clone() } else { format!("{d}/") };
            let cand = format!("{d}{name}");
            if material_exists(&cand) {
                return cand;
            }
        }
        dirs.first()
            .map(|d| {
                let d = if d.is_empty() || d.ends_with('/') { d.clone() } else { format!("{d}/") };
                format!("{d}{name}")
            })
            .unwrap_or(name)
    };
    let mut skins = Vec::new();
    for table in model.skin_tables() {
        let mut names = Vec::new();
        let mut i = 0;
        while let Some(t) = table.texture(i) {
            names.push(resolve(t));
            i += 1;
        }
        if names.is_empty() {
            names = model.textures().iter().map(|t| resolve(&t.name)).collect();
        }
        skins.push(names);
    }
    if skins.is_empty() {
        skins.push(model.textures().iter().map(|t| resolve(&t.name)).collect());
    }

    let verts = model.vertices();
    let mut meshes: Vec<ModelMesh> = Vec::new();
    for (material_index, eyeball, tris) in raw_meshes {
        let mut out = ModelMesh {
            material_index,
            eyeball,
            ..Default::default()
        };
        let mut remap = std::collections::HashMap::new();
        for t in tris.chunks_exact(3) {
            let mut tri = [0u32; 3];
            let mut ok = true;
            for (k, &vi) in t.iter().enumerate() {
                let Some(v) = verts.get(vi as usize) else {
                    ok = false;
                    break;
                };
                tri[k] = *remap.entry(vi).or_insert_with(|| {
                    let p = mdl_vec(v.position);
                    let n = mdl_vec(v.normal);
                    out.positions.push(source_to_world(p).to_array());
                    out.normals.push(source_dir_to_world(n).normalize_or_zero().to_array());
                    out.uvs.push(v.texture_coordinates);
                    out.source_normals.push(n);
                    out.vvd_index.push(vi);
                    out.source_positions.push(p);
                    let raw: &[u8] = bytemuck::bytes_of(&v.bone_weights);
                    let w = |k: usize| f32::from_le_bytes(raw[k * 4..k * 4 + 4].try_into().unwrap());
                    let count = raw[15].min(3) as usize;
                    let mut bw = [(0u8, 0.0f32); 3];
                    for k in 0..count {
                        bw[k] = (raw[12 + k], w(k));
                    }
                    if count == 0 {
                        bw[0] = (0, 1.0);
                    }
                    out.bone_weights.push(bw);
                    (out.positions.len() - 1) as u32
                });
            }
            if !ok {
                continue;
            }
            // Orient each triangle to agree with its vertex normals.
            let p: Vec<Vec3> = tri.iter().map(|&i| Vec3::from(out.positions[i as usize])).collect();
            let n: Vec3 = tri.iter().map(|&i| Vec3::from(out.normals[i as usize])).sum();
            let face = (p[1] - p[0]).cross(p[2] - p[0]);
            if face.length_squared() < 1e-14 {
                continue;
            }
            if face.dot(n) >= 0.0 {
                out.indices.extend_from_slice(&tri);
            } else {
                out.indices.extend_from_slice(&[tri[0], tri[2], tri[1]]);
            }
        }
        if !out.indices.is_empty() {
            meshes.push(out);
        }
    }
    let (mins, maxs) = model.bounding_box();
    let physics = read(&format!("{stem}.phy"))
        .map(|d| crate::phy::parse_phy(&d))
        .unwrap_or_default();
    Ok(ModelData {
        path,
        meshes,
        skins,
        surface_prop: model.surface_prop().to_ascii_lowercase(),
        mins: mdl_vec(mins),
        maxs: mdl_vec(maxs),
        vertex_count: verts.len(),
        lod0_vertex_order,
        physics,
        skeleton,
        is_static_prop,
        eyeballs,
        illum_position: {
            let f = |o: usize| f32::from_le_bytes(mdl_data[o..o + 4].try_into().unwrap());
            if mdl_data.len() >= 104 { Vec3::new(f(92), f(96), f(100)) } else { Vec3::ZERO }
        },
    })
}

/// One LOD-0 mesh of a VTX file.
#[derive(Clone, Debug, Default)]
struct VtxMesh {
    /// Mesh-relative VVD vertex id of every strip-group vertex, in file order.
    order: Vec<u16>,
    /// Triangle list of mesh-relative VVD vertex ids.
    tris: Vec<u16>,
}

fn rd_i32(d: &[u8], o: usize) -> Option<i32> {
    Some(i32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}
fn rd_u16(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(o..o + 2)?.try_into().ok()?))
}
fn rd_vec3(d: &[u8], o: usize) -> Option<Vec3> {
    let f = |k: usize| rd_i32(d, o + 4 * k).map(|v| f32::from_bits(v as u32));
    Some(Vec3::new(f(0)?, f(1)?, f(2)?))
}

/// `(count, absolute offset)` of an array whose offset is relative to `at`.
fn rd_array(d: &[u8], at: usize, field: usize) -> Option<(usize, usize)> {
    let n = rd_i32(d, at + field)?;
    let off = rd_i32(d, at + field + 4)?;
    (n >= 0).then_some(())?;
    Some((n as usize, (at as i64 + off as i64).try_into().ok()?))
}

/// LOD 0 of a VTX file: `[body part][model][mesh]`. `extended` selects the
/// Portal 2 / MDL v49 layout, whose strip-group (33 bytes) and strip
/// (35 bytes) headers end with a topology index count and offset; SDK 2013
/// (v48) uses 25 and 27. Returns `None` if any strip lies outside its
/// group's index buffer, i.e. the layout is wrong.
fn read_vtx_lod0(d: &[u8], extended: bool) -> Option<Vec<Vec<Vec<VtxMesh>>>> {
    let (sg_size, strip_size) = if extended { (33, 35) } else { (25, 27) };
    let (n_bp, bp_off) = rd_array(d, 0, 28)?;
    let mut out = Vec::with_capacity(n_bp);
    for b in 0..n_bp {
        let bp = bp_off + b * 8;
        let (n_models, m_off) = rd_array(d, bp, 0)?;
        let mut models = Vec::with_capacity(n_models);
        for m in 0..n_models {
            let mp = m_off + m * 8;
            let (n_lods, lod_off) = rd_array(d, mp, 0)?;
            let mut meshes = Vec::new();
            if n_lods > 0 {
                let (n_meshes, mesh_off) = rd_array(d, lod_off, 0)?;
                for k in 0..n_meshes {
                    let mesh = mesh_off + k * 9;
                    let (n_groups, sg_off) = rd_array(d, mesh, 0)?;
                    let mut vm = VtxMesh::default();
                    for g in 0..n_groups {
                        let sg = sg_off + g * sg_size;
                        let (n_verts, v_off) = rd_array(d, sg, 0)?;
                        let (n_idx, i_off) = rd_array(d, sg, 8)?;
                        let (n_strips, s_off) = rd_array(d, sg, 16)?;
                        // Vertex_t: 3 weight indices, bone count, then origMeshVertID.
                        let ids: Vec<u16> =
                            (0..n_verts).map(|v| rd_u16(d, v_off + v * 9 + 4)).collect::<Option<_>>()?;
                        let idx: Vec<u16> = (0..n_idx).map(|i| rd_u16(d, i_off + i * 2)).collect::<Option<_>>()?;
                        let id = |i: usize| idx.get(i).and_then(|&v| ids.get(v as usize)).copied();
                        for s in 0..n_strips {
                            let st = s_off + s * strip_size;
                            let (count, first) = (rd_i32(d, st)?, rd_i32(d, st + 4)?);
                            let flags = *d.get(st + 18)?;
                            let (count, first) = (usize::try_from(count).ok()?, usize::try_from(first).ok()?);
                            if first + count > n_idx || flags & 3 == 0 {
                                return None;
                            }
                            let mut tri = |a: usize, b: usize, c: usize| -> Option<()> {
                                vm.tris.extend_from_slice(&[id(a)?, id(b)?, id(c)?]);
                                Some(())
                            };
                            if flags & 2 != 0 {
                                // Triangle strip.
                                for i in first..(first + count).saturating_sub(2) {
                                    tri(i, i + 1, i + 2)?;
                                }
                            } else {
                                for t in (first..first + count - count % 3).step_by(3) {
                                    tri(t, t + 1, t + 2)?;
                                }
                            }
                        }
                        vm.order.extend(ids);
                    }
                    meshes.push(vm);
                }
            }
            models.push(meshes);
        }
        out.push(models);
    }
    Some(out)
}

/// Per body part, per model: the model's eyeballs and, per mesh, the index
/// (within that model) of the eyeball an eye mesh draws.
fn read_eyeballs(d: &[u8]) -> Vec<Vec<(Vec<Eyeball>, Vec<Option<usize>>)>> {
    // studiohdr_t: numbodyparts/bodypartindex at 232; mstudiobodyparts_t is
    // 16 bytes (nummodels at 4, modelindex at 12); mstudiomodel_t is 148
    // (nummeshes 72, numeyeballs 100); mstudiomesh_t 116 (materialtype 24,
    // materialparam 28); mstudioeyeball_t 172.
    let read = || -> Option<Vec<Vec<(Vec<Eyeball>, Vec<Option<usize>>)>>> {
        let (n_bp, bp_off) = rd_array(d, 0, 232)?;
        let mut out = Vec::new();
        for b in 0..n_bp {
            let bp = bp_off + b * 16;
            let n_models = rd_i32(d, bp + 4)?.max(0) as usize;
            let m_off = (bp as i64 + rd_i32(d, bp + 12)? as i64) as usize;
            let mut models = Vec::new();
            for m in 0..n_models {
                let mp = m_off + m * 148;
                let (n_eyes, e_off) = rd_array(d, mp, 100)?;
                let eyes = (0..n_eyes)
                    .map(|e| {
                        let ep = e_off + e * 172;
                        let f = |o: usize| rd_i32(d, ep + o).map(|v| f32::from_bits(v as u32));
                        Some(Eyeball {
                            bone: rd_i32(d, ep + 4)?.max(0) as usize,
                            org: rd_vec3(d, ep + 8)?,
                            zoffset: f(20)?,
                            radius: f(24)?,
                            up: rd_vec3(d, ep + 28)?,
                            forward: rd_vec3(d, ep + 40)?,
                            iris_scale: f(60)?,
                        })
                    })
                    .collect::<Option<Vec<_>>>()?;
                let (n_meshes, mesh_off) = rd_array(d, mp, 72)?;
                let mesh_eyes = (0..n_meshes)
                    .map(|k| {
                        let mesh = mesh_off + k * 116;
                        let (ty, param) = (rd_i32(d, mesh + 24)?, rd_i32(d, mesh + 28)?);
                        Some((ty == 1 && param >= 0 && (param as usize) < eyes.len()).then_some(param as usize))
                    })
                    .collect::<Option<Vec<_>>>()?;
                models.push((eyes, mesh_eyes));
            }
            out.push(models);
        }
        Some(out)
    };
    read().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_portal2_cube() {
        let cfg = crate::config::GameConfig::from_env();
        if !cfg.root.join("portal2").is_dir() {
            return;
        }
        let fs = crate::fs::GameFs::mount(cfg).unwrap();
        let m = load_model(
            "models/props/metal_box.mdl",
            &|p| fs.read(p),
            &|m| fs.exists(&format!("materials/{m}.vmt")),
        )
        .unwrap();
        assert!(!m.meshes.is_empty());
        let tris: usize = m.meshes.iter().map(|m| m.indices.len() / 3).sum();
        assert!(tris > 50, "{tris}");
        // a cube is ~ 64 units across
        let size = m.maxs - m.mins;
        assert!(size.x > 30.0 && size.x < 100.0, "{size}");
        assert!(m.material_for(0, m.meshes[0].material_index).is_some());
    }
}

#[cfg(test)]
mod pose_tests {
    use super::*;
    use bevy::math::Vec3;

    fn extents(meshes: &[ModelMesh], rot: bevy::math::Quat) -> (Vec3, Vec3) {
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for p in meshes.iter().flat_map(|m| m.positions.iter()) {
            let p = rot * Vec3::from(*p);
            lo = lo.min(p);
            hi = hi.max(p);
        }
        (lo, hi)
    }

    /// sp_a1_intro4's cube dropper hangs from the ceiling with its tube
    /// pointing straight down (world -Y), as in the game.
    #[test]
    fn intro4_dropper_points_down() {
        let cfg = crate::config::GameConfig::from_env();
        if !cfg.root.join("portal2").is_dir() {
            return;
        }
        let fs = crate::fs::GameFs::mount(cfg).unwrap();
        let sp = crate::surfaceprops::SurfaceProps::default();
        let map = crate::map::load_map(&fs, &sp, "sp_a1_intro4").unwrap();
        let e = map
            .entities
            .iter()
            .find(|e| e.targetname.as_deref() == Some("box_dropper-cube_dropper_model"))
            .unwrap();
        let model = &map.models[&e.studio_model().unwrap()];
        let rot = e.world_rotation();
        let posed = model.posed_meshes(e.get("defaultanim"));
        let (lo, hi) = extents(&posed, rot);
        eprintln!("posed {lo} {hi}; ref {:?}", extents(&model.meshes, rot));
        // The tube axis runs from the mount (model origin) through the mass
        // of the model. Posed, that must be straight down; in the bind pose
        // it was horizontal (the bug the user saw).
        let centroid = |ms: &[ModelMesh]| {
            let n = ms.iter().map(|m| m.positions.len()).sum::<usize>() as f32;
            ms.iter().flat_map(|m| m.positions.iter()).map(|p| rot * Vec3::from(*p)).sum::<Vec3>() / n
        };
        let posed_axis = centroid(&posed).normalize();
        let bind_axis = centroid(&model.meshes).normalize();
        eprintln!("tube axis: bind {bind_axis}, posed {posed_axis}");
        assert!(bind_axis.y.abs() < 0.5, "{bind_axis}");
        assert!(posed_axis.dot(Vec3::NEG_Y) > 0.9, "{posed_axis}");
        // and it hangs below its origin
        assert!(lo.y < -1.0, "{lo} {hi}");
    }
}

#[cfg(test)]
mod pose_sweep {
    /// Lists entity models whose default pose differs from the bind pose
    /// (run with --nocapture); fails if posing produces non-finite vertices.
    #[test]
    fn sweep_intro_maps() {
        let cfg = crate::config::GameConfig::from_env();
        if !cfg.root.join("portal2").is_dir() {
            return;
        }
        let fs = crate::fs::GameFs::mount(cfg).unwrap();
        let sp = crate::surfaceprops::SurfaceProps::default();
        let mut seen = std::collections::BTreeMap::new();
        for i in 1..=7 {
            let map = crate::map::load_map(&fs, &sp, &format!("sp_a1_intro{i}")).unwrap();
            for e in &map.entities {
                let Some(path) = crate::map::entity_model(e) else { continue };
                let Some(model) = map.models.get(&path) else { continue };
                let posed = model.posed_meshes(e.get("defaultanim"));
                let mut max = 0f32;
                for (a, b) in posed.iter().zip(&model.meshes) {
                    for (p, q) in a.positions.iter().zip(&b.positions) {
                        assert!(p.iter().all(|v| v.is_finite()), "{path}");
                        max = max.max((bevy::math::Vec3::from(*p) - bevy::math::Vec3::from(*q)).length());
                    }
                }
                seen.entry((e.classname.clone(), path.clone(), e.get("defaultanim").unwrap_or("").to_string()))
                    .or_insert(max);
            }
        }
        for ((c, p, a), m) in &seen {
            if *m > 0.05 {
                eprintln!("posed {m:6.2}m  {c:28} {p} anim={a}");
            }
        }
    }
}

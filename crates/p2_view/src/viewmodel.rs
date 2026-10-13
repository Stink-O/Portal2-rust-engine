//! The first-person portal gun: Portal 2's `models/weapons/v_portalgun.mdl`
//! (read from the install), GPU-skinned, playing its own draw / idle /
//! fire / fizzle sequences. A dedicated camera draws it at `cl_viewmodelfov`
//! 50 so it never clips into walls.
//!
//! Lifted from the old route-A `crates/portal/src/viewmodel.rs` and adapted to
//! the `p2_core` contract. Single-player skin only (skin 0).
use std::sync::Arc;

use bevy::asset::{embedded_asset, RenderAssetUsages};
use bevy::camera::visibility::{NoFrustumCulling, RenderLayers};
use bevy::camera::Hdr;
use bevy::math::{Affine3A, Mat3, Mat4};
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;
use std::collections::HashMap;

use p2_assets::anim::Animator;
use p2_assets::source_assets;
use p2_assets::GameFsRes;
use p2_core::{
    Crouch, GunFired, InterpolatedFeet, PortalShotFailed, Player, ViewAngles, ViewmodelAttachments,
    LAYER_VIEWMODEL, U,
};

use crate::{eye_transform, source_fov_to_vertical, ViewConfig};

/// Default `cl_viewmodelfov` (50, horizontal at 4:3; same convention as the world FOV).
pub const VIEWMODEL_FOV_DEG: f32 = 50.0;
/// Near plane of the viewmodel camera, metres (the old sandbox's value).
pub const VIEWMODEL_NEAR: f32 = 0.01;
/// Default game path of the portal gun model.
pub const VIEWMODEL_PATH: &str = "models/weapons/v_portalgun.mdl";

/// Marks the camera that draws the viewmodel. It has the same transform as
/// the [`MainCamera`] and only renders `LAYER_VIEWMODEL`.
#[derive(Component)]
pub struct ViewModelCamera;

/// Animation and lag state of the gun. Sits on the viewmodel root, which is
/// a child of the [`ViewModelCamera`]. Query it to read the current sequence.
#[derive(Component)]
pub struct ViewmodelRig {
    animator: Arc<Animator>,
    joints: Vec<Entity>,
    seq: usize,
    time: f32,
    /// Sequence to return to when a one-shot finishes.
    idle: usize,
    /// Source `lastFacing` (world space), for CalcViewModelLag.
    last_facing: Option<Vec3>,
}

impl ViewmodelRig {
    /// Name of the sequence now playing, e.g. `"idle"`.
    pub fn sequence(&self) -> &str {
        self.animator.sequences.get(self.seq).map_or("", |s| s.label.as_str())
    }
    /// Seconds into the current sequence.
    pub fn sequence_time(&self) -> f32 {
        self.time
    }
    /// Plays a sequence from its start, if the model has it. Returns false otherwise.
    pub fn play(&mut self, name: &str) -> bool {
        match self.animator.find(name) {
            Some(seq) => {
                self.seq = seq;
                self.time = 0.0;
                true
            }
            None => false,
        }
    }
}

/// Source view space (x forward, y left, z up, inches) to Bevy camera space, scaled to metres.
/// Private: the gun's own coordinate convention.
fn source_view_to_camera() -> Transform {
    let m = Mat3::from_cols(Vec3::NEG_Z, Vec3::NEG_X, Vec3::Y);
    Transform::from_rotation(Quat::from_mat3(&m)).with_scale(Vec3::splat(U))
}

/// Lighting for the viewmodel: a Source ambient cube (sides +X, -X, +Y, -Y,
/// +Z, -Z in Source axes, linear). The map is not loaded here, so this is a
/// neutral, slightly top-lit default; a host with its own lighting can overwrite it.
#[derive(Resource, Clone, Debug)]
pub struct ViewModelLighting {
    /// Ambient light per Source side, linear RGB.
    pub cube: [Vec3; 6],
}

impl Default for ViewModelLighting {
    fn default() -> Self {
        let c = |v: f32| Vec3::splat(v);
        Self { cube: [c(0.55), c(0.5), c(0.55), c(0.5), c(0.75), c(0.3)] }
    }
}

/// Material extension that adds the ambient-cube lighting to `StandardMaterial`.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct AmbientCubeExt {
    /// The six ambient-cube sides, uploaded as a uniform.
    #[uniform(100)]
    pub sides: AmbientCubeSides,
}

/// GPU layout of the ambient cube (six `vec4` sides, alpha unused).
#[derive(Clone, Copy, Debug, Default, Reflect, ShaderType)]
pub struct AmbientCubeSides {
    /// Sides in Source order +X, -X, +Y, -Y, +Z, -Z.
    pub sides: [Vec4; 6],
}

impl MaterialExtension for AmbientCubeExt {
    fn fragment_shader() -> ShaderRef {
        "embedded://p2_view/viewmodel.wgsl".into()
    }
}

/// The viewmodel's material: Bevy's `StandardMaterial` plus the ambient cube.
pub type ViewModelMaterial = ExtendedMaterial<StandardMaterial, AmbientCubeExt>;

pub(crate) fn build(app: &mut App) {
    embedded_asset!(app, "viewmodel.wgsl");
    app.add_plugins(MaterialPlugin::<ViewModelMaterial>::default())
        .init_resource::<ViewModelLighting>()
        .add_systems(Update, update_lighting);
}

/// Spawns the viewmodel camera and the gun once, when the player appears.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_viewmodel_camera_and_gun(
    mut commands: Commands,
    cfg: Res<ViewConfig>,
    added: Query<(), Added<Player>>,
    existing: Query<(), With<ViewModelCamera>>,
    fs: Option<Res<GameFsRes>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ViewModelMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
) {
    if !cfg.viewmodel || added.is_empty() || !existing.is_empty() {
        return;
    }
    // The gun is Portal 2's own model: without the game (no
    // `p2_assets::AssetsPlugin`) there is no viewmodel at all.
    let Some(fs) = fs.map(|f| f.0.clone()) else { return };
    let layers = RenderLayers::layer(LAYER_VIEWMODEL);
    let cam = commands
        .spawn((
            Name::new("Viewmodel camera"),
            ViewModelCamera,
            Camera3d::default(),
            Hdr,
            // Same linear output as the main camera (see lib.rs).
            bevy::core_pipeline::tonemapping::Tonemapping::None,
            Camera { order: 1, clear_color: ClearColorConfig::None, ..default() },
            Projection::Perspective(PerspectiveProjection {
                fov: source_fov_to_vertical(cfg.viewmodel_fov_deg),
                near: VIEWMODEL_NEAR,
                ..default()
            }),
            layers.clone(),
            Transform::default(),
        ))
        .id();

    let read = |p: &str| fs.read(p);
    let exists = |m: &str| fs.exists(&format!("materials/{m}.vmt"));
    let model = match source_assets::models::load_model(&cfg.viewmodel_model, &read, &exists) {
        Ok(m) => m,
        Err(e) => {
            warn!("p2_view: viewmodel: {e}");
            return;
        }
    };
    let Some(animator) = fs.read(&cfg.viewmodel_model).and_then(|d| Animator::parse(Arc::new(d))) else {
        warn!("p2_view: viewmodel: no skeleton");
        return;
    };
    let animator = Arc::new(animator);

    // Skeleton joints under a root that converts Source view space.
    let root = commands
        .spawn((
            Name::new("Viewmodel"),
            source_view_to_camera(),
            Visibility::default(),
            layers.clone(),
            ChildOf(cam),
        ))
        .id();
    let bones = &animator.skeleton.bones;
    let pose = animator.sample(animator.find("draw").unwrap_or(0), 0.0);
    let mut joints: Vec<Entity> = Vec::with_capacity(bones.len());
    for (i, b) in bones.iter().enumerate() {
        let parent = if b.parent >= 0 && (b.parent as usize) < i { joints[b.parent as usize] } else { root };
        let (p, q) = pose[i];
        let j = commands
            .spawn((Name::new(b.name.clone()), Transform::from_translation(p).with_rotation(q), ChildOf(parent)))
            .id();
        joints.push(j);
    }
    let inverse: Vec<Mat4> = bones.iter().map(|b| Mat4::from(b.pose_to_bone)).collect();
    let inverse = bindposes.add(SkinnedMeshInverseBindposes::from(inverse));

    let mut mat_cache: HashMap<String, Handle<ViewModelMaterial>> = HashMap::new();
    for m in &model.meshes {
        let name = model.material_for(0, m.material_index).unwrap_or("").to_string();
        let material = mat_cache
            .entry(name.clone())
            .or_insert_with(|| {
                materials.add(ExtendedMaterial {
                    base: viewmodel_material(&name, &fs, &mut images),
                    extension: AmbientCubeExt::default(),
                })
            })
            .clone();
        let n = m.source_positions.len();
        let joints_idx: Vec<[u16; 4]> = (0..n)
            .map(|i| {
                let w = m.bone_weights[i];
                [w[0].0 as u16, w[1].0 as u16, w[2].0 as u16, 0]
            })
            .collect();
        let weights: Vec<[f32; 4]> = (0..n)
            .map(|i| {
                let w = m.bone_weights[i];
                let s = (w[0].1 + w[1].1 + w[2].1).max(1e-6);
                [w[0].1 / s, w[1].1 / s, w[2].1 / s, 0.0]
            })
            .collect();
        let positions: Vec<[f32; 3]> = m.source_positions.iter().map(|p| p.to_array()).collect();
        let normals: Vec<[f32; 3]> = m.source_normals.iter().map(|p| p.to_array()).collect();
        // Source triangles are clockwise; the loader re-winds them against world
        // normals and the Source->camera root is a proper rotation, so the
        // indices are used as they are.
        let mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, m.uvs.clone())
            .with_inserted_attribute(Mesh::ATTRIBUTE_JOINT_INDEX, VertexAttributeValues::Uint16x4(joints_idx))
            .with_inserted_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, weights)
            .with_inserted_indices(Indices::U32(m.indices.clone()));
        commands.spawn((
            Name::new(format!("Viewmodel mesh {name}")),
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(material),
            NoFrustumCulling,
            Transform::IDENTITY,
            layers.clone(),
            bevy::light::NotShadowCaster,
            SkinnedMesh { inverse_bindposes: inverse.clone(), joints: joints.clone() },
            ChildOf(root),
        ));
    }

    let draw = animator.find("draw").unwrap_or(0);
    let idle = animator.find("idle").unwrap_or(0);
    commands.entity(root).insert(ViewmodelRig {
        animator,
        joints,
        seq: draw,
        time: 0.0,
        idle,
        last_facing: None,
    });
}

fn viewmodel_material(name: &str, fs: &source_assets::GameFs, images: &mut Assets<Image>) -> StandardMaterial {
    let read = |p: &str| fs.read(p);
    let (_, kv) = source_assets::vmt::load_vmt(&read, name).unwrap_or_default();
    let mut tex = |key: &str, srgb: bool| {
        let t = kv.get(key)?;
        let data = fs.read(&format!("materials/{}.vtf", source_assets::vmt::canonical_name(t)))?;
        let img = source_assets::vtf::VtfTexture::parse(&data).ok()?.into_image(false, srgb);
        Some(images.add(img))
    };
    let translucent = kv.get_bool("$translucent");
    let additive = kv.get_bool("$additive");
    StandardMaterial {
        base_color_texture: tex("$basetexture", true),
        normal_map_texture: tex("$bumpmap", false),
        // Source normal maps are DirectX-style (green down).
        flip_normal_map_y: true,
        perceptual_roughness: if kv.get_bool("$phong") { 0.35 } else { 0.6 },
        reflectance: 0.3,
        alpha_mode: if additive {
            AlphaMode::Add
        } else if translucent {
            AlphaMode::Blend
        } else {
            AlphaMode::Opaque
        },
        cull_mode: if kv.get_bool("$nocull") { None } else { Some(bevy::render::render_resource::Face::Back) },
        ..default()
    }
}

fn update_lighting(lighting: Res<ViewModelLighting>, mut materials: ResMut<Assets<ViewModelMaterial>>) {
    let sides = AmbientCubeSides { sides: lighting.cube.map(|c| c.extend(1.0)) };
    let ids: Vec<_> = materials.ids().collect();
    for id in ids {
        if materials.get(id).is_some_and(|m| m.extension.sides.sides != sides.sides) {
            if let Some(mut m) = materials.get_mut(id) {
                m.extension.sides = sides;
            }
        }
    }
}

/// Model-space bone transforms of a pose.
fn model_transforms(animator: &Animator, pose: &[(Vec3, Quat)]) -> Vec<Affine3A> {
    let mut out: Vec<Affine3A> = Vec::with_capacity(pose.len());
    for (i, b) in animator.skeleton.bones.iter().enumerate() {
        let m = Affine3A::from_rotation_translation(pose[i].1, pose[i].0);
        let m = if b.parent >= 0 && (b.parent as usize) < i { out[b.parent as usize] * m } else { m };
        out.push(m);
    }
    out
}

/// Drives the gun for the local player: the Source viewmodel lag, the
/// sequences (draw, idle, fire, fizzle), the joint poses, and
/// [`ViewmodelAttachments`] on the player. Runs in `P2Systems::View` after
/// the camera pose.
#[allow(clippy::too_many_arguments)]
pub(crate) fn update_viewmodel(
    time: Res<Time>,
    fixed: Res<Time<Fixed>>,
    mut players: Query<
        (Entity, &InterpolatedFeet, &ViewAngles, &Crouch, &mut ViewmodelAttachments),
        With<Player>,
    >,
    mut rigs: Query<(&mut ViewmodelRig, &mut Transform)>,
    mut joints: Query<&mut Transform, Without<ViewmodelRig>>,
    mut fired: MessageReader<GunFired>,
    mut failed: MessageReader<PortalShotFailed>,
    mut teleported: MessageReader<p2_core::PortalTeleported>,
) {
    let Some((player, feet, angles, crouch, mut attach)) = players.iter_mut().next() else {
        fired.clear();
        failed.clear();
        teleported.clear();
        return;
    };
    let Ok((mut rig, mut root)) = rigs.single_mut() else {
        fired.clear();
        failed.clear();
        teleported.clear();
        return;
    };

    // Reactions. A failed shot also sends GunFired, so the fizzle read last wins.
    for f in fired.read() {
        if f.gun == player {
            rig.play("fire1");
        }
    }
    for f in failed.read() {
        if f.gun == player && !rig.play("fizzle") {
            rig.play("dryfire");
        }
    }

    // A portal transit turns the view instantly. Carry the remembered facing
    // through the same pair, as Portal 2 does, or the lag reads the turn as a
    // huge flick and throws the gun off screen for a few frames.
    for t in teleported.read() {
        if t.entity == player {
            rig.last_facing = rig.last_facing.map(|f| t.pair.map_dir(f).normalize_or(f));
        }
    }

    // Source CalcViewModelLag: the gun trails fast turns, world-space offset.
    let dt = time.delta_secs();
    let forward = angles.forward();
    let mut lag = Vec3::ZERO;
    if dt > 0.0 {
        let last = rig.last_facing.unwrap_or(forward);
        let diff = forward - last;
        let len = diff.length();
        let speed = if len > 1.5 { len * (5.0 / 1.5) } else { 5.0 };
        rig.last_facing = Some((last + diff * speed * dt).normalize_or(forward));
        lag = -diff * 5.0 * U;
    }
    let rot = angles.rotation();
    root.translation = rot.inverse() * lag;
    root.rotation = source_view_to_camera().rotation;
    root.scale = Vec3::splat(U);

    // Sequence playback: one-shots return to idle when they finish.
    rig.time += dt;
    let looping = rig.animator.sequences.get(rig.seq).is_some_and(|s| s.looping);
    if !looping && rig.time >= rig.animator.duration(rig.seq) {
        rig.seq = rig.idle;
        rig.time = 0.0;
    }
    let pose = rig.animator.sample(rig.seq, rig.time);
    for (i, &j) in rig.joints.iter().enumerate() {
        if let Ok(mut t) = joints.get_mut(j) {
            let (p, q) = pose[i];
            t.translation = p;
            t.rotation = q;
        }
    }

    // Attachments in world space: the eye, then the root (lag included), then the bone pose.
    let eye = eye_transform(feet.at(fixed.overstep_fraction()), crouch.eye_height, angles);
    let world_from_root = Affine3A::from(eye.compute_affine()) * Affine3A::from(root.compute_affine());
    let model = model_transforms(&rig.animator, &pose);
    let attach_world = |name: &str| -> Option<Transform> {
        let (p, r) = rig.animator.attachment(name, &model)?;
        let m = world_from_root;
        let pos = m.transform_point3(p);
        let basis = Mat3::from(m.matrix3) * r;
        let basis = Mat3::from_cols(basis.x_axis.normalize(), basis.y_axis.normalize(), basis.z_axis.normalize());
        Some(Transform::from_translation(pos).with_rotation(Quat::from_mat3(&basis)))
    };
    attach.muzzle = attach_world("muzzle");
    attach.top_light = attach_world("Body_light");
    attach.beam_point1 = attach_world("Beam_point1");
    attach.beam_point5 = attach_world("Beam_point5");

}

#[cfg(test)]
mod tests {
    use super::*;

    fn animator() -> Option<Arc<Animator>> {
        let fs = p2_assets::find_portal2().ok()?;
        Some(Arc::new(Animator::parse(Arc::new(fs.read(VIEWMODEL_PATH)?))?))
    }

    fn posed(a: &Animator, seq: &str, t: f32) -> Vec<Affine3A> {
        model_transforms(a, &a.sample(a.find(seq).unwrap(), t))
    }

    /// Camera-space direction of a Source viewmodel-space vector.
    fn to_camera(v: Vec3) -> Vec3 {
        source_view_to_camera().rotation * v
    }

    #[test]
    fn source_view_axes_map_to_camera() {
        // Source viewmodel space: +X forward, +Y left, +Z up.
        assert!(to_camera(Vec3::X).distance(Vec3::NEG_Z) < 1e-5);
        assert!(to_camera(Vec3::Y).distance(Vec3::NEG_X) < 1e-5);
        assert!(to_camera(Vec3::Z).distance(Vec3::Y) < 1e-5);
    }

    #[test]
    fn barrel_points_along_camera_forward() {
        let Some(a) = animator() else { return };
        for seq in ["idle", "fire1"] {
            let m = posed(&a, seq, 0.5);
            let muzzle = a.attachment("muzzle", &m).unwrap().0;
            let back = a.attachment("Body_light", &m).unwrap().0;
            let barrel = to_camera(muzzle - back).normalize();
            let angle = barrel.angle_between(Vec3::NEG_Z).to_degrees();
            assert!(angle < 10.0, "{seq}: barrel {angle} deg off camera forward");
            // Lower right of the view, in front of the eye.
            let tip = to_camera(muzzle);
            assert!(tip.z < 0.0 && tip.x > 0.0 && tip.y < 0.0, "{seq}: muzzle at {tip}");
            // The three prongs point forward (toward the crosshair side).
            for arm in 1..=3 {
                let b = a.attachment(&format!("Arm{arm}_attach1"), &m).unwrap().0;
                let t = a.attachment(&format!("Arm{arm}_attach3"), &m).unwrap().0;
                let d = to_camera(t - b).normalize();
                assert!(d.angle_between(Vec3::NEG_Z).to_degrees() < 25.0, "{seq}: prong {arm} {d}");
            }
        }
    }

    #[test]
    fn gun_is_portal_gun_sized() {
        // Muzzle ~32 u ahead of the eye.
        let Some(a) = animator() else { return };
        let m = posed(&a, "idle", 0.0);
        let muzzle = a.attachment("muzzle", &m).unwrap().0;
        assert!((25.0..40.0).contains(&muzzle.x), "{muzzle}");
    }
}

//! Chell's skinned body and the world gun in her hand, from the Portal 2
//! install.
//!
//! * Mesh and skin: `models/player/chell/player.mdl`. That file has no
//!   animations of its own; they come from the included
//!   `models/player_animations.mdl`, whose bones are matched to Chell's by name.
//! * Gun: `models/weapons/w_portalgun.mdl`, held at the `weapon_bone` bone.
//!
//! Skinning uses Bevy's `SkinnedMesh` over joint entities, the same pattern as
//! the first-person viewmodel in the old `portal` crate. The joints sit under
//! a node that maps Source model space (x forward, y left, z up, inches) to
//! body space (-Z forward, metres), so joint poses stay in Source units.

use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::{NoFrustumCulling, RenderLayers};
use bevy::light::NotShadowCaster;
use bevy::math::{Affine3A, Mat3, Mat4};
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::pbr::StandardMaterial;
use bevy::prelude::*;
use p2_assets::anim::Animator;
use p2_assets::GameFsRes;
use p2_core::{CharacterVelocity, Crouch, Grounded, Player, ViewAngles, LAYER_PLAYER_BODY, U};
use source_assets::GameFs;

use crate::ghost::{ghost_material, GhostMaterial};
use crate::{BodyConfig, BodyModel, PlayerBody};

/// Sequence names in `models/player_animations.mdl` (Portal 2's player
/// animations; Chell holds the portal gun, so the `portalgun_*` set is used).
/// The base sequences hold the gun in both hands; each auto-layers the aim
/// matrix (`portalgun_*_aimmatrix`, additive, upper body) that turns the
/// torso and arms towards the view.
mod names {
    pub const IDLE: &[&str] = &["portalgun_standing_idle", "nogun_standing_idle"];
    pub const RUN: &[&str] = &["portalgun_run", "nogun_run"];
    pub const CROUCH_IDLE: &[&str] = &["portalgun_crouch_idle", "nogun_crouch_idle"];
    pub const CROUCH_WALK: &[&str] = &["portalgun_crouchWalk", "nogun_crouchWalk"];
    pub const JUMP: &[&str] = &["portalgun_standing_jump", "nogun_standing_jump"];
    pub const FLOAT: &[&str] = &["portalgun_jump_float", "nogun_airwalk"];
    pub const AIRWALK: &[&str] = &["portalgun_airwalk", "nogun_airwalk"];
    pub const LAND: &[&str] = &["portalgun_jump_land", "nogun_jump_land"];
    pub const FIRE: &[&str] = &["portalgun_standing_fire"];
    pub const FIRE_CROUCH: &[&str] = &["portalgun_crouch_fire", "portalgun_standing_fire"];
    /// The bone the world gun is bone-merged onto.
    pub const GUN_BONE: &str = "weapon_bone";
}

/// Seconds to cross-fade between two sequences.
const BLEND_TIME: f32 = 0.2;
/// Below this horizontal speed (m/s) the body is idle.
const IDLE_SPEED: f32 = 0.5;

/// Model space to body space. Animated bones are in Source model space:
/// x forward, y left, z up (the reference pose of these models is stored
/// Y-up, but every animation puts the root back into Z-up, as Source draws
/// it). Body space is Bevy's: forward -Z, left -X, up +Y.
fn model_to_body() -> Quat {
    Quat::from_mat3(&Mat3::from_cols(Vec3::NEG_Z, Vec3::NEG_X, Vec3::Y))
}

/// One mesh of a rig: its Bevy mesh and the base material from its VMT.
pub(crate) struct RigMesh {
    mesh: Handle<Mesh>,
    material: StandardMaterial,
}

/// A skinned model, loaded once and spawned per body.
pub(crate) struct RigAsset {
    skeleton: Arc<Animator>,
    meshes: Vec<RigMesh>,
    bindposes: Handle<SkinnedMeshInverseBindposes>,
}

/// Chell's body, its animations and the gun, if the game was found.
pub(crate) struct ChellData {
    body: RigAsset,
    /// The animation source (`player_animations.mdl`).
    anims: Arc<Animator>,
    /// For each of Chell's bones, the index of the same-named bone in `anims`.
    bone_map: Vec<Option<usize>>,
    gun: Option<RigAsset>,
    seqs: ChellSeqs,
    gun_bone: Option<usize>,
}

/// Sequence indices in the animation source.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ChellSeqs {
    idle: usize,
    run: usize,
    crouch_idle: usize,
    crouch_walk: usize,
    jump: usize,
    float: usize,
    airwalk: usize,
    land: Option<usize>,
    fire: Option<usize>,
    fire_crouch: Option<usize>,
}

/// Pose parameter indices. Portal 2's player model: move_x (+ forward) and
/// move_y (+ to the right) in -1..1 of the run's ground speed; body_yaw +-45
/// (+ aims to the right); body_pitch -45..90 (+ aims up). The signs were read
/// off the animation data: the aim matrix's `+45 yaw` column turns the gun to
/// the model's right, its `+45 pitch` row raises it, and the run grid's
/// `move_y = -1` column holds the animations that move to the model's left.
#[derive(Clone, Copy, Debug, Default)]
struct PoseIdx {
    move_x: Option<usize>,
    move_y: Option<usize>,
    body_yaw: Option<usize>,
    body_pitch: Option<usize>,
}

/// Resource: `Some` when Chell loaded from the Portal 2 install.
#[derive(Resource)]
pub(crate) struct ChellAssets(pub Option<ChellData>);

/// The running pose of one body, on its root entity.
#[derive(Component)]
pub struct ChellRig {
    skeleton: Arc<Animator>,
    anims: Arc<Animator>,
    bone_map: Vec<Option<usize>>,
    joints: Vec<Entity>,
    /// The gun node, the player bone it is merged onto, and the inverse of
    /// the gun's own root-bone rest transform (Source bone-merge).
    gun: Option<(Entity, usize, Affine3A)>,
    seqs: ChellSeqs,
    pose_idx: PoseIdx,
    state: AnimState,
}

/// Portal 2's player animation state (the multiplayer anim state the
/// `ACT_MP_*` activities belong to), reduced to what a single player needs.
#[derive(Clone, Debug)]
struct AnimState {
    /// Base sequence and its cycle (0..1).
    seq: usize,
    cycle: f32,
    /// The sequence being faded out, its cycle, and the fade progress (0..1).
    prev: Option<(usize, f32)>,
    fade: f32,
    /// World yaw the legs face; the torso turns to the eye through body_yaw.
    feet_yaw: f32,
    grounded: bool,
    /// In the air after a jump (jump start, then the float loop).
    jumped: bool,
    /// One-shot additive gestures: (sequence, cycle).
    gestures: Vec<(usize, f32)>,
}

fn find_any(anim: &Animator, names: &[&str]) -> usize {
    names.iter().find_map(|n| anim.find(n)).unwrap_or(0)
}

/// Loads Chell and the gun once, at startup, from the Portal 2 install
/// (`p2_assets::AssetsPlugin`). Without them no body is drawn.
pub(crate) fn load_chell(
    mut commands: Commands,
    cfg: Res<BodyConfig>,
    game: Option<Res<GameFsRes>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
) {
    let Some(game) = game.filter(|_| matches!(cfg.model, BodyModel::Chell)) else { return };
    let data = load_body(&game.0, &cfg, &mut meshes, &mut images, &mut bindposes);
    if data.is_none() {
        error!("p2_body: could not load {} from the Portal 2 install; no body is drawn", cfg.chell_model);
    }
    commands.insert_resource(ChellAssets(data));
}

fn load_body(
    fs: &GameFs,
    cfg: &BodyConfig,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    bindposes: &mut Assets<SkinnedMeshInverseBindposes>,
) -> Option<ChellData> {
    let body = load_rig(fs, &cfg.chell_model, meshes, images, bindposes)?;
    let mut anims = Animator::parse(Arc::new(fs.read(&cfg.chell_animations)?))?;
    // Almost all of the player's animations live in the external `.ani` file;
    // without it every sequence samples as the reference pose.
    if let Some(ani) = anims.anim_block_file.clone() {
        match fs.read(&ani) {
            Some(bytes) => anims.set_anim_blocks(Arc::new(bytes)),
            None => warn!("p2_body: {ani} not found; Chell will stand in her reference pose"),
        }
    }
    let anims = Arc::new(anims);
    let bone_map = body
        .skeleton
        .skeleton
        .bones
        .iter()
        .map(|b| anims.skeleton.bones.iter().position(|a| a.name.eq_ignore_ascii_case(&b.name)))
        .collect();
    let f = |names: &[&str]| find_any(&anims, names);
    let o = |names: &[&str]| names.iter().find_map(|n| anims.find(n));
    let seqs = ChellSeqs {
        idle: f(names::IDLE),
        run: f(names::RUN),
        crouch_idle: f(names::CROUCH_IDLE),
        crouch_walk: f(names::CROUCH_WALK),
        jump: f(names::JUMP),
        float: f(names::FLOAT),
        airwalk: f(names::AIRWALK),
        land: o(names::LAND),
        fire: o(names::FIRE),
        fire_crouch: o(names::FIRE_CROUCH),
    };
    let gun = load_rig(fs, &cfg.gun_model, meshes, images, bindposes);
    let gun_bone = body.skeleton.skeleton.bones.iter().position(|b| b.name == names::GUN_BONE);
    Some(ChellData { body, anims, bone_map, gun, seqs, gun_bone })
}

fn load_rig(
    fs: &GameFs,
    path: &str,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    bindposes: &mut Assets<SkinnedMeshInverseBindposes>,
) -> Option<RigAsset> {
    let read = |p: &str| fs.read(p);
    let exists = |m: &str| fs.exists(&format!("materials/{m}.vmt"));
    let model = match source_assets::models::load_model(path, &read, &exists) {
        Ok(m) => m,
        Err(e) => {
            warn!("p2_body: {e}");
            return None;
        }
    };
    let skeleton = Animator::parse(Arc::new(fs.read(path)?))?;
    let inverse: Vec<Mat4> = skeleton.skeleton.bones.iter().map(|b| Mat4::from(b.pose_to_bone)).collect();
    let bindposes = bindposes.add(SkinnedMeshInverseBindposes::from(inverse));
    let rig_meshes = model
        .meshes
        .iter()
        .map(|m| {
            let name = model.material_for(0, m.material_index).unwrap_or("").to_string();
            let eye = m.eyeball.and_then(|i| model.eyeballs.get(i));
            let uvs = match eye {
                Some(e) => eye_uvs(m, e, &skeleton),
                None => m.uvs.clone(),
            };
            let mut material = vmt_material(&name, fs, images);
            if eye.is_some() {
                // Wet cornea.
                material.perceptual_roughness = 0.12;
                material.reflectance = 0.5;
            }
            RigMesh { mesh: meshes.add(rig_mesh(m, uvs)), material }
        })
        .collect();
    Some(RigAsset { skeleton: Arc::new(skeleton), meshes: rig_meshes, bindposes })
}

/// Texture coordinates of an eyeball mesh: the iris texture projected onto
/// the eye along its forward axis (as Source's eye shaders do), for eyes
/// looking straight ahead. `u` runs along the eye's right, `v` down its up,
/// both `0.5 + offset * iris_scale` from the eye centre; the iris yaw
/// (`zoffset`, a tangent) turns the projection axis outwards.
fn eye_uvs(m: &source_assets::models::ModelMesh, e: &source_assets::models::Eyeball, skeleton: &Animator) -> Vec<[f32; 2]> {
    let bone_to_model = skeleton.skeleton.bones.get(e.bone).map_or(Affine3A::IDENTITY, |b| b.pose_to_bone.inverse());
    let org = bone_to_model.transform_point3(e.org);
    let up = bone_to_model.transform_vector3(e.up).normalize_or_zero();
    let fwd = bone_to_model.transform_vector3(e.forward).normalize_or_zero();
    let right = fwd.cross(up).normalize_or_zero();
    let fwd = (fwd + right * e.zoffset).normalize_or_zero();
    let right = fwd.cross(up).normalize_or_zero();
    let up = right.cross(fwd).normalize_or_zero();
    m.source_positions
        .iter()
        .map(|&p| {
            let d = p - org;
            [0.5 + d.dot(right) * e.iris_scale, 0.5 - d.dot(up) * e.iris_scale]
        })
        .collect()
}

/// A skinned Bevy mesh from the MDL mesh's bind pose (Source units).
fn rig_mesh(m: &source_assets::models::ModelMesh, uvs: Vec<[f32; 2]>) -> Mesh {
    let n = m.source_positions.len();
    let joints: Vec<[u16; 4]> = (0..n)
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
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, m.source_positions.iter().map(|p| p.to_array()).collect::<Vec<_>>())
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, m.source_normals.iter().map(|p| p.to_array()).collect::<Vec<_>>())
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        .with_inserted_attribute(Mesh::ATTRIBUTE_JOINT_INDEX, VertexAttributeValues::Uint16x4(joints))
        .with_inserted_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, weights)
        .with_inserted_indices(Indices::U32(m.indices.clone()))
}

/// The base material of a VMT (basetexture, bumpmap, translucency, culling).
fn vmt_material(name: &str, fs: &GameFs, images: &mut Assets<Image>) -> StandardMaterial {
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
        // Eyes (`EyeRefract`) have no base texture: their `$iris` texture is
        // projected (see `eye_uvs`).
        base_color_texture: tex("$basetexture", true).or_else(|| tex("$iris", true)),
        normal_map_texture: tex("$bumpmap", false),
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

/// Spawns Chell under `root` and attaches the [`ChellRig`] to it.
pub(crate) fn spawn(commands: &mut Commands, root: Entity, data: &ChellData, ghosts: &mut Assets<GhostMaterial>) {
    let model = commands
        .spawn((
            Name::new("chell model"),
            Transform::from_rotation(model_to_body()).with_scale(Vec3::splat(U)),
            Visibility::default(),
            ChildOf(root),
        ))
        .id();
    let joints = spawn_rig(commands, model, &data.body, ghosts);
    let gun = match (&data.gun, data.gun_bone) {
        (Some(g), Some(bone)) => {
            let node = commands
                .spawn((Name::new("world gun"), Transform::default(), Visibility::default(), ChildOf(model)))
                .id();
            spawn_rig(commands, node, g, ghosts);
            // Bone-merge: the gun's own `weapon_bone` must land on Chell's, so
            // undo the gun root bone's rest transform (a 90 degree turn).
            let gun_root = g.skeleton.skeleton.bones.iter().position(|b| b.name == names::GUN_BONE).unwrap_or(0);
            let rest = bone_to_model(&g.skeleton, &rest_pose(&g.skeleton));
            let inv = rest.get(gun_root).map(|m| m.inverse()).unwrap_or(Affine3A::IDENTITY);
            Some((node, bone, inv))
        }
        _ => None,
    };
    let a = &data.anims;
    let pose_idx = PoseIdx {
        move_x: a.pose_param("move_x"),
        move_y: a.pose_param("move_y"),
        body_yaw: a.pose_param("body_yaw"),
        body_pitch: a.pose_param("body_pitch"),
    };
    commands.entity(root).insert(ChellRig {
        skeleton: data.body.skeleton.clone(),
        anims: data.anims.clone(),
        bone_map: data.bone_map.clone(),
        joints,
        gun,
        seqs: data.seqs,
        pose_idx,
        state: AnimState { seq: data.seqs.idle, cycle: 0.0, prev: None, fade: 1.0, feet_yaw: 0.0, grounded: true, jumped: false, gestures: Vec::new() },
    });
}

/// Spawns joints and skinned meshes for a rig under `parent`. Returns the joints.
fn spawn_rig(commands: &mut Commands, parent: Entity, rig: &RigAsset, ghosts: &mut Assets<GhostMaterial>) -> Vec<Entity> {
    let bones = &rig.skeleton.skeleton.bones;
    let rest = rest_pose(&rig.skeleton);
    let mut joints: Vec<Entity> = Vec::with_capacity(bones.len());
    for (i, b) in bones.iter().enumerate() {
        let p = if b.parent >= 0 && (b.parent as usize) < i { joints[b.parent as usize] } else { parent };
        let (t, r) = rest[i];
        let j = commands
            .spawn((Name::new(b.name.clone()), Transform::from_translation(t).with_rotation(r), ChildOf(p)))
            .id();
        joints.push(j);
    }
    for m in &rig.meshes {
        commands.spawn((
            Mesh3d(m.mesh.clone()),
            MeshMaterial3d(ghosts.add(ghost_material(m.material.clone()))),
            SkinnedMesh { inverse_bindposes: rig.bindposes.clone(), joints: joints.clone() },
            NoFrustumCulling,
            NotShadowCaster,
            RenderLayers::layer(LAYER_PLAYER_BODY),
            ChildOf(parent),
        ));
    }
    joints
}

/// The rest pose (parent-space bone position and rotation) of a skeleton.
fn rest_pose(a: &Animator) -> Vec<(Vec3, Quat)> {
    a.skeleton.bones.iter().map(|b| (b.pos, b.quat)).collect()
}

/// Model-space bone transforms of a pose.
fn bone_to_model(skeleton: &Animator, pose: &[(Vec3, Quat)]) -> Vec<Affine3A> {
    let mut out: Vec<Affine3A> = Vec::with_capacity(pose.len());
    for (i, b) in skeleton.skeleton.bones.iter().enumerate() {
        let m = Affine3A::from_rotation_translation(pose[i].1, pose[i].0);
        let m = if b.parent >= 0 && (b.parent as usize) < i { out[b.parent as usize] * m } else { m };
        out.push(m);
    }
    out
}

/// Blends two bone poses by `w` (0 = a, 1 = b).
fn blend_poses(a: &[(Vec3, Quat)], b: &[(Vec3, Quat)], w: f32) -> Vec<(Vec3, Quat)> {
    a.iter().zip(b).map(|(x, y)| (x.0.lerp(y.0, w), x.1.slerp(y.1, w).normalize())).collect()
}

/// Maps a pose of the animation source onto Chell's bones; bones with no
/// same-named partner keep their rest pose.
fn to_chell(rig: &ChellRig, anim_pose: &[(Vec3, Quat)]) -> Vec<(Vec3, Quat)> {
    let rest = rest_pose(&rig.skeleton);
    rest.iter().enumerate().map(|(i, r)| rig.bone_map[i].and_then(|j| anim_pose.get(j).copied()).unwrap_or(*r)).collect()
}

/// Fallback ground speed when a sequence records no movement (Portal 2's 175 u/s).
const RUN_SPEED: f32 = 175.0 * U;
/// Standing still, the legs keep facing until the torso is this far round
/// (the aim layer covers +-45 degrees), then turn to catch up.
const MAX_BODY_YAW: f32 = 45.0;
/// How fast the legs turn to catch up (degrees per second).
const FEET_TURN_RATE: f32 = 360.0;

fn wrap_deg(a: f32) -> f32 {
    (a + 180.0).rem_euclid(360.0) - 180.0
}

/// Picks the base sequence for this tick (Portal 2's `ACT_MP_*` choice).
fn base_sequence(seqs: &ChellSeqs, state: &AnimState, grounded: bool, ducked: bool, speed: f32) -> usize {
    let moving = speed > IDLE_SPEED;
    if grounded {
        return match (ducked, moving) {
            (true, true) => seqs.crouch_walk,
            (true, false) => seqs.crouch_idle,
            (false, true) => seqs.run,
            (false, false) => seqs.idle,
        };
    }
    if state.jumped {
        // Jump start once, then the float loop.
        if state.seq == seqs.jump && state.cycle < 1.0 {
            return seqs.jump;
        }
        return seqs.float;
    }
    seqs.airwalk
}

/// Drives Chell like Portal 2's player animation state: base activity from
/// movement, the run blend from velocity (move_x/move_y), the aim layer from
/// the view (body_yaw/body_pitch, legs turning in place past 45 degrees), the
/// landing and firing gestures, and the gun bone-merged onto her hand.
pub(crate) fn drive_parts(
    time: Res<Time>,
    mut fired: MessageReader<p2_core::GunFired>,
    mut teleported: MessageReader<p2_core::PortalTeleported>,
    players: Query<(&CharacterVelocity, &Grounded, &Crouch, &ViewAngles), With<Player>>,
    mut rigs: Query<(&PlayerBody, &mut ChellRig, &mut Transform)>,
    mut transforms: Query<&mut Transform, Without<ChellRig>>,
) {
    let dt = time.delta_secs();
    let shots: Vec<Entity> = fired.read().map(|f| f.gun).collect();
    let moved: Vec<Entity> = teleported.read().map(|t| t.entity).collect();
    for (body, mut rig, mut root_tf) in &mut rigs {
        let rig = &mut *rig;
        let Ok((vel, grounded, crouch, view)) = players.get(body.player) else { continue };
        let st = &mut rig.state;
        let flat = Vec3::new(vel.0.x, 0.0, vel.0.z);
        let speed = flat.length();
        let eye_yaw = view.yaw.to_degrees();
        // A portal turned the whole player: the legs turn with it.
        if moved.contains(&body.player) {
            st.feet_yaw = eye_yaw;
        }

        // Legs: follow the eye while moving; standing still, only once the
        // torso has turned past the aim layer's range.
        let diff = wrap_deg(eye_yaw - st.feet_yaw);
        if speed > IDLE_SPEED {
            let step = (FEET_TURN_RATE * 2.0 * dt).min(diff.abs());
            st.feet_yaw = wrap_deg(st.feet_yaw + step * diff.signum());
        } else if diff.abs() > MAX_BODY_YAW {
            let excess = diff.abs() - MAX_BODY_YAW;
            let step = (FEET_TURN_RATE * dt).max(excess).min(diff.abs());
            st.feet_yaw = wrap_deg(st.feet_yaw + step * diff.signum());
        }
        // Eye to the left of the legs (+ yaw) aims the torso left: negative.
        let body_yaw = -wrap_deg(eye_yaw - st.feet_yaw).clamp(-MAX_BODY_YAW, MAX_BODY_YAW);

        // Air state, landing gesture.
        if st.grounded && !grounded.on_ground {
            st.jumped = vel.0.y > 0.5;
            if st.jumped {
                st.seq = rig.seqs.jump;
                st.cycle = 0.0;
            }
        }
        if !st.grounded && grounded.on_ground {
            st.jumped = false;
            if let Some(land) = rig.seqs.land {
                st.gestures.push((land, 0.0));
            }
        }
        st.grounded = grounded.on_ground;
        if shots.contains(&body.player) {
            let g = if crouch.ducked { rig.seqs.fire_crouch } else { rig.seqs.fire };
            if let Some(g) = g {
                st.gestures.push((g, 0.0));
            }
        }

        // Base sequence, cross-faded.
        let want = base_sequence(&rig.seqs, st, grounded.on_ground, crouch.ducked, speed);
        if want != st.seq {
            st.prev = Some((st.seq, st.cycle));
            st.fade = 0.0;
            st.seq = want;
            st.cycle = 0.0;
        }
        let dur = |seq: usize| rig.anims.duration(seq).max(1.0 / 30.0);
        // Run and crouch-walk play at the speed she moves.
        // Movement blends: the move pose parameters are the speed as a
        // fraction of the sequence's own ground speed, so the stride matches
        // the speed; only past that speed does the cycle play faster.
        let ground = match rig.anims.ground_speed(st.seq) * U {
            g if g > 0.01 => g,
            _ => RUN_SPEED,
        };
        let move_frac = speed / ground;
        let rate = if move_frac > 1.0 { move_frac.min(4.0) } else { 1.0 };
        st.cycle += dt * rate / dur(st.seq);
        let looping = rig.anims.sequences.get(st.seq).is_some_and(|q| q.looping);
        if looping {
            st.cycle = st.cycle.rem_euclid(1.0);
        } else {
            st.cycle = st.cycle.min(1.0);
        }
        if let Some((pseq, pc)) = st.prev.as_mut() {
            *pc = (*pc + dt / dur(*pseq)).rem_euclid(1.0);
        }
        st.fade = (st.fade + dt / BLEND_TIME).min(1.0);
        for g in &mut st.gestures {
            g.1 += dt / dur(g.0);
        }
        st.gestures.retain(|g| g.1 < 1.0);

        // Pose parameters (Source units and signs).
        let mut pose = vec![0.0; rig.anims.pose_params.len()];
        let feet_rot = Quat::from_rotation_y(st.feet_yaw.to_radians());
        let local = feet_rot.inverse() * flat; // forward = -Z, left = -X
        let set = |pose: &mut Vec<f32>, i: Option<usize>, v: f32| {
            if let Some(i) = i {
                pose[i] = v;
            }
        };
        let mv = Vec2::new(-local.z, local.x) / ground; // (forward, right)
        let mv = if mv.length() > 1.0 { mv.normalize() } else { mv };
        set(&mut pose, rig.pose_idx.move_x, mv.x);
        set(&mut pose, rig.pose_idx.move_y, mv.y);
        set(&mut pose, rig.pose_idx.body_yaw, body_yaw);
        set(&mut pose, rig.pose_idx.body_pitch, view.pitch.to_degrees().clamp(-45.0, 90.0));

        let gestures: Vec<(usize, f32, f32)> = st.gestures.iter().map(|&(g, c)| (g, c, 1.0)).collect();
        let mut anim_pose = rig.anims.evaluate(st.seq, st.cycle, &pose, &gestures);
        if st.fade < 1.0 {
            if let Some((pseq, pc)) = st.prev {
                let old = rig.anims.evaluate(pseq, pc, &pose, &gestures);
                let w = st.fade * st.fade * (3.0 - 2.0 * st.fade);
                anim_pose = blend_poses(&old, &anim_pose, w);
            }
        }
        let chell_pose = to_chell(rig, &anim_pose);
        for (i, &j) in rig.joints.iter().enumerate() {
            if let Ok(mut t) = transforms.get_mut(j) {
                t.translation = chell_pose[i].0;
                t.rotation = chell_pose[i].1;
            }
        }
        // The legs face `feet_yaw`; the root otherwise follows the player.
        root_tf.rotation = feet_rot;
        if let Some((node, bone, inv_rest)) = rig.gun {
            let m = bone_to_model(&rig.skeleton, &chell_pose)[bone] * inv_rest;
            let (_, rot, pos) = m.to_scale_rotation_translation();
            if let Ok(mut t) = transforms.get_mut(node) {
                t.translation = pos;
                t.rotation = rot;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_axes_map_to_body_axes() {
        let q = model_to_body();
        // Source: x forward, y left, z up; body: -Z forward, -X left, +Y up.
        assert!((q * Vec3::X).distance(Vec3::NEG_Z) < 1e-5);
        assert!((q * Vec3::Y).distance(Vec3::NEG_X) < 1e-5);
        assert!((q * Vec3::Z).distance(Vec3::Y) < 1e-5);
    }
}

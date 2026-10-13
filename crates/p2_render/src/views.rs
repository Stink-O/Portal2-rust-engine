//! The view tree: which virtual cameras see which portals, and where they sit.
//!
//! A node is a sequence of portal crossings from the main camera. The root
//! (code 1) is the main camera. The node "through X" from parent P has pose
//! `pair(X) * P.pose` (see [`p2_core::portal_pair`]) and an oblique near plane
//! on X's partner. Node codes are binary: the leading 1 marks the root, and
//! every crossing appends one bit (0 = Blue, 1 = Orange). So the level of a
//! node is `floor(log2(code))`, and a child is `(code << 1) | kind`.
//!
//! Level 1 is seen directly through a portal from the main view. The deepest
//! level with real child views is `recursion_depth`. The level below it (the
//! "plain" level) draws its portals with the depth doubler: last frame's image
//! of that node, re-mapped from the outer opening's screen rectangle into the
//! inner one, which gives Portal 2's endless tunnel without more passes.

use std::collections::BTreeMap;

use bevy::camera::visibility::RenderLayers;
use bevy::camera::{CameraProjection, Hdr, RenderTarget};
use bevy::core_pipeline::tonemapping::{DebandDither, Tonemapping};
use bevy::math::Affine3A;
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use bevy::transform::helper::TransformHelper;
use bevy::window::PrimaryWindow;

use p2_core::math::PortalFrame;
use p2_core::{
    player_ghost_layer, LocalEye, MainCamera, Player, Portal, PortalAnimation, PortalKind, PortalLink, PortalPair,
    PortalViewCamera, LAYER_PLAYER_BODY, LAYER_RENDER_FIRST,
};

use crate::projection::PortalProjection;
use crate::{node_layer, PortalRenderConfig};

/// How far behind the exit plane a portal view's oblique near plane sits (1 u).
const CLIP_BEHIND_EXIT: f32 = 0.0254;

/// The root (main camera) node code.
pub const ROOT: u32 = 1;

/// Code of the child "through `kind`" of node `parent`.
pub fn child_code(parent: u32, kind: PortalKind) -> u32 {
    (parent << 1) | kind.index() as u32
}

/// Level of a node code: 0 for the root, 1 for a direct view, and so on.
pub const fn node_level(code: u32) -> u32 {
    31 - code.leading_zeros()
}

/// Portal crossings from the main camera to this node, in order.
pub fn node_path(code: u32) -> Vec<PortalKind> {
    let level = node_level(code);
    (0..level)
        .rev()
        .map(|i| if (code >> i) & 1 == 0 { PortalKind::Blue } else { PortalKind::Orange })
        .collect()
}

/// Everything about one placed portal, as seen this frame.
#[derive(Clone, Debug)]
pub struct PortalInfo {
    pub entity: Entity,
    pub frame: PortalFrame,
    pub half: Vec2,
    /// The partner's kind, when the pair is linked (both halves exist and
    /// point at each other).
    pub partner: Option<PortalKind>,
    pub open: f32,
    pub static_amount: f32,
}

/// The two halves of the pair, indexed by [`PortalKind::index`].
#[derive(Resource, Clone, Debug, Default)]
pub struct PortalInfos(pub [Option<PortalInfo>; 2]);

impl PortalInfos {
    pub fn get(&self, kind: PortalKind) -> Option<&PortalInfo> {
        self.0[kind.index()].as_ref()
    }
}

/// One camera of the tree this frame.
#[derive(Clone, Debug)]
pub struct NodeView {
    pub code: u32,
    pub level: u32,
    pub pose: Affine3A,
    pub proj: PortalProjection,
    /// The portal this node looks through (`None` for the root).
    pub via: Option<PortalKind>,
    pub via_entity: Option<Entity>,
    /// Where that portal's opening sits on screen, as seen by the parent
    /// camera: (min.x, min.y, max.x, max.y) in screen UV (y down). Every
    /// node image is screen-aligned, so this is also where this node's image
    /// shows. Used by the depth doubler.
    pub via_rect: Option<Vec4>,
}

/// Clip-from-world matrix of a node camera.
pub fn node_clip_from_world(node: &NodeView) -> Mat4 {
    node.proj.get_clip_from_view() * Mat4::from(node.pose.inverse())
}

/// Screen rectangle (UV, y down) of a portal opening as seen through
/// `clip_from_world`, or `None` if any corner is behind the camera.
pub fn screen_rect(clip_from_world: Mat4, frame: &PortalFrame, half: Vec2) -> Option<Vec4> {
    let mut lo = Vec2::splat(f32::MAX);
    let mut hi = Vec2::splat(f32::MIN);
    for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        let w = frame.to_world(Vec3::new(sx * half.x, sy * half.y, 0.0));
        let c = clip_from_world * w.extend(1.0);
        if c.w <= 1e-4 {
            return None;
        }
        let ndc = c.truncate().truncate() / c.w;
        let uv = Vec2::new(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
        lo = lo.min(uv);
        hi = hi.max(uv);
    }
    (hi.x - lo.x > 1e-5 && hi.y - lo.y > 1e-5).then(|| Vec4::new(lo.x, lo.y, hi.x, hi.y))
}

/// The active nodes of the tree this frame (root included).
#[derive(Resource, Clone, Debug, Default)]
pub struct ViewTree {
    pub nodes: BTreeMap<u32, NodeView>,
}

impl ViewTree {
    /// Number of virtual cameras (not counting the main camera) this frame.
    pub fn active_views(&self) -> usize {
        self.nodes.len().saturating_sub(1)
    }
}

/// Conservative "can this camera see this portal's front face" test: the
/// camera must be in front of the plane, and the opening's corners must not
/// all be off one side of the view.
pub fn portal_visible(clip_from_world: Mat4, cam_pos: Vec3, frame: &PortalFrame, half: Vec2) -> bool {
    if frame.plane_distance(cam_pos) <= 0.0 {
        return false;
    }
    let corners = [
        frame.to_world(Vec3::new(-half.x, -half.y, 0.0)),
        frame.to_world(Vec3::new(half.x, -half.y, 0.0)),
        frame.to_world(Vec3::new(half.x, half.y, 0.0)),
        frame.to_world(Vec3::new(-half.x, half.y, 0.0)),
    ];
    let clip: Vec<Vec4> = corners.iter().map(|c| clip_from_world * c.extend(1.0)).collect();
    if clip.iter().all(|c| c.w <= 1e-4) {
        return false;
    }
    if clip.iter().any(|c| c.w <= 1e-4) {
        return true;
    }
    let outside = |f: &dyn Fn(&Vec4) -> bool| clip.iter().all(f);
    !(outside(&|c| c.x < -c.w) || outside(&|c| c.x > c.w) || outside(&|c| c.y < -c.w) || outside(&|c| c.y > c.w))
}

/// Builds the view tree from the main camera and the two portal halves,
/// down to level `depth + 1` (the plain level).
/// A child exists only if its parent's camera can see the portal it looks
/// through, so most scenes have a handful of active views.
pub fn build_tree(
    root_pose: Affine3A,
    root_proj: &PortalProjection,
    infos: &[Option<PortalInfo>; 2],
    depth: u32,
) -> BTreeMap<u32, NodeView> {
    let mut nodes = BTreeMap::new();
    nodes.insert(
        ROOT,
        NodeView { code: ROOT, level: 0, pose: root_pose, proj: root_proj.clone(), via: None, via_entity: None, via_rect: None },
    );
    let mut frontier = vec![ROOT];
    for _ in 0..=depth {
        let mut next = Vec::new();
        for code in std::mem::take(&mut frontier) {
            let (pose, proj, level) = {
                let n = &nodes[&code];
                (n.pose, n.proj.clone(), n.level)
            };
            let clip_from_world = proj.get_clip_from_view() * Mat4::from(pose.inverse());
            let eye = Vec3::from(pose.translation);
            for kind in [PortalKind::Blue, PortalKind::Orange] {
                let Some(x) = infos[kind.index()].as_ref() else { continue };
                let Some(partner) = x.partner else { continue };
                let Some(exit) = infos[partner.index()].as_ref() else { continue };
                if !portal_visible(clip_from_world, eye, &x.frame, x.half) {
                    continue;
                }
                let child_pose = PortalPair::new(x.frame, exit.frame).affine() * pose;
                // Oblique near plane: the exit portal's plane, in the child's view space.
                let view_from_world = child_pose.inverse();
                // It sits 1 u behind the exit plane: the surface quad stands a few
                // mm off the wall, so a plane exactly on the wall would drop the
                // sliver of a body that straddles the exit (a thin seam between
                // the half seen through the portal and its ghost in front of it).
                // Only the wall's own back faces lie in that 1 u, and they are culled.
                let n = view_from_world.transform_vector3(exit.frame.normal()).normalize();
                let p = view_from_world.transform_point3(exit.frame.position - exit.frame.normal() * CLIP_BEHIND_EXIT);
                let plane = n.extend(-n.dot(p));
                let clip = (plane.w < 0.0).then_some(plane);
                let child = child_code(code, kind);
                nodes.insert(
                    child,
                    NodeView {
                        code: child,
                        level: level + 1,
                        pose: child_pose,
                        proj: PortalProjection { clip_plane: clip, ..root_proj.clone() },
                        via: Some(kind),
                        via_entity: Some(x.entity),
                        via_rect: screen_rect(clip_from_world, &x.frame, x.half),
                    },
                );
                next.push(child);
            }
        }
        frontier = next;
    }
    nodes
}

/// The main camera as of this frame (captured before the tree is built).
#[derive(Resource, Clone, Default)]
pub struct MainView {
    pub entity: Option<Entity>,
    pub global: Affine3A,
    pub fov: f32,
    pub near: f32,
    pub far: f32,
    pub aspect: f32,
    pub size: UVec2,
    pub exposure: Option<bevy::camera::Exposure>,
    pub fog: Option<bevy::pbr::DistanceFog>,
}

impl MainView {
    pub fn projection(&self) -> PortalProjection {
        PortalProjection { fov: self.fov, aspect_ratio: self.aspect, near: self.near, far: self.far, clip_plane: None }
    }
}

/// Node cameras, created once at startup and reused (inactive when unused).
#[derive(Resource, Default)]
pub struct NodeCameras {
    pub cameras: BTreeMap<u32, Entity>,
    /// 1x1 target for inactive cameras, so they hold no full-size texture.
    pub dummy: Option<Handle<Image>>,
}

/// Render targets of the active node cameras, keyed by node code.
#[derive(Resource, Default)]
pub struct NodeImages {
    pub size: UVec2,
    pub images: BTreeMap<u32, Handle<Image>>,
    /// Last frame's image of each plain-level node (ping-pong with `images`),
    /// sampled by the depth doubler while `images` is being rendered.
    pub prev: BTreeMap<u32, Handle<Image>>,
}

pub(crate) fn new_target(images: &mut Assets<Image>, size: UVec2) -> Handle<Image> {
    let mut image = Image::new_target_texture(size.x, size.y, TextureFormat::Rgba16Float, None);
    image.asset_usage = bevy::asset::RenderAssetUsages::RENDER_WORLD;
    images.add(image)
}

pub(crate) fn capture_main_view(
    helper: TransformHelper,
    cams: Query<(Entity, &Camera, Option<&Projection>, Option<&bevy::camera::visibility::RenderLayers>), With<MainCamera>>,
    exposure: Query<&bevy::camera::Exposure, With<MainCamera>>,
    fog: Query<&bevy::pbr::DistanceFog, With<MainCamera>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut view: ResMut<MainView>,
) {
    let Some((e, cam, proj, _layers)) = cams.iter().next() else {
        view.entity = None;
        return;
    };
    let Ok(global) = helper.compute_global_transform(e) else { return };
    view.entity = Some(e);
    view.global = global.affine();
    let default = PerspectiveProjection::default();
    let p = match proj {
        Some(Projection::Perspective(p)) => p,
        _ => &default,
    };
    view.fov = p.fov;
    view.near = p.near;
    view.far = p.far;
    let size = cam
        .physical_target_size()
        .or_else(|| windows.iter().next().map(|w| UVec2::new(w.physical_width(), w.physical_height())))
        .unwrap_or(UVec2::new(1280, 720));
    view.size = size.max(UVec2::ONE);
    view.aspect = view.size.x as f32 / view.size.y as f32;
    view.exposure = exposure.iter().next().copied();
    view.fog = fog.iter().next().cloned();
}

/// Spawns the node cameras (levels 1 to depth + 1), all inactive to begin with.
pub(crate) fn spawn_node_cameras(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut cams: ResMut<NodeCameras>,
    config: Res<PortalRenderConfig>,
) {
    if !cams.cameras.is_empty() {
        return;
    }
    let mut dummy = Image::new_target_texture(1, 1, TextureFormat::Rgba16Float, None);
    dummy.asset_usage = bevy::asset::RenderAssetUsages::RENDER_WORLD;
    let dummy = images.add(dummy);
    cams.dummy = Some(dummy.clone());
    for code in 2..(1u32 << (config.recursion_depth + 2)) {
        let level = node_level(code);
        let e = commands
            .spawn((
                Name::new(format!("Portal view {code} (level {level})")),
                Camera3d::default(),
                Camera { order: -((level as isize) * 100) - code as isize, is_active: false, ..default() },
                RenderTarget::Image(dummy.clone().into()),
                Projection::custom(PortalProjection::default()),
                Hdr,
                Tonemapping::None,
                DebandDither::Disabled,
                Msaa::Off,
                RenderLayers::layer(0),
                PortalViewCamera { portal: Entity::PLACEHOLDER, level },
                Transform::default(),
            ))
            .id();
        cams.cameras.insert(code, e);
    }
}

/// Collects both portal halves and rebuilds the view tree, then points each
/// node camera at its pose, projection, layers and target. Runs in
/// `PostUpdate` after the main camera's transform is set this frame, so the
/// views have no lag.
#[allow(clippy::type_complexity)]
pub(crate) fn update_views(
    mut commands: Commands,
    main: Res<MainView>,
    portals: Query<(Entity, &Portal, &Transform, &PortalAnimation, Option<&PortalLink>), Without<PortalViewCamera>>,
    mut infos: ResMut<PortalInfos>,
    mut tree: ResMut<ViewTree>,
    config: Res<PortalRenderConfig>,
    cams: Res<NodeCameras>,
    mut images: ResMut<Assets<Image>>,
    mut node_images: ResMut<NodeImages>,
    mut cam_q: Query<
        (
            &mut Camera,
            &mut Transform,
            &mut Projection,
            &mut RenderLayers,
            &mut RenderTarget,
            &mut PortalViewCamera,
        ),
        (Without<MainCamera>, Without<Portal>, Without<Player>),
    >,
    eye: Res<LocalEye>,
) {
    // Portal halves and their link state.
    let mut found: [Option<(Entity, Portal, Transform, PortalAnimation, Option<Entity>)>; 2] = [None, None];
    for (e, portal, tf, anim, link) in &portals {
        found[portal.kind.index()] = Some((e, portal.clone(), *tf, *anim, link.map(|l| l.0)));
    }
    let mut out: [Option<PortalInfo>; 2] = [None, None];
    for kind in [PortalKind::Blue, PortalKind::Orange] {
        let Some((e, portal, tf, anim, link)) = found[kind.index()].clone() else { continue };
        let partner = link.and_then(|target| {
            [PortalKind::Blue, PortalKind::Orange]
                .into_iter()
                .filter(|k| *k != kind)
                .find(|k| found[k.index()].as_ref().is_some_and(|(pe, ..)| *pe == target))
        });
        out[kind.index()] = Some(PortalInfo {
            entity: e,
            frame: Portal::frame(&tf),
            half: portal.half_size,
            partner,
            open: anim.open_amount,
            static_amount: anim.static_amount,
        });
    }
    infos.0 = out.clone();

    if main.entity.is_none() || main.size.x == 0 {
        tree.nodes.clear();
        deactivate_all(&cams, &mut cam_q);
        return;
    }

    let new_tree = build_tree(main.global, &main.projection(), &out, config.recursion_depth);
    let target_size = (main.size.as_vec2() * config.target_scale).round().as_uvec2().max(UVec2::ONE);
    // Node images: one per active child, sized like the main view.
    if node_images.size != target_size {
        node_images.size = target_size;
        node_images.images.clear();
        node_images.prev.clear();
    }
    for &code in new_tree.keys().filter(|c| **c != ROOT) {
        if !node_images.images.contains_key(&code) {
            let h = new_target(&mut images, target_size);
            node_images.images.insert(code, h);
        }
        // Plain level: swap with last frame's image, which the doubler samples.
        if node_level(code) == config.recursion_depth + 1 {
            let prev = match node_images.prev.remove(&code) {
                Some(p) => p,
                None => new_target(&mut images, target_size),
            };
            let cur = node_images.images.insert(code, prev).expect("inserted above");
            node_images.prev.insert(code, cur);
        }
    }

    // Point every pooled camera at its node, or park it.
    for (&code, &entity) in cams.cameras.iter() {
        let Ok((mut cam, mut tf, mut proj, mut layers, mut target, mut pvc)) = cam_q.get_mut(entity) else {
            continue;
        };
        let Some(node) = new_tree.get(&code) else {
            cam.is_active = false;
            if let Some(dummy) = cams.dummy.clone() {
                *target = RenderTarget::Image(dummy.into());
            }
            continue;
        };
        {
                let (_, rot, pos) = node.pose.to_scale_rotation_translation();
                let want_tf = Transform::from_translation(pos).with_rotation(rot.normalize());
                if *tf != want_tf {
                    *tf = want_tf;
                }
                *proj = Projection::custom(node.proj.clone());
                // Every level draws its portal surfaces; the plain level's use
                // the depth doubler. Plus the local player's body and ghosts,
                // by Portal 2's rules (player-in-portals.md §2-3).
                let want_layers = player_layers(RenderLayers::from_layers(&[0, node_layer(code)]), &eye, Some(node));
                if *layers != want_layers {
                    *layers = want_layers;
                }
                let image = node_images.images.get(&code).cloned();
                if let Some(image) = image {
                    *target = RenderTarget::Image(image.into());
                }
                cam.is_active = true;
                cam.order = -((node.level as isize) * 100) - code as isize;
                pvc.level = node.level;
                if let Some(e) = node.via_entity {
                    pvc.portal = e;
                }
                // Match the main camera's exposure and fog so a view looks like the room.
                match main.exposure {
                    Some(x) => commands.entity(entity).insert(x),
                    None => commands.entity(entity).remove::<bevy::camera::Exposure>(),
                };
                match main.fog.clone() {
                    Some(x) => commands.entity(entity).insert(x),
                    None => commands.entity(entity).remove::<bevy::pbr::DistanceFog>(),
                };
        }
    }
    // Keep the images of nodes that go out of view: re-creating one when the
    // view comes back (walking back and forth through a pair) can leave its
    // first frame empty, a one-frame flash. They are only freed on resize.
    tree.nodes = new_tree;
}

/// `base` plus the local player's body and ghost layers a camera draws
/// (player-in-portals.md §2-3); `node` is `None` for the main camera.
///
/// * Third person: everything, everywhere.
/// * Main view: the body only while the eye has gone through a portal ahead of
///   it (the camera is then on the far side); the ghosts only while it has not
///   (else the camera sits inside one).
/// * Level-1 view through the portal the eye looks out of: no body (that
///   camera is the real eye, inside the body). Level-1 view through the portal
///   whose opening a ghost comes from, while the eye has not gone through: no
///   ghost (that camera sits inside the ghost). Deeper levels draw both.
pub fn player_layers(base: RenderLayers, eye: &LocalEye, node: Option<&NodeView>) -> RenderLayers {
    let ghosts = [PortalKind::Blue, PortalKind::Orange];
    if eye.third_person {
        return ghosts.into_iter().fold(base.with(LAYER_PLAYER_BODY), |l, k| l.with(player_ghost_layer(k)));
    }
    let level1 = node.filter(|n| n.level == 1);
    let body = match node {
        None => eye.through.is_some(),
        Some(_) => !level1.is_some_and(|n| eye.through.is_some() && n.via_entity == eye.through),
    };
    let ghost = |k: PortalKind| match node {
        None => eye.through.is_none(),
        Some(_) => !level1.is_some_and(|n| eye.through.is_none() && n.via == Some(k)),
    };
    let base = if body { base.with(LAYER_PLAYER_BODY) } else { base };
    ghosts.into_iter().filter(|k| ghost(*k)).fold(base, |l, k| l.with(player_ghost_layer(k)))
}

fn deactivate_all(
    cams: &NodeCameras,
    cam_q: &mut Query<
        (&mut Camera, &mut Transform, &mut Projection, &mut RenderLayers, &mut RenderTarget, &mut PortalViewCamera),
        (Without<MainCamera>, Without<Portal>, Without<Player>),
    >,
) {
    for &entity in cams.cameras.values() {
        if let Ok((mut cam, ..)) = cam_q.get_mut(entity) {
            cam.is_active = false;
        }
    }
}

/// Adds the root surface layer to the host's main camera, inserting
/// `RenderLayers` if the host did not set any (default layer 0 is kept), and
/// sets its local player body and ghost layers for this frame ([`player_layers`]).
pub(crate) fn ensure_main_layers(
    mut commands: Commands,
    cams: Query<(Entity, Option<&RenderLayers>), With<MainCamera>>,
    eye: Res<LocalEye>,
) {
    for (entity, layers) in &cams {
        let player = [LAYER_PLAYER_BODY, player_ghost_layer(PortalKind::Blue), player_ghost_layer(PortalKind::Orange)];
        let base = player.iter().fold(layers.cloned().unwrap_or_default(), |l, &x| l.without(x));
        let base = base.with(LAYER_RENDER_FIRST);
        let want = player_layers(base, &eye, None);
        if layers != Some(&want) {
            commands.entity(entity).insert(want);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_and_levels() {
        assert_eq!(node_level(ROOT), 0);
        assert_eq!(node_level(child_code(ROOT, PortalKind::Blue)), 1);
        assert_eq!(node_level(child_code(child_code(ROOT, PortalKind::Orange), PortalKind::Blue)), 2);
        assert_eq!(node_path(child_code(child_code(ROOT, PortalKind::Orange), PortalKind::Blue)), vec![PortalKind::Orange, PortalKind::Blue]);
        assert!(node_path(ROOT).is_empty());
    }

    fn pair_infos() -> ([Option<PortalInfo>; 2], PortalFrame, PortalFrame) {
        // Blue on the west wall (x = -12) facing +X, Orange on the east wall (x = 12) facing -X.
        let blue = PortalFrame::new(Vec3::new(-11.99, 1.5, 0.0), Quat::from_rotation_y(std::f32::consts::FRAC_PI_2));
        let orange = PortalFrame::new(Vec3::new(11.99, 1.5, 0.0), Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2));
        let mk = |kind: PortalKind, frame: PortalFrame, partner: PortalKind| PortalInfo {
            entity: Entity::from_raw_u32(kind.index() as u32 + 1).unwrap(),
            frame,
            half: p2_core::PORTAL_HALF_SIZE,
            partner: Some(partner),
            open: 1.0,
            static_amount: 0.0,
        };
        let infos = [Some(mk(PortalKind::Blue, blue, PortalKind::Orange)), Some(mk(PortalKind::Orange, orange, PortalKind::Blue))];
        (infos, blue, orange)
    }

    fn root_proj() -> PortalProjection {
        PortalProjection { fov: 1.0, aspect_ratio: 16.0 / 9.0, near: 0.1, far: 1000.0, clip_plane: None }
    }

    #[test]
    fn facing_pair_sees_itself_through_itself() {
        // Standing at x = 2 looking west at the blue portal.
        let main = Vec3::new(2.0, 1.5, 0.0);
        let pose = Affine3A::from_rotation_translation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2), main);
        let (infos, blue, orange) = pair_infos();
        let tree = build_tree(pose, &root_proj(), &infos, 2);
        // Root sees blue; level 1 (through blue) sees blue again.
        let l1 = child_code(ROOT, PortalKind::Blue);
        assert!(tree.contains_key(&l1), "root should see blue");
        assert!(tree.contains_key(&child_code(l1, PortalKind::Blue)), "level 1 should see blue");
        // The level-1 camera is the pair image of the main camera: behind orange.
        let cam1 = tree[&l1].pose.translation;
        let want = PortalPair::new(blue, orange).map_point(main);
        assert!((Vec3::from(cam1) - want).length() < 1e-4);
        assert!(orange.plane_distance(Vec3::from(cam1)) < 0.0, "level-1 camera must sit behind the exit");
        // Its clip plane is the exit plane: the oblique plane is set.
        assert!(tree[&l1].proj.clip_plane.is_some());
    }

    #[test]
    fn player_layers_follow_portal_2() {
        use p2_core::{LAYER_PLAYER_GHOST, LAYER_PLAYER_GHOST_ORANGE};
        let (infos, ..) = pair_infos();
        let blue = infos[0].as_ref().unwrap().entity;
        let orange = infos[1].as_ref().unwrap().entity;
        let node = |level: u32, via: PortalKind, e: Entity| NodeView {
            code: 2,
            level,
            pose: Affine3A::IDENTITY,
            proj: root_proj(),
            via: Some(via),
            via_entity: Some(e),
            via_rect: None,
        };
        let has = |l: &RenderLayers, x: usize| l.intersects(&RenderLayers::layer(x));
        let base = RenderLayers::layer(0);
        let first = LocalEye::default();
        let through = LocalEye { third_person: false, through: Some(orange) };
        // Main view: ghosts only, or the body only once the eye went through.
        let m = player_layers(base.clone(), &first, None);
        assert!(!has(&m, LAYER_PLAYER_BODY) && has(&m, LAYER_PLAYER_GHOST) && has(&m, LAYER_PLAYER_GHOST_ORANGE));
        let m = player_layers(base.clone(), &through, None);
        assert!(has(&m, LAYER_PLAYER_BODY) && !has(&m, LAYER_PLAYER_GHOST));
        // Level 1 through blue: body, no blue ghost; through orange while the eye looks out of it: no body.
        let l = player_layers(base.clone(), &first, Some(&node(1, PortalKind::Blue, blue)));
        assert!(has(&l, LAYER_PLAYER_BODY) && !has(&l, LAYER_PLAYER_GHOST) && has(&l, LAYER_PLAYER_GHOST_ORANGE));
        let l = player_layers(base.clone(), &through, Some(&node(1, PortalKind::Orange, orange)));
        assert!(!has(&l, LAYER_PLAYER_BODY) && has(&l, LAYER_PLAYER_GHOST) && has(&l, LAYER_PLAYER_GHOST_ORANGE));
        // Level 2 draws everything.
        let l = player_layers(base, &first, Some(&node(2, PortalKind::Blue, blue)));
        assert!(has(&l, LAYER_PLAYER_BODY) && has(&l, LAYER_PLAYER_GHOST));
    }

    #[test]
    fn camera_facing_east_sees_only_orange() {
        // Standing at x = 2 looking east, away from blue.
        let pose = Affine3A::from_rotation_translation(Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2), Vec3::new(2.0, 1.5, 0.0));
        let (infos, ..) = pair_infos();
        let tree = build_tree(pose, &root_proj(), &infos, 2);
        // Looking east, the orange half is in view (its front faces the room), blue is behind.
        assert!(tree.contains_key(&child_code(ROOT, PortalKind::Orange)));
        assert!(!tree.contains_key(&child_code(ROOT, PortalKind::Blue)));
    }
}

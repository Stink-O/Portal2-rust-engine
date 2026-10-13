//! Ghost copies of [`Ghostable`] entities across linked portals, and the
//! clip planes that make the two halves join (player-in-portals.md §3).
//!
//! For every linked portal `P -> Q` whose opening an entity's bounds overlap
//! (and whose centre is no more than [`GHOST_CENTRE_BEHIND`] behind `P`),
//! the entity gets, as in Portal 2:
//!
//! * a **clip plane** on its own meshes at `P`, pushed [`CLIP_OVERLAP`]
//!   behind it (the part that has gone through `P` is discarded), and
//! * a **ghost**: a mirror of its whole entity tree, placed by
//!   `portal_pair(P, Q)` and clipped [`CLIP_OVERLAP`] behind `Q` (only the
//!   part that has come out of `Q` is shown).
//!
//! The two halves overlap by `2 * CLIP_OVERLAP` across the opening, so no
//! crack opens between them; the overlap itself sits behind a portal plane,
//! where the portal surface (main view) or the oblique near plane (portal
//! views) hides it.
//!
//! The mirror copies the local `Transform` of every descendant each frame, so
//! skinned meshes (joints), animated glTF scenes and plain props all stay in
//! sync without any per-type code. Mirrors of the local player's body (a root
//! on [`p2_core::LAYER_PLAYER_BODY`]) go on [`p2_core::player_ghost_layer`] of
//! `P`'s kind, so `p2_render` can apply the per-view rules; other mirrors
//! copy their root's layers.

use bevy::camera::visibility::{NoFrustumCulling, RenderLayers};
use bevy::light::NotShadowCaster;
use bevy::pbr::{ExtendedMaterial, MaterialExtension, StandardMaterial};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;
use p2_core::{
    player_ghost_layer, portal_pair, Portal, PortalFrame, PortalKind, PortalLink, PortalPair, LAYER_PLAYER_BODY,
};

/// Number of clip planes the ghost shader evaluates.
pub const MAX_CLIP_PLANES: usize = 4;

/// Half-thickness (metres) of the slab either side of a portal plane in which
/// an entity counts as touching the opening.
pub const GHOST_SLAB: f32 = 0.1;

/// How far behind its portal plane each half keeps drawing (2 Source units).
pub const CLIP_OVERLAP: f32 = p2_core::u(2.0);

/// An entity whose bounds centre is further than this behind a portal's
/// plane (5 Source units) gets no ghost from that portal.
pub const GHOST_CENTRE_BEHIND: f32 = p2_core::u(5.0);

/// A plane that never clips (`dot(0, p) + 1 >= 0`).
pub const DISABLED_PLANE: Vec4 = Vec4::new(0.0, 0.0, 0.0, 1.0);

/// Opt-in marker: the entity (a top-level root, unit scale) is duplicated
/// through linked portals while its [`GhostBounds`] overlap an opening.
///
/// Its meshes may use `StandardMaterial` (converted to [`GhostMaterial`]
/// automatically, including glTF scenes spawned under it) or [`GhostMaterial`]
/// directly. Set the bounds of a character body to its hull.
#[derive(Component, Clone, Copy, Debug, Default)]
#[require(GhostBounds)]
pub struct Ghostable;

/// Axis-aligned bounds of a [`Ghostable`], in the entity's local space (the
/// entity's origin is its `Transform` translation). Default: a unit cube.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct GhostBounds {
    pub min: Vec3,
    pub max: Vec3,
}

impl Default for GhostBounds {
    fn default() -> Self {
        Self { min: Vec3::splat(-0.5), max: Vec3::splat(0.5) }
    }
}

/// Up to [`MAX_CLIP_PLANES`] world-space planes; a point `p` is discarded when
/// `dot(plane.xyz, p) + plane.w < 0` for any of them. The default (all zero)
/// clips nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Reflect, ShaderType)]
pub struct ClipPlanes {
    pub planes: [Vec4; MAX_CLIP_PLANES],
}

/// The shader extension holding the clip planes. Use through [`GhostMaterial`].
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct GhostClipExt {
    #[uniform(100)]
    pub clip: ClipPlanes,
}

impl MaterialExtension for GhostClipExt {
    fn fragment_shader() -> ShaderRef {
        "embedded://p2_body/clip.wgsl".into()
    }
}

/// A `StandardMaterial` with ghost clipping. Every [`Ghostable`] mesh gets its
/// own instance, so planes never leak between entities.
pub type GhostMaterial = ExtendedMaterial<StandardMaterial, GhostClipExt>;

/// Wraps a `StandardMaterial` for a [`Ghostable`] mesh.
pub fn ghost_material(base: StandardMaterial) -> GhostMaterial {
    ExtendedMaterial { base, extension: GhostClipExt::default() }
}

/// Marks a mirror entity created by this module.
#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct GhostPart;

/// The live ghosts of one [`Ghostable`] root.
#[derive(Component, Default)]
pub(crate) struct Ghosts {
    copies: Vec<GhostCopy>,
}

struct GhostCopy {
    portal: Entity,
    exit: Entity,
    root: Entity,
    /// (source descendant, mirror) for every descendant, parents first.
    pairs: Vec<(Entity, Entity)>,
}

/// The plane at a portal's front face, keeping the front side: a point is
/// discarded when it is behind the opening.
pub fn front_plane(frame: &PortalFrame) -> Vec4 {
    let n = frame.normal();
    n.extend(-n.dot(frame.position))
}

/// [`front_plane`] pushed [`CLIP_OVERLAP`] behind the opening: the plane both
/// halves of a ghosted entity are clipped with.
pub fn clip_plane(frame: &PortalFrame) -> Vec4 {
    front_plane(frame) + Vec4::new(0.0, 0.0, 0.0, CLIP_OVERLAP)
}

/// The transform of a ghost of `root` placed through `pair`.
pub fn ghost_transform(pair: &PortalPair, root: &Transform) -> Transform {
    Transform {
        translation: pair.map_point(root.translation),
        rotation: pair.map_rotation(root.rotation),
        scale: root.scale,
    }
}

/// True when the entity's bounds (at its origin) overlap the opening of
/// `frame` (half extents `half`) within [`GHOST_SLAB`] of its plane, and the
/// bounds centre is not more than [`GHOST_CENTRE_BEHIND`] behind the plane.
pub fn overlaps_opening(frame: &PortalFrame, half: Vec2, root: &Transform, bounds: &GhostBounds) -> bool {
    let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
    for i in 0..8u32 {
        let c = Vec3::new(
            if i & 1 == 0 { bounds.min.x } else { bounds.max.x },
            if i & 2 == 0 { bounds.min.y } else { bounds.max.y },
            if i & 4 == 0 { bounds.min.z } else { bounds.max.z },
        );
        let l = frame.to_local(root.transform_point(c));
        lo = lo.min(l);
        hi = hi.max(l);
    }
    let centre = frame.plane_distance(root.transform_point((bounds.min + bounds.max) * 0.5));
    centre >= -GHOST_CENTRE_BEHIND
        && lo.x <= half.x && hi.x >= -half.x && lo.y <= half.y && hi.y >= -half.y && lo.z <= GHOST_SLAB && hi.z >= -GHOST_SLAB
}

type SourceData = (
    Option<&'static bevy::mesh::skinning::SkinnedMesh>,
    Option<&'static bevy::prelude::Mesh3d>,
    Option<&'static MeshMaterial3d<GhostMaterial>>,
    &'static Transform,
    Option<&'static Name>,
    Has<NoFrustumCulling>,
    Has<NotShadowCaster>,
);

/// Turns `StandardMaterial` meshes under a [`Ghostable`] into [`GhostMaterial`]
/// (one unique instance per mesh).
pub(crate) fn convert_ghost_materials(
    mut commands: Commands,
    added: Query<(Entity, &MeshMaterial3d<StandardMaterial>), Added<MeshMaterial3d<StandardMaterial>>>,
    parents: Query<&ChildOf>,
    ghostable: Query<(), With<Ghostable>>,
    standard: Res<Assets<StandardMaterial>>,
    mut ghost_materials: ResMut<Assets<GhostMaterial>>,
) {
    for (e, mat) in &added {
        if !ghostable.contains(top_ancestor(e, &parents)) {
            continue;
        }
        let Some(base) = standard.get(&mat.0) else { continue };
        let handle = ghost_materials.add(ghost_material(base.clone()));
        commands.entity(e).remove::<MeshMaterial3d<StandardMaterial>>().insert(MeshMaterial3d(handle));
    }
}

fn top_ancestor(mut e: Entity, parents: &Query<&ChildOf>) -> Entity {
    while let Ok(p) = parents.get(e) {
        e = p.parent();
    }
    e
}

/// All descendants of `root` (not including it), parents before children.
fn descendants(root: Entity, children: &Query<&Children>) -> Vec<Entity> {
    let mut out = Vec::new();
    let mut stack: Vec<Entity> = children.get(root).map(|c| c.to_vec()).unwrap_or_default();
    stack.reverse();
    while let Some(e) = stack.pop() {
        out.push(e);
        if let Ok(c) = children.get(e) {
            for k in c.iter().rev() {
                stack.push(k);
            }
        }
    }
    out
}

fn set_planes(materials: &mut Assets<GhostMaterial>, handle: &Handle<GhostMaterial>, planes: ClipPlanes) {
    if materials.get(handle).is_some_and(|m| m.extension.clip != planes) {
        if let Some(mut m) = materials.get_mut(handle) {
            m.extension.clip = planes;
        }
    }
}

/// Builds the mirror tree of `root` through `pair`, with the exit plane on its
/// materials. Returns the mirror root and the (source, mirror) pairs.
#[allow(clippy::too_many_arguments)]
fn spawn_copy(
    commands: &mut Commands,
    materials: &mut Assets<GhostMaterial>,
    sources: &Query<SourceData, Without<GhostPart>>,
    desc: &[Entity],
    root: Entity,
    root_tf: &Transform,
    parents: &Query<&ChildOf>,
    exit_plane: Vec4,
    layers: &RenderLayers,
) -> (Entity, Vec<(Entity, Entity)>) {
    let planes = ClipPlanes { planes: [exit_plane, DISABLED_PLANE, DISABLED_PLANE, DISABLED_PLANE] };
    let mirror_root = commands
        .spawn((
            Name::new("ghost root"),
            GhostPart,
            *root_tf,
            Visibility::default(),
            layers.clone(),
        ))
        .id();
    let mut map = std::collections::HashMap::new();
    map.insert(root, mirror_root);
    let mut pairs = Vec::with_capacity(desc.len());
    for &s in desc {
        let Ok((_, mesh, mat, tf, name, no_cull, no_shadow)) = sources.get(s) else { continue };
        let parent = parents.get(s).ok().and_then(|p| map.get(&p.parent()).copied()).unwrap_or(mirror_root);
        let mut e = commands.spawn((
            GhostPart,
            *tf,
            Visibility::default(),
            layers.clone(),
            ChildOf(parent),
        ));
        if let Some(n) = name {
            e.insert(Name::new(format!("mirror of {}", n.as_str())));
        }
        if let Some(Mesh3d(m)) = mesh {
            e.insert(Mesh3d(m.clone()));
        }
        if let Some(h) = mat {
            let base = materials.get(&h.0).cloned();
            if let Some(base) = base {
                let mut copy = base;
                copy.extension.clip = planes;
                e.insert(MeshMaterial3d(materials.add(copy)));
            }
        }
        if no_cull {
            e.insert(NoFrustumCulling);
        }
        if no_shadow {
            e.insert(NotShadowCaster);
        }
        let id = e.id();
        map.insert(s, id);
        pairs.push((s, id));
    }
    // Skinned meshes point at their joints, which are mirrored too.
    for &(s, m) in &pairs {
        if let Ok((Some(skin), ..)) = sources.get(s) {
            let joints = skin.joints.iter().map(|j| map.get(j).copied().unwrap_or(*j)).collect();
            commands.entity(m).insert(bevy::mesh::skinning::SkinnedMesh {
                inverse_bindposes: skin.inverse_bindposes.clone(),
                joints,
            });
        }
    }
    (mirror_root, pairs)
}

/// Creates, updates and removes ghosts and clip planes for every
/// [`Ghostable`] root. Runs in `P2Systems::Body` after the poses are written.
#[allow(clippy::too_many_arguments)]
pub(crate) fn sync_ghosts(
    mut commands: Commands,
    portals: Query<(Entity, &Portal, &PortalLink, &Transform), Without<GhostPart>>,
    mut roots: Query<
        (Entity, &Transform, &GhostBounds, Option<&RenderLayers>, Option<&mut Ghosts>),
        (With<Ghostable>, Without<ChildOf>, Without<GhostPart>),
    >,
    children: Query<&Children>,
    parents: Query<&ChildOf>,
    sources: Query<SourceData, Without<GhostPart>>,
    mut mirror_tf: Query<&mut Transform, With<GhostPart>>,
    mirror_mats: Query<&MeshMaterial3d<GhostMaterial>, With<GhostPart>>,
    mut materials: ResMut<Assets<GhostMaterial>>,
) {
    let list: Vec<(Entity, Portal, Entity, Transform)> =
        portals.iter().map(|(e, p, l, t)| (e, p.clone(), l.0, *t)).collect();
    for (root, root_tf, bounds, root_layers, ghosts) in &mut roots {
        let root_tf = *root_tf;
        let desc = descendants(root, &children);

        // The local player's body: its mirrors go on the entry portal's ghost layer.
        let player = root_layers.is_some_and(|l| l.intersects(&RenderLayers::layer(LAYER_PLAYER_BODY)));
        let mirror_layers = |kind: PortalKind| match (player, root_layers) {
            (true, _) => RenderLayers::layer(player_ghost_layer(kind)),
            (false, Some(l)) => l.clone(),
            (false, None) => RenderLayers::default(),
        };

        // Portals the root overlaps: (entry, exit, pair, entry plane, exit plane, entry kind).
        let mut hits: Vec<(Entity, Entity, PortalPair, Vec4, Vec4, PortalKind)> = Vec::new();
        for (pe, portal, link, ptf) in &list {
            let Some((_, _, _, qtf)) = list.iter().find(|(q, ..)| q == link) else { continue };
            let frame = Portal::frame(ptf);
            if overlaps_opening(&frame, portal.half_size, &root_tf, bounds) {
                let exit_frame = Portal::frame(qtf);
                hits.push((*pe, *link, portal_pair(ptf, qtf), clip_plane(&frame), clip_plane(&exit_frame), portal.kind));
            }
        }

        // Keep the copies still wanted and with the same tree; drop the rest.
        let mut kept: Vec<GhostCopy> = Vec::new();
        let old = match ghosts {
            Some(mut g) => std::mem::take(&mut g.copies),
            None => Vec::new(),
        };
        for copy in old {
            let wanted = hits.iter().any(|h| h.0 == copy.portal && h.1 == copy.exit);
            let same_tree = copy.pairs.len() == desc.len() && copy.pairs.iter().zip(&desc).all(|(p, d)| p.0 == *d);
            if wanted && same_tree {
                kept.push(copy);
            } else {
                commands.entity(copy.root).despawn();
            }
        }
        for (entry, exit, pair, _, exit_plane, kind) in &hits {
            if kept.iter().any(|c| c.portal == *entry && c.exit == *exit) {
                continue;
            }
            let (mirror_root, pairs) = spawn_copy(
                &mut commands,
                &mut materials,
                &sources,
                &desc,
                root,
                &ghost_transform(pair, &root_tf),
                &parents,
                *exit_plane,
                &mirror_layers(*kind),
            );
            kept.push(GhostCopy { portal: *entry, exit: *exit, root: mirror_root, pairs });
        }

        // Per frame: move each ghost with the root, copy the local poses, keep
        // the exit plane current on the mirror materials.
        for copy in &kept {
            let Some(h) = hits.iter().find(|h| h.0 == copy.portal && h.1 == copy.exit) else { continue };
            if let Ok(mut t) = mirror_tf.get_mut(copy.root) {
                *t = ghost_transform(&h.2, &root_tf);
            }
            let planes = ClipPlanes { planes: [h.4, DISABLED_PLANE, DISABLED_PLANE, DISABLED_PLANE] };
            for &(s, m) in &copy.pairs {
                if let Ok((_, _, _, tf, ..)) = sources.get(s) {
                    if let Ok(mut t) = mirror_tf.get_mut(m) {
                        *t = *tf;
                    }
                }
                if let Ok(mat) = mirror_mats.get(m) {
                    set_planes(&mut materials, &mat.0, planes);
                }
            }
        }

        // The root's own meshes are clipped at every portal it has passed.
        let mut source_planes = [DISABLED_PLANE; MAX_CLIP_PLANES];
        for (i, h) in hits.iter().take(MAX_CLIP_PLANES).enumerate() {
            source_planes[i] = h.3;
        }
        let source_clip = ClipPlanes { planes: source_planes };
        for e in std::iter::once(root).chain(desc.iter().copied()) {
            if let Ok((_, _, Some(h), ..)) = sources.get(e) {
                set_planes(&mut materials, &h.0, source_clip);
            }
        }

        commands.entity(root).insert(Ghosts { copies: kept });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use p2_core::PortalFrame;

    fn frame_at(pos: Vec3, n: Vec3) -> PortalFrame {
        PortalFrame::from_normal_up(pos, n, Vec3::Y).unwrap()
    }

    #[test]
    fn front_plane_discards_behind() {
        let f = frame_at(Vec3::new(0.0, 1.0, 0.0), Vec3::Z);
        let p = front_plane(&f);
        let eval = |x: Vec3| p.xyz().dot(x) + p.w;
        assert!(eval(Vec3::new(0.0, 1.0, 0.5)) > 0.0);
        assert!(eval(Vec3::new(0.0, 1.0, -0.5)) < 0.0);
    }

    #[test]
    fn pair_maps_front_to_back_of_exit() {
        // A point 0.5 m in front of the entry ends up 0.5 m behind the exit.
        let entry = Transform::from_translation(Vec3::ZERO);
        let exit = Transform::from_translation(Vec3::new(10.0, 0.0, 0.0)).with_rotation(Quat::from_rotation_y(0.3));
        let pair = portal_pair(&entry, &exit);
        let front = pair.map_point(Vec3::new(0.0, 0.0, 0.5));
        let d = Portal::frame(&exit).plane_distance(front);
        assert!((d + 0.5).abs() < 1e-4, "{d}");
    }

    #[test]
    fn overlap_needs_the_opening_and_the_plane() {
        let entry = frame_at(Vec3::ZERO, Vec3::Z);
        let half = Vec2::new(0.8, 1.4);
        let b = GhostBounds { min: Vec3::new(-0.4, 0.0, -0.4), max: Vec3::new(0.4, 1.8, 0.4) };
        // Feet on the plane, in the middle: overlaps.
        assert!(overlaps_opening(&entry, half, &Transform::from_translation(Vec3::new(0.0, 0.0, 0.0)), &b));
        // Well to the side of the hole: no.
        assert!(!overlaps_opening(&entry, half, &Transform::from_translation(Vec3::new(3.0, 0.0, 0.0)), &b));
        // Well in front of the plane: no.
        assert!(!overlaps_opening(&entry, half, &Transform::from_translation(Vec3::new(0.0, 0.0, 1.0)), &b));
        // Centre more than 5 u behind the plane (bounds still touching it): no.
        assert!(!overlaps_opening(&entry, half, &Transform::from_translation(Vec3::new(0.0, 0.0, -0.2)), &b));
    }

    #[test]
    fn clip_plane_keeps_two_units_behind() {
        let f = frame_at(Vec3::ZERO, Vec3::Z);
        let p = clip_plane(&f);
        let eval = |z: f32| p.xyz().dot(Vec3::new(0.0, 0.0, z)) + p.w;
        assert!(eval(-p2_core::u(1.0)) > 0.0 && eval(-p2_core::u(3.0)) < 0.0);
    }
}

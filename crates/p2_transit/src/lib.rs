//! Portal transit for Bevy + avian3d characters: going through a linked portal
//! pair, and walking into the wall behind a portal (Portal 2's rules, from
//! `research/specs/portal-transit.md`).
//!
//! # What it does
//!
//! * **Teleport** (`FixedUpdate`, [`P2Systems::Transit`]): when a character's
//!   hull centre crosses a linked portal's plane from the front, inside the
//!   opening, the character is moved through the pair: position, velocity
//!   (gravity fix-up, exit speed rules), view (exact, roll eased out by the
//!   view code) and render interpolation. [`PortalTeleported`] and, where
//!   Portal 2 forces a crouch, [`ForceDuck`] are sent.
//! * **Region** (same set): while the hull overlaps a portal's hole, the
//!   portal's [`Portal::surfaces`] go into the character's [`IgnoreColliders`],
//!   a ring of [`PortalHelperCollider`]s stands in for the wall around the hole,
//!   and the exit's nearby static geometry is mirrored behind the entrance as
//!   helper colliders. All of it is removed when the character leaves.
//! * **Unstick**: after a teleport, a hull left in solid is moved to a nearby
//!   free spot.
//!
//! # Contract for a character controller
//!
//! Transit works on any entity that has all of: [`Transform`] (translation =
//! feet, rotation = yaw only), [`CharacterVelocity`], [`ViewAngles`],
//! [`Crouch`], [`InterpolatedFeet`] and [`IgnoreColliders`]. Your controller must:
//!
//! 1. Run in [`P2Systems::Move`] in `FixedUpdate`, so transit runs after it
//!    (`P2Systems::Transit` is chained after `Move`).
//! 2. Set `InterpolatedFeet::prev = curr` before moving and `curr` after.
//!    Transit maps both.
//! 3. Honour [`IgnoreColliders`] in every shape cast and overlap test. The set
//!    holds the walls behind the portal you are inside.
//! 4. Treat [`PortalHelperCollider`] like any other collider: it is what keeps
//!    you out of the wall and lets you stand half-way through a portal.
//! 5. Handle [`ForceDuck`] by ducking instantly (Portal 2 air-duck).
//!
//! Add [`Plugin`] and [`p2_core::CorePlugin`] once in your app. Nothing else is
//! spawned by this crate; tune [`TransitConfig`] before adding the plugin, or
//! insert it later to change the values.
pub mod rules;

pub use rules::TransitConfig;

use avian3d::prelude::*;
use bevy::ecs::entity::EntityHashSet;
use bevy::prelude::*;
use p2_core::math::PortalFrame;
use p2_core::{
    CharacterVelocity, Crouch, ForceDuck, IgnoreColliders, InterpolatedFeet, P2Systems, Portal, PortalHelperCollider,
    PortalLink, PortalTeleported, ViewAngles,
};

/// Per-character transit state. Attached automatically to any entity that has
/// the contract components (see the crate docs). Hosts never need to add it.
#[derive(Component, Default, Debug)]
pub struct TransitState {
    /// Portals whose hole the hull overlapped at the last region update.
    pub inside: EntityHashSet,
    /// Colliders this crate added to [`IgnoreColliders`] (only those are removed).
    added: EntityHashSet,
    /// Set by the teleport, read by the unstick system.
    just_teleported: bool,
    /// The exit of the most recent teleport.
    pub last_exit: Option<Entity>,
}

/// Helper colliders this crate spawned for one portal while a character is in
/// its hole. Lives on the portal entity and is removed (with the colliders)
/// when no character is in the hole any more.
#[derive(Component, Default, Debug, Clone)]
pub struct PortalHelpers {
    /// The ring around the hole and its tunnel behind the wall.
    pub ring: Vec<Entity>,
    /// Mirrored exit-side geometry (linked portals with `mirror_exit_geometry`).
    pub remote: Vec<Entity>,
    /// The exit the remote geometry was copied from.
    pub remote_of: Option<Entity>,
}

/// The transit plugin. Add once, after [`p2_core::CorePlugin`].
///
/// Registers [`TransitConfig`] (only if you have not inserted one) and the
/// transit systems in [`P2Systems::Transit`].
pub struct Plugin;

impl bevy::app::Plugin for Plugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TransitConfig>()
            .add_observer(despawn_helpers_on_remove)
            .add_systems(
                FixedUpdate,
                (attach_state, teleport_characters, update_regions, unstick_characters)
                    .chain()
                    .in_set(P2Systems::Transit),
            );
    }
}

/// Filter for characters: entities with every contract component.
type Character = (
    With<InterpolatedFeet>,
    With<ViewAngles>,
    With<CharacterVelocity>,
    With<Crouch>,
    With<IgnoreColliders>,
    // Disjoint from the portal queries that read `Transform` (B0001).
    Without<Portal>,
);

/// Adds [`TransitState`] to any new character.
fn attach_state(mut commands: Commands, new: Query<Entity, (Character, Without<TransitState>)>) {
    for e in &new {
        commands.entity(e).insert(TransitState::default());
    }
}

fn despawn_helpers_on_remove(remove: On<Remove, Portal>, helpers: Query<&PortalHelpers>, mut commands: Commands) {
    let Ok(h) = helpers.get(remove.entity) else { return };
    for &e in h.ring.iter().chain(&h.remote) {
        if let Ok(mut ec) = commands.get_entity(e) {
            ec.despawn();
        }
    }
}

/// A linked pair the character's centre may cross.
struct Candidate {
    entry: Entity,
    exit: Entity,
    entry_frame: PortalFrame,
    exit_frame: PortalFrame,
    exit_half: Vec2,
    dist: f32,
}

#[allow(clippy::type_complexity)]
fn teleport_characters(
    mut characters: Query<
        (Entity, &mut Transform, &mut InterpolatedFeet, &mut ViewAngles, &mut CharacterVelocity, &mut Crouch, &mut TransitState),
        Character,
    >,
    portals: Query<(Entity, &Portal, &Transform, &PortalLink)>,
    exits: Query<(&Portal, &Transform)>,
    time: Res<Time>,
    cfg: Res<TransitConfig>,
    mut teleported: MessageWriter<PortalTeleported>,
    mut ducks: MessageWriter<ForceDuck>,
) {
    let dt = time.delta_secs();
    for (entity, mut tf, mut feet, mut view, mut vel, mut crouch, mut state) in &mut characters {
        let off = crouch.hull_centre_offset();
        let c0 = feet.prev + off;
        let c1 = tf.translation + off;

        // Teleport on the hull centre crossing the plane (§1.2), from the front
        // (§1.1), with the centre's projection inside the opening.
        let mut best: Option<Candidate> = None;
        for (entry_e, entry, entry_tf, link) in &portals {
            let Ok((exit, exit_tf)) = exits.get(link.0) else { continue };
            let frame = Portal::frame(entry_tf);
            let d0 = frame.plane_distance(c0);
            let d1 = frame.plane_distance(c1);
            if !(d0 > 0.0 && d1 < 0.0) {
                continue;
            }
            let l = frame.to_local(c1);
            if l.x.abs() > entry.half_size.x || l.y.abs() > entry.half_size.y {
                continue;
            }
            let dist = (c1 - entry_tf.translation).length();
            if best.as_ref().is_none_or(|b| dist < b.dist) {
                best = Some(Candidate {
                    entry: entry_e,
                    exit: link.0,
                    entry_frame: frame,
                    exit_frame: Portal::frame(exit_tf),
                    exit_half: exit.half_size,
                    dist,
                });
            }
        }
        let Some(c) = best else { continue };

        let x = rules::cross(
            &c.entry_frame,
            &c.exit_frame,
            c.exit_half,
            c0,
            c1,
            vel.0,
            view.rotation(),
            dt,
            &cfg,
        );
        // §1.5/§1.6: a forced crouch happens in this same tick. The mapped hull
        // CENTRE stays put and the feet follow from the crouched hull, so the
        // crouch cannot push the hull back across the exit plane (which would
        // teleport the player straight back, a one-frame flicker). The centre
        // is then nudged to keep the crouched hull inside the opening.
        let mut centre = x.centre;
        let mut prev_centre = x.prev_centre;
        let mut off = off;
        if x.force_duck && !crouch.ducked {
            crouch.ducked = true;
            crouch.eye_height = p2_core::VIEW_DUCK;
            off = crouch.hull_centre_offset();
            // An airborne crouch keeps the head where it is and pulls the feet
            // up (Source in-air duck): raise the centre so the crouched eye sits
            // exactly where the standing eye came out, so the view does not
            // drop. Never toward the exit plane, though (a ceiling exit): that
            // would put the centre back through the portal.
            let raise = Vec3::Y * ((p2_core::VIEW_STAND - p2_core::HULL_STAND.y * 0.5) - (p2_core::VIEW_DUCK - p2_core::HULL_DUCK.y * 0.5));
            let n = c.exit_frame.normal();
            let before = c.exit_frame.plane_distance(centre);
            let mut raised = centre + raise;
            let after = c.exit_frame.plane_distance(raised);
            if after < before {
                raised += n * (before - after);
            }
            prev_centre += raised - centre;
            centre = raised;
            let l = c.exit_frame.to_local(centre);
            let lim = c.exit_half - Vec2::splat(p2_core::u(5.0));
            let clamped = Vec3::new(l.x.clamp(-lim.x, lim.x), l.y.clamp(-lim.y, lim.y), l.z);
            let nudge = c.exit_frame.to_world(clamped) - centre;
            centre += nudge;
            prev_centre += nudge;
        }
        let feet_out = centre - off;
        tf.translation = feet_out;
        tf.rotation = Quat::from_rotation_y(x.view.yaw);
        feet.prev = prev_centre - off;
        feet.curr = feet_out;
        view.yaw = x.view.yaw;
        view.pitch = x.view.pitch;
        view.roll = x.view.roll;
        vel.0 = x.velocity;
        state.just_teleported = true;
        state.last_exit = Some(c.exit);

        if x.force_duck {
            ducks.write(ForceDuck { entity });
        }
        debug!("transit: {entity:?} through {:?} -> {:?}", c.entry, c.exit);
        teleported.write(PortalTeleported {
            entity,
            from_portal: c.entry,
            to_portal: c.exit,
            pair: x.pair,
            velocity_out: x.velocity,
        });
    }
}

/// Hull half extents (world axes) of a crouch state.
fn hull_half(crouch: &Crouch) -> Vec3 {
    crouch.hull() * 0.5
}

/// Works out which holes each character's hull touches, sets its ignores, and
/// adds or removes the helper colliders of those portals.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_regions(
    mut commands: Commands,
    mut characters: Query<(&Transform, &Crouch, &CharacterVelocity, &mut IgnoreColliders, &mut TransitState), Character>,
    portals: Query<(Entity, &Portal, &Transform, Option<&PortalLink>)>,
    helpers: Query<&PortalHelpers>,
    colliders: Query<(Entity, &Collider, &GlobalTransform, Option<&RigidBody>, Has<PortalHelperCollider>)>,
    spatial: SpatialQuery,
    cfg: Res<TransitConfig>,
) {
    // 1. Which holes does each character's hull touch, and what to ignore.
    let mut wanted_portals: EntityHashSet = EntityHashSet::default();
    for (tf, crouch, vel, mut ignore, mut state) in &mut characters {
        let he = hull_half(crouch);
        let centre = tf.translation + crouch.hull_centre_offset();
        let ahead = centre + vel.0 * cfg.region_lookahead;
        let mut inside = EntityHashSet::default();
        let mut want_ignore = EntityHashSet::default();
        for (e, portal, ptf, link) in &portals {
            // An unlinked portal is plain wall: no hole, ring or ignores.
            if link.is_none() {
                continue;
            }
            let frame = Portal::frame(ptf);
            if rules::in_hole(&frame, portal.half_size, centre, he, &cfg)
                || rules::in_hole(&frame, portal.half_size, ahead, he, &cfg)
            {
                inside.insert(e);
                want_ignore.extend(portal.surfaces.iter().copied());
            }
        }
        wanted_portals.extend(inside.iter().copied());

        for e in state.added.iter() {
            if !want_ignore.contains(e) {
                ignore.0.remove(e);
            }
        }
        for &e in want_ignore.iter() {
            ignore.0.insert(e);
        }
        state.added = want_ignore;
        state.inside = inside;
    }

    // 2. Helper colliders for the portals a character is in; remove the rest.
    for (e, portal, ptf, link) in &portals {
        let existing = helpers.get(e).ok();
        if !wanted_portals.contains(&e) {
            if let Some(h) = existing {
                for &c in h.ring.iter().chain(&h.remote) {
                    if let Ok(mut ec) = commands.get_entity(c) {
                        ec.despawn();
                    }
                }
                commands.entity(e).remove::<PortalHelpers>();
            }
            continue;
        }
        let frame = Portal::frame(ptf);
        let mut h = existing.cloned().unwrap_or_default();
        if h.ring.is_empty() {
            for (c, s) in rules::ring_pieces(portal.half_size, &cfg) {
                let pos = frame.to_world(c);
                let id = commands
                    .spawn((
                        Name::new("Portal helper ring"),
                        RigidBody::Static,
                        Collider::cuboid(s.x, s.y, s.z),
                        Transform::from_translation(pos).with_rotation(frame.rotation),
                        Position(pos),
                        Rotation(frame.rotation),
                        PortalHelperCollider { portal: e },
                    ))
                    .id();
                h.ring.push(id);
            }
        }
        let exit = link.map(|l| l.0);
        if h.remote_of != exit {
            for &c in &h.remote {
                if let Ok(mut ec) = commands.get_entity(c) {
                    ec.despawn();
                }
            }
            h.remote.clear();
            h.remote_of = exit;
            if let (Some(exit), true) = (exit, cfg.mirror_exit_geometry) {
                if let Ok((_, exit_portal, exit_tf, _)) = portals.get(exit) {
                    h.remote = mirror_exit_geometry(
                        &mut commands,
                        &spatial,
                        &colliders,
                        e,
                        &frame,
                        portal,
                        exit,
                        &Portal::frame(exit_tf),
                        exit_portal,
                        &cfg,
                    );
                }
            }
        }
        commands.entity(e).insert(h);
    }
}

/// The exit's nearby static geometry in front of the exit, mapped through the
/// inverse pair so it sits behind the entrance (§4 "remote geometry").
///
/// Only cuboids are copied. A cuboid straddling the exit plane is clipped if
/// one of its axes is the plane normal, and otherwise copied only when wholly
/// in front. Other shapes are skipped.
#[allow(clippy::too_many_arguments)]
fn mirror_exit_geometry(
    commands: &mut Commands,
    spatial: &SpatialQuery,
    colliders: &Query<(Entity, &Collider, &GlobalTransform, Option<&RigidBody>, Has<PortalHelperCollider>)>,
    entry_e: Entity,
    entry: &PortalFrame,
    entry_portal: &Portal,
    exit_e: Entity,
    exit: &PortalFrame,
    exit_portal: &Portal,
    cfg: &TransitConfig,
) -> Vec<Entity> {
    let e = cfg.env_radius;
    let hs = exit_portal.half_size;
    let env = Collider::cuboid(2.0 * (hs.x + e), 2.0 * (hs.y + e), 2.0 * e);
    let hits = spatial.shape_intersections(&env, exit.position, exit.rotation, &SpatialQueryFilter::default());
    // Exit-side points to the entrance side: the inverse of entry -> exit.
    let back = p2_core::math::PortalPair::new(*exit, *entry);
    let n = exit.normal();
    let mut out = Vec::new();
    for hit in hits {
        if hit == entry_e || hit == exit_e || entry_portal.surfaces.contains(&hit) || exit_portal.surfaces.contains(&hit) {
            continue;
        }
        let Ok((_, col, gtf, rb, is_helper)) = colliders.get(hit) else { continue };
        if is_helper || rb.is_some_and(|r| !r.is_static()) {
            continue;
        }
        // World pose (colliders may be children of other entities).
        let (_, rot_w, pos_w) = gtf.to_scale_rotation_translation();
        let tf = Transform::from_translation(pos_w).with_rotation(rot_w);
        let tunnel = Tunnel { centre: exit.position - n * (cfg.hole_depth * 0.5), rotation: exit.rotation, half: hs, depth: cfg.hole_depth };
        let Some((c_world, rot_world, shape)) = front_piece(col, &tf, exit.position, n, &tunnel) else { continue };
        let pos = back.map_point(c_world);
        let rot = back.map_rotation(rot_world);
        let id = commands
            .spawn((
                Name::new("Portal remote geometry"),
                RigidBody::Static,
                shape,
                Transform::from_translation(pos).with_rotation(rot),
                Position(pos),
                Rotation(rot),
                PortalHelperCollider { portal: entry_e },
            ))
            .id();
        out.push(id);
    }
    out
}

/// The volume behind an opening that a mirrored collider must stay out of:
/// anything there would block the way into the hole on the entry side.
struct Tunnel {
    centre: Vec3,
    rotation: Quat,
    half: Vec2,
    depth: f32,
}

/// The part of a static collider lying in front of the plane (origin `o`,
/// normal `n`), as (centre, rotation, shape) in world space.
///
/// * Boxes aligned with the plane are clipped exactly to the front part.
/// * Any shape wholly in front is mirrored as is; wholly behind, not at all.
/// * Any other shape straddling the plane (a floor running under the exit
///   wall, a mesh, a rotated box) is mirrored whole if it stays out of the
///   tunnel behind the opening, else skipped.
fn front_piece(col: &Collider, tf: &Transform, o: Vec3, n: Vec3, tunnel: &Tunnel) -> Option<(Vec3, Quat, Collider)> {
    let s = (tf.translation - o).dot(n);
    if let Some(cuboid) = col.shape_scaled().as_cuboid() {
        let half: Vec3 = cuboid.half_extents.into();
        for (k, unit) in [Vec3::X, Vec3::Y, Vec3::Z].into_iter().enumerate() {
            let axis = tf.rotation * unit;
            if axis.dot(n).abs() > 0.999 {
                let ext = half[k];
                let (lo, hi) = (s - ext, s + ext);
                if hi <= 0.0 {
                    return None;
                }
                if lo >= 0.0 {
                    return Some((tf.translation, tf.rotation, col.clone()));
                }
                // Clip to [0, hi] along n.
                let mut h = half;
                h[k] = hi / 2.0;
                let c = tf.translation + n * (hi / 2.0 - s);
                let size = h * 2.0;
                return Some((c, tf.rotation, Collider::cuboid(size.x, size.y, size.z)));
            }
        }
    }
    // General shapes: the extent along n from the world AABB (conservative).
    let aabb = col.aabb(tf.translation, tf.rotation);
    let (mn, mx) = (Vec3::from(aabb.min), Vec3::from(aabb.max));
    let (c, h) = ((mn + mx) * 0.5, (mx - mn) * 0.5);
    let sc = (c - o).dot(n);
    let ext = h.x * n.x.abs() + h.y * n.y.abs() + h.z * n.z.abs();
    if sc + ext <= 0.0 {
        return None;
    }
    if sc - ext >= 0.0 {
        return Some((tf.translation, tf.rotation, col.clone()));
    }
    let tunnel_box = Collider::cuboid(2.0 * tunnel.half.x, 2.0 * tunnel.half.y, tunnel.depth);
    let blocks = avian3d::collision::collider::contact_query::intersection_test(
        col,
        tf.translation,
        tf.rotation,
        &tunnel_box,
        tunnel.centre,
        tunnel.rotation,
    )
    .unwrap_or(true);
    (!blocks).then(|| (tf.translation, tf.rotation, col.clone()))
}

/// After a teleport: if the hull is in solid, move it to the nearest free spot.
///
/// The search is small and documented: first the centre projected onto the
/// exit's normal axis, then `unstick_steps` steps of `unstick_step` metres
/// along the exit's normal, right, and up. If nothing is free, the character
/// stays where the transit put it.
#[allow(clippy::type_complexity)]
fn unstick_characters(
    mut characters: Query<(&mut Transform, &mut InterpolatedFeet, &Crouch, &IgnoreColliders, &mut TransitState), Character>,
    exits: Query<(&Portal, &Transform)>,
    spatial: SpatialQuery,
    cfg: Res<TransitConfig>,
) {
    for (mut tf, mut feet, crouch, ignores, mut state) in &mut characters {
        if !std::mem::take(&mut state.just_teleported) {
            continue;
        }
        let off = crouch.hull_centre_offset();
        let he = hull_half(crouch);
        let centre = tf.translation + off;
        if !hull_blocked(&spatial, centre, he, ignores) {
            continue;
        }
        let Some(exit_frame) = state.last_exit.and_then(|e| exits.get(e).ok()).map(|(_, t)| Portal::frame(t)) else {
            continue;
        };
        let f = exit_frame.normal();
        let r = exit_frame.right();
        let u = exit_frame.up();
        let projected = exit_frame.position + f * (centre - exit_frame.position).dot(f);
        let mut found = None;
        'search: for start in [projected, centre] {
            if !hull_blocked(&spatial, start, he, ignores) {
                found = Some(start);
                break 'search;
            }
            for step in 1..=cfg.unstick_steps {
                let d = step as f32 * cfg.unstick_step;
                for dir in [f, -r, r, u, -u] {
                    let c = start + dir * d;
                    if !hull_blocked(&spatial, c, he, ignores) {
                        found = Some(c);
                        break 'search;
                    }
                }
            }
        }
        if let Some(c) = found {
            let delta = c - centre;
            tf.translation += delta;
            feet.curr += delta;
            feet.prev += delta;
        }
    }
}

/// Is a hull (centre, half extents) overlapping any collider not in `ignores`?
fn hull_blocked(spatial: &SpatialQuery, centre: Vec3, he: Vec3, ignores: &IgnoreColliders) -> bool {
    const EPS: f32 = 0.01;
    let half = he - Vec3::splat(EPS);
    let shape = Collider::cuboid(2.0 * half.x, 2.0 * half.y, 2.0 * half.z);
    spatial
        .shape_intersections(&shape, centre, Quat::IDENTITY, &SpatialQueryFilter::default())
        .into_iter()
        .any(|e| !ignores.contains(&e))
}

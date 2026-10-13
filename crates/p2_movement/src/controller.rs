//! The fixed-tick player move (research/specs/portal-movement.md §1-§9,
//! view-feel.md §5, portal-transit.md §1.6) on avian's continuous shape casts,
//! so flings can't tunnel. Runs in `FixedUpdate` in `P2Systems::Move`.
//!
//! Internal velocity is in Source units per second (u/s, Y-up world); the
//! casts and the public `CharacterVelocity` are metres per second. All time
//! steps use the fixed delta, so the move is dt-correct at any tick rate.
use std::time::Duration;

use avian3d::character_controller::move_and_slide::MoveHitData;
use avian3d::prelude::*;
use bevy::ecs::entity::EntityHashSet;
use bevy::prelude::*;
use p2_core::{
    CharacterVelocity, Crouch, ForceDuck, Grounded, IgnoreColliders, InterpolatedFeet, MoveInput, Player,
    Portal, PortalLink, ViewAngles, HULL_DUCK, HULL_STAND, U, VIEW_DUCK, VIEW_STAND,
};

use crate::config::MovementConfig;
use crate::funnel::{self, FunnelPortal};
use crate::rules::{self, EyeEase, DUCK_SPEED_SCALE, EDGE_PROBE_AHEAD, EDGE_PROBE_DEPTH, FEET_RISE, GROUND_CHECK, NON_JUMP_VELOCITY, SKIN};

/// Crouch and jump bookkeeping, private to the controller.
#[derive(Component, Clone, Debug)]
pub(crate) struct Duck {
    /// Eye height above the feet (m), eased. Mirrored into `Crouch::eye_height`.
    pub eye: EyeEase,
    /// Airborne from a jump: crouch input is ignored (prevent_crouch_jump, §4).
    pub jumped: bool,
    /// Steep surface normals touched last tick: the air wish may not push into them.
    pub steep: Vec<Vec3>,
}

impl Default for Duck {
    fn default() -> Self {
        Self { eye: EyeEase::standing(), jumped: false, steep: Vec::new() }
    }
}

/// Collision helper for one character. Every cast and overlap uses `filter`,
/// which skips the entities in `IgnoreColliders`.
struct Mover<'a, 'w, 's> {
    mas: &'a MoveAndSlide<'w, 's>,
    filter: SpatialQueryFilter,
    config: MoveAndSlideConfig,
    stand: &'a Collider,
    duck: &'a Collider,
    cfg: MovementConfig,
    /// Every body the hull slid against this tick, with the hit normal.
    hits: std::cell::RefCell<Vec<(Entity, Vec3)>>,
}

fn half_height(ducked: bool) -> f32 {
    0.5 * if ducked { HULL_DUCK.y } else { HULL_STAND.y }
}

impl Mover<'_, '_, '_> {
    fn shape(&self, ducked: bool) -> &Collider {
        if ducked {
            self.duck
        } else {
            self.stand
        }
    }

    /// Collide-and-slide `vel` (m/s) for `dt` from `feet`. Returns the new
    /// feet and the velocity after clipping. Touched normals go into `touched`.
    fn slide(&self, ducked: bool, feet: Vec3, vel: Vec3, dt: f32, touched: &mut Vec<Vec3>) -> (Vec3, Vec3) {
        let centre = feet + Vec3::Y * half_height(ducked);
        let out = self.mas.move_and_slide(
            self.shape(ducked),
            centre,
            Quat::IDENTITY,
            vel,
            Duration::from_secs_f32(dt),
            &self.config,
            &self.filter,
            |hit| {
                touched.push(hit.normal.as_vec3());
                self.hits.borrow_mut().push((hit.entity, hit.normal.as_vec3()));
                MoveAndSlideHitResponse::Accept
            },
        );
        (out.position - Vec3::Y * half_height(ducked), out.projected_velocity)
    }

    /// Sweep the hull from `feet` by `delta` (m).
    fn cast(&self, ducked: bool, feet: Vec3, delta: Vec3) -> Option<MoveHitData> {
        self.cast_shape(self.shape(ducked), feet + Vec3::Y * half_height(ducked), delta)
    }

    /// Sweep a hull shape centred at `centre` by `delta` (m).
    fn cast_shape(&self, shape: &Collider, centre: Vec3, delta: Vec3) -> Option<MoveHitData> {
        self.mas.cast_move(shape, centre, Quat::IDENTITY, delta, SKIN, &self.filter)
    }

    /// Does the hull at `feet` overlap anything, beyond a hair?
    fn blocked(&self, ducked: bool, feet: Vec3) -> bool {
        let mut hit = false;
        self.mas.intersections(
            self.shape(ducked),
            feet + Vec3::Y * half_height(ducked),
            Quat::IDENTITY,
            0.0,
            &self.filter,
            |_, cp, _| {
                if cp.penetration > 1e-3 {
                    hit = true;
                    false
                } else {
                    true
                }
            },
        );
        hit
    }
}

/// Ground check (CategorizePosition, §1): when a walkable floor is within 2 u
/// and we are not rising fast, snap onto it. Returns the floor entity and normal.
fn categorize(m: &Mover, ducked: bool, feet: &mut Vec3, v: Vec3) -> Option<(Entity, Vec3)> {
    if v.y > NON_JUMP_VELOCITY {
        return None;
    }
    let hit = m.cast(ducked, *feet, Vec3::NEG_Y * GROUND_CHECK * U)?;
    if hit.normal1.y >= m.cfg.min_walk_normal {
        *feet -= Vec3::Y * hit.distance;
        Some((hit.entity, hit.normal1))
    } else {
        None
    }
}

/// Edge friction probe (§3): true when there is no floor under a point
/// 16 u ahead of the feet along the horizontal velocity (standing hull, 49 u down).
fn near_edge(m: &Mover, feet: Vec3, v: Vec3) -> bool {
    let dir = Vec3::new(v.x, 0.0, v.z).normalize_or_zero();
    if dir == Vec3::ZERO {
        return false;
    }
    let centre = feet + Vec3::Y * (half_height(false) + U) + dir * (EDGE_PROBE_AHEAD * U);
    m.cast_shape(m.stand, centre, Vec3::NEG_Y * EDGE_PROBE_DEPTH * U).is_none()
}

/// Walk move (§3, StepMove): slide along the ground. If something blocked us,
/// also try stepping up `step_height` and keep whichever went farther.
fn walk_move(m: &Mover, ducked: bool, feet: Vec3, vel: Vec3, dt: f32, touched: &mut Vec<Vec3>) -> (Vec3, Vec3) {
    let mut step_touched = Vec::new();
    let (down_pos, down_vel) = m.slide(ducked, feet, vel, dt, touched);
    let wanted = feet + vel * dt;
    if down_pos.distance(wanted) < 1e-4 {
        return (down_pos, down_vel);
    }
    let step = m.cfg.step_height * U;
    let up = m.cast(ducked, feet, Vec3::Y * step).map_or(step, |h| h.distance);
    if up <= 1e-4 {
        return (down_pos, down_vel);
    }
    let (over_pos, over_vel) = m.slide(ducked, feet + Vec3::Y * up, Vec3::new(vel.x, 0.0, vel.z), dt, &mut step_touched);
    let Some(hit) = m.cast(ducked, over_pos, Vec3::NEG_Y * (up + 0.01)) else {
        return (down_pos, down_vel);
    };
    if hit.normal1.y < m.cfg.min_walk_normal {
        return (down_pos, down_vel);
    }
    let step_pos = over_pos - Vec3::Y * hit.distance;
    let d_down = Vec2::new(down_pos.x - feet.x, down_pos.z - feet.z).length_squared();
    let d_step = Vec2::new(step_pos.x - feet.x, step_pos.z - feet.z).length_squared();
    if d_down >= d_step {
        (down_pos, down_vel)
    } else {
        *touched = step_touched;
        (step_pos, Vec3::new(over_vel.x, down_vel.y, over_vel.z))
    }
}

/// Keep ground contact when walking down a step or slope, instead of hopping.
fn stay_on_ground(m: &Mover, ducked: bool, feet: &mut Vec3) {
    let lift = 2.0 * U;
    let from = match m.cast(ducked, *feet, Vec3::Y * lift) {
        Some(h) => *feet + Vec3::Y * h.distance,
        None => *feet + Vec3::Y * lift,
    };
    let reach = (from.y - feet.y) + m.cfg.step_height * U;
    if let Some(h) = m.cast(ducked, from, Vec3::NEG_Y * reach) {
        if h.distance > 0.0 && h.normal1.y >= m.cfg.min_walk_normal {
            *feet = from - Vec3::Y * h.distance;
        }
    }
}

/// Crouch state machine. `intent` is the duck input; `force` is a
/// [`ForceDuck`] this tick, which completes the crouch at once.
///
/// * On the ground the eye lowers over `duck_time` and the hull switches once
///   the eye has finished; the feet stay planted.
/// * In the air the hull switches at once and the feet rise 36 u (head planted).
/// * Standing up needs room: on the ground at the same feet; in the air with
///   the feet dropping back 36 u. If blocked, the player stays crouched.
fn duck_step(crouch: &mut Crouch, duck: &mut Duck, m: &Mover, feet: &mut Vec3, grounded: bool, intent: bool, force: bool, dt: f32) {
    crouch.wants_duck = intent;
    // A forced crouch (portal transit, §1.6) finishes in this tick, like
    // Portal 2's finish-duck step: hull crouched, eye straight at the crouched
    // height. Standing back up is only tried from the next tick.
    if force {
        if !crouch.ducked {
            crouch.ducked = true;
            if !grounded {
                *feet += Vec3::Y * FEET_RISE;
            }
        }
        duck.eye = EyeEase { from: VIEW_DUCK, to: VIEW_DUCK, elapsed: 0.0, dur: 0.0 };
        crouch.eye_height = VIEW_DUCK;
        return;
    }
    let mut feet_delta = 0.0;
    if !crouch.ducked {
        if intent && !grounded {
            crouch.ducked = true;
            if !grounded {
                feet_delta = FEET_RISE;
            }
        } else if intent && duck.eye.to == VIEW_DUCK && duck.eye.done() {
            crouch.ducked = true;
        }
    } else if !intent {
        if grounded {
            if !m.blocked(false, *feet) {
                crouch.ducked = false;
            }
        } else {
            let lower = *feet - Vec3::Y * FEET_RISE;
            if !m.blocked(false, lower) {
                crouch.ducked = false;
                feet_delta = -FEET_RISE;
            } else if !m.blocked(false, *feet) {
                crouch.ducked = false;
            }
        }
    }
    *feet += Vec3::Y * feet_delta;

    let target = if intent || crouch.ducked { VIEW_DUCK } else { VIEW_STAND };
    duck.eye.retarget(target, m.cfg.duck_time, m.cfg.unduck_time);
    duck.eye.shift(feet_delta);
    duck.eye.advance(dt);
    crouch.eye_height = duck.eye.value();
}

/// The fixed-tick move for every `Player`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn move_players(
    time: Res<Time>,
    cfg: Res<MovementConfig>,
    mas: MoveAndSlide,
    portal_q: Query<&Transform, (With<Portal>, With<PortalLink>)>,
    mut forced: MessageReader<ForceDuck>,
    mut players: Query<
        (
            Entity,
            &mut Transform,
            &mut CharacterVelocity,
            &IgnoreColliders,
            &ViewAngles,
            &mut MoveInput,
            &mut Crouch,
            &mut Grounded,
            &mut InterpolatedFeet,
            &mut Duck,
        ),
        (With<Player>, Without<Portal>),
    >,
    mut bodies: Query<(&RigidBody, &mut LinearVelocity), Without<Player>>,
) {
    let dt = time.delta_secs();
    let forced: EntityHashSet = forced.read().map(|m| m.entity).collect();
    if dt <= 0.0 {
        return;
    }
    let cfg = *cfg;
    let stand = Collider::cuboid(HULL_STAND.x, HULL_STAND.y, HULL_STAND.z);
    let duck_shape = Collider::cuboid(HULL_DUCK.x, HULL_DUCK.y, HULL_DUCK.z);
    let portals: Vec<FunnelPortal> = if cfg.funnel_into_portals {
        portal_q.iter().map(FunnelPortal::from_transform).collect()
    } else {
        Vec::new()
    };

    for (entity, mut tf, mut vel, ignore, view, mut input, mut crouch, mut grounded, mut interp, mut duck) in
        &mut players
    {
        let mover = Mover {
            mas: &mas,
            filter: SpatialQueryFilter::from_excluded_entities(ignore.0.iter().copied()),
            config: MoveAndSlideConfig { skin_width: SKIN, move_and_slide_iterations: 6, ..default() },
            stand: &stand,
            duck: &duck_shape,
            cfg,
            hits: Default::default(),
        };
        // The velocity the hull was moved with this tick (for pushing props).
        let push_v: Vec3;

        interp.prev = interp.curr;
        let mut feet = tf.translation;
        let mut v = vel.0 / U;
        let jump_pressed = std::mem::take(&mut input.jump);

        // Ground state at the start of the tick.
        let mut floor = categorize(&mover, crouch.ducked, &mut feet, v);

        // Duck. Crouch input is ignored while airborne from a jump. A forced
        // crouch (portal transit §1.6) is not held: like Source, the player
        // stands back up as soon as there is room (next tick in the air).
        let force = forced.contains(&entity);
        let intent = input.duck && !duck.jumped;
        duck_step(&mut crouch, &mut duck, &mover, &mut feet, floor.is_some(), intent, force, dt);

        // Jump (§4): not while the duck is in progress.
        let ducking_now = crouch.wants_duck && !crouch.ducked;
        let mut jumped_now = false;
        if jump_pressed && floor.is_some() && !ducking_now {
            v.y = cfg.jump_speed() - cfg.gravity * dt * 0.5;
            floor = None;
            duck.jumped = true;
            jumped_now = true;
        }

        let mut touched = Vec::new();
        if floor.is_some() {
            // Ground move: friction, accelerate, walk and step.
            v.y = 0.0;
            let edge = near_edge(&mover, feet, v);
            v = rules::friction(v, rules::friction_factor(&cfg, edge), cfg.stop_speed, dt);
            let mut wish = rules::wish_velocity(input.wish, view.yaw, 1.0, cfg.max_speed);
            if crouch.ducked || crouch.wants_duck {
                wish *= DUCK_SPEED_SCALE;
            }
            let wish_len = wish.length();
            if wish_len > 1e-4 {
                v = rules::accelerate(v, wish / wish_len, wish_len, cfg.accelerate, dt);
            }
            v = rules::clamp_velocity(v, cfg.max_velocity);
            push_v = v * U;
            let (f, nv) = walk_move(&mover, crouch.ducked, feet, v * U, dt, &mut touched);
            feet = f;
            v = nv / U;
            stay_on_ground(&mover, crouch.ducked, &mut feet);
        } else {
            // Air move (§5, §6): split-step gravity, funnel or air lock, air accel, slide.
            if !jumped_now {
                v.y -= cfg.gravity * dt * 0.5;
            }
            let mut wish = rules::wish_velocity(input.wish, view.yaw, rules::air_forward_scale(view.pitch), cfg.max_speed);
            let mut funnel_add = Vec3::ZERO;
            if Vec2::new(v.x, v.z).length() <= cfg.air_lock_speed {
                if !portals.is_empty() {
                    let centre = feet + Vec3::Y * half_height(crouch.ducked);
                    let mut vm = v * U;
                    funnel_add = funnel::air_funnel(cfg.gravity, &portals, centre, &mut vm, wish * U, view.forward(), dt) / U;
                    v = vm / U;
                }
            } else {
                wish = rules::air_lock(wish, v, cfg.air_lock_speed, cfg.air_lock_axis);
            }
            let mut total = wish + funnel_add;
            total.y = 0.0;
            for n in &duck.steep {
                total = rules::no_push_into(total, *n);
            }
            let total_len = total.length();
            if total_len > 1e-4 {
                v = rules::air_accelerate(v, total / total_len, total_len.min(cfg.max_speed), cfg.air_accelerate, cfg.air_speed_cap, dt);
            }
            v = rules::clamp_velocity(v, cfg.max_velocity);
            push_v = v * U;
            let (f, nv) = mover.slide(crouch.ducked, feet, v * U, dt, &mut touched);
            feet = f;
            v = nv / U;
        }

        duck.steep.clear();
        duck.steep.extend(touched.into_iter().filter(|n| n.y > 0.01 && n.y < cfg.min_walk_normal));

        // Land or fall: second half of split-step gravity (§7).
        floor = categorize(&mover, crouch.ducked, &mut feet, v);
        if floor.is_some() {
            v.y = 0.0;
            duck.jumped = false;
        } else {
            v.y -= cfg.gravity * dt * 0.5;
        }
        v = rules::clamp_velocity(v, cfg.max_velocity);

        tf.translation = feet;
        tf.rotation = Quat::from_rotation_y(view.yaw);
        vel.0 = v * U;
        interp.curr = feet;
        grounded.on_ground = floor.is_some();
        grounded.normal = floor.map_or(Vec3::Y, |(_, n)| n);
        grounded.entity = floor.map(|(e, _)| e);

        // Push physics props walked into, as Source's player shadow does: a
        // dynamic body hit from the side moves at least as fast as the player
        // along the push direction (floors and ceilings are not pushed).
        for (e, n) in mover.hits.take() {
            let Ok((body, mut lv)) = bodies.get_mut(e) else { continue };
            if !body.is_dynamic() {
                continue;
            }
            let dir = Vec3::new(-n.x, 0.0, -n.z);
            if dir.length() < 0.5 {
                continue;
            }
            let dir = dir.normalize();
            let into = push_v.dot(dir);
            let cur = lv.0.dot(dir);
            if into > cur {
                lv.0 += dir * (into - cur);
            }
        }
    }
}

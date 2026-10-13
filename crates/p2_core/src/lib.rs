//! The contract between the `p2_*` crates: the types, messages and system
//! order they all agree on. Every `p2_*` crate depends on it.
//!
//! # Conventions
//!
//! * World space is Bevy's: Y-up, right-handed, metres. Source is Z-up
//!   inches: convert with [`source_to_world`] / [`u`]. Specs in
//!   `research/specs/*.md` are in Source units; keep a `* U` (or [`u`]) next to
//!   every constant so the original number stays readable.
//! * Simulation runs in `FixedUpdate` (Portal 2's tick is 60 Hz, [`TICK_HZ`];
//!   opt in with [`P2Settings::set_fixed_timestep`]); rendering in `Update`/`PostUpdate` interpolates with
//!   [`InterpolatedFeet`].
//! * The player entity's `Transform.translation` is the **feet** (Source
//!   origin: bottom centre of the hull). Its rotation is yaw only. The view
//!   direction lives in [`ViewAngles`]. The first-person camera is a separate
//!   top-level entity owned by `p2_view` (not a child).
//! * A portal's `Transform` is its frame ([`math::PortalFrame`]): centre of
//!   the opening, local +Z out of the surface, local +Y up along the portal,
//!   unit scale, top-level entity.
use bevy::prelude::*;

pub mod math;

pub use math::{PortalFrame, PortalPair, PORTAL_HALF_SIZE};
pub use shared::{
    source_dir_to_world, source_to_world, CharacterVelocity, EyeOffset, IgnoreColliders, Player, Portalable,
    SOURCE_UNIT,
};

/// Source units to metres (scalar).
pub const fn u(units: f32) -> f32 {
    units * SOURCE_UNIT
}

/// Shorthand: metres per Source unit.
pub const U: f32 = SOURCE_UNIT;

/// Portal 2's server tick rate.
pub const TICK_HZ: f64 = 60.0;

// ------------------------------------------------------------------ player --

/// Standing hull: 32 x 32 x 72 u, origin at the feet (research/specs/portal-movement.md §6 [P]).
pub const HULL_STAND: Vec3 = Vec3::new(32.0 * U, 72.0 * U, 32.0 * U);
/// Crouched hull: 32 x 32 x 36 u.
pub const HULL_DUCK: Vec3 = Vec3::new(32.0 * U, 36.0 * U, 32.0 * U);
/// Eye height standing (64 u) and crouched (28 u) above the feet.
pub const VIEW_STAND: f32 = 64.0 * U;
pub const VIEW_DUCK: f32 = 28.0 * U;

/// Where the player looks, radians. Yaw 0 looks down world -Z, positive yaw
/// turns left (counter-clockwise seen from above), positive pitch looks up.
/// `roll` is the left-over roll after a portal transit; `p2_view` eases it
/// back to 0 (half-life 0.125 s, at least 10 deg/s; portal-transit.md).
///
/// Writers: `p2_movement` (mouse look, every frame), `p2_transit` (on
/// teleport). Readers: everyone.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct ViewAngles {
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
}

impl ViewAngles {
    /// Full view rotation (yaw, then pitch, then roll).
    pub fn rotation(&self) -> Quat {
        Quat::from_euler(EulerRot::YXZ, self.yaw, self.pitch, self.roll)
    }
    /// Unit forward vector of the view.
    pub fn forward(&self) -> Vec3 {
        self.rotation() * Vec3::NEG_Z
    }
    /// Body rotation (yaw only).
    pub fn body_rotation(&self) -> Quat {
        Quat::from_rotation_y(self.yaw)
    }
    /// Recover yaw/pitch/roll from a full view rotation.
    pub fn from_rotation(r: Quat) -> Self {
        let (yaw, pitch, roll) = r.to_euler(EulerRot::YXZ);
        Self { yaw, pitch, roll }
    }
}

/// Crouch state, owned by `p2_movement`. `eye_height` is the smoothed eye
/// height above the feet (metres) that `p2_view` places the camera at
/// (duck 0.4 s, unduck 0.2 s, smoothstep; view-feel.md §5).
#[derive(Component, Clone, Copy, Debug)]
pub struct Crouch {
    /// The hull is the crouched one.
    pub ducked: bool,
    /// The duck button (or a forced duck) is held.
    pub wants_duck: bool,
    /// Smoothed eye height above the feet, metres.
    pub eye_height: f32,
}

impl Default for Crouch {
    fn default() -> Self {
        Self { ducked: false, wants_duck: false, eye_height: VIEW_STAND }
    }
}

impl Crouch {
    pub fn hull(&self) -> Vec3 {
        if self.ducked {
            HULL_DUCK
        } else {
            HULL_STAND
        }
    }
    /// Hull centre relative to the feet.
    pub fn hull_centre_offset(&self) -> Vec3 {
        Vec3::Y * self.hull().y * 0.5
    }
}

/// Ground state, owned by `p2_movement`.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct Grounded {
    pub on_ground: bool,
    /// Ground normal when on ground.
    pub normal: Vec3,
    /// The collider stood on.
    pub entity: Option<Entity>,
}

/// Feet position at the previous and the current fixed tick, for render
/// interpolation. `p2_movement` sets `prev = curr` before its move and
/// `curr` after. `p2_transit` maps **both** through the portal on a
/// teleport so interpolation continues on the exit side without a jump.
/// `p2_view` and `p2_body` draw at `prev.lerp(curr, overstep_fraction)`.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct InterpolatedFeet {
    pub prev: Vec3,
    pub curr: Vec3,
}

impl InterpolatedFeet {
    pub fn at(&self, alpha: f32) -> Vec3 {
        self.prev.lerp(self.curr, alpha)
    }
}

/// Sent by `p2_transit` when the player must crouch because of a transit
/// (portal-transit.md §1.6). `p2_movement` ducks instantly (air duck).
#[derive(Message, Clone, Copy, Debug)]
pub struct ForceDuck {
    pub entity: Entity,
}

/// Movement input for this tick, written by `p2_movement`'s input system
/// (keyboard) and readable by others (e.g. the sandbox script driver can
/// overwrite it to drive the player).
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct MoveInput {
    /// x = right, y = forward, each -1..1.
    pub wish: Vec2,
    pub jump: bool,
    pub duck: bool,
    pub use_key: bool,
}

/// Set to disable all keyboard/mouse reading (scripted runs, tests).
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct InputLocked(pub bool);

/// Spawn the player with every contract component: feet at `feet`, facing
/// `yaw`, a full portal gun. Crates add their own private components to it
/// with `#[require]` hooks or an `Added<Player>` system; examples and the
/// app call this and nothing else.
pub fn spawn_player(commands: &mut Commands, feet: Vec3, yaw: f32) -> Entity {
    commands
        .spawn((
            Name::new("player"),
            Player,
            Transform::from_translation(feet).with_rotation(Quat::from_rotation_y(yaw)),
            Visibility::default(),
            ViewAngles { yaw, pitch: 0.0, roll: 0.0 },
            Crouch::default(),
            Grounded::default(),
            CharacterVelocity::default(),
            InterpolatedFeet { prev: feet, curr: feet },
            MoveInput::default(),
            IgnoreColliders::default(),
            EyeOffset(Vec3::Y * VIEW_STAND),
            PortalGun::full(),
            ViewmodelAttachments::default(),
        ))
        .id()
}

/// A collider that only exists to help things pass through portals (the
/// tunnel/ring `p2_transit` builds around an opening). Shots pass through
/// it; characters collide with it normally.
#[derive(Component, Clone, Copy, Debug)]
pub struct PortalHelperCollider {
    pub portal: Entity,
}

// ------------------------------------------------------------------ portals --

/// Which half of a pair. Blue is primary (left click), orange secondary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Reflect)]
pub enum PortalKind {
    Blue,
    Orange,
}

impl PortalKind {
    pub fn index(self) -> usize {
        match self {
            PortalKind::Blue => 0,
            PortalKind::Orange => 1,
        }
    }
    pub fn other(self) -> Self {
        match self {
            PortalKind::Blue => PortalKind::Orange,
            PortalKind::Orange => PortalKind::Blue,
        }
    }
    /// Single-player colour, 0-255 RGB (portal-rendering.md §3): blue
    /// (0, 60, 255), orange (233, 78, 2). Used for particle CPs and the HUD.
    pub fn rgb255(self) -> Vec3 {
        match self {
            PortalKind::Blue => Vec3::new(0.0, 60.0, 255.0),
            PortalKind::Orange => Vec3::new(233.0, 78.0, 2.0),
        }
    }
}

/// A placed portal. Spawned/despawned by `p2_gun` only.
#[derive(Component, Clone, Debug)]
#[require(Transform, Visibility, PortalAnimation)]
pub struct Portal {
    pub kind: PortalKind,
    /// Half extents of the opening (metres), normally [`PORTAL_HALF_SIZE`].
    pub half_size: Vec2,
    /// The collider(s) it sits on (the wall behind it).
    pub surfaces: Vec<Entity>,
    /// `Time::elapsed_secs()` when it was placed.
    pub placed_at: f32,
}

impl Portal {
    pub fn frame(transform: &Transform) -> PortalFrame {
        PortalFrame::new(transform.translation, transform.rotation)
    }
}

/// The portal this one leads to. Set on both halves of a pair by `p2_gun`;
/// only linked portals render a view and let things through.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct PortalLink(pub Entity);

/// The pair transform for travelling from `entry` to `exit` (both portal
/// transforms): enter the front of `entry`, come out of the front of `exit`.
pub fn portal_pair(entry: &Transform, exit: &Transform) -> PortalPair {
    PortalPair::new(Portal::frame(entry), Portal::frame(exit))
}

/// Shader inputs of a portal (Portal 2's `PortalOpenAmount` and
/// `PortalStatic` proxies). Animated by [`CorePlugin`].
#[derive(Component, Clone, Copy, Debug)]
pub struct PortalAnimation {
    /// 0 when placed, rising linearly to 1 over [`PORTAL_OPEN_TIME`].
    pub open_amount: f32,
    /// 1 while unlinked; reset to 1 on (re)link and falls 1 per second.
    pub static_amount: f32,
}

impl Default for PortalAnimation {
    fn default() -> Self {
        Self { open_amount: 0.0, static_amount: 1.0 }
    }
}

pub const PORTAL_OPEN_TIME: f32 = 0.5;
pub const PORTAL_STATIC_FADE_TIME: f32 = 1.0;

/// The portal gun. Lives on the player entity. `p2_gun` owns all writes.
#[derive(Component, Clone, Debug, Default)]
pub struct PortalGun {
    /// Currently open portals, indexed by [`PortalKind::index`].
    pub portals: [Option<Entity>; 2],
    /// Colour of the last successful placement (viewmodel top light, HUD pip).
    pub last_placed: Option<PortalKind>,
    /// Which shots this gun can fire (Portal 2 starts with blue only in Ch1).
    pub can_fire: [bool; 2],
}

impl PortalGun {
    pub fn full() -> Self {
        Self { can_fire: [true, true], ..default() }
    }
    pub fn portal(&self, kind: PortalKind) -> Option<Entity> {
        self.portals[kind.index()]
    }
}

/// Ask a gun to fire (mouse input or a script).
#[derive(Message, Clone, Copy, Debug)]
pub struct FirePortal {
    pub gun: Entity,
    pub kind: PortalKind,
}

/// A shot left the gun (valid or not). Drives the viewmodel `fire` anim,
/// muzzle flash, fire sound and the projectile visual.
#[derive(Message, Clone, Copy, Debug)]
pub struct GunFired {
    pub gun: Entity,
    pub kind: PortalKind,
    /// Muzzle-ish start (the eye) and where the shot ended (hit point, or
    /// far along the ray on a miss).
    pub from: Vec3,
    pub to: Vec3,
}

/// A portal was opened (after any replaced one was closed).
#[derive(Message, Clone, Copy, Debug)]
pub struct PortalPlaced {
    pub gun: Entity,
    pub portal: Entity,
    pub kind: PortalKind,
    pub transform: Transform,
}

/// A portal closed (replaced by a new shot, or fizzled). For close particles/sounds.
#[derive(Message, Clone, Copy, Debug)]
pub struct PortalClosed {
    pub kind: PortalKind,
    pub transform: Transform,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShotFailReason {
    /// Hit nothing within range.
    NoHit,
    /// Hit a surface without [`Portalable`] (or one that isn't portalable for another reason).
    NotPortalable,
    /// Portalable, but the opening could not be fitted (too small, edge bump failed).
    DoesNotFit,
    /// Would overlap the other portal.
    Overlap,
}

/// A shot did not make a portal. Drives `portal_badsurface` / fizzle sound.
#[derive(Message, Clone, Copy, Debug)]
pub struct PortalShotFailed {
    pub gun: Entity,
    pub kind: PortalKind,
    pub reason: ShotFailReason,
    /// Where the shot hit, with the surface normal there.
    pub point: Option<Vec3>,
    pub normal: Option<Vec3>,
}

/// Something went through a portal. Sent by `p2_transit` in the same tick.
#[derive(Message, Clone, Copy, Debug)]
pub struct PortalTeleported {
    pub entity: Entity,
    pub from_portal: Entity,
    pub to_portal: Entity,
    /// The pair transform that was applied.
    pub pair: PortalPair,
    pub velocity_out: Vec3,
}

// --------------------------------------------------------------- rendering --

/// Things drawn in the main view only, never through portals (e.g. particle
/// systems whose renderer says "cull starting at recursion depth 1"). The
/// main camera must render it (`p2_view` adds it; a host camera should too).
pub const LAYER_MAIN_VIEW_ONLY: usize = 25;
/// Layer of the first-person viewmodel (its own camera, main view only).
pub const LAYER_VIEWMODEL: usize = 26;
/// Layer of the local player's third-person body. `p2_render` picks which
/// cameras draw it from [`LocalEye`] (player-in-portals.md §2).
pub const LAYER_PLAYER_BODY: usize = 27;
/// Layer of the local player's ghost copies made by the **blue** portal (the
/// body overlaps blue's opening; the copy comes out of orange). `p2_body`
/// draws them, `p2_render` picks the cameras (player-in-portals.md §3).
pub const LAYER_PLAYER_GHOST: usize = 24;
/// Like [`LAYER_PLAYER_GHOST`], for ghosts made by the **orange** portal.
pub const LAYER_PLAYER_GHOST_ORANGE: usize = 23;

/// The layer of the local player's ghosts made by a portal of this kind.
pub const fn player_ghost_layer(kind: PortalKind) -> usize {
    match kind {
        PortalKind::Blue => LAYER_PLAYER_GHOST,
        PortalKind::Orange => LAYER_PLAYER_GHOST_ORANGE,
    }
}

/// How the local player is being viewed this frame, which decides where its
/// body and ghosts are drawn (player-in-portals.md §2-3). Written by `p2_view`
/// in `P2Systems::View`, read by `p2_render`.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LocalEye {
    /// The main camera is not at the player's eye (third person): the body
    /// and ghosts are drawn in every view.
    pub third_person: bool,
    /// The eye has gone through a linked portal ahead of the body, so the
    /// main camera renders from the mapped pose; this is the portal it now
    /// looks out of (the exit).
    pub through: Option<Entity>,
}

/// If `eye` is behind a linked portal's plane (by less than a metre) and
/// inside its opening, the eye mapped through that portal and the exit portal
/// it now looks out of. `portals` yields (portal transform, portal, exit
/// entity, exit transform) for every linked portal.
pub fn eye_through_portal<'a>(
    eye: Transform,
    portals: impl IntoIterator<Item = (&'a Transform, &'a Portal, Entity, &'a Transform)>,
) -> Option<(Transform, Entity)> {
    portals.into_iter().find_map(|(tf, portal, exit, exit_tf)| {
        let l = Portal::frame(tf).to_local(eye.translation);
        let inside = l.z < 0.0 && l.z > -1.0 && l.x.abs() < portal.half_size.x && l.y.abs() < portal.half_size.y;
        inside.then(|| {
            let pair = portal_pair(tf, exit_tf);
            (Transform::from_translation(pair.map_point(eye.translation)).with_rotation(pair.map_rotation(eye.rotation)), exit)
        })
    })
}
/// Layers 28 and up belong to `p2_render` (portal surfaces per recursion level).
pub const LAYER_RENDER_FIRST: usize = 28;

/// The main first-person camera (spawned by `p2_view`). Exactly one.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct MainCamera;

/// A virtual camera looking through `portal` at recursion `level` (1 = seen
/// directly through a portal in the main view). Spawned by `p2_render`.
#[derive(Component, Clone, Copy, Debug)]
pub struct PortalViewCamera {
    pub portal: Entity,
    pub level: u32,
}

/// World-space poses of the first-person gun's attachments this frame,
/// written by `p2_view` (in `P2Systems::View`) on the player entity, read by
/// `p2_fx` for the first-person muzzle flash and the top light. Effects
/// drawn there go on [`LAYER_VIEWMODEL`] (the viewmodel camera only).
/// Positions are in the viewmodel camera's world space: the same as the main
/// camera's, because both cameras share the eye transform.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct ViewmodelAttachments {
    pub muzzle: Option<Transform>,
    pub top_light: Option<Transform>,
    /// `Beam_point1` / `Beam_point5`: the top light's control points 3 and 4
    /// (its core glow is emitted at and locked to point 1).
    pub beam_point1: Option<Transform>,
    pub beam_point5: Option<Transform>,
}

/// Recursion depth (`r_portal_stencil_depth` 2).
pub const PORTAL_RECURSION_DEPTH: u32 = 2;

// ------------------------------------------------------------------ order --

/// Frame/tick order. In `FixedUpdate`: `Move` then `Transit`.
/// In `Update`: `Input` then `Fire`.
/// In `PostUpdate` (before transform propagation): `View` (main camera
/// pose) then `Body` then `Render` (portal cameras/surfaces) then `Effects`.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum P2Systems {
    Input,
    Fire,
    Move,
    Transit,
    View,
    Body,
    Render,
    Effects,
}

/// Host-app options for [`CorePlugin`]. Insert before adding the plugin to
/// change them; the defaults never touch the host's global settings.
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct P2Settings {
    /// Set `Time<Fixed>` to Portal 2's 60 Hz tick ([`TICK_HZ`]). Off by
    /// default so a host keeps its own fixed rate; movement is dt-correct
    /// at any rate, and exactly Portal 2's numbers at 60 Hz.
    pub set_fixed_timestep: bool,
}

/// Registers the messages, resources, set ordering and portal animation.
/// Every `p2_*` plugin may assume it is added (add it in your example too).
pub struct CorePlugin;

impl Plugin for CorePlugin {
    fn build(&self, app: &mut App) {
        use bevy::transform::TransformSystems;
        let settings = *app.world_mut().get_resource_or_insert_with(P2Settings::default);
        if settings.set_fixed_timestep {
            app.insert_resource(Time::<Fixed>::from_hz(TICK_HZ));
        }
        app.init_resource::<InputLocked>()
            .init_resource::<LocalEye>()
            .add_message::<FirePortal>()
            .add_message::<GunFired>()
            .add_message::<PortalPlaced>()
            .add_message::<PortalClosed>()
            .add_message::<PortalShotFailed>()
            .add_message::<PortalTeleported>()
            .add_message::<ForceDuck>()
            .configure_sets(Update, (P2Systems::Input, P2Systems::Fire).chain())
            .configure_sets(FixedUpdate, (P2Systems::Move, P2Systems::Transit).chain())
            .configure_sets(
                PostUpdate,
                (P2Systems::View, P2Systems::Body, P2Systems::Render, P2Systems::Effects)
                    .chain()
                    .before(TransformSystems::Propagate),
            )
            .add_systems(PostUpdate, animate_portals.before(P2Systems::Render));
    }
}

fn animate_portals(
    time: Res<Time>,
    mut linked: Query<(&Portal, &mut PortalAnimation, Ref<PortalLink>)>,
    mut unlinked: Query<(&Portal, &mut PortalAnimation), Without<PortalLink>>,
) {
    let now = time.elapsed_secs();
    let dt = time.delta_secs();
    for (portal, mut anim, link) in &mut linked {
        if link.is_changed() {
            anim.static_amount = 1.0;
        }
        anim.static_amount = (anim.static_amount - dt / PORTAL_STATIC_FADE_TIME).max(0.0);
        anim.open_amount = ((now - portal.placed_at) / PORTAL_OPEN_TIME).clamp(0.0, 1.0);
    }
    for (portal, mut anim) in &mut unlinked {
        anim.open_amount = ((now - portal.placed_at) / PORTAL_OPEN_TIME).clamp(0.0, 1.0);
        anim.static_amount = 1.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_angles_round_trip() {
        let a = ViewAngles { yaw: 0.7, pitch: -0.3, roll: 0.1 };
        let b = ViewAngles::from_rotation(a.rotation());
        assert!((a.yaw - b.yaw).abs() < 1e-5 && (a.pitch - b.pitch).abs() < 1e-5 && (a.roll - b.roll).abs() < 1e-5);
        // yaw 0 looks down -Z, positive pitch looks up
        let f = ViewAngles { yaw: 0.0, pitch: 0.2, roll: 0.0 }.forward();
        assert!(f.z < 0.0 && f.y > 0.0);
    }

    #[test]
    fn sizes() {
        assert!((HULL_STAND.y - 1.8288).abs() < 1e-4);
        assert!((PORTAL_HALF_SIZE.y - 56.0 * U).abs() < 1e-6);
    }
}

//! First-person camera, portal gun viewmodel and crosshair for the portal-only port.
//!
//! What this plugin does (spec sections in the README):
//! * spawns the one [`MainCamera`] (Hdr, perspective, Source hor+ FOV 75),
//!   unless [`ViewConfig::spawn_main_camera`] is off and the host tags its own;
//! * every frame in `PostUpdate` / `P2Systems::View` places it at the
//!   interpolated feet + crouch eye height with the view angles, and eases
//!   leftover roll back to level (half-life 0.125 s, at least 10 deg/s);
//! * spawns the viewmodel camera (FOV 50, layer `LAYER_VIEWMODEL`) and the
//!   GPU-skinned portal gun, animated with `p2_assets::anim::Animator`, with
//!   the Source viewmodel lag, and writes [`ViewmodelAttachments`] on the player;
//! * draws Portal 2's crosshair: centre dots and the two portal brackets.
use bevy::camera::visibility::RenderLayers;
use bevy::camera::Hdr;
use bevy::prelude::*;
use p2_core::{Crouch, InterpolatedFeet, MainCamera, P2Systems, Player, ViewAngles};

pub mod crosshair;
pub mod viewmodel;

pub use viewmodel::{ViewModelCamera, ViewmodelRig};

/// Source `fov_desired` default (75, range 75-90). Horizontal at 4:3; wider
/// screens widen the horizontal FOV ("hor+") while the vertical stays fixed.
pub const FOV_DESIRED_DEG: f32 = 75.0;

/// Default near plane of the main camera, metres (about 2.6 u). The spec does
/// not give one; this is the value the old route-A sandbox used.
pub const VIEW_NEAR: f32 = 0.05;

/// Options for the first-person view. Insert before adding [`Plugin`] to
/// change them; otherwise the defaults (Portal 2's values) are used.
#[derive(Resource, Clone, Debug)]
pub struct ViewConfig {
    /// Spawn the [`MainCamera`]. Turn off to bring your own camera and tag it
    /// with [`MainCamera`]: this plugin then only moves it, and the viewmodel
    /// camera follows it.
    pub spawn_main_camera: bool,
    /// Horizontal FOV at 4:3, degrees (Source `fov_desired`). Only used when
    /// the main camera is spawned here.
    pub fov_desired_deg: f32,
    /// Near plane of the main camera, metres. Only used when spawned here.
    pub near_plane: f32,
    /// Spawn the viewmodel camera and the gun. Turn off if the host draws its
    /// own gun; [`ViewmodelAttachments`] is then left empty.
    pub viewmodel: bool,
    /// Horizontal FOV of the viewmodel camera at 4:3 (`cl_viewmodelfov`).
    pub viewmodel_fov_deg: f32,
    /// Game-relative path of the gun model, read from the Portal 2 install
    /// through [`p2_assets::GameFsRes`] (Portal 2 is required).
    pub viewmodel_model: String,
    /// Draw the HUD crosshair (needs the portal textures from `p2_assets`).
    pub crosshair: bool,
    /// Multiplier on the crosshair size. 1.0 is the game's own size: the
    /// 44x64 texel brackets drawn at one screen pixel per texel, at every resolution.
    pub crosshair_scale: f32,
}

impl Default for ViewConfig {
    fn default() -> Self {
        Self {
            spawn_main_camera: true,
            fov_desired_deg: FOV_DESIRED_DEG,
            near_plane: VIEW_NEAR,
            viewmodel: true,
            viewmodel_fov_deg: viewmodel::VIEWMODEL_FOV_DEG,
            viewmodel_model: viewmodel::VIEWMODEL_PATH.to_string(),
            crosshair: true,
            crosshair_scale: 1.0,
        }
    }
}

/// The first-person view plugin. Requires `p2_core::CorePlugin` and a
/// `Player` (see `p2_core::spawn_player`) and `p2_assets::AssetsPlugin` for the
/// gun model and crosshair, which are Portal 2's own (nothing is shipped).
pub struct Plugin;

impl bevy::app::Plugin for Plugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ViewConfig>();
        viewmodel::build(app);
        crosshair::build(app);
        app.add_systems(
            Update,
            (spawn_main_camera, viewmodel::spawn_viewmodel_camera_and_gun, crosshair::spawn_crosshair),
        )
        .add_systems(
            PostUpdate,
            (punch_on_fire, update_view, viewmodel::update_viewmodel).chain().in_set(P2Systems::View),
        );
    }
}

/// Source horizontal FOV at 4:3 (degrees) to Bevy's vertical FOV (radians).
/// Vertical is independent of the window aspect, so this is hor+ scaling.
pub fn source_fov_to_vertical(deg: f32) -> f32 {
    2.0 * ((deg.to_radians() * 0.5).tan() * 0.75).atan()
}

/// Transform of an eye at `feet` (already interpolated) raised by
/// `eye_height` metres, looking along `angles` (roll included).
pub fn eye_transform(feet: Vec3, eye_height: f32, angles: &ViewAngles) -> Transform {
    Transform::from_translation(feet + Vec3::Y * eye_height).with_rotation(angles.rotation())
}

fn spawn_main_camera(
    mut commands: Commands,
    cfg: Res<ViewConfig>,
    added: Query<(), Added<Player>>,
    existing: Query<(), With<MainCamera>>,
) {
    if !cfg.spawn_main_camera || added.is_empty() || !existing.is_empty() {
        return;
    }
    commands.spawn((
        Name::new("Main camera"),
        MainCamera,
        Camera3d::default(),
        Hdr,
        // Portal 2 renders integer HDR: shaders multiply by the exposure scale
        // and write straight to an 8-bit target (post-processing.md), a linear
        // scale + clamp with no filmic curve. A filmic tonemapper desaturates
        // the portal rims and flames.
        bevy::core_pipeline::tonemapping::Tonemapping::None,
        Projection::Perspective(PerspectiveProjection {
            fov: source_fov_to_vertical(cfg.fov_desired_deg),
            near: cfg.near_plane,
            ..default()
        }),
        RenderLayers::from_layers(&[0, p2_core::LAYER_MAIN_VIEW_ONLY]),
        Transform::default(),
    ));
}

/// Source view punch (view-feel.md §4): an angle offset (degrees, Source
/// signs: +pitch looks down, +yaw turns left) with a damped spring. Firing
/// the portal gun kicks it (portal-placement.md: pitch -1..-0.5, yaw -1..1).
/// It moves the world view only; the viewmodel stays put on screen.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct ViewPunch {
    /// Current offset (pitch, yaw), degrees.
    pub angle: Vec2,
    /// Its velocity, degrees per second.
    pub velocity: Vec2,
}

impl ViewPunch {
    /// Source `ViewPunch`: adds `angles * 20` to the velocity, not the angle.
    pub fn kick(&mut self, pitch_deg: f32, yaw_deg: f32) {
        self.velocity += Vec2::new(pitch_deg, yaw_deg) * 20.0;
    }

    /// One step of the spring: damping 9, spring 65; snaps to rest when tiny.
    pub fn step(&mut self, dt: f32) {
        if self.angle.length_squared() < 0.001 && self.velocity.length_squared() < 0.001 {
            *self = Self::default();
            return;
        }
        self.angle += self.velocity * dt;
        self.velocity *= (1.0 - 9.0 * dt).max(0.0);
        self.velocity -= self.angle * (65.0 * dt).clamp(0.0, 2.0);
        self.angle = Vec2::new(self.angle.x.clamp(-89.0, 89.0), self.angle.y.clamp(-179.0, 179.0));
    }

    /// The offset as a rotation to apply after the view rotation (our pitch
    /// is +up, Source's is +down).
    pub fn rotation(&self) -> Quat {
        Quat::from_euler(EulerRot::YXZ, self.angle.y.to_radians(), (-self.angle.x).to_radians(), 0.0)
    }
}

/// Small deterministic PRNG for the punch angles (no extra dependency).
fn rand01(seed: &mut u32) -> f32 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 17;
    *seed ^= *seed << 5;
    (*seed >> 8) as f32 / (1u32 << 24) as f32
}

/// Kicks the view when this player's gun fires.
fn punch_on_fire(
    mut commands: Commands,
    mut fired: MessageReader<p2_core::GunFired>,
    mut players: Query<Option<&mut ViewPunch>, With<Player>>,
    mut seed: Local<u32>,
) {
    if *seed == 0 {
        *seed = 0x9e37_79b9;
    }
    for shot in fired.read() {
        let Ok(punch) = players.get_mut(shot.gun) else { continue };
        let pitch = -1.0 + 0.5 * rand01(&mut seed);
        let yaw = -1.0 + 2.0 * rand01(&mut seed);
        match punch {
            Some(mut p) => p.kick(pitch, yaw),
            None => {
                let mut p = ViewPunch::default();
                p.kick(pitch, yaw);
                commands.entity(shot.gun).insert(p);
            }
        }
    }
}

/// Places the main camera and the viewmodel camera at the eye, eases roll and
/// applies the view punch (main camera only).
fn update_view(
    time: Res<Time>,
    fixed: Res<Time<Fixed>>,
    mut players: Query<(&InterpolatedFeet, &mut ViewAngles, &Crouch, Option<&mut ViewPunch>), With<Player>>,
    mut cams: Query<(&mut Transform, Has<MainCamera>), (Or<(With<MainCamera>, With<ViewModelCamera>)>, Without<p2_core::Portal>)>,
    portals: Query<(&Transform, &p2_core::Portal, &p2_core::PortalLink), (Without<MainCamera>, Without<ViewModelCamera>)>,
    exits: Query<&Transform, (With<p2_core::Portal>, Without<MainCamera>, Without<ViewModelCamera>)>,
    mut local_eye: ResMut<p2_core::LocalEye>,
) {
    let Some((feet, mut angles, crouch, punch)) = players.iter_mut().next() else {
        return;
    };
    if angles.roll != 0.0 {
        let roll = p2_core::math::relevel_roll(angles.roll, time.delta_secs());
        if roll != angles.roll {
            angles.roll = roll;
        }
    }
    let punch_rot = match punch {
        Some(mut p) => {
            p.step(time.delta_secs());
            p.rotation()
        }
        None => Quat::IDENTITY,
    };
    let eye = eye_transform(feet.at(fixed.overstep_fraction()), crouch.eye_height, &angles);
    // The rendered eye can sit just behind a portal's plane, inside its
    // opening: right after a teleport (the smoothing runs from the previous
    // tick, mapped through the pair, which lies behind the exit), or when the
    // eye crosses before the body centre does (jumping up into a ceiling
    // portal). There the eye is really on the other side: render the world
    // from the mapped pose, or the frame shows the inside of the wall.
    // `LocalEye::through` tells p2_render which views then skip the body.
    let linked = portals.iter().filter_map(|(tf, p, l)| exits.get(l.0).ok().map(|x| (tf, p, l.0, x)));
    let through = p2_core::eye_through_portal(eye, linked);
    let world_eye = through.map_or(eye, |(t, _)| t);
    let want = p2_core::LocalEye { third_person: false, through: through.map(|(_, exit)| exit) };
    if *local_eye != want {
        *local_eye = want;
    }
    for (mut t, is_main) in &mut cams {
        *t = if is_main { world_eye.with_rotation(world_eye.rotation * punch_rot) } else { eye };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_fov_is_horizontal_at_4_3() {
        // Converting back: horizontal = 2 atan(tan(v/2) * 4/3) must give 75 deg.
        let v = source_fov_to_vertical(75.0);
        let h = 2.0 * ((v * 0.5).tan() * 4.0 / 3.0).atan();
        assert!((h.to_degrees() - 75.0).abs() < 1e-3, "{}", h.to_degrees());
        // Vertical stays put on a wide screen (hor+): 16:9 horizontal widens.
        let h16 = 2.0 * ((v * 0.5).tan() * 16.0 / 9.0).atan();
        assert!(h16.to_degrees() > 85.0 && h16.to_degrees() < 95.0, "{}", h16.to_degrees());
    }

    #[test]
    fn punch_springs_back() {
        let mut p = ViewPunch::default();
        p.kick(-1.0, 0.5);
        let mut peak = 0.0f32;
        for _ in 0..120 {
            p.step(1.0 / 60.0);
            peak = peak.max(p.angle.length());
        }
        assert!(peak > 0.5 && peak < 2.0, "peak {peak}");
        assert!(p.angle.length() < 0.05, "settled {:?}", p.angle);
    }

    #[test]
    fn eye_sits_above_feet() {
        let t = eye_transform(Vec3::new(1.0, 2.0, 3.0), 1.5, &ViewAngles::default());
        assert!((t.translation - Vec3::new(1.0, 3.5, 3.0)).length() < 1e-6);
    }
}

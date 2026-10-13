//! Chell's third-person body, and the ghost copies that make it look whole
//! while it is half-way through a portal (player-in-portals.md).
//!
//! * The body is on [`p2_core::LAYER_PLAYER_BODY`], its ghosts on
//!   [`p2_core::player_ghost_layer`]; `p2_render` decides per camera which it
//!   draws (Portal 2's rules, player-in-portals.md §2-3).
//! * Every body is a [`Ghostable`] root. While its bounds overlap a linked
//!   opening, the part that has passed through is clipped off and a mirror of
//!   the body is drawn at the other portal, clipped so the two halves join.
//! * The model is chosen by [`BodyConfig::model`]: Chell from the player's
//!   Portal 2 install (needs `p2_assets::AssetsPlugin`; nothing from the game
//!   is shipped), or a host-supplied glTF scene. If Chell can't be loaded, no
//!   body is drawn.
//!
//! Add [`Plugin`] after [`p2_core::CorePlugin`]. Any entity can opt into the
//! same ghosting with [`Ghostable`] (see [`ghost`]).
//!
//! Spec: research/specs/player-in-portals.md.

use bevy::camera::visibility::RenderLayers;
use bevy::pbr::MaterialPlugin;
use bevy::prelude::*;
use bevy::world_serialization::{WorldAsset, WorldAssetRoot};
use p2_core::{InterpolatedFeet, Crouch, P2Systems, Player, ViewAngles, LAYER_PLAYER_BODY};

mod chell;
pub mod ghost;

pub use chell::ChellRig;
pub use ghost::{
    clip_plane, front_plane, ghost_material, ghost_transform, overlaps_opening, ClipPlanes, GhostBounds, GhostClipExt,
    GhostMaterial, Ghostable, CLIP_OVERLAP, DISABLED_PLANE, GHOST_CENTRE_BEHIND, GHOST_SLAB, MAX_CLIP_PLANES,
};

use ghost::{convert_ghost_materials, sync_ghosts, GhostPart};

/// Which model a body uses.
#[derive(Clone, Debug, Default)]
pub enum BodyModel {
    /// `models/player/chell/player.mdl` from the Portal 2 install, with the
    /// world gun in her hand. Without it no body is drawn.
    #[default]
    Chell,
    /// A host-supplied scene (any glTF with skinned or plain meshes). It is
    /// spawned under the body root with its origin at the feet and its front
    /// along -Z. Its animation is the host's job; ghosts copy its poses.
    Scene(Handle<WorldAsset>),
}

/// Settings for the body. Insert before adding [`Plugin`] to change them.
#[derive(Resource, Clone, Debug)]
pub struct BodyConfig {
    /// Spawn a body for every `Player`. Off means no body at all.
    pub enabled: bool,
    /// The model to use.
    pub model: BodyModel,
    /// Source path of Chell's model in the Portal 2 install.
    pub chell_model: String,
    /// Source path of the animations Chell's model is driven by.
    pub chell_animations: String,
    /// Source path of the world gun Chell holds.
    pub gun_model: String,
}

impl Default for BodyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            model: BodyModel::Chell,
            chell_model: "models/player/chell/player.mdl".into(),
            chell_animations: "models/player_animations.mdl".into(),
            gun_model: "models/weapons/w_portalgun.mdl".into(),
        }
    }
}

/// The root of one player's body (a top-level [`Ghostable`] entity). Its
/// `Transform` is the feet with yaw only, at the interpolated position.
#[derive(Component, Clone, Copy, Debug)]
pub struct PlayerBody {
    /// The `Player` entity this body follows.
    pub player: Entity,
}

/// The plugin: spawns bodies, animates them and keeps their ghosts in sync.
pub struct Plugin;

impl bevy::app::Plugin for Plugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "clip.wgsl");
        app.add_plugins(MaterialPlugin::<GhostMaterial>::default())
            .init_resource::<BodyConfig>()
            .add_systems(Startup, chell::load_chell)
            .add_systems(
                PostUpdate,
                (
                    spawn_bodies,
                    convert_ghost_materials,
                    drive_roots,
                    chell::drive_parts,
                    layer_scene_meshes,
                    sync_ghosts,
                )
                    .chain()
                    .in_set(P2Systems::Body),
            );
    }
}

fn spawn_bodies(
    mut commands: Commands,
    cfg: Res<BodyConfig>,
    chell: Option<Res<chell::ChellAssets>>,
    mut ghost_materials: ResMut<Assets<GhostMaterial>>,
    players: Query<Entity, Added<Player>>,
) {
    if !cfg.enabled {
        return;
    }
    let chell = chell.as_deref().and_then(|c| c.0.as_ref());
    for player in &players {
        if matches!(cfg.model, BodyModel::Chell) && chell.is_none() {
            continue;
        }
        let root = commands
            .spawn((
                Name::new("player body"),
                PlayerBody { player },
                Ghostable,
                GhostBounds::default(),
                Transform::default(),
                Visibility::default(),
                RenderLayers::layer(LAYER_PLAYER_BODY),
            ))
            .id();
        match &cfg.model {
            BodyModel::Chell => {
                if let Some(assets) = chell {
                    chell::spawn(&mut commands, root, assets, &mut ghost_materials);
                }
            }
            BodyModel::Scene(handle) => {
                commands.spawn((WorldAssetRoot(handle.clone()), ChildOf(root)));
            }
        }
    }
}

/// Places every body root at its player's interpolated feet, yaw only, and
/// sizes its bounds to the hull in the current crouch state.
fn drive_roots(
    fixed: Res<Time<Fixed>>,
    players: Query<(&InterpolatedFeet, &ViewAngles, &Crouch), With<Player>>,
    mut bodies: Query<(&PlayerBody, &mut Transform, &mut GhostBounds)>,
) {
    let alpha = fixed.overstep_fraction();
    for (body, mut tf, mut bounds) in &mut bodies {
        let Ok((feet, view, crouch)) = players.get(body.player) else { continue };
        tf.translation = feet.at(alpha);
        tf.rotation = Quat::from_rotation_y(view.yaw);
        let hull = crouch.hull();
        let half = Vec3::new(hull.x * 0.5, 0.0, hull.z * 0.5);
        *bounds = GhostBounds { min: Vec3::new(-half.x, 0.0, -half.z), max: Vec3::new(half.x, hull.y, half.z) };
    }
}

/// Scene meshes do not inherit a render layer: put them on the body's layer.
fn layer_scene_meshes(
    mut commands: Commands,
    parents: Query<&ChildOf>,
    bodies: Query<(), With<PlayerBody>>,
    meshes: Query<Entity, (With<Mesh3d>, Without<RenderLayers>, Without<GhostPart>)>,
) {
    for e in &meshes {
        let mut top = e;
        while let Ok(p) = parents.get(top) {
            top = p.parent();
        }
        if bodies.contains(top) {
            commands.entity(e).insert(RenderLayers::layer(LAYER_PLAYER_BODY));
        }
    }
}

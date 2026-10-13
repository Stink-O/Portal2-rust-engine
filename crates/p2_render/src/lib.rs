//! Portals in Bevy: the view through a portal (with recursion), the portal
//! surface (a port of Portal 2's `PortalRefract`) and the near-plane cap that
//! keeps the opening drawn when the eye is closer than the near plane.
//!
//! Add [`Plugin`] after `DefaultPlugins` and `p2_core::CorePlugin`. The host
//! supplies one camera tagged [`p2_core::MainCamera`]; this plugin adds the
//! portal view cameras and the surfaces. It never spawns a window, camera (other
//! than its own view cameras), light or player.
//!
//! Every knob lives in [`PortalRenderConfig`]. Defaults are Portal 2's values.
//!
//! See `README.md` for the full list of behaviour, the spec sections and the gaps.
use bevy::prelude::*;

use p2_core::{P2Systems, LAYER_RENDER_FIRST};

mod projection;
mod surface;
mod views;

pub use projection::PortalProjection;
pub use surface::{PortalSurfaceCap, PortalSurfaceMaterial, PortalSurfaceParams, PortalSurfaceQuad, SurfaceKey};
pub use views::{
    child_code, node_level, node_path, portal_visible, NodeCameras, NodeImages, NodeView, PortalInfo, PortalInfos,
    ViewTree, ROOT,
};

/// Tunables for portal rendering. Defaults are Portal 2's values.
///
/// Insert this resource before adding [`Plugin`] to change a value; otherwise
/// the defaults are used.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct PortalRenderConfig {
    /// How many nested portal views carry real surfaces. Portal 2's
    /// `r_portal_stencil_depth` is 2. Clamped to 1..=3; one more plain level
    /// is drawn past it (see the README on the depth doubler).
    pub recursion_depth: u32,
    /// Size of the portal view targets relative to the main camera's target.
    /// 1.0 matches it pixel for pixel. Lower it to save fill rate.
    pub target_scale: f32,
    /// Distance in front of the wall at which the surface quad sits, metres.
    /// It stops z-fighting with the wall. Keep it a few millimetres.
    pub quad_standoff: f32,
    /// Depth of the near-plane cap box behind the opening, metres.
    pub cap_depth: f32,
    /// How far beyond the main camera's near plane (metres) the cap starts to
    /// take over. Raise it if the quad flickers at the near plane.
    pub cap_margin: f32,
}

impl Default for PortalRenderConfig {
    fn default() -> Self {
        Self { recursion_depth: 2, target_scale: 1.0, quad_standoff: 0.004, cap_depth: 0.35, cap_margin: 0.02 }
    }
}

impl PortalRenderConfig {
    /// The config with every value pushed into its valid range.
    pub fn clamped(self) -> Self {
        Self {
            recursion_depth: self.recursion_depth.clamp(1, 3),
            target_scale: self.target_scale.clamp(0.1, 2.0),
            quad_standoff: self.quad_standoff.max(1e-4),
            cap_depth: self.cap_depth.max(0.01),
            cap_margin: self.cap_margin.max(0.0),
        }
    }

    /// Number of surface nodes (root plus every level up to the depth).
    pub fn surface_nodes(&self) -> usize {
        (1usize << (self.recursion_depth + 1)) - 1
    }

    /// Surface nodes including the plain level below the depth, whose
    /// portals are drawn with the depth doubler.
    pub fn doubler_nodes(&self) -> usize {
        (1usize << (self.recursion_depth + 2)) - 1
    }
}

/// Render layer of the surface quads and caps of node `code`
/// (the root is code 1 and uses [`LAYER_RENDER_FIRST`]).
pub fn node_layer(code: u32) -> usize {
    LAYER_RENDER_FIRST + code as usize - 1
}

/// Registers the portal view cameras and portal surfaces.
///
/// The plugin does not add `CorePlugin`, the physics engine or a camera, and
/// it does not touch `ClearColor`, the window or `Time<Fixed>`. Add it once.
#[derive(Default)]
pub struct Plugin {
    /// The configuration. Inserted as a resource unless one already exists.
    pub config: PortalRenderConfig,
}

impl bevy::app::Plugin for Plugin {
    fn build(&self, app: &mut App) {
        let config = self.config.clamped();
        if !app.world().contains_resource::<PortalRenderConfig>() {
            app.insert_resource(config);
        }
        surface::plugin(app);
        app.init_resource::<views::MainView>()
            .init_resource::<PortalInfos>()
            .init_resource::<ViewTree>()
            .init_resource::<NodeCameras>()
            .init_resource::<NodeImages>()
            .add_systems(Startup, views::spawn_node_cameras)
            // Node cameras switch target, pose and projection here; Bevy's
            // camera_system must see that in the same frame, or a view that
            // just came into sight keeps the parking target's size and shows
            // a stale image for one frame (e.g. right after a transit).
            .configure_sets(PostUpdate, P2Systems::Render.before(bevy::camera::CameraUpdateSystems))
            .add_systems(
                PostUpdate,
                (
                    surface::ensure_surfaces,
                    views::ensure_main_layers,
                    views::capture_main_view,
                    views::update_views,
                    surface::sync_surfaces,
                )
                    .chain()
                    .in_set(P2Systems::Render),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers_and_counts() {
        assert_eq!(node_layer(1), LAYER_RENDER_FIRST);
        assert_eq!(node_layer(7), LAYER_RENDER_FIRST + 6);
        let c = PortalRenderConfig::default();
        assert_eq!(c.surface_nodes(), 7);
        assert_eq!(PortalRenderConfig { recursion_depth: 3, ..c }.surface_nodes(), 15);
        assert_eq!(PortalRenderConfig { recursion_depth: 9, ..c }.clamped().recursion_depth, 3);
    }
}

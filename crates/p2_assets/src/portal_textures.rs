//! Portal 2's own assets, read from the user's install through
//! `source_assets` (nothing is copied into the repo): the portal overlay
//! textures, the HUD crosshair, soundscripts and particle definitions.
//!
//! A texture missing from the install is logged and left as Bevy's default image.
use bevy::image::{ImageAddressMode, ImageSampler};
use bevy::prelude::*;
use source_assets::vtf::VtfTexture;

use p2_core::PortalKind;

/// Portal 2 files the portal crate uses.
#[derive(Resource, Clone)]
pub struct PortalGameAssets {
    /// `models/portals/noise-blur-256x256` (PortalRefract `$PortalMaskTexture`).
    pub noise: Handle<Image>,
    /// `models/portals/portal-{blue,orange}-color` (1-D flame colour ramps).
    pub ramps: [Handle<Image>; 2],
    /// `sprites/hud/portal_crosshairs`.
    pub crosshair: Handle<Image>,
    /// `$PortalColorScale` of `portalstaticoverlay_N`.
    pub color_scale: f32,
}

impl PortalGameAssets {
    pub fn ramp(&self, kind: PortalKind) -> Handle<Image> {
        self.ramps[kind.index()].clone()
    }
}

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(PreStartup, load_game_assets);
}

/// Load the textures from the mounted game.
fn load_game_assets(
    mut commands: Commands,
    fsres: Res<crate::GameFsRes>,
    mut images: ResMut<Assets<Image>>,
) {
    let load = |images: &mut Assets<Image>, path: &str, srgb: bool, wrap: bool| -> Handle<Image> {
        let Some(data) = fsres.read(path) else {
            error!("p2_assets: {path} missing from the Portal 2 install");
            return Handle::default();
        };
        let tex = match VtfTexture::parse(&data) {
            Ok(t) => t,
            Err(e) => {
                error!("p2_assets: {path}: {e}");
                return Handle::default();
            }
        };
        let mut img = tex.into_image(false, srgb);
        if let ImageSampler::Descriptor(d) = &mut img.sampler {
            let mode = if wrap { ImageAddressMode::Repeat } else { ImageAddressMode::ClampToEdge };
            d.address_mode_u = mode;
            d.address_mode_v = mode;
        }
        images.add(img)
    };

    // PortalRefract reads the noise texture linear and the colour ramp as sRGB.
    let noise = load(&mut images, "materials/models/portals/noise-blur-256x256.vtf", false, true);
    let blue = load(&mut images, "materials/models/portals/portal-blue-color.vtf", true, false);
    let orange = load(&mut images, "materials/models/portals/portal-orange-color.vtf", true, false);
    let crosshair = load(&mut images, "materials/sprites/hud/portal_crosshairs.vtf", true, false);

    commands.insert_resource(PortalGameAssets {
        noise,
        ramps: [blue, orange],
        crosshair,
        color_scale: 4.0,
    });
}

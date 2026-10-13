//! Portal 2 (Source engine) assets in Bevy.
//!
//! Mount the user's install (`PORTAL2_DIR` or [`config::DEFAULT_PORTAL2_DIR`]),
//! convert a BSP into lightmapped meshes, avian colliders tagged with
//! [`shared::Portalable`], and typed entity spawns:
//!
//! ```ignore
//! app.add_plugins(source_assets::SourceMapPlugin::default());
//! fn setup(mut commands: Commands, mut assets: source_assets::SourceMapAssets) {
//!     let info = source_assets::spawn_map(&mut commands, &mut assets, "sp_a1_intro3", Vec3::ZERO).unwrap();
//!     // info.player_start, info.entities, info.light_environment, info.skybox ...
//! }
//! ```

pub mod bsp_raw;
pub mod collision;
pub mod config;
pub mod entities;
pub mod fs;
pub mod geometry;
pub mod kv;
pub mod map;
pub mod material;
pub mod models;
pub mod phy;
pub mod plugin;
pub mod skeleton;
pub mod sm;
pub mod valve;
pub mod valve_render;
pub mod surfaceprops;
pub mod vcs;
pub mod vmt;
pub mod vtf;

pub use config::{GameConfig, DEFAULT_PORTAL2_DIR, PORTAL2_DIR_ENV};
pub use fs::GameFs;
pub use map::LoadedMap;
pub use material::SourceMaterial;
pub use plugin::{
    load_map, spawn_loaded_map, spawn_map, LightEnvironment, MapEntity, MapFog, DEFAULT_LIGHT_SCALE, MapEntityKind, MapInfo,
    PortalShotPassthrough, SourceCache, SourceCollider, SourceEntityRef, SourceGame, SourceMapAssets,
    SourceMapPlugin, SourceMapRoot, SourceRenderSettings,
};

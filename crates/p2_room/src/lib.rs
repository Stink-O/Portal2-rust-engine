//! Generated greybox test chamber for the portal-only port (NOT a Portal 2
//! level) and the shared screenshot harness.
//!
//! The chamber is built in code (see [`chamber`]) from Portal 2's own chamber
//! materials (read from the player's install with `source_assets`; nothing
//! from the game is shipped). Public API:
//! [`RoomPlugin`], [`RoomSpots`] / [`RoomSpot`], [`spawn_box`], [`spawn_slab`],
//! [`HarnessPlugin`], [`HarnessArgs`]. See `README.md`.

pub mod chamber;
pub mod harness;

pub use chamber::{spawn_box, spawn_slab, RoomPlugin, RoomSpot, RoomSpots, TEXELS_PER_UNIT};
pub use harness::{HarnessArgs, HarnessPlugin};

//! The contract between crates. Change a type here only if every crate that
//! uses it is changed in the same pass.
//!
//! World space: Bevy's Y-up, right-handed, metres. One Minecraft block is
//! exactly 1 m. Source is Z-up, in inches ("units"); convert with
//! [`source_to_world`].
use bevy::ecs::entity::EntityHashSet;
use bevy::prelude::*;

/// Metres per Source unit (1 unit = 1 inch).
pub const SOURCE_UNIT: f32 = 0.0254;

/// Source position (Z-up, inches) to world position (Y-up, metres).
pub fn source_to_world(v: Vec3) -> Vec3 {
    Vec3::new(v.x, v.z, -v.y) * SOURCE_UNIT
}

/// Source direction (Z-up) to world direction (Y-up), unscaled.
pub fn source_dir_to_world(v: Vec3) -> Vec3 {
    Vec3::new(v.x, v.z, -v.y)
}

/// A collider a portal may be placed on. Put it on the collider entity
/// (a Portal 2 wall of portalable material, a Minecraft block section, a
/// sandbox wall). Surfaces without it reject portal shots.
#[derive(Component, Default, Clone, Copy, Debug)]
pub struct Portalable;

/// The player the camera follows. Exactly one.
#[derive(Component, Default, Clone, Copy, Debug)]
pub struct Player;

/// Velocity of a kinematic character, in m/s, world space. The movement
/// crate integrates it; the portal crate rotates it when the character
/// passes through a portal pair.
#[derive(Component, Default, Clone, Copy, Debug, Deref, DerefMut)]
pub struct CharacterVelocity(pub Vec3);

/// Colliders the character ignores this frame. The portal crate fills it with
/// the wall behind a portal while the character is inside that portal's
/// opening; the movement crate must skip these in every cast and overlap.
#[derive(Component, Default, Clone, Debug, Deref, DerefMut)]
pub struct IgnoreColliders(pub EntityHashSet);

/// Where the eyes sit relative to the character's Transform, in metres.
/// The first-person camera is a child entity at this offset.
#[derive(Component, Clone, Copy, Debug, Deref, DerefMut)]
pub struct EyeOffset(pub Vec3);

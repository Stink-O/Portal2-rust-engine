//! The avian physics world as seen by the gun: shots and probes ignore the
//! shooter, `PortalHelperCollider`s, sensors and dynamic bodies.
use avian3d::prelude::*;
use bevy::prelude::*;

use crate::rules::{Hit, World};

/// Avian-backed [`World`]. `skip` returns true for colliders a shot passes through.
pub struct PhysicsWorld<'a> {
    /// Avian spatial queries.
    pub spatial: &'a SpatialQuery<'a, 'a>,
    /// Excludes the shooter.
    pub filter: SpatialQueryFilter,
    /// True for colliders a shot passes through.
    pub skip: &'a dyn Fn(Entity) -> bool,
    /// True for colliders (or bodies) tagged `Portalable`.
    pub portalable: &'a dyn Fn(Entity) -> bool,
}

impl World for PhysicsWorld<'_> {
    fn ray(&self, origin: Vec3, dir: Vec3, max: f32) -> Option<Hit> {
        let dir3 = Dir3::new(dir).ok()?;
        let h = self.spatial.cast_ray_predicate(origin, dir3, max, true, &self.filter, &|e| !(self.skip)(e))?;
        Some(Hit { entity: h.entity, point: origin + dir * h.distance, normal: h.normal, distance: h.distance })
    }

    fn portalable(&self, entity: Entity) -> bool {
        (self.portalable)(entity)
    }
}

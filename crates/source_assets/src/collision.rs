//! Brushes to convex collision hulls, split by portalability.
//!
//! A brush whose visible sides disagree about portalability (a white
//! portalable front with nodraw/black sides) is split exactly into one
//! pyramid per side, apex at the brush centroid. The pyramids of each class
//! become a compound collider, so every collider entity has a single answer
//! to "can a portal go here".

use bevy::math::{DVec3, Vec3};
use shared::{source_dir_to_world, source_to_world};

pub const CONTENTS_SOLID: u32 = 0x1;
pub const CONTENTS_WINDOW: u32 = 0x2;
pub const CONTENTS_GRATE: u32 = 0x8;
pub const CONTENTS_WATER: u32 = 0x20;
pub const CONTENTS_MOVEABLE: u32 = 0x4000;
pub const CONTENTS_PLAYERCLIP: u32 = 0x10000;
pub const CONTENTS_MONSTERCLIP: u32 = 0x20000;
pub const CONTENTS_DETAIL: u32 = 0x8000000;

/// Contents a player collides with (MASK_PLAYERSOLID minus CONTENTS_MONSTER).
pub const MASK_PLAYERSOLID: u32 =
    CONTENTS_SOLID | CONTENTS_MOVEABLE | CONTENTS_PLAYERCLIP | CONTENTS_WINDOW | CONTENTS_GRATE;
/// Contents that stop a portal shot (Portal 2 fires through grates and clips).
pub const MASK_SHOT_PORTAL: u32 = CONTENTS_SOLID | CONTENTS_MOVEABLE | CONTENTS_WINDOW;

/// One outward face of a brush.
#[derive(Clone, Debug)]
pub struct BrushSide {
    /// Source-space plane normal and distance.
    pub normal: DVec3,
    pub dist: f64,
    /// Source-space polygon (empty for bevel or degenerate sides).
    pub polygon: Vec<DVec3>,
    /// Index into the map's material table, if the side has a texinfo.
    pub material: Option<usize>,
    pub texinfo_flags: u32,
}

/// Polygon on plane `n.x = d` clipped by every other plane (keeping the back side).
pub fn side_polygon(n: DVec3, d: f64, others: &[(DVec3, f64)]) -> Vec<DVec3> {
    // Base quad: a huge square on the plane.
    let up = if n.z.abs() > 0.9 { DVec3::X } else { DVec3::Z };
    let right = up.cross(n).normalize();
    let up = n.cross(right).normalize();
    let c = n * d;
    let s = 65536.0;
    let mut poly = vec![
        c - right * s + up * s,
        c + right * s + up * s,
        c + right * s - up * s,
        c - right * s - up * s,
    ];
    for &(pn, pd) in others {
        if poly.is_empty() {
            break;
        }
        let mut out = Vec::with_capacity(poly.len() + 1);
        for i in 0..poly.len() {
            let a = poly[i];
            let b = poly[(i + 1) % poly.len()];
            let da = a.dot(pn) - pd;
            let db = b.dot(pn) - pd;
            const EPS: f64 = 0.01;
            if da <= EPS {
                out.push(a);
            }
            if (da > EPS && db < -EPS) || (da < -EPS && db > EPS) {
                let t = da / (da - db);
                out.push(a + (b - a) * t);
            }
        }
        poly = out;
    }
    // drop near-duplicate points
    let mut clean: Vec<DVec3> = Vec::with_capacity(poly.len());
    for p in poly {
        if clean.last().is_none_or(|q: &DVec3| q.distance_squared(p) > 1e-4) {
            clean.push(p);
        }
    }
    if clean.len() > 1 && clean[0].distance_squared(*clean.last().unwrap()) <= 1e-4 {
        clean.pop();
    }
    if clean.len() < 3 {
        clean.clear();
    }
    clean
}

/// Build the sides of brush `bi` with polygons.
pub fn brush_sides(bsp: &vbsp::Bsp, bi: usize, texinfo_material: &dyn Fn(i16) -> Option<usize>) -> Vec<BrushSide> {
    let brush = &bsp.brushes[bi];
    let sides = &bsp.brush_sides
        [brush.brush_side as usize..(brush.brush_side + brush.num_brush_sides) as usize];
    let planes: Vec<(DVec3, f64, bool)> = sides
        .iter()
        .map(|s| {
            let p = &bsp.planes[s.plane as usize];
            (
                DVec3::new(p.normal.x as f64, p.normal.y as f64, p.normal.z as f64),
                p.dist as f64,
                s.bevel != 0,
            )
        })
        .collect();
    let bounding: Vec<(DVec3, f64)> = planes.iter().filter(|p| !p.2).map(|p| (p.0, p.1)).collect();
    sides
        .iter()
        .zip(&planes)
        .map(|(s, &(n, d, bevel))| {
            let others: Vec<(DVec3, f64)> = bounding
                .iter()
                .copied()
                .filter(|(on, od)| !(on.abs_diff_eq(n, 1e-6) && (od - d).abs() < 1e-6))
                .collect();
            let polygon = if bevel { Vec::new() } else { side_polygon(n, d, &others) };
            let texinfo_flags = bsp
                .textures_info
                .get(s.texture_info as usize)
                .filter(|_| s.texture_info >= 0)
                .map(|t| t.flags.bits())
                .unwrap_or(0);
            BrushSide {
                normal: n,
                dist: d,
                polygon,
                material: texinfo_material(s.texture_info),
                texinfo_flags,
            }
        })
        .collect()
}

/// Per-side facts the collider carries.
#[derive(Clone, Debug)]
pub struct SideInfo {
    /// Outward normal in world axes.
    pub normal: Vec3,
    /// Material name (canonical), empty if the side had no texinfo.
    pub material: String,
    pub surface_prop: Option<String>,
    pub portalable: bool,
}

/// A collider ready to spawn: convex pieces in world axes (metres), relative to the map origin.
#[derive(Clone, Debug)]
pub struct ColliderData {
    /// Convex point clouds; one piece = a plain convex hull, several = a compound.
    pub pieces: Vec<Vec<Vec3>>,
    pub portalable: bool,
    pub contents: u32,
    pub brush: u32,
    /// Brush model index: 0 = world, N = the entity with model `*N`.
    pub model: u32,
    /// The outward faces this collider covers.
    pub sides: Vec<SideInfo>,
}

impl ColliderData {
    /// Portal shots pass through (grates, player clips).
    pub fn shot_passthrough(&self) -> bool {
        self.contents & MASK_SHOT_PORTAL == 0 || self.contents & CONTENTS_GRATE != 0
    }
}

fn to_world(p: DVec3, origin: DVec3) -> Vec3 {
    source_to_world((p - origin).as_vec3())
}

/// Turn one brush into one or two colliders.
/// `side_portalable(side, polygon_centroid)` decides each side's class.
pub fn brush_colliders(
    sides: &[BrushSide],
    contents: u32,
    brush: u32,
    model: u32,
    origin: DVec3,
    side_portalable: &dyn Fn(&BrushSide, DVec3) -> bool,
    side_info: &dyn Fn(&BrushSide) -> (String, Option<String>),
) -> Vec<ColliderData> {
    let faces: Vec<&BrushSide> = sides.iter().filter(|s| s.polygon.len() >= 3).collect();
    if faces.len() < 4 {
        return Vec::new();
    }
    let mut all: Vec<DVec3> = faces.iter().flat_map(|s| s.polygon.iter().copied()).collect();
    all.dedup_by(|a, b| a.distance_squared(*b) < 1e-4);
    let centroid = all.iter().copied().sum::<DVec3>() / all.len() as f64;
    let classes: Vec<bool> = faces
        .iter()
        .map(|s| {
            let c = s.polygon.iter().copied().sum::<DVec3>() / s.polygon.len() as f64;
            side_portalable(s, c)
        })
        .collect();
    let infos: Vec<SideInfo> = faces
        .iter()
        .zip(&classes)
        .map(|(s, &p)| {
            let (material, surface_prop) = side_info(s);
            SideInfo {
                normal: source_dir_to_world(s.normal.as_vec3()),
                material,
                surface_prop,
                portalable: p,
            }
        })
        .collect();

    let uniform = classes.iter().all(|&c| c == classes[0]);
    if uniform {
        return vec![ColliderData {
            pieces: vec![all.iter().map(|&p| to_world(p, origin)).collect()],
            portalable: classes[0],
            contents,
            brush,
            model,
            sides: infos,
        }];
    }
    let mut out = Vec::new();
    for class in [true, false] {
        let mut pieces = Vec::new();
        let mut sides_c = Vec::new();
        for ((s, &c), info) in faces.iter().zip(&classes).zip(&infos) {
            if c != class {
                continue;
            }
            let mut pts: Vec<Vec3> = s.polygon.iter().map(|&p| to_world(p, origin)).collect();
            pts.push(to_world(centroid, origin));
            pieces.push(pts);
            sides_c.push(info.clone());
        }
        if !pieces.is_empty() {
            out.push(ColliderData {
                pieces,
                portalable: class,
                contents,
                brush,
                model,
                sides: sides_c,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube_sides(half: f64) -> Vec<BrushSide> {
        let axes = [DVec3::X, -DVec3::X, DVec3::Y, -DVec3::Y, DVec3::Z, -DVec3::Z];
        let planes: Vec<(DVec3, f64)> = axes.iter().map(|&n| (n, half)).collect();
        planes
            .iter()
            .enumerate()
            .map(|(i, &(n, d))| {
                let others: Vec<_> = planes.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, p)| *p).collect();
                BrushSide {
                    normal: n,
                    dist: d,
                    polygon: side_polygon(n, d, &others),
                    material: Some(i),
                    texinfo_flags: 0,
                }
            })
            .collect()
    }

    #[test]
    fn cube_side_polygons_are_squares() {
        for s in cube_sides(32.0) {
            assert_eq!(s.polygon.len(), 4);
            for p in &s.polygon {
                assert!((p.abs().max_element() - 32.0).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn mixed_brush_splits_into_two_classes() {
        let sides = cube_sides(32.0);
        let cols = brush_colliders(
            &sides,
            CONTENTS_SOLID,
            0,
            0,
            DVec3::ZERO,
            &|s, _| s.normal == DVec3::X,
            &|_| (String::new(), None),
        );
        assert_eq!(cols.len(), 2);
        let port = cols.iter().find(|c| c.portalable).unwrap();
        assert_eq!(port.pieces.len(), 1);
        assert_eq!(port.pieces[0].len(), 5); // square + apex
        let non = cols.iter().find(|c| !c.portalable).unwrap();
        assert_eq!(non.pieces.len(), 5);

        let uniform = brush_colliders(&sides, CONTENTS_SOLID, 0, 0, DVec3::ZERO, &|_, _| false, &|_| (String::new(), None));
        assert_eq!(uniform.len(), 1);
        assert_eq!(uniform[0].pieces.len(), 1);
    }
}

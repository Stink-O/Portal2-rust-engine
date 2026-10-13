//! Portal 2's placement rules and the shot trace (research/specs/portal-placement.md),
//! as pure maths over a [`World`]. No ECS here, so every rule is unit-tested
//! against fake geometry and asserts the exact resulting centre and orientation.
use bevy::math::{Vec2, Vec3};
use bevy::prelude::Entity;
use p2_core::{PortalFrame, PortalPair, ShotFailReason, PORTAL_HALF_SIZE, U};

/// A solid hit along a ray.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    /// The collider that was hit.
    pub entity: Entity,
    /// World point of the hit.
    pub point: Vec3,
    /// Surface normal at the hit, facing the ray origin.
    pub normal: Vec3,
    /// Distance from the ray origin to the hit.
    pub distance: f32,
}

/// What the placement rules need to know about the world (metres, Y up).
/// Shots and probes ignore helper colliders, dynamic bodies and the shooter;
/// the implementor decides that, not the rules.
pub trait World {
    /// Nearest solid hit along `origin + t * dir` for `0 <= t <= max`.
    /// `dir` is unit length.
    fn ray(&self, origin: Vec3, dir: Vec3, max: f32) -> Option<Hit>;
    /// Whether the surface of this entity takes portals (`Portalable`).
    fn portalable(&self, entity: Entity) -> bool;
}

/// Max bump rounds (`FitPortalOnSurface` depth 5).
pub const MAX_BUMPS: usize = 5;
/// Maximum portal segments a shot may take through linked portals.
pub const MAX_PORTAL_SEGMENTS: usize = 16;

/// Largest total slide a fit may make: sqrt(2 (hw^2 + hh^2)), 91.2 u for a Portal 2 portal.
pub fn max_slide(half: Vec2) -> f32 {
    (2.0 * (half.x * half.x + half.y * half.y)).sqrt()
}

/// Probe for a portalable surface right behind `p` on the plane of `n`:
/// trace `p + n` to `p - n` and require a portalable face facing `n`
/// at one unit. Returns that surface's entity.
pub fn surface_at(w: &dyn World, p: Vec3, n: Vec3) -> Option<Entity> {
    let h = w.ray(p + n * U, -n, 2.0 * U)?;
    (h.distance > 0.0 && (h.distance - U).abs() < 0.5 * U && h.normal.dot(n) > 0.98 && w.portalable(h.entity))
        .then_some(h.entity)
}

/// The nearest thing sticking out of the surface between `from` and `to`
/// (traced one unit in front of the surface, so the surface itself is not hit).
fn obstruction(w: &dyn World, from: Vec3, to: Vec3, n: Vec3) -> Option<f32> {
    let d = to - from;
    let len = d.length();
    if len <= 0.0 {
        return None;
    }
    w.ray(from + n * U, d / len, len).map(|h| h.distance)
}

/// Distance from `from` toward `to` at which the usable surface stops:
/// an obstruction, or the surface ending or turning non-portalable
/// (5 % marches, then a 0.01 u bisection, like `TraceAlongSurface`).
fn blocked_along(w: &dyn World, from: Vec3, to: Vec3, n: Vec3) -> Option<f32> {
    let len = from.distance(to);
    if len <= 0.0 {
        return None;
    }
    let dir = (to - from) / len;
    let ok = |t: f32| surface_at(w, from + dir * t, n).is_some();
    let mut best = obstruction(w, from, to, n);
    let step = len * 0.05;
    let mut good = 0.0f32;
    let mut s = step;
    while s <= len + 1e-6 {
        let t = s.min(len);
        if !ok(t) {
            let (mut lo, mut hi) = (good, t);
            for _ in 0..100 {
                if hi - lo <= 0.01 * U {
                    break;
                }
                let mid = 0.5 * (lo + hi);
                if ok(mid) {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            best = Some(best.map_or(lo, |b: f32| b.min(lo)));
            break;
        }
        good = t;
        s += step;
    }
    best
}

/// One blocked half-axis. `dir` points from the centre to the block, `t` is
/// the distance to it, `half` the portal's half extent on that axis.
#[derive(Clone, Copy, Debug)]
struct Block {
    dir: Vec3,
    t: f32,
    half: f32,
}

impl Block {
    /// Distance to move the centre away from the block so the portal edge
    /// ends 2 u clear of the surface end (spec §4 step 3: edge distance + 2 u).
    fn push(&self) -> f32 {
        self.half - self.t + 2.0 * U
    }
}

/// Two opposite blocks (a strip narrower than the portal): move one half
/// extent toward whichever side is free, or fail if neither is.
fn opposing(w: &dyn World, n: Vec3, pos: Vec3, a: Block, b: Block) -> Option<Vec3> {
    let free = |dir: Vec3, h: f32| surface_at(w, pos - dir * 2.0 * h, n).is_some();
    if free(a.dir, a.half) {
        Some(-a.dir * a.half)
    } else if free(b.dir, b.half) {
        Some(-b.dir * b.half)
    } else {
        None
    }
}

/// Two blocks at an angle: solve the centre move that pushes both edges
/// off their blocks at once (exact for perpendicular blocks).
fn corner(a: Block, b: Block) -> Vec3 {
    let (pa, pb, d) = (a.push(), b.push(), a.dir.dot(b.dir));
    let det = 1.0 - d * d;
    let al = (-pa + d * pb) / det;
    let be = (-pb + d * pa) / det;
    al * a.dir + be * b.dir
}

/// Fit a portal on its surface, sliding it off edges and obstructions
/// (`FitPortalOnSurface`): up to [`MAX_BUMPS`] rounds, total slide within
/// [`max_slide`]. Returns the fitted frame, or `None` if it does not fit.
pub fn fit_on_surface(w: &dyn World, frame: PortalFrame, half: Vec2) -> Option<PortalFrame> {
    let n = frame.normal();
    let (r, u) = (frame.right(), frame.up());
    let start = frame.position;
    let mut pos = start;
    for depth in 0..=MAX_BUMPS {
        let mut blocks: Vec<Block> = Vec::with_capacity(4);
        for (dir, h) in [(u, half.y), (-u, half.y), (r, half.x), (-r, half.x)] {
            let reach = (h - 2.0 * U).max(0.0);
            if let Some(t) = blocked_along(w, pos, pos + dir * reach, n) {
                blocks.push(Block { dir, t, half: h });
            }
        }
        if blocks.is_empty() {
            return ((pos - start).length() <= max_slide(half) + 1e-5).then(|| PortalFrame::new(pos, frame.rotation));
        }
        if depth == MAX_BUMPS {
            return None;
        }
        let delta = match blocks.as_slice() {
            [a] => -a.dir * a.push(),
            [a, b] if a.dir.dot(b.dir) < -0.9 => opposing(w, n, pos, *a, *b)?,
            [a, b] => corner(*a, *b),
            [_, _, _] => {
                // Two opposite blocks plus one across the strip: fit the strip,
                // then also push out of the third edge.
                let (i, j, k) = [(0, 1, 2), (0, 2, 1), (1, 2, 0)]
                    .into_iter()
                    .find(|&(i, j, _)| blocks[i].dir.dot(blocks[j].dir) < -0.9)?;
                opposing(w, n, pos, blocks[i], blocks[j])? + (-blocks[k].dir * blocks[k].push())
            }
            _ => return None,
        };
        pos += delta;
    }
    None
}

/// Wall portals shot within 1.5 u of a floor land flush on it (spec §3 step 6).
/// Returns the (possibly moved) centre.
pub fn floor_snap(w: &dyn World, frame: &PortalFrame, half: Vec2) -> Vec3 {
    let (n, up) = (frame.normal(), frame.up());
    let pos = frame.position;
    if up.y <= 0.7 {
        return pos;
    }
    let from = pos + n * (0.05 * U);
    let down = -up;
    let far = w.ray(from, down, half.y + 1.5 * U);
    let near = w.ray(from, down, half.y - 0.1 * U);
    match (far, near) {
        (Some(h), None) => h.point - n * (0.05 * U) + up * half.y,
        _ => pos,
    }
}

/// The surfaces behind the centre and the four corners inset 2.2 u (spec §3
/// step 9). Returns the distinct surface entities, or `None` if any probe fails.
pub fn corner_surfaces(w: &dyn World, frame: &PortalFrame, half: Vec2) -> Option<Vec<Entity>> {
    let (n, r, u) = (frame.normal(), frame.right(), frame.up());
    let (hw, hh) = (half.x - 2.2 * U, half.y - 2.2 * U);
    let mut out: Vec<Entity> = Vec::with_capacity(5);
    for (x, y) in [(0.0, 0.0), (hw, hh), (-hw, hh), (hw, -hh), (-hw, -hh)] {
        let e = surface_at(w, frame.position + r * x + u * y, n)?;
        if !out.contains(&e) {
            out.push(e);
        }
    }
    Some(out)
}

/// An open portal the new one must not overlap.
#[derive(Clone, Copy, Debug)]
pub struct Open {
    /// Frame of the open portal.
    pub frame: PortalFrame,
    /// Its half extents in metres.
    pub half: Vec2,
}

/// Portal 2's overlap test (spec §5): only portals facing the same way
/// (dot >= 0.95). The new portal is a 1 u thin box; the other a box 64 u deep
/// in front of it. Oriented-box overlap.
pub fn overlaps(new: &PortalFrame, new_half: Vec2, other: &PortalFrame, other_half: Vec2) -> bool {
    if new.normal().dot(other.normal()) < 0.95 {
        return false;
    }
    let a_c = new.position + new.normal() * (0.5 * U);
    let a_ax = [new.normal(), new.right(), new.up()];
    let a_h = [0.5 * U, new_half.x, new_half.y];
    let b_c = other.position + other.normal() * (32.0 * U);
    let b_ax = [other.normal(), other.right(), other.up()];
    let b_h = [32.0 * U, other_half.x, other_half.y];
    obb_overlap(a_c, a_ax, a_h, b_c, b_ax, b_h)
}

/// Separating-axis test for two oriented boxes (centre, axes, half extents).
pub fn obb_overlap(ac: Vec3, aa: [Vec3; 3], ah: [f32; 3], bc: Vec3, ba: [Vec3; 3], bh: [f32; 3]) -> bool {
    let d = bc - ac;
    let mut axes: Vec<Vec3> = Vec::with_capacity(15);
    axes.extend_from_slice(&aa);
    axes.extend_from_slice(&ba);
    for x in aa {
        for y in ba {
            let c = x.cross(y);
            if c.length_squared() > 1e-8 {
                axes.push(c.normalize());
            }
        }
    }
    for axis in axes {
        let ra: f32 = (0..3).map(|i| ah[i] * aa[i].dot(axis).abs()).sum();
        let rb: f32 = (0..3).map(|i| bh[i] * ba[i].dot(axis).abs()).sum();
        if d.dot(axis).abs() > ra + rb {
            return false;
        }
    }
    true
}

/// Initial frame of a shot (spec §2.2): the centre is the hit point. On walls
/// up is world up projected onto the surface. Floors and ceilings (normal
/// exactly vertical) take up from the shot direction.
pub fn initial_frame(point: Vec3, normal: Vec3, shot_dir: Vec3) -> Option<PortalFrame> {
    let n = normal.try_normalize()?;
    let vertical = n.x.abs() < 0.001 && n.z.abs() < 0.001;
    let up = if vertical { shot_dir } else { Vec3::Y };
    PortalFrame::from_normal_up(point, n, up).or_else(|| PortalFrame::from_normal_up(point, n, Vec3::NEG_Z))
}

/// A portal that a shot placed.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    /// The fitted frame: centre of the opening, +Z out of the surface.
    pub frame: PortalFrame,
    /// The colliders behind the opening (centre and inset corners).
    pub surfaces: Vec<Entity>,
}

/// The whole placement of one shot whose final leg hit `hit`, travelling
/// `shot_dir`, given the other open portals. Checks run in spec order.
pub fn place(
    w: &dyn World,
    hit: Hit,
    shot_dir: Vec3,
    half: Vec2,
    others: &[Open],
) -> Result<Placed, ShotFailReason> {
    if !w.portalable(hit.entity) {
        return Err(ShotFailReason::NotPortalable);
    }
    let frame = initial_frame(hit.point, hit.normal, shot_dir).ok_or(ShotFailReason::DoesNotFit)?;
    surface_at(w, frame.position, frame.normal()).ok_or(ShotFailReason::DoesNotFit)?;
    let mut fitted = fit_on_surface(w, frame, half).ok_or(ShotFailReason::DoesNotFit)?;
    fitted.position = floor_snap(w, &fitted, half);
    // Final position is snapped to 1/512 u before use (spec §5).
    let grid = U / 512.0;
    fitted.position = (fitted.position / grid).round() * grid;
    if others.iter().any(|o| overlaps(&fitted, half, &o.frame, o.half)) {
        return Err(ShotFailReason::Overlap);
    }
    let surfaces = corner_surfaces(w, &fitted, half).ok_or(ShotFailReason::DoesNotFit)?;
    Ok(Placed { frame: fitted, surfaces })
}

/// A linked portal the shot may pass through: its opening and the frame of its partner.
#[derive(Clone, Copy, Debug)]
pub struct PortalWindow {
    /// The portal entity (for the caller's bookkeeping).
    pub entity: Entity,
    /// Its frame.
    pub frame: PortalFrame,
    /// Its half extents in metres.
    pub half: Vec2,
    /// `None` when the portal is not linked (a shot does not pass it).
    pub exit: Option<PortalFrame>,
}

/// Result of tracing a shot through linked portals.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Trace {
    /// Final solid hit, if the shot ended on a surface within range.
    pub hit: Option<Hit>,
    /// Where the shot ends: the hit point, or far along the last leg on a miss.
    pub end: Vec3,
    /// Direction of the last leg (after any portal bends).
    pub dir: Vec3,
}

/// Trace a shot from `origin` along `dir` for `range`, passing through the
/// front of linked portals (up to [`MAX_PORTAL_SEGMENTS`] segments).
pub fn trace_shot(w: &dyn World, origin: Vec3, dir: Vec3, range: f32, windows: &[PortalWindow]) -> Trace {
    let mut o = origin;
    let mut d = dir.normalize();
    let mut remaining = range;
    for _ in 0..MAX_PORTAL_SEGMENTS {
        let hit = w.ray(o, d, remaining);
        let limit = hit.map_or(remaining, |h| h.distance);
        let mut best: Option<(f32, PortalWindow, PortalFrame)> = None;
        for pw in windows {
            let Some(exit) = pw.exit else { continue };
            let n = pw.frame.normal();
            let denom = d.dot(n);
            if denom >= -1e-5 {
                continue;
            }
            let t = (pw.frame.position - o).dot(n) / denom;
            if t < 1e-3 || t > limit + 0.01 {
                continue;
            }
            let l = pw.frame.to_local(o + d * t);
            if l.x.abs() > pw.half.x || l.y.abs() > pw.half.y {
                continue;
            }
            if best.as_ref().is_none_or(|(bt, ..)| t < *bt) {
                best = Some((t, *pw, exit));
            }
        }
        if let Some((t, pw, exit)) = best {
            let pair = PortalPair::new(pw.frame, exit);
            let p = o + d * t;
            o = pair.map_point(p) + pair.map_dir(d) * 1e-3;
            d = pair.map_dir(d).normalize();
            remaining -= t;
            continue;
        }
        return match hit {
            Some(h) => Trace { hit: Some(h), end: h.point, dir: d },
            None => Trace { hit: None, end: o + d * remaining, dir: d },
        };
    }
    Trace { hit: None, end: o, dir: d }
}

/// Default portal opening (64 x 112 u).
pub const PORTAL_SIZE: Vec2 = PORTAL_HALF_SIZE;

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_1_SQRT_2;

    /// A flat, finite quad in a plane (for the fake world).
    #[derive(Clone, Copy)]
    struct Quad {
        e: Entity,
        c: Vec3,
        n: Vec3,
        r: Vec3,
        u: Vec3,
        half: Vec2,
        portalable: bool,
    }

    fn quad(id: u32, c: Vec3, n: Vec3, r: Vec3, u: Vec3, half: Vec2) -> Quad {
        Quad { e: Entity::from_raw_u32(id).unwrap(), c, n: n.normalize(), r, u, half, portalable: true }
    }

    /// Rectangle `x0..x1 by y0..y1` on the plane z = 0, facing +Z.
    fn wall_z(id: u32, x0: f32, x1: f32, y0: f32, y1: f32) -> Quad {
        quad(id, Vec3::new((x0 + x1) / 2.0, (y0 + y1) / 2.0, 0.0), Vec3::Z, Vec3::X, Vec3::Y, Vec2::new((x1 - x0) / 2.0, (y1 - y0) / 2.0))
    }

    struct Quads(Vec<Quad>);

    impl World for Quads {
        fn ray(&self, o: Vec3, d: Vec3, max: f32) -> Option<Hit> {
            let mut best: Option<Hit> = None;
            for q in &self.0 {
                let denom = d.dot(q.n);
                if denom.abs() < 1e-9 {
                    continue;
                }
                let t = (q.c - o).dot(q.n) / denom;
                if t < 0.0 || t > max {
                    continue;
                }
                let p = o + d * t;
                let l = p - q.c;
                if l.dot(q.r).abs() > q.half.x || l.dot(q.u).abs() > q.half.y {
                    continue;
                }
                if best.is_none_or(|b| t < b.distance) {
                    best = Some(Hit { entity: q.e, point: p, normal: q.n, distance: t });
                }
            }
            best
        }
        fn portalable(&self, e: Entity) -> bool {
            self.0.iter().find(|q| q.e == e).is_some_and(|q| q.portalable)
        }
    }

    fn v(x: f32, y: f32, z: f32) -> Vec3 {
        Vec3::new(x, y, z)
    }

    fn close(a: Vec3, b: Vec3, tol: f32) -> bool {
        (a - b).length() <= tol
    }

    fn assert_close(a: Vec3, b: Vec3, tol: f32, what: &str) {
        assert!(close(a, b, tol), "{what}: got {a:?}, expected {b:?} (tol {tol})");
    }

    #[test]
    fn size_and_slide_limit() {
        // Spec §1: 64 x 112 u opening, half 32 x 56 u.
        assert!((PORTAL_SIZE.x / U - 32.0).abs() < 1e-3 && (PORTAL_SIZE.y / U - 56.0).abs() < 1e-3);
        // Spec §3 step 5: sqrt(2 (32^2 + 56^2)) = 91.2 u.
        assert!((max_slide(PORTAL_SIZE) / U - 91.21).abs() < 0.01);
    }

    #[test]
    fn open_wall_keeps_its_centre() {
        let w = Quads(vec![wall_z(1, -10.0, 10.0, -10.0, 10.0)]);
        let frame = PortalFrame::from_normal_up(v(0.2, 0.1, 0.0), Vec3::Z, Vec3::Y).unwrap();
        let fitted = fit_on_surface(&w, frame, PORTAL_SIZE).expect("fits");
        assert_close(fitted.position, v(0.2, 0.1, 0.0), 1e-5, "centre");
    }

    #[test]
    fn bumps_off_a_surface_end_with_two_units_clearance() {
        // The wall ends at x = 0.3 m. Shot at its centre, the right edge hangs over.
        // Right edge must land 2 u inside the end: centre x = 0.3 - 32u - 2u.
        let w = Quads(vec![wall_z(1, -10.0, 0.3, -10.0, 10.0)]);
        let frame = PortalFrame::from_normal_up(Vec3::ZERO, Vec3::Z, Vec3::Y).unwrap();
        let fitted = fit_on_surface(&w, frame, PORTAL_SIZE).expect("bumps and fits");
        let expect = v(0.3 - PORTAL_SIZE.x - 2.0 * U, 0.0, 0.0);
        assert_close(fitted.position, expect, 5e-4, "centre after edge bump");
        assert!((fitted.position.x - (-0.5636)).abs() < 5e-4);
        assert!(corner_surfaces(&w, &fitted, PORTAL_SIZE).is_some(), "corners on surface after bump");
    }

    #[test]
    fn bumps_out_of_a_corner_on_both_axes() {
        // An L-shaped surface: x <= 0.3 and y <= 0.3. Centre is inside the corner.
        // Bumps: right by 32u + 2u - 0.3 m ... both pushes solved together.
        let w = Quads(vec![wall_z(1, -10.0, 0.3, -10.0, 0.3)]);
        let frame = PortalFrame::from_normal_up(Vec3::ZERO, Vec3::Z, Vec3::Y).unwrap();
        let fitted = fit_on_surface(&w, frame, PORTAL_SIZE).expect("fits out of the corner");
        let expect = v(0.3 - PORTAL_SIZE.x - 2.0 * U, 0.3 - PORTAL_SIZE.y - 2.0 * U, 0.0);
        assert_close(fitted.position, expect, 5e-4, "centre after corner bump");
        assert!((fitted.position.y - (-1.1732)).abs() < 5e-4);
        assert!(corner_surfaces(&w, &fitted, PORTAL_SIZE).is_some());
    }

    #[test]
    fn too_narrow_a_strip_does_not_fit() {
        // A 1.4 m strip is narrower than the 1.63 m opening: no fit.
        let w = Quads(vec![wall_z(1, -0.7, 0.7, -10.0, 10.0)]);
        let frame = PortalFrame::from_normal_up(Vec3::ZERO, Vec3::Z, Vec3::Y).unwrap();
        assert!(fit_on_surface(&w, frame, PORTAL_SIZE).is_none());
    }

    #[test]
    fn floor_snap_lands_a_nearby_wall_portal_on_the_floor() {
        // Floor at y = 0 under a wall at z = 0. Shot 0.02 m above the snap
        // threshold: the bottom edge sits 0.8 u above the floor, within 1.5 u.
        let w = Quads(vec![
            quad(1, v(0.0, 5.0, 0.0), Vec3::Z, Vec3::X, Vec3::Y, Vec2::new(10.0, 5.0)),
            quad(2, Vec3::ZERO, Vec3::Y, Vec3::X, Vec3::Z, Vec2::new(10.0, 10.0)),
        ]);
        let hh = PORTAL_SIZE.y;
        let frame = PortalFrame::from_normal_up(v(0.0, hh + 0.02, 0.0), Vec3::Z, Vec3::Y).unwrap();
        let snapped = floor_snap(&w, &frame, PORTAL_SIZE);
        assert_close(snapped, v(0.0, hh, 0.0), 1e-4, "snapped centre (bottom edge on the floor)");
        // Well above the floor: no snap.
        let high = PortalFrame::from_normal_up(v(0.0, 3.0, 0.0), Vec3::Z, Vec3::Y).unwrap();
        assert_close(floor_snap(&w, &high, PORTAL_SIZE), v(0.0, 3.0, 0.0), 1e-6, "no snap high up");
    }

    #[test]
    fn floor_and_ceiling_orient_along_the_shot() {
        // Floor: normal up, up follows the horizontal shot direction (+X here).
        let f = initial_frame(Vec3::ZERO, Vec3::Y, v(1.0, -1.0, 0.0)).unwrap();
        assert_close(f.normal(), Vec3::Y, 1e-5, "floor normal");
        assert_close(f.up(), Vec3::X, 1e-5, "floor up = shot heading");
        // Ceiling: normal down, up follows the horizontal shot direction (-Z here).
        let c = initial_frame(Vec3::ZERO, Vec3::NEG_Y, v(0.0, 1.0, -1.0)).unwrap();
        assert_close(c.normal(), Vec3::NEG_Y, 1e-5, "ceiling normal");
        assert_close(c.up(), Vec3::NEG_Z, 1e-5, "ceiling up = shot heading");
        // Wall: up is world up.
        let w = initial_frame(Vec3::ZERO, Vec3::NEG_X, v(-1.0, 0.0, 0.0)).unwrap();
        assert_close(w.up(), Vec3::Y, 1e-5, "wall up = world up");
        // Slope: world up projected onto the slope.
        let n = v(1.0, 1.0, 0.0).normalize();
        let s = initial_frame(Vec3::ZERO, n, Vec3::NEG_Z).unwrap();
        assert_close(s.up(), v(-FRAC_1_SQRT_2, FRAC_1_SQRT_2, 0.0), 1e-5, "slope up = projected world up");
    }

    #[test]
    fn overlap_same_facing_only_and_exact_extent() {
        let hs = PORTAL_SIZE;
        let a = PortalFrame::from_normal_up(Vec3::ZERO, Vec3::Z, Vec3::Y).unwrap();
        let at = |p: Vec3, n: Vec3| PortalFrame::from_normal_up(p, n, Vec3::Y).unwrap();
        // 0.5 m sideways on the same wall: overlaps.
        assert!(overlaps(&at(v(0.5, 0.0, 0.0), Vec3::Z), hs, &a, hs));
        // 2.0 m sideways: the openings are 1.6256 m across, so clear.
        assert!(!overlaps(&at(v(2.0, 0.0, 0.0), Vec3::Z), hs, &a, hs));
        // Perpendicular facing: never overlaps.
        assert!(!overlaps(&at(v(0.5, 0.0, 0.0), Vec3::X), hs, &a, hs));
        // Behind the other portal (away from its front): clear.
        assert!(!overlaps(&at(v(0.0, 0.0, -1.0), Vec3::Z), hs, &a, hs));
        // In front of it, inside its 64 u deep box: overlaps.
        assert!(overlaps(&at(v(0.0, 0.0, 1.0), Vec3::Z), hs, &a, hs));
    }

    #[test]
    fn place_success_and_each_failure() {
        let half = PORTAL_SIZE;
        let mut wall = wall_z(1, -10.0, 10.0, -10.0, 10.0);
        let hit = |e: u32, p: Vec3| Hit { entity: Entity::from_raw_u32(e).unwrap(), point: p, normal: Vec3::Z, distance: 1.0 };

        // Success on open wall: centre is the hit point, surface is the wall.
        let w = Quads(vec![wall]);
        let ok = place(&w, hit(1, v(0.5, 0.25, 0.0)), Vec3::NEG_Z, half, &[]).expect("placed");
        assert_close(ok.frame.position, v(0.5, 0.25, 0.0), 1e-4, "placed centre (within one 1/512 u snap step)");
        assert_eq!(ok.surfaces, vec![Entity::from_raw_u32(1).unwrap()]);

        // Not portalable.
        wall.portalable = false;
        let w = Quads(vec![wall]);
        assert_eq!(place(&w, hit(1, Vec3::ZERO), Vec3::NEG_Z, half, &[]).unwrap_err(), ShotFailReason::NotPortalable);

        // Overlap with the other portal.
        let wall = wall_z(1, -10.0, 10.0, -10.0, 10.0);
        let w = Quads(vec![wall]);
        let other = Open { frame: PortalFrame::from_normal_up(v(0.6, 0.0, 0.0), Vec3::Z, Vec3::Y).unwrap(), half };
        assert_eq!(place(&w, hit(1, Vec3::ZERO), Vec3::NEG_Z, half, &[other]).unwrap_err(), ShotFailReason::Overlap);

        // Too narrow: DoesNotFit.
        let w = Quads(vec![wall_z(1, -0.7, 0.7, -10.0, 10.0)]);
        assert_eq!(place(&w, hit(1, Vec3::ZERO), Vec3::NEG_Z, half, &[]).unwrap_err(), ShotFailReason::DoesNotFit);
    }

    #[test]
    fn shot_passes_through_a_linked_pair_and_lands_on_the_far_wall() {
        // Blue on the wall z = 0 (facing +Z), orange on the wall x = 10 (facing -X),
        // and a room wall at x = 5 facing +X. A shot from (0,1,2) along -Z goes
        // through blue, comes out of orange and runs -X into the x = 5 wall.
        let blue_f = PortalFrame::from_normal_up(v(0.0, 1.0, 0.0), Vec3::Z, Vec3::Y).unwrap();
        let orange_f = PortalFrame::from_normal_up(v(10.0, 1.0, 0.0), Vec3::NEG_X, Vec3::Y).unwrap();
        let w = Quads(vec![
            wall_z(1, -5.0, 5.0, -4.0, 6.0),
            quad(2, v(10.0, 1.0, 0.0), Vec3::NEG_X, Vec3::Z, Vec3::Y, Vec2::new(5.0, 5.0)),
            quad(3, v(5.0, 1.0, 0.0), Vec3::X, Vec3::Z, Vec3::Y, Vec2::new(5.0, 5.0)),
        ]);
        let windows = [
            PortalWindow { entity: Entity::from_raw_u32(10).unwrap(), frame: blue_f, half: PORTAL_SIZE, exit: Some(orange_f) },
            PortalWindow { entity: Entity::from_raw_u32(11).unwrap(), frame: orange_f, half: PORTAL_SIZE, exit: Some(blue_f) },
        ];
        let t = trace_shot(&w, v(0.0, 1.0, 2.0), Vec3::NEG_Z, 100.0, &windows);
        let hit = t.hit.expect("hits the far wall");
        assert_eq!(hit.entity, Entity::from_raw_u32(3).unwrap());
        assert_close(hit.point, v(5.0, 1.0, 0.0), 1e-3, "far wall hit point");
        assert_close(t.dir, Vec3::NEG_X, 1e-4, "leaves orange heading -X");

        // Unlinked: the shot hits the blue wall itself.
        let unlinked = [PortalWindow { exit: None, ..windows[0] }];
        let t = trace_shot(&w, v(0.0, 1.0, 2.0), Vec3::NEG_Z, 100.0, &unlinked);
        assert_eq!(t.hit.unwrap().entity, Entity::from_raw_u32(1).unwrap());
        assert_close(t.end, v(0.0, 1.0, 0.0), 1e-4, "stops at the wall");
    }
}

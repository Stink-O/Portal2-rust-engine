//! `.phy` collision models (VPhysics / IVP compact surfaces) as convex
//! pieces, for static-prop collision.
//!
//! Layout: a 16-byte header, then per solid an `i32` size followed by a
//! `VPHY` compact-surface header and the IVP compact surface: a ledge tree
//! whose leaf nodes point at convex ledges (triangles over a point array).

use bevy::math::Vec3;

fn i32_at(d: &[u8], o: usize) -> Option<i32> {
    Some(i32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}
fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}
fn f32_at(d: &[u8], o: usize) -> Option<f32> {
    Some(f32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

/// IVP metres (Y down, Z back) to world metres (Y up).
fn ivp_to_world(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, -y, -z)
}

/// Convex pieces of every solid, in world axes and metres, model space.
pub fn parse_phy(data: &[u8]) -> Vec<Vec<Vec3>> {
    let mut out = Vec::new();
    let Some(header_size) = i32_at(data, 0) else { return out };
    let Some(solid_count) = i32_at(data, 8) else { return out };
    let mut off = header_size.max(16) as usize;
    for _ in 0..solid_count.clamp(0, 256) {
        let Some(size) = i32_at(data, off) else { break };
        let start = off + 4;
        let end = (start + size.max(0) as usize).min(data.len());
        if data.get(start..start + 4) == Some(b"VPHY") {
            // compactsurfaceheader_t is 28 bytes after the size field
            parse_compact_surface(data, start + 28, end, &mut out);
        } else {
            // legacy header: 4-byte size already consumed, 44 more bytes of header? Legacy
            // surfaces start directly with the IVP compact surface.
            parse_compact_surface(data, start, end, &mut out);
        }
        off = end;
    }
    out
}

fn parse_compact_surface(d: &[u8], surf: usize, end: usize, out: &mut Vec<Vec<Vec3>>) {
    // IVP_Compact_Surface: mass_center[3], rotation_inertia[3], upper_limit_radius,
    // max_deviation:8|byte_size:24, offset_ledgetree_root, dummy[3]
    let Some(root_off) = i32_at(d, surf + 32) else { return };
    let root = surf as i64 + root_off as i64;
    if root <= surf as i64 || root as usize >= end {
        return;
    }
    let mut stack = vec![root as usize];
    let mut guard = 0;
    while let Some(node) = stack.pop() {
        guard += 1;
        if guard > 100_000 || node + 28 > end {
            break;
        }
        // IVP_Compact_Ledgetree_Node: offset_right_node, offset_compact_ledge, center[3], radius, box_sizes[3], free
        let Some(right) = i32_at(d, node) else { break };
        let Some(ledge_off) = i32_at(d, node + 4) else { break };
        if right == 0 {
            if ledge_off != 0 {
                let ledge = (node as i64 + ledge_off as i64) as usize;
                if let Some(pts) = parse_ledge(d, ledge) {
                    out.push(pts);
                }
            }
        } else {
            stack.push(node + 28);
            let r = node as i64 + right as i64;
            if r > 0 && (r as usize) < end {
                stack.push(r as usize);
            }
        }
    }
}

fn parse_ledge(d: &[u8], ledge: usize) -> Option<Vec<Vec3>> {
    let point_off = i32_at(d, ledge)?;
    let n_tris = i16::from_le_bytes(d.get(ledge + 12..ledge + 14)?.try_into().ok()?) as usize;
    let _flags = u32_at(d, ledge + 8)?;
    let points = (ledge as i64 + point_off as i64) as usize;
    let mut idx = Vec::with_capacity(n_tris * 3);
    for t in 0..n_tris {
        let tri = ledge + 16 + t * 16;
        for e in 0..3 {
            let edge = u32_at(d, tri + 4 + e * 4)?;
            idx.push((edge & 0xffff) as usize);
        }
    }
    idx.sort_unstable();
    idx.dedup();
    if idx.len() < 4 {
        return None;
    }
    let mut pts = Vec::with_capacity(idx.len());
    for i in idx {
        let p = points + i * 16;
        pts.push(ivp_to_world(f32_at(d, p)?, f32_at(d, p + 4)?, f32_at(d, p + 8)?));
    }
    pts.iter().all(|p| p.is_finite() && p.abs().max_element() < 1000.0).then_some(pts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_phy() {
        let cfg = crate::config::GameConfig::from_env();
        if !cfg.root.join("portal2").is_dir() {
            return;
        }
        let fs = crate::fs::GameFs::mount(cfg).unwrap();
        let data = fs.read("models/props/metal_box.phy").unwrap();
        let pieces = parse_phy(&data);
        assert!(!pieces.is_empty());
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for p in pieces.iter().flatten() {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
        let size = hi - lo;
        // The collision hull matches the rendered cube's extent.
        let m = crate::models::load_model("models/props/metal_box.mdl", &|p| fs.read(p), &|_| false).unwrap();
        let (mut rlo, mut rhi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for p in m.meshes.iter().flat_map(|m| m.positions.iter()) {
            rlo = rlo.min(Vec3::from(*p));
            rhi = rhi.max(Vec3::from(*p));
        }
        eprintln!("phy {lo} {hi} render {rlo} {rhi}");
        assert!(((rhi - rlo) - size).abs().max_element() < 0.15, "{size} vs {}", rhi - rlo);
        assert!((lo - rlo).abs().max_element() < 0.15);
    }
}

//! Print what a map converts to, without opening a window.
//!
//! cargo run -p source_assets --example info -- sp_a1_intro3 [--near x,y,z radius_m]
//! (`--near` is world metres; lists entities and static props within the radius.)

use source_assets::entities::source_angles_to_world;
use source_assets::{map, surfaceprops::SurfaceProps, GameFs};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let name = args.first().cloned().unwrap_or_else(|| "sp_a1_intro3".into());
    let fs = GameFs::from_env()?;
    for m in fs.describe_mounts() {
        println!("mount {m}");
    }
    let sp = SurfaceProps::load(&fs);
    let map = map::load_map(&fs, &sp, &name)?;
    println!("timings {:?}", map.timings);
    let tris: usize = map.brush_meshes.iter().map(|b| b.mesh.triangle_count()).sum();
    println!(
        "{}: {} brush meshes / {tris} tris, lightmap {}x{} (hdr {}), {} colliders ({} portalable), {} static props ({} with vhv), {} models, {} textures, sky {:?}",
        map.name,
        map.brush_meshes.len(),
        map.lightmap.width,
        map.lightmap.height,
        map.lightmap.hdr,
        map.colliders.len(),
        map.colliders.iter().filter(|c| c.portalable).count(),
        map.static_props.len(),
        map.static_props.iter().filter(|p| p.vertex_light.is_some()).count(),
        map.models.len(),
        map.textures.len(),
        map.skyname
    );
    // Materials actually drawn: brush meshes and model skins.
    let mut used: std::collections::BTreeSet<usize> = map.brush_meshes.iter().map(|b| b.material).collect();
    for m in map.models.values() {
        for n in m.all_materials() {
            if let Some(i) = map.material_id(n) {
                used.insert(i);
            }
        }
    }
    let missing: Vec<_> = used
        .iter()
        .map(|&i| &map.materials[i])
        .filter(|m| m.is_drawn() && (m.shader == "missing" || m.base_texture.as_ref().is_some_and(|t| !map.textures.contains_key(t))))
        .map(|m| format!("{} ({})", m.name, m.shader))
        .collect();
    println!("missing materials/textures: {missing:?}");

    if let Some(i) = args.iter().position(|a| a == "--near") {
        let v: Vec<f32> = args[i + 1].split(',').filter_map(|s| s.parse().ok()).collect();
        let r: f32 = args.get(i + 2).and_then(|s| s.parse().ok()).unwrap_or(10.0);
        let p = bevy::math::Vec3::new(v[0], v[1], v[2]);
        for e in &map.entities {
            let d = e.world_position().distance(p);
            if d < r {
                println!(
                    "  {:.1}m {} {:?} model={:?} at {:?} fwd {:?}",
                    d,
                    e.classname,
                    e.targetname,
                    e.get("model"),
                    e.world_position(),
                    source_angles_to_world(e.angles) * bevy::math::Vec3::X
                );
            }
        }
        for s in &map.static_props {
            let d = shared::source_to_world(s.origin).distance(p);
            if d < r {
                println!("  {:.1}m prop_static {}", d, s.model);
            }
        }
    }
    Ok(())
}

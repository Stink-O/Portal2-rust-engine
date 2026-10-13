//! Inspect Portal 2's compiled shaders (`shaders/fxc/*.vcs`).
//!
//! cargo run -p source_assets --example shaders -- lightmappedgeneric_ps20b [--statics]
//!
//! Prints the combo counts and, with `--statics`, every present static combo
//! with the constants and samplers its first dynamic combo uses (the basis
//! for mapping combo digits to material features).

use source_assets::vcs::{parse_ctab, shader_version, VcsFile};
use source_assets::GameFs;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let name = args.first().cloned().unwrap_or_else(|| "lightmappedgeneric_ps20b".into());
    let fs = GameFs::from_env()?;
    let data = fs
        .read(&format!("shaders/fxc/{name}.vcs"))
        .ok_or_else(|| anyhow::anyhow!("no shaders/fxc/{name}.vcs"))?;
    let vcs = VcsFile::parse(data)?;
    println!(
        "{name}: {} combos = {} static x {} dynamic; {} static combos stored",
        vcs.total_combos,
        vcs.static_combos(),
        vcs.dynamic_combos,
        vcs.static_ids().count()
    );
    if args.iter().any(|a| a == "--opcodes") {
        let mut hist: std::collections::BTreeMap<String, usize> = Default::default();
        let mut errors = 0;
        for s in vcs.static_ids() {
            for (_, code) in vcs.static_block(s)? {
                let ops = source_assets::sm::opcode_names(&code);
                if ops.iter().any(|o| o.contains("error")) {
                    errors += 1;
                }
                for o in ops {
                    *hist.entry(o).or_default() += 1;
                }
            }
        }
        println!("decode errors: {errors}");
        for (o, n) in hist {
            println!("  {o}: {n}");
        }
    }
    if let Some(i) = args.iter().position(|a| a == "--dyn") {
        let st: u32 = args[i + 1].parse()?;
        for (d, code) in vcs.static_block(st)? {
            let mut names: Vec<String> = parse_ctab(&code).into_iter().map(|c| c.name).collect();
            names.sort();
            println!("dyn {d} {} bytes {} :: {}", code.len(), source_assets::sm::io_summary(&code), names.join(" "));
        }
    }
    if args.iter().any(|a| a == "--statics") {
        for s in vcs.static_ids() {
            let block = vcs.static_block(s)?;
            let Some((d, code)) = block.first() else { continue };
            let ver = shader_version(code).unwrap_or_default();
            let mut names: Vec<String> = block
                .iter()
                .flat_map(|(_, c)| parse_ctab(c))
                .map(|c| format!("{}@{:?}{}", c.name, c.set, c.register))
                .collect();
            names.sort();
            names.dedup();
            println!(
                "static {s} ({} dyn [{}], first {d}, {}_{}_{} {} bytes) texld {:?} {}: {}",
                block.len(),
                block.iter().map(|b| b.0.to_string()).collect::<Vec<_>>().join(","),
                if ver.0 { "ps" } else { "vs" },
                ver.1,
                ver.2,
                code.len(),
                source_assets::sm::texld_samplers(code),
                source_assets::sm::io_summary(code),
                names.join(" ")
            );
        }
    }
    Ok(())
}

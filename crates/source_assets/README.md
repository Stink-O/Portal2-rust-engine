# source_assets

Portal 2 test chambers from the user's own install, in Bevy 0.19 + avian3d 0.7.
A map converts in well under a second (sp_a1_intro3: ~0.08 s warm, including
textures, models and colliders), so there is no on-disk cache.

## What works

- **Filesystem** (`fs.rs`, `config.rs`): search paths mounted the way the
  engine does for Portal 2 (`update/`, `portal2_dlc2/`, `portal2_dlc1/`,
  `portal2/`, `platform/`; each dir's `*_dir.vpk` before its loose files),
  case-insensitive, plus the BSP pakfile on top (`MapFs`). Install path:
  `PORTAL2_DIR`, else `config::DEFAULT_PORTAL2_DIR`. A Portal 1 layout
  (`portal/` + `hl2/`) is detected but untested.
- **Brush geometry** (`geometry.rs`): faces grouped per (brush model,
  material), displacements with smooth normals and WorldVertexTransition blend
  alpha, converted with `shared::source_to_world`. Winding is taken from the
  face plane (`planenum` is already face-oriented; `side` is ignored).
- **Lightmaps**: HDR lighting lump + `LUMP_FACES_HDR` (LDR fallback), style 0,
  flat (non-bump) lightmap, packed into one RGB9E5 atlas per map with 1-texel
  borders. Unlit faces point at a white texel.
- **Materials** (`vmt.rs`, `kv.rs`, `vtf.rs`, `material.rs`): own KeyValues
  parser (platform conditionals, `patch` include/insert/replace). VTF 7.0-7.5
  with the full mip chain; DXT1/3/5 uploaded compressed when the GPU supports
  BC, every other format converted per mip. Tool textures (nodraw, clip,
  trigger, sky...) are skipped by name and `%compile*` keys; `toolsblack` is
  drawn. `$translucent`, `$additive`, `$alphatest`/`$alphatestreference`,
  `$nocull`, `$color`, `$alpha`, `$basetexture2` (WVT blend) are honoured.
  `SourceMaterial` = base texture x baked light x `light_scale`, unaffected
  by Bevy lights, then Bevy's fog / in-shader tonemapping / alpha
  premultiply (same post step as `StandardMaterial`).
- **Props** (`models.rs`, `phy.rs`): static props from the game lump
  (parsed directly; vbsp misreads version 9), entity props (`prop_dynamic`,
  cubes by `CubeType`, test chamber doors, buttons, laser emitters/catchers).
  VTX strips are walked here (vmdl's iterator panics on triangle strips) and a
  model that still fails is logged and skipped (vmdl never sees animation
  data; `skeleton.rs` reads bones, animations and sequences itself). Entity
  props are skinned into frame 0 of their default sequence (`DefaultAnim`,
  per-class defaults such as `idleclose` for test chamber doors, else
  sequence 0), as the game draws them; static props keep their compiled pose.
  Lighting is sampled at the model's `illumposition`, nudged out of solid
  leaves. Static props use their baked
  per-vertex light (`sp_hdr_N.vhv`); other props use the leaf ambient cube at
  their origin. Collision from the `.phy` convex pieces (bbox fallback).
- **Collision** (`collision.rs`): every player-solid brush
  (`SOLID|MOVEABLE|PLAYERCLIP|WINDOW|GRATE`) becomes a convex hull under the
  map root (`RigidBody::Static`). Brushes whose sides disagree about
  portalability are split exactly into per-side pyramids (apex at the brush
  centroid), so each collider entity is either all-portalable or not.
- **Portalability**: a side is portalable when its brush is `SOLID`/`MOVEABLE`
  (not grate/window/clip), the texinfo lacks `SURF_NOPORTAL|SKY|SKY2D|NODRAW|
  TRIGGER|TRANS`, the material is drawn, not `%noportal`, not
  translucent/refract/water, its surfaceprop's game material is not glass
  (`scripts/surfaceproperties*.txt`, with `base` inheritance), and the side is
  outside every enabled `func_noportal_volume`. Static/dynamic props are never
  portalable.
- **Entities** (`entities.rs`): the entity lump with lowercased keys (outputs
  kept), Source angle conversion, typed `MapEntity` list.
- **Fog / sky**: first enabled `env_fog_controller` as `MapFog`
  (`distance_fog()` gives a Bevy `DistanceFog`); the 2D skybox (`skyname`,
  `_hdr` variant preferred) as a cubemap for `Skybox`. Portal 2 fogs its white
  sky, so with fog on use the fog colour as the clear colour (the example does).

## Portal 2's own shaders (`valve.rs`, `valve_render.rs`, `sm.rs`, `vcs.rs`)

Surfaces are drawn with the game's compiled D3D9 shaders when possible:
`vcs.rs` reads `platform/shaders/fxc/*.vcs` (combo directory, LZMA blocks,
CTAB), `sm.rs` links a VS/PS pair and translates it to WGSL with the vendored
`d3d9_sm3`, `valve.rs` picks combos and fills constant registers the way the
Source shader DLL does, and `valve_render.rs` draws every `ValveSurface`
right after Bevy's opaque pass into the same colour/depth targets (one
storage buffer of `vec4` constants per frame, binding arrays of 2D/cube
textures and 8 samplers; needs binding-array support, present on desktop
Vulkan). Disable with `SourceRenderSettings { valve_shaders: false, .. }`
(`SOURCE_NO_VALVE=1` in the example); `SourceMaterial` is the fallback, and
each spawn logs the materials still on it (`SourceCache::fallback_materials`).

- Combo layouts were inferred from which samplers/constants each stored combo
  uses (no combo names ship in the files):
  `lightmappedgeneric_ps20b`: BASETEXTURE2 x1, BUMPMAP x2 (0..2, 2 = ssbump),
  BUMPMAP2 x6, CUBEMAP x12 (0..2), ENVMAPMASK x36, BASEALPHAENVMAPMASK x72,
  SELFILLUM x144, NORMALMAPALPHAENVMAPMASK x288, FANCY_BLENDING x576,
  unknown x1152, DETAIL x2304 (0..11 blend mode, 12 = none); dynamic 0.
  `vertexlit_and_unlit_generic_ps20b`: DETAIL x1, CUBEMAP x2,
  DIFFUSELIGHTING x4, ENVMAPMASK x8, SELFILLUM x32; VS static 0 lit / 16
  unlit, VS dynamic 1 = per-vertex static light from COLOR1.
- Brushes: LightmappedGeneric, WorldVertexTransition (BASETEXTURE2 + vertex
  alpha), UnlitGeneric; bumped lightmaps (the four vrad lightmaps are packed
  side by side, offset in TEXCOORD2), cubemap patch materials from the
  pakfile, $detail, ssbump, self-illum. `info_overlay` decals (signs,
  indicator dots, stains) are quads lit by the face they sit on.
- Models: VertexLitGeneric / UnlitGeneric; static props feed their `.vhv`
  bytes as COLOR1 (decoded by the VS as `(2b/255)^2.2`), dynamic props get
  the leaf ambient cube baked into COLOR1 (no dynamic local lights).
- Exposure: `ValveExposure` is Source's tonemap scale (`cLightScale.x`),
  constant (default 3). Fog is the map's first enabled
  `env_fog_controller`, applied by the shaders themselves (squared vertex fog).

## Public API

```rust
app.add_plugins(source_assets::SourceMapPlugin::default()); // mounts the game, registers SourceMaterial

fn setup(mut commands: Commands, mut assets: source_assets::SourceMapAssets) {
    let info = source_assets::spawn_map(&mut commands, &mut assets, "sp_a1_intro3", Vec3::ZERO)?;
}
```

`MapInfo`: `root` (entity with `SourceMapRoot` + `RigidBody::Static`),
`player_start` (the literal `info_player_start`, usually a hidden room),
`arrival` (the `@arrival_teleport` inside the arrival elevator, the real start),
`entities: Vec<MapEntity>` (kind, classname, targetname, world `Transform`,
model / brush model, skin, spawned render `Entity`, bounds, keyvalues),
`light_environment`, `fog`, `skybox`, `bounds`, `convert_seconds`.
`MapEntityKind`: `PlayerStart, StaticProp, WeightedCube, DynamicProp,
PhysicsProp, Door, Button, BrushEntity, LightEnvironment, OtherProp, Other`.

Components other crates query:

| component | on | meaning |
|---|---|---|
| `shared::Portalable` | brush collider | every face this collider covers accepts portals |
| `SourceCollider` | every brush collider | `contents`, `brush`, `model` (0 = world), `sides` (world normal, material, surfaceprop, portalable); `side_for_normal(hit_normal)` |
| `PortalShotPassthrough` | grate / player-clip colliders | the portal shot passes through (player still collides) |
| `SourceEntityRef` | spawned props / brush entities | index into `MapInfo::entities` |

Other entry points: `load_map` (pure CPU, can run on a worker thread) then
`spawn_loaded_map`; `SourceRenderSettings { light_scale, fullbright }`
(default `DEFAULT_LIGHT_SCALE = 3.0`, see gaps); `SourceCache` keeps converted
maps and uploaded images, so spawning a chamber twice is instant.

## Examples

```sh
cargo run -p source_assets --example view -- sp_a1_intro3
# screenshot mode: N frames then save and exit (use >= 60 frames so pipelines are compiled)
... --example view -- sp_a2_laser_intro --screenshot out.png --frames 90 --setpos "-560 0 -128 0 0"
```

`view`: fly camera (WASD, mouse after right-click/Tab, Space/Ctrl, Shift),
starts at `arrival`. `--setpos "x y z pitch yaw"` uses the game's
`setpos`/`setang` convention (feet, eye +64 u, Source FOV 90 at 4:3 =
73.74 degrees vertical) so shots line up with in-game captures;
`--cam x,y,z,yaw,pitch` is world metres. `--collision` draws colliders
(blue portalable, red not). `SOURCE_FULLBRIGHT=1`, `SOURCE_LIGHT_ONLY=1`,
`SOURCE_NOCULL=1` are debug views.

`info`: conversion stats, timings, missing materials, `--near x,y,z r`
entity listing. `shaders`: dump a `.vcs` (combo counts, per-static-combo
constants/samplers).

`tools/p2_capture.py`: reference captures from the real game (see gaps).

Verified by screenshot: sp_a1_intro3, sp_a2_laser_intro, sp_a2_bridge_intro
(and sp_a1_intro1 loads without errors).

## Known gaps

- **Verdict provisional**: no real-game reference captures yet (the game
  cannot be launched headlessly here), so combo choices and constant values
  are unverified against Portal 2 itself.
- Still on the fallback: Refract (glass windows), Water/slime,
  UnlitTwoTexture (monitors), sprites/particles, the 2D sky shader, `$phong`
  / bump on models (the `_bump_` model shaders), `env_cubemap` on props
  (needs the nearest cubemap), dynamic local lights on props.
- No auto-exposure, bloom (`engine_post`) or `color_correction` LUT yet.
- **Exposure is a constant.** Portal 2 auto-exposes (maps set roughly
  0.5..3 via `env_tonemap_controller`) and adds bloom and a color-correction
  LUT; here `light_scale` (default 3) stands in. `.vhv` vertex light is
  decoded as gamma-space bytes x2 and ambient cubes x255, both calibrated by
  eye against neighbouring lightmaps, not against the game.
- **Reference captures are blocked.** `tools/p2_capture.py` launches the game
  windowed with `setpos`/`setang`/`screenshot`, but on this machine the Steam
  client runs in its own container: a direct launch hangs at SteamAPI init
  ("connect to global user failed") and `steam -applaunch` did not start the
  game. It needs Steam to launch the game (launch options / confirmation).
- Lightmap styles other than 0 (switchable lights), animated textures,
  texture scrolling, and other proxies are ignored.
- Brush entities are static at their spawn pose; props show one frame of
  their default sequence (no playback, no .ani animation blocks, no
  procedural bones or pose-parameter blends).
- Displacement lightmap UVs interpolate the base face's corners (close, not
  exact). Refract/water surfaces are a tinted translucent pane.
- The skybox cubemap face orientation is unverified (Portal 2 chambers fog
  their `sky_white`).
- Portal 1 fallback is detected but untested.

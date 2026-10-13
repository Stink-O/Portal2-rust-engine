# p2_fx: portal and portal-gun effects for Bevy

Particle effects and sounds for portals and the portal gun. The particle
systems are Portal 2's own `.pcf` files, read at runtime from your Portal 2
install, so Portal 2 is required: add `p2_assets::AssetsPlugin`, which exits
the app when the game is missing. There are no stand-ins.

No game files are in this repository. Nothing here is decompiled Valve code.

## Using it in your app

```rust
use bevy::prelude::*;

App::new()
    .add_plugins((
        DefaultPlugins,
        p2_assets::AssetsPlugin, // mounts the Portal 2 install (optional)
        p2_core::CorePlugin,     // portal messages and components
        p2_fx::Plugin,           // this crate
    ))
    .insert_resource(p2_fx::FxSettings { sounds: false, ..default() }) // optional
    .run();
```

The plugin adds no cameras, no Bevy plugins and no physics. Your app must have
`DefaultPlugins` (audio and rendering) and `p2_core::CorePlugin`.

**Camera requirement.** Rim effects use render layer `p2_fx::LAYER_PORTAL_MAIN_ONLY`
(25). Your main camera must include layer 25. Portal view cameras must not, or
rims show up twice inside portals. The first-person top light and muzzle flash
use `p2_core::LAYER_VIEWMODEL` (26), so your viewmodel camera must draw that
layer.

**Inputs.** The plugin reacts to these `p2_core` messages: `PortalPlaced`,
`PortalClosed`, `PortalShotFailed`, `GunFired`. It reads the `Portal` transform
and the `PortalGun` and `ViewmodelAttachments` components. Send the messages from
your own gun or script; you do not need `p2_gun`.

## Config: `FxSettings`

| field | default | meaning |
|---|---|---|
| `particles` | `true` | spawn all particle effects |
| `sounds` | `true` | play all sounds |
| `master_volume` | `1.0` | multiplies every sound volume |
| `muzzle_flash` | `true` | first-person muzzle flash (needs `ViewmodelAttachments::muzzle`) |
| `top_light` | `true` | first-person top light (needs `ViewmodelAttachments::top_light`) |
| `projectile_delay` | `0.1` | seconds after a shot before the projectile shows (`cl_portal_projectile_delay_sp`) |
| `projectile_flight` | `0.1` | muzzle-to-hit time of the visual projectile, s (Portal 2: always 0.1 s in single player, from the decompiled client) |

## Public API

- `Plugin`: the entry point.
- `FxSettings`: the config resource above.
- `ParticleEffect`: a running effect. `ParticleEffect::new("portal_close").with_cp(0, pos, basis).with_cp_vector(2, colour)`. Control points are world space; CP2 is the colour (0-255) for portal bursts, CP7 for rims, CP1 for gun effects. You can spawn any system name from the game's `.pcf` files.
- `ParticleStats`: particles emitted and alive per system, for debugging.
- `PlaySound`: a message for any sound by soundscript name, e.g. `PlaySound::at("Portal.open_blue", pos)`.
- `SheetGrids`: sprite-sheet layouts by material name (see gaps).
- `ParticleLibrary`, `SystemDef`: the parsed `.pcf` data.
- `edge_system(kind)`: the rim system name for a colour.
- `gain(distance_m, soundlevel_db)`: the distance falloff used for sounds.
- `LAYER_PORTAL_MAIN_ONLY`: see the camera requirement.
- `StepSurface("metal")`: put on a collider (or its parent) to pick the
  footstep sound of a `surfaceproperties` name. Untagged ground uses the
  surfaceprop of Portal 2's white chamber floor (`default`). The ground
  collider is read from `Grounded::entity`.

## What it does (spec sections from `research/specs/`)

| effect | spec | status |
|---|---|---|
| rim `portal_edge` (blue) / `portal_edge_reverse` (orange) while a portal exists, follows its transform, restarts on re-placement | portal-rendering §3 | works |
| `portal_success` at a new placement; open sound `Portal.open_*` | portal-rendering §3, sound-portals §2 | works |
| `portal_close` at the old spot; close sound | portal-rendering §3 | works |
| `portal_badsurface` at the hit point, `Portal.fizzle_invalid_surface` (does not fit) or `Portal.fizzle_moved` (overlap, not portalable) | portal-placement §5 | works; see mapping note |
| projectile stream from the muzzle to the hit point, starting `projectile_delay` after the shot, lasting at least 0.1 s | portal-placement §2 | works (speed estimated) |
| fire sound `Weapon_Portalgun.fire_blue/red` on every shot | sound-portals §2 | works |
| first-person muzzle flash and top light on the viewmodel layer | portal-rendering §4 | works; world-model flash not drawn |
| looping ambient hum `Portal.ambient_loop` per open portal, fades in over 0.5 s | sound-portals §2 | works; not panned |
| sounds: volumes and pitch ranges from the soundscripts | sound-portals §2 | works |
| footsteps (cadence, volume, left/right, surface), jump push-off, landing step + `PortalPlayer.FallRecover`, wall/ceiling slams `JumpLand.HighVelocityImpact[Ceiling]`, falling wind `PortalPlayer.Woosh` | player-sounds §4-§8 | works |

**Mapping note.** Spec §5 says overlap and invalid-surface shots play a *close*
effect at the attempted spot with `fizzle_moved`. The original design asked for
`portal_badsurface` on every failed shot, and that is what this crate does. The
two disagree; this is an open question.

**Missing files.** A particle system, particle material or sound the game does
not provide is logged once and not drawn or played.

## The particle simulator

A CPU port of the operators the effects use, in Source units, following
`particles-runtime.md` and `particles-params.md`:

- emitters: instantaneous (with per-step cap), continuous.
- initializers: position in sphere / along ring / warp / offset, lifetime, radius,
  alpha, colour, rotation, sequence, velocity, noise, remaps.
- operators: movement (Verlet, drag, gravity), lock to control point,
  lifespan, fades (simple, random, fade-and-decay), colour fade, radius scale,
  clamp, ramp, oscillate, direction-to-CP, control-point setters, rotation, cull.
- forces and constraints: pull, random force, twist, distance constraint.
- renderers: sprites, ropes (Catmull-Rom, SplineCard orientation), sprite trails.
- children with delays, end-cap behaviour, operator fade windows.

Unsupported operators are listed in `SystemInstance::unsupported` and skipped.

## Running the example

The example runs the portal effects on a 60 Hz virtual clock in the test
chamber, and saves six screenshots.

```text
cargo build -p p2_fx --example fx
D=out; mkdir -p $D
target/debug/examples/fx --size 960x540 \
  --shot 50:$D/blue_1m.png --shot 110:$D/blue_3m.png --shot 170:$D/orange_1m.png \
  --shot 200:$D/badsurface.png --shot 251:$D/projectile.png --shot 312:$D/close.png
```

It logs particle counts per system at each shot. Unit tests: `cargo test -p p2_fx --lib` (5 tests).

## Known gaps (visible in the screenshots)

- **The rim is faint and thin** next to Portal 2's. The colour pipeline was
  fixed (the old code multiplied sRGB values as linear light, and ignored
  `alpha2`), but the screenshot still shows a thin rim. Bevy's tonemapping on the
  main camera is probably part of it; that is the camera owner's call.
- **The projectile stream is hard to see** in the mid-flight shot. Its
  children run (9 fibres, 5 balls alive), but they are small and dim.
- **Sprite sheets** (`sequence_number`, `animation rate`) work when a grid is
  given in `SheetGrids`. The game's sheet layouts are not decoded yet, so the
  sparks show their whole texture. Their `$distancealpha` threshold is a
  guess (`smoothstep` 0.42 to 0.58).
- **Refraction particles** (`Refract` materials, e.g. the portal-close warp):
  drawn by bending the view's opaque-pass copy; `$bluramount` is ignored.
  `Velocity Repulse from World` only does its offset mode, along CP0's normal.
- **Not implemented:** `Collision via traces` (sparks do not bounce off walls), `Restart Effect after Duration`, `Lerp Initial
  Scalar`, soft particles (`$depthblend`), operator time offset and scale.
- **Projectile speed** (120 m/s) is an estimate. The spec only gives the
  timing: a stream from 0.1 s, lasting at least 0.1 s.
- **Gun model muzzle flash** on the world model: no world-model attachment
  data reaches this crate.
- **No panning.** Sounds use distance gain only. Spatial audio needs a
  `SpatialListener` on the camera, which the host would add.
- **Sound falloff** is an approximation of Source's soundlevel.
- **WAV decoding.** Bevy's default audio features decode only Ogg/Vorbis, so
  this crate decodes 16-bit PCM WAV itself (`PcmClip`). Other formats are not
  played.
- **No-hit shots** (sky) play no sound and no burst. Spec behaviour unclear.

## Open questions

1. Should the main camera's render layers include 25 (rims)? `p2_view` must add it.
2. Spec §5 vs the original design on which failed shots get `portal_badsurface` (see the mapping note).
3. Bevy audio features: WAV/MP3 are not enabled by default. Happy with the
   crate's own PCM decoder, or do you want a feature change?
4. Projectile speed: is 120 m/s right, or is there a Portal 2 value to use?
5. Tonemapping on the main camera: does it wash out the additive rims?

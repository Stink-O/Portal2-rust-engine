# p2_body

Chell's third-person body, and the ghost copies that keep it whole while it
is half-way through a portal. Spec: `research/specs/player-in-portals.md`.

## What works

| Spec | Status |
|---|---|
| §2 Body on `LAYER_PLAYER_BODY` (27), ghosts on `player_ghost_layer(entry kind)` (24 blue, 23 orange) | Done. `p2_render` picks per camera which of them it draws (Portal 2's rules, `player_layers` there). |
| §3 Ghosts: a mirror of the body through `portal_pair(entry, exit)`, while the bounds overlap an opening | Done. `sync_ghosts` builds the mirror, places it through the pair, and keeps local poses in sync every frame. Verified: the wall and floor cases produce exactly one ghost at the expected exit position. |
| §3 Clip 2 u behind the entry plane (original) and 2 u behind the exit plane (ghost), in the shader | Done in `clip.wgsl` (world-space planes, up to four, `discard`). The halves overlap by 4 u, so no seam; checked in close-up screenshots. |
| §3 No ghost once the bounds centre is more than 5 u behind the entry plane | Done (`overlaps_opening`). |
| §3 Generic `Ghostable` for any entity | Done. Any top-level unit-scale root with `Ghostable` and `GhostBounds` is ghosted. `StandardMaterial` meshes under it are converted to `GhostMaterial` automatically. |
| §1 Chell's model, animated like Portal 2's player | Done with the Portal 2 install. Animations come from `models/player_animations.mdl` and its `.ani` file (frame-major data, see `research/specs/player-animation.md`). Base sequence by activity (idle, run, crouch idle/walk, jump, float, airwalk), the 9-way run/crouch-walk blend from velocity (`move_x`/`move_y`), the aim matrix auto-layer from the view (`body_yaw`/`body_pitch`), land and fire gestures. Idle, run, crouch and pitch up/down seen in screenshots. |
| §1 World gun in her hands | Done: bone-merged on `weapon_bone`; she holds it in both hands (right on the grip, left under the front), and it follows the aim matrix. |

## Public API

```rust
use p2_body::{BodyConfig, BodyModel, Ghostable, GhostBounds, ghost_material, GhostMaterial};

app.insert_resource(BodyConfig { model: BodyModel::Chell, ..default() })
   .add_plugins(p2_body::Plugin);   // after p2_core::CorePlugin
```

- `Plugin`: spawns a body for every `Player` (`Added<Player>`), drives it, and
  runs the ghost sync in `P2Systems::Body`.
- `BodyConfig` (resource): `enabled`, `model`, `chell_model`,
  `chell_animations`, `gun_model`.
- `BodyModel`: `Chell` (default; needs Portal 2 through
  `p2_assets::AssetsPlugin`, nothing from the game is shipped) or `Scene(Handle<WorldAsset>)` for your own glTF/world asset (spawned under the
  body root with its origin at the feet, front along -Z; animate it yourself).
- `PlayerBody { player }`: the body root, a top-level `Ghostable`.
- `Ghostable` (marker, requires `GhostBounds`): duplicate this entity through
  linked portals. Root must be top-level with unit scale; its meshes can use
  `StandardMaterial` (converted) or `GhostMaterial`.
- `GhostBounds { min, max }`: local AABB of the entity, at its origin.
- `GhostMaterial` / `ghost_material(StandardMaterial)`: the clipped material.
- `front_plane`, `clip_plane`, `ghost_transform`, `overlaps_opening`,
  `ClipPlanes`, `CLIP_OVERLAP`, `GHOST_CENTRE_BEHIND`, `GHOST_SLAB`,
  `MAX_CLIP_PLANES`, `DISABLED_PLANE`: the pure pieces, usable from your own code.

Mirrors of the local player's body go on `p2_core::player_ghost_layer` of the
portal whose opening the body overlaps (24 blue, 23 orange); mirrors of other
`Ghostable`s copy their root's layers. Which camera draws the body and which
ghost is `p2_render`'s job, from `p2_core::LocalEye`.

## Run it

```text
cargo run -p p2_body --example body -- --case idle  --screenshot idle.png
cargo run -p p2_body --example body -- --case run   --exit-after 60
cargo run -p p2_body --example body -- --case wall  --cam 8 1.4 3.5 --target 12 1.2 0 --screenshot wall_exit.png
cargo run -p p2_body --example body -- --case wall  --cam -7 1.4 2.5 --target -12 1.2 0 --screenshot wall_entry.png
cargo run -p p2_body --example body -- --case floor --cam 4 2.5 0 --target 4 0.8 -7 --screenshot floor.png
```

Extra example flags: `--pitch DEG` (view pitch, + up), `--crouch`,
`--feet X Y Z` / `--yaw DEG` (free placement, no scripted asserts),
`--walk X Y Z` (move every frame, 60 Hz steps), `--portal-view` (real portal
views), `--first-person` (the camera rides the eye, through a portal like
`p2_view`, and sets `LocalEye`), `--side` / `--corner` (wall pair side by side
on the west wall, or on the west and north walls).

The example needs Portal 2 (it exits with an error without it). Each case asserts its
ghost count and position at frame 30 and panics on failure. Harness flags
(`--frames`, `--shot F:path`, `--exit-after`, `--size`) are shared with the
other p2 examples. The example uses the debug camera on layers 0 and 27 and
has no portal rendering.

Tests: `cargo test -p p2_body` (5 unit tests: plane sign, clip overlap, pair
mapping, opening overlap, source-to-body axes).

## Known gaps

- Chell's skin, pose and gun are checked in screenshots only. Jump, float
  and the gestures are mapped but not screenshot.
- Not done: the IK rules of the animations (foot ground contact and the left
  hand locked to the right wrist). The left hand already sits under the gun
  from the animation data alone. No head look (`head_yaw`/`head_pitch`) and
  no turn-in-place animation (the legs snap round past 45 degrees).
- Ghosts exist only as layer-27 mirrors; the joining is only visible through a
  portal renderer, which is not in this crate. The screenshots show the
  positions, not the seam through a real portal view.
- The clip is in the forward pass only. If a camera uses a depth prepass,
  the prepass is not clipped.
- Model orientation: animated bones are in Source model space (x forward,
  y left, z up); `model_to_body` maps that to -Z forward, Y up. (The
  reference pose of these files is stored Y-up, which is why the old code
  turned the model differently; every animation puts the root back Z-up.)
- Bounds are a plain AABB with a 0.1 m slab (`GHOST_SLAB`), not the exact
  hull. Clip planes are infinite planes, not limited to the opening.
- Floor/ceiling exits: Portal 2 clips the ghost 2 u *in front of* the exit
  plane in the view through the ghost's own portal; one material per mesh
  cannot vary per camera, so this is not done.
- Body meshes are `NotShadowCaster`, so the body casts no shadow.
- Scene-variant meshes get layer 27 via a system; untested with a real glTF.
- Mirrors are rebuilt when the descendant set changes; there is no pooling.
- Shadows and the gun's own animation are not done.

## Open questions

1. Should portal views draw layer 27 (body and ghosts) in every portal camera,
   as the spec says? This crate assumes yes.
2. Should the body cast shadows in portal views? It does not now.
3. Is a 0.1 m slab and an AABB good enough, or should the bounds be the exact
   collision hull?

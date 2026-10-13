# p2_room

The example world for the portal library. It is not part of the library:
modders copy it as the reference for "how to set up a world for portals".
It is a generated greybox chamber (no Portal 2 level), built in code.

## What works

- **Chamber geometry** (`src/chamber.rs`): a 1024 x 1024 u room, 448 u
  ceiling, 32 u outer walls. Pit (512 u deep, portalable floor, black walls),
  raised white ledge (128 u), 45 degree portalable panel, portalable ceiling
  patch, 48 x 96 u white insert too small for a portal, white wall next to a
  black edge, black north wall, 128 u corridor, two small coloured markers.
- **Portal 2 materials**: read from the player's install with
  `source_assets` (vmt + vtf). Chamber tiles are 1024 px at 4 texels per
  unit (texture scale 0.25, verified against `sp_a2_laser_intro` and
  `sp_a2_pit_flings` texinfo), so a tile spans 256 u. Nothing from the game
  is shipped. If the game is not found (`PORTAL2_DIR` or the default path) the
  surfaces are untextured and an error is logged; apps that add
  `p2_assets::AssetsPlugin` exit instead.
- **Lighting**: four neutral point lights under the ceiling. An ambient light
  is added to each camera as it appears (the chamber's ambient is a component
  on the camera, so it must follow the camera).
- **Harness** (`src/harness.rs`): screenshots, frame counts and window size
  for every example.

## Public API

| item | what it is |
|---|---|
| `RoomPlugin` | Spawns the chamber at `Startup`, inserts `RoomSpots`, adds ambient to cameras. |
| `RoomSpots` | Named places, see below. Resource. |
| `RoomSpot { feet, yaw, pitch }` | A place: feet (metres, y = 0 on the floor), yaw (0 looks down -Z, positive turns left), pitch (positive looks up). |
| `spawn_box(commands, meshes, mat, min, max, portalable, name)` | Static avian cuboid, metres. `portalable` adds `p2_core::Portalable`. |
| `spawn_slab(commands, meshes, mat, centre, rotation, half, portalable, name)` | Same, rotated. Used for the 45 degree panel. |
| `TEXELS_PER_UNIT` | 4.0, the chamber texture density. |
| `HarnessPlugin`, `HarnessArgs` | Harness (below). |

### Tagging a surface as portalable

A portal can be placed on any collider that carries `p2_core::Portalable`.
Give a collider the marker and leave it static:

```rust
commands.spawn((
    RigidBody::Static,
    Collider::cuboid(w, h, d),
    Mesh3d(mesh),
    MeshMaterial3d(mat),
    Transform::from_translation(centre),
    Portalable,            // this surface accepts portals
));
```

Anything without `Portalable` (black metal, the pit walls) is a non-portal
surface. `spawn_box` does the same thing for an axis-aligned box.

### RoomSpots fields

All coordinates are Source units converted with `u()`. Feet are on the floor
(y = 0) unless stated.

| field | where | feet (u) | facing |
|---|---|---|---|
| `spawn` / `spawn_yaw` | start | (400, 0, 400) | -Z |
| `recursion` | between the big white west and east walls (1024 u apart) | (-256, 0, -64) | +X |
| `pit_rim` | rim of the 512 u pit with a portalable floor (on the black pit-wall lip) | (-304, 0, 168) | +Z, 60 down |
| `ledge` | base of the 128 u white ledge (x 224..448, z -448..-256) | (120, 0, -352) | +X |
| `angled_panel` | 45 degree portalable panel rising north from (0,0,-79) to (0,226,-305) | (0, 0, 40) | -Z, 20 up |
| `ceiling` | portalable ceiling patch (x -192..192, z -64..192, y 448) | (0, 0, 320) | -Z, 60 up |
| `small_panel` | 48 x 96 u white insert in the south wall (x -96..-48, y 112..208); too small for a 64 x 112 u portal | (-72, 0, 360) | +Z, 10 up |
| `edge_bump` | white east wall next to the black strip (z 0..128) | (360, 0, -40) | +X |
| `black_wall` | black non-portal north wall (z = -512) | (-200, 0, -300) | -Z |
| `corridor` | 128 u (1 block) corridor, x 128..256, z 256..544 | (192, 0, 440) | -Z |

## How to run

```sh
# interactive, from a spot (eye 64 u above the feet)
cargo run -p p2_room --example room -- --spot recursion

# screenshot run: 120 frames, then save and exit
cargo build -p p2_room --example room
./target/debug/examples/room --spot pit_rim --screenshot out.png --frames 120 --size 1280x720
```

`room` flags: `--spot NAME` (`spawn`, `recursion`, `pit_rim`, `ledge`,
`angled_panel`, `ceiling`, `small_panel`, `edge_bump`, `black_wall`,
`corridor`), `--pos X Y Z` (world metres), `--look YAW_DEG PITCH_DEG`.
`--pos` and `--look` override the spot.

Harness flags (any example that adds `HarnessPlugin`):
`--screenshot <png>` with `--frames N` (default 90), `--shot <frame>:<png>`
(repeatable), `--exit-after <frames>`, `--size WxH` (default 1280x720).
Unknown arguments go to `HarnessArgs.rest`. A scripted run sets
`p2_core::InputLocked`.

The example sets `p2_core::P2Settings { set_fixed_timestep: true }` before
`CorePlugin`, which owns the 60 Hz fixed timestep. Other worlds must do the
same if they want the portal simulation's tick.

## Known gaps

- Materials are base textures only: no bump, no lightmaps, no Portal 2
  shaders. The white panels read grimier than the game's clean lit panels.
- Lighting is a few point lights and a flat ambient, not baked. Brightness
  was tuned by eye, not against a game capture.
- Texture alignment is world-aligned per box, not per-brush as Hammer does.
- No doors, buttons, cubes or light panels. Those belong to other crates or
  to a later pass.
- Coloured markers are placeholders.
- Only the `room` example exists; other crates bring their own.
- The corridor's south end is a black wall, not a portal target.

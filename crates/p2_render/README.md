# p2_render

Portal views and the portal surface for a Bevy 0.19 app. This is the part that
makes a portal look like one: what you see through the opening (with
recursion), the glowing rim, and the cap that keeps the opening filled when
your eye is closer than the near plane.

It is a library. It does not spawn a window, a camera (beyond its own view
cameras), a light, a player or a physics engine. Your app provides those.

## Add it to your app

```rust
use bevy::prelude::*;
use p2_core::{CorePlugin, MainCamera, P2Settings};

App::new()
    .add_plugins(DefaultPlugins)
    .add_plugins(avian3d::prelude::PhysicsPlugins::default())
    // Your main camera must carry MainCamera. Its HDR, tonemapping, exposure,
    // fog and MSAA are read from it, so the view through a portal matches the room.
    .insert_resource(P2Settings { set_fixed_timestep: true }) // optional: Portal 2's 60 Hz
    .add_plugins(CorePlugin)
    .add_plugins(p2_assets::AssetsPlugin)   // optional: Portal 2's textures from the user's install
    .add_plugins(p2_render::Plugin::default())
    // .insert_resource(p2_render::PortalRenderConfig { recursion_depth: 2, ..default() })
    .run();
```

Spawn portals yourself (or let `p2_gun` do it), with `Portal { kind, half_size, surfaces, placed_at }`,
a `Transform` whose local +Z points out of the wall, and `PortalLink(other)` on
both halves once they are paired. `p2_core::PORTAL_HALF_SIZE` gives the Portal 2
opening size (64 x 112 units).

## What you get

| Feature | Spec | Notes |
|---|---|---|
| View through a linked portal, recursion depth 2 | portal-rendering.md §1.1 | One render target and camera per node of a portal-crossing tree. |
| Camera = pair transform of the main camera, oblique near plane on the exit | §1.1 | `p2_core::portal_pair`, `oblique_reverse_z`. |
| Zero frame lag | design | Cameras are set in `PostUpdate` from this frame's main camera pose. |
| Surface shader (PortalRefract port) | §1.2, §1.3 | Stage 1 hole, stage 2 burning rim, membrane for unlinked portals. |
| Open animation | §2.1 | Driven by `PortalAnimation.open_amount`. |
| Static fade | §2.2 | Driven by `PortalAnimation.static_amount`. |
| Unlinked and opening: depth-only pass | §1.2 (`_anims`) | Writes depth inside the hole, no colour. |
| Near-plane cap | (our fix) | A shallow box behind the opening takes over when the eye is within the near plane. See below. |

## Public API

- `Plugin { config: PortalRenderConfig }`. Add once, after `CorePlugin`.
- `PortalRenderConfig`, a resource. Fields:
  - `recursion_depth` (default 2, clamped 1..=3): how many nested views carry surfaces.
  - `target_scale` (1.0): view target size relative to the main target.
  - `quad_standoff` (0.004 m): how far the surface quad sits in front of the wall.
  - `cap_depth` (0.35 m): depth of the near-plane cap box.
  - `cap_margin` (0.02 m): how far past the near plane the cap starts to show.
- `PortalViewCamera { portal, level }` (from `p2_core`): the view cameras. Level
  `recursion_depth + 1` is the plain level.
- `ViewTree`: the active nodes this frame (for debugging). `active_views()` counts the cameras.
- `PortalSurfaceMaterial`, `PortalSurfaceParams`, `PortalSurfaceQuad`, `PortalSurfaceCap`:
  the surface components and material, for hosts that want to read them.
- `PortalProjection`: the oblique-near-plane projection the view cameras use.
- `node_layer(code)`: render layer of a node's surfaces (28 for the main view).

Render layers used: the main camera gets layer 28 added (or inserted if it has no
`RenderLayers`). Node `n` uses layer `28 + n - 1`. View cameras see layer 0
and their own surface layer. The local player's body (27) and ghosts (24 made
by blue, 23 by orange) are added to the main and view cameras by Portal 2's
rules in `player_layers`, from `p2_core::LocalEye`: main view body only while
the eye is through a portal ahead of it, ghosts only while it is not; no body
in the level-1 view through the portal the eye looks out of; no ghost in the
level-1 view through the portal it came from while the eye is not through.
Keep your own layers below 23.

## Textures

The noise and colour ramps come from Portal 2 through
`p2_assets::portal_textures::PortalGameAssets`, so `p2_assets::AssetsPlugin` is
required (it exits the app when Portal 2 is missing). Without it no portal surface
is drawn. Nothing from Portal 2 is bundled.

## Near-plane cap

When the main camera is closer than `near + cap_margin` in front of an opening,
the surface quad would be cut by the near plane, and the wall would show through
for a frame. So a second box is drawn behind the opening
with the depth test off. It samples the same view image at the same pixels, so
the seam is invisible. It is shown only while the eye is that close, so it does
not overdraw nearby props at other times.

## Run the example

```sh
cargo run -p p2_render --example render -- \
    --preset facing --look 90 0 --screenshot out.png --frames 240
cargo run -p p2_render --example render -- \
    --preset facing --near 0.005 90 --screenshot near.png --frames 150
cargo test -p p2_render --lib
```

`--preset facing|floor`, `--pos X Y Z`, `--look YAW PITCH` (degrees, yaw 90 looks
west), `--near DIST ANGLE` (camera in front of blue at that distance and angle to
the plane), `--age SECS` (placement time offset for the open animation),
`--unlinked`, `--depth N`.

Unit tests cover the node codes, the pair maths and the view tree on a facing pair.

## Verified so far

- Facing pair, looking through blue: the rim, the view inside it, and a nested inner
  ring (recursion) render correctly. Screenshot `facing_view.png`.
- Near-plane: camera 5 mm, 2 cm and 10 cm in front of blue, straight (90°) and at
  60° to the plane. The opening stays filled with the view; no wall-behind frame is
  visible in the captures.

## Known gaps

- **One portal pair.** The renderer tracks one blue and one orange portal
  (Portal 2 single player). Several pairs at once (co-op style) are not drawn
  correctly yet.
- **Depth doubler** (done, lead 2026-10-13). The level below `recursion_depth`
  draws its portals with last frame's image of that level, re-mapped from the
  outer opening's screen rectangle into the inner one (ping-pong targets). This
  is an affine approximation of the game's alternate-view-matrix re-projection,
  exact for facing parallel pairs and close for others. It gives the endless
  tunnel.
- **Stage 0 refraction** (done, lead 2026-10-13). The live surface reads
  Bevy's copy of the opaque pass ("view transmission texture"), like
  PortalRefract reads the frame buffer, and composites the rim over it. It is
  therefore drawn opaque. Transparent things behind a rim (particles) are not
  in that copy.
- **Writez pass.** An unlinked, opening portal always draws its visible
  surface; the separate depth-only pass is not used (a lone portal must be
  visible while it opens).
- **Cost.** Each active node is a full extra scene render with shadows. A
  facing pair at depth 2 usually activates 3 cameras plus the doubler level.
- **Viewmodel and particles.** Not drawn through portals here. That is
  `p2_view` and `p2_fx`.

## Open questions

1. Should the main camera near plane drop to about 0.01 m? That would shrink the
   visible seam at the wall, and it is owned by `p2_view`.
2. Does Portal 2's `_anims` material draw colour as well as depth? Only the decompiled
   shader can say; `research/` would need to check it.
3. Is the stage-0 ring acceptable as an approximation, or do we need a frame-buffer copy
   (a render-graph node)?

# p2_view

First-person camera, portal gun viewmodel and crosshair. A library plugin: it
does not spawn a window, a player, a level or physics. Drop it into a Bevy
0.19 app that already runs `p2_core::CorePlugin`.

## Add it to your app

```rust
use bevy::prelude::*;

App::new()
    .add_plugins(DefaultPlugins)
    .insert_resource(p2_core::P2Settings { set_fixed_timestep: true }) // optional, see p2_core
    .add_plugins(p2_core::CorePlugin)
    .add_plugins(p2_assets::AssetsPlugin) // required: Portal 2's gun model and crosshair
    .add_plugins(p2_view::Plugin)
    // ... your player (p2_core::spawn_player) and your controller
    .run();
```

`p2_view` needs a `Player` entity (`p2_core::spawn_player`) and a
`ViewAngles`/`Crouch`/`InterpolatedFeet` on it, which your controller keeps up
to date. The gun and crosshair are Portal 2's own, read from the player's
install; nothing from the game is shipped. `p2_assets::AssetsPlugin` exits the
app with an error when Portal 2 is missing.

## Options: `p2_view::ViewConfig`

Insert it before adding the plugin. Defaults are Portal 2's values.

| field | default | what it does |
|---|---|---|
| `spawn_main_camera` | `true` | Spawn the `MainCamera`. Set `false` to bring your own camera (below). |
| `fov_desired_deg` | `75.0` | Source `fov_desired`, horizontal at 4:3; wider screens widen it (hor+). Only used when the camera is spawned here. |
| `near_plane` | `0.05` m | Near plane of the spawned camera. Not from the spec. |
| `viewmodel` | `true` | Spawn the viewmodel camera and the gun. Set `false` if you draw your own gun. |
| `viewmodel_fov_deg` | `50.0` | `cl_viewmodelfov`, same convention as the world FOV. |
| `viewmodel_model` | `models/weapons/v_portalgun.mdl` | Game-relative path read through `p2_assets::GameFsRes`. |
| `crosshair` | `true` | Draw the two-bracket HUD crosshair with the last-placed pip. |
| `crosshair_scale` | `1.0` | Multiplier on the crosshair size. `1.0` = the game's size (below). |

## Use your own camera

Set `spawn_main_camera: false` and tag your camera with `p2_core::MainCamera`.
Each frame, in `PostUpdate` (before transform propagation), the plugin sets its
transform to the eye (interpolated feet + crouch eye height, with the view
angles and roll). The viewmodel camera gets the same transform. Your camera
should be a `Camera3d` with `Hdr` (the viewmodel camera is HDR, so the two must
match) and keep its own projection. The crosshair draws through the viewmodel
camera when it exists, else through the `MainCamera`.

## Your own gun

Set `viewmodel: false`, then read `p2_core::ViewmodelAttachments` on the player
for the muzzle and top-light world poses (written every frame). Fire effects in
`p2_fx` read these. Your gun must sit on `p2_core::LAYER_VIEWMODEL` if you want
the viewmodel camera to draw it.

To play your own gun's animations, use `p2_view::ViewmodelRig` only when the
built-in gun is on; otherwise drive your own. Listen to `p2_core::GunFired`
and `p2_core::PortalShotFailed`.

## What works, against the specs

Sections refer to `research/specs/`.

- **Main camera (view-feel §1).** One `MainCamera`, Hdr, perspective. FOV 75 is
  horizontal at 4:3 and converted to Bevy's vertical FOV (`source_fov_to_vertical`),
  so wider screens get hor+ widening. Covered by a unit test.
- **Render interpolation (view-feel §5, contract).** Position is
  `InterpolatedFeet.at(Time<Fixed>::overstep_fraction())` plus `Crouch.eye_height`.
  The crouch transition itself (0.4 s down, 0.2 s up) is done by the movement
  crate; this plugin reads `eye_height`.
- **No bob, no landing punch (view-feel §2-4).** None are applied.
- **Roll recovery (portal-transit §1.7).** `ViewAngles.roll` eases to 0 with
  `p2_core::math::relevel_roll`: exponential, half-life 0.125 s, at least 10 deg/s,
  snapped under 0.1 deg. Verified numerically (below). Only `p2_view` decays roll.
- **Viewmodel (portal-rendering §4).** `models/weapons/v_portalgun.mdl`, GPU-skinned,
  animated with `p2_assets::anim::Animator`. Draw → idle on spawn; `fire1` on
  `GunFired` (from this player); `fizzle` on `PortalShotFailed` (falls back to
  `dryfire`). Its own camera at `cl_viewmodelfov` 50, `LAYER_VIEWMODEL` only,
  higher order, clears depth but not colour.
- **Viewmodel lag (view-feel §2).** Source `CalcViewModelLag` as written in the spec:
  the gun trails fast turns by up to about 7.5 u.
- **Attachments.** `ViewmodelAttachments.muzzle` and `.top_light` (`Body_light`) are
  written on the player every frame, in world space.
- **Crosshair (portal-rendering §5, particles-runtime item 5).** Left bracket blue,
  right orange, filled when that portal is placed and outlined otherwise; a
  last-placed pip in the middle. Colours brightened as `c + (1 - c/255) * 64`, alpha 150/255.
  Laid out at the 640x480 HUD scale and scaled to window height (`hudlayout.res`
  gives the proportional frame 640x480; scale = height / 480). Measured: 92 px
  tall at 1280x720, 140 px at 1920x1080 (the 64-texel bracket at 1.5x and 2.25x,
  less the anti-aliased edge). Set `crosshair_scale` to resize.

## Run the example

```
cargo build -p p2_view --example view
target/debug/examples/view [--look YAW PITCH] [--fire blue|orange] [--fire-frame N] [--roll DEG]
                                   [--screenshot out.png --frames N] [--shot F:path] [--exit-after N] [--size WxH]
```

- `--look YAW PITCH`: view angles in degrees.
- `--fire blue|orange --fire-frame N`: writes a `GunFired` at frame N (default 30).
- `--roll DEG`: starts rolled by DEG and prints the roll against the half-life curve,
  then `roll half-life check: PASS|FAIL`.

Unit tests: `cargo test -p p2_view --lib`
(6 tests: FOV conversion, eye height, HUD colours, viewmodel axes, barrel
direction, gun size).

## Known gaps

- **Firing view punch not implemented** (view-feel §4 / portal-rendering §4: pitch
  -1..-0.5, yaw -1..1). It is a camera-only spring.
- **`cl_mouselook_roll_compensation`** (view-feel §5) is not implemented; it belongs to input.
- **Viewmodel through portals:** no clipping of the gun when it is near a portal
  (`portal_viewmodel_offset` / radius).
- **Skin:** single-player skin only. No coop skins.
- **Lighting:** the viewmodel uses a fixed neutral ambient cube
  (`ViewModelLighting`), not the room's baked light.
- **Fizzle mapping is my reading.** The spec names `ACT_VM_FIZZLE` and `ACT_VM_DRYFIRE`
  but not which failure uses which; all failures use `fizzle`.
- **Intermittent crash at shutdown.** One run of the example segfaulted after its
  screenshot was saved; a rerun exited cleanly. Not investigated.
- **Near plane** (0.05 m) is not from the spec.

## Open questions

1. Who fixes the crosshair texture load in `p2_assets`?
2. Should firing view punch live in `p2_view` (camera only) or in `p2_movement`?
3. Confirm `fizzle` for every failed shot, or should `DoesNotFit` / `Overlap` use `dryfire`?

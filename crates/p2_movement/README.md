# p2_movement

Portal 2's first-person player controller as a Bevy 0.19 plugin, on avian3d
shape casts. GPL-3.0-only. It contains no Portal 2 code or assets.

## Add it to your app

```rust
use bevy::prelude::*;
use p2_core::{CorePlugin, spawn_player};
use p2_movement::{InputPlugin, MovementConfig, Plugin as MovementPlugin};

App::new()
    .add_plugins((DefaultPlugins, avian3d::prelude::PhysicsPlugins::default()))
    .add_plugins((CorePlugin, MovementPlugin))
    .add_plugins(InputPlugin)                // optional: keyboard and mouse
    .insert_resource(MovementConfig { mouse_sensitivity: 2.5, ..default() }) // optional tuning
    .add_systems(Startup, |mut commands: Commands| {
        // Feet position and yaw (0 looks down -Z).
        spawn_player(&mut commands, Vec3::new(0.0, 0.0, 2.0), 0.0);
    })
    .run();
```

* `MovementPlugin` (`p2_movement::Plugin`) is the core. It needs `p2_core::CorePlugin`
  and avian's `PhysicsPlugins`. It adds `MovementConfig` with Portal 2's defaults if you
  have not inserted one. It spawns no camera, window, room or player.
* `InputPlugin` is separate. Leave it out to drive the player yourself (below).
* Add `p2_core::P2Settings { set_fixed_timestep: true }` before `CorePlugin` to run at
  Portal 2's 60 Hz tick. Otherwise the controller runs at your fixed rate. Every step
  uses the fixed delta, so it is dt-correct at any rate, but the numbers are Portal 2's
  only at 60 Hz.

## Your own input

The core reads only `p2_core::MoveInput` and `p2_core::ViewAngles`.

| Field | Meaning |
|---|---|
| `MoveInput::wish` | `x` = right, `y` = forward, each -1..1 (full input = 175 u/s) |
| `MoveInput::jump` | Set true for one tick to jump. The move clears it. |
| `MoveInput::duck` | Hold to crouch |
| `ViewAngles::yaw` / `pitch` | Radians. Yaw 0 looks down -Z, positive turns left. Pitch is clamped to ±89°. |

Write these from any system that runs before `P2Systems::Move` (FixedUpdate). Write
`ViewAngles` in `Update`. Set `p2_core::InputLocked(true)` to stop `InputPlugin`
reading anything.

## What it does, with spec references

Source: `research/specs/portal-movement.md`, `view-feel.md` §5, `portal-funnel.md`.

| Feature | Spec | Where |
|---|---|---|
| Ground accel 10, friction 4, stop speed 100, max 175 u/s | §2, §3 | `rules::accelerate`, `rules::friction` |
| Edge friction ×2 near ledges (16 u ahead, 49 u down) | §3 | `controller::near_edge` |
| Air accel 5, air cap 60, cos(pitch) forward scale past 30° | §5 | `rules::air_accelerate`, `rules::air_forward_scale` |
| Air lock above 300 u/s horizontal, 150 u/s per axis | §5 | `rules::air_lock` |
| Jump √(90·g) = 232.4 u/s, 45 u apex | §4 | `MovementConfig::jump_speed` |
| No crouch-jump; a jump started crouched can't tuck | §4 | `controller::Duck::jumped` |
| Duck 0.4 s, unduck 0.2 s, smoothstep eye 64 → 28 u | view-feel §5 | `rules::EyeEase` |
| Air duck instant, feet rise 36 u (head planted) | §8 | `controller::duck_step` |
| Crouch at 1/3 ground speed | §2 | `controller::move_players` |
| Step up 18 u, slope limit 0.7 | §0, §3 | `controller::walk_move` |
| Split-step gravity 600 u/s², terminal 3500 u/s per axis | §7, §10 | `controller::move_players` |
| Forced crouch on a `ForceDuck` message, 1 s hold | portal-transit §1.6 | `controller::move_players` |
| Air funnelling into linked floor and ceiling portals | portal-funnel §1–§4 | `funnel::air_funnel` |
| Exit "gimme" straightening | portal-funnel §5 | `funnel::exit_gimme` (not yet called; see gaps) |

Collisions use avian3d `MoveAndSlide` shape casts, so fast flings don't tunnel.
Entities in `IgnoreColliders` are skipped in every cast and overlap.

## Config

`MovementConfig` holds every tunable. Defaults are Portal 2's values.
Speeds are u/s. Times are seconds. Angles are degrees.

| Field | Default | Meaning |
|---|---|---|
| `gravity` | 600 | u/s² |
| `accelerate` / `air_accelerate` | 10 / 5 | |
| `friction` / `stop_speed` / `edge_friction` | 4 / 100 / 2 | |
| `max_speed` / `air_speed_cap` | 175 / 60 | |
| `air_lock_speed` / `air_lock_axis` | 300 / 150 | |
| `jump_factor` | 90 | apex = factor/2 u |
| `step_height` / `min_walk_normal` | 18 / 0.7 | |
| `max_velocity` | 3500 | per axis |
| `duck_time` / `unduck_time` | 0.4 / 0.2 | A forced crouch (portal transit) isn't held: the player stands up as soon as there's room. |
| `mouse_m_yaw_deg` / `mouse_sensitivity` | 0.022 / 3.0 | degrees per count, times sensitivity |
| `pitch_limit_deg` | 89 | |
| `funnel_into_portals` | true | air funnelling on or off |

Other public items: `hull_collider(&Crouch)` returns the hull for your own casts.
Its centre is `feet + crouch.hull_centre_offset()`. `rules` and `funnel` are public,
pure functions with docs.

## Run the example

```sh
# play (click to grab, Esc releases)
cargo run -p p2_movement --example playground
# scripted checks, PASS/FAIL per check, non-zero exit on failure
cargo run -p p2_movement --example playground -- --script all
# one check, with a screenshot
cargo run -p p2_movement --example playground -- --script jump --screenshot out.png --frames 120
# unit tests
cargo test -p p2_movement --lib
```

Scripts: `ground stop jump crouch crouch_jump step fall force`.

## Measured (60 Hz, `--script all`)

| Check | Measured | Spec |
|---|---|---|
| ground: max speed | 175.00 u/s | 175 |
| stop: distance from 175 u/s | 29.17 u (24 ticks) | 31.25 u continuous; about 29 u discrete at 60 Hz |
| jump: apex | 45.16 u | 45 |
| crouch: eye at 0.2 s / 0.4 s / after unduck | 46.00 / 28.00 / 64.00 u | 46 / 28 / 64 |
| crouch_jump: ducked in the air | false, apex 45.16 u | no crouch-jump |
| step: onto 18 u, blocked by 40 u wall | 18.16 u, stopped at z -316 u | 18 u step, wall is too high |
| fall: 3 m landing speed | −365 u/s (sampled one tick late) | 376 u/s |
| force: ducked, eye eases, stands again | ducked tick 1, eye 28 at 0.4 s, stands by tick 72 | 1 s hold |

## Gaps

* Air funnelling and exit gimme are implemented and unit-tested, but no
  example drives them. Nothing in the playground places a portal.
* No footsteps, landing or jump sounds. Hosts hook `Grounded` and the velocity.
* No `use_key` behaviour. `MoveInput::use_key` is set but unused.
* The player has no collider of its own. Use `hull_collider` where another crate needs one.
* Speeds under 0.1 u/s are zeroed in friction, so the player stops fully.
  The spec returns unchanged there, so this is a deliberate deviation.
* The 1 s forced hold is read from transit §1.6 as a crouch timer that also blocks
  jumping. The spec doesn't say whether it also holds the crouch. Please check.
* Mouse sensitivity assumes Source's m_yaw 0.022 × sensitivity 3. Check it against
  a real Portal 2 config before you release.

## Open questions

1. The duck-jump timer: is it set only by a forced crouch (as implemented), or also
   by ducked jumps?
2. Should `ForceDuck` hold the crouch for the full 1 s, or only block jumping?
3. Should the player have a collider of its own? Currently it has none.

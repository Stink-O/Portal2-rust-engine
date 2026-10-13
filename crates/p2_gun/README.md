# p2_gun

Portal gun for a Bevy 0.19 + avian3d game: shot tracing, Portal 2 placement
rules (bumping, floor snap, overlap, corner checks), portal linking, and an
optional mouse-input plugin. Behaviour follows
`research/specs/portal-placement.md` (section numbers below refer to it).

## Add it to your app

```rust
use bevy::prelude::*;
use avian3d::prelude::*;

App::new()
    .add_plugins((
        DefaultPlugins,
        PhysicsPlugins::default(),
        p2_core::CorePlugin,      // messages, system sets, portal animation
        p2_gun::Plugin,           // shots, placement, linking
        p2_gun::MouseInputPlugin, // optional: left/right mouse fire
    ))
    .run();
```

1. Tag every collider a portal may sit on with `p2_core::Portalable`. The
   tag is read from the collider entity or its rigid body, so any shape works.
2. Give the shooter a `PortalGun`, `Crouch`, `ViewAngles` and a `Transform`
   (feet, yaw rotation). `p2_core::spawn_player` does this for you; you can
   also add the components to your own character.
3. To fire from your own input, skip `MouseInputPlugin` and write
   `p2_core::FirePortal { gun, kind }` yourself.

Nothing else is assumed. The crate spawns no window, camera, light, room or
player, and touches no cursor or time settings.

## What you get

| Behaviour | Spec | Where |
|---|---|---|
| Opening 64 x 112 u (half 32 x 56) | §1 | `GunConfig::half_size` |
| Left = blue, right = orange; 0.20 s lock after a shot; held re-fire every 0.50 s | §2 | `MouseInputPlugin`, `GunConfig::fire_lock`, `held_refire` |
| Shot from the eye along the view, range 56755.84 u | §2.1 | `GunConfig::max_range` |
| Shots pass through the shooter, `PortalHelperCollider`s, sensors and dynamic bodies | §2.1 | `fire` |
| Shots pass through linked portals, up to 16 segments | §2.1 | `rules::trace_shot` |
| Floor and ceiling take up from the shot heading; walls use world up | §2.2 | `rules::initial_frame` |
| Edge bumping: +2 u clearance, up to 5 rounds, total slide <= 91.2 u | §4 | `rules::fit_on_surface` |
| Corner checks at 2.2 u inset | §3 step 9 | `rules::corner_surfaces` |
| Floor snap for wall portals within 1.5 u of a floor | §3 step 6 | `rules::floor_snap` |
| Overlap with another portal facing the same way (dot >= 0.95), box test | §5 | `rules::overlaps` |
| Positions snapped to 1/512 u | §5 | `rules::place` |
| Replacing your own same-colour portal: old one despawned, `PortalClosed` sent | §5 | `fire` |
| Linking: both halves get `PortalLink` when both are open | §5 | `fire` |

Failures send `PortalShotFailed` with a `ShotFailReason`: `NoHit`,
`NotPortalable`, `DoesNotFit`, or `Overlap`. A failed shot leaves the gun's
existing portal alone. Every accepted shot sends `GunFired` first.

## Public API

- `p2_gun::Plugin`: core. Needs `p2_core::CorePlugin`. Inserts `GunConfig`
  unless you already did. Runs `Fire` (fire -> placement) in `Update`.
- `p2_gun::MouseInputPlugin`: optional. Needs `Plugin`. Writes `FirePortal`
  from the mouse, only while the primary window's cursor is grabbed and
  `InputLocked` is false.
- `p2_gun::GunConfig` (resource): `fire_lock`, `held_refire`, `max_range`,
  `half_size`. Defaults are Portal 2's values.
- `p2_gun::FIRE_LOCK`, `HELD_REFIRE`, `MAX_TRACE_LENGTH`: the same defaults as constants.
- `p2_gun::rules`: the pure maths (`World` trait, `place`, `trace_shot`,
  `fit_on_surface`, `floor_snap`, `overlaps`, `initial_frame`, ...). Usable
  without ECS; `World` is the adapter you implement for a different physics engine.
- Messages used: `FirePortal` (in), `GunFired`, `PortalPlaced`,
  `PortalClosed`, `PortalShotFailed` (out), all from `p2_core`.
- Components used: `PortalGun` (`portals`, `last_placed`, `can_fire`),
  `Portal`, `PortalLink`, `Portalable`, `PortalHelperCollider`.

## Running the example

```sh
# Scripted checks: fires at fixed spots in the test chamber and exits
# non-zero on any FAIL. Opens a short window.
cargo run -p p2_gun --example gun -- --script

# Unit tests (rules and timing).
cargo test -p p2_gun
```

Without `--script` the example shows the chamber with `MouseInputPlugin`
enabled. It only fires with the cursor grabbed, and the example does not
grab it (no movement crate here), so use `--script` to test.

The scripted checks cover: a plain wall, an overlap with the other portal,
a linked pair across a room, replacing your own portal, a bump off a wall
edge, a black (non-portalable) pillar, a floor, a ceiling, and a 45 degree
angled panel. Each check asserts the portal entity really exists and has
the expected centre, normal and up vector. Failed shots also confirm the
gun's portal is unchanged.

## Known gaps

- **Fizzle when the surface disappears** (spec §5, re-tests after 0.1 s): not
  implemented. A portal stays open until replaced.
- **Eye behind a portal** (spec §2.1, shooting out of the far side): not
  implemented. Shots start at the eye and only traverse linked portals in front.
- **No-portal volumes, `SURF_NOPORTAL` / glass / pass-through surfaces**
  (spec §3 steps 3, 4, 7): not implemented. Only `Portalable` is checked.
- **Placement helpers** (spec §6, `info_placement_helper`): not implemented.
- **Player standing on a floor portal spot** (spec §3 step 10): not implemented.
- **Shot origin** is the rendered eye: `InterpolatedFeet::at(overstep)` plus
  `Crouch::eye_height`, along `ViewAngles`. The shooter needs `InterpolatedFeet`
  (`spawn_player` adds it). Your own character must update it too.
- **Projectile visuals, sounds, crosshair, viewmodel** are not here. They
  belong to `p2_fx` and `p2_view`, driven by `GunFired` and `PortalPlaced`.
- Only `RigidBody::Dynamic` is skipped by shots. Kinematic bodies block them.

## Open questions

- Fizzle-when-surface-disappears (spec §5) is a known gap and belongs in this
  crate, to be added later.

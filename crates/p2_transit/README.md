# p2_transit

Portal transit for a Bevy 0.19 + avian3d 0.7 character: walking or falling
through a linked portal pair with Portal 2's rules (velocity, view, forced
crouch, the hole in the wall, and the geometry on the far side).

Spec: `research/specs/portal-transit.md` (§ numbers below refer to it).

## Add it to your app

```rust
app.insert_resource(p2_core::P2Settings { set_fixed_timestep: true }) // optional: Portal 2's 60 Hz
    .add_plugins((p2_core::CorePlugin, p2_transit::Plugin));
```

Tune before or after adding the plugin with the `TransitConfig` resource (defaults are Portal 2's values):

| field | default | meaning |
|---|---|---|
| `gravity` | 600 u/s² | gravity for the fix-up and the exit arc (§1.3) |
| `gravity_readd` | 1.008 | re-applied gravity factor (§1.3) |
| `hole_scale` | 0.98 | hole size as a fraction of the opening (§4) |
| `env_radius` | 75 u | helper ring width and region radius (§4) |
| `hole_depth` | 500 u | how far the hole and its tunnel go behind the wall (§4) |
| `hole_front` | 0.5 u | allowance in front of the plane (§4) |
| `region_lookahead` | 0.08 s | look-ahead for the region test (§1.8) |
| `mirror_exit_geometry` | true | copy the exit's nearby geometry behind the entrance (§4) |
| `unstick_step` / `unstick_steps` | 0.05 m / 10 | free-spot search after a teleport (§1.5) |

Everything is dt-correct: it runs at whatever `Time<Fixed>` the host uses.

## What a character needs

A character is any entity with all of these (the plugin attaches its own
`TransitState` automatically):

- `Transform`: translation = **feet** (bottom centre of the hull), rotation = yaw only.
- `CharacterVelocity` (`shared`/`p2_core`): metres per second, world space.
- `ViewAngles`: yaw, pitch, roll. Roll is left over after a transit; the view should ease it out.
- `Crouch`: `ducked`, `hull()`, `hull_centre_offset()`.
- `InterpolatedFeet { prev, curr }`.
- `IgnoreColliders`: the set this plugin fills.

Your controller **must**:

1. Run in `FixedUpdate` in `P2Systems::Move`. Transit is chained after it in `P2Systems::Transit`.
2. Set `InterpolatedFeet::prev = curr` before it moves, and `curr` after.
3. Skip every entity in `IgnoreColliders` in every shape cast and overlap test.
   The set holds the walls behind the portal the character is inside.
4. Treat `PortalHelperCollider` entities as ordinary colliders. They are what
   keep the character out of the wall and let it stand half-way through a portal.
5. On `ForceDuck` (§1.6), duck instantly (air duck).

## Portals

Portals are spawned by your gun or script as `Portal { kind, half_size,
surfaces, placed_at }` plus a `PortalLink` on both halves, with the frame as
`Transform` (+Z out of the surface, +Y up). `surfaces` is the collider(s) the
opening sits in. These are the walls the character ignores while inside.
`Portal::half_size` defaults to `p2_core::PORTAL_HALF_SIZE` (64 x 112 u).

## What it does

- **Teleport** (§1.1-1.7): on the hull centre crossing a linked portal's plane
  from the front, inside the opening. Position, velocity (gravity fix-up,
  floor minimum 300 u/s, tilted-exit arc, 1000 u/s cap), exact view with roll
  left for the view code, `InterpolatedFeet` mapped on both ends. Sends
  `PortalTeleported`, and `ForceDuck` when Portal 2 forces a crouch.
- **Hole and helpers** (§4): while the hull overlaps a hole, the portal's
  `surfaces` go into `IgnoreColliders`, and a ring (0.98x hole out to 75 u, 500 u deep)
  of `PortalHelperCollider`s covers the wall around the hole.
- **Remote geometry** (§4): the exit's static cuboids in front of the exit,
  within 75 u, are copied behind the entrance through the inverse pair, so a
  half-way character stands on the exit side's floor.
- **Unstick** (§1.5): a hull left in solid after a teleport moves to a free
  spot (small search, see config).
- All helper colliders and ignores are removed when the character leaves the hole.

## API

- `p2_transit::Plugin`: the plugin.
- `p2_transit::TransitConfig`: the tunables above.
- `p2_transit::TransitState`: per-character state (`inside`, `last_exit`).
- `p2_transit::PortalHelpers`: helper entities on a portal (for debugging).
- `p2_transit::rules`: the pure maths (`cross`, `rotate_velocity`, `exit_velocity`,
  `forced_duck`, `in_hole`, `ring_pieces`), unit-tested.
- Messages (from `p2_core`): `PortalTeleported`, `ForceDuck`.

## Run the example

```
cargo run -p p2_transit --example transit -- --case wall --exit-after 400
```

Cases: `wall`, `fall`, `wallfloor`, `straddle`, `ceiling`. The example's mover
is a stand-in: a scripted walk or fall with gravity and axis-separated
collision that honours `IgnoreColliders`. It is not a controller to copy.

Unit tests: `cargo test -p p2_transit --lib`.

## Gaps

- Only **cuboid** colliders are copied as remote geometry. Trimeshes and other
  shapes on the exit side are not mirrored. A cuboid that straddles the exit
  plane is clipped only when one of its axes is the plane normal; otherwise it
  is copied only when wholly in front.
- Helper colliders spawned in a tick are visible to avian's spatial queries
  after the next physics step, so a character reaches the ring one tick late.
  The look-ahead covers the normal cases.
- The hole test is conservative (projects the hull onto the portal axes), so it
  can start the ignore a little early. This is safe, not visible.
- The forced-duck centre nudge (§1.5) uses the centre before the duck is
  applied. The hull top is then lowered by the controller, not by this crate.
- A portal whose hole starts at floor level is blocked by the wall lip under the
  hole (0.02 x half height, a few cm) unless the controller can step up. Portal 2
  steps up 18 u. The example's stand-in mover cannot, so its wall portals are
  raised to y = 50 u. Your controller needs a step-up (or the portal set higher).
- Remote geometry, the unstick search and the teleport are only tested on the
  example scene, and the example uses a stand-in mover.
- No Portal 2 spec items beyond transit: no mobile portals, no bowie maneuver
  (§2.2, non-player entities), no body/ghost transit (that is `p2_body`).

## Open questions

- Spec §4 writes the hole as `f >= -0.5` and also "500 u behind the plane". I
  read it as z in [-500 u, +0.5 u] (behind the plane, 0.5 u allowance in front).
  Please confirm against the decompiled source.
- Is `Portal` allowed without a `PortalLink`? This crate still builds its ring
  for an unlinked portal, and the character can still enter its hole.

# p2_portals

Portal 2's portals, portal gun and first-person player for your own Bevy 0.19 +
avian3d 0.7 game, in one plugin group. Behaviour follows the shipped game:
placement rules and edge bumping, Source movement numbers, transit velocity
rules, recursive portal views, the real portal shader look and `.pcf`
effects, and Chell's body duplicated across a portal as you pass through.

## Quick start

```rust
use avian3d::prelude::*;
use bevy::prelude::*;
use p2_portals::core::{spawn_player, P2Settings, Portalable};

fn main() {
    App::new()
        .add_plugins((DefaultPlugins, PhysicsPlugins::default()))
        // Optional: Portal 2's exact 60 Hz tick (otherwise your fixed rate is kept).
        .insert_resource(P2Settings { set_fixed_timestep: true })
        .add_plugins(p2_portals::PortalPlugins)
        .add_systems(Startup, setup)
        .run();
}

fn setup(mut commands: Commands) {
    // Any static collider tagged `Portalable` takes portals; others fizzle shots.
    commands.spawn((RigidBody::Static, Collider::cuboid(20.0, 0.5, 20.0), Portalable, Transform::default()));
    // The player: feet position, yaw. Camera, viewmodel, crosshair and body come with it.
    spawn_player(&mut commands, Vec3::new(0.0, 0.5, 0.0), 0.0);
}
```

Controls (built-in input): WASD, mouse look (click to grab, Esc to release),
Space jump, Ctrl crouch, left/right click blue/orange.

## What's in the group

| plugin | does | replace it by |
|---|---|---|
| `core::CorePlugin` | messages, system order, portal open/static animation | (required) |
| `assets::AssetsPlugin` | mounts the player's Portal 2 install (required; exits the app without it) | inserting your own `GameFsRes` first |
| `movement::Plugin` / `InputPlugin` | Source/Portal 2 controller / keyboard+mouse | your controller (see `p2_transit`'s README) / writing `MoveInput` + `ViewAngles` |
| `view::Plugin` | first-person camera, viewmodel, crosshair (`ViewConfig`) | your camera tagged `MainCamera` |
| `gun::Plugin` / `MouseInputPlugin` | shooting + placement rules / mouse buttons | sending `FirePortal` yourself |
| `render::Plugin` | views through portals with recursion, portal surface (`PortalRenderConfig`) | |
| `fx::Plugin` | portal edge/open/close/fizzle/projectile particles and sounds | |
| `transit::Plugin` | teleporting characters, walking into the wall behind a portal (`TransitConfig`) | |
| `body::Plugin` | Chell seen through portals + ghost copies across portals (`BodyConfig`, `Ghostable`) | your own model via `BodyModel::Scene` |

Disable a piece with
`PortalPlugins.build().disable::<p2_portals::movement::InputPlugin>()`.

## Game assets

Portal 2 is required, and nothing from it is shipped. At runtime the crates
read the player's own install: `PORTAL2_DIR` (the folder that contains
`portal2/`) if set, otherwise Portal 2 (Steam app 620) is found in any of the
machine's Steam libraries (`libraryfolders.vdf`; native and Flatpak Steam on
Linux, the default Steam folders on Windows and macOS). From it come the
portal gun viewmodel, Chell, portal textures, particle systems and sounds. There are no stand-ins. Without the game,
`assets::AssetsPlugin` logs this and exits the app with an error status:

> Portal 2 is required. These crates load Portal 2's own models, textures,
> particles and sounds from your copy of the game; nothing from the game is
> shipped. Install Portal 2 through Steam, or set PORTAL2_DIR to the folder
> that contains `portal2/`.

A binary can run the same check before opening a window with
`assets::find_portal2()` (as `p2_app` does) and insert the result as
`assets::GameFsRes`.

## Example

`crates/p2_app` (binary `portal2-demo`) is a playable test chamber built
with this group plus the example world `p2_room`.
`portal2-demo --demo <dir>` runs a scripted shoot, walk-through and look-back,
and saves screenshots there.

## Licence

MIT. Behaviour was reverse-engineered from the user's own copy of the game
into plain-English specs. No Valve code or assets are in this repository.

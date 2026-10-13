//! The player's movement sounds (research/specs/player-sounds.md §4-§8):
//! footsteps, the jump push-off, landings, wall and ceiling slams, and the
//! falling wind.
//!
//! Everything is derived each fixed tick from the player contract
//! (`CharacterVelocity`, `Grounded`, `Crouch`) and the tick before it, so it
//! works with any controller that fills those components. The step sound
//! comes from the surface under the player: a [`StepSurface`] on the ground
//! collider (or its parent), else the surfaceprop of Portal 2's white test
//! chamber floor.
use std::collections::HashMap;

use bevy::prelude::*;
use p2_assets::GameFsRes;
use p2_core::{CharacterVelocity, Crouch, Grounded, P2Systems, Player, PortalTeleported, U};
use source_assets::kv::{self, KvValue};

use crate::sounds::{PlaySound, PortalLoop};
use crate::FxSettings;

/// The surfaceprop (a `scripts/surfaceproperties*.txt` name such as
/// `"concrete"` or `"metal"`) of a collider, used for the footsteps of a
/// player standing on it. Put it on the collider entity or on its parent.
#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub struct StepSurface(pub String);

/// Material whose surfaceprop is the default for untagged ground.
const DEFAULT_FLOOR: &str = "tile/white_floor_tile004d";
/// Above this upward speed (u/s) leaving the ground counts as a jump.
const JUMP_UP_SPEED: f32 = 140.0;
/// Seconds the falling wind takes to reach a new volume.
const WOOSH_RAMP: f32 = 0.1;

/// Footstep speed thresholds `(walk, run)` in u/s: below `walk` no steps,
/// below `run` the quiet, slow walking cadence.
pub fn step_speeds(ducked: bool) -> (f32, f32) {
    if ducked {
        (10.0, 60.0)
    } else {
        (90.0, 220.0)
    }
}

/// Milliseconds until the next footstep after one is played.
pub fn step_interval_ms(walking: bool, ducked: bool) -> f32 {
    (if walking { 400.0 } else { 300.0 }) + if ducked { 100.0 } else { 0.0 }
}

/// Footstep volume for a surface's game material character.
pub fn step_volume(material: char, walking: bool, ducked: bool) -> f32 {
    let (walk, run) = match material.to_ascii_uppercase() {
        'D' => (0.25, 0.55),
        'V' => (0.4, 0.7),
        _ => (0.2, 0.5),
    };
    (if walking { walk } else { run }) * if ducked { 0.65 } else { 1.0 }
}

/// Counts the step timer down by `dt_ms`. True when a step may play this tick.
pub fn step_timer_ready(timer_ms: &mut f32, dt_ms: f32) -> bool {
    if *timer_ms > 0.0 {
        *timer_ms -= dt_ms;
        if *timer_ms < 0.0 {
            *timer_ms = 0.0;
        }
    }
    *timer_ms <= 0.0
}

/// Landing step volume for a fall speed (u/s), `None` for a soft landing.
pub fn landing_volume(fall: f32) -> Option<f32> {
    if fall < 350.0 {
        None
    } else if fall > 580.0 {
        Some(1.0)
    } else if fall > 290.0 {
        Some(0.85)
    } else if fall >= 200.0 {
        Some(0.5)
    } else {
        None
    }
}

/// Step volume for a sudden loss of speed (u/s) against a wall or ceiling.
pub fn slam_volume(stop: f32) -> Option<f32> {
    if stop > 580.0 {
        Some(1.0)
    } else if stop > 290.0 {
        Some(0.85)
    } else {
        None
    }
}

/// Volume and soundlevel (dB) of `PortalPlayer.FallRecover` for a fall speed.
pub fn fall_recover_params(fall: f32) -> (f32, f32) {
    let v = fall * 0.00075;
    (if v < 1.38 { v - 0.38 } else { 1.0 }, (125.0 - fall * 0.03).trunc())
}

/// Volume and soundlevel of `JumpLand.HighVelocityImpact` (wall slam).
pub fn wall_impact_params(speed: f32) -> (f32, f32) {
    let v = speed * 0.00075;
    (if v < 0.88 { (v - 0.38) * 2.0 } else { 1.0 }, (125.0 - speed * 0.06).trunc())
}

/// Volume and soundlevel of `JumpLand.HighVelocityImpactCeiling`.
pub fn ceiling_impact_params(speed: f32) -> (f32, f32) {
    let v = speed * 0.00075;
    (if v < 0.63 { (v - 0.38) * 4.0 } else { 1.0 }, (125.0 - speed * 0.06).trunc())
}

/// Target volume of the falling wind at a speed (u/s).
pub fn woosh_volume(speed: f32) -> f32 {
    if speed < 300.0 {
        0.0
    } else {
        ((speed - 300.0) * 0.0005).min(1.0)
    }
}

/// Step sounds of every surfaceprop: `(base, stepleft, stepright, gamematerial)`.
#[derive(Resource, Default)]
struct StepSurfaces {
    props: HashMap<String, [Option<String>; 4]>,
    default: String,
}

impl StepSurfaces {
    fn add_text(&mut self, text: &str) {
        for (name, v) in &kv::parse(text).entries {
            let KvValue::Block(b) = v else { continue };
            let f = |k: &str| b.get(k).map(|s| s.trim().to_string());
            self.props.insert(name.to_ascii_lowercase(), [f("base"), f("stepleft"), f("stepright"), f("gamematerial")]);
        }
    }

    /// One field of a surfaceprop, following `base`.
    fn field(&self, prop: &str, i: usize) -> Option<&str> {
        let mut cur = prop.to_ascii_lowercase();
        for _ in 0..16 {
            let p = self.props.get(&cur)?;
            if let Some(v) = &p[i] {
                return Some(v);
            }
            cur = p[0].as_ref()?.to_ascii_lowercase();
        }
        None
    }

    /// `(left sound, right sound, game material)` of a surfaceprop.
    fn resolve(&self, prop: Option<&str>) -> (String, String, char) {
        let prop = prop.unwrap_or(&self.default);
        let left = self.field(prop, 1).unwrap_or("Default.StepLeft").to_string();
        let right = self.field(prop, 2).unwrap_or("Default.StepRight").to_string();
        let gm = self.field(prop, 3).and_then(|s| s.chars().next()).unwrap_or('C');
        (left, right, gm)
    }
}

/// Per-player bookkeeping: the step timer and side, last tick's state.
#[derive(Component, Default)]
pub(crate) struct StepState {
    timer_ms: f32,
    left: bool,
    was_grounded: bool,
    prev_vel: Vec3,
    surface: Option<String>,
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<StepSurfaces>()
        .add_systems(Startup, load_surfaces)
        .add_systems(FixedUpdate, (player_sounds, woosh).chain().after(P2Systems::Transit));
}

fn load_surfaces(game: Option<Res<GameFsRes>>, mut out: ResMut<StepSurfaces>) {
    out.default = "default".into();
    let Some(fs) = game.map(|g| g.0.clone()) else { return };
    let files: Vec<String> = fs
        .read("scripts/surfaceproperties_manifest.txt")
        .and_then(|d| {
            let k = kv::parse(&String::from_utf8_lossy(&d));
            let (_, b) = k.root()?;
            Some(b.entries.iter().filter(|(k, _)| k == "file").filter_map(|(_, v)| match v {
                KvValue::Str(s) => Some(s.clone()),
                _ => None,
            }).collect())
        })
        .unwrap_or_else(|| vec!["scripts/surfaceproperties.txt".into()]);
    for f in files {
        if let Some(d) = fs.read(&f) {
            out.add_text(&String::from_utf8_lossy(&d));
        }
    }
    let read = |p: &str| fs.read(p);
    if let Some(prop) = source_assets::vmt::load_vmt(&read, DEFAULT_FLOOR).and_then(|(_, k)| k.get("$surfaceprop").map(str::to_string)) {
        out.default = prop.trim().to_ascii_lowercase();
    }
}

fn surface_of(entity: Option<Entity>, tags: &Query<&StepSurface>, parents: &Query<&ChildOf>) -> Option<String> {
    let e = entity?;
    let tagged = tags.get(e).ok().or_else(|| parents.get(e).ok().and_then(|p| tags.get(p.parent()).ok()));
    tagged.map(|s| s.0.clone())
}

fn player_sounds(
    mut commands: Commands,
    time: Res<Time>,
    settings: Res<FxSettings>,
    surfaces: Res<StepSurfaces>,
    mut teleports: MessageReader<PortalTeleported>,
    mut players: Query<(Entity, &Transform, &CharacterVelocity, &Grounded, &Crouch, Option<&mut StepState>), With<Player>>,
    tags: Query<&StepSurface>,
    parents: Query<&ChildOf>,
    mut sounds: MessageWriter<PlaySound>,
) {
    let teleported: Vec<Entity> = teleports.read().map(|t| t.entity).collect();
    let dt_ms = 1000.0 * time.delta_secs();
    for (entity, tf, vel, ground, crouch, state) in &mut players {
        let v = vel.0 / U;
        let Some(mut st) = state else {
            commands.entity(entity).insert(StepState { was_grounded: ground.on_ground, prev_vel: v, ..default() });
            if settings.sounds {
                sounds.write(PlaySound::looping_on("PortalPlayer.Woosh", entity));
            }
            continue;
        };
        let st: &mut StepState = &mut st;
        let feet = tf.translation;
        let mut out: Vec<PlaySound> = Vec::new();
        // A step sound on the side whose turn it is, then the other side next.
        let step = |st: &mut StepState, out: &mut Vec<PlaySound>, volume: f32| {
            let (l, r, _) = surfaces.resolve(st.surface.as_deref());
            out.push(PlaySound::at(if st.left { l } else { r }, feet).with_volume(volume));
            st.left = !st.left;
        };

        // What the player stands on now (a landing uses the new ground).
        if ground.on_ground {
            st.surface = surface_of(ground.entity, &tags, &parents);
        }
        if !teleported.contains(&entity) {
            let prev = st.prev_vel;
            if st.was_grounded && !ground.on_ground && v.y > JUMP_UP_SPEED {
                // Jump: two full-volume steps (one per foot) on the same
                // channel, so the second replaces the first; the side ends
                // where it started.
                st.left = !st.left;
                step(st, &mut out, 1.0);
            } else if !st.was_grounded && ground.on_ground {
                let fall = -prev.y;
                if let Some(vol) = landing_volume(fall) {
                    st.timer_ms = 400.0;
                    step(st, &mut out, vol.max(0.1));
                    if vol >= 1.0 {
                        let (vol, level) = fall_recover_params(fall);
                        out.push(PlaySound::at("PortalPlayer.FallRecover", feet).with_volume(vol).with_soundlevel(level));
                    }
                }
            }
            // Slams: speed lost this tick sideways (wall) or upwards (ceiling).
            let (lat0, lat1) = (Vec2::new(prev.x, prev.z), Vec2::new(v.x, v.z));
            if let Some(vol) = slam_volume(lat0.length() - lat1.length()) {
                st.timer_ms = 400.0;
                step(st, &mut out, vol.max(0.1));
                if vol >= 1.0 {
                    let d = lat0 - lat1;
                    let (vol, level) = wall_impact_params(d.dot(lat0) / (d.length() + f32::EPSILON));
                    out.push(PlaySound::at("JumpLand.HighVelocityImpact", feet).with_volume(vol).with_soundlevel(level));
                }
            }
            if slam_volume(prev.y - v.y) == Some(1.0) {
                let (vol, level) = ceiling_impact_params(prev.y.abs());
                out.push(PlaySound::at("JumpLand.HighVelocityImpactCeiling", feet).with_volume(vol).with_soundlevel(level));
            }
        }
        st.was_grounded = ground.on_ground;
        st.prev_vel = v;

        // Footsteps, from this tick's end state (the next tick's start).
        let speed = v.length();
        let (walk, run) = step_speeds(crouch.ducked);
        if step_timer_ready(&mut st.timer_ms, dt_ms)
            && speed >= walk
            && ground.on_ground
            && Vec2::new(v.x, v.z).length() > 0.0001
        {
            let walking = speed < run;
            st.timer_ms = step_interval_ms(walking, crouch.ducked);
            let (_, _, gm) = surfaces.resolve(st.surface.as_deref());
            step(st, &mut out, step_volume(gm, walking, crouch.ducked));
        }
        if settings.sounds {
            // The engine drops sounds at zero or negative volume.
            sounds.write_batch(out.into_iter().filter(|s| s.volume.is_none_or(|v| v > 0.0)));
        }
    }
}

/// Fades each player's falling-wind loop toward the volume for its speed.
fn woosh(
    time: Res<Time>,
    players: Query<&CharacterVelocity, With<Player>>,
    mut loops: Query<(&ChildOf, &mut PortalLoop)>,
) {
    let k = (time.delta_secs() / WOOSH_RAMP).min(1.0);
    for (parent, mut l) in &mut loops {
        if let Ok(vel) = players.get(parent.parent()) {
            let s = l.scale();
            l.set_scale(s + (woosh_volume(vel.0.length() / U) - s) * k);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ticks between steps at 60 Hz for a cadence, counted like the game.
    fn ticks_between_steps(interval: f32) -> u32 {
        let mut t = interval;
        let mut n = 0;
        loop {
            n += 1;
            if step_timer_ready(&mut t, 1000.0 * (1.0 / 60.0f32)) {
                return n;
            }
        }
    }

    #[test]
    fn walking_and_crouched_cadence() {
        // Standing at 175 u/s (Portal 2's top speed) is "walking": 400 ms.
        assert_eq!(step_speeds(false), (90.0, 220.0));
        assert!(175.0 < step_speeds(false).1);
        assert_eq!(step_interval_ms(true, false), 400.0);
        assert_eq!(step_interval_ms(false, false), 300.0);
        assert_eq!(step_interval_ms(true, true), 500.0);
        // f32 rounding leaves a sliver after 24 ticks: one step per 25 ticks.
        assert_eq!(ticks_between_steps(400.0), 25);
        assert_eq!(ticks_between_steps(300.0), 18);
        assert_eq!(ticks_between_steps(500.0), 31);
    }

    #[test]
    fn step_volumes() {
        assert_eq!(step_volume('C', true, false), 0.2);
        assert_eq!(step_volume('M', false, false), 0.5);
        assert_eq!(step_volume('D', true, false), 0.25);
        assert_eq!(step_volume('V', false, false), 0.7);
        assert!((step_volume('C', true, true) - 0.13).abs() < 1e-6);
    }

    #[test]
    fn landing_thresholds() {
        assert_eq!(landing_volume(232.0), None, "a normal jump lands silently");
        assert_eq!(landing_volume(349.0), None);
        assert_eq!(landing_volume(350.0), Some(0.85));
        assert_eq!(landing_volume(580.0), Some(0.85));
        assert_eq!(landing_volume(581.0), Some(1.0));
        let (v, l) = fall_recover_params(1000.0);
        assert!((v - 0.37).abs() < 1e-5 && l == 95.0);
        assert_eq!(fall_recover_params(2000.0).0, 1.0);
    }

    #[test]
    fn slams() {
        assert_eq!(slam_volume(290.0), None);
        assert_eq!(slam_volume(300.0), Some(0.85));
        assert_eq!(slam_volume(600.0), Some(1.0));
        let (v, l) = wall_impact_params(800.0);
        assert!((v - 0.44).abs() < 1e-5 && l == 77.0);
        let (v, _) = ceiling_impact_params(600.0);
        assert!((v - 0.28).abs() < 1e-5);
        assert_eq!(ceiling_impact_params(900.0).0, 1.0);
    }

    #[test]
    fn woosh_curve() {
        assert_eq!(woosh_volume(299.0), 0.0);
        assert!((woosh_volume(1300.0) - 0.5).abs() < 1e-6);
        assert_eq!(woosh_volume(3000.0), 1.0);
    }

    /// The system in a minimal app: walk, jump, fall, land, hit a ceiling.
    #[test]
    fn walk_jump_land_ceiling_in_an_app() {
        let mut app = App::new();
        app.add_message::<PlaySound>()
            .add_message::<PortalTeleported>()
            .init_resource::<FxSettings>()
            .insert_resource(StepSurfaces { default: "default".into(), ..default() })
            .init_resource::<Time>()
            .add_systems(Update, player_sounds);
        let p = app
            .world_mut()
            .spawn((Player, Transform::default(), CharacterVelocity::default(), Crouch::default(), Grounded { on_ground: true, ..default() }))
            .id();
        let tick = |app: &mut App, v: Vec3, on_ground: bool| -> Vec<(String, f32)> {
            app.world_mut().get_mut::<CharacterVelocity>(p).unwrap().0 = v * U;
            app.world_mut().get_mut::<Grounded>(p).unwrap().on_ground = on_ground;
            app.world_mut().resource_mut::<Time>().advance_by(std::time::Duration::from_secs_f32(1.0 / 60.0));
            app.update();
            let mut m = app.world_mut().resource_mut::<Messages<PlaySound>>();
            m.drain().map(|s| (s.name, s.volume.unwrap_or(-1.0))).collect()
        };
        assert_eq!(tick(&mut app, Vec3::ZERO, true)[0].0, "PortalPlayer.Woosh");
        // Walking at 175 u/s: right foot now, left foot 25 ticks later, at 0.2.
        let walk: Vec<Vec<(String, f32)>> = (0..26).map(|_| tick(&mut app, Vec3::X * 175.0, true)).collect();
        assert_eq!(walk[0], vec![("Default.StepRight".to_string(), 0.2)]);
        assert!(walk[1..25].iter().all(Vec::is_empty));
        assert_eq!(walk[25], vec![("Default.StepLeft".to_string(), 0.2)]);
        // Jump: one full-volume step on the other foot.
        assert_eq!(tick(&mut app, Vec3::new(175.0, 222.0, 0.0), false), vec![("Default.StepLeft".to_string(), 1.0)]);
        // Land at 700 u/s: a landing step plus the long-fall sound.
        assert!(tick(&mut app, Vec3::new(0.0, -700.0, 0.0), false).is_empty());
        let land = tick(&mut app, Vec3::ZERO, true);
        assert_eq!(land[0], ("Default.StepRight".to_string(), 1.0));
        assert_eq!(land[1].0, "PortalPlayer.FallRecover");
        assert!((land[1].1 - (700.0 * 0.00075 - 0.38)).abs() < 1e-5);
        // Rising at 700 u/s into a ceiling.
        tick(&mut app, Vec3::new(0.0, 700.0, 0.0), false);
        let ceiling = tick(&mut app, Vec3::ZERO, false);
        assert_eq!(ceiling[0].0, "JumpLand.HighVelocityImpactCeiling");
        assert!((ceiling[0].1 - (700.0 * 0.00075 - 0.38) * 4.0).abs() < 1e-5);
    }

    #[test]
    fn surfaceprop_inheritance() {
        let mut s = StepSurfaces { default: "default".into(), ..default() };
        s.add_text(r#""default" { "stepleft" "Default.StepLeft" "stepright" "Default.StepRight" "gamematerial" "C" }
            "metal" { "base" "default" "stepleft" "Metal.StepLeft" "gamematerial" "M" }
            "metalvent" { "base" "metal" "gamematerial" "V" }"#);
        assert_eq!(s.resolve(Some("MetalVent")), ("Metal.StepLeft".into(), "Default.StepRight".into(), 'V'));
        assert_eq!(s.resolve(None).2, 'C');
    }
}

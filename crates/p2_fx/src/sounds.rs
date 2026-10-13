//! Portal sounds: the game's soundscripts (`scripts/game_sounds*.txt`) and
//! waves, read from the player's Portal 2 install. Played with Bevy audio, with
//! volumes and pitch ranges from the soundscript. A sound the game does not
//! provide is logged once and not played.
//!
//! Distance falloff is an approximation of Source's soundlevel (see [`gain`]).
//! Sounds are not panned: the host's `SpatialListener` is not required.
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use std::num::NonZero;
use std::time::Duration;

use bevy::audio::{AddAudioSource, AudioSinkPlayback, Decodable, Source, Volume};
use bevy::prelude::*;
use p2_assets::GameFsRes;
use p2_core::MainCamera;
use source_assets::kv::{self, KvValue};

use crate::FxSettings;

/// Soundscript files searched for entries, in the game's manifest order.
const SOUNDSCRIPTS: &[&str] = &[
    "scripts/game_sounds_weapons_portal.txt",
    "scripts/game_sounds_portal.txt",
    "scripts/game_sounds_physics.txt",
    "scripts/game_sounds_player_portal2.txt",
];

/// Seconds a looping portal hum takes to fade up after the portal opens.
const LOOP_FADE_IN: f32 = 0.5;

/// Decoded PCM audio (16-bit input, stored as f32). Plays through Bevy's audio
/// without a codec: Bevy's default features cannot read WAV.
#[derive(Asset, TypePath, Clone)]
pub struct PcmClip {
    samples: Arc<[f32]>,
    channels: u16,
    rate: u32,
}

/// Iterator over a [`PcmClip`] for the audio mixer.
#[derive(Clone)]
pub struct PcmIter {
    clip: PcmClip,
    pos: usize,
}

impl Iterator for PcmIter {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.clip.samples.get(self.pos).copied();
        self.pos += 1;
        s
    }
}

impl Source for PcmIter {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> bevy::audio::ChannelCount {
        NonZero::new(self.clip.channels).unwrap_or(NonZero::<u16>::MIN)
    }
    fn sample_rate(&self) -> bevy::audio::SampleRate {
        NonZero::new(self.clip.rate).unwrap_or(NonZero::<u32>::MIN)
    }
    fn total_duration(&self) -> Option<Duration> {
        let frames = self.clip.samples.len() as f32 / self.clip.channels.max(1) as f32;
        Some(Duration::from_secs_f32(frames / self.clip.rate.max(1) as f32))
    }
}

impl Decodable for PcmClip {
    type Decoder = PcmIter;
    fn decoder(&self) -> PcmIter {
        PcmIter { clip: self.clone(), pos: 0 }
    }
}

/// One soundscript entry as the portal sounds use it.
#[derive(Clone, Debug)]
struct SoundEntry {
    waves: Vec<String>,
    volume: f32,
    /// `operator_stacks` / `update_stack` / `volume_mult` / `input1` (1 if absent).
    mult: f32,
    pitch: (f32, f32),
    soundlevel: f32,
}

/// Every resolved sound: the soundscript entries and their loaded waves.
#[derive(Resource, Default)]
pub struct PortalSounds {
    entries: HashMap<String, SoundEntry>,
    clips: HashMap<String, Handle<PcmClip>>,
    counter: u32,
}

/// Ask for a sound by soundscript name (e.g. `"Portal.open_blue"`).
///
/// With `loop_on`, the sound loops as a child of that entity (a portal's
/// `Portal.ambient_loop`), fades up over [`LOOP_FADE_IN`] s and follows the
/// distance to the main camera. Otherwise it plays once at `position`.
#[derive(Message, Clone, Debug)]
pub struct PlaySound {
    /// Soundscript name, e.g. `Portal.open_blue`.
    pub name: String,
    /// World position for distance falloff (`None` = at the listener).
    pub position: Option<Vec3>,
    /// Loop as a child of this entity.
    pub loop_on: Option<Entity>,
    /// Volume set by the caller in place of the soundscript's own (the
    /// soundscript's operator-stack multiplier still applies).
    pub volume: Option<f32>,
    /// Soundlevel (dB) set by the caller in place of the soundscript's own.
    pub soundlevel: Option<f32>,
}

impl PlaySound {
    /// A one-shot sound at the listener.
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into(), position: None, loop_on: None, volume: None, soundlevel: None }
    }
    /// A one-shot sound at a world position.
    pub fn at(name: impl Into<String>, position: Vec3) -> Self {
        Self { name: name.into(), position: Some(position), loop_on: None, volume: None, soundlevel: None }
    }
    /// Overrides the soundscript volume.
    pub fn with_volume(mut self, volume: f32) -> Self {
        self.volume = Some(volume);
        self
    }
    /// Overrides the soundscript soundlevel (dB).
    pub fn with_soundlevel(mut self, db: f32) -> Self {
        self.soundlevel = Some(db);
        self
    }
    /// A looping sound on an entity.
    pub fn looping_on(name: impl Into<String>, entity: Entity) -> Self {
        Self { name: name.into(), position: None, loop_on: Some(entity), volume: None, soundlevel: None }
    }
}

/// Component on a looping portal sound: its base gain and soundlevel, and when it started.
#[derive(Component, Clone, Copy, Debug)]
pub struct PortalLoop {
    base: f32,
    soundlevel: f32,
    started: f32,
    scale: f32,
}

impl PortalLoop {
    /// Extra gain its owner drives (e.g. the falling wind's speed fade). Default 1.
    pub fn scale(&self) -> f32 {
        self.scale
    }
    /// Sets the extra gain.
    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale;
    }
}

/// Distance gain for a Source soundlevel (dB), 1 at or inside the reference
/// distance and falling as `1/d` beyond it. The reference distance is 3 m at
/// the normal level (75 dB) and grows 10x per 20 dB. This is an approximation of
/// Source's attenuation, not the engine's curve.
pub fn gain(distance_m: f32, soundlevel_db: f32) -> f32 {
    let reference = 3.0 * 10f32.powf((soundlevel_db - 75.0) / 20.0);
    (reference / distance_m.max(1.0)).min(1.0)
}

pub(crate) fn plugin(app: &mut App) {
    app.add_message::<PlaySound>()
        .init_asset::<PcmClip>()
        .add_audio_source::<PcmClip>()
        .init_resource::<PortalSounds>()
        .add_systems(Startup, load_sounds)
        .add_systems(Update, (play_sounds, update_loops).chain());
}

fn parse_range(s: &str) -> (f32, f32) {
    let mut it = s.split(',').filter_map(|v| kv::parse_f32(v.trim()));
    let a = it.next().unwrap_or(100.0);
    let b = it.next().unwrap_or(a);
    (a, b)
}

/// `SNDLVL_105dB` -> 105, `SNDLVL_NORM` -> 75, anything else -> 75.
fn parse_soundlevel(s: &str) -> f32 {
    let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    digits.parse().unwrap_or(75.0)
}

fn parse_entries(text: &str, out: &mut HashMap<String, SoundEntry>) {
    let root = kv::parse(text);
    for (name, v) in &root.entries {
        let KvValue::Block(b) = v else { continue };
        let strings = |block: &kv::Kv| -> Vec<String> {
            block
                .entries
                .iter()
                .filter(|(k, _)| k == "wave")
                .filter_map(|(_, v)| match v {
                    KvValue::Str(s) => Some(s.clone()),
                    _ => None,
                })
                .collect()
        };
        let mut waves = strings(b);
        if let Some(r) = b.block("rndwave") {
            waves.extend(strings(r));
        }
        if waves.is_empty() {
            continue;
        }
        let volume = b.get("volume").map(parse_range).map(|(a, b)| (a + b) * 0.5).unwrap_or(1.0);
        let pitch = b.get("pitch").map(parse_range).unwrap_or((100.0, 100.0));
        let soundlevel = b.get("soundlevel").map(parse_soundlevel).unwrap_or(75.0);
        let mult = b
            .block("operator_stacks")
            .and_then(|o| o.block("update_stack"))
            .and_then(|u| u.block("volume_mult"))
            .and_then(|m| m.get("input1"))
            .and_then(|v| kv::parse_f32(v.trim()))
            .unwrap_or(1.0);
        out.insert(name.to_ascii_lowercase(), SoundEntry { waves, volume, mult, pitch, soundlevel });
    }
}

/// Strip soundscript prefix characters (`)` spatial, `*` stream, ...).
fn wave_path(w: &str) -> String {
    let w = w.trim_start_matches(|c: char| "*#@<>^)}$!?&~`+%".contains(c));
    format!("sound/{}", w.replace('\\', "/"))
}

/// Parses a RIFF/WAVE file of 16-bit PCM into f32 samples. `None` for anything else.
fn parse_pcm16(b: &[u8]) -> Option<PcmClip> {
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return None;
    }
    let (mut fmt, mut data) = (None, None);
    let mut i = 12;
    while i + 8 <= b.len() {
        let size = u32::from_le_bytes([b[i + 4], b[i + 5], b[i + 6], b[i + 7]]) as usize;
        let body = i + 8;
        let end = (body + size).min(b.len());
        match &b[i..i + 4] {
            b"fmt " if end - body >= 16 => fmt = Some(&b[body..end]),
            b"data" => data = Some(&b[body..end]),
            _ => {}
        }
        i = body + size + (size & 1);
    }
    let fmt = fmt?;
    let (tag, channels) = (u16::from_le_bytes([fmt[0], fmt[1]]), u16::from_le_bytes([fmt[2], fmt[3]]));
    let rate = u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]);
    let bits = u16::from_le_bytes([fmt[14], fmt[15]]);
    if tag != 1 || bits != 16 || channels == 0 {
        return None;
    }
    let samples: Vec<f32> = data?
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
        .collect();
    Some(PcmClip { samples: samples.into(), channels, rate })
}

fn load_sounds(game: Option<Res<GameFsRes>>, mut sounds: ResMut<PortalSounds>) {
    let Some(game) = game else { return };
    for f in SOUNDSCRIPTS {
        match game.read(f) {
            Some(d) => parse_entries(&String::from_utf8_lossy(&d), &mut sounds.entries),
            None => warn!("p2_fx: {f} missing from the Portal 2 install"),
        }
    }
}

/// Resolve a name to (clip, volume, soundlevel, pitch range), loading the wave on first use.
fn resolve(
    name: &str,
    sounds: &mut PortalSounds,
    game: &GameFsRes,
    sources: &mut Assets<PcmClip>,
) -> Option<(Handle<PcmClip>, f32, f32, f32, (f32, f32))> {
    let entry = sounds.entries.get(&name.to_ascii_lowercase()).cloned()?;
    let picks = entry.waves.len();
    sounds.counter = sounds.counter.wrapping_add(1);
    let wave = &entry.waves[(sounds.counter.wrapping_mul(2_654_435_761) >> 7) as usize % picks];
    let path = wave_path(wave);
    let h = match sounds.clips.get(&path) {
        Some(h) => h.clone(),
        // Only plain 16-bit PCM WAV is decoded; anything else is not played.
        None => {
            let h = sources.add(game.read(&path).and_then(|b| parse_pcm16(&b))?);
            sounds.clips.insert(path, h.clone());
            h
        }
    };
    Some((h, entry.volume, entry.mult, entry.soundlevel, entry.pitch))
}

fn play_sounds(
    mut reader: MessageReader<PlaySound>,
    mut sounds: ResMut<PortalSounds>,
    settings: Res<FxSettings>,
    game: Option<Res<GameFsRes>>,
    mut sources: ResMut<Assets<PcmClip>>,
    listener: Query<&GlobalTransform, With<MainCamera>>,
    time: Res<Time>,
    mut missing: Local<HashSet<String>>,
    mut commands: Commands,
) {
    let requests: Vec<PlaySound> = reader.read().cloned().collect();
    let Some(game) = game.filter(|_| settings.sounds) else { return };
    let ear = listener.iter().next().map(|t| t.translation());
    for msg in requests {
        let Some((clip, volume, mult, level, pitch_range)) = resolve(&msg.name, &mut sounds, &game, &mut sources) else {
            if missing.insert(msg.name.clone()) {
                warn!("p2_fx: sound {} not found in the Portal 2 install; not played", msg.name);
            }
            continue;
        };
        sounds.counter = sounds.counter.wrapping_add(1);
        let t = ((sounds.counter.wrapping_mul(747_796_405) >> 9) % 1000) as f32 / 1000.0;
        let speed = (pitch_range.0 + (pitch_range.1 - pitch_range.0) * t) / 100.0;
        let dist = match (msg.position, ear) {
            (Some(p), Some(e)) => p.distance(e),
            _ => 0.0,
        };
        let level = msg.soundlevel.unwrap_or(level);
        let base = msg.volume.unwrap_or(volume) * mult * settings.master_volume;
        debug!("p2_fx: play {} volume {base:.3} soundlevel {level}", msg.name);
        if let Some(parent) = msg.loop_on {
            if let Ok(mut ec) = commands.get_entity(parent) {
                ec.with_child((
                    AudioPlayer(clip),
                    PlaybackSettings { speed, volume: Volume::Linear(0.0), ..PlaybackSettings::LOOP },
                    PortalLoop { base, soundlevel: level, started: time.elapsed_secs(), scale: 1.0 },
                    Transform::IDENTITY,
                ));
            }
            continue;
        }
        commands.spawn((
            AudioPlayer(clip),
            PlaybackSettings {
                speed,
                volume: Volume::Linear(base * gain(dist, level)),
                ..PlaybackSettings::DESPAWN
            },
        ));
    }
}

/// Keep looping portal hums at the right volume: distance falloff and the fade-in.
fn update_loops(
    time: Res<Time>,
    listener: Query<&GlobalTransform, With<MainCamera>>,
    mut loops: Query<(&mut bevy::audio::AudioSink, &PortalLoop, &GlobalTransform)>,
) {
    let ear = listener.iter().next().map(|t| t.translation());
    let now = time.elapsed_secs();
    for (mut sink, l, tf) in &mut loops {
        let dist = ear.map(|e| tf.translation().distance(e)).unwrap_or(0.0);
        let fade = ((now - l.started) / LOOP_FADE_IN).clamp(0.0, 1.0);
        sink.set_volume(Volume::Linear(l.base * gain(dist, l.soundlevel) * fade * l.scale));
    }
}

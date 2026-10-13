//! Optional keyboard and mouse input for the controller. Host apps that drive
//! [`MoveInput`] and [`ViewAngles`] themselves don't need this plugin.
//!
//! Nothing is read while [`InputLocked`] is set, and the cursor is never grabbed then.
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use p2_core::{InputLocked, MoveInput, Player, P2Systems, ViewAngles};

use crate::config::MovementConfig;
use crate::rules;

/// Keyboard, mouse look and cursor grab, in `Update` / `P2Systems::Input`.
///
/// WASD writes [`MoveInput::wish`], Ctrl [`MoveInput::duck`], Space latches
/// [`MoveInput::jump`] (the fixed tick consumes it), E sets [`MoveInput::use_key`].
/// Mouse motion turns [`ViewAngles`] while the cursor is grabbed. Left click
/// grabs the cursor and Esc releases it. Does nothing while [`InputLocked`] is true.
pub struct InputPlugin;

impl bevy::app::Plugin for InputPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (grab_cursor, mouse_look, read_keys).in_set(P2Systems::Input));
    }
}

/// Click to grab the cursor, Esc to release it.
fn grab_cursor(
    locked: Res<InputLocked>,
    mouse: Option<Res<ButtonInput<MouseButton>>>,
    keys: Option<Res<ButtonInput<KeyCode>>>,
    mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>,
) {
    if locked.0 {
        return;
    }
    let Ok(mut c) = cursor.single_mut() else { return };
    if mouse.is_some_and(|m| m.just_pressed(MouseButton::Left)) && c.grab_mode == CursorGrabMode::None {
        c.grab_mode = CursorGrabMode::Locked;
        c.visible = false;
    }
    if keys.is_some_and(|k| k.just_pressed(KeyCode::Escape)) {
        c.grab_mode = CursorGrabMode::None;
        c.visible = true;
    }
}

/// Mouse look into [`ViewAngles`], only while the cursor is grabbed.
fn mouse_look(
    locked: Res<InputLocked>,
    cfg: Res<MovementConfig>,
    mouse: Option<Res<AccumulatedMouseMotion>>,
    cursor: Query<&CursorOptions, With<PrimaryWindow>>,
    mut q: Query<&mut ViewAngles, With<Player>>,
) {
    if locked.0 {
        return;
    }
    let grabbed = cursor.single().map(|c| c.grab_mode != CursorGrabMode::None).unwrap_or(false);
    let Some(mouse) = mouse else { return };
    if !grabbed || mouse.delta == Vec2::ZERO {
        return;
    }
    for mut view in &mut q {
        let (yaw, pitch) = rules::look(&cfg, view.yaw, view.pitch, mouse.delta);
        view.yaw = yaw;
        view.pitch = pitch;
    }
}

/// Keyboard into [`MoveInput`].
fn read_keys(
    locked: Res<InputLocked>,
    keys: Option<Res<ButtonInput<KeyCode>>>,
    mut q: Query<&mut MoveInput, With<Player>>,
) {
    if locked.0 {
        return;
    }
    let Some(keys) = keys else { return };
    let axis = |pos: KeyCode, neg: KeyCode| (keys.pressed(pos) as i32 - keys.pressed(neg) as i32) as f32;
    let wish = Vec2::new(axis(KeyCode::KeyD, KeyCode::KeyA), axis(KeyCode::KeyW, KeyCode::KeyS));
    let duck = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    let jump = keys.just_pressed(KeyCode::Space);
    let use_key = keys.just_pressed(KeyCode::KeyE);
    for mut input in &mut q {
        input.wish = wish;
        input.duck = duck;
        input.use_key = use_key;
        if jump {
            input.jump = true;
        }
    }
}

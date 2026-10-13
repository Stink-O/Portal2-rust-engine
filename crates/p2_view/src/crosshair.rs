//! Portal 2's HUD crosshair (portal-rendering.md §5):
//!
//! * five single-pixel white dots: the screen centre and one 10 px away in
//!   each of the four directions;
//! * the portal brackets, the blue one centred 6 px left and 10 px above the
//!   screen centre and the orange one 6 px right and 10 px below it. The
//!   outline ("invalid") bracket always draws, at alpha 150; the filled
//!   ("valid") bracket draws over it at full alpha while that portal is placed.
//!   With a one-portal gun all four pieces take that portal's colour at
//!   alpha 150. With no portal gun only the dots draw.
//!
//! Everything is in screen pixels, unscaled: the brackets are their 44x64
//! texels at every resolution, as in the game. Textures:
//! `sprites/hud/portal_crosshairs` from `p2_assets` (`PortalGameAssets::crosshair`).
use bevy::prelude::*;
use bevy::ui::UiTargetCamera;
use p2_assets::portal_textures::PortalGameAssets;
use p2_core::{MainCamera, Player, PortalGun, PortalKind};

/// Texture rectangles (x, y, w, h) in the 256x64 `portal_crosshairs` texture.
const LEFT_EMPTY: [f32; 4] = [2.0, 0.0, 44.0, 64.0];
const RIGHT_EMPTY: [f32; 4] = [50.0, 0.0, 44.0, 64.0];
const LEFT_FULL: [f32; 4] = [98.0, 0.0, 44.0, 64.0];
const RIGHT_FULL: [f32; 4] = [146.0, 0.0, 44.0, 64.0];

/// Alpha of the outline brackets (150 / 255). Filled brackets are opaque.
pub const HUD_ALPHA: f32 = 150.0 / 255.0;
/// Bracket centre offset from the screen centre, in pixels (left bracket is
/// up-left, right bracket down-right).
pub const BRACKET_OFFSET: Vec2 = Vec2::new(6.0, 10.0);
/// Distance of the four outer dots from the centre dot, in pixels.
pub const DOT_GAP: f32 = 10.0;

/// Portal colour brightened for the HUD, as 0-1: each channel above 4 becomes
/// `c + (1 - c/255) * 64` (at most 255); 4 or less becomes 5.
pub fn hud_color(kind: PortalKind) -> Vec3 {
    let c = kind.rgb255();
    let f = |v: f32| if v > 4.0 { (v + (1.0 - v / 255.0) * 64.0).min(255.0).trunc() / 255.0 } else { 5.0 / 255.0 };
    Vec3::new(f(c.x), f(c.y), f(c.z))
}

/// Adds the crosshair update system.
pub(crate) fn build(app: &mut App) {
    app.add_systems(Update, (target_crosshair, update_crosshair));
}

/// Draws the crosshair through the last camera of the view (the viewmodel
/// camera when present, else the [`MainCamera`]), unless the host already set
/// a `UiTargetCamera` on the root. The viewmodel camera runs last, and its
/// post-processing would otherwise overwrite UI drawn on the main camera.
fn target_crosshair(
    mut commands: Commands,
    roots: Query<Entity, (With<CrosshairRoot>, Without<UiTargetCamera>)>,
    main: Query<Entity, With<MainCamera>>,
    viewmodel: Query<Entity, With<crate::viewmodel::ViewModelCamera>>,
) {
    let Some(cam) = viewmodel.single().ok().or_else(|| main.single().ok()) else { return };
    for root in &roots {
        commands.entity(root).insert(UiTargetCamera(cam));
    }
}

/// Root node of the crosshair UI. Hosts may move or hide it.
#[derive(Component)]
pub struct CrosshairRoot;

#[derive(Component, Clone, Copy)]
pub(crate) enum Part {
    /// Outline bracket of the portal drawn on that side (blue left, orange right).
    Outline(PortalKind),
    /// Filled bracket, drawn over the outline.
    Fill(PortalKind),
    /// One dot, at `DOT_GAP` times this offset from the centre.
    Dot(i8, i8),
}

fn rect(r: [f32; 4]) -> Rect {
    Rect::new(r[0], r[1], r[0] + r[2], r[1] + r[3])
}

fn texture_rect(part: Part) -> [f32; 4] {
    match part {
        Part::Outline(PortalKind::Blue) => LEFT_EMPTY,
        Part::Outline(PortalKind::Orange) => RIGHT_EMPTY,
        Part::Fill(PortalKind::Blue) => LEFT_FULL,
        Part::Fill(PortalKind::Orange) => RIGHT_FULL,
        Part::Dot(..) => [0.0, 0.0, 1.0, 1.0],
    }
}

/// Top-left corner and size of a piece relative to the screen centre, in
/// screen pixels. Integer maths as the game's (`centre ± offset - size / 2`).
fn layout(part: Part) -> (Vec2, Vec2) {
    let [_, _, w, h] = texture_rect(part);
    let half = Vec2::new((w / 2.0).floor(), (h / 2.0).floor());
    match part {
        Part::Outline(kind) | Part::Fill(kind) => {
            let side = if kind == PortalKind::Blue { -1.0 } else { 1.0 };
            (BRACKET_OFFSET * side - half, Vec2::new(w, h))
        }
        Part::Dot(x, y) => (Vec2::new(x as f32, y as f32) * DOT_GAP, Vec2::ONE),
    }
}

/// Spawns the crosshair once, when the player appears and the textures are loaded.
pub(crate) fn spawn_crosshair(
    mut commands: Commands,
    cfg: Res<crate::ViewConfig>,
    added: Query<(), Added<Player>>,
    existing: Query<(), With<CrosshairRoot>>,
    game: Option<Res<PortalGameAssets>>,
) {
    if !cfg.crosshair || added.is_empty() || !existing.is_empty() {
        return;
    }
    // Portal 2's own crosshair texture: without the game there is no crosshair.
    let Some(game) = game else { return };
    let image = game.crosshair.clone();
    // Each piece hangs off the screen centre; `update_crosshair` sizes it.
    let node = || Node { position_type: PositionType::Absolute, left: Val::Percent(50.0), top: Val::Percent(50.0), ..default() };
    let bracket = |part: Part| {
        (part, node(), ImageNode { image: image.clone(), rect: Some(rect(texture_rect(part))), color: Color::NONE, ..default() })
    };
    let dot = |x: i8, y: i8| (Part::Dot(x, y), node(), BackgroundColor(Color::WHITE));
    // Drawing order as in the game: dots, outlines, then fills.
    commands.spawn((
        Name::new("Portal crosshair"),
        CrosshairRoot,
        Node { position_type: PositionType::Absolute, width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() },
        bevy::picking::Pickable::IGNORE,
        children![
            dot(0, 0),
            dot(1, 0),
            dot(-1, 0),
            dot(0, 1),
            dot(0, -1),
            bracket(Part::Outline(PortalKind::Blue)),
            bracket(Part::Outline(PortalKind::Orange)),
            bracket(Part::Fill(PortalKind::Blue)),
            bracket(Part::Fill(PortalKind::Orange)),
        ],
    ));
}

/// Colour of a bracket piece for the gun's state.
fn bracket_color(part: Part, gun: Option<&PortalGun>) -> Color {
    let Some(gun) = gun else { return Color::NONE };
    let rgba = |kind: PortalKind, a: f32| {
        let c = hud_color(kind);
        Color::srgba(c.x, c.y, c.z, a)
    };
    let fires = |k: PortalKind| gun.can_fire[k.index()];
    match (fires(PortalKind::Blue), fires(PortalKind::Orange)) {
        (true, true) => match part {
            Part::Outline(kind) => rgba(kind, HUD_ALPHA),
            Part::Fill(kind) if gun.portal(kind).is_some() => rgba(kind, 1.0),
            _ => Color::NONE,
        },
        // A one-portal gun: every piece in that portal's colour.
        (true, false) => rgba(PortalKind::Blue, HUD_ALPHA),
        (false, true) => rgba(PortalKind::Orange, HUD_ALPHA),
        (false, false) => Color::NONE,
    }
}

/// Keeps the pieces at their pixel positions and the brackets in step with
/// the gun. Does nothing when there is no crosshair.
pub(crate) fn update_crosshair(
    cfg: Res<crate::ViewConfig>,
    ui_scale: Option<Res<UiScale>>,
    guns: Query<&PortalGun, With<Player>>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut parts: Query<(&Part, &mut Node, Option<&mut ImageNode>)>,
) {
    let gun = guns.iter().next();
    // UI `Px` values are logical; the game's offsets are physical pixels.
    let sf = windows.iter().next().map_or(1.0, |w| w.scale_factor()) * ui_scale.map_or(1.0, |s| s.0);
    let px = cfg.crosshair_scale / sf;
    for (&part, mut node, img) in &mut parts {
        let (min, size) = layout(part);
        let (w, h, ml, mt) = (Val::Px(size.x * px), Val::Px(size.y * px), Val::Px(min.x * px), Val::Px(min.y * px));
        if node.width != w || node.height != h || node.margin.left != ml || node.margin.top != mt {
            node.width = w;
            node.height = h;
            node.margin = UiRect { left: ml, top: mt, ..default() };
        }
        if let Some(mut img) = img {
            let color = bracket_color(part, gun);
            if img.color != color {
                img.color = color;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hud_colours_are_brightened() {
        // Blue (0, 60, 255): R 0 -> 5 (channels of 4 or less), G 60 -> 60 + (1 - 60/255)*64 = 108.
        let b = hud_color(PortalKind::Blue);
        assert!((b.x * 255.0 - 5.0).abs() < 1e-3, "{b}");
        assert!((b.y * 255.0 - 108.0).abs() < 1e-3, "{b}");
        assert!((b.z * 255.0 - 255.0).abs() < 1e-3, "{b}");
    }

    #[test]
    fn brackets_sit_diagonally_about_the_centre() {
        // Blue bracket centred at (-6, -10): top-left = (-6 - 22, -10 - 32).
        assert_eq!(layout(Part::Outline(PortalKind::Blue)), (Vec2::new(-28.0, -42.0), Vec2::new(44.0, 64.0)));
        assert_eq!(layout(Part::Fill(PortalKind::Orange)), (Vec2::new(-16.0, -22.0), Vec2::new(44.0, 64.0)));
        assert_eq!(layout(Part::Dot(0, -1)), (Vec2::new(0.0, -10.0), Vec2::ONE));
    }

    #[test]
    fn fills_show_only_placed_portals() {
        let gun = PortalGun::full();
        assert_eq!(bracket_color(Part::Fill(PortalKind::Blue), Some(&gun)), Color::NONE);
        assert_ne!(bracket_color(Part::Outline(PortalKind::Blue), Some(&gun)), Color::NONE);
        assert_eq!(bracket_color(Part::Outline(PortalKind::Blue), None), Color::NONE);
        // No portal gun ability at all: only the dots draw.
        assert_eq!(bracket_color(Part::Outline(PortalKind::Blue), Some(&PortalGun::default())), Color::NONE);
    }
}

//! Speech bubbles: FF7-style boxes that pop in anywhere on the virtual
//! screen, with an optional tail pointing at the speaker, and a
//! grow-from-center / shrink-to-center animation.
//!
//! Bubbles render on the UI layer (`screen.rs`), over the background and
//! 3D content; the post-process camera then applies the same PSX-era
//! dithering to them as the rest of the frame.
//!
//! Colors come from the shared [`BubbleTheme`] resource, loaded from
//! `assets/ui.ron` — one place to restyle every bubble (see
//! `sync_theme`).

use std::path::Path;

use bevy::asset::Asset;
use bevy::camera::visibility::RenderLayers;
use bevy::math::primitives::{Rectangle, Triangle2d};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;
use bevy::sprite_render::Material2d;
use bevy::text::TextLayoutInfo;
use serde::Deserialize;

use crate::screen::{GAME_HEIGHT, GAME_WIDTH, UI_LAYER};
use crate::text::{self, TextAssets};

/// Draw order inside a bubble, back to front: black border box, black
/// tail, white fill box, white tail inset, text on top.
const Z_BACK: f32 = 0.0;
const Z_TAIL_BLACK: f32 = 0.1;
const Z_FRONT: f32 = 0.2;
const Z_TAIL_WHITE: f32 = 0.3;
const Z_TEXT: f32 = 0.4;

/// Border thickness in virtual pixels, kept at 1 px by sizing the black
/// shapes 2 px larger than the white ones.
const BORDER: f32 = 1.0;
/// Whitespace between the text and the box edge, per side.
const PADDING: f32 = 3.0;
/// Floor for the fitted box so tiny text still reads as a bubble.
const MIN_SIZE: Vec2 = Vec2::new(10.0, 8.0);
/// Tail length and half-width in virtual pixels.
const TAIL_LEN: f32 = 7.0;
const TAIL_HALF_W: f32 = 3.0;
/// How deep the tail's base sits inside the box, so the white fill
/// covers the seam between tail and box.
const TAIL_OVERLAP: f32 = 2.0;
const OPEN_SECS: f32 = 0.12;
const CLOSE_SECS: f32 = 0.08;
/// Small but nonzero, so the animation never produces a degenerate
/// transform.
const MIN_SCALE: f32 = 1e-3;
/// The `assets/ui.ron` file: global UI tuning, loaded once at startup.
/// Colors are `(r, g, b, a)` floats.
#[derive(Deserialize)]
pub(crate) struct UiConfig {
    bubble: BubbleThemeFile,
}

#[derive(Deserialize)]
struct BubbleThemeFile {
    top_left: [f32; 4],
    top_right: [f32; 4],
    bottom_right: [f32; 4],
    bottom_left: [f32; 4],
    text: [f32; 4],
}

/// Every bubble's colors: a four-corner gradient for the fill plus the
/// text color. Defaults to the classic white box with black text.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub(crate) struct BubbleTheme {
    pub(crate) top_left: Color,
    pub(crate) top_right: Color,
    pub(crate) bottom_right: Color,
    pub(crate) bottom_left: Color,
    pub(crate) text: Color,
}

impl Default for BubbleTheme {
    fn default() -> Self {
        Self::from_file_data([1.0; 4], [1.0; 4], [1.0; 4], [1.0; 4], [0.0, 0.0, 0.0, 1.0])
    }
}

impl BubbleTheme {
    fn from_file_data(
        top_left: [f32; 4],
        top_right: [f32; 4],
        bottom_right: [f32; 4],
        bottom_left: [f32; 4],
        text: [f32; 4],
    ) -> Self {
        Self {
            top_left: srgba(top_left),
            top_right: srgba(top_right),
            bottom_right: srgba(bottom_right),
            bottom_left: srgba(bottom_left),
            text: srgba(text),
        }
    }

    /// Loads `path`, falling back to the default theme when the file is
    /// missing and warning when it exists but is broken.
    pub(crate) fn from_file(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        match ron::from_str::<UiConfig>(&text) {
            Ok(config) => config.bubble.into(),
            Err(e) => {
                warn!(
                    "{} is not a valid UI config, bubbles use the default theme: {e}",
                    path.display()
                );
                Self::default()
            }
        }
    }
}

impl From<BubbleThemeFile> for BubbleTheme {
    fn from(file: BubbleThemeFile) -> Self {
        Self::from_file_data(
            file.top_left,
            file.top_right,
            file.bottom_right,
            file.bottom_left,
            file.text,
        )
    }
}

fn srgba(data: [f32; 4]) -> Color {
    Color::srgba(data[0], data[1], data[2], data[3])
}

/// The bubble fill: a bilinear gradient between the four theme corners.
#[derive(Asset, AsBindGroup, Clone, TypePath, Default)]
pub(crate) struct GradientMaterial {
    #[uniform(0)]
    top_left: LinearRgba,
    #[uniform(1)]
    top_right: LinearRgba,
    #[uniform(2)]
    bottom_right: LinearRgba,
    #[uniform(3)]
    bottom_left: LinearRgba,
}

impl Material2d for GradientMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/bubble_gradient.wgsl".into()
    }
}

impl GradientMaterial {
    fn from_theme(theme: &BubbleTheme) -> Self {
        let mut material = Self::default();
        material.set_corners(theme);
        material
    }

    fn set_corners(&mut self, theme: &BubbleTheme) {
        self.top_left = theme.top_left.into();
        self.top_right = theme.top_right.into();
        self.bottom_right = theme.bottom_right.into();
        self.bottom_left = theme.bottom_left.into();
    }
}

/// The tail fill blends with the box, so it takes the average of the
/// four corners, averaged in sRGB to match what the eye expects.
fn tail_color(theme: &BubbleTheme) -> Color {
    let mut sum = [0.0; 4];
    for color in [
        theme.top_left,
        theme.top_right,
        theme.bottom_right,
        theme.bottom_left,
    ] {
        let [r, g, b, a] = Srgba::from(color).to_f32_array();
        sum = [sum[0] + r, sum[1] + g, sum[2] + b, sum[3] + a];
    }
    Color::srgba(sum[0] / 4.0, sum[1] / 4.0, sum[2] / 4.0, sum[3] / 4.0)
}

/// Everything needed to draw one bubble's shapes; shared handles, built
/// once by `setup` from the theme.
#[derive(Resource, Clone)]
pub(crate) struct BubbleAssets {
    pub(crate) rect: Handle<Mesh>,
    tail: Handle<Mesh>,
    tail_inset: Handle<Mesh>,
    /// Solid border + tail-border color.
    pub(crate) border: Handle<ColorMaterial>,
    /// Tail fill: the theme's corner average, so it blends into the box.
    pub(crate) tail_fill: Handle<ColorMaterial>,
    /// Shared fill gradient; restyled in place on theme changes.
    pub(crate) fill: Handle<GradientMaterial>,
    /// The theme these handles currently reflect; `sync_theme` compares
    /// against it to catch theme changes without relying on change
    /// detection, which bare-world tests can't exercise.
    applied: BubbleTheme,
}

/// Marks a bubble's text child so theme changes can recolor it.
#[derive(Component)]
pub(crate) struct BubbleText;

/// A bubble's state machine; scale is animated on the root transform so
/// box, tail, and text grow from the bubble's center together.
#[derive(Component)]
pub(crate) struct SpeechBubble {
    state: BubbleState,
    /// Fitted = box and tail sized from measured text.
    fitted: bool,
    tail: Option<Vec2>,
    /// Free-placed: the box is clamped on-screen when fitted.
    free: bool,
    /// Stay open until the confirm key, not a timer.
    wait: bool,
    /// Seconds left once fully open; `None` stays open until dismissed.
    ttl: Option<f32>,
    parts: BubbleParts,
}

enum BubbleState {
    Opening { elapsed: f32 },
    Open,
    Closing { elapsed: f32 },
}

impl SpeechBubble {
    /// True once the open animation has finished; wait-mode dismissal
    /// and the script-facing `waiting()` flag both gate on it.
    pub(crate) fn is_open(&self) -> bool {
        matches!(self.state, BubbleState::Open)
    }

    /// Whether this bubble holds the script until the player confirms.
    pub(crate) fn is_waiting(&self) -> bool {
        self.wait
    }
}

struct BubbleParts {
    back: Entity,
    front: Entity,
    tail_black: Option<Entity>,
    tail_white: Option<Entity>,
    text: Entity,
}

/// What a bubble shows and where. `at` is the bubble's center in
/// virtual-screen pixels, (0, 0) top-left; `tail` is the unit direction
/// the pointy bit aims (toward the speaker), `None` for no tail.
pub(crate) struct BubbleParams {
    /// The bubble's content; must be non-empty, since fitting waits for
    /// a nonzero text measurement.
    pub text: String,
    pub at: Vec2,
    pub tail: Option<Vec2>,
    /// Free-placed: the box is clamped to stay on-screen once fitted.
    /// Actor-anchored bubbles are never clamped.
    pub free: bool,
    /// Auto-dismiss once the bubble has been open this long.
    pub ttl: Option<f32>,
    /// Stay open until the player presses confirm; `ttl` still applies
    /// if set, so both can time a bubble out and let the player skip it.
    pub wait: bool,
}

/// Builds the shared shape/material handles bubbles draw with.
pub(crate) fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
    mut fills: ResMut<Assets<GradientMaterial>>,
    theme: Res<BubbleTheme>,
) {
    let (rect, tail, tail_inset) = add_meshes(&mut meshes);
    let (border, tail_fill, fill) = add_materials(&mut materials, &mut fills, &theme);

    commands.insert_resource(BubbleAssets {
        rect,
        tail,
        tail_inset,
        border,
        tail_fill,
        fill,
        applied: *theme,
    });
}

/// Shared with actor-script tests, which need a `BubbleAssets` to spawn
/// through the real path.
pub(crate) fn add_meshes(meshes: &mut Assets<Mesh>) -> (Handle<Mesh>, Handle<Mesh>, Handle<Mesh>) {
    (
        meshes.add(Rectangle::new(1.0, 1.0)),
        meshes.add(centroid_triangle(TAIL_LEN, TAIL_HALF_W)),
        meshes.add(centroid_triangle(TAIL_LEN - BORDER, TAIL_HALF_W - BORDER)),
    )
}

fn add_materials(
    materials: &mut Assets<ColorMaterial>,
    fills: &mut Assets<GradientMaterial>,
    theme: &BubbleTheme,
) -> (
    Handle<ColorMaterial>,
    Handle<ColorMaterial>,
    Handle<GradientMaterial>,
) {
    (
        materials.add(ColorMaterial::from_color(Color::BLACK)),
        materials.add(ColorMaterial::from_color(tail_color(theme))),
        fills.add(GradientMaterial::from_theme(theme)),
    )
}

/// Builds shared handles against a bare world, for tests that spawn
/// bubbles through the real path.
#[cfg(test)]
pub(crate) fn test_assets(world: &mut World) -> BubbleAssets {
    test_assets_with_theme(world, BubbleTheme::default())
}

/// [`test_assets`] with an explicit theme, for tests that assert on
/// themed colors.
#[cfg(test)]
pub(crate) fn test_assets_with_theme(world: &mut World, theme: BubbleTheme) -> BubbleAssets {
    // Sequential borrows: a bare world hands out one resource at a
    // time, unlike a system's disjoint `ResMut`s.
    let (rect, tail, tail_inset) = {
        let mut meshes = world.resource_mut::<Assets<Mesh>>();
        add_meshes(&mut meshes)
    };
    let (border, tail_fill) = {
        let mut materials = world.resource_mut::<Assets<ColorMaterial>>();
        let border = materials.add(ColorMaterial::from_color(Color::BLACK));
        let tail_fill = materials.add(ColorMaterial::from_color(tail_color(&theme)));
        (border, tail_fill)
    };
    let fill = {
        let mut fills = world.resource_mut::<Assets<GradientMaterial>>();
        fills.add(GradientMaterial::from_theme(&theme))
    };
    BubbleAssets {
        rect,
        tail,
        tail_inset,
        border,
        tail_fill,
        fill,
        applied: theme,
    }
}

/// Spawns a bubble and returns the root entity, for `dismiss_bubble`.
pub(crate) fn spawn_bubble(
    commands: &mut Commands,
    assets: &BubbleAssets,
    text_assets: &TextAssets,
    theme: &BubbleTheme,
    params: BubbleParams,
) -> Entity {
    let tail = params
        .tail
        .filter(|d| *d != Vec2::ZERO)
        .map(Vec2::normalize);
    // RenderLayers doesn't propagate to children, and the UI camera
    // only looks at UI_LAYER — every renderable child needs it.
    let layers = RenderLayers::layer(UI_LAYER);
    let (text2d, text_font) = text::pixel_text(params.text, text_assets);
    let text = commands
        .spawn((
            text2d,
            text_font,
            TextColor(theme.text),
            BubbleText,
            Transform::from_xyz(0.0, 0.0, Z_TEXT),
            layers.clone(),
        ))
        .id();
    let back = commands
        .spawn((
            Mesh2d(assets.rect.clone()),
            MeshMaterial2d(assets.border.clone()),
            Transform::from_xyz(0.0, 0.0, Z_BACK),
            layers.clone(),
        ))
        .id();
    let front = commands
        .spawn((
            Mesh2d(assets.rect.clone()),
            MeshMaterial2d(assets.fill.clone()),
            Transform::from_xyz(0.0, 0.0, Z_FRONT),
            layers.clone(),
        ))
        .id();
    let (tail_black, tail_white) = match tail {
        Some(_) => {
            let black = commands
                .spawn((
                    Mesh2d(assets.tail.clone()),
                    MeshMaterial2d(assets.border.clone()),
                    Transform::from_xyz(0.0, 0.0, Z_TAIL_BLACK),
                    layers.clone(),
                ))
                .id();
            let white = commands
                .spawn((
                    Mesh2d(assets.tail_inset.clone()),
                    MeshMaterial2d(assets.tail_fill.clone()),
                    Transform::from_xyz(0.0, 0.0, Z_TAIL_WHITE),
                    layers,
                ))
                .id();
            (Some(black), Some(white))
        }
        None => (None, None),
    };
    let mut children = vec![back, front, text];
    children.extend(tail_black);
    children.extend(tail_white);

    commands
        .spawn((
            SpeechBubble {
                state: BubbleState::Opening { elapsed: 0.0 },
                fitted: false,
                tail,
                free: params.free,
                wait: params.wait,
                ttl: params.ttl,
                parts: BubbleParts {
                    back,
                    front,
                    tail_black,
                    tail_white,
                    text,
                },
            },
            // Root carries Visibility so the shape/text children inherit
            // it; a bare transform root trips warning B0004 instead.
            Visibility::default(),
            // Start collapsed; `animate_bubbles` grows it from here.
            Transform::from_translation(screen_to_world(params.at).extend(0.0))
                .with_scale(Vec3::splat(MIN_SCALE)),
        ))
        .add_children(&children)
        .id()
}

/// Starts a bubble's shrink-away animation; the entity despawns itself
/// when done. Idempotent while already closing.
pub(crate) fn dismiss_bubble(bubble: &mut SpeechBubble) {
    if !matches!(bubble.state, BubbleState::Closing { .. }) {
        bubble.state = BubbleState::Closing { elapsed: 0.0 };
    }
}

/// Closes wait-mode bubbles when the player presses confirm. Only
/// fully-open bubbles respond, so the keypress that opened one never
/// dismisses it in the same breath.
pub(crate) fn dismiss_on_confirm(
    input: Res<crate::input::InputManager>,
    mut bubbles: Query<&mut SpeechBubble>,
) {
    if !input.just_pressed(crate::input::PadButton::Cross) {
        return;
    }
    for mut bubble in &mut bubbles {
        if bubble.wait && bubble.is_open() {
            dismiss_bubble(&mut bubble);
        }
    }
}

/// Restyles every bubble when the theme moves: the shared fill
/// material and tail fill update in place, and open bubbles' text
/// recolors. The editor UI edits the resource (and writes the config
/// file) and this carries it to everything on screen.
pub(crate) fn sync_theme(
    theme: Res<BubbleTheme>,
    assets: Option<ResMut<BubbleAssets>>,
    mut fills: ResMut<Assets<GradientMaterial>>,
    mut tail_fills: ResMut<Assets<ColorMaterial>>,
    mut texts: Query<&mut TextColor, With<BubbleText>>,
) {
    let Some(mut assets) = assets else {
        return;
    };
    if assets.applied == *theme {
        return;
    }
    if let Some(mut fill) = fills.get_mut(&assets.fill) {
        fill.set_corners(&theme);
    }
    if let Some(mut tail_fill) = tail_fills.get_mut(&assets.tail_fill) {
        tail_fill.color = tail_color(&theme);
    }
    for mut color in &mut texts {
        color.0 = theme.text;
    }
    assets.applied = *theme;
}

/// Sizes the box and places the tail once the text has been measured.
/// Until then the bubble stays collapsed at its anchor point.
pub(crate) fn fit_bubbles(
    mut bubbles: Query<(Entity, &mut SpeechBubble)>,
    texts: Query<&TextLayoutInfo>,
    mut transforms: Query<&mut Transform>,
) {
    for (entity, mut bubble) in &mut bubbles {
        if bubble.fitted {
            continue;
        }
        let Ok(layout) = texts.get(bubble.parts.text) else {
            continue;
        };
        if layout.size == Vec2::ZERO {
            continue;
        }
        let size = (layout.size + 2.0 * PADDING).max(MIN_SIZE);
        let half = size / 2.0;
        transforms.get_mut(bubble.parts.back).unwrap().scale = (size + 2.0 * BORDER).extend(Z_BACK);
        transforms.get_mut(bubble.parts.front).unwrap().scale = size.extend(Z_FRONT);

        if bubble.free {
            let mut root = transforms.get_mut(entity).unwrap();
            root.translation =
                clamp_to_screen(root.translation.xy(), half + Vec2::splat(BORDER)).extend(0.0);
        }

        if let Some(dir) = bubble.tail {
            let edge = edge_distance(dir, half);
            // Base sits just inside the box so the white fill hides the
            // seam; the apex pokes out past the border.
            let base = dir * (edge - TAIL_OVERLAP);
            let rotation = Quat::from_rotation_z(dir.y.atan2(dir.x));
            let mut black = transforms
                .get_mut(bubble.parts.tail_black.unwrap())
                .unwrap();
            *black = Transform {
                translation: (base + dir * (TAIL_LEN / 3.0)).extend(Z_TAIL_BLACK),
                rotation,
                ..default()
            };
            // The inset triangle shares the base line; its shorter length
            // leaves the black border showing at the tip and sides.
            let mut white = transforms
                .get_mut(bubble.parts.tail_white.unwrap())
                .unwrap();
            *white = Transform {
                translation: (base + dir * ((TAIL_LEN - BORDER) / 3.0)).extend(Z_TAIL_WHITE),
                rotation,
                ..default()
            };
        }
        bubble.fitted = true;
    }
}

/// Runs the open/close animation and despawns finished bubbles.
pub(crate) fn animate_bubbles(
    mut commands: Commands,
    time: Res<Time>,
    mut bubbles: Query<(Entity, &mut SpeechBubble, &mut Transform)>,
) {
    let dt = time.delta_secs();
    for (entity, mut bubble, mut transform) in &mut bubbles {
        match &mut bubble.state {
            BubbleState::Opening { elapsed } => {
                *elapsed += dt;
                let t = (*elapsed / OPEN_SECS).min(1.0);
                set_scale(&mut transform, ease_out_cubic(t));
                if t >= 1.0 {
                    bubble.state = BubbleState::Open;
                }
            }
            BubbleState::Open => {
                if let Some(ttl) = bubble.ttl.as_mut() {
                    *ttl -= dt;
                    if *ttl <= 0.0 {
                        bubble.state = BubbleState::Closing { elapsed: 0.0 };
                    }
                }
            }
            BubbleState::Closing { elapsed } => {
                *elapsed += dt;
                let t = (*elapsed / CLOSE_SECS).min(1.0);
                set_scale(&mut transform, 1.0 - ease_out_cubic(t));
                if t >= 1.0 {
                    commands.entity(entity).despawn();
                }
            }
        }
    }
}

fn set_scale(transform: &mut Transform, progress: f32) {
    transform.scale = Vec3::splat(MIN_SCALE + progress * (1.0 - MIN_SCALE));
}

/// Virtual-screen pixels (origin top-left, y-down) to Camera2d world
/// coordinates (origin center, y-up).
pub(crate) fn screen_to_world(at: Vec2) -> Vec2 {
    Vec2::new(
        at.x - GAME_WIDTH as f32 / 2.0,
        GAME_HEIGHT as f32 / 2.0 - at.y,
    )
}

/// Keeps the center of a box with `half` extents (border included) so
/// the box stays on the virtual screen. A bubble larger than the screen
/// is left with a small wander range rather than snapping to center.
fn clamp_to_screen(center: Vec2, half: Vec2) -> Vec2 {
    let screen_half = Vec2::new(GAME_WIDTH as f32, GAME_HEIGHT as f32) / 2.0;
    let reach = screen_half - half;
    center.clamp(reach.min(-reach), reach.max(-reach))
}

/// Distance from the box center to its boundary along `dir`.
fn edge_distance(dir: Vec2, half: Vec2) -> f32 {
    let dx = if dir.x != 0.0 {
        half.x / dir.x.abs()
    } else {
        f32::INFINITY
    };
    let dy = if dir.y != 0.0 {
        half.y / dir.y.abs()
    } else {
        f32::INFINITY
    };
    dx.min(dy)
}

/// Isoceles tail triangle with its apex toward +X and centroid at the
/// origin: the base line sits `length / 3` behind, the apex
/// `2 * length / 3` ahead.
fn centroid_triangle(length: f32, half_width: f32) -> Triangle2d {
    Triangle2d::new(
        Vec2::new(length * 2.0 / 3.0, 0.0),
        Vec2::new(-length / 3.0, half_width),
        Vec2::new(-length / 3.0, -half_width),
    )
}

fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    fn bubble_assets(world: &mut World) -> BubbleAssets {
        test_assets(world)
    }

    fn spawn_test_bubble(world: &mut World, tail: Option<Vec2>) -> Entity {
        let assets = bubble_assets(world);
        let text_assets = text::test_assets();
        let theme = BubbleTheme::default();
        let mut commands = world.commands();
        let entity = spawn_bubble(
            &mut commands,
            &assets,
            &text_assets,
            &theme,
            BubbleParams {
                text: "hello".into(),
                at: Vec2::new(160.0, 120.0),
                tail,
                free: false,
                ttl: None,
                wait: false,
            },
        );
        world.flush();
        entity
    }

    #[test]
    fn screen_to_world_maps_corners() {
        assert_eq!(screen_to_world(Vec2::ZERO), Vec2::new(-160.0, 120.0));
        assert_eq!(
            screen_to_world(Vec2::new(320.0, 240.0)),
            Vec2::new(160.0, -120.0)
        );
    }

    #[test]
    fn edge_distance_hits_the_sides_it_should() {
        assert_eq!(edge_distance(Vec2::X, Vec2::new(10.0, 5.0)), 10.0);
        assert_eq!(edge_distance(Vec2::NEG_Y, Vec2::new(10.0, 5.0)), 5.0);
        // The diagonal exits through the top edge at (5, 5).
        assert!(
            (edge_distance(Vec2::ONE.normalize(), Vec2::new(10.0, 5.0)) - 5.0 / 0.5f32.sqrt())
                .abs()
                < 1e-5
        );
    }

    #[test]
    fn ease_out_cubic_starts_ends_and_eases() {
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert_eq!(ease_out_cubic(1.0), 1.0);
        assert!((ease_out_cubic(0.5) - 0.875).abs() < 1e-6);
    }

    #[test]
    fn a_new_bubble_starts_collapsed_and_opening() {
        let mut world = World::new();
        world.insert_resource(Assets::<Mesh>::default());
        world.insert_resource(Assets::<ColorMaterial>::default());
        world.insert_resource(Assets::<GradientMaterial>::default());
        let entity = spawn_test_bubble(&mut world, None);

        let bubble = world.get::<SpeechBubble>(entity).unwrap();
        assert!(matches!(
            bubble.state,
            BubbleState::Opening { elapsed: 0.0 }
        ));
        assert!(!bubble.fitted);
        let transform = world.get::<Transform>(entity).unwrap();
        assert!(transform.scale.x <= MIN_SCALE + 1e-6);
        // Center of the virtual screen maps back to the world origin.
        assert!(transform.translation.abs_diff_eq(Vec3::ZERO, 1e-5));
    }

    #[test]
    fn a_bubble_without_tail_spawns_no_tail_entities() {
        let mut world = World::new();
        world.insert_resource(Assets::<Mesh>::default());
        world.insert_resource(Assets::<ColorMaterial>::default());
        world.insert_resource(Assets::<GradientMaterial>::default());
        let entity = spawn_test_bubble(&mut world, None);

        let parts = &world.get::<SpeechBubble>(entity).unwrap().parts;
        assert!(parts.tail_black.is_none());
        assert!(parts.tail_white.is_none());
    }

    #[test]
    fn the_animation_opens_then_dismissal_closes_and_despawns() {
        let mut world = World::new();
        world.insert_resource(Time::<()>::default());
        world.insert_resource(Assets::<Mesh>::default());
        world.insert_resource(Assets::<ColorMaterial>::default());
        world.insert_resource(Assets::<GradientMaterial>::default());
        let entity = spawn_test_bubble(&mut world, None);

        // Halfway through opening, the scale is ease-out cubic at 0.5.
        world
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(OPEN_SECS / 2.0));
        world.run_system_once(animate_bubbles).unwrap();
        let scale = world.get::<Transform>(entity).unwrap().scale.x;
        assert!((scale - 0.875).abs() < 1e-3);

        world
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(OPEN_SECS));
        world.run_system_once(animate_bubbles).unwrap();
        assert!(matches!(
            world.get::<SpeechBubble>(entity).unwrap().state,
            BubbleState::Open
        ));

        dismiss_bubble(world.get_mut::<SpeechBubble>(entity).unwrap().into_inner());
        world
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(CLOSE_SECS));
        world.run_system_once(animate_bubbles).unwrap();

        assert!(world.get::<SpeechBubble>(entity).is_none());
    }

    #[test]
    fn a_timed_bubble_closes_itself_after_its_ttl() {
        let mut world = World::new();
        world.insert_resource(Time::<()>::default());
        world.insert_resource(Assets::<Mesh>::default());
        world.insert_resource(Assets::<ColorMaterial>::default());
        world.insert_resource(Assets::<GradientMaterial>::default());
        let assets = bubble_assets(&mut world);
        let text_assets = text::test_assets();
        let theme = BubbleTheme::default();
        let mut commands = world.commands();
        let entity = spawn_bubble(
            &mut commands,
            &assets,
            &text_assets,
            &theme,
            BubbleParams {
                text: "timed".into(),
                at: Vec2::new(160.0, 120.0),
                tail: None,
                free: false,
                ttl: Some(1.0),
                wait: false,
            },
        );
        world.flush();

        // Fully open, ttl not yet spent: still open.
        world
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(OPEN_SECS));
        world.run_system_once(animate_bubbles).unwrap();
        assert!(matches!(
            world.get::<SpeechBubble>(entity).unwrap().state,
            BubbleState::Open
        ));

        // Open past the ttl: closing; then the close animation despawns.
        world
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(1.0 + CLOSE_SECS));
        world.run_system_once(animate_bubbles).unwrap();
        world.run_system_once(animate_bubbles).unwrap();
        assert!(world.get::<SpeechBubble>(entity).is_none());
    }

    #[test]
    fn dismiss_bubble_is_idempotent_while_closing() {
        let mut world = World::new();
        world.insert_resource(Time::<()>::default());
        world.insert_resource(Assets::<Mesh>::default());
        world.insert_resource(Assets::<ColorMaterial>::default());
        world.insert_resource(Assets::<GradientMaterial>::default());
        let entity = spawn_test_bubble(&mut world, None);

        dismiss_bubble(world.get_mut::<SpeechBubble>(entity).unwrap().into_inner());
        world
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(CLOSE_SECS / 2.0));
        world.run_system_once(animate_bubbles).unwrap();
        dismiss_bubble(world.get_mut::<SpeechBubble>(entity).unwrap().into_inner());

        let state = &world.get::<SpeechBubble>(entity).unwrap().state;
        // Restarting the close would be visible as a scale jump.
        assert!(matches!(state, BubbleState::Closing { elapsed } if *elapsed >= CLOSE_SECS / 2.0));
    }

    #[test]
    fn fitting_sizes_the_box_and_aims_the_tail() {
        let mut world = World::new();
        world.insert_resource(Assets::<Mesh>::default());
        world.insert_resource(Assets::<ColorMaterial>::default());
        world.insert_resource(Assets::<GradientMaterial>::default());
        let entity = spawn_test_bubble(&mut world, Some(Vec2::NEG_Y));
        let text = world.get::<SpeechBubble>(entity).unwrap().parts.text;
        world.entity_mut(text).insert(TextLayoutInfo {
            size: Vec2::new(20.0, 9.0),
            ..default()
        });

        world.run_system_once(fit_bubbles).unwrap();

        let parts = &world.get::<SpeechBubble>(entity).unwrap().parts;
        assert!(world.get::<SpeechBubble>(entity).unwrap().fitted);
        let back = world.get::<Transform>(parts.back).unwrap().scale;
        assert_eq!(
            back.truncate(),
            Vec2::new(20.0 + 2.0 * PADDING + 2.0, 9.0 + 2.0 * PADDING + 2.0)
        );
        let front = world.get::<Transform>(parts.front).unwrap().scale;
        assert_eq!(
            front.truncate(),
            Vec2::new(20.0 + 2.0 * PADDING, 9.0 + 2.0 * PADDING)
        );

        // Tail points up-screen (-Y): base 2 px inside the box, black
        // triangle's centroid a third of its length further out.
        let half_y = (9.0 + 2.0 * PADDING) / 2.0;
        let base_y = -(half_y - TAIL_OVERLAP);
        let black = world.get::<Transform>(parts.tail_black.unwrap()).unwrap();
        assert!(
            (black.translation.y - (base_y - TAIL_LEN / 3.0)).abs() < 1e-5,
            "black tail at {:?}",
            black.translation
        );
        assert!(
            black
                .rotation
                .abs_diff_eq(Quat::from_rotation_z(-core::f32::consts::FRAC_PI_2), 1e-6)
        );
        let white = world.get::<Transform>(parts.tail_white.unwrap()).unwrap();
        assert!(
            (white.translation.y - (base_y - (TAIL_LEN - BORDER) / 3.0)).abs() < 1e-5,
            "white tail at {:?}",
            white.translation
        );
    }

    #[test]
    fn fitting_waits_for_measured_text() {
        let mut world = World::new();
        world.insert_resource(Assets::<Mesh>::default());
        world.insert_resource(Assets::<ColorMaterial>::default());
        world.insert_resource(Assets::<GradientMaterial>::default());
        let entity = spawn_test_bubble(&mut world, None);

        world.run_system_once(fit_bubbles).unwrap();

        let parts = &world.get::<SpeechBubble>(entity).unwrap().parts;
        // Front rect is still unit-scaled: no measurement, no fit.
        assert_eq!(
            world.get::<Transform>(parts.front).unwrap().scale,
            Vec3::ONE
        );
        assert!(!world.get::<SpeechBubble>(entity).unwrap().fitted);
    }

    fn themed_world() -> (World, BubbleTheme, BubbleAssets) {
        let mut world = World::new();
        world.insert_resource(Assets::<Mesh>::default());
        world.insert_resource(Assets::<ColorMaterial>::default());
        world.insert_resource(Assets::<GradientMaterial>::default());
        let theme = BubbleTheme::from_file_data(
            [1.0, 0.0, 0.0, 1.0],
            [0.0, 1.0, 0.0, 1.0],
            [0.0, 0.0, 1.0, 1.0],
            [1.0, 1.0, 0.0, 1.0],
            [0.5, 0.5, 0.5, 1.0],
        );
        let assets = test_assets_with_theme(&mut world, theme);
        world.insert_resource(assets.clone());
        (world, theme, assets)
    }

    #[test]
    fn the_theme_styles_the_bubble() {
        let (mut world, theme, assets) = themed_world();
        let text_assets = text::test_assets();
        let mut commands = world.commands();
        let entity = spawn_bubble(
            &mut commands,
            &assets,
            &text_assets,
            &theme,
            BubbleParams {
                text: "styled".into(),
                at: Vec2::new(160.0, 120.0),
                tail: Some(Vec2::NEG_Y),
                free: false,
                ttl: None,
                wait: false,
            },
        );
        world.flush();

        let bubble = world.get::<SpeechBubble>(entity).unwrap();
        let text = world.get::<TextColor>(bubble.parts.text).unwrap();
        assert_eq!(text.0, theme.text);
        let materials = world.resource::<Assets<ColorMaterial>>();
        let tail_fill = materials.get(&assets.tail_fill).unwrap();
        assert_eq!(tail_fill.color, tail_color(&theme));
        let fills = world.resource::<Assets<GradientMaterial>>();
        let fill = fills.get(&assets.fill).unwrap();
        assert_eq!(fill.top_left, LinearRgba::from(theme.top_left));
        assert_eq!(fill.bottom_right, LinearRgba::from(theme.bottom_right));
    }

    #[test]
    fn sync_theme_restyles_material_and_text() {
        let (mut world, _, assets) = themed_world();
        world.insert_resource(BubbleTheme::default());
        let text_assets = text::test_assets();
        let mut commands = world.commands();
        let entity = spawn_bubble(
            &mut commands,
            &assets,
            &text_assets,
            &BubbleTheme::default(),
            BubbleParams {
                text: "restyled".into(),
                at: Vec2::new(160.0, 120.0),
                tail: None,
                free: false,
                ttl: None,
                wait: false,
            },
        );
        world.flush();
        let replacement = BubbleTheme::from_file_data(
            [0.1, 0.2, 0.3, 1.0],
            [0.4, 0.5, 0.6, 1.0],
            [0.7, 0.8, 0.9, 1.0],
            [1.0, 0.9, 0.8, 1.0],
            [0.2, 0.4, 0.6, 1.0],
        );
        world.insert_resource(replacement);

        world.run_system_once(sync_theme).unwrap();

        let fills = world.resource::<Assets<GradientMaterial>>();
        let fill = fills.get(&assets.fill).unwrap();
        assert_eq!(fill.top_left, LinearRgba::from(replacement.top_left));
        let materials = world.resource::<Assets<ColorMaterial>>();
        assert_eq!(
            materials.get(&assets.tail_fill).unwrap().color,
            tail_color(&replacement)
        );
        let text = world
            .get::<TextColor>(world.get::<SpeechBubble>(entity).unwrap().parts.text)
            .unwrap();
        assert_eq!(text.0, replacement.text);
    }

    /// Spawns a wait-mode bubble in its initial opening state.
    fn spawn_wait_bubble(world: &mut World, wait: bool) -> Entity {
        let assets = bubble_assets(world);
        let text_assets = text::test_assets();
        let theme = BubbleTheme::default();
        let mut commands = world.commands();
        let entity = spawn_bubble(
            &mut commands,
            &assets,
            &text_assets,
            &theme,
            BubbleParams {
                text: "press z".into(),
                at: Vec2::new(160.0, 120.0),
                tail: None,
                free: false,
                ttl: None,
                wait,
            },
        );
        world.flush();
        entity
    }

    /// Animates a bubble until fully open.
    fn open_bubble(world: &mut World, entity: Entity) {
        world
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(OPEN_SECS));
        world.run_system_once(animate_bubbles).unwrap();
        assert!(world.get::<SpeechBubble>(entity).unwrap().is_open());
    }

    fn press_confirm(world: &mut World) {
        let manager =
            crate::input::InputManager::with_just_pressed(&[crate::input::PadButton::Cross]);
        world.insert_resource(manager);
    }

    #[test]
    fn confirm_dismisses_open_wait_bubbles_only() {
        let mut world = World::new();
        world.insert_resource(Time::<()>::default());
        world.insert_resource(Assets::<Mesh>::default());
        world.insert_resource(Assets::<ColorMaterial>::default());
        world.insert_resource(Assets::<GradientMaterial>::default());
        let waiting = spawn_wait_bubble(&mut world, true);
        let plain = spawn_wait_bubble(&mut world, false);
        open_bubble(&mut world, waiting);
        open_bubble(&mut world, plain);
        press_confirm(&mut world);

        world.run_system_once(dismiss_on_confirm).unwrap();

        assert!(matches!(
            world.get::<SpeechBubble>(waiting).unwrap().state,
            BubbleState::Closing { .. }
        ));
        assert!(matches!(
            world.get::<SpeechBubble>(plain).unwrap().state,
            BubbleState::Open
        ));
    }

    #[test]
    fn opening_bubbles_ignore_confirm() {
        let mut world = World::new();
        world.insert_resource(Time::<()>::default());
        world.insert_resource(Assets::<Mesh>::default());
        world.insert_resource(Assets::<ColorMaterial>::default());
        world.insert_resource(Assets::<GradientMaterial>::default());
        let opened = spawn_wait_bubble(&mut world, true);
        open_bubble(&mut world, opened);
        // A second bubble still in its opening animation.
        let fresh = spawn_wait_bubble(&mut world, true);
        press_confirm(&mut world);

        world.run_system_once(dismiss_on_confirm).unwrap();

        assert!(matches!(
            world.get::<SpeechBubble>(fresh).unwrap().state,
            BubbleState::Opening { .. }
        ));
        assert!(matches!(
            world.get::<SpeechBubble>(opened).unwrap().state,
            BubbleState::Closing { .. }
        ));
    }

    #[test]
    fn free_bubbles_clamp_onscreen_when_fitted() {
        let mut world = World::new();
        world.insert_resource(Assets::<Mesh>::default());
        world.insert_resource(Assets::<ColorMaterial>::default());
        world.insert_resource(Assets::<GradientMaterial>::default());
        let assets = bubble_assets(&mut world);
        let text_assets = text::test_assets();
        let theme = BubbleTheme::default();
        let mut commands = world.commands();
        let free = spawn_bubble(
            &mut commands,
            &assets,
            &text_assets,
            &theme,
            BubbleParams {
                text: "free".into(),
                // Near the top-left corner of the virtual screen.
                at: Vec2::new(5.0, 5.0),
                tail: None,
                free: true,
                ttl: None,
                wait: false,
            },
        );
        let anchored = spawn_bubble(
            &mut commands,
            &assets,
            &text_assets,
            &theme,
            BubbleParams {
                text: "anchored".into(),
                at: Vec2::new(5.0, 5.0),
                tail: None,
                free: false,
                ttl: None,
                wait: false,
            },
        );
        world.flush();
        // Oversized text: the fitted box cannot fully fit at (5, 5).
        let layout = TextLayoutInfo {
            size: Vec2::new(300.0, 200.0),
            ..default()
        };
        for bubble in [free, anchored] {
            let text = world.get::<SpeechBubble>(bubble).unwrap().parts.text;
            world.entity_mut(text).insert(layout.clone());
        }

        world.run_system_once(fit_bubbles).unwrap();

        let half = (Vec2::new(300.0, 200.0) + 2.0 * PADDING) / 2.0 + Vec2::splat(BORDER);
        let reach = Vec2::new(GAME_WIDTH as f32, GAME_HEIGHT as f32) / 2.0 - half;
        let free_at = world.get::<Transform>(free).unwrap().translation;
        assert!(free_at.x >= -reach.x - 1e-4 && free_at.y <= reach.y + 1e-4);
        // Actor-anchored bubbles keep their exact placement.
        let anchored_at = world.get::<Transform>(anchored).unwrap().translation;
        assert_eq!(
            anchored_at,
            screen_to_world(Vec2::new(5.0, 5.0)).extend(0.0)
        );
    }

    #[test]
    fn tail_color_blends_the_corners() {
        let theme = BubbleTheme::from_file_data(
            [1.0, 0.0, 0.0, 1.0],
            [0.0, 1.0, 0.0, 1.0],
            [0.0, 0.0, 1.0, 1.0],
            [1.0, 1.0, 0.0, 1.0],
            [0.0, 0.0, 0.0, 1.0],
        );
        assert_eq!(tail_color(&theme), Color::srgba(0.5, 0.5, 0.25, 1.0));
    }

    #[test]
    fn a_missing_theme_file_falls_back_to_the_default() {
        assert_eq!(
            BubbleTheme::from_file(Path::new("no/such/ui.ron")),
            BubbleTheme::default()
        );
    }
}

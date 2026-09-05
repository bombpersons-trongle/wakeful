//! Speech bubbles: FF7-style boxes that pop in anywhere on the virtual
//! screen, with an optional tail pointing at the speaker, and a
//! grow-from-center / shrink-to-center animation.
//!
//! Bubbles render on the UI layer (`screen.rs`), over the background and
//! 3D content; the post-process camera then applies the same PSX-era
//! dithering to them as the rest of the frame.

use bevy::camera::visibility::RenderLayers;
use bevy::math::primitives::{Rectangle, Triangle2d};
use bevy::prelude::*;
use bevy::text::TextLayoutInfo;

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

/// Everything needed to draw one bubble's shapes; shared handles, built
/// once by `setup`.
#[derive(Resource)]
pub(crate) struct BubbleAssets {
    rect: Handle<Mesh>,
    tail: Handle<Mesh>,
    tail_inset: Handle<Mesh>,
    black: Handle<ColorMaterial>,
    white: Handle<ColorMaterial>,
}

/// A bubble's state machine; scale is animated on the root transform so
/// box, tail, and text grow from the bubble's center together.
#[derive(Component)]
pub(crate) struct SpeechBubble {
    state: BubbleState,
    /// Fitted = box and tail sized from measured text.
    fitted: bool,
    tail: Option<Vec2>,
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
    /// True once the open animation has finished. Test-only so far;
    /// drop the gate when a game system needs it.
    #[cfg(test)]
    pub(crate) fn is_open(&self) -> bool {
        matches!(self.state, BubbleState::Open)
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
    /// Auto-dismiss once the bubble has been open this long.
    pub ttl: Option<f32>,
}

/// Builds the shared shape/material handles bubbles draw with.
pub(crate) fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    let (rect, tail, tail_inset) = add_meshes(&mut meshes);
    let (black, white) = add_materials(&mut materials);

    commands.insert_resource(BubbleAssets {
        rect,
        tail,
        tail_inset,
        black,
        white,
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
) -> (Handle<ColorMaterial>, Handle<ColorMaterial>) {
    (
        materials.add(ColorMaterial::from_color(Color::BLACK)),
        materials.add(ColorMaterial::from_color(Color::WHITE)),
    )
}

/// Builds shared handles against a bare world, for tests that spawn
/// bubbles through the real path.
#[cfg(test)]
pub(crate) fn test_assets(world: &mut World) -> BubbleAssets {
    // Sequential borrows: a bare world hands out one resource at a
    // time, unlike a system's disjoint `ResMut`s.
    let (rect, tail, tail_inset) = {
        let mut meshes = world.resource_mut::<Assets<Mesh>>();
        add_meshes(&mut meshes)
    };
    let (black, white) = {
        let mut materials = world.resource_mut::<Assets<ColorMaterial>>();
        add_materials(&mut materials)
    };
    BubbleAssets {
        rect,
        tail,
        tail_inset,
        black,
        white,
    }
}

/// Spawns a bubble and returns the root entity, for `dismiss_bubble`.
pub(crate) fn spawn_bubble(
    commands: &mut Commands,
    assets: &BubbleAssets,
    text_assets: &TextAssets,
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
            TextColor(Color::BLACK),
            Transform::from_xyz(0.0, 0.0, Z_TEXT),
            layers.clone(),
        ))
        .id();
    let back = commands
        .spawn((
            Mesh2d(assets.rect.clone()),
            MeshMaterial2d(assets.black.clone()),
            Transform::from_xyz(0.0, 0.0, Z_BACK),
            layers.clone(),
        ))
        .id();
    let front = commands
        .spawn((
            Mesh2d(assets.rect.clone()),
            MeshMaterial2d(assets.white.clone()),
            Transform::from_xyz(0.0, 0.0, Z_FRONT),
            layers.clone(),
        ))
        .id();
    let (tail_black, tail_white) = match tail {
        Some(_) => {
            let black = commands
                .spawn((
                    Mesh2d(assets.tail.clone()),
                    MeshMaterial2d(assets.black.clone()),
                    Transform::from_xyz(0.0, 0.0, Z_TAIL_BLACK),
                    layers.clone(),
                ))
                .id();
            let white = commands
                .spawn((
                    Mesh2d(assets.tail_inset.clone()),
                    MeshMaterial2d(assets.white.clone()),
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

/// Sizes the box and places the tail once the text has been measured.
/// Until then the bubble stays collapsed at its anchor point.
pub(crate) fn fit_bubbles(
    mut bubbles: Query<&mut SpeechBubble>,
    texts: Query<&TextLayoutInfo>,
    mut transforms: Query<&mut Transform>,
) {
    for mut bubble in &mut bubbles {
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
fn screen_to_world(at: Vec2) -> Vec2 {
    Vec2::new(
        at.x - GAME_WIDTH as f32 / 2.0,
        GAME_HEIGHT as f32 / 2.0 - at.y,
    )
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
        let mut commands = world.commands();
        let entity = spawn_bubble(
            &mut commands,
            &assets,
            &text_assets,
            BubbleParams {
                text: "hello".into(),
                at: Vec2::new(160.0, 120.0),
                tail,
                ttl: None,
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
        let assets = bubble_assets(&mut world);
        let text_assets = text::test_assets();
        let mut commands = world.commands();
        let entity = spawn_bubble(
            &mut commands,
            &assets,
            &text_assets,
            BubbleParams {
                text: "timed".into(),
                at: Vec2::new(160.0, 120.0),
                tail: None,
                ttl: Some(1.0),
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
}

//! Offscreen rendering: the game draws to a fixed-size texture, then a
//! present camera blits that texture to the window, integer-scaled and
//! letterboxed with black bars.
//!
//! Rendering everything into one virtual screen (rather than zooming the
//! camera) means 2D and 3D content get the same pixelation, and geometry
//! can't escape the virtual resolution.
//!
//! This module owns the camera stack that draws into that texture, in
//! order: background (0, `scene.rs`), 3D (1, `camera.rs`), UI (2),
//! post-process (3), then the present camera (4) takes the finished
//! image to the window. The UI camera draws everything on [`UI_LAYER`];
//! the post-process camera draws nothing and exists to carry fullscreen
//! effects over the finished frame.

use crate::dither::{self, DitherPostProcess};
use bevy::asset::RenderAssetUsages;
use bevy::camera::RenderTarget;
use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};
use bevy::window::PrimaryWindow;

/// Virtual resolution the whole game is rendered at.
pub const GAME_WIDTH: u32 = 320;
pub const GAME_HEIGHT: u32 = 240;

/// Size of the offscreen texture as a window-size vector.
pub fn game_size() -> UVec2 {
    UVec2::new(GAME_WIDTH, GAME_HEIGHT)
}

/// Largest whole multiple of `game` that fits inside `window`.
///
/// Whole-number scaling keeps virtual pixels square and evenly sized.
/// Clamped to at least 1 so tiny windows crop the picture instead of
/// breaking the math.
pub fn integer_scale(window: UVec2, game: UVec2) -> u32 {
    (window.x / game.x).min(window.y / game.y).max(1)
}

/// Window-space size of the presented game image.
pub fn presented_size(window: UVec2, game: UVec2) -> UVec2 {
    game * integer_scale(window, game)
}

#[derive(Component)]
pub struct PresentSprite;

/// Draw orders of the cameras after 3D content (order 1) hits the game
/// image; see the module docs for the full stack.
const UI_ORDER: isize = 2;
const POST_PROCESS_ORDER: isize = 3;
const PRESENT_ORDER: isize = 4;

/// Render layer the UI camera draws — speech bubbles today; menus and
/// other UI screens can join.
pub(crate) const UI_LAYER: usize = 3;

/// Render layer no content uses, so the post-process camera's main pass
/// stays empty and its effects see the finished frame.
const POST_PROCESS_LAYER: usize = 31;

/// Marks the UI camera, which draws [`UI_LAYER`] into the game image
/// after 3D content.
#[derive(Component)]
pub(crate) struct UiCamera;

/// Marks the post-process camera: the last camera into the game image,
/// carrying the fullscreen effects that must cover the finished frame.
#[derive(Component)]
pub(crate) struct PostProcessCamera;

/// Spawns the UI camera. Separate from [`setup_screen`] so the headless
/// snapshot_mac_os binary can share it against its own game image.
pub(crate) fn spawn_ui_camera(commands: &mut Commands, game_image: &Handle<Image>) {
    commands.spawn((
        UiCamera,
        Camera2d,
        Camera {
            order: UI_ORDER,
            clear_color: ClearColorConfig::None,
            ..default()
        },
        Msaa::Off,
        RenderTarget::Image(game_image.clone().into()),
        RenderLayers::layer(UI_LAYER),
    ));
}

/// Spawns the post-process camera. Separate from [`setup_screen`] so the
/// headless snapshot_mac_os binary can share it against its own game image.
pub(crate) fn spawn_post_process_camera(commands: &mut Commands, game_image: &Handle<Image>) {
    commands.spawn((
        PostProcessCamera,
        Camera2d,
        Camera {
            order: POST_PROCESS_ORDER,
            clear_color: ClearColorConfig::None,
            ..default()
        },
        Msaa::Off,
        DitherPostProcess { ..dither::tuned() },
        RenderTarget::Image(game_image.clone().into()),
        RenderLayers::layer(POST_PROCESS_LAYER),
    ));
}

/// Handle to the texture the game camera renders into.
///
/// Game code uses this to point its camera at the virtual screen.
#[derive(Resource)]
pub struct GameImage(pub Handle<Image>);

pub fn setup_screen(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let mut image = Image::new_fill(
        Extent3d {
            width: GAME_WIDTH,
            height: GAME_HEIGHT,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[0, 0, 0, 255],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    // new_fill omits RENDER_ATTACHMENT, but the game camera renders into
    // this texture; COPY_SRC lets the screenshot tooling read it back.
    image.texture_descriptor.usage =
        TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_SRC;
    let handle = images.add(image);
    commands.insert_resource(GameImage(handle.clone()));

    spawn_ui_camera(&mut commands, &handle);
    spawn_post_process_camera(&mut commands, &handle);

    // The present camera owns the window: black bars, then the finished
    // game image, integer-scaled and letterboxed.
    commands.spawn((
        Camera2d,
        Camera {
            order: PRESENT_ORDER,
            clear_color: ClearColorConfig::Custom(Color::BLACK),
            ..default()
        },
        // Explicit off like every other camera: the component default is
        // 4x MSAA, which mismatches the single-sampled game image when a
        // 2d pass gains a depth attachment.
        Msaa::Off,
        RenderLayers::layer(1),
        PresentSprite,
        Sprite {
            image: handle,
            ..default()
        },
    ));
}

/// Runs every frame; cheap, and also covers window resizes and scale
/// factor changes for free.
pub fn resize_present(
    window: Query<&Window, With<PrimaryWindow>>,
    mut present: Query<&mut Sprite, With<PresentSprite>>,
) {
    let (Ok(window), Ok(mut sprite)) = (window.single(), present.single_mut()) else {
        return;
    };

    // Sprite sizes are logical units, so scale from the logical window
    // size; physical pixels would over-size the image on scaled displays.
    let logical = UVec2::new(window.width() as u32, window.height() as u32);
    let size = presented_size(logical, game_size()).as_vec2();
    sprite.custom_size = Some(size);
}

/// One game-image camera, reduced to what the post-process layout
/// depends on.
struct GameImageCamera {
    order: isize,
    carries_post_effects: bool,
}

/// Why the game-image camera layout would break the post-process pass:
/// effects must exist on exactly one camera, and nothing may draw after
/// that camera — content above it would render un-dithered (or below a
/// future effect that should cover it).
fn post_process_violations(cameras: &[GameImageCamera]) -> Vec<String> {
    let mut violations = Vec::new();
    let effect_cameras = cameras.iter().filter(|c| c.carries_post_effects).count();
    if effect_cameras != 1 {
        violations.push(format!(
            "expected exactly one post-process camera targeting the game image, found {effect_cameras}"
        ));
    }
    if let Some(effects) = cameras
        .iter()
        .filter(|c| c.carries_post_effects)
        .max_by_key(|c| c.order)
        && let Some(later) = cameras
            .iter()
            .filter(|c| !c.carries_post_effects && c.order > effects.order)
            .max_by_key(|c| c.order)
    {
        violations.push(format!(
            "camera at order {} draws into the game image after the post-process camera at order {} and renders without its effects",
            later.order, effects.order
        ));
    }
    violations
}

/// Warns when a camera breaks the layout the post-process pass depends
/// on (see [`post_process_violations`]). The query is tiny, so this runs
/// every frame; it warns only when the violation set changes, so a
/// steady problem is one warning, not a spam.
pub(crate) fn validate_post_process_layout(
    game_image: Res<GameImage>,
    cameras: Query<(&Camera, &RenderTarget, Option<&DitherPostProcess>)>,
    mut seen: Local<Vec<String>>,
) {
    let mut layout = Vec::new();
    for (camera, target, effects) in &cameras {
        let on_game_image = match target {
            RenderTarget::Image(image) => image.handle == game_image.0,
            _ => false,
        };
        if on_game_image {
            layout.push(GameImageCamera {
                order: camera.order,
                carries_post_effects: effects.is_some(),
            });
        }
    }
    let violations = post_process_violations(&layout);
    if violations != *seen {
        for violation in &violations {
            warn!("{violation}");
        }
        *seen = violations;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_to_exact_multiple() {
        assert_eq!(integer_scale(UVec2::new(1280, 960), game_size()), 4);
    }

    #[test]
    fn fits_the_smaller_axis() {
        // width allows 6x but height only 4x
        assert_eq!(integer_scale(UVec2::new(1920, 1080), game_size()), 4);
    }

    #[test]
    fn never_scales_below_one() {
        assert_eq!(integer_scale(UVec2::new(300, 200), game_size()), 1);
        assert_eq!(integer_scale(UVec2::new(500, 400), game_size()), 1);
    }

    #[test]
    fn presented_size_is_game_size_times_scale() {
        assert_eq!(
            presented_size(UVec2::new(1920, 1080), game_size()),
            UVec2::new(1280, 960)
        );
    }

    #[test]
    fn a_valid_layout_has_no_violations() {
        let cameras = vec![
            GameImageCamera {
                order: 0,
                carries_post_effects: false,
            },
            GameImageCamera {
                order: 2,
                carries_post_effects: false,
            },
            GameImageCamera {
                order: 3,
                carries_post_effects: true,
            },
        ];
        assert!(post_process_violations(&cameras).is_empty());
    }

    #[test]
    fn a_missing_post_process_camera_is_a_violation() {
        let cameras = vec![GameImageCamera {
            order: 0,
            carries_post_effects: false,
        }];
        assert_eq!(post_process_violations(&cameras).len(), 1);
    }

    #[test]
    fn two_post_process_cameras_are_a_violation() {
        let cameras = vec![
            GameImageCamera {
                order: 3,
                carries_post_effects: true,
            },
            GameImageCamera {
                order: 4,
                carries_post_effects: true,
            },
        ];
        assert_eq!(post_process_violations(&cameras).len(), 1);
    }

    #[test]
    fn content_drawing_after_post_process_is_a_violation() {
        let cameras = vec![
            GameImageCamera {
                order: 0,
                carries_post_effects: false,
            },
            GameImageCamera {
                order: 3,
                carries_post_effects: true,
            },
            GameImageCamera {
                order: 4,
                carries_post_effects: false,
            },
        ];
        let violations = post_process_violations(&cameras);
        assert_eq!(violations.len(), 1);
        assert!(violations[0].contains("order 4"));
    }

    #[test]
    fn resize_present_sizes_the_sprite_in_logical_units() {
        use bevy::ecs::system::RunSystemOnce;

        // A scale-2 (Retina) window: physical 1280x960, logical 640x480.
        // Sizing the sprite from physical pixels over-sizes it 2x, which
        // crops the picture and skews cursor mapping.
        let mut world = World::new();
        world.spawn((
            PrimaryWindow,
            Window {
                resolution: bevy::window::WindowResolution::new(1280, 960)
                    .with_scale_factor_override(2.0),
                ..default()
            },
        ));
        let sprite_entity = world.spawn((PresentSprite, Sprite::default())).id();

        world.run_system_once(resize_present).unwrap();

        let size = world.get::<Sprite>(sprite_entity).unwrap().custom_size;
        assert_eq!(size, Some(Vec2::new(640.0, 480.0)));
    }
}

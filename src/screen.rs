//! Offscreen rendering: the game draws to a fixed-size texture, then a
//! present camera blits that texture to the window, integer-scaled and
//! letterboxed with black bars.
//!
//! Rendering everything into one virtual screen (rather than zooming the
//! camera) means 2D and 3D content get the same pixelation, and geometry
//! can't escape the virtual resolution.

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};
use bevy::window::PrimaryWindow;

/// Virtual resolution the whole game is rendered at.
pub const GAME_WIDTH: u32 = 640;
pub const GAME_HEIGHT: u32 = 480;

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

    // The present camera owns the window: black bars, then the texture.
    commands.spawn((
        Camera2d,
        Camera {
            order: 1,
            clear_color: ClearColorConfig::Custom(Color::BLACK),
            ..default()
        },
        RenderLayers::layer(1),
        PresentSprite,
        Sprite {
            image: handle,
            ..default()
        },
    ));
}

/// Runs every frame; cheap, and also covers window resizes for free.
pub fn resize_present(
    window: Query<&Window, With<PrimaryWindow>>,
    mut present: Query<&mut Sprite, With<PresentSprite>>,
) {
    let (Ok(window), Ok(mut sprite)) = (window.single(), present.single_mut()) else {
        return;
    };

    let size = presented_size(window.physical_size(), game_size()).as_vec2();
    sprite.custom_size = Some(size);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_to_exact_multiple() {
        assert_eq!(integer_scale(UVec2::new(1280, 960), game_size()), 2);
    }

    #[test]
    fn fits_the_smaller_axis() {
        // width allows 3x but height only 2x
        assert_eq!(integer_scale(UVec2::new(1920, 1080), game_size()), 2);
    }

    #[test]
    fn never_scales_below_one() {
        assert_eq!(integer_scale(UVec2::new(300, 200), game_size()), 1);
        assert_eq!(integer_scale(UVec2::new(1000, 900), game_size()), 1);
    }

    #[test]
    fn presented_size_is_game_size_times_scale() {
        assert_eq!(
            presented_size(UVec2::new(1920, 1080), game_size()),
            UVec2::new(1280, 960)
        );
    }
}

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
    // Order 2 = after the background (0) and 3D (1) cameras.
    commands.spawn((
        Camera2d,
        Camera {
            order: 2,
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

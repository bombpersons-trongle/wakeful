//! The game's shared pixel-font text: one pre-rendered style for
//! speech bubbles today and menus later.
//!
//! The font is Pixel Operator 8 (Jayvee Enaguas, CC0 — see
//! `assets/fonts/README.md`), whose em is exactly its 8px design block:
//! at [`PIXEL_SIZE`] every glyph pixel lands on the game's virtual
//! pixel grid, and [`FontSmoothing::None`] rasterizes the glyphs once
//! into Bevy's font atlas as hard-edged bitmaps. Layout is UTF-8 end to
//! end, and the text engine falls back to system fonts for glyphs the
//! font doesn't cover — wider script support later is a font-file
//! change, not a code change.

use bevy::prelude::*;
use bevy::text::FontSmoothing;

/// Asset path of the committed pixel font.
const FONT_PATH: &str = "fonts/PixelOperator8.ttf";

/// Pixel Operator 8's native design size: one glyph pixel per virtual
/// game pixel.
pub(crate) const PIXEL_SIZE: f32 = 8.0;

/// The shared font handle, loaded once at startup.
#[derive(Resource)]
pub(crate) struct TextAssets {
    font: Handle<Font>,
}

/// Loads the pixel font.
pub(crate) fn setup(mut commands: Commands, server: Res<AssetServer>) {
    commands.insert_resource(TextAssets {
        font: server.load(FONT_PATH),
    });
}

/// The one text style every in-game text uses. Spawning through here
/// keeps the font, size, and smoothing uniform everywhere.
pub(crate) fn pixel_text(text: impl Into<String>, assets: &TextAssets) -> (Text2d, TextFont) {
    (
        Text2d::new(text),
        TextFont {
            font: assets.font.clone().into(),
            font_size: FontSize::Px(PIXEL_SIZE),
            font_smoothing: FontSmoothing::None,
            ..default()
        },
    )
}

/// A weak font handle for bare test worlds, where nothing rasterizes.
#[cfg(test)]
pub(crate) fn test_assets() -> TextAssets {
    TextAssets {
        font: Handle::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_text_uses_the_shared_pixel_style() {
        let (_, font) = pixel_text("hi", &test_assets());
        assert_eq!(font.font_size, FontSize::Px(PIXEL_SIZE));
        assert_eq!(font.font_smoothing, FontSmoothing::None);
    }
}

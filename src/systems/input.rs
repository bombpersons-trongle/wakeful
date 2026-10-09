//! Input-driven systems that aren't gameplay.

use bevy::prelude::*;

/// Quits with Escape.
pub fn quit_on_escape(keys: Res<ButtonInput<KeyCode>>, mut exit: MessageWriter<AppExit>) {
    if keys.just_pressed(KeyCode::Escape) {
        exit.write(AppExit::Success);
    }
}

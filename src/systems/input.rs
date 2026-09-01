//! Input-driven systems that aren't gameplay.

use bevy::prelude::*;
use bevy_egui::EguiContexts;

use crate::editor::EditorState;

/// Quits with Escape, unless the editor is open — then Escape just closes
/// the editor (unless egui is consuming the key, e.g. in a text field).
pub fn quit_on_escape(
    keys: Res<ButtonInput<KeyCode>>,
    mut exit: MessageWriter<AppExit>,
    mut editor: ResMut<EditorState>,
    mut ctxs: EguiContexts,
) {
    if !keys.just_pressed(KeyCode::Escape) {
        return;
    }
    if editor.open {
        let typing = ctxs
            .ctx_mut()
            .map(|ctx| ctx.egui_wants_keyboard_input())
            .unwrap_or(false);
        if !typing {
            editor.open = false;
        }
        return;
    }
    exit.write(AppExit::Success);
}

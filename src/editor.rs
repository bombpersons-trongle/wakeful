//! In-game scene editor.
//!
//! Toggle with `E`: an egui panel over the game view for editing the live
//! scene asset in place — camera pose, background image and depth map.
//! Scenes save back to their RON file.
//!
//! Because edits go straight into the `Assets<Scene>` entry, gameplay picks
//! them up immediately; there is no separate editor state to reconcile.

use std::path::PathBuf;

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui};

use crate::CurrentScene;
use crate::GameCameraQuery;
use crate::assets::assets_root;
use crate::scene::{CameraPose, Scene};
use crate::screen;
use crate::systems::debug_draw::{draw_teleporters, draw_walk_mesh};

/// Toggles the editor. Safe to use for movement: the player walks with the
/// arrow keys, typing goes to egui fields only while the editor is open.
const TOGGLE_KEY: KeyCode = KeyCode::KeyE;

pub fn plugin(app: &mut App) {
    app.add_plugins(EguiPlugin::default())
        .insert_resource(EditorState::default())
        .add_systems(Startup, open_from_env)
        // Panel and overlay run inside the egui pass so widget state
        // (wants-pointer etc.) is current as the panel is built.
        .add_systems(EguiPrimaryContextPass, (ui, overlay).chain())
        .add_systems(Update, (toggle, sync_camera).chain());
}

#[derive(Resource, Default)]
pub(crate) struct EditorState {
    pub(crate) open: bool,
    background_field: String,
    status: Option<String>,
}

/// Serializes a scene to the RON text written to `.scene` files.
fn scene_to_ron(scene: &Scene) -> Result<String, ron::Error> {
    ron::ser::to_string_pretty(scene, ron::ser::PrettyConfig::default())
}

/// Writes a scene back to its file under the asset folder, returning the
/// written path.
fn save_scene(scene: &Scene, asset_path: &str) -> std::io::Result<PathBuf> {
    let path = assets_root().join(asset_path);
    let ron = scene_to_ron(scene).map_err(std::io::Error::other)?;
    std::fs::write(&path, ron)?;
    Ok(path)
}

fn toggle(
    keys: Res<ButtonInput<KeyCode>>,
    scenes: Res<Assets<Scene>>,
    current: Option<Res<CurrentScene>>,
    mut state: ResMut<EditorState>,
    mut ctxs: EguiContexts,
) {
    let Ok(ctx) = ctxs.ctx_mut() else {
        return;
    };
    if keys.just_pressed(TOGGLE_KEY) && !ctx.egui_wants_keyboard_input() {
        let open = !state.open;
        set_open(&mut state, open, &scenes, current.as_deref());
    }
}

/// Starts with the editor open when `WAKEFUL_EDITOR=1` is set, so the tool
/// works in sandboxed or scripted runs that can't send keystrokes.
fn open_from_env(
    mut state: ResMut<EditorState>,
    scenes: Res<Assets<Scene>>,
    current: Option<Res<CurrentScene>>,
) {
    if std::env::var_os("WAKEFUL_EDITOR").is_none_or(|value| value != "1") {
        return;
    }
    set_open(&mut state, true, &scenes, current.as_deref());
}

/// Opens or closes the editor, re-syncing the text fields from the scene
/// on open.
fn set_open(
    state: &mut EditorState,
    open: bool,
    scenes: &Assets<Scene>,
    current: Option<&CurrentScene>,
) {
    state.open = open;
    state.status = None;
    if open && let Some(scene) = current.and_then(|c| scenes.get(&c.handle)) {
        state.background_field = scene.background.clone().unwrap_or_default();
    }
}

/// While the editor is open the panel is the source of truth for the
/// camera pose; the game camera follows the scene asset every frame.
fn sync_camera(
    state: Option<Res<EditorState>>,
    scenes: Res<Assets<Scene>>,
    current: Option<Res<CurrentScene>>,
    mut cameras: GameCameraQuery,
) {
    let Some(state) = state else {
        return;
    };
    if !state.open {
        return;
    }
    let Some(scene) = current.as_ref().and_then(|c| scenes.get(&c.handle)) else {
        return;
    };
    let Ok((mut transform, mut projection, _)) = cameras.single_mut() else {
        return;
    };
    *transform = Transform::from_translation(scene.camera.position.into())
        .looking_at(scene.camera.target.into(), Vec3::Y);
    *projection = Projection::Perspective(PerspectiveProjection {
        fov: scene.camera.fov_degrees.to_radians(),
        aspect_ratio: screen::GAME_WIDTH as f32 / screen::GAME_HEIGHT as f32,
        ..default()
    });
}

/// Builds the whole panel. Bevy UI systems legitimately gather many
/// params, so the usual arity lint is relaxed here.
#[allow(clippy::too_many_arguments)]
fn ui(
    mut ctxs: EguiContexts,
    mut scenes: ResMut<Assets<Scene>>,
    current: Option<Res<CurrentScene>>,
    state: ResMut<EditorState>,
) {
    let state = state.into_inner();
    if !state.open {
        return;
    }
    let Some(current) = current else {
        return;
    };
    let Some(mut scene) = scenes.get_mut(&current.handle) else {
        return;
    };
    let Ok(ctx) = ctxs.ctx_mut() else {
        return;
    };

    egui::Window::new("Scene editor").show(ctx, |ui| {
        camera_ui(ui, &mut scene);
        ui.separator();
        background_ui(ui, &mut scene, &mut state.background_field);
        ui.separator();
        save_ui(ui, &scene, &current.path, &mut state.status);
    });
}

fn camera_ui(ui: &mut egui::Ui, scene: &mut Scene) {
    ui.label("Fixed camera");
    let mut position = scene.camera.position;
    let mut target = scene.camera.target;
    let mut fov = scene.camera.fov_degrees;
    axis_fields(ui, &mut position, "pos");
    axis_fields(ui, &mut target, "aim");
    ui.horizontal(|ui| {
        ui.label("fov");
        ui.add(
            egui::DragValue::new(&mut fov)
                .range(1.0..=179.0)
                .suffix("°"),
        );
    });
    if position != scene.camera.position
        || target != scene.camera.target
        || fov != scene.camera.fov_degrees
    {
        scene.camera = CameraPose {
            position,
            target,
            fov_degrees: fov,
        };
    }
}

/// One drag field per axis, labeled `x`/`y`/`z`.
fn axis_fields(ui: &mut egui::Ui, value: &mut [f32; 3], label: &str) {
    ui.horizontal(|ui| {
        ui.monospace(label);
        for (axis, field) in value.iter_mut().enumerate() {
            ui.add(
                egui::DragValue::new(field)
                    .speed(0.1)
                    .prefix(['x', 'y', 'z'][axis].to_string() + " "),
            );
        }
    });
}

fn background_ui(ui: &mut egui::Ui, scene: &mut Scene, field: &mut String) {
    ui.label("Background image (path under assets/, empty = none)");
    ui.text_edit_singleline(field);
    // Applies to the scene file only; the card (and the rest of the
    // scene) rebuilds when the scene next applies. The editor's
    // background/depth-map story is due for a rework anyway.
    if ui.button("Apply").clicked() {
        let path = trimmed_path(field);
        if path != scene.background {
            scene.background = path.clone();
        }
    }
}

fn save_ui(ui: &mut egui::Ui, scene: &Scene, asset_path: &str, status: &mut Option<String>) {
    if ui.button("Save scene").clicked() {
        *status = Some(match save_scene(scene, asset_path) {
            Ok(path) => format!("Saved to {}", path.display()),
            Err(e) => format!("Save failed: {e}"),
        });
    }
    if let Some(status) = status {
        ui.label(status.as_str());
    }
}

/// Trims the text field into an asset path: whitespace stripped, empty
/// fields become `None`.
fn trimmed_path(field: &str) -> Option<String> {
    let trimmed = field.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Draws the scene's overlays while editing: the walk mesh in green,
/// teleporter triggers orange. Same rendering as the F2 debug overlay.
fn overlay(
    state: Option<Res<EditorState>>,
    scenes: Res<Assets<Scene>>,
    current: Option<Res<CurrentScene>>,
    mut gizmos: Gizmos,
) {
    let Some(state) = state else {
        return;
    };
    if !state.open {
        return;
    }
    let Some(scene) = current.as_ref().and_then(|c| scenes.get(&c.handle)) else {
        return;
    };
    if let Some(mesh) = &scene.walk_mesh {
        draw_walk_mesh(&mut gizmos, mesh);
    }
    draw_teleporters(&mut gizmos, &scene.teleporters);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenes_round_trip_through_ron() {
        let src = r#"(
            background: Some("backgrounds/room1.png"),
            camera: (position: (0.0, 6.0, 9.0), target: (0.0, 0.0, 0.0), fov_degrees: 45.0),
            depth_map: Some("backgrounds/room1_depth.png"),
        )"#;
        let scene: Scene = ron::from_str(src).unwrap();
        let text = scene_to_ron(&scene).unwrap();
        let reparsed: Scene = ron::from_str(&text).unwrap();
        assert_eq!(reparsed.camera.position, scene.camera.position);
        assert_eq!(reparsed.camera.fov_degrees, scene.camera.fov_degrees);
        assert_eq!(reparsed.background, scene.background);
        assert_eq!(reparsed.depth_map, scene.depth_map);
    }

    #[test]
    fn trimmed_paths_drop_whitespace_and_emptiness() {
        assert_eq!(
            trimmed_path("  models/hero.gltf  "),
            Some("models/hero.gltf".into())
        );
        assert_eq!(trimmed_path("   "), None);
        assert_eq!(trimmed_path(""), None);
    }
}

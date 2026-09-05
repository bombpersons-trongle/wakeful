//! In-game scene editor.
//!
//! Toggle with `E`: an egui panel over the game view for editing the live
//! scene asset in place — camera pose, background image, character model,
//! and the walkable grid (painted with the mouse via a raycast onto the
//! ground plane). Scenes save back to their RON file.
//!
//! Because edits go straight into the `Assets<Scene>` entry, gameplay picks
//! them up immediately; there is no separate editor state to reconcile.

use std::path::PathBuf;

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui};

use crate::scene::{CameraPose, Scene, WalkableGrid};
use crate::screen;
use crate::systems::debug_draw::{draw_teleporters, draw_walkable_grid};
use crate::systems::scene::{gltf_asset_path, spawn_background};
use crate::{BackgroundSprite, CurrentScene, GameCamera, GameCameraQuery, Player, PlayerModel};

/// Read-only camera access for picking rays in the editor.
type GameCameraRefs<'w, 's> =
    Query<'w, 's, (&'static Camera, &'static GlobalTransform), (With<GameCamera>, Without<Player>)>;

/// Toggles the editor. Safe to use for movement: the player walks with the
/// arrow keys, typing goes to egui fields only while the editor is open.
const TOGGLE_KEY: KeyCode = KeyCode::KeyE;

/// Geometry of a freshly added walkable grid, sized to fit the placeholder
/// ground plane.
const NEW_GRID_ORIGIN: [f32; 2] = [-4.0, -4.0];
const NEW_GRID_CELL: f32 = 1.0;
const NEW_GRID_COLS: usize = 8;
const NEW_GRID_ROWS: usize = 8;

/// Where asset files live: the asset server's folder. Mirrors the asset
/// server's root resolution (env override, cargo manifest, or next to
/// the executable) so file access works no matter how the game is
/// launched. Shared with actor script loading.
pub(crate) const ASSETS_DIR: &str = "assets";

pub(crate) fn assets_root() -> PathBuf {
    if let Some(root) = std::env::var_os("BEVY_ASSET_ROOT") {
        return PathBuf::from(root);
    }
    if let Some(root) = std::env::var_os("CARGO_MANIFEST_DIR") {
        return PathBuf::from(root).join(ASSETS_DIR);
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(ASSETS_DIR)))
        .unwrap_or_else(|| PathBuf::from(ASSETS_DIR))
}

pub fn plugin(app: &mut App) {
    app.add_plugins(EguiPlugin::default())
        .insert_resource(EditorState::default())
        .add_systems(Startup, open_from_env)
        // Panel/paint/overlay run inside the egui pass so widget state
        // (wants-pointer etc.) is current when painting is evaluated.
        .add_systems(EguiPrimaryContextPass, (ui, paint, overlay).chain())
        .add_systems(Update, (toggle, sync_camera).chain());
}

#[derive(Resource, Default)]
pub(crate) struct EditorState {
    pub(crate) open: bool,
    /// Walkable value being painted during an active stroke: `Some(true)`
    /// for a left-drag, `Some(false)` for a right-drag, `None` when idle.
    painting: Option<bool>,
    background_field: String,
    character_field: String,
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

/// Window cursor position (logical px) mapped into game-texture pixels,
/// undoing the letterbox math of the present camera (which sizes the
/// picture from the logical window size — see `screen::resize_present`).
/// Returns `None` when the cursor sits on a black bar or outside the window.
pub fn cursor_to_game(
    cursor_logical: Vec2,
    scale_factor: f32,
    window_physical: UVec2,
    game: UVec2,
) -> Option<Vec2> {
    let scale = screen::integer_scale(window_physical, game) as f32;
    let presented = screen::presented_size(window_physical, game).as_vec2();
    let offset = (window_physical.as_vec2() - presented) / 2.0;
    let game_px = (cursor_logical * scale_factor - offset) / scale;
    if game_px.x < 0.0
        || game_px.y < 0.0
        || game_px.x >= game.x as f32
        || game_px.y >= game.y as f32
    {
        return None;
    }
    Some(game_px)
}

/// Intersects a ray with the ground plane (`y = 0`), returning world XZ.
/// `None` for rays that point up or start at/behind the plane.
pub fn raycast_ground(origin: Vec3, dir: Vec3) -> Option<Vec2> {
    if dir.y >= 0.0 {
        return None;
    }
    let t = -origin.y / dir.y;
    if t <= 0.0 {
        return None;
    }
    Some(Vec2::new(origin.x + dir.x * t, origin.z + dir.z * t))
}

/// Remaps row-major cell flags to a new grid size, preserving the cells
/// that exist in both grids.
pub fn resize_cells(
    cells: &[bool],
    old_cols: usize,
    old_rows: usize,
    new_cols: usize,
    new_rows: usize,
) -> Vec<bool> {
    let mut out = vec![false; new_cols * new_rows];
    for row in 0..old_rows.min(new_rows) {
        for col in 0..old_cols.min(new_cols) {
            out[row * new_cols + col] = cells.get(row * old_cols + col).copied().unwrap_or(false);
        }
    }
    out
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
        state.character_field = scene.character_model.clone().unwrap_or_default();
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
    let Ok((mut transform, mut projection)) = cameras.single_mut() else {
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
    mut commands: Commands,
    assets: Res<AssetServer>,
    mut scenes: ResMut<Assets<Scene>>,
    current: Option<Res<CurrentScene>>,
    state: ResMut<EditorState>,
    background_sprites: Query<Entity, With<BackgroundSprite>>,
    players: Query<Entity, With<Player>>,
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
        background_ui(
            ui,
            &mut scene,
            &mut state.background_field,
            &mut commands,
            &assets,
            &background_sprites,
        );
        ui.separator();
        character_ui(
            ui,
            &mut scene,
            &mut state.character_field,
            &mut commands,
            &assets,
            &players,
        );
        ui.separator();
        walkable_ui(ui, &mut scene);
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

fn background_ui(
    ui: &mut egui::Ui,
    scene: &mut Scene,
    field: &mut String,
    commands: &mut Commands,
    assets: &AssetServer,
    sprites: &Query<Entity, With<BackgroundSprite>>,
) {
    ui.label("Background image (path under assets/, empty = none)");
    ui.text_edit_singleline(field);
    if ui.button("Apply").clicked() {
        let path = trimmed_path(field);
        if path != scene.background {
            scene.background = path.clone();
            for entity in sprites.iter() {
                commands.entity(entity).despawn();
            }
            if let Some(path) = &scene.background {
                spawn_background(commands, assets, path);
            }
        }
    }
}

fn character_ui(
    ui: &mut egui::Ui,
    scene: &mut Scene,
    field: &mut String,
    commands: &mut Commands,
    assets: &AssetServer,
    players: &Query<Entity, With<Player>>,
) {
    ui.label("Character model (glTF path under assets/, empty = capsule)");
    ui.text_edit_singleline(field);
    if ui.button("Apply").clicked() {
        let path = trimmed_path(field);
        if path != scene.character_model {
            scene.character_model = path.clone();
            // Rebuild the model: drop the current one, then let the normal
            // player-model application pick the new glTF up once loaded.
            for player in players.iter() {
                commands.entity(player).despawn_children();
            }
            commands.remove_resource::<PlayerModel>();
            if let Some(path) = &scene.character_model {
                commands.insert_resource(PlayerModel(assets.load(gltf_asset_path(path))));
            }
        }
    }
}

fn walkable_ui(ui: &mut egui::Ui, scene: &mut Scene) {
    ui.label("Walkable grid (paint on the ground: left = walk, right = block)");
    let Some(grid) = &mut scene.walkable else {
        if ui.button("Add grid").clicked() {
            scene.walkable = Some(WalkableGrid {
                origin: NEW_GRID_ORIGIN,
                cell_size: NEW_GRID_CELL,
                cols: NEW_GRID_COLS,
                rows: NEW_GRID_ROWS,
                cells: vec![false; NEW_GRID_COLS * NEW_GRID_ROWS],
            });
        }
        return;
    };

    let mut origin = grid.origin;
    let mut cell_size = grid.cell_size;
    let mut cols = grid.cols as isize;
    let mut rows = grid.rows as isize;
    ui.horizontal(|ui| {
        ui.monospace("origin");
        ui.add(
            egui::DragValue::new(&mut origin[0])
                .prefix("x ")
                .speed(0.25),
        );
        ui.add(
            egui::DragValue::new(&mut origin[1])
                .prefix("z ")
                .speed(0.25),
        );
    });
    ui.horizontal(|ui| {
        ui.monospace("cell");
        ui.add(
            egui::DragValue::new(&mut cell_size)
                .range(0.1..=8.0)
                .speed(0.05),
        );
        ui.monospace("grid");
        ui.add(egui::DragValue::new(&mut cols).range(1..=64));
        ui.add(egui::DragValue::new(&mut rows).range(1..=64));
    });
    if origin != grid.origin {
        grid.origin = origin;
    }
    if cell_size > 0.0 && cell_size != grid.cell_size {
        grid.cell_size = cell_size;
    }
    let (cols, rows) = (cols.max(1) as usize, rows.max(1) as usize);
    if (cols, rows) != (grid.cols, grid.rows) {
        grid.cells = resize_cells(&grid.cells, grid.cols, grid.rows, cols, rows);
        grid.cols = cols;
        grid.rows = rows;
    }
    if ui.button("Remove grid").clicked() {
        scene.walkable = None;
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

fn paint(
    mut ctxs: EguiContexts,
    buttons: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    cameras: GameCameraRefs,
    mut scenes: ResMut<Assets<Scene>>,
    current: Option<Res<CurrentScene>>,
    mut state: ResMut<EditorState>,
) {
    if buttons.just_pressed(MouseButton::Left) {
        state.painting = Some(true);
    }
    if buttons.just_pressed(MouseButton::Right) {
        state.painting = Some(false);
    }
    if buttons.any_just_released([MouseButton::Left, MouseButton::Right]) {
        state.painting = None;
    }
    let Some(value) = state.painting else {
        return;
    };
    if !state.open {
        state.painting = None;
        return;
    }
    let Ok(ctx) = ctxs.ctx_mut() else {
        return;
    };
    if ctx.egui_wants_pointer_input() {
        return;
    }

    let Some(mut scene) = current.as_ref().and_then(|c| scenes.get_mut(&c.handle)) else {
        return;
    };
    let Some(grid) = &mut scene.walkable else {
        return;
    };
    let Ok((camera, camera_transform)) = cameras.single() else {
        return;
    };
    let Ok(window) = windows.single() else {
        return;
    };
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    let Some(game_px) = cursor_to_game(
        cursor,
        window.scale_factor(),
        window.physical_size(),
        screen::game_size(),
    ) else {
        return;
    };
    let Ok(ray) = camera.viewport_to_world(camera_transform, game_px) else {
        return;
    };
    let Some(hit) = raycast_ground(ray.origin, *ray.direction) else {
        return;
    };
    grid.set_walkable(hit.x, hit.y, value);
}

/// Draws the scene's overlays on the ground while editing: walkable cells
/// bright, blocked dim, teleporter triggers orange. Same rendering as the
/// F2 debug overlay.
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
    if let Some(grid) = &scene.walkable {
        draw_walkable_grid(&mut gizmos, grid);
    }
    draw_teleporters(&mut gizmos, &scene.teleporters);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ray_hits_the_ground_ahead() {
        let hit = raycast_ground(Vec3::new(0.0, 6.0, 9.0), Vec3::new(0.0, -1.0, 0.0));
        assert_eq!(hit, Some(Vec2::new(0.0, 9.0)));
    }

    #[test]
    fn ray_hits_where_it_points() {
        // looking from above toward -Z: the hit is along the ray
        let hit = raycast_ground(Vec3::new(0.0, 9.0, 0.0), Vec3::new(0.0, -1.0, -1.0));
        assert_eq!(hit, Some(Vec2::new(0.0, -9.0)));
    }

    #[test]
    fn rays_pointing_up_miss() {
        assert_eq!(raycast_ground(Vec3::Y, Vec3::Y), None);
    }

    #[test]
    fn rays_parallel_to_the_ground_miss() {
        assert_eq!(raycast_ground(Vec3::Y, Vec3::X), None);
    }

    #[test]
    fn rays_away_from_the_ground_miss() {
        // origin below the plane, heading further down
        assert_eq!(raycast_ground(Vec3::new(0.0, -1.0, 0.0), Vec3::NEG_Y), None);
    }

    #[test]
    fn cursor_maps_through_the_letterbox() {
        let game = UVec2::new(640, 480);
        // 1280x960 window: 2x scale, no bars; center of the game view
        let center = cursor_to_game(Vec2::new(640.0, 480.0), 1.0, UVec2::new(1280, 960), game);
        assert_eq!(center, Some(Vec2::new(320.0, 240.0)));
    }

    #[test]
    fn cursor_on_the_bars_maps_to_nothing() {
        let game = UVec2::new(640, 480);
        // 1000x900 window: scale 1, so 180px of black on the left/right
        let bar = cursor_to_game(Vec2::new(90.0, 450.0), 1.0, UVec2::new(1000, 900), game);
        assert_eq!(bar, None);
    }

    #[test]
    fn cursor_accounts_for_scale_factor() {
        let game = UVec2::new(640, 480);
        // 2x window scale (HiDPI): logical 640x480 equals physical 1280x960
        let center = cursor_to_game(Vec2::new(320.0, 240.0), 2.0, UVec2::new(1280, 960), game);
        assert_eq!(center, Some(Vec2::new(320.0, 240.0)));
    }

    #[test]
    fn cursor_agrees_with_the_presented_size_on_hidpi() {
        // Pins cursor_to_game against the sprite sizing of resize_present
        // (which scales from the logical window size): the cursor on the
        // presented picture's top-left corner must sample game pixel
        // (0, 0), and the top-left of the last game pixel (319, 239).
        let window = UVec2::new(1400, 1000); // physical
        let scale = 2.0;
        let game = screen::game_size();
        let logical = UVec2::new(
            (window.x as f32 / scale) as u32,
            (window.y as f32 / scale) as u32,
        );
        let presented = screen::presented_size(logical, game).as_vec2();
        let offset = (logical.as_vec2() - presented) / 2.0;
        let game_pixel = presented / game.as_vec2();

        assert_eq!(
            cursor_to_game(offset, scale, window, game),
            Some(Vec2::ZERO)
        );
        assert_eq!(
            cursor_to_game(offset + presented - game_pixel, scale, window, game),
            Some(Vec2::new(319.0, 239.0))
        );
    }

    #[test]
    fn resize_preserves_the_overlap() {
        let cells = resize_cells(&[true, false, false, true], 2, 2, 3, 2);
        // first two columns of each row survive, third column is new
        assert_eq!(cells, [true, false, false, false, true, false]);
    }

    #[test]
    fn resize_shrinks_dropping_out_of_range_cells() {
        let cells = resize_cells(&[true, false, false, true], 2, 2, 1, 1);
        assert_eq!(cells, [true]);
    }

    #[test]
    fn resize_tolerates_short_input() {
        let cells = resize_cells(&[true], 2, 2, 2, 2);
        assert_eq!(cells, [true, false, false, false]);
    }

    #[test]
    fn scenes_round_trip_through_ron() {
        let src = r#"(
            background: Some("backgrounds/room1.png"),
            camera: (position: (0.0, 6.0, 9.0), target: (0.0, 0.0, 0.0), fov_degrees: 45.0),
            walkable: Some((
                origin: (-2.0, -2.0),
                cell_size: 1.0,
                cols: 2,
                rows: 2,
                cells: [true, false, false, true],
            )),
            character_model: None,
        )"#;
        let scene: Scene = ron::from_str(src).unwrap();
        let text = scene_to_ron(&scene).unwrap();
        let reparsed: Scene = ron::from_str(&text).unwrap();
        assert_eq!(reparsed.camera.position, scene.camera.position);
        assert_eq!(reparsed.camera.fov_degrees, scene.camera.fov_degrees);
        assert_eq!(reparsed.background, scene.background);
        assert_eq!(reparsed.character_model, scene.character_model);
        let grid = reparsed.walkable.unwrap();
        assert_eq!(grid.cols, 2);
        assert_eq!(grid.cells, [true, false, false, true]);
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

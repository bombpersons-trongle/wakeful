//! What the editor draws: a menu bar, the actor list, what is selected,
//! and the file it all writes back to.
//!
//! Everything drawn here edits the editor's own copy of the scene (see
//! `scene_view::Working`), never the asset itself — a mutable borrow of an
//! asset announces itself as a modification, and an edit would read as the
//! file changing. The models in the viewport are that copy's view (see
//! `actors`), so an edit needs no apply step: the number you drag is the
//! number the model stands at.
//!
//! Widget layout is not unit tested. egui has no headless tree worth
//! asserting on, and a test that checked our own bookkeeping instead would
//! only prove the test passes.

use bevy::prelude::*;
use bevy::window::{MonitorSelection, WindowMode};
use bevy_egui::{EguiContexts, egui};

use wakeful::assets::assets_root;
use wakeful::scene::{Actor, Scene, to_ron};

use crate::actors::{GroundPointer, Selected};
use crate::scene_view::{OpenScene, Shown, Working};

/// How far from the original a pasted actor lands, so the copy is next to
/// what it was copied from rather than exactly under it.
const PASTE_OFFSET: f32 = 1.0;

/// Where the windows sit before anyone drags them: down the left edge,
/// clear of each other. egui remembers a window's position once it has been
/// moved, so these are only ever the first frame.
const DOCK_LEFT: f32 = 12.0;
const ACTORS_TOP: f32 = 40.0;
const PROPERTIES_TOP: f32 = 320.0;
const STATUS_TOP: f32 = 600.0;
const DIALOG_TOP: f32 = 40.0;

/// A window's starting place, a little in from the left edge.
fn docked(top: f32) -> egui::Pos2 {
    egui::pos2(DOCK_LEFT, top)
}

/// The last actor copied, for paste.
#[derive(Resource, Default)]
pub struct Clipboard(Option<Actor>);

/// What the status line says: the path a save wrote, or why one failed.
#[derive(Resource, Default)]
pub struct Status(String);

/// The glTF files in `assets/models`, so the add menu can offer what is
/// actually there rather than a hardcoded list that goes stale.
#[derive(Resource, Default)]
pub struct Models(Vec<String>);

/// The scene files under `assets/scenes`, by asset path, for File > Open.
///
/// A list rather than a filesystem dialog: everything the editor can open
/// lives in one folder, so a browser that cannot wander off it is both
/// smaller and harder to get wrong than a picker.
#[derive(Resource, Default)]
pub struct Scenes(Vec<String>);

/// What File asked for: which window, and what was typed into it.
#[derive(Resource, Default)]
pub struct Dialog {
    open: bool,
    save_as: bool,
    name: String,
    complaint: Option<String>,
}

/// An edit to the actor list, from either the Edit menu or a row's
/// right-click menu.
#[derive(Clone, Copy)]
enum Edit {
    Copy,
    Paste,
    Delete,
}

/// Lists the models and the scenes on disk, once, at startup.
pub fn list_assets(mut models: ResMut<Models>, mut scenes: ResMut<Scenes>) {
    list_models(&mut models);
    let Ok(entries) = std::fs::read_dir(assets_root().join("scenes")) else {
        return;
    };
    scenes.0 = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "scene"))
        .filter_map(|path| {
            let name = path.file_stem()?.to_str()?;
            Some(format!("scenes/{name}.scene"))
        })
        .collect();
    scenes.0.sort();
}

/// The glTF files a new actor can be.
fn list_models(models: &mut Models) {
    if !models.0.is_empty() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(assets_root().join("models")) else {
        return;
    };
    models.0 = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "glb"))
        .filter_map(|path| {
            let name = path.file_stem()?.to_str()?;
            Some(format!("models/{name}.glb"))
        })
        .collect();
    models.0.sort();
}

/// Everything the editor draws, in one pass: the menu bar, the windows,
/// and whichever dialog File asked for.
#[allow(clippy::too_many_arguments)]
pub fn panels(
    mut ctxs: EguiContexts,
    windows: Query<&mut Window>,
    assets: Res<AssetServer>,
    scenes: Res<Scenes>,
    mut working: ResMut<Working>,
    mut selected: ResMut<Selected>,
    mut clipboard: ResMut<Clipboard>,
    mut status: ResMut<Status>,
    mut dialog: ResMut<Dialog>,
    models: Res<Models>,
    mut open: ResMut<OpenScene>,
    mut shown: ResMut<Shown>,
    pointer: Res<GroundPointer>,
    keys: Res<ButtonInput<KeyCode>>,
    mut exit: MessageWriter<AppExit>,
) {
    let Ok(ctx) = ctxs.ctx_mut() else {
        return;
    };
    // Ctrl-S saves from anywhere, including while a number is being
    // dragged, which is when saving is most likely to be what you want.
    let save_now = keys.just_pressed(KeyCode::KeyS) && keys.pressed(KeyCode::ControlLeft);

    menu_bar(
        ctx,
        windows,
        &mut working,
        &mut selected,
        &mut clipboard,
        &mut status,
        &mut dialog,
        &open,
        &mut shown,
        &pointer,
        save_now,
        &mut exit,
    );
    if dialog.open {
        open_dialog(
            ctx,
            &scenes,
            &assets,
            &mut open,
            &mut shown,
            &mut selected,
            &mut dialog,
        );
    }
    if dialog.save_as {
        save_as_dialog(
            ctx,
            &mut dialog,
            &working,
            &mut status,
            &assets,
            &mut open,
            &mut shown,
            &mut selected,
        );
    }
    let Some(scene) = working.0.as_mut() else {
        return;
    };
    actor_list(ctx, scene, &mut selected, &mut clipboard, &models, &pointer);
    properties(ctx, scene, &selected);
    status_line(ctx, &status, &open);
}

/// The menu bar across the top: what can be done to the file, and to what
/// is selected.
#[allow(clippy::too_many_arguments)]
fn menu_bar(
    ctx: &egui::Context,
    mut windows: Query<&mut Window>,
    working: &mut Working,
    selected: &mut Selected,
    clipboard: &mut Clipboard,
    status: &mut Status,
    dialog: &mut Dialog,
    open: &OpenScene,
    shown: &mut Shown,
    pointer: &GroundPointer,
    save_now: bool,
    exit: &mut MessageWriter<AppExit>,
) {
    // egui's panels take a Ui rather than a Context, so the bar starts
    // from one covering the whole viewport.
    let mut root = egui::Ui::new(
        ctx.clone(),
        "menu-bar".into(),
        egui::UiBuilder::new()
            // Middle, not the background layer: `is_pointer_over_egui`
            // ignores anything on the background, and the camera has to
            // know when a menu has the pointer.
            .layer_id(egui::LayerId::new(
                egui::Order::Middle,
                egui::Id::new("editor"),
            ))
            .max_rect(ctx.viewport_rect()),
    );
    egui::Panel::top("menu").show(&mut root, |ui| {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Open\u{2026}").clicked() {
                    dialog.open = true;
                    ui.close();
                }
                let loaded = working.0.is_some();
                if ui.add_enabled(loaded, egui::Button::new("Save")).clicked()
                    || (save_now && loaded)
                {
                    save_or_report(working.0.as_ref(), &open.path, shown, status);
                    ui.close();
                }
                if ui
                    .add_enabled(loaded, egui::Button::new("Save as\u{2026}"))
                    .clicked()
                {
                    dialog.name = open.name();
                    dialog.complaint = None;
                    dialog.save_as = true;
                    ui.close();
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    exit.write(AppExit::Success);
                }
            });
            ui.menu_button("Edit", |ui| {
                let mut asked = None;
                if ui
                    .add_enabled(selected.0.is_some(), egui::Button::new("Copy actor"))
                    .clicked()
                {
                    asked = Some(Edit::Copy);
                }
                if ui
                    .add_enabled(clipboard.0.is_some(), egui::Button::new("Paste actor"))
                    .clicked()
                {
                    asked = Some(Edit::Paste);
                }
                if ui
                    .add_enabled(selected.0.is_some(), egui::Button::new("Delete actor"))
                    .clicked()
                {
                    asked = Some(Edit::Delete);
                }
                if let Some(edit) = asked {
                    apply_edit(edit, working, selected, clipboard, pointer);
                    ui.close();
                }
            });
            ui.menu_button("View", |ui| {
                let fullscreen = windows
                    .iter()
                    .next()
                    .is_some_and(|window| window.mode != WindowMode::Windowed);
                if ui.selectable_label(fullscreen, "Fullscreen").clicked() {
                    for mut window in &mut windows {
                        window.mode = if fullscreen {
                            WindowMode::Windowed
                        } else {
                            // Borderless, not exclusive: a tool keeps the
                            // desktop's resolution and its escape hatches,
                            // which is what an exclusive fullscreen takes
                            // away along with the title bar.
                            WindowMode::BorderlessFullscreen(MonitorSelection::Current)
                        };
                    }
                    ui.close();
                }
            });
            ui.separator();
            let actors = working.0.as_ref().map_or(0, |scene| scene.actors.len());
            ui.label(format!("{} \u{2014} {actors} actors", open.path));
        });
    });
}

/// Carries out an edit to the actor list, leaving the selection pointing
/// at whatever took the edited actor's place.
fn apply_edit(
    edit: Edit,
    working: &mut Working,
    selected: &mut Selected,
    clipboard: &mut Clipboard,
    pointer: &GroundPointer,
) {
    let Some(scene) = working.0.as_mut() else {
        return;
    };
    match edit {
        Edit::Copy => {
            if let Some(index) = selected.0 {
                copy_actor(scene, index, clipboard);
            }
        }
        Edit::Paste => paste_into(scene, clipboard, pointer, selected),
        Edit::Delete => {
            if let Some(index) = selected.0 {
                delete_actor(scene, index, selected);
            }
        }
    }
}

/// The actor list, with its right-click menu.
fn actor_list(
    ctx: &egui::Context,
    scene: &mut Scene,
    selected: &mut Selected,
    clipboard: &mut Clipboard,
    models: &Models,
    pointer: &GroundPointer,
) {
    let count = scene.actors.len();
    // Which row the pointer is over, which is what tells the two menus
    // apart: a right-click on a name is about that actor, anywhere else
    // in the window — beside a name, under the list — is about the list.
    let mut over_row = None;
    let shown = egui::Window::new("Actors")
        .default_pos(docked(ACTORS_TOP))
        .resizable(true)
        .show(ctx, |ui| {
            if count == 0 {
                ui.label("None yet. Right-click to add one.");
            }
            for index in 0..count {
                let name = actor_label(&scene.actors[index]);
                let chosen = selected.0 == Some(index);
                // The name and not the width of the window, so the space
                // beside a name still belongs to the list. Selection is
                // marked in the text rather than with a bar across the
                // window, because a bar would have to be the width of the
                // window to line up with the rows.
                let row = ui.add(
                    egui::Label::new(if chosen { format!("> {name}") } else { name })
                        .sense(egui::Sense::click()),
                );
                if row.clicked() || row.secondary_clicked() {
                    selected.0 = Some(index);
                }
                if row.contains_pointer() {
                    over_row = Some(index);
                }
            }
        });
    let Some(shown) = shown else {
        return;
    };
    // One menu for the window, wherever the right-click lands: a list
    // sized to its rows has no empty space below to aim at, and a menu
    // that only appeared there is a menu nobody finds.
    shown.response.context_menu(|ui| {
        // A right-click on a name means that actor, whether or not a left
        // click had selected it first.
        if let Some(index) = over_row {
            selected.0 = Some(index);
        }
        let chosen = selected.0.filter(|index| *index < count);
        if over_row.is_some() {
            // The actor's own menu: only what can be done to that actor.
            copy_item(ui, chosen, scene, clipboard);
            delete_item(ui, chosen, scene, selected);
        } else {
            // The list's: what can be done to the scene around them.
            copy_item(ui, chosen, scene, clipboard);
            if ui
                .add_enabled(clipboard.0.is_some(), egui::Button::new("Paste"))
                .clicked()
            {
                paste_into(scene, clipboard, pointer, selected);
            }
            ui.separator();
            ui.label("Add");
            for model in &models.0 {
                if ui.button(short_name(model)).clicked() {
                    scene.actors.push(new_actor(model, pointer));
                    selected.0 = Some(scene.actors.len() - 1);
                }
            }
        }
    });
}

/// Copying, offered only when there is an actor to copy.
fn copy_item(ui: &mut egui::Ui, chosen: Option<usize>, scene: &Scene, clipboard: &mut Clipboard) {
    if ui
        .add_enabled(chosen.is_some(), egui::Button::new("Copy"))
        .clicked()
        && let Some(index) = chosen
    {
        copy_actor(scene, index, clipboard);
    }
}

/// Deleting, likewise.
fn delete_item(ui: &mut egui::Ui, chosen: Option<usize>, scene: &mut Scene, selected: &mut Selected) {
    if ui
        .add_enabled(chosen.is_some(), egui::Button::new("Delete"))
        .clicked()
        && let Some(index) = chosen
    {
        delete_actor(scene, index, selected);
    }
}

/// Copying an actor takes the one it was copied from.
fn copy_actor(scene: &Scene, index: usize, clipboard: &mut Clipboard) {
    clipboard.0 = scene.actors.get(index).cloned();
}

/// Deleting takes the actor out of the list, and deselects it: it is the
/// actor the selection was pointing at that is now gone.
fn delete_actor(scene: &mut Scene, index: usize, selected: &mut Selected) {
    if scene.actors.get(index).is_some() {
        scene.actors.remove(index);
    }
    if selected.0 == Some(index) {
        selected.0 = None;
    }
}

/// Pasting adds a copy of whatever was copied, standing beside where the
/// camera looks, and selects it.
fn paste_into(
    scene: &mut Scene,
    clipboard: &Clipboard,
    pointer: &GroundPointer,
    selected: &mut Selected,
) {
    if let Some(copy) = clipboard.0.clone() {
        let held: Vec<Option<String>> = scene.actors.iter().map(|a| a.id.clone()).collect();
        scene.actors.push(paste_actor(copy, pointer, &held));
        selected.0 = Some(scene.actors.len() - 1);
    }
}

/// The last path segment of an asset path, which is what a model menu
/// item says.
fn short_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// What a row in the list says: the actor's id when it has one, else its
/// model, since that is what distinguishes two unnamed ones.
fn actor_label(actor: &Actor) -> String {
    match &actor.id {
        Some(id) => id.clone(),
        None => actor.model.clone(),
    }
}

/// A new actor of the given model, standing where the camera looks.
fn new_actor(model: &str, pointer: &GroundPointer) -> Actor {
    let at = pointer.0.unwrap_or(Vec3::ZERO);
    Actor {
        id: None,
        shared: false,
        params: Default::default(),
        model: model.to_owned(),
        position: [at.x, at.z],
        facing: None,
        constrained: true,
        script: None,
    }
}

/// The copy of an actor that paste puts down: beside wherever the camera
/// looks, and with its own identity, so two actors never share one
/// script's memory.
fn paste_actor(copy: Actor, pointer: &GroundPointer, taken: &[Option<String>]) -> Actor {
    let at = pointer.0.unwrap_or(Vec3::ZERO);
    let mut pasted = copy;
    pasted.position = [at.x + PASTE_OFFSET, at.z];
    pasted.id = pasted.id.map(|id| unique_id(&id, taken));
    pasted
}

/// An id no actor in the scene is already using: `goblin`, `goblin.1`, and
/// so on. A duplicate id would leave two actors sharing one script's
/// private storage, which is a confusing bug to chase from a character's
/// line of dialogue.
///
/// The search ends because each step tests a candidate no earlier step did,
/// against a finite set of ids.
fn unique_id(wanted: &str, taken: &[Option<String>]) -> String {
    let held = |id: &str| taken.iter().any(|held| held.as_deref() == Some(id));
    if !held(wanted) {
        return wanted.to_owned();
    }
    for suffix in 1.. {
        let candidate = format!("{wanted}.{suffix}");
        if !held(&candidate) {
            return candidate;
        }
    }
    unreachable!("a candidate is free by construction")
}

/// What the properties window shows for the selected actor, and the two
/// numbers an editor actually moves: where it stands, and which way it
/// faces.
fn properties(ctx: &egui::Context, scene: &mut Scene, selected: &Selected) {
    let Some(index) = selected.0 else {
        return;
    };
    let Some(actor) = scene.actors.get(index) else {
        return;
    };
    let mut position = actor.position;
    let mut facing = actor.facing;
    egui::Window::new(format!("Actor: {}", actor_label(actor)))
        .default_pos(docked(PROPERTIES_TOP))
        .resizable(false)
        .show(ctx, |ui| {
            ui.label(format!("model: {}", actor.model));
            if let Some(script) = &actor.script {
                ui.label(format!("script: {script}"));
            }
            if let Some(id) = &actor.id {
                ui.label(format!("id: {id}"));
            }
            ui.label(format!("constrained: {}", actor.constrained));
            ui.separator();
            egui::Grid::new("placement").num_columns(2).show(ui, |ui| {
                ui.label("x");
                ui.add(egui::DragValue::new(&mut position[0]).speed(0.05));
                ui.end_row();
                ui.label("z");
                ui.add(egui::DragValue::new(&mut position[1]).speed(0.05));
                ui.end_row();
                ui.label("facing");
                // A model with no pinned facing shows the scene camera's,
                // which is a number worth showing and editing: writing it
                // in pins the actor to it.
                let degrees = facing.get_or_insert(0.0);
                ui.add(egui::DragValue::new(degrees).speed(0.5).range(0.0..=360.0));
                ui.end_row();
            });
        });
    // The edits land on the editor's copy, which is what the models are
    // placed from; `sync_actors` writes them onto the standing actors.
    if let Some(actor) = scene.actors.get_mut(index) {
        actor.position = position;
        actor.facing = facing;
    }
}

/// The status line: what the last save did, and which file it did it to.
fn status_line(ctx: &egui::Context, status: &Status, open: &OpenScene) {
    egui::Window::new("Scene")
        .default_pos(docked(STATUS_TOP))
        .resizable(false)
        .collapsible(false)
        .show(ctx, |ui| {
            ui.label(format!("file: {}", open.path));
            if !status.0.is_empty() {
                ui.separator();
                ui.label(&status.0);
            }
        });
}

/// File > Open: every scene file in the folder, click to switch to it.
fn open_dialog(
    ctx: &egui::Context,
    scenes: &Scenes,
    assets: &AssetServer,
    open: &mut OpenScene,
    shown: &mut Shown,
    selected: &mut Selected,
    dialog: &mut Dialog,
) {
    egui::Window::new("Open scene")
        .default_pos(docked(DIALOG_TOP))
        .resizable(false)
        .show(ctx, |ui| {
            if scenes.0.is_empty() {
                ui.label("No scenes found.");
            }
            for path in &scenes.0 {
                if ui
                    .selectable_label(
                        &open.path == path,
                        short_name(path).trim_end_matches(".scene"),
                    )
                    .clicked()
                {
                    open.open(assets, path.clone());
                    shown.forget();
                    *selected = Selected::default();
                    dialog.open = false;
                }
            }
        });
}

/// File > Save as: a name, the scene written under it, and the editor
/// switched to that file so the next save goes to the same place.
#[allow(clippy::too_many_arguments)]
fn save_as_dialog(
    ctx: &egui::Context,
    dialog: &mut Dialog,
    working: &Working,
    status: &mut Status,
    assets: &AssetServer,
    open: &mut OpenScene,
    shown: &mut Shown,
    selected: &mut Selected,
) {
    let ready = valid_scene_name(&dialog.name) && working.0.is_some();
    egui::Window::new("Save scene as")
        .default_pos(docked(DIALOG_TOP))
        .resizable(false)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("name");
                ui.text_edit_singleline(&mut dialog.name);
            });
            if let Some(complaint) = &dialog.complaint {
                ui.colored_label(ui.visuals().warn_fg_color, complaint);
            }
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    dialog.save_as = false;
                }
                if ui.add_enabled(ready, egui::Button::new("Save")).clicked() {
                    let path = format!("scenes/{}.scene", dialog.name.trim());
                    let outcome = match working.0.as_ref() {
                        Some(scene) => save(scene, &path, shown),
                        None => Err(std::io::Error::other("no scene is open")),
                    };
                    match outcome {
                        Ok(()) => {
                            // The editor is now looking at the file it just
                            // wrote, so the next save goes there too.
                            open.open(assets, path.clone());
                            shown.forget();
                            *selected = Selected::default();
                            *status = Status(format!("saved {path}"));
                            dialog.save_as = false;
                        }
                        Err(error) => dialog.complaint = Some(format!("could not save: {error}")),
                    }
                }
            });
        });
}

/// A scene name that can become a file: not empty, and nothing that would
/// send it somewhere other than the scenes folder.
fn valid_scene_name(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty() && !name.contains(['/', '\\']) && name != ".."
}

/// Saves the open scene, recording the result for the status line.
fn save_or_report(scene: Option<&Scene>, path: &str, shown: &mut Shown, status: &mut Status) {
    let outcome = match scene {
        Some(scene) => save(scene, path, shown),
        None => Err(std::io::Error::other("no scene is open")),
    };
    *status = match outcome {
        Ok(()) => Status(format!("saved {path}")),
        Err(error) => Status(format!("could not save {path}: {error}")),
    };
}

/// Writes the scene back to its own file, in the same shape `scene-merge`
/// writes it, so the two never fight over the formatting.
///
/// The editor's own write is recorded, which stops the reload poll from
/// reading the file straight back in and rebuilding everything under the
/// pointer: what was just written is what the editor is already showing.
fn save(scene: &Scene, path: &str, shown: &mut Shown) -> std::io::Result<()> {
    let ron = to_ron(scene).map_err(std::io::Error::other)?;
    let full = assets_root().join(path);
    std::fs::write(&full, ron)?;
    shown.written = std::fs::metadata(&full)
        .and_then(|meta| meta.modified())
        .ok();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actor(id: Option<&str>) -> Actor {
        Actor {
            id: id.map(str::to_owned),
            shared: false,
            params: Default::default(),
            model: "models/goblin.glb".to_owned(),
            position: [1.0, 2.0],
            facing: Some(90.0),
            constrained: true,
            script: Some("scripts/goblin.rhai".to_owned()),
        }
    }

    fn working(actors: Vec<Actor>) -> Working {
        Working(Some(Scene {
            camera: wakeful::scene::CameraPose {
                position: [0.0, 1.0, 2.0],
                target: [0.0, 0.0, 0.0],
                fov_degrees: 45.0,
            },
            walk_mesh: None,
            background: None,
            depth_map: None,
            depth_range: 10.0,
            pan: None,
            teleporters: Vec::new(),
            actors,
            script: None,
        }))
    }

    #[test]
    fn copying_takes_the_selected_actor() {
        let mut copy = working(vec![actor(Some("goblin"))]);
        let mut selected = Selected(Some(0));
        let mut clipboard = Clipboard::default();
        apply_edit(
            Edit::Copy,
            &mut copy,
            &mut selected,
            &mut clipboard,
            &pointer(),
        );
        assert_eq!(
            clipboard.0.as_ref().map(|a| a.id.clone()),
            Some(Some("goblin".to_owned()))
        );
    }

    #[test]
    fn copying_nothing_copies_nothing() {
        let mut copy = working(vec![actor(Some("goblin"))]);
        let mut selected = Selected(None);
        let mut clipboard = Clipboard::default();
        apply_edit(
            Edit::Copy,
            &mut copy,
            &mut selected,
            &mut clipboard,
            &pointer(),
        );
        assert!(clipboard.0.is_none());
    }

    #[test]
    fn pasting_adds_an_actor_after_the_last_and_selects_it() {
        let mut copy = working(vec![actor(Some("goblin"))]);
        let mut selected = Selected(Some(0));
        let mut clipboard = Clipboard(Some(actor(Some("goblin"))));
        apply_edit(
            Edit::Paste,
            &mut copy,
            &mut selected,
            &mut clipboard,
            &pointer(),
        );
        let scene = copy.0.unwrap();
        assert_eq!(scene.actors.len(), 2);
        assert_eq!(selected.0, Some(1));
    }

    #[test]
    fn a_pasted_actor_gets_an_id_of_its_own() {
        // Two actors sharing one id would share one script's memory, and
        // the copy would quietly overwrite the original's.
        let mut copy = working(vec![actor(Some("goblin"))]);
        let mut selected = Selected(Some(0));
        let mut clipboard = Clipboard(Some(actor(Some("goblin"))));
        apply_edit(
            Edit::Paste,
            &mut copy,
            &mut selected,
            &mut clipboard,
            &pointer(),
        );
        let scene = copy.0.unwrap();
        assert_eq!(scene.actors[1].id.as_deref(), Some("goblin.1"));
    }

    #[test]
    fn deleting_takes_the_selected_actor_out() {
        let mut copy = working(vec![actor(Some("a")), actor(Some("b")), actor(Some("c"))]);
        let mut selected = Selected(Some(1));
        let mut clipboard = Clipboard::default();
        apply_edit(
            Edit::Delete,
            &mut copy,
            &mut selected,
            &mut clipboard,
            &pointer(),
        );
        let scene = copy.0.unwrap();
        assert_eq!(scene.actors.len(), 2);
        assert_eq!(scene.actors[1].id.as_deref(), Some("c"));
        // The thing that was selected is the thing that is gone.
        assert_eq!(selected.0, None);
    }

    #[test]
    fn deleting_a_stale_selection_takes_nothing_but_still_selects_nothing() {
        // A selection can outlive its actor: a scene reloaded from disk
        // may be shorter than the list on screen was.
        let mut copy = working(vec![actor(Some("a"))]);
        let mut selected = Selected(Some(7));
        let mut clipboard = Clipboard::default();
        apply_edit(
            Edit::Delete,
            &mut copy,
            &mut selected,
            &mut clipboard,
            &pointer(),
        );
        assert_eq!(copy.0.unwrap().actors.len(), 1);
        assert_eq!(selected.0, None);
    }

    #[test]
    fn deleting_with_nothing_selected_changes_nothing() {
        let mut copy = working(vec![actor(Some("a"))]);
        let mut selected = Selected(None);
        let mut clipboard = Clipboard::default();
        apply_edit(
            Edit::Delete,
            &mut copy,
            &mut selected,
            &mut clipboard,
            &pointer(),
        );
        assert_eq!(copy.0.unwrap().actors.len(), 1);
    }

    #[test]
    fn a_paste_lands_beside_where_the_camera_looks() {
        // A step past the camera's own ground point, so the copy does not
        // land inside the original.
        let pasted = paste_actor(actor(Some("goblin")), &pointer(), &[]);
        assert_eq!(pasted.position, [4.0, 4.0]);
    }

    #[test]
    fn ids_are_made_unique_rather_than_duplicated() {
        let held = vec![Some("goblin".to_owned()), Some("goblin.1".to_owned())];
        assert_eq!(unique_id("goblin", &held), "goblin.2");
        assert_eq!(unique_id("chest", &held), "chest");
    }

    #[test]
    fn a_scene_name_that_would_escape_the_scenes_folder_is_refused() {
        assert!(valid_scene_name("Village_Entrance"));
        assert!(!valid_scene_name(""));
        assert!(!valid_scene_name("   "));
        assert!(!valid_scene_name("../elsewhere"));
        assert!(!valid_scene_name("scenes/nested"));
        assert!(!valid_scene_name(".."));
    }

    /// A camera looking at `(3, ?, 4)`, which is where a paste lands.
    fn pointer() -> GroundPointer {
        GroundPointer(Some(Vec3::new(3.0, 0.0, 4.0)))
    }
}

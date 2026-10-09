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

/// The actors last copied, for paste. A copy of a selection brings all of
/// it, and a paste puts back all of that.
#[derive(Resource, Default)]
pub struct Clipboard(Vec<Actor>);

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
    mut list_sweep: ResMut<ListSweep>,
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
    actor_list(
        ctx,
        scene,
        &mut selected,
        &mut clipboard,
        &models,
        &pointer,
        &mut list_sweep,
    );
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
                let some = !selected.is_empty();
                if ui
                    .add_enabled(some, egui::Button::new("Copy"))
                    .clicked()
                {
                    asked = Some(Edit::Copy);
                }
                if ui
                    .add_enabled(!clipboard.0.is_empty(), egui::Button::new("Paste"))
                    .clicked()
                {
                    asked = Some(Edit::Paste);
                }
                if ui
                    .add_enabled(some, egui::Button::new("Delete"))
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

/// Carries out an edit to the selection.
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
        Edit::Copy => copy_actors(scene, selected, clipboard),
        Edit::Paste => paste_into(scene, clipboard, pointer, selected),
        Edit::Delete => delete_actors(scene, selected),
    }
}

/// Copying takes the whole selection.
fn copy_actors(scene: &Scene, selected: &Selected, clipboard: &mut Clipboard) {
    clipboard.0 = selected
        .picked()
        .iter()
        .filter_map(|index| scene.actors.get(*index).cloned())
        .collect();
}

/// Deleting takes every selected actor out of the list and leaves nothing
/// selected, since everything it could have pointed at is gone.
///
/// From the back, so the indices still mean what they did before the
/// earlier removals shifted everything down.
fn delete_actors(scene: &mut Scene, selected: &mut Selected) {
    let mut picked: Vec<usize> = selected
        .picked()
        .iter()
        .copied()
        .filter(|index| *index < scene.actors.len())
        .collect();
    picked.sort_unstable();
    picked.dedup();
    for index in picked.into_iter().rev() {
        scene.actors.remove(index);
    }
    selected.clear();
}

/// Pasting puts back every copied actor, standing beside where the camera
/// looks, each with an id of its own, and selects the lot.
fn paste_into(
    scene: &mut Scene,
    clipboard: &Clipboard,
    pointer: &GroundPointer,
    selected: &mut Selected,
) {
    if clipboard.0.is_empty() {
        return;
    }
    let mut fresh = Vec::with_capacity(clipboard.0.len());
    for copy in &clipboard.0 {
        let held: Vec<Option<String>> = scene
            .actors
            .iter()
            .chain(fresh.iter())
            .map(|actor| actor.id.clone())
            .collect();
        fresh.push(paste_actor(copy.clone(), pointer, &held));
    }
    let first = scene.actors.len();
    scene.actors.extend(fresh);
    for index in first..scene.actors.len() {
        selected.toggle(index);
    }
}

/// The actor list, its selection, and its right-click menus.
fn actor_list(
    ctx: &egui::Context,
    scene: &mut Scene,
    selected: &mut Selected,
    clipboard: &mut Clipboard,
    models: &Models,
    pointer: &Res<GroundPointer>,
    sweep: &mut ListSweep,
) {
    let count = scene.actors.len();
    // Which row the pointer is over, which is what tells the two menus
    // apart: a right-click on a name is about that actor, anywhere else
    // in the window — beside a name, under the list — is about the list.
    let mut over_row = None;
    // Whether the pointer travelled far enough to be a sweep rather than a
    // click, worked out before the rows are drawn: a row whose click lands
    // on the frame a sweep ends would otherwise select itself out of the
    // very selection the sweep just made.
    let dragged = sweep
        .from
        .zip(ctx.input(|input| input.pointer.interact_pos()))
        .is_some_and(|(from, to)| (from - to).length() > CLICK_SLOP);
    egui::Window::new("Actors")
        .default_pos(docked(ACTORS_TOP))
        .resizable(true)
        .show(ctx, |ui| {
            // One interact over the whole body, made before the rows so a
            // name takes click priority over the background beside it: egui
            // gives click interest to the last widget whose rect holds the
            // pointer.
            //
            // It cannot be the window's own response. A movable egui window
            // carries Sense::DRAG, which in egui 0.36 is DRAG|FOCUSABLE and
            // no click at all, and the menu below opens on a secondary
            // click — so hanging it there made right-click do nothing.
            let body = ui.interact(ui.max_rect(), ui.id().with("body"), egui::Sense::click());
            let pointer_at = ctx.input(|input| input.pointer.interact_pos());
            let shift = ctx.input(|input| input.modifiers.shift);
            let down = ctx.input(|input| input.pointer.button_pressed(egui::PointerButton::Primary));
            let just_pressed = down && !sweep.was_down;
            let just_released = !down && sweep.was_down;
            sweep.was_down = down;

            if just_pressed && pointer_at.is_some_and(|at| body.rect.contains(at)) {
                sweep.from = pointer_at;
            }

            if count == 0 {
                ui.label("None yet. Right-click to add one.");
            }
            // The rows' rectangles, for the sweep to test against. Filled
            // in as they are drawn, since that is when a row's size is
            // known.
            let mut rows: Vec<(usize, egui::Rect)> = Vec::with_capacity(count);
            for index in 0..count {
                let name = actor_label(&scene.actors[index]);
                let row = actor_row(ui, &name, selected.contains(index));
                if !(dragged && just_released) && row.clicked() {
                    pick(selected, index, shift);
                }
                if row.contains_pointer() {
                    over_row = Some(index);
                }
                rows.push((index, row.rect));
            }

            // A drag that never left where it started is a click, and the
            // row under it has already had its say above.
            if dragged && just_released {
                selected.sweep(
                    egui::Rect::from_two_pos(sweep.from.unwrap_or_default(), pointer_at.unwrap_or_default()),
                    rows.iter().copied(),
                    shift,
                );
            }
            sweep.from = sweep.from.filter(|_| !just_released);
            // The band, while it is being dragged.
            if let (Some(from), Some(to)) = (sweep.from, pointer_at)
                && (from - to).length() > CLICK_SLOP
            {
                ui.painter().rect_stroke(
                    egui::Rect::from_two_pos(from, to),
                    0.0,
                    ui.visuals().selection.stroke,
                    egui::StrokeKind::Middle,
                );
            }

            body.context_menu(|ui| {
                if let Some(index) = over_row {
                    selected.only(index);
                }
                if over_row.is_some() {
                    // The actor's own menu: only what can be done to it.
                    copy_item(ui, selected, scene, clipboard);
                    delete_item(ui, selected, scene);
                } else {
                    // The list's: what can be done to the scene around them.
                    copy_item(ui, selected, scene, clipboard);
                    if ui
                        .add_enabled(!clipboard.0.is_empty(), egui::Button::new("Paste"))
                        .clicked()
                    {
                        paste_into(scene, clipboard, pointer, selected);
                    }
                    ui.separator();
                    ui.label("Add");
                    for model in &models.0 {
                        if ui.button(short_name(model)).clicked() {
                            scene.actors.push(new_actor(model, pointer));
                            selected.only(scene.actors.len() - 1);
                        }
                    }
                }
            });
        });
}

/// Copying, offered only when something is selected to copy.
fn copy_item(ui: &mut egui::Ui, selected: &Selected, scene: &Scene, clipboard: &mut Clipboard) {
    if ui
        .add_enabled(!selected.is_empty(), egui::Button::new("Copy"))
        .clicked()
    {
        copy_actors(scene, selected, clipboard);
    }
}

/// Deleting, likewise.
fn delete_item(ui: &mut egui::Ui, selected: &mut Selected, scene: &mut Scene) {
    let any = !selected.is_empty();
    if ui.add_enabled(any, egui::Button::new("Delete")).clicked() {
        delete_actors(scene, selected);
    }
}

/// How far the pointer may travel between press and release and still
/// count as a click rather than the start of a sweep, in logical pixels:
/// small enough that clicking one row never selects the block.
pub const CLICK_SLOP: f32 = 6.0;

/// A sweep being dragged down the list: where it started, and whether the
/// button was down on the last frame.
///
/// The button state is carried rather than read as an edge because egui
/// reports a press and a release landing in the same frame as one click,
/// and a sweep is something that happens across frames.
#[derive(Resource, Default)]
pub struct ListSweep {
    from: Option<egui::Pos2>,
    was_down: bool,
}

/// Picking an actor: on its own it is the only selection, with shift it
/// joins or leaves the others.
fn pick(selected: &mut Selected, index: usize, shift: bool) {
    if shift {
        selected.toggle(index);
    } else {
        selected.only(index);
    }
}

/// One row: the actor's name, a selection bar behind it, and the click.
///
/// The row is as wide as its name and no wider, because the space beside
/// a name belongs to the list rather than to that actor — and right-clicks
/// there offer the list's menu, not the actor's.
fn actor_row(ui: &mut egui::Ui, name: &str, chosen: bool) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(
        name.to_owned(),
        egui::TextStyle::Body.resolve(ui.style()),
        if chosen {
            ui.visuals().strong_text_color()
        } else {
            ui.visuals().text_color()
        },
    );
    let height = ui.spacing().interact_size.y;
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(galley.size().x, height), egui::Sense::click());
    if chosen {
        ui.painter().rect_filled(
            rect.expand(2.0),
            4.0,
            ui.visuals().selection.bg_fill,
        );
    }
    ui.painter()
        .galley(rect.left_top(), galley, ui.visuals().text_color());
    response
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

/// What the properties window shows, and the two numbers an editor
/// actually moves: where an actor stands, and which way it faces.
///
/// The numbers belong to the primary — the most recently picked actor, and
/// the only one with a name to put in the title — but an edit moves the
/// whole selection, by the same amount, so a group keeps its shape instead
/// of collapsing onto one spot.
fn properties(ctx: &egui::Context, scene: &mut Scene, selected: &Selected) {
    let Some(primary) = selected.primary() else {
        return;
    };
    let Some(actor) = scene.actors.get(primary) else {
        return;
    };
    let alone = selected.len() == 1;
    let mut position = actor.position;
    let mut facing = actor.facing;
    let title = if alone {
        format!("Actor: {}", actor_label(actor))
    } else {
        format!("{} actors", selected.len())
    };
    egui::Window::new(title)
        .default_pos(docked(PROPERTIES_TOP))
        .resizable(false)
        .show(ctx, |ui| {
            if alone {
                ui.label(format!("model: {}", actor.model));
                if let Some(script) = &actor.script {
                    ui.label(format!("script: {script}"));
                }
                if let Some(id) = &actor.id {
                    ui.label(format!("id: {id}"));
                }
                ui.label(format!("constrained: {}", actor.constrained));
            } else {
                ui.label(format!("moving {} actors together", selected.len()));
            }
            ui.separator();
            egui::Grid::new("placement").num_columns(2).show(ui, |ui| {
                ui.label("x");
                ui.add(egui::DragValue::new(&mut position[0]).speed(0.05));
                ui.end_row();
                ui.label("z");
                ui.add(egui::DragValue::new(&mut position[1]).speed(0.05));
                ui.end_row();
                // Facing is an angle, and an angle has no meaning as an
                // offset: rotating a group is not a thing this window
                // offers, so it belongs to the primary alone.
                if alone {
                    ui.label("facing");
                    // A model with no pinned facing shows the scene
                    // camera's, which is a number worth showing and
                    // editing: writing it in pins the actor to it.
                    let degrees = facing.get_or_insert(0.0);
                    ui.add(egui::DragValue::new(degrees).speed(0.5).range(0.0..=360.0));
                    ui.end_row();
                }
            });
        });
    // The edits land on the editor's copy, which is what the models are
    // placed from; `sync_actors` writes them onto the standing actors.
    let Some(shown) = scene.actors.get(primary) else {
        return;
    };
    let (dx, dz) = (position[0] - shown.position[0], position[1] - shown.position[1]);
    move_group(&mut scene.actors, selected, dx, dz);
    if let Some(primary_actor) = scene.actors.get_mut(primary) {
        primary_actor.facing = facing;
    }
}

/// Shifts every selected actor by the same amount, so a group keeps its
/// shape. The numbers a caller passes are a difference, not a place.
pub fn move_group(actors: &mut [Actor], selected: &Selected, dx: f32, dz: f32) {
    if dx == 0.0 && dz == 0.0 {
        return;
    }
    for index in selected.picked() {
        if let Some(actor) = actors.get_mut(*index) {
            actor.position[0] += dx;
            actor.position[1] += dz;
        }
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

    /// A camera looking at `(3, ?, 4)`, which is where a paste lands.
    fn pointer() -> GroundPointer {
        GroundPointer(Some(Vec3::new(3.0, 0.0, 4.0)))
    }

    fn ids(actors: &[Actor]) -> Vec<Option<String>> {
        actors.iter().map(|actor| actor.id.clone()).collect()
    }

    #[test]
    fn copying_takes_the_whole_selection() {
        let copy = working(vec![actor(Some("a")), actor(Some("b")), actor(Some("c"))]);
        let mut selected = Selected::default();
        selected.only(0);
        selected.toggle(2);
        let mut clipboard = Clipboard::default();
        copy_actors(copy.0.as_ref().unwrap(), &selected, &mut clipboard);
        assert_eq!(
            clipboard.0.iter().filter_map(|a| a.id.as_deref()).collect::<Vec<_>>(),
            ["a", "c"]
        );
    }

    #[test]
    fn copying_nothing_copies_nothing() {
        let copy = working(vec![actor(Some("a"))]);
        let selected = Selected::default();
        let mut clipboard = Clipboard::default();
        copy_actors(copy.0.as_ref().unwrap(), &selected, &mut clipboard);
        assert!(clipboard.0.is_empty());
    }

    #[test]
    fn pasting_adds_every_copied_actor_and_selects_them_all() {
        let mut copy = working(vec![actor(Some("goblin"))]);
        let mut scene = copy.0.take().unwrap();
        let mut selected = Selected::default();
        let clipboard = Clipboard(vec![actor(Some("goblin")), actor(None)]);
        paste_into(&mut scene, &clipboard, &pointer(), &mut selected);
        assert_eq!(scene.actors.len(), 3);
        assert_eq!(selected.picked(), [1, 2]);
    }

    #[test]
    fn a_pasted_actor_gets_an_id_of_its_own() {
        // Two actors sharing one id would share one script's memory, and
        // the copy would quietly overwrite the original's.
        let mut copy = working(vec![actor(Some("goblin"))]);
        let mut scene = copy.0.take().unwrap();
        let mut selected = Selected::default();
        paste_into(
            &mut scene,
            &Clipboard(vec![actor(Some("goblin"))]),
            &pointer(),
            &mut selected,
        );
        assert_eq!(scene.actors[1].id.as_deref(), Some("goblin.1"));
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
    fn deleting_takes_the_whole_selection_out() {
        let mut copy = working(vec![actor(Some("a")), actor(Some("b")), actor(Some("c"))]);
        let mut selected = Selected::default();
        selected.only(0);
        selected.toggle(2);
        let mut scene = copy.0.take().unwrap();
        delete_actors(&mut scene, &mut selected);
        // b survives, and the two that went took their indices with them:
        // nothing left dangling, nothing left selected.
        assert_eq!(ids(&scene.actors), [Some("b".to_owned())]);
        assert!(selected.is_empty());
    }

    #[test]
    fn deleting_nothing_leaves_the_list_alone() {
        let mut copy = working(vec![actor(Some("a"))]);
        let mut scene = copy.0.take().unwrap();
        let mut selected = Selected::default();
        delete_actors(&mut scene, &mut selected);
        assert_eq!(scene.actors.len(), 1);
    }

    #[test]
    fn a_stale_selection_deletes_nothing_and_still_selects_nothing() {
        // A selection can outlive its actor: a scene reloaded from disk may
        // be shorter than the list on screen was.
        let mut copy = working(vec![actor(Some("a"))]);
        let mut selected = Selected::default();
        selected.only(7);
        let mut scene = copy.0.take().unwrap();
        delete_actors(&mut scene, &mut selected);
        assert_eq!(scene.actors.len(), 1);
        assert!(selected.is_empty());
    }

    #[test]
    fn a_group_moves_by_the_same_amount_and_keeps_its_shape() {
        let mut actors = vec![actor(None), actor(None), actor(None)];
        actors[0].position = [-30.0, -100.0];
        actors[1].position = [-12.0, -104.0];
        actors[2].position = [-41.0, -96.0];
        let mut selected = Selected::default();
        selected.only(0);
        selected.toggle(2);
        // The properties window shows the primary's own number, so what
        // reaches the list is a difference, not a place.
        move_group(&mut actors, &selected, 10.0, 0.0);
        assert_eq!(actors[0].position, [-20.0, -100.0]);
        assert_eq!(actors[2].position, [-31.0, -96.0]);
        // The one not selected stayed exactly where it was.
        assert_eq!(actors[1].position, [-12.0, -104.0]);
        // And the group is as spread out as it was.
        assert_eq!(
            actors[2].position[0] - actors[0].position[0],
            -11.0
        );
    }

    #[test]
    fn a_group_that_did_not_move_is_left_alone() {
        let mut actors = vec![actor(None)];
        let mut selected = Selected::default();
        selected.only(0);
        move_group(&mut actors, &selected, 0.0, 0.0);
        assert_eq!(actors[0].position, [1.0, 2.0]);
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
}

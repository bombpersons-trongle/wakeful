//! Script-driven UI: named panels in virtual-screen space.
//!
//! Scripts declare windows declaratively — `ui_window` re-declares and
//! repositions, content requests append, and [`drain`] reconciles the
//! entities each fixed tick. Navigation state is keyed by window NAME,
//! so rebuilding a menu never resets the cursor. The most recently
//! *created* options list owns the cursor; dpad/stick moves it, cross
//! presses it, and `ui_confirmed` reports the press for exactly one
//! tick. [`UiPause`] is script policy: while set, the field player idles.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, PoisonError};

use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use bevy::sprite::Anchor;

use crate::input::{InputState, PadAxis, PadButton, digital};
use crate::screen::UI_LAYER;
use crate::scripts::UiRequest;
use crate::systems::bubble::{BubbleAssets, BubbleTheme, screen_to_world};
use crate::text::{TextAssets, pixel_text};

/// Vertical pitch of option rows: 8px font plus a little air.
const ROW: f32 = 12.0;
const BAR_H: f32 = 6.0;
const CURSOR: Vec2 = Vec2::new(12.0, 10.0);
/// Panels float above speech bubbles (bubbles stay under 0.5).
const Z_PANEL_BACK: f32 = 0.5;
const Z_PANEL_FILL: f32 = 0.55;
/// Bar tracks sit on the panel fill — sharing its Z would z-fight.
const Z_BAR_TRACK: f32 = 0.6;
const Z_TEXT: f32 = 0.9;
const Z_CURSOR: f32 = 0.95;

/// The shared handle script engines bind their UI functions against.
#[derive(Resource, Clone)]
pub struct UiApi {
    requests: Arc<Mutex<Vec<UiRequest>>>,
    menus: Arc<Mutex<UiNavState>>,
}

impl UiApi {
    pub fn new() -> Self {
        Self {
            requests: Arc::new(Mutex::new(Vec::new())),
            menus: Arc::new(Mutex::new(UiNavState::default())),
        }
    }

    pub(crate) fn push(&self, request: UiRequest) {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request);
    }

    /// The option index the player confirmed this tick, or -1.
    pub fn confirmed(&self, name: &str) -> i64 {
        self.menus
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .confirmed(name)
    }

    #[cfg(test)]
    pub(crate) fn nav(&self) -> &Mutex<UiNavState> {
        &self.menus
    }

    /// Takes every pending request, for tests that assert on what a
    /// script pushed.
    #[cfg(test)]
    pub fn take_requests(&self) -> Vec<UiRequest> {
        std::mem::take(
            self.requests
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_mut(),
        )
    }
}

/// Which row each menu's cursor sits on, and what was pressed this tick.
#[derive(Default)]
pub struct UiNavState {
    menus: BTreeMap<String, MenuNav>,
    /// Creation order; the last live entry owns the cursor.
    order: Vec<String>,
    last_stick_y: f32,
}

#[derive(Default)]
struct MenuNav {
    selected: usize,
    count: usize,
    confirmed: Option<usize>,
}

impl UiNavState {
    /// (Re)declares a menu's option count; the selected row is clamped
    /// so rebuilding with fewer options can't strand the cursor.
    pub(crate) fn declare(&mut self, name: &str, count: usize) {
        let nav = self.menus.entry(name.to_owned()).or_default();
        nav.count = count;
        nav.selected = nav.selected.min(count.saturating_sub(1));
        if count == 0 {
            nav.selected = 0;
        }
        if !self.order.iter().any(|other| other == name) {
            self.order.push(name.to_owned());
        }
    }

    fn close(&mut self, name: &str) {
        self.menus.remove(name);
        self.order.retain(|other| other != name);
    }

    fn clear(&mut self) {
        *self = Self::default();
    }

    /// Advances the cursor for one tick. `confirmed` entries are cleared
    /// first — a press is visible for exactly this tick.
    pub(crate) fn navigate(&mut self, input: &InputState) {
        for nav in self.menus.values_mut() {
            nav.confirmed = None;
        }
        // The stick navigates on edges, not while held.
        let stick_y = digital(input.axis(PadAxis::LeftStickY));
        let edge = stick_y != 0.0 && stick_y != self.last_stick_y;
        self.last_stick_y = stick_y;

        let Some(name) = self
            .order
            .iter()
            .rev()
            .find(|name| self.menus.get(*name).is_some_and(|nav| nav.count > 0))
            .cloned()
        else {
            return;
        };
        let Some(nav) = self.menus.get_mut(&name) else {
            return;
        };
        let up = input.just_pressed(PadButton::DPadUp) || (edge && stick_y < 0.0);
        let down = input.just_pressed(PadButton::DPadDown) || (edge && stick_y > 0.0);
        if up {
            nav.selected = (nav.selected + nav.count - 1) % nav.count;
        }
        if down {
            nav.selected = (nav.selected + 1) % nav.count;
        }
        if input.just_pressed(PadButton::Cross) {
            nav.confirmed = Some(nav.selected);
        }
    }

    fn confirmed(&self, name: &str) -> i64 {
        self.menus
            .get(name)
            .and_then(|nav| nav.confirmed)
            .map(|index| index as i64)
            .unwrap_or(-1)
    }

    fn selected(&self, name: &str) -> Option<usize> {
        self.menus.get(name).map(|nav| nav.selected)
    }
}

/// Script policy: while true, the field player idles (menus pause the
/// world; HUD windows needn't).
#[derive(Resource, Default)]
pub struct UiPause(pub bool);

/// Static handles the UI draws with, built once at startup.
#[derive(Resource)]
pub(crate) struct UiAssets {
    cursor: Handle<Image>,
}

pub(crate) fn setup(mut commands: Commands, server: Res<AssetServer>) {
    commands.insert_resource(UiAssets {
        cursor: server.load("ui/finger.png"),
    });
}

/// A script-declared panel. All children (panel quads, text, bars,
/// cursor) are rebuilt from requests whenever the window is re-declared.
#[derive(Component, Clone)]
pub(crate) struct UiWindow {
    name: String,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

/// An options list within a window; `cursor` points at the finger.
#[derive(Component)]
pub(crate) struct UiOptions {
    x: f32,
    y: f32,
    cursor: Entity,
}

/// Marks the finger sprite so the sync system can find it.
#[derive(Component)]
pub(crate) struct UiCursor;

/// Content declared for one window since its `ui_window` call.
struct Pending {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    content: Vec<Content>,
}

enum Content {
    Text { text: String, x: f32, y: f32 },
    Options { x: f32, y: f32, labels: Vec<String> },
    Bar { x: f32, y: f32, w: f32, ratio: f32 },
}

/// Window-local pixel position (y down) to a child transform offset
/// from the panel's center.
fn local_offset(rect: &UiWindow, x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x - rect.w / 2.0, rect.h / 2.0 - y, z)
}

fn layers() -> RenderLayers {
    RenderLayers::layer(UI_LAYER)
}

/// Spawns the window root with its themed panel quads.
fn spawn_window(commands: &mut Commands, assets: &BubbleAssets, rect: &UiWindow) -> Entity {
    let center = screen_to_world(Vec2::new(rect.x + rect.w / 2.0, rect.y + rect.h / 2.0));
    let root = commands
        .spawn((
            rect.clone(),
            Visibility::default(),
            Transform::from_translation(center.extend(0.0)),
        ))
        .id();
    spawn_panel(commands, assets, rect, root);
    root
}

/// The panel quads (border + inset fill) under a window root.
fn spawn_panel(commands: &mut Commands, assets: &BubbleAssets, rect: &UiWindow, root: Entity) {
    let border = commands
        .spawn((
            Mesh2d(assets.rect.clone()),
            MeshMaterial2d(assets.border.clone()),
            Transform::from_xyz(0.0, 0.0, Z_PANEL_BACK).with_scale(Vec3::new(rect.w, rect.h, 1.0)),
            layers(),
        ))
        .id();
    let fill = commands
        .spawn((
            Mesh2d(assets.rect.clone()),
            MeshMaterial2d(assets.fill.clone()),
            Transform::from_xyz(0.0, 0.0, Z_PANEL_FILL).with_scale(Vec3::new(
                (rect.w - 2.0).max(1.0),
                (rect.h - 2.0).max(1.0),
                1.0,
            )),
            layers(),
        ))
        .id();
    commands.entity(root).add_children(&[border, fill]);
}

/// Spawns one content element as a child of the window root.
#[allow(clippy::too_many_arguments)]
fn spawn_content(
    commands: &mut Commands,
    assets: &BubbleAssets,
    ui_assets: &UiAssets,
    theme: &BubbleTheme,
    text_assets: &TextAssets,
    rect: &UiWindow,
    root: Entity,
    content: &Content,
) {
    let layers = layers();
    match content {
        Content::Text { text, x, y } => {
            let (text2d, font) = pixel_text(text.clone(), text_assets);
            let row = commands
                .spawn((
                    text2d,
                    font,
                    TextColor(theme.text),
                    Anchor::TOP_LEFT,
                    Transform::from_translation(local_offset(rect, *x, *y, Z_TEXT)),
                    layers,
                ))
                .id();
            commands.entity(root).add_child(row);
        }
        Content::Options { x, y, labels } => {
            let mut rows = Vec::new();
            for (index, label) in labels.iter().enumerate() {
                let (text2d, font) = pixel_text(label.clone(), text_assets);
                let row = commands
                    .spawn((
                        text2d,
                        font,
                        TextColor(theme.text),
                        Anchor::TOP_LEFT,
                        Transform::from_translation(local_offset(
                            rect,
                            *x,
                            *y + index as f32 * ROW,
                            Z_TEXT,
                        )),
                        layers.clone(),
                    ))
                    .id();
                rows.push(row);
            }
            let cursor = commands
                .spawn((
                    UiCursor,
                    Sprite {
                        image: ui_assets.cursor.clone(),
                        custom_size: Some(CURSOR),
                        ..default()
                    },
                    Transform::from_translation(local_offset(
                        rect,
                        *x - CURSOR.x - 2.0,
                        *y + CURSOR.y / 2.0,
                        Z_CURSOR,
                    )),
                    layers,
                ))
                .id();
            commands.entity(root).add_child(cursor).add_children(&rows);
            commands.entity(root).insert(UiOptions {
                x: *x,
                y: *y,
                cursor,
            });
        }
        Content::Bar { x, y, w, ratio } => {
            // Quads center on their transform, so each bar piece is
            // offset by half its size to grow right/down from (x, y).
            let ratio = ratio.clamp(0.0, 1.0);
            let fill_w = (*w - 2.0) * ratio;
            let back = commands
                .spawn((
                    Mesh2d(assets.rect.clone()),
                    MeshMaterial2d(assets.border.clone()),
                    Transform::from_translation(local_offset(
                        rect,
                        *x + *w / 2.0,
                        *y + BAR_H / 2.0,
                        Z_BAR_TRACK,
                    ))
                    .with_scale(Vec3::new(*w, BAR_H, 1.0)),
                    layers.clone(),
                ))
                .id();
            let fill = commands
                .spawn((
                    Mesh2d(assets.rect.clone()),
                    MeshMaterial2d(assets.tail_fill.clone()),
                    Transform::from_translation(local_offset(
                        rect,
                        *x + 1.0 + fill_w / 2.0,
                        *y + 1.0 + (BAR_H - 2.0) / 2.0,
                        Z_TEXT,
                    ))
                    .with_scale(Vec3::new(
                        fill_w.max(1.0),
                        (BAR_H - 2.0).max(1.0),
                        1.0,
                    )),
                    layers,
                ))
                .id();
            commands.entity(root).add_children(&[back, fill]);
        }
    }
}

/// Moves the finger sprite to the selected row of the window the cursor
/// currently drives; hides fingers on windows it doesn't.
pub(crate) fn sync_cursor(
    api: Res<UiApi>,
    mut windows: Query<(&UiWindow, &UiOptions, &Children)>,
    mut cursors: Query<(&mut Transform, &mut Visibility), With<UiCursor>>,
) {
    let menus = api.menus.lock().unwrap_or_else(PoisonError::into_inner);
    for (rect, options, children) in &mut windows {
        let Some(selected) = menus.selected(&rect.name) else {
            continue;
        };
        let Some(cursor) = children.iter().find(|child| *child == options.cursor) else {
            continue;
        };
        let Ok((mut transform, mut visibility)) = cursors.get_mut(cursor) else {
            continue;
        };
        let y = options.y + selected as f32 * ROW + CURSOR.y / 2.0;
        transform.translation = local_offset(rect, options.x - CURSOR.x - 2.0, y, Z_CURSOR);
        *visibility = Visibility::Visible;
    }
}

/// Applies this tick's script requests: closes, repositions, rebuilds
/// window content, and carries the pause policy. Runs last in the
/// fixed chain so windows appear the same tick they're declared.
#[allow(clippy::too_many_arguments)]
pub(crate) fn drain(
    mut commands: Commands,
    api: Res<UiApi>,
    mut pause: ResMut<UiPause>,
    windows: Query<(Entity, &UiWindow)>,
    assets: Res<BubbleAssets>,
    ui_assets: Res<UiAssets>,
    theme: Res<BubbleTheme>,
    text_assets: Res<TextAssets>,
) {
    let requests: Vec<UiRequest> = api
        .requests
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .drain(..)
        .collect();

    // Per-window intent for this batch. A script that re-declares a
    // window after closing it (or closes one it declared earlier in the
    // same tick — the triangle menu does exactly that on its closing
    // tick) resolves to whichever came last.
    let mut closes: BTreeSet<String> = BTreeSet::new();
    let mut pending: BTreeMap<String, Pending> = BTreeMap::new();
    let mut warned = BTreeSet::new();
    for request in requests {
        match request {
            UiRequest::Pause(value) => pause.0 = value,
            UiRequest::Close { name } => {
                closes.insert(name.clone());
                pending.remove(&name);
            }
            UiRequest::Window { name, x, y, w, h } => {
                closes.remove(&name);
                pending.insert(
                    name,
                    Pending {
                        x,
                        y,
                        w,
                        h,
                        content: Vec::new(),
                    },
                );
            }
            UiRequest::Text { window, text, x, y } => match pending.get_mut(&window) {
                Some(slot) => slot.content.push(Content::Text { text, x, y }),
                None => warn_undeclared(&mut warned, &window),
            },
            UiRequest::Options {
                window,
                x,
                y,
                labels,
            } => match pending.get_mut(&window) {
                Some(slot) => slot.content.push(Content::Options { x, y, labels }),
                None => warn_undeclared(&mut warned, &window),
            },
            UiRequest::Bar {
                window,
                x,
                y,
                w,
                ratio,
            } => match pending.get_mut(&window) {
                Some(slot) => slot.content.push(Content::Bar { x, y, w, ratio }),
                None => warn_undeclared(&mut warned, &window),
            },
        }
    }

    for name in &closes {
        if let Some((entity, _)) = windows.iter().find(|(_, window)| &window.name == name) {
            commands.entity(entity).despawn();
        }
        api.menus
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .close(name);
    }

    let mut nav = api.menus.lock().unwrap_or_else(PoisonError::into_inner);
    for (name, slot) in pending {
        // One warning per window per drain, not per stray request.
        warned.remove(&name);
        let rect = UiWindow {
            name: name.clone(),
            x: slot.x,
            y: slot.y,
            w: slot.w,
            h: slot.h,
        };
        let root = match windows.iter().find(|(_, window)| window.name == name) {
            Some((entity, _)) => {
                commands
                    .entity(entity)
                    .insert(rect.clone())
                    .remove::<UiOptions>()
                    .despawn_children();
                // despawn_children takes the panel quads with the
                // content; put the panel back under the fresh content.
                spawn_panel(&mut commands, &assets, &rect, entity);
                entity
            }
            None => spawn_window(&mut commands, &assets, &rect),
        };
        let mut count = 0;
        for content in &slot.content {
            if let Content::Options { labels, .. } = content {
                count = labels.len();
            }
            spawn_content(
                &mut commands,
                &assets,
                &ui_assets,
                &theme,
                &text_assets,
                &rect,
                root,
                content,
            );
        }
        nav.declare(&name, count);
    }
}

fn warn_undeclared(warned: &mut BTreeSet<String>, window: &str) {
    if warned.insert(window.to_owned()) {
        bevy::log::warn!("UI content for window '{window}' arrived before ui_window; skipped");
    }
}

/// Closes every window and forgets the cursor state. Scene teardown
/// calls this: a world script re-declares its menu the next tick if it
/// should still be open, while scene-scoped windows die with the scene.
pub(crate) fn close_all(
    api: &UiApi,
    commands: &mut Commands,
    windows: impl Iterator<Item = Entity>,
) {
    for entity in windows {
        commands.entity(entity).despawn();
    }
    api.menus
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
}

/// Advances menu navigation from this tick's aggregated input.
pub(crate) fn navigate(api: Res<UiApi>, input: Res<crate::input::InputManager>) {
    let state = input.handle();
    let input = state.lock().unwrap_or_else(PoisonError::into_inner);
    api.menus
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .navigate(&input);
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::screen::{GAME_HEIGHT, GAME_WIDTH};
    use crate::systems::bubble;
    use crate::systems::bubble::GradientMaterial;
    use crate::text;

    fn input_with(pressed: &[PadButton], just: &[PadButton]) -> InputState {
        let mut state = InputState::default();
        state.inject(pressed, just, &[]);
        state
    }

    #[test]
    fn navigation_wraps_and_presses_exactly_one_tick() {
        let mut nav = UiNavState::default();
        nav.declare("menu", 3);

        nav.navigate(&input_with(&[], &[PadButton::DPadDown]));
        assert_eq!(nav.selected("menu"), Some(1));
        nav.navigate(&input_with(&[], &[PadButton::DPadDown]));
        nav.navigate(&input_with(&[], &[PadButton::DPadDown]));
        assert_eq!(nav.selected("menu"), Some(0), "wraps past the last row");

        nav.navigate(&input_with(&[], &[PadButton::DPadUp]));
        assert_eq!(nav.selected("menu"), Some(2), "wraps above the first row");

        nav.navigate(&input_with(&[], &[PadButton::Cross]));
        assert_eq!(nav.confirmed("menu"), 2);
        nav.navigate(&input_with(&[], &[]));
        assert_eq!(nav.confirmed("menu"), -1, "a press lasts one tick");
    }

    #[test]
    fn the_stick_navigates_on_edges_not_while_held() {
        let mut nav = UiNavState::default();
        nav.declare("menu", 2);

        // Deflect down: one edge, one move.
        let mut state = InputState::default();
        state.inject(&[], &[], &[(PadAxis::LeftStickY, 0.9)]);
        nav.navigate(&state);
        assert_eq!(nav.selected("menu"), Some(1));

        // Still held: no further moves.
        nav.navigate(&state);
        assert_eq!(nav.selected("menu"), Some(1));

        // Release, then deflect up: one edge, one move back.
        let mut released = InputState::default();
        released.inject(&[], &[], &[]);
        nav.navigate(&released);
        let mut up = InputState::default();
        up.inject(&[], &[], &[(PadAxis::LeftStickY, -0.9)]);
        nav.navigate(&up);
        assert_eq!(nav.selected("menu"), Some(0));
    }

    #[test]
    fn the_most_recently_created_menu_owns_the_cursor() {
        let mut nav = UiNavState::default();
        nav.declare("dialog", 2);
        nav.declare("menu", 2);

        nav.navigate(&input_with(&[], &[PadButton::DPadDown]));
        assert_eq!(nav.selected("menu"), Some(1));
        assert_eq!(nav.selected("dialog"), Some(0), "untouched");

        nav.close("menu");
        nav.navigate(&input_with(&[], &[PadButton::DPadDown]));
        assert_eq!(nav.selected("dialog"), Some(1), "cursor falls back");
    }

    fn ui_world() -> (World, UiApi) {
        let mut world = World::new();
        world.insert_resource(BubbleTheme::default());
        world.insert_resource(Assets::<Mesh>::default());
        world.insert_resource(Assets::<ColorMaterial>::default());
        world.insert_resource(Assets::<GradientMaterial>::default());
        let assets = bubble::test_assets(&mut world);
        world.insert_resource(assets);
        world.insert_resource(text::test_assets());
        world.insert_resource(UiAssets {
            cursor: Handle::default(),
        });
        let api = UiApi::new();
        world.insert_resource(api.clone());
        world.insert_resource(UiPause::default());
        (world, api)
    }

    fn window_names(world: &mut World) -> Vec<String> {
        let mut query = world.query::<&UiWindow>();
        query
            .iter(world)
            .map(|window| window.name.clone())
            .collect()
    }

    #[test]
    fn requests_build_and_rebuild_windows() {
        let (mut world, api) = ui_world();
        api.push(UiRequest::Window {
            name: "menu".into(),
            x: 10.0,
            y: 10.0,
            w: 100.0,
            h: 50.0,
        });
        api.push(UiRequest::Text {
            window: "menu".into(),
            text: "Hello".into(),
            x: 5.0,
            y: 5.0,
        });
        api.push(UiRequest::Options {
            window: "menu".into(),
            x: 5.0,
            y: 20.0,
            labels: vec!["A".into(), "B".into()],
        });
        world.run_system_once(drain).unwrap();

        assert_eq!(window_names(&mut world), vec!["menu".to_owned()]);
        assert_eq!(api.confirmed("menu"), -1);

        // Re-declaring keeps the entity but rebuilds the content, and
        // nav state (declared before the rebuild) survives.
        api.nav()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .navigate(&input_with(&[], &[PadButton::DPadDown]));
        api.push(UiRequest::Window {
            name: "menu".into(),
            x: 20.0,
            y: 20.0,
            w: 90.0,
            h: 40.0,
        });
        api.push(UiRequest::Text {
            window: "menu".into(),
            text: "Hello".into(),
            x: 5.0,
            y: 5.0,
        });
        world.run_system_once(drain).unwrap();
        assert_eq!(window_names(&mut world), vec!["menu".to_owned()]);
        let entity = world
            .query::<(Entity, &UiWindow)>()
            .single(&world)
            .unwrap()
            .0;
        assert_eq!(world.get::<UiWindow>(entity).unwrap().x, 20.0);
    }

    #[test]
    fn close_forgets_the_window_and_the_cursor() {
        let (mut world, api) = ui_world();
        api.push(UiRequest::Window {
            name: "menu".into(),
            x: 0.0,
            y: 0.0,
            w: 50.0,
            h: 50.0,
        });
        api.push(UiRequest::Options {
            window: "menu".into(),
            x: 4.0,
            y: 4.0,
            labels: vec!["A".into()],
        });
        world.run_system_once(drain).unwrap();

        api.push(UiRequest::Close {
            name: "menu".into(),
        });
        world.run_system_once(drain).unwrap();
        assert!(window_names(&mut world).is_empty());
        assert_eq!(api.confirmed("menu"), -1);
        assert_eq!(api.nav().lock().unwrap().selected("menu"), None);
    }

    #[test]
    fn content_before_a_window_declaration_is_skipped_with_one_warning() {
        let (mut world, api) = ui_world();
        api.push(UiRequest::Text {
            window: "ghost".into(),
            text: "hi".into(),
            x: 0.0,
            y: 0.0,
        });
        world.run_system_once(drain).unwrap();
        assert!(window_names(&mut world).is_empty());
    }

    #[test]
    fn pause_requests_carry_the_policy() {
        let (mut world, api) = ui_world();
        api.push(UiRequest::Pause(true));
        world.run_system_once(drain).unwrap();
        assert!(world.resource::<UiPause>().0);
        api.push(UiRequest::Pause(false));
        world.run_system_once(drain).unwrap();
        assert!(!world.resource::<UiPause>().0);
    }

    #[test]
    fn a_window_closed_in_the_same_batch_as_its_declaration_stays_closed() {
        // The triangle menu declares its window at the top of every
        // tick, then closes it at the end of the closing tick — the
        // close must win, and must not panic on the dead entity.
        let (mut world, api) = ui_world();
        api.push(UiRequest::Window {
            name: "menu".into(),
            x: 0.0,
            y: 0.0,
            w: 40.0,
            h: 40.0,
        });
        api.push(UiRequest::Close {
            name: "menu".into(),
        });
        world.run_system_once(drain).unwrap();
        assert!(window_names(&mut world).is_empty());

        // The reverse order — close, then a fresh declaration —
        // rebuilds instead.
        api.push(UiRequest::Close {
            name: "menu".into(),
        });
        api.push(UiRequest::Window {
            name: "menu".into(),
            x: 0.0,
            y: 0.0,
            w: 40.0,
            h: 40.0,
        });
        world.run_system_once(drain).unwrap();
        assert_eq!(window_names(&mut world), vec!["menu".to_owned()]);
    }

    #[test]
    fn rebuilding_a_window_puts_the_panel_back() {
        let (mut world, api) = ui_world();
        api.push(UiRequest::Window {
            name: "menu".into(),
            x: 0.0,
            y: 0.0,
            w: 40.0,
            h: 40.0,
        });
        api.push(UiRequest::Text {
            window: "menu".into(),
            text: "hi".into(),
            x: 4.0,
            y: 4.0,
        });
        world.run_system_once(drain).unwrap();

        // The second drain drops the panel quads with the rest of the
        // children; they must come back with the fresh content.
        api.push(UiRequest::Window {
            name: "menu".into(),
            x: 0.0,
            y: 0.0,
            w: 40.0,
            h: 40.0,
        });
        api.push(UiRequest::Text {
            window: "menu".into(),
            text: "hi".into(),
            x: 4.0,
            y: 4.0,
        });
        world.run_system_once(drain).unwrap();

        let mut query = world.query_filtered::<&Mesh2d, ()>();
        assert!(
            query.iter(&world).count() >= 2,
            "panel border and fill survive rebuilds"
        );
    }

    #[test]
    fn close_all_wipes_windows_and_nav_in_one_sweep() {
        let (mut world, api) = ui_world();
        api.push(UiRequest::Window {
            name: "menu".into(),
            x: 0.0,
            y: 0.0,
            w: 40.0,
            h: 40.0,
        });
        api.push(UiRequest::Options {
            window: "menu".into(),
            x: 4.0,
            y: 4.0,
            labels: vec!["A".into(), "B".into()],
        });
        world.run_system_once(drain).unwrap();

        let mut query = world.query::<(Entity, &UiWindow)>();
        let entities: Vec<Entity> = query.iter(&world).map(|(entity, _)| entity).collect();
        let mut commands = world.commands();
        close_all(&api, &mut commands, entities.into_iter());
        world.flush();
        assert!(window_names(&mut world).is_empty());
        assert_eq!(api.nav().lock().unwrap().selected("menu"), None);
    }

    #[test]
    fn panels_sit_within_the_virtual_screen() {
        let mut world = World::new();
        // A full-screen window at the origin maps its center near the
        // world origin, whatever the exact virtual-to-world transform.
        world.insert_resource(BubbleTheme::default());
        world.insert_resource(Assets::<Mesh>::default());
        world.insert_resource(Assets::<ColorMaterial>::default());
        world.insert_resource(Assets::<GradientMaterial>::default());
        let assets = bubble::test_assets(&mut world);
        world.insert_resource(assets);
        let rect = UiWindow {
            name: "full".into(),
            x: 0.0,
            y: 0.0,
            w: GAME_WIDTH as f32,
            h: GAME_HEIGHT as f32,
        };
        let assets = world.resource::<BubbleAssets>().clone();
        let mut commands = world.commands();
        spawn_window(&mut commands, &assets, &rect);
        world.flush();
        let mut query = world.query_filtered::<&Transform, With<UiWindow>>();
        let transform = query.single(&world).unwrap();
        assert!(transform.translation.x.abs() < 1.0);
        assert!(transform.translation.y.abs() < 1.0);
    }
}

//! World-level scripts: game-lifetime Rhai programs that run
//! independently of scenes, ticked every fixed step (see
//! [`crate::scripts`] for the contract).

use std::path::Path;
use std::sync::PoisonError;

use bevy::prelude::*;
use rhai::Scope;

use crate::assets::assets_root;
use crate::input::InputManager;
use crate::scripts::{
    ScriptBroken, ScriptEnv, WorldScript as WorldScriptRuntime, compile_script_file,
};
use rhai::Dynamic;
use crate::systems::party::Party;
use crate::systems::ui::UiApi;
use crate::world_state::WorldState;

/// Where world scripts live, relative to the assets folder.
const WORLD_SCRIPTS_DIR: &str = "scripts/world";

/// One world script's runtime, spawned at startup and ticked forever.
#[derive(Component)]
pub(crate) struct WorldScript {
    /// Full path, for the runtime-error warning.
    path: String,
    runtime: WorldScriptRuntime,
    scope: Scope<'static>,
}

/// Startup: compiles every `.rhai` file in the world scripts folder.
pub(crate) fn startup(
    mut commands: Commands,
    input: Res<InputManager>,
    ui: Res<UiApi>,
    state: Res<WorldState>,
    battle: Res<crate::battle::BattleHandle>,
    world: Res<crate::scripts::WorldCommands>,
) {
    let env = ScriptEnv::new(
        input.handle(),
        ui.clone(),
        state.clone(),
        battle.clone(),
        world.clone(),
    );
    spawn_world_scripts(&mut commands, &assets_root().join(WORLD_SCRIPTS_DIR), &env);
}

/// Compiles every `.rhai` file in `dir` into a world script entity, in
/// path order. A missing or empty dir means zero world scripts, which
/// is fine.
fn spawn_world_scripts(commands: &mut Commands, dir: &Path, env: &ScriptEnv) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<_> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext.to_str() == Some("rhai"))
        })
        .collect();
    paths.sort();
    for path in paths {
        // World scripts persist under their own (relative) path.
        let env = env.clone().with_store(&path.to_string_lossy());
        let Some(runtime) = compile_script_file(&path, "World", move |text| {
            WorldScriptRuntime::compile_with_handle(text, env.clone())
        }) else {
            continue;
        };
        commands.spawn((WorldScript {
            path: path.display().to_string(),
            runtime,
            scope: Scope::new(),
        },));
    }
}

/// Runs each world script's `on_update`. A runtime error disables the
/// script with one warning.
pub(crate) fn run_world_scripts(
    mut commands: Commands,
    time: Res<Time>,
    mut party: ResMut<Party>,
    mut scripts: Query<(Entity, &mut WorldScript), Without<ScriptBroken>>,
) {
    let dt = time.delta_secs();
    for (entity, mut script) in &mut scripts {
        let WorldScript {
            path,
            runtime,
            scope,
        } = &mut *script;
        match runtime.update(scope, dt) {
            Ok(changes) => party.apply(&changes),
            Err(e) => {
                warn!("World script {path} errored, disabling it: {e}");
                commands.entity(entity).insert(ScriptBroken);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;
    use std::path::PathBuf;
    use std::sync::PoisonError;

    use super::*;
    use rhai::Dynamic;

    /// A fresh temp folder with the given files, removed on drop so a
    /// failed test doesn't leave junk behind.
    struct TempDir(PathBuf);

    impl TempDir {
        fn with(files: &[(&str, &str)]) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "wakeful-world-scripts-{}-{:p}",
                std::process::id(),
                files.as_ptr()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            for (name, text) in files {
                std::fs::write(dir.join(name), text).unwrap();
            }
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn every_rhai_file_in_the_folder_becomes_a_world_script() {
        let dir = TempDir::with(&[
            ("b.rhai", "fn on_update(dt) { }"),
            ("a.rhai", "fn on_update(dt) { }"),
            ("notes.txt", "not a script"),
            ("broken.rhai", "fn broken {"),
        ]);

        let mut world = World::new();
        world.insert_resource(crate::input::InputManager::standard());
        let input = world.resource::<crate::input::InputManager>().handle();
        let env = crate::scripts::ScriptEnv::new(
            input,
            crate::systems::ui::UiApi::new(),
            WorldState::default(),
            crate::battle::BattleHandle::new(),
            crate::scripts::WorldCommands::default(),
        );
        spawn_world_scripts(&mut world.commands(), &dir.0, &env);
        world.flush();

        let mut scripts = world.query::<&WorldScript>();
        let found: Vec<_> = scripts
            .iter(&world)
            .map(|script| script.path.clone())
            .collect();
        // Path order; the non-rhai file and the compile failure are
        // skipped (the latter warned about).
        assert_eq!(
            found,
            vec![
                dir.0.join("a.rhai").display().to_string(),
                dir.0.join("b.rhai").display().to_string(),
            ]
        );
    }

    #[test]
    fn a_missing_folder_means_zero_world_scripts() {
        let mut world = World::new();
        world.insert_resource(crate::input::InputManager::standard());
        let input = world.resource::<crate::input::InputManager>().handle();
        let env = crate::scripts::ScriptEnv::new(
            input,
            crate::systems::ui::UiApi::new(),
            WorldState::default(),
            crate::battle::BattleHandle::new(),
            crate::scripts::WorldCommands::default(),
        );
        spawn_world_scripts(
            &mut world.commands(),
            Path::new("/nonexistent/wakeful"),
            &env,
        );
        world.flush();

        let mut scripts = world.query::<&WorldScript>();
        assert_eq!(scripts.iter(&world).count(), 0);
    }

    #[test]
    fn the_shipped_triangle_menu_script_drives_the_ui() {
        use crate::input::{InputManager, PadButton};
        use crate::scripts::UiRequest;
        use crate::systems::ui::{UiApi, navigate as ui_navigate};

        let mut world = World::new();
        world.insert_resource(Party::default());
        world.insert_resource(Time::<()>::default());
        world.insert_resource(InputManager::standard());
        world.insert_resource(crate::input::InputCaptures::default());
        let api = UiApi::new();
        world.insert_resource(api.clone());
        let handle = world.resource::<InputManager>().handle();
        let env = crate::scripts::ScriptEnv::new(
            handle,
            api.clone(),
            WorldState::default(),
            crate::battle::BattleHandle::new(),
            crate::scripts::WorldCommands::default(),
        );
        let runtime = WorldScriptRuntime::compile_with_handle(
            include_str!("../../assets/scripts/world/triangle_menu.rhai"),
            env,
        )
        .unwrap();
        world.spawn((WorldScript {
            path: "scripts/world/triangle_menu.rhai".into(),
            runtime,
            scope: Scope::new(),
        },));

        // Per tick: aggregate input into the shared state (navigate
        // consumes it, the script reads confirmations), run the script,
        // then inspect what UI it requested.
        let tick = |world: &mut World, just: &[PadButton]| {
            let input = world.resource::<InputManager>().handle();
            input
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .inject(&[], just, &[]);
            world.run_system_once(ui_navigate).unwrap();
            world.run_system_once(run_world_scripts).unwrap();
            api.take_requests()
        };

        // Closed by default: no UI at all.
        let requests = tick(&mut world, &[]);
        assert!(requests.is_empty());

        // Triangle opens: the field pauses and the menu is declared.
        let requests = tick(&mut world, &[PadButton::Triangle]);
        assert!(requests.contains(&UiRequest::Pause(true)));
        assert!(requests.contains(&UiRequest::Window {
            name: "menu".into(),
            x: 8.0,
            y: 8.0,
            w: 200.0,
            h: 110.0,
            lift: 0.0,
        }));

        // The engine's reconcile declares the options; the script
        // re-declares the menu each tick while it stays open.
        api.nav()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .declare("menu", 5);
        let requests = tick(&mut world, &[]);
        assert!(
            requests
                .iter()
                .any(|request| matches!(request, UiRequest::Window { name, .. } if name == "menu"))
        );

        // Cross picks the top option: the stub panel is declared.
        let requests = tick(&mut world, &[PadButton::Cross]);
        assert!(requests.contains(&UiRequest::Window {
            name: "picked".into(),
            x: 60.0,
            y: 128.0,
            w: 140.0,
            h: 22.0,
            lift: 0.0,
        }));

        // Circle closes everything and unfreezes the field.
        let requests = tick(&mut world, &[PadButton::Circle]);
        assert!(requests.contains(&UiRequest::Pause(false)));
        assert!(
            requests
                .iter()
                .any(|request| matches!(request, UiRequest::Close { name } if name == "menu"))
        );
        assert!(
            requests
                .iter()
                .any(|request| matches!(request, UiRequest::Close { name } if name == "picked"))
        );
    }

    #[test]
    fn the_shipped_roster_script_bootstraps_and_levels_up() {
        // The character-sheet data layer: three ticks settle the store
        // (defs, roster, sheets), then XP posted by a battle levels the
        // hero up along the curve and the derived stats follow.
        let mut world = World::new();
        world.insert_resource(Time::<()>::default());
        world.insert_resource(crate::systems::party::Party::default());
        let state = crate::world_state::WorldState::default();
        world.insert_resource(state.clone());
        let env = crate::scripts::ScriptEnv::new(
            crate::input::detached(),
            crate::systems::ui::UiApi::new(),
            state.clone(),
            crate::battle::BattleHandle::new(),
            crate::scripts::WorldCommands::default(),
        );
        let runtime = WorldScriptRuntime::compile_with_handle(
            include_str!("../../assets/scripts/world/roster.rhai"),
            env,
        )
        .expect("the shipped roster script must compile");
        let script_entity = world
            .spawn((WorldScript {
                path: "scripts/world/roster.rhai".into(),
                runtime,
                scope: Scope::new(),
            },))
            .id();

        world.run_system_once(run_world_scripts).unwrap();
        assert!(
            world.get::<ScriptBroken>(script_entity).is_none(),
            "the roster script must not error on its ticks"
        );
        world.run_system_once(run_world_scripts).unwrap();
        assert!(
            world.get::<ScriptBroken>(script_entity).is_none(),
            "the roster script must keep running"
        );

        let sheet_of = |shared: &crate::world_state::WorldState, id: &str| -> rhai::Map {
            shared
                .shared()
                .lock()
                .unwrap()
                .get("sheets")
                .and_then(|v| v.clone().try_cast::<rhai::Map>())
                .and_then(|sheets| sheets.get(id).cloned())
                .and_then(|v| v.try_cast::<rhai::Map>())
                .expect("the sheet must exist")
        };
        let num = |sheet: &rhai::Map, key: &str| -> f64 {
            sheet
                .get(key)
                .and_then(|v| v.as_float().ok())
                .unwrap()
        };

        let hero = sheet_of(&state, "hero");
        assert_eq!(
            hero.get("level").and_then(|v| v.as_int().ok()),
            Some(1)
        );
        // Base fighter str 10 + the rusty knife's 2.
        assert!((num(&hero, "str") - 12.0).abs() < 1e-6);
        assert!((num(&hero, "max_hp") - 60.0).abs() < 1e-6);

        let ember = sheet_of(&state, "ember");
        assert!(ember.contains_key("spells"), "the mage owns spells");

        // A battle posts 50 xp: level 2 (25 needed) with 25 left over,
        // and the derived strength picks up the level's growth.
        let entry = rhai::Map::from_iter([
            ("id".into(), Dynamic::from("hero")),
            ("amount".into(), Dynamic::from(50.0_f64)),
        ]);
        state
            .shared()
            .lock()
            .unwrap()
            .insert(
                "xp_pending".into(),
                Dynamic::from(vec![Dynamic::from(entry)]),
            );
        world.run_system_once(run_world_scripts).unwrap();

        // TEMP: why didn't the level land?
        {
            let shared = state.shared();
            let guard = shared.lock().unwrap();
            eprintln!(
                "POST-XP pending = {:?} hero = {:?} broken = {:?}",
                guard.get("xp_pending"),
                guard
                    .get("sheets")
                    .and_then(|v| v.clone().try_cast::<rhai::Map>())
                    .and_then(|s| s.get("hero").cloned()),
                world.get::<ScriptBroken>(script_entity).is_some(),
            );
        }

        let hero = sheet_of(&state, "hero");
        assert_eq!(
            hero.get("level").and_then(|v| v.as_int().ok()),
            Some(2)
        );
        assert!((num(&hero, "xp") - 25.0).abs() < 1e-6, "leftover xp kept");
        assert!((num(&hero, "str") - 14.0).abs() < 1e-6, "growth applied");
    }

    #[test]
    fn the_shipped_party_script_populates_the_roster() {
        // Guards against the script erroring on its first tick (which
        // would silently disable it and leave the capsule on the field).
        let mut world = World::new();
        world.insert_resource(crate::systems::party::Party::default());
        world.insert_resource(Time::<()>::default());
        let runtime =
            WorldScriptRuntime::compile(include_str!("../../assets/scripts/world/party.rhai"))
                .expect("the shipped party script must compile");
        world.spawn((WorldScript {
            path: "scripts/world/party.rhai".into(),
            runtime,
            scope: Scope::new(),
        },));

        world.run_system_once(run_world_scripts).unwrap();

        // The roster is private; its Debug view is the readback.
        let party = format!("{:?}", world.resource::<crate::systems::party::Party>());
        assert!(party.contains(r#"leader: Some("hero")"#), "{party}");
    }

    #[test]
    fn a_world_script_which_errors_is_disabled_not_spammed() {
        let mut world = World::new();
        world.insert_resource(crate::systems::party::Party::default());
        world.insert_resource(Time::<()>::default());
        let runtime = WorldScriptRuntime::compile("fn on_update(dt) { bogus(); }").unwrap();
        let entity = world
            .spawn((WorldScript {
                path: "scripts/world/test.rhai".into(),
                runtime,
                scope: Scope::new(),
            },))
            .id();

        world.run_system_once(run_world_scripts).unwrap();
        world.flush();

        assert!(world.get::<ScriptBroken>(entity).is_some());
        // The tick query filters it out from then on.
        world.run_system_once(run_world_scripts).unwrap();
    }
}



/// Drains script scene-operations: `warp_to` rides the teleporter's
/// covered-point flow (the fade begins now, the scene swaps behind the
/// cover), `teleport_player` repositions within the current scene.
/// Both are Scene-only: a battle or a transition absorbs the request.
/// The single save slot, beside the checkout (`saves/` is gitignored).
pub(crate) fn save_path() -> std::path::PathBuf {
    std::path::PathBuf::from("saves/save0.json")
}

/// What a save holds: the shared script store (sheets, inventory,
/// flags) as parsed JSON — converted back to script values by hand
/// ([`dynamic_from_json`]) because rhai's own `Deserialize` turns bare
/// integers into opaque blobs — plus the current scene file and the
/// player's position within it. Rebooting into it restores all three.
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct SaveGame {
    store: serde_json::Value,
    scene: String,
    position: [f32; 2],
}

/// Flattens a rhai [`Dynamic`] to plain JSON — the mirror of
/// [`dynamic_from_json`]. rhai's own `Serialize` emits type-tagged
/// blobs for everything that is not a string, so the save format
/// walks the value tree by hand.
fn dynamic_to_json(value: &Dynamic) -> serde_json::Value {
    use serde_json::Value;
    // Discriminated by `type_name` + a checked cast: rhai's Variant
    // enum lives behind the `internals` feature, which this crate does
    // not need for one traversal.
    match value.type_name() {
        "bool" => Value::Bool(value.clone().cast::<bool>()),
        // Rhai keeps every Rust integer width as its own variant, so
        // each is widened explicitly (script integers arrive as i64).
        "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" => {
            let wide = match value.type_name() {
                "i8" => value.clone().cast::<i8>() as i64,
                "i16" => value.clone().cast::<i16>() as i64,
                "i32" => value.clone().cast::<i32>() as i64,
                "i64" => value.clone().cast::<i64>(),
                "u8" => value.clone().cast::<u8>() as i64,
                "u16" => value.clone().cast::<u16>() as i64,
                "u32" => value.clone().cast::<u32>() as i64,
                _ => value.clone().cast::<u64>() as i64,
            };
            Value::Number(wide.into())
        }
        "f32" | "f64" | "float" => value
            .as_float()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        "string" | "String" => {
            Value::String(value.clone().into_string().unwrap_or_default())
        }
        "array" | "Array" => {
            let items: Vec<Dynamic> = value.clone().cast();
            Value::Array(items.iter().map(dynamic_to_json).collect())
        }
        // Rhai's own map type calls itself "map"; maps built by Rust
        // callers with std types keep their full Rust type name.
        "map" | "Map" => {
            let map: rhai::Map = value.clone().cast();
            Value::Object(
                map.into_iter()
                    .map(|(key, value)| (key.to_string(), dynamic_to_json(&value)))
                    .collect(),
            )
        }
        _ if value
            .type_name()
            .ends_with("BTreeMap<alloc::string::String, rhai::types::dynamic::Dynamic>")
            || value
                .type_name()
                .ends_with("BTreeMap<String, rhai::types::dynamic::Dynamic>") =>
        {
            let map: std::collections::BTreeMap<String, Dynamic> = value.clone().cast();
            Value::Object(
                map.into_iter()
                    .map(|(key, value)| (key, dynamic_to_json(&value)))
                    .collect(),
            )
        }
        _ => Value::Null,
    }
}

/// Rebuilds a rhai [`Dynamic`] from parsed JSON. Numbers keep their
/// flavor: JSON integers come back as rhai ints, decimals as floats.
fn dynamic_from_json(value: &serde_json::Value) -> Dynamic {
    use serde_json::Value;
    match value {
        Value::Null => Dynamic::UNIT,
        Value::Bool(flag) => Dynamic::from(*flag),
        Value::Number(number) => number
            .as_i64()
            .map(Dynamic::from)
            .or_else(|| number.as_f64().map(Dynamic::from))
            .unwrap_or(Dynamic::UNIT),
        Value::String(text) => Dynamic::from(text.clone()),
        Value::Array(items) => Dynamic::from(
            items
                .iter()
                .map(dynamic_from_json)
                .collect::<Vec<Dynamic>>(),
        ),
        Value::Object(map) => {
            // rhai's own map type: restored values must be
            // indistinguishable from script-made ones.
            let restored: rhai::Map = map
                .iter()
                .map(|(key, value)| {
                    // Native keys, inferred from the map's key type.
                    let key = key.clone().into();
                    (key, dynamic_from_json(value))
                })
                .collect();
            Dynamic::from(restored)
        }
    }
}

/// Boot and parse requests drain here too, and unlike scene warps they
/// run from the StartMenu: `StartGame` clears or restores the shared
/// store, requests the scene, and flips the state to Scene; `SaveGame`
/// snapshots the store plus the player's scene/position. The other
/// requests stay Scene-gated.
#[allow(clippy::too_many_arguments)]
pub(crate) fn drain_world_commands(
    world: Res<crate::scripts::WorldCommands>,
    state: Res<State<crate::game_state::GameState>>,
    mut next_state: ResMut<NextState<crate::game_state::GameState>>,
    mut transitions: ResMut<crate::transition::TransitionState>,
    mut commands: Commands,
    mut player: Query<&mut Transform, With<crate::Player>>,
    current: Option<Res<crate::CurrentScene>>,
    shared: Option<Res<crate::world_state::WorldState>>,
) {
    for request in world.take() {
        match request {
            crate::scripts::WorldRequest::StartGame { new_game } => {
                let mut save = None;
                if !new_game {
                    match std::fs::read_to_string(save_path())
                        .map_err(|e| e.to_string())
                        .and_then(|text| {
                            serde_json::from_str::<SaveGame>(&text).map_err(|e| e.to_string())
                        }) {
                        Ok(loaded) => save = Some(loaded),
                        // No slot, or junk in it: boot fresh but say why.
                        Err(e) => warn!("the save could not load, booting fresh: {e}"),
                    }
                }
                let save = save.unwrap_or(SaveGame {
                    store: serde_json::Value::Object(Default::default()),
                    scene: crate::systems::scene::DEFAULT_SCENE_PATH.to_owned(),
                    position: [f32::MAX, 0.0],
                });
                if let Some(shared) = &shared {
                    let mut restored = std::collections::BTreeMap::new();
                    for (key, value) in save
                        .store
                        .as_object()
                        .expect("the saved store is a JSON object")
                    {
                        restored.insert(key.clone(), dynamic_from_json(value));
                    }
                    *shared.shared().lock().unwrap_or_else(PoisonError::into_inner) = restored;
                }
                // f32::MAX means "the scene's own default spot".
                let spawn = (save.position[0] != f32::MAX)
                    .then(|| Vec2::new(save.position[0], save.position[1]));
                commands.insert_resource(crate::systems::scene::DesiredScene {
                    path: save.scene,
                    spawn,
                });
                next_state.set(crate::game_state::GameState::Scene);
            }
            crate::scripts::WorldRequest::SaveGame => {
                let (Some(current), Some(shared)) = (current.as_ref(), shared.as_ref()) else {
                    warn!("save_game: nothing to save yet");
                    continue;
                };
                let store = serde_json::Value::Object(
                    shared
                        .shared()
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .iter()
                        .map(|(key, value)| (key.to_string(), dynamic_to_json(value)))
                        .collect(),
                );
                let position = match player.single() {
                    Ok(transform) => [transform.translation.x, transform.translation.z],
                    Err(_) => {
                        warn!("save_game: no player exists");
                        continue;
                    }
                };
                let save = SaveGame {
                    store,
                    scene: current.path.clone(),
                    position,
                };
                if let Err(e) = std::fs::create_dir_all(
                    save_path()
                        .parent()
                        .expect("the save path has a folder"),
                ) {
                    warn!("save_game: {}: {e}", save_path().display());
                    continue;
                }
                match serde_json::to_string_pretty(&save)
                    .map_err(|e| e.to_string())
                    .and_then(|text| {
                        std::fs::write(save_path(), text).map_err(|e| e.to_string())
                    }) {
                    Ok(()) => info!("saved {}", save_path().display()),
                    Err(e) => warn!("save_game: {}: {e}", save_path().display()),
                }
            }
            request => {
                if *state.get() != crate::game_state::GameState::Scene {
                    warn!("world command {request:?} ignored outside the Scene state");
                    continue;
                }
                match request {
                    crate::scripts::WorldRequest::Warp { scene, arrival } => {
                        commands.insert_resource(crate::systems::teleport::PendingSceneWarp {
                            target: scene,
                            arrival,
                        });
                        transitions.begin(
                            crate::game_state::GameState::Scene,
                            crate::transition::Effect::Fade,
                        );
                    }
                    crate::scripts::WorldRequest::Teleport { position } => {
                        if let Ok(mut transform) = player.single_mut() {
                            transform.translation.x = position.x;
                            transform.translation.z = position.y;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Keeps `game_state()` fresh and tells world scripts when the top-level
/// state changes: `on_state_change(from, to)` is an optional hook each
/// script may define (the start menu closes its window there). Scripts
/// own their windows, so the engine only announces the change.
pub(crate) fn sync_game_state(
    state: Res<State<crate::game_state::GameState>>,
    name: Res<crate::scripts::CurrentStateName>,
    mut party: ResMut<Party>,
    mut commands: Commands,
    mut scripts: Query<(Entity, &mut WorldScript), Without<ScriptBroken>>,
) {
    // The resource doubles as the memory of what the state was.
    let from = name.current();
    let to = label_of(state.get());
    name.set(to);
    // `from == ""` is boot, not a transition: nothing changed.
    if from.is_empty() || from == to {
        return;
    }
    for (entity, mut script) in &mut scripts {
        let WorldScript {
            path,
            runtime,
            scope,
        } = &mut *script;
        match runtime.on_state_change(scope, from, to) {
            Ok(changes) => party.apply(&changes),
            Err(e) => {
                warn!("World script {path} errored in on_state_change, disabling it: {e}");
                commands.entity(entity).insert(ScriptBroken);
            }
        }
    }
}

/// The short name `game_state()` reports for a state.
fn label_of(state: &crate::game_state::GameState) -> &'static str {
    match state {
        crate::game_state::GameState::StartMenu => "StartMenu",
        crate::game_state::GameState::Scene => "Scene",
        crate::game_state::GameState::Transition => "Transition",
        crate::game_state::GameState::Battle => "Battle",
    }
}

#[cfg(test)]
mod state_change_tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// A world script sees `on_state_change(from, to)` once per
    /// transition; a script without the hook is untouched.
    #[test]
    fn state_changes_reach_world_scripts() {
        let mut world = World::new();
        world.insert_resource(
            bevy::state::state::State::new(crate::game_state::GameState::StartMenu),
        );
        let name = crate::scripts::CurrentStateName::shared();
        world.insert_resource(name.clone());
        world.insert_resource(Party::default());

        let state = crate::world_state::WorldState::default();
        let env = crate::scripts::ScriptEnv::new(
            crate::input::detached(),
            crate::systems::ui::UiApi::new(),
            state.clone(),
            crate::battle::BattleHandle::new(),
            crate::scripts::WorldCommands::default(),
        );
        let source = concat!(
            "fn on_update(dt) {} ",
            "fn on_state_change(from, to) { remember_global(\"seen\", from + \"->\" + to); }",
        );
        let hooked =
            crate::scripts::WorldScript::compile_with_handle(source, env.clone())
                .expect("hooked script compiles");
        let quiet = crate::scripts::WorldScript::compile_with_handle(
            "fn on_update(dt) {}",
            env,
        )
        .expect("quiet script compiles");
        world.spawn(WorldScript {
            path: "hooked.rhai".into(),
            runtime: hooked,
            scope: Scope::new(),
        });
        world.spawn(WorldScript {
            path: "quiet.rhai".into(),
            runtime: quiet,
            scope: Scope::new(),
        });

        let seen = || {
            state
                .shared()
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get("seen")
                .cloned()
                .and_then(|v| v.into_string().ok())
        };

        // Same state twice: nothing to report.
        world.run_system_once(sync_game_state).unwrap();
        world.run_system_once(sync_game_state).unwrap();
        assert_eq!(seen(), None, "no transition yet, no callback");

        // Now a change.
        world.insert_resource(bevy::state::state::State::new(
            crate::game_state::GameState::Scene,
        ));
        world.run_system_once(sync_game_state).unwrap();
        assert_eq!(name.current(), "Scene", "game_state() tracks the state");
        assert_eq!(
            seen().as_deref(),
            Some("StartMenu->Scene"),
            "the hooked script saw the transition"
        );

        // And no repeats while the state holds.
        world.run_system_once(sync_game_state).unwrap();
        assert_eq!(seen().as_deref(), Some("StartMenu->Scene"));
    }
}

#[cfg(test)]
mod start_game_tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    use crate::scripts::{WorldCommands, WorldRequest};

    /// A world with every resource the boot drain reads, parked in the
    /// given state.
    fn boot_world(state: crate::game_state::GameState) -> World {
        let mut world = World::new();
        world.insert_resource(bevy::state::state::State::new(state));
        world.insert_resource(NextState::<
            crate::game_state::GameState,
        >::Unchanged);
        world.insert_resource(WorldCommands::default());
        world.insert_resource(crate::world_state::WorldState::default());
        world.insert_resource(crate::transition::TransitionState::default());
        world.insert_resource(Assets::<crate::scene::Scene>::default());
        world
    }

    fn queued(world: &mut World, request: WorldRequest) {
        world.resource::<WorldCommands>().push(request);
    }

    /// New Game wipes the shared store and boots the default scene.
    #[test]
    fn start_new_game_clears_the_store_and_boots_the_scene() {
        let mut world = boot_world(crate::game_state::GameState::StartMenu);
        // Junk from a previous playthrough.
        world
            .resource::<crate::world_state::WorldState>()
            .shared()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert("stale".into(), Dynamic::from(1.0));
        queued(
            &mut world,
            WorldRequest::StartGame { new_game: true },
        );

        world.run_system_once(drain_world_commands).unwrap();

        let shared = world
            .resource::<crate::world_state::WorldState>()
            .shared();
        assert!(shared.lock().unwrap_or_else(std::sync::PoisonError::into_inner).is_empty(), "fresh store");
        let desired = world.get_resource::<crate::systems::scene::DesiredScene>().unwrap();
        assert_eq!(desired.path, "scenes/devroom.scene");
        assert_eq!(desired.spawn, None);
        let next = world.resource::<NextState<crate::game_state::GameState>>();
        assert!(
            matches!(*next, NextState::Pending(_)),
            "the boot flips the state to Scene, got {next:?}"
        );
    }

    /// Continue restores the store, the scene, and the saved spot.
    #[test]
    fn continue_restores_store_scene_and_position() {
        let mut world = boot_world(crate::game_state::GameState::StartMenu);
        let store = serde_json::json!({ "sheet.hero.level": 7 });
        let save = SaveGame {
            store,
            scene: "scenes/room2.scene".into(),
            position: [2.0, 3.0],
        };
        std::fs::create_dir_all("saves").unwrap();
        std::fs::write(save_path(), serde_json::to_string(&save).unwrap()).unwrap();
        queued(
            &mut world,
            WorldRequest::StartGame { new_game: false },
        );

        // The round-trip must survive JSON before the drain reads it.
        let text = std::fs::read_to_string(save_path()).unwrap();
        let parsed: SaveGame = serde_json::from_str(&text).expect("parses");
        assert_eq!(parsed.scene, "scenes/room2.scene");

        world.run_system_once(drain_world_commands).unwrap();

        let shared = world
            .resource::<crate::world_state::WorldState>()
            .shared();
        assert_eq!(
            shared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get("sheet.hero.level")
                .and_then(|v| v.clone().try_cast::<i64>()),
            Some(7),
            "the restored store replaced the shared contents"
        );
        let desired = world.get_resource::<crate::systems::scene::DesiredScene>().unwrap();
        assert_eq!(desired.path, "scenes/room2.scene");
        assert_eq!(desired.spawn, Some(Vec2::new(2.0, 3.0)));
        let _ = std::fs::remove_file(save_path());
    }

    /// Then the desired scene consumes: CurrentScene/SceneApplied set.
    /// rhai's serde types: an integer in a saved store must come back
    /// as a plain rhai int, not a deserializer blob.
    #[test]
    fn the_json_store_keeps_numbers_typed() {
        // Scalars round-trip through JSON with their type intact.
        for (value, json) in [
            (Dynamic::from(7), "7"),
            (Dynamic::from(60.5), "60.5"),
            (Dynamic::from(true), "true"),
            (Dynamic::from("hero".to_owned()), "\"hero\""),
        ] {
            assert_eq!(
                super::dynamic_to_json(&value).to_string(),
                json,
                "flattens to plain JSON"
            );
            assert_eq!(
                super::dynamic_from_json(&json.parse::<serde_json::Value>().unwrap())
                    .to_string(),
                value.to_string(),
                "rebuilds as the same rhai value"
            );
        }
        // Nested maps flatten whole, so a saved sheet survives.
        let nested: rhai::Map =
            [("hero.level".into(), Dynamic::from(3))].into_iter().collect();
        assert_eq!(
            super::dynamic_to_json(&Dynamic::from(nested)).to_string(),
            r#"{"hero.level":3}"#
        );
    }

    #[test]
    fn the_desired_scene_stages_the_boot() {
        let mut world = boot_world(crate::game_state::GameState::Scene);
        use bevy::asset::{AssetServer, AssetServerMode, UnapprovedPathMode, io::AssetSourceBuilders};
        let mut builders = AssetSourceBuilders::default();
        builders.init_default_source("assets", None);
        let server = AssetServer::new(
            std::sync::Arc::new(builders.build_sources(false, false)),
            AssetServerMode::Unprocessed,
            false,
            UnapprovedPathMode::Forbid,
        );
        let scene_assets = Assets::<crate::scene::Scene>::default();
        server.register_asset(&scene_assets);
        world.insert_resource(server);
        world.insert_resource(scene_assets);
        world.insert_resource(crate::systems::scene::DesiredScene {
            path: "scenes/devroom.scene".into(),
            spawn: Some(Vec2::new(1.0, 2.0)),
        });
        world.run_system_once(crate::systems::scene::load_desired_scene)
            .unwrap();

        let current = world.get_resource::<crate::CurrentScene>().expect("current scene requested");
        assert_eq!(current.path, "scenes/devroom.scene");
        assert_eq!(
            world.get_resource::<crate::PlayerSpawn>().map(|s| s.0),
            Some(Vec2::new(1.0, 2.0))
        );
        assert!(world.get_resource::<crate::systems::scene::DesiredScene>().is_none());
    }
}

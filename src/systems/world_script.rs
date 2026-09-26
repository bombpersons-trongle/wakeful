//! World-level scripts: game-lifetime Rhai programs that run
//! independently of scenes, ticked every fixed step (see
//! [`crate::scripts`] for the contract).

use std::path::Path;

use bevy::prelude::*;
use rhai::Scope;

use crate::assets::assets_root;
use crate::input::{InputHandle, InputManager};
use crate::scripts::{ScriptBroken, WorldScript as WorldScriptRuntime, compile_script_file};
use crate::systems::party::Party;
use crate::systems::ui::UiApi;

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
pub(crate) fn startup(mut commands: Commands, input: Res<InputManager>, ui: Res<UiApi>) {
    spawn_world_scripts(
        &mut commands,
        &assets_root().join(WORLD_SCRIPTS_DIR),
        &input.handle(),
        &ui,
    );
}

/// Compiles every `.rhai` file in `dir` into a world script entity, in
/// path order. A missing or empty dir means zero world scripts, which
/// is fine.
fn spawn_world_scripts(commands: &mut Commands, dir: &Path, input: &InputHandle, ui: &UiApi) {
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
        let handle = input.clone();
        let ui = ui.clone();
        let Some(runtime) = compile_script_file(&path, "World", move |text| {
            WorldScriptRuntime::compile_with_handle(text, handle.clone(), ui.clone())
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
        world.insert_resource(crate::systems::ui::UiApi::new());
        let input = world.resource::<crate::input::InputManager>().handle();
        let ui = world.resource::<crate::systems::ui::UiApi>().clone();
        spawn_world_scripts(&mut world.commands(), &dir.0, &input, &ui);
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
        world.insert_resource(crate::systems::ui::UiApi::new());
        let input = world.resource::<crate::input::InputManager>().handle();
        let ui = world.resource::<crate::systems::ui::UiApi>().clone();
        spawn_world_scripts(
            &mut world.commands(),
            Path::new("/nonexistent/wakeful"),
            &input,
            &ui,
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
        let api = UiApi::new();
        world.insert_resource(api.clone());
        let handle = world.resource::<InputManager>().handle();
        let runtime = WorldScriptRuntime::compile_with_handle(
            include_str!("../../assets/scripts/world/triangle_menu.rhai"),
            handle.clone(),
            api.clone(),
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

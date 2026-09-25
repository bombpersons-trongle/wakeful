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
pub(crate) fn startup(mut commands: Commands, input: Res<InputManager>) {
    spawn_world_scripts(
        &mut commands,
        &assets_root().join(WORLD_SCRIPTS_DIR),
        &input.handle(),
    );
}

/// Compiles every `.rhai` file in `dir` into a world script entity, in
/// path order. A missing or empty dir means zero world scripts, which
/// is fine.
fn spawn_world_scripts(commands: &mut Commands, dir: &Path, input: &InputHandle) {
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
        let Some(runtime) = compile_script_file(&path, "World", move |text| {
            WorldScriptRuntime::compile_with_handle(text, handle.clone())
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
        let input = world.resource::<crate::input::InputManager>().handle();
        spawn_world_scripts(&mut world.commands(), &dir.0, &input);
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
        spawn_world_scripts(
            &mut world.commands(),
            Path::new("/nonexistent/wakeful"),
            &input,
        );
        world.flush();

        let mut scripts = world.query::<&WorldScript>();
        assert_eq!(scripts.iter(&world).count(), 0);
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

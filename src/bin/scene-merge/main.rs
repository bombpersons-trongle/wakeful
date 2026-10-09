//! Folds Blender's half of a scene into the scene file itself.
//!
//!     cargo run --bin scene-merge -- assets/scenes/Village_Entrance.export
//!
//! The exporter writes what it owns — plate, depth, camera, walk mesh —
//! and leaves the actors, teleporters and scene script alone; this puts
//! the two halves together. See `src/scene_merge.rs` for what that split
//! is. An export whose scene file does not exist yet becomes the scene.
//!
//! With no arguments it merges every export under the assets folder, so
//! `tools/export_scenes.sh` can hand it a glob without knowing what came
//! out of Blender.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use wakeful::assets::{ASSETS_DIR, assets_root};
use wakeful::scene::Scene;
use wakeful::scene_merge::{ExportedScene, merge};

/// The suffix the exporter writes; the scene file is the same name
/// without it.
const EXPORT_SUFFIX: &str = "export";

fn main() -> ExitCode {
    let given: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let exports = match given.as_slice() {
        [] => pending_exports(),
        paths => paths.to_vec(),
    };
    if exports.is_empty() {
        println!("scene-merge: nothing to merge");
        return ExitCode::SUCCESS;
    }
    let mut failures = 0;
    for export in &exports {
        match merge_one(export) {
            Ok(Some(Merged { scene_path, actors, teleporters })) => println!(
                "scene-merge: {} -> {} ({actors} actors, {teleporters} teleporters)",
                export.display(),
                scene_path.display(),
            ),
            Ok(None) => {}
            Err(error) => {
                eprintln!("scene-merge: {}: {error}", export.display());
                failures += 1;
            }
        }
    }
    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Every `.export` currently sitting in `assets/scenes/`, sorted so a run
/// is reproducible.
fn pending_exports() -> Vec<PathBuf> {
    let dir = assets_root().join("scenes");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        eprintln!(
            "scene-merge: no {} folder at {}",
            ASSETS_DIR,
            dir.display()
        );
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == EXPORT_SUFFIX)
                && path.file_name().is_some_and(|name| name != EXPORT_SUFFIX)
        })
        .collect();
    found.sort();
    found
}

/// What one merge produced, for the line that reports it.
struct Merged {
    scene_path: PathBuf,
    actors: usize,
    teleporters: usize,
}

/// Merges one export into its sibling scene file. `Ok(None)` means the
/// path was not an export after all.
fn merge_one(export: &Path) -> std::io::Result<Option<Merged>> {
    if !export
        .extension()
        .is_some_and(|ext| ext == EXPORT_SUFFIX)
    {
        return Ok(None);
    }
    let scene_path = export.with_extension("scene");
    let existing = match read_scene(&scene_path) {
        Ok(scene) => Some(scene),
        // A first export: nothing on disk yet, so the export is the scene.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let exported: ExportedScene = read_ron(export)?;
    let scene = merge(existing.as_ref(), &exported);
    write_scene(&scene_path, &scene)?;
    Ok(Some(Merged {
        scene_path,
        actors: scene.actors.len(),
        teleporters: scene.teleporters.len(),
    }))
}

fn read_scene(path: &Path) -> std::io::Result<Scene> {
    let text = std::fs::read_to_string(path)?;
    ron::from_str(&text).map_err(std::io::Error::other)
}

fn read_ron<T: serde::de::DeserializeOwned>(path: &Path) -> std::io::Result<T> {
    let text = std::fs::read_to_string(path)?;
    ron::from_str(&text).map_err(std::io::Error::other)
}

/// Writes a scene the way the exporter's output reads: indented, but with
/// arrays left on one line. A walk mesh is thousands of vertices; one per
/// line turns every re-export into a ten-thousand-line diff nobody can
/// read, which is the whole reason this split exists.
fn write_scene(path: &Path, scene: &Scene) -> std::io::Result<()> {
    let config = ron::ser::PrettyConfig::default().compact_arrays(true);
    let mut ron = ron::ser::to_string_pretty(scene, config).map_err(std::io::Error::other)?;
    // The exporter ends its files with a newline; keep that, so the file
    // does not show up as changed just for the last line.
    ron.push('\n');
    std::fs::write(path, ron)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn export_ron(name: &str) -> String {
        format!(
            r#"(
    camera: (
        position: (1.0, 2.0, 3.0),
        target: (4.0, 5.0, 6.0),
        fov_degrees: 101.65,
    ),
    background: Some("backgrounds/{name}.png"),
    depth_map: Some("backgrounds/{name}_depth.png"),
    depth_range: 42.0,
)"#
        )
    }

    fn scene_with_a_cast() -> String {
        r#"(
    camera: (position: (0.0, 0.0, 0.0), target: (0.0, 0.0, 0.0), fov_degrees: 45.0),
    background: None,
    depth_map: None,
    depth_range: 30.0,
    teleporters: [
        (position: (0.0, 0.0), size: (2.0, 2.0), arrival: (0.0, 0.0), target: "scenes/room2.scene"),
    ],
    actors: [
        (
            model: "models/goblin.glb",
            position: (-28.3, -106.86),
            facing: Some(90.0),
            script: Some("scripts/goblin.rhai"),
            id: Some("goblin"),
        ),
    ],
    script: Some("scripts/world/party.rhai"),
)"#
        .to_owned()
    }

    /// A scratch directory named after the test, cleaned up afterwards.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("wakeful-merge-{name}"));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("the scratch directory is writable");
            Self(dir)
        }

        fn write(&self, name: &str, text: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, text).expect("the scratch file is writable");
            path
        }

        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn merging_keeps_the_cast_and_takes_the_plate() {
        let scratch = Scratch::new("keeps-cast");
        scratch.write("Village_Entrance.export", &export_ron("village_entrance"));
        scratch.write("Village_Entrance.scene", &scene_with_a_cast());
        let export = scratch.path("Village_Entrance.export");

        let merged = merge_one(&export).unwrap().expect("an export merges");
        let scene: Scene = read_scene(&merged.scene_path).unwrap();
        assert_eq!(merged.actors, 1);
        assert_eq!(merged.teleporters, 1);
        assert_eq!(
            scene.background.as_deref(),
            Some("backgrounds/village_entrance.png")
        );
        assert_eq!(scene.camera.fov_degrees, 101.65);
        assert_eq!(scene.actors.len(), 1);
        assert_eq!(scene.actors[0].id.as_deref(), Some("goblin"));
        assert_eq!(scene.teleporters.len(), 1);
        assert_eq!(scene.script.as_deref(), Some("scripts/world/party.rhai"));
    }

    #[test]
    fn a_first_export_writes_the_scene_file() {
        let scratch = Scratch::new("first-export");
        scratch.write("Arena.export", &export_ron("arena"));
        let export = scratch.path("Arena.export");
        assert!(!export.with_extension("scene").exists());

        let merged = merge_one(&export).unwrap().expect("an export merges");

        let scene: Scene = read_scene(&merged.scene_path).unwrap();
        assert_eq!(scene.depth_range, 42.0);
        assert!(scene.actors.is_empty());
        // Re-merging the same export changes nothing: it is idempotent.
        merge_one(&export).unwrap();
        assert_eq!(
            read_scene(&merged.scene_path).unwrap().depth_range,
            42.0
        );
    }

    #[test]
    fn a_path_that_is_not_an_export_is_left_alone() {
        let scratch = Scratch::new("not-an-export");
        let scene = scratch.write("notes.txt", "hello");
        assert!(merge_one(&scene).unwrap().is_none());
    }
}
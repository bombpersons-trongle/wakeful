//! The two halves of a scene file, and how they are put back together.
//!
//! A scene is authored by two parties who never watch each other work. The
//! Blender exporter owns the plate, its depth map, the camera pose it was
//! baked through, and the walk mesh — everything it derives from geometry.
//! The editor and hand-written scenes own the actors, the teleporters and
//! the scene script. A re-export rewrites the first half wholesale; if it
//! also rewrote the second, every placement made since would be lost.
//!
//! So the exporter writes only its half, to a `.export` file, and this
//! module finishes the job: [`ExportedScene`] is that half — a type with
//! nowhere to put an actor, so a stray field in an export is dropped
//! rather than merged — and [`merge`] folds it into what is on disk.

use serde::{Deserialize, Serialize};

use crate::scene::{CameraPose, PanSpec, Scene, WalkMesh};

/// The half of a scene file that Blender owns, and the only half a
/// re-export is allowed to replace. Every field here is one a merge
/// overwrites from an export; every field missing from it is one that only
/// the scene already on disk has.
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct ExportedScene {
    /// The pose the plate was baked through. The depth card is unprojected
    /// through this, so a re-export that moves the camera must rewrite it.
    pub camera: CameraPose,
    /// The ground the scene's characters walk on, when the blend carries
    /// a mesh for it.
    #[serde(default)]
    pub walk_mesh: Option<WalkMesh>,
    /// Plate image, relative to `assets/`.
    #[serde(default)]
    pub background: Option<String>,
    /// The plate's depth map, relative to `assets/`. A background with no
    /// depth map cannot become geometry.
    #[serde(default)]
    pub depth_map: Option<String>,
    /// The ray distance the depth map's far value encodes, in meters.
    pub depth_range: f32,
    /// How a plate larger than the game view is cropped to it.
    #[serde(default)]
    pub pan: Option<PanSpec>,
}

/// Folds an export into what is already on disk.
///
/// The exported half wins; the actors, teleporters and scene script are
/// kept as they are, since nothing in the blend knows they exist. With no
/// existing scene — a first export — the result is the export whole.
pub fn merge(existing: Option<&Scene>, exported: &ExportedScene) -> Scene {
    let mut scene = existing.cloned().unwrap_or(Scene {
        camera: exported.camera,
        walk_mesh: None,
        background: None,
        depth_map: None,
        depth_range: exported.depth_range,
        pan: None,
        teleporters: Vec::new(),
        actors: Vec::new(),
        script: None,
    });
    scene.camera = exported.camera;
    scene.walk_mesh = exported.walk_mesh.clone();
    scene.background = exported.background.clone();
    scene.depth_map = exported.depth_map.clone();
    scene.depth_range = exported.depth_range;
    scene.pan = exported.pan;
    scene
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{Actor, Teleporter};

    fn exported() -> ExportedScene {
        ExportedScene {
            camera: CameraPose {
                position: [1.0, 2.0, 3.0],
                target: [4.0, 5.0, 6.0],
                fov_degrees: 101.65,
            },
            walk_mesh: Some(WalkMesh::new(
                vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                vec![[0, 1, 2]],
            )),
            background: Some("backgrounds/village.png".to_owned()),
            depth_map: Some("backgrounds/village_depth.png".to_owned()),
            depth_range: 42.0,
            pan: Some(PanSpec {
                plate: (640, 480),
                window: (320, 240),
            }),
        }
    }

    /// A scene as it looks after a save: the editor has filled in the
    /// half the blend knows nothing about.
    fn placed(exported: &ExportedScene) -> Scene {
        let mut scene = merge(None, exported);
        scene.actors.push(Actor {
            id: Some("goblin".to_owned()),
            shared: false,
            params: Default::default(),
            model: "models/goblin.glb".to_owned(),
            position: [-28.3, -106.86],
            facing: Some(90.0),
            constrained: true,
            script: Some("scripts/goblin.rhai".to_owned()),
        });
        scene.teleporters.push(Teleporter {
            position: [-2.05, 0.0],
            size: [2.0, 2.0],
            arrival: [0.0, 0.0],
            target: "scenes/room2.scene".to_owned(),
        });
        scene.script = Some("scripts/world/party.rhai".to_owned());
        scene
    }

    #[test]
    fn a_first_export_becomes_the_scene_whole() {
        let exported = exported();
        let scene = merge(None, &exported);
        assert_eq!(scene.camera, exported.camera);
        assert_eq!(scene.background, exported.background);
        assert_eq!(scene.depth_map, exported.depth_map);
        assert_eq!(scene.depth_range, exported.depth_range);
        assert_eq!(scene.pan, exported.pan);
        assert_eq!(scene.walk_mesh.as_ref().map(|m| m.vertices.len()), Some(3));
        assert!(scene.actors.is_empty());
        assert!(scene.teleporters.is_empty());
        assert_eq!(scene.script, None);
    }

    #[test]
    fn a_re_export_keeps_what_the_blend_knows_nothing_about() {
        let exported = exported();
        let existing = placed(&exported);
        // A different plate, camera and mesh, as a second export of an
        // edited blend would carry.
        let mut second = exported.clone();
        second.camera.position = [9.0, 9.0, 9.0];
        second.background = Some("backgrounds/renamed.png".to_owned());
        second.depth_range = 99.0;

        let scene = merge(Some(&existing), &second);

        // The export's half is replaced...
        assert_eq!(scene.camera, second.camera);
        assert_eq!(scene.background, second.background);
        assert_eq!(scene.depth_range, second.depth_range);
        // ...and the placed half is not touched. Actor has no PartialEq:
        // its params are rhai values, which do not compare, so the check
        // is on the fields a placement is made of.
        assert_eq!(scene.actors.len(), 1);
        assert_eq!(scene.actors[0].id.as_deref(), Some("goblin"));
        assert_eq!(scene.actors[0].model, "models/goblin.glb");
        assert_eq!(scene.actors[0].position, [-28.3, -106.86]);
        assert_eq!(scene.actors[0].script.as_deref(), Some("scripts/goblin.rhai"));
        assert_eq!(scene.teleporters, existing.teleporters);
        assert_eq!(scene.script, existing.script);
    }

    #[test]
    fn a_re_export_replaces_the_walk_mesh_and_the_pan() {
        let exported = exported();
        let existing = placed(&exported);
        let mut second = exported.clone();
        second.walk_mesh = None;
        second.pan = None;

        let scene = merge(Some(&existing), &second);
        // These are geometry, not placement: an export that drops them
        // means the blend dropped them, and the file should say so.
        assert!(scene.walk_mesh.is_none());
        assert!(scene.pan.is_none());
    }

    #[test]
    fn an_export_that_smuggles_actors_cannot_replace_the_real_ones() {
        // ron ignores fields the type does not name, so an export carrying
        // a cast parses and is quietly empty of it. That is the property
        // worth having: the worst a stale or hand-edited export can do is
        // lose its own actors, never overwrite the scene's.
        let smuggles = r#"(
            camera: (position: (0.0, 0.0, 0.0), target: (1.0, 0.0, 0.0), fov_degrees: 45.0),
            depth_range: 30.0,
            actors: [
                (model: "models/statue.glb", position: (9.0, 9.0)),
            ],
        )"#;
        let smuggled: ExportedScene = ron::from_str(smuggles).unwrap();
        let existing = placed(&exported());
        let scene = merge(Some(&existing), &smuggled);
        assert_eq!(scene.actors.len(), 1);
        assert_eq!(scene.actors[0].model, "models/goblin.glb");
        // The rest of the export still applies; only the actors are lost.
        assert_eq!(scene.depth_range, 30.0);
    }
}
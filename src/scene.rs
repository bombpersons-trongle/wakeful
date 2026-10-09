//! Scene definitions: the data behind one "room" of the game — background
//! image, fixed camera pose, walkable ground, and the placed actors. Loaded from RON files in `assets/scenes/`.

use bevy::asset::Asset;
use bevy::math::Vec2;
use bevy::reflect::TypePath;
use rhai::Dynamic;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub use crate::walkmesh::WalkMesh;

#[derive(Asset, TypePath, Deserialize, Serialize, Clone)]
pub struct Scene {
    /// Path to the background image, relative to `assets/`. Expected to be
    /// the game's virtual resolution (320x240), like a pre-rendered FF7 room.
    pub background: Option<String>,
    /// Path to the background's depth map, relative to `assets/`: one value
    /// per background pixel encodes the ray distance from the scene camera
    /// (normalized over `depth_range`). With a map, the background renders
    /// as a depth card — real geometry in the scene camera's pass — so the
    /// z-buffer resolves character/background occlusion in both
    /// directions.
    #[serde(default)]
    pub depth_map: Option<String>,
    /// The world-space distance the depth map's maximum value encodes, in
    /// meters. Only meaningful alongside `depth_map`.
    #[serde(default = "default_depth_range")]
    pub depth_range: f32,
    /// The background plate is bigger than the game's 320x240 view: the
    /// view is a window onto it that follows the player, clamping at the
    /// plate's edges. `None` (the default) pins the view to the plate
    /// exactly.
    #[serde(default)]
    pub pan: Option<PanSpec>,
    pub camera: CameraPose,
    /// The ground characters walk on: a triangulated surface, exported
    /// from the blend file's walk mesh. Absent means nothing bounds
    /// movement, which is what a bare test room wants.
    #[serde(default)]
    pub walk_mesh: Option<WalkMesh>,
    /// Trigger rects that load another scene when the player touches one.
    #[serde(default)]
    pub teleporters: Vec<Teleporter>,
    /// Path to the scene's Rhai script, relative to `assets/`. It runs
    /// for as long as the scene does — `on_enter(player_x, player_z)`
    /// when the scene applies, `on_update(player_x, player_z, dt)` every
    /// fixed tick, `on_exit()` when it tears down; missing hooks are
    /// no-ops.
    #[serde(default)]
    pub script: Option<String>,
    /// Characters placed in the scene: a model, a ground position, and an
    /// optional Rhai script that moves them each tick.
    #[serde(default)]
    pub actors: Vec<Actor>,
}

fn default_depth_range() -> f32 {
    32.0
}

/// The background pans: rendered as a `plate`, shown through a smaller
/// `window` (normally the game's 320x240) that slides after the player.
/// Both aspects must be 4:3 — the window is a clean crop of the plate.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq)]
pub struct PanSpec {
    pub plate: (u32, u32),
    pub window: (u32, u32),
}

impl PanSpec {
    /// The window's travel range: how far its top-left corner may slide
    /// inside the plate.
    pub fn travel(&self) -> (f32, f32) {
        (
            (self.plate.0.saturating_sub(self.window.0)) as f32,
            (self.plate.1.saturating_sub(self.window.1)) as f32,
        )
    }
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq)]
pub struct CameraPose {
    pub position: [f32; 3],
    pub target: [f32; 3],
    pub fov_degrees: f32,
}

/// The scene files that ship with the game, by name under `assets/scenes`.
/// The editor's scene picker will offer exactly these.
pub const SHIPPED_SCENES: &[&str] = &[
    "devroom.scene",
    "room2.scene",
    "Village_Entrance.scene",
    "Shops_And_Bar.scene",
];

/// Reads a shipped scene file straight off disk, by name. A running game
/// gets its scene through the asset server instead; this is the path for
/// anything that wants the file rather than the loaded asset, which today
/// means the tests on both sides of the library.
pub fn read_shipped(name: &str) -> Scene {
    let path = crate::assets::assets_root().join("scenes").join(name);
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?} is readable: {e}"));
    ron::from_str(&text).unwrap_or_else(|e| panic!("{name} parses: {e}"))
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct Teleporter {
    /// World XZ center of the trigger rect.
    pub position: [f32; 2],
    /// Full XZ extents of the trigger rect.
    pub size: [f32; 2],
    /// Scene file (path relative to `assets/`) to load on touch.
    pub target: String,
    /// World XZ position where the player appears in the target scene.
    pub arrival: [f32; 2],
}

/// Serializes a scene the way the exporter's output reads: indented, but
/// with arrays left on one line.
///
/// The arrays are the point. A walk mesh is thousands of vertices, and one
/// per line turns every re-export into a ten-thousand-line diff nobody can
/// read — which is the whole reason the exporter stopped writing scene
/// files itself.
///
/// Comments do not survive this: RON has no way to write one back, so a
/// scene file is a generated artifact and anything a scene needs said
/// belongs somewhere else.
pub fn to_ron(scene: &Scene) -> Result<String, ron::Error> {
    let config = ron::ser::PrettyConfig::default().compact_arrays(true);
    let mut ron = ron::ser::to_string_pretty(scene, config)?;
    ron.push('\n');
    Ok(ron)
}

impl Teleporter {
    /// Whether the world XZ position lies inside the trigger rect. The
    /// low edge counts as inside, the high edge belongs to the next rect
    /// over, so walking along a shared edge fires the teleporter ahead
    /// rather than behind.
    pub fn contains(&self, x: f32, z: f32) -> bool {
        let half_x = self.size[0] / 2.0;
        let half_z = self.size[1] / 2.0;
        (self.position[0] - half_x..self.position[0] + half_x).contains(&x)
            && (self.position[1] - half_z..self.position[1] + half_z).contains(&z)
    }
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Actor {
    /// Optional game-wide identity: the key this actor's private
    /// script storage lives under. Two actors with the same id share
    /// memory wherever they appear.
    pub id: Option<String>,
    /// With an id, share its storage across scenes instead of scoping
    /// it to this one.
    #[serde(default)]
    pub shared: bool,
    /// Per-instance parameters the script reads with `param(key)`.
    #[serde(default)]
    pub params: BTreeMap<String, Dynamic>,
    /// glTF model path relative to `assets/` (default scene used, a
    /// `#SceneN` suffix, if present, is ignored).
    pub model: String,
    /// Ground-plane XZ position.
    pub position: [f32; 2],
    /// World yaw in degrees for the spawned model. `None` (the
    /// default) faces the scene camera's forward; a value pins the
    /// actor to a world facing — props like chests read better from
    /// a fixed angle.
    #[serde(default)]
    pub facing: Option<f32>,
    /// Whether the scene's walk mesh keeps this actor on it. On by
    /// default: a character that walks stays on the ground. Actors whose
    /// script places them elsewhere — a bird, a chest on a wall — turn it
    /// off, and only they are free of the mesh.
    #[serde(default = "default_constrained")]
    pub constrained: bool,
    /// Rhai script file relative to `assets/`. The script's
    /// `on_update(x, z, player_x, player_z, dt)` runs every fixed tick;
    /// returning `[x, z]` moves the actor there, returning nothing keeps
    /// it put.
    #[serde(default)]
    pub script: Option<String>,
}

fn default_constrained() -> bool {
    true
}

impl Scene {
    /// The camera's facing direction projected onto the ground plane:
    /// what "screen up" means for movement and the player's spawn facing.
    /// Zero when the camera looks straight down.
    pub fn camera_forward(&self) -> Vec2 {
        let position = Vec2::new(self.camera.position[0], self.camera.position[2]);
        let target = Vec2::new(self.camera.target[0], self.camera.target[2]);
        (target - position).normalize_or_zero()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scene_survives_being_written_and_read_back() {
        // What the editor's save does. Dynamic has no PartialEq, so the
        // params are compared by what they print as: the values a script
        // reads back with `param` have to be the values that went in.
        let src = r#"(
            camera: (position: (0.0, 6.0, 9.0), target: (0.0, 0.0, 0.0), fov_degrees: 45.0),
            walk_mesh: Some((
                vertices: [(0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (0.0, 1.0, 0.0)],
                triangles: [(0, 1, 2)],
            )),
            depth_range: 12.5,
            pan: Some((plate: (640, 480), window: (320, 240))),
            actors: [
                (
                    model: "models/goblin.glb",
                    position: (-28.3, -106.86),
                    id: Some("goblin"),
                    script: Some("scripts/goblin.rhai"),
                    params: { "item": "bread", "count": 2 },
                ),
            ],
        )"#;
        let before: Scene = ron::from_str(src).unwrap();

        let text = to_ron(&before).unwrap();
        let after: Scene = ron::from_str(&text).unwrap();

        assert_eq!(after.camera, before.camera);
        assert_eq!(after.depth_range, before.depth_range);
        assert_eq!(after.pan, before.pan);
        assert_eq!(after.walk_mesh.map(|m| m.vertices), before.walk_mesh.map(|m| m.vertices));
        assert_eq!(
            after.actors[0].params.values().map(ToString::to_string).collect::<Vec<_>>(),
            before.actors[0].params.values().map(ToString::to_string).collect::<Vec<_>>(),
        );
        assert_eq!(after.actors[0].position, before.actors[0].position);
        assert_eq!(after.actors[0].id, before.actors[0].id);
    }

    #[test]
    fn parsing_scene_ron() {
        let src = r#"(
            background: Some("backgrounds/room1.png"),
            camera: (position: (0.0, 6.0, 9.0), target: (0.0, 0.0, 0.0), fov_degrees: 45.0),
            walk_mesh: Some(WalkMesh(
                vertices: [(-2.0, 0.0, -2.0), (0.0, 0.0, -2.0), (-2.0, 0.0, 0.0)],
                triangles: [(0, 1, 2)],
            )),
        )"#;
        let scene: Scene = ron::from_str(src).unwrap();
        assert_eq!(scene.camera.position, [0.0, 6.0, 9.0]);
        assert_eq!(scene.camera.fov_degrees, 45.0);
        let mesh = scene.walk_mesh.expect("the walk mesh parses");
        assert!(mesh.contains(-1.5, -1.5));
        assert!(!mesh.contains(-0.5, -0.5));
        // The pan spec is optional: absent means the view is pinned.
        assert_eq!(scene.pan, None);
    }

    #[test]
    fn the_walk_mesh_is_optional() {
        // Scenes written before walk meshes existed keep loading, and
        // bound nothing.
        let src = r#"(
            background: None,
            camera: (position: (0.0, 6.0, 9.0), target: (0.0, 0.0, 0.0), fov_degrees: 45.0),
        )"#;
        let scene: Scene = ron::from_str(src).unwrap();
        assert!(scene.walk_mesh.is_none());
    }

    #[test]
    fn a_walk_mesh_round_trips_through_ron() {
        let mut mesh = WalkMesh::default();
        mesh.vertices = vec![[-2.0, 0.5, -2.0], [0.0, 0.0, -2.0], [-2.0, 0.0, 0.0]];
        mesh.triangles = vec![[0, 1, 2]];
        let scene = Scene {
            walk_mesh: Some(mesh),
            ..devroom_scene()
        };
        let text = ron::ser::to_string_pretty(&scene, ron::ser::PrettyConfig::default()).unwrap();
        let reparsed: Scene = ron::from_str(&text).unwrap();
        let mesh = reparsed.walk_mesh.expect("the walk mesh round trips");
        assert_eq!(mesh.vertices.len(), 3);
        assert_eq!(mesh.triangles, [[0, 1, 2]]);
        assert!(mesh.contains(-1.5, -1.5));
        // The skipped acceleration buckets must not leak into the file.
        assert!(!text.contains("buckets"));
    }

    #[test]
    fn parses_the_pan_spec() {
        let src = r#"(
            background: Some("backgrounds/room1.png"),
            camera: (position: (0.0, 6.0, 9.0), target: (0.0, 0.0, 0.0), fov_degrees: 45.0),
            pan: Some((plate: (640, 480), window: (320, 240))),
        )"#;
        let scene: Scene = ron::from_str(src).unwrap();
        let pan = scene.pan.expect("the pan spec parses");
        assert_eq!(pan, crate::scene::PanSpec {
            plate: (640, 480),
            window: (320, 240),
        });
        assert_eq!(pan.travel(), (320.0, 240.0));
    }

    #[test]
    fn parses_teleporters() {
        let src = r#"(
            background: None,
            camera: (position: (0.0, 6.0, 9.0), target: (0.0, 0.0, 0.0), fov_degrees: 45.0),
            walk_mesh: None,
            teleporters: [
                (
                    position: (4.0, 0.0),
                    size: (2.0, 1.0),
                    target: "scenes/room2.scene",
                    arrival: (1.0, 2.0),
                ),
            ],
        )"#;
        let scene: Scene = ron::from_str(src).unwrap();
        let [teleporter] = &scene.teleporters[..] else {
            panic!("expected exactly one teleporter");
        };
        assert_eq!(teleporter.position, [4.0, 0.0]);
        assert_eq!(teleporter.size, [2.0, 1.0]);
        assert_eq!(teleporter.target, "scenes/room2.scene");
        assert_eq!(teleporter.arrival, [1.0, 2.0]);
        assert!(teleporter.contains(4.0, 0.0));
        assert!(!teleporter.contains(0.0, 0.0));
    }

    #[test]
    fn teleporters_default_to_empty_when_omitted() {
        // Scenes written before teleporters existed keep loading.
        let src = r#"(
            background: None,
            camera: (position: (0.0, 6.0, 9.0), target: (0.0, 0.0, 0.0), fov_degrees: 45.0),
            walk_mesh: None,
        )"#;
        let scene: Scene = ron::from_str(src).unwrap();
        assert!(scene.teleporters.is_empty());
    }

    #[test]
    fn teleporters_round_trip_through_ron() {
        let scene = Scene {
            background: None,
            camera: CameraPose {
                position: [0.0, 6.0, 9.0],
                target: [0.0, 0.0, 0.0],
                fov_degrees: 45.0,
            },
            walk_mesh: None,
            teleporters: vec![Teleporter {
                position: [4.0, 0.0],
                size: [2.0, 1.0],
                target: "scenes/room2.scene".into(),
                arrival: [1.0, 2.0],
            }],
            depth_map: None,
            depth_range: 32.0,
            pan: None,
            script: None,
            actors: Vec::new(),
        };
        let text = ron::ser::to_string_pretty(&scene, ron::ser::PrettyConfig::default()).unwrap();
        let reparsed: Scene = ron::from_str(&text).unwrap();
        assert_eq!(reparsed.teleporters, scene.teleporters);
    }

    #[test]
    fn scenes_from_before_the_party_still_load() {
        // character_model left the format when the party took over the
        // player's model; old files carrying it must keep parsing.
        let src = r#"(
            background: None,
            camera: (position: (0.0, 6.0, 9.0), target: (0.0, 0.0, 0.0), fov_degrees: 45.0),
            character_model: Some("models/old.glb"),
        )"#;
        let scene: Scene = ron::from_str(src).unwrap();
        assert_eq!(scene.background, None);
        assert_eq!(scene.script, None);
    }

    #[test]
    fn the_scene_script_is_optional_and_defaults_to_none() {
        let src = r#"(
            background: None,
            camera: (position: (0.0, 6.0, 9.0), target: (0.0, 0.0, 0.0), fov_degrees: 45.0),
            walk_mesh: None,
            script: Some("scripts/room_intro.rhai"),
        )"#;
        let scene: Scene = ron::from_str(src).unwrap();
        assert_eq!(scene.script.as_deref(), Some("scripts/room_intro.rhai"));

        // Scenes written before the field existed keep loading.
        let src = r#"(
            background: None,
            camera: (position: (0.0, 6.0, 9.0), target: (0.0, 0.0, 0.0), fov_degrees: 45.0),
            walk_mesh: None,
        )"#;
        let scene: Scene = ron::from_str(src).unwrap();
        assert_eq!(scene.script, None);
    }

    #[test]
    fn teleporter_contains_covers_the_rect_interior() {
        let teleporter = Teleporter {
            position: [4.0, 0.0],
            size: [2.0, 1.0],
            target: String::new(),
            arrival: [0.0, 0.0],
        };
        // center and interior
        assert!(teleporter.contains(4.0, 0.0));
        assert!(teleporter.contains(3.1, 0.4));
        // outside each side
        assert!(!teleporter.contains(2.9, 0.0));
        assert!(!teleporter.contains(5.1, 0.0));
        assert!(!teleporter.contains(4.0, 0.6));
        assert!(!teleporter.contains(4.0, -0.6));
        // low edges inclusive, high edges exclusive
        assert!(teleporter.contains(3.0, 0.0));
        assert!(teleporter.contains(4.0, -0.5));
        assert!(!teleporter.contains(5.0, 0.0));
        assert!(!teleporter.contains(4.0, 0.5));
    }

    #[test]
    fn every_shipped_scene_parses() {
        for name in SHIPPED_SCENES {
            read_shipped(name);
        }
    }

    #[test]
    fn shipped_walk_meshes_carry_their_actors() {
        // An actor placed off its scene's mesh stands on nothing (or is
        // clamped onto it on the first scripted step), so every placed
        // character has to start on the ground the player walks on.
        for name in SHIPPED_SCENES {
            let scene = read_shipped(name);
            let Some(mesh) = &scene.walk_mesh else {
                continue;
            };
            for actor in &scene.actors {
                assert!(
                    mesh.contains(actor.position[0], actor.position[1]),
                    "{name}: actor {:?} at {:?} is off the walk mesh",
                    actor.id,
                    actor.position
                );
            }
        }
    }

    #[test]
    fn a_shipped_walk_mesh_carries_the_ground_its_characters_stand_on() {
        // The village terrain is not flat, which is the whole reason the
        // grid was replaced: a character has to ride the surface, not a
        // constant height.
        let mesh = read_shipped("Village_Entrance.scene")
            .walk_mesh
            .expect("the village entrance exports a walk mesh");
        let heights: Vec<f32> = [
            (-28.3, -106.86),
            (-30.0, -105.0),
            (-26.5, -105.0),
            (-31.5, -108.5),
        ]
        .iter()
        .map(|(x, z)| mesh.height_at(*x, *z).expect("the cast stands on it"))
        .collect();
        assert!(heights.iter().all(|h| h.is_finite() && *h > 0.0));
        let lowest = heights.iter().copied().fold(f32::MAX, f32::min);
        let highest = heights.iter().copied().fold(f32::MIN, f32::max);
        assert!(
            highest - lowest > 0.2,
            "the arrival, the goblin and the chests stand at {heights:?} — a \
             flat ground would mean the terrain never made it into the mesh"
        );
    }

    #[test]
    fn the_shipped_teleporter_pair_is_consistent() {
        // Both halves must parse, and each side's arrival point must sit
        // inside the destination's trigger region, or the player re-triggers
        // the moment the transition lands.
        let devroom = read_shipped("devroom.scene");
        let room2 = read_shipped("room2.scene");

        let [to_room2] = devroom
            .teleporters
            .iter()
            .filter(|t| t.target == "scenes/room2.scene")
            .collect::<Vec<_>>()[..]
        else {
            panic!("devroom ships one teleporter to room2");
        };
        assert_eq!(to_room2.target, "scenes/room2.scene");
        let [from_room2] = &room2.teleporters[..] else {
            panic!("room2 ships exactly one return teleporter");
        };
        assert_eq!(from_room2.target, "scenes/devroom.scene");
        assert!(from_room2.contains(to_room2.arrival[0], to_room2.arrival[1]));
        assert!(to_room2.contains(from_room2.arrival[0], from_room2.arrival[1]));
    }

    #[test]
    fn camera_forward_points_from_the_camera_toward_its_target() {
        // devroom-style: camera above +Z looking at the origin -> -Z.
        let mut scene = devroom_scene();
        assert_eq!(scene.camera_forward(), Vec2::NEG_Y);
        // room2-style: camera above -Z -> +Z.
        scene.camera.position = [0.0, 7.0, -6.0];
        assert_eq!(scene.camera_forward(), Vec2::Y);
        // Looking straight down has no ground facing.
        scene.camera.position = [0.0, 6.0, 0.0];
        scene.camera.target = [0.0, 0.0, 0.0];
        assert_eq!(scene.camera_forward(), Vec2::ZERO);
    }

    fn devroom_scene() -> Scene {
        Scene {
            background: None,
            camera: CameraPose {
                position: [0.0, 6.0, 9.0],
                target: [0.0, 0.0, 0.0],
                fov_degrees: 45.0,
            },
            walk_mesh: None,
            teleporters: Vec::new(),
            depth_map: None,
            depth_range: 32.0,
            pan: None,
            script: None,
            actors: Vec::new(),
        }
    }

    #[test]
    fn parses_actors() {
        let src = r#"(
            background: None,
            camera: (position: (0.0, 6.0, 9.0), target: (0.0, 0.0, 0.0), fov_degrees: 45.0),
            walk_mesh: None,
            actors: [
                (
                    model: "models/goblin.glb",
                    position: (1.0, 2.0),
                    script: Some("scripts/goblin.rhai"),
                ),
                (
                    model: "models/statue.glb",
                    position: (3.0, 4.0),
                ),
            ],
        )"#;
        let scene: Scene = ron::from_str(src).unwrap();
        let [with_script, without] = &scene.actors[..] else {
            panic!("expected exactly two actors");
        };
        assert_eq!(with_script.model, "models/goblin.glb");
        assert_eq!(with_script.position, [1.0, 2.0]);
        assert_eq!(with_script.script.as_deref(), Some("scripts/goblin.rhai"));
        // Actors stay on the walk mesh unless a script needs them off it.
        assert!(with_script.constrained);
        assert_eq!(without.model, "models/statue.glb");
        assert_eq!(without.position, [3.0, 4.0]);
        assert_eq!(without.script, None);
        assert!(without.constrained);
    }

    #[test]
    fn an_actor_can_opt_out_of_the_walk_mesh() {
        let src = r#"(
            background: None,
            camera: (position: (0.0, 6.0, 9.0), target: (0.0, 0.0, 0.0), fov_degrees: 45.0),
            actors: [
                (
                    model: "models/bird.glb",
                    position: (3.0, 4.0),
                    constrained: false,
                ),
            ],
        )"#;
        let scene: Scene = ron::from_str(src).unwrap();
        assert!(!scene.actors[0].constrained);
    }

    #[test]
    fn actors_default_to_empty_when_omitted() {
        let src = r#"(
            background: None,
            camera: (position: (0.0, 6.0, 9.0), target: (0.0, 0.0, 0.0), fov_degrees: 45.0),
            walk_mesh: None,
        )"#;
        let scene: Scene = ron::from_str(src).unwrap();
        assert!(scene.actors.is_empty());
    }

    #[test]
    fn actors_round_trip_through_ron() {
        let mut scene = devroom_scene();
        scene.actors = vec![Actor {
            model: "models/goblin.glb".into(),
            position: [1.0, 2.0],
            id: Some("goblin".into()),
            shared: false,
            params: BTreeMap::new(),
            script: Some("scripts/goblin.rhai".into()),
            facing: None,
            constrained: true,
        }];
        let text = ron::ser::to_string_pretty(&scene, ron::ser::PrettyConfig::default()).unwrap();
        let reparsed: Scene = ron::from_str(&text).unwrap();
        // Actor holds Dynamic params, which have no structural
        // equality; the round trip is checked through the format.
        let again =
            ron::ser::to_string_pretty(&reparsed, ron::ser::PrettyConfig::default()).unwrap();
        assert_eq!(again, text);
    }

    #[test]
    fn ships_a_valid_devroom_scene() {
        // The file the game loads at startup must stay parseable and
        // carry a camera, whatever else it holds.
        let scene = read_shipped("devroom.scene");
        assert!(scene.camera.fov_degrees > 0.0);
    }
}

//! Showing a scene: its background as geometry, and its walk mesh.
//!
//! The editor opens one scene file and keeps it open. The game's scene
//! application is a different job — it stages a player, scripts, actors
//! and a transition — so none of that comes along; what is left is
//! "rebuild whatever this file describes", and that rebuild is re-run
//! whenever the file changes on disk. Re-exporting the blend and saving
//! the scene file therefore updates the view without a keypress.

use std::time::{Duration, SystemTime};

use bevy::asset::{AssetEvent, AssetId};
use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use wakeful::assets::assets_root;
use wakeful::depth_card_mesh;
use wakeful::scene::{CameraPose, Scene};

use crate::actors::Selected;
use crate::camera::EditorCamera;

/// How often the open scene file is checked for a newer write. Half a
/// second is invisible while a blend re-exports and instant once it
/// finishes.
const RELOAD_POLL: Duration = Duration::from_millis(500);

/// How far in front of the camera pose a background with no depth map is
/// hung. There is no depth to unproject, so it is a painting on a stand.
const FLAT_BACKGROUND_DISTANCE: f32 = 20.0;

/// The scene file the editor has open, by its path under `assets/`.
#[derive(Resource)]
pub struct OpenScene {
    pub path: String,
    /// The loaded scene, which is what every system edits: the editor
    /// holds the asset rather than a copy of it, so an edit is the scene.
    pub handle: Handle<Scene>,
}

impl OpenScene {
    pub fn new(assets: &AssetServer, path: String) -> Self {
        let handle = assets.load(path.clone());
        OpenScene { path, handle }
    }

    /// The file's name on its own, which is how the editor names a scene in
    /// menus and titles.
    pub fn name(&self) -> String {
        self.path
            .rsplit('/')
            .next()
            .unwrap_or(&self.path)
            .trim_end_matches(".scene")
            .to_owned()
    }

    /// Switches to another scene file, as File > Open and Save As do.
    pub fn open(&mut self, assets: &AssetServer, path: String) {
        self.handle = assets.load(path.clone());
        self.path = path;
    }
}

/// The scene as the editor holds it: a working copy of the asset, which
/// the panels edit.
///
/// The asset itself is only how a scene arrives. Editing it in place
/// would be tidier and does not work: a mutable borrow of an asset emits
/// `AssetEvent::Modified`, so every drag of a number would look to the
/// reloader exactly like the file changing, and the editor would rebuild
/// the world under the pointer sixty times a second.
#[derive(Resource, Default)]
pub struct Working(pub Option<Scene>);

/// Everything the current scene put in the world, so a reload can take
/// it all down again.
#[derive(Component)]
pub struct SceneContent;

/// How the background becomes geometry: unprojected from a depth map,
/// or — with no depth to read — a painting hung in front of the camera.
enum Background {
    /// The plate's pixels, pushed out along their own rays.
    Card { depth: Handle<Image>, depth_range: f32 },
    /// A flat quad; `build_flat` sizes it from the plate once loaded.
    Flat,
}

/// A background waiting for its image(s) to load.
#[derive(Component)]
pub struct Pending {
    pose: CameraPose,
    background: Handle<Image>,
    how: Background,
}

/// How far the open scene file has been read: whether it has been shown
/// at all, and the write it was last read from. Bevy's change detection
/// on the asset collection stays "changed" for a frame or two after a
/// load, and rebuilding twice in a row is what tore the pending
/// background down from under itself; the asset's own events say once.
#[derive(Resource, Default)]
pub struct Shown {
    read: bool,
    /// The write the scene file was last read from, or last saved to.
    pub written: Option<SystemTime>,
    since_poll: Duration,
    /// The scene the camera was posed for. A reload of the same scene
    /// leaves the camera where the editor's owner has flown it; only a
    /// different scene puts it back where that scene's camera is.
    posed_for: Option<AssetId<Scene>>,
}

impl Shown {
    /// Forgets what has been read and shown, so the next scene — opened
    /// from the menu, or written under a new name — is applied from
    /// scratch and posed on the camera.
    pub fn forget(&mut self) {
        self.read = false;
        self.written = None;
        self.posed_for = None;
    }
}

/// Re-reads the open scene file when it changes on disk, or when F5 asks.
/// Bevy's own file watching is a cargo feature this project does not
/// enable, and an editor that cannot see a re-export of the blend is not
/// much of an editor — so the file's own write time is polled, which
/// works in any build.
pub fn reload_when_changed(
    time: Res<Time>,
    assets: Res<AssetServer>,
    open: Res<OpenScene>,
    keys: Res<ButtonInput<KeyCode>>,
    mut shown: ResMut<Shown>,
) {
    shown.since_poll += time.delta();
    if !keys.just_pressed(KeyCode::F5) && shown.since_poll < RELOAD_POLL {
        return;
    }
    shown.since_poll = Duration::ZERO;
    let path = assets_root().join(&open.path);
    let Ok(metadata) = path.metadata() else {
        return;
    };
    let Ok(written) = metadata.modified() else {
        return;
    };
    // A first read is not a change: the scene has not been shown yet.
    if shown.written == Some(written) {
        return;
    }
    let first_read = shown.written.is_none();
    shown.written = Some(written);
    if first_read {
        return;
    }
    info!("[editor] {} changed, reloading", open.path);
    assets.reload(&open.path);
}

/// Applies the open scene when its file first lands and again whenever
/// it changes on disk — re-exporting the blend and saving the scene file
/// is all it takes to see the result. Puts the camera where the game
/// camera is, looking where it looks, so the editor opens on the game's
/// own framing.
#[allow(clippy::too_many_arguments)]
pub fn apply_scene(
    mut commands: Commands,
    assets: Res<AssetServer>,
    scenes: Res<Assets<Scene>>,
    open: Res<OpenScene>,
    mut events: MessageReader<AssetEvent<Scene>>,
    mut shown: ResMut<Shown>,
    mut working: ResMut<Working>,
    mut selected: ResMut<Selected>,
    content: Query<Entity, With<SceneContent>>,
    mut cameras: Query<(&mut Transform, &mut Projection), With<EditorCamera>>,
) {
    let reloaded = events.read().any(|event| match event {
        AssetEvent::Added { id } | AssetEvent::Modified { id } => *id == open.handle.id(),
        _ => false,
    });
    // Apply on the first load and on every change to the file, and on no
    // other frame: reapplying unconditionally would despawn the
    // background the builder had just made, every frame, forever.
    if shown.read && !reloaded {
        return;
    }
    let Some(scene) = scenes.get(&open.handle) else {
        return;
    };
    shown.read = true;
    // A scene edited outside the editor replaces whatever was here: an
    // external change wins, and the models are rebuilt from it below.
    working.0 = Some(scene.clone());
    // A scene that came back shorter than the one on screen has taken
    // actors away, and a selection that kept pointing would be describing
    // whatever now stands there.
    selected.retain(scene.actors.len());
    for entity in &content {
        commands.entity(entity).despawn();
    }
    info!(
        "{}: camera {:?} -> {:?} at {:.0} degrees, {} walk-mesh vertices, {} teleporters, {} actors",
        open.path,
        scene.camera.position,
        scene.camera.target,
        scene.camera.fov_degrees,
        scene
            .walk_mesh
            .as_ref()
            .map_or(0, |mesh| mesh.vertices.len()),
        scene.teleporters.len(),
        scene.actors.len(),
    );
    // Once per scene, not once per reload: a scene saved from the editor
    // is re-read, and a camera yanked back to the scene's pose at that
    // moment would be the tool moving the view out from under you.
    if shown.posed_for != Some(open.handle.id()) {
        shown.posed_for = Some(open.handle.id());
        if let Ok((mut transform, mut projection)) = cameras.single_mut() {
            *transform = Transform::from_translation(scene.camera.position.into())
                .looking_at(scene.camera.target.into(), Vec3::Y);
            // Through the scene's own fov, the card is exactly the plate:
            // the editor opens on the game's framing, and the scroll
            // wheel is there for the times that is too wide to work in.
            if let Projection::Perspective(perspective) = &mut *projection {
                perspective.fov = scene.camera.fov_degrees.to_radians();
            }
        }
    }
    match (&scene.background, &scene.depth_map) {
        (Some(background), Some(depth)) => {
            commands.spawn((
                Pending {
                    pose: scene.camera,
                    background: assets.load(background),
                    how: Background::Card {
                        depth: assets.load(depth),
                        depth_range: scene.depth_range,
                    },
                },
                SceneContent,
            ));
        }
        (Some(background), None) => {
            warn!("{}: no depth map, so the plate is shown flat", open.path);
            commands.spawn((
                Pending {
                    pose: scene.camera,
                    background: assets.load(background),
                    how: Background::Flat,
                },
                SceneContent,
            ));
        }
        (None, _) => warn!("{}: no background, nothing to show", open.path),
    }
    if scene.walk_mesh.is_none() {
        warn!("{}: no walk mesh, nothing to draw", open.path);
    }
}

/// Builds pending backgrounds once their images land. The try_ variants
/// are not paranoia: a reload despawns a background that was still
/// waiting for its plate, and plain `insert` on the corpse is a panic
/// rather than a shrug.
pub fn build_backgrounds(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    images: Res<Assets<Image>>,
    pending: Query<(Entity, &Pending)>,
) {
    for (entity, waiting) in &pending {
        let Some(image) = images.get(&waiting.background) else {
            continue;
        };
        let mesh = match &waiting.how {
            Background::Card {
                depth,
                depth_range,
            } => {
                let Some(depth) = images.get(depth) else {
                    continue;
                };
                match depth_card_mesh::build(depth, &waiting.pose, *depth_range) {
                    Some(mesh) => mesh,
                    None => {
                        warn!("the depth map could not be read; showing nothing");
                        continue;
                    }
                }
            }
            Background::Flat => flat_quad(&waiting.pose, image.width() as f32, image.height() as f32),
        };
        commands.entity(entity).try_remove::<Pending>();
        let vertices = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .map_or(0, |values| values.len());
        info!("[editor] background: {vertices} vertices");
        commands.entity(entity).try_insert((
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color_texture: Some(waiting.background.clone()),
                // The background is pre-lit paint either way.
                unlit: true,
                ..default()
            })),
        ));
    }
}

/// A textured quad standing where the background would be, cut to the
/// camera's frustum at [`FLAT_BACKGROUND_DISTANCE`] and to the plate's
/// own proportions, so it fills the view the way the card does.
fn flat_quad(pose: &CameraPose, plate_width: f32, plate_height: f32) -> Mesh {
    let position = Vec3::from(pose.position);
    let forward = (Vec3::from(pose.target) - position).normalize();
    let half_height = (pose.fov_degrees.to_radians() * 0.5).tan() * FLAT_BACKGROUND_DISTANCE;
    // The frustum is as wide as the window; the plate is as wide as it
    // was baked. Taking the narrower of the two fills the view without
    // stretching the art.
    let frustum_half_width = half_height * 4.0 / 3.0;
    let plate_half_width = half_height * plate_width / plate_height.max(1.0);
    let half_width = frustum_half_width.min(plate_half_width);
    let half_height = half_width * plate_height / plate_width.max(1.0);
    let corners = [
        Vec3::new(-half_width, -half_height, 0.0),
        Vec3::new(half_width, -half_height, 0.0),
        Vec3::new(half_width, half_height, 0.0),
        Vec3::new(-half_width, half_height, 0.0),
    ];
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        corners
            .map(|corner| position + forward * FLAT_BACKGROUND_DISTANCE + corner)
            .to_vec(),
    );
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_UV_0,
        vec![[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]],
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![forward; 4]);
    mesh.insert_indices(Indices::U32(vec![0, 1, 2, 0, 2, 3]));
    mesh
}

/// Draws the walk mesh every frame. The editor has one job here and this
/// is the point of it, so unlike the game there is no toggle: the mesh is
/// always on screen.
pub fn draw_walk_mesh(working: Res<Working>, mut gizmos: Gizmos) {
    let Some(mesh) = working
        .0
        .as_ref()
        .and_then(|scene| scene.walk_mesh.as_ref())
    else {
        return;
    };
    wakeful::walkmesh::draw(&mut gizmos, mesh);
}

//! The scene's actors, shown where they stand.
//!
//! The editor holds the open [`Scene`] asset as the truth and the entities
//! here as its view of it: an actor's position is written to its model
//! every frame from the scene file, so a number dragged in the properties
//! window moves the model immediately, and a list that grows or shrinks is
//! reconciled rather than rebuilt. Nothing here runs scripts or animation —
//! an actor stands where it is placed, which is the point of an authoring
//! view.
//!
//! Which entry of the list an entity is, is its index. There is no stable
//! id across an edit of the list, and the list is short enough that a
//! linear reconciliation is free.

use bevy::gltf::Gltf;
use bevy::prelude::*;

use wakeful::scene::{Actor, Scene};
use wakeful::walkmesh;

use crate::scene_view::Working;

/// Half the width and depth of the box drawn around a selected actor, and
/// its height above the ground. Roughly a person: wide enough to click,
/// tall enough to see what is selected from across the street.
pub const BOX_HALF_WIDTH: f32 = 0.45;
pub const BOX_HEIGHT: f32 = 1.6;

/// An actor standing in the viewport, and which entry of the scene's list
/// it is. The model is remembered so a reload that swaps one actor's model
/// rebuilds it instead of leaving the old one standing there.
#[derive(Component)]
pub struct EditorActor {
    pub index: usize,
    model: String,
}

/// The glTF a new actor is waiting for; removed once it has been attached.
#[derive(Component)]
pub struct PendingModel(Handle<Gltf>);

/// The actor the properties window is showing, by its index in the
/// scene's list.
#[derive(Resource, Default)]
pub struct Selected(pub Option<usize>);

/// Where the camera is pointing, as a place on the ground.
///
/// This is what a new actor lands on: the view already answers "where
/// here", and asking the pointer for a position would mean the menu had to
/// know where the cursor was, which a right-click menu does not.
#[derive(Resource, Default)]
pub struct GroundPointer(pub Option<Vec3>);

/// What the reconciler decided: entries to build, and entries to take
/// down.
#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    spawn: Vec<usize>,
    despawn: Vec<usize>,
}

/// Which entries of the scene's actor list have no model standing for
/// them, and which models stand for an entry that is gone or now holds
/// something else.
///
/// Comparing the model as well as the index is what makes a reload honest:
/// re-exporting a blend cannot change the list, but an edited scene file
/// can, and a model swapped onto an existing index has to follow.
pub fn reconcile(standing: &[(usize, &str)], wanted: &[Actor]) -> Plan {
    let mut plan = Plan::default();
    for (index, actor) in wanted.iter().enumerate() {
        let standing_here = standing
            .iter()
            .find(|(at, _)| *at == index)
            .map(|(_, model)| *model);
        if standing_here != Some(actor.model.as_str()) {
            plan.spawn.push(index);
        }
    }
    plan.despawn = standing
        .iter()
        .filter(|(at, _)| *at >= wanted.len())
        .map(|(at, _)| *at)
        .collect();
    plan
}

/// Brings the standing actors in line with the scene file: builds what is
/// missing, takes down what is no longer wanted, and writes every actor's
/// placement onto its model.
pub fn sync_actors(
    mut commands: Commands,
    assets: Res<AssetServer>,
    working: Res<Working>,
    standing: Query<(Entity, &EditorActor)>,
    mut transforms: Query<(&mut Transform, &EditorActor), Without<PendingModel>>,
) {
    let Some(scene) = working.0.as_ref() else {
        return;
    };
    let in_place: Vec<(usize, &str)> = standing
        .iter()
        .map(|(_, actor)| (actor.index, actor.model.as_str()))
        .collect();
    let plan = reconcile(&in_place, &scene.actors);
    for index in plan.despawn {
        if let Some((entity, _)) = standing.iter().find(|(_, actor)| actor.index == index) {
            commands.entity(entity).despawn();
        }
    }
    for index in plan.spawn {
        let Some(actor) = scene.actors.get(index) else {
            continue;
        };
        let Some(placement) = placement(scene, index) else {
            continue;
        };
        commands.spawn((
            EditorActor {
                index,
                model: actor.model.clone(),
            },
            // A glTF scene root propagates visibility, so the entity it
            // hangs from has to carry one of its own.
            Visibility::default(),
            PendingModel(assets.load(model_path(&actor.model))),
            placement,
        ));
    }
    // The transform write is what makes an edit in the properties window
    // move the model as the number is dragged, and what puts a reloaded
    // actor back where the file says.
    for (mut transform, actor) in &mut transforms {
        if let Some(wanted) = placement(scene, actor.index) {
            *transform = wanted;
        }
    }
}

/// The transform an actor stands at: on the ground, facing where its
/// scene says.
fn placement(scene: &Scene, index: usize) -> Option<Transform> {
    let actor = scene.actors.get(index)?;
    let at = Vec2::new(actor.position[0], actor.position[1]);
    let ground = walkmesh::ground_height(scene.walk_mesh.as_ref(), at.x, at.y, at);
    let rotation = match actor.facing {
        Some(degrees) => Quat::from_rotation_y(degrees.to_radians()),
        // No pinned facing means the scene camera's, exactly as the game
        // reads it: the editor should show what the game will do.
        None => facing_rotation(scene.camera_forward()),
    };
    Some(Transform::from_xyz(at.x, ground, at.y).with_rotation(rotation))
}

/// The rotation that points a model's front (+Z, the glTF model-forward)
/// along a heading on the ground, keeping it upright. This is the game's
/// own rule, copied rather than imported: the editor binary and the game
/// share the scene format, not the game's systems, and an editor that
/// previewed facings by a different rule would be lying about the result.
fn facing_rotation(heading: Vec2) -> Quat {
    if heading == Vec2::ZERO {
        return Quat::IDENTITY;
    }
    Quat::from_rotation_arc(Vec3::Z, Vec3::new(heading.x, 0.0, heading.y).normalize())
}

/// The asset path for an actor's model, which the scene file names by file
/// alone.
fn model_path(model: &str) -> String {
    if model.contains('/') {
        return model.to_owned();
    }
    format!("models/{model}")
}

/// Attaches each new actor's model once its glTF has loaded. The glTF's
/// own scene root is spawned as the child rather than its mesh, so a
/// skinned model brings the skeleton it needs to stand up in bind pose.
pub fn attach_models(
    mut commands: Commands,
    gltfs: Res<Assets<Gltf>>,
    waiting: Query<(Entity, &PendingModel)>,
) {
    for (entity, model) in &waiting {
        let Some(gltf) = gltfs.get(&model.0) else {
            continue;
        };
        let Some(scene) = gltf
            .default_scene
            .clone()
            .or_else(|| gltf.scenes.first().cloned())
        else {
            continue;
        };
        commands.entity(entity).with_child(WorldAssetRoot(scene));
        commands.entity(entity).remove::<PendingModel>();
    }
}

/// Draws a box around the selected actor. Gizmos are drawn without a depth
/// test, so the selection shows through the background card — which is the
/// whole difficulty with a scene this flat and this far away.
pub fn draw_selection(
    selected: Res<Selected>,
    standing: Query<(&EditorActor, &Transform)>,
    mut gizmos: Gizmos,
) {
    let Some(index) = selected.0 else {
        return;
    };
    let Some((_, transform)) = standing.iter().find(|(actor, _)| actor.index == index) else {
        return;
    };
    let color = Color::srgb(1.0, 0.65, 0.15);
    let base = transform.translation;
    let low = base.y + 0.02;
    let high = base.y + BOX_HEIGHT;
    let corner = |dx: f32, dz: f32, y: f32| base + Vec3::new(dx, y, dz);
    let (w, d) = (BOX_HALF_WIDTH, BOX_HALF_WIDTH);
    let edges = [
        // The four uprights.
        (corner(-w, -d, low), corner(-w, -d, high)),
        (corner(w, -d, low), corner(w, -d, high)),
        (corner(-w, d, low), corner(-w, d, high)),
        (corner(w, d, low), corner(w, d, high)),
        // The base it stands on, and the line across its head, so the box
        // reads as a thing rather than a post.
        (corner(-w, -d, low), corner(w, -d, low)),
        (corner(-w, d, low), corner(w, d, low)),
        (corner(-w, -d, low), corner(-w, d, low)),
        (corner(w, -d, low), corner(w, d, low)),
        (corner(-w, -d, high), corner(w, -d, high)),
        (corner(-w, d, high), corner(w, d, high)),
        (corner(-w, -d, high), corner(-w, d, high)),
        (corner(w, -d, high), corner(w, d, high)),
    ];
    for (from, to) in edges {
        gizmos.line(from, to, color);
    }
}

/// Where the camera is pointing, on the ground.
pub fn track_ground_pointer(
    working: Res<Working>,
    cameras: Query<&GlobalTransform, With<crate::camera::EditorCamera>>,
    mut pointer: ResMut<GroundPointer>,
) {
    pointer.0 = cameras.single().ok().and_then(|camera| {
        let forward: Vec3 = camera.forward().into();
        let eye = camera.translation();
        let mesh = working.0.as_ref()?.walk_mesh.as_ref()?;
        // No ground under the crosshair: aim a few paces ahead instead,
        // so a camera looking at the sky still has somewhere to put it.
        mesh.raycast(eye, forward)
            .or_else(|| mesh.raycast(eye + forward * 6.0, Vec3::NEG_Y))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actor(model: &str) -> Actor {
        Actor {
            id: None,
            shared: false,
            params: Default::default(),
            model: model.to_owned(),
            position: [0.0, 0.0],
            facing: None,
            constrained: true,
            script: None,
        }
    }

    #[test]
    fn an_empty_scene_wants_no_actors() {
        let plan = reconcile(&[], &[]);
        assert_eq!(plan, Plan::default());
    }

    #[test]
    fn every_actor_starts_out_needing_a_model() {
        let wanted = [actor("goblin.glb"), actor("chest.glb")];
        let plan = reconcile(&[], &wanted);
        assert_eq!(plan.spawn, vec![0, 1]);
        assert!(plan.despawn.is_empty());
    }

    #[test]
    fn actors_already_standing_are_left_alone() {
        let wanted = [actor("goblin.glb"), actor("chest.glb")];
        let standing = [(0usize, "goblin.glb"), (1usize, "chest.glb")];
        let plan = reconcile(&standing, &wanted);
        assert_eq!(plan, Plan::default());
    }

    #[test]
    fn a_grown_list_adds_only_the_new_actors() {
        let wanted = [actor("goblin.glb"), actor("chest.glb"), actor("statue.glb")];
        let standing = [(0usize, "goblin.glb")];
        let plan = reconcile(&standing, &wanted);
        assert_eq!(plan.spawn, vec![1, 2]);
        assert!(plan.despawn.is_empty());
    }

    #[test]
    fn a_shrunk_list_takes_the_leftovers_down() {
        let wanted = [actor("goblin.glb")];
        let standing = [(0usize, "goblin.glb"), (1usize, "chest.glb")];
        let plan = reconcile(&standing, &wanted);
        assert!(plan.spawn.is_empty());
        assert_eq!(plan.despawn, vec![1]);
    }

    #[test]
    fn an_actor_whose_model_changed_stands_again() {
        // A scene file edited by hand swaps one model for another on the
        // same index; the old model has to come down and the new one go
        // up, or the edit is invisible in the viewport.
        let wanted = [actor("goblin.glb"), actor("statue.glb")];
        let standing = [(0usize, "goblin.glb"), (1usize, "chest.glb")];
        let plan = reconcile(&standing, &wanted);
        assert_eq!(plan.spawn, vec![1]);
        assert!(plan.despawn.is_empty());
    }

    #[test]
    fn a_model_named_by_path_is_left_alone() {
        assert_eq!(model_path("goblin.glb"), "models/goblin.glb");
        assert_eq!(model_path("models/custom.glb"), "models/custom.glb");
    }
}
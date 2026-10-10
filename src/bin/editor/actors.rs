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
use bevy_egui::egui;

use wakeful::scene::{Actor, Scene};
use wakeful::walkmesh;

use crate::scene_view::Working;

/// Half the width and depth of the box drawn around a selected actor, and
/// its height above the ground. Roughly a person: wide enough to click,
/// tall enough to see what is selected from across the street.
pub const BOX_HALF_WIDTH: f32 = 0.45;
pub const BOX_HEIGHT: f32 = 1.6;

/// The eight corners of the box an actor is picked within and drawn with,
/// base ring first so the two rings line up: corner 4 is corner 0 raised.
///
/// One shape for both, so a click lands where the outline is. The base ring
/// rides 2cm off the ground it was snapped to rather than sitting on it,
/// which keeps an outline on a lit surface from fighting the mesh.
pub fn box_corners(at: Vec3) -> [Vec3; 8] {
    let w = BOX_HALF_WIDTH;
    let lip = 0.02;
    [
        Vec3::new(-w, lip, -w),
        Vec3::new(w, lip, -w),
        Vec3::new(w, lip, w),
        Vec3::new(-w, lip, w),
        Vec3::new(-w, BOX_HEIGHT, -w),
        Vec3::new(w, BOX_HEIGHT, -w),
        Vec3::new(w, BOX_HEIGHT, w),
        Vec3::new(-w, BOX_HEIGHT, w),
    ]
    .map(|corner| at + corner)
}

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

/// The actors selected, by their index in the scene's list.
///
/// The order is the whole of "which one is primary": the most recently
/// picked is last, the properties window shows and edits that one, and the
/// group moves around it. It is a list rather than a set so that picking a
/// second actor and then a first one leaves the second as the primary.
#[derive(Resource, Default)]
pub struct Selected(Vec<usize>);

impl Selected {
    /// A plain click: this one and nothing else.
    pub fn only(&mut self, index: usize) {
        self.0.clear();
        self.0.push(index);
    }

    /// Shift-click: this one in or out, the rest untouched.
    pub fn toggle(&mut self, index: usize) {
        match self.0.iter().position(|picked| *picked == index) {
            Some(at) => {
                self.0.remove(at);
            }
            None => self.0.push(index),
        }
    }

    /// Takes everything the brush covers, in the order the boxes were
    /// given. A brush held with shift adds to what is already selected
    /// rather than replacing it.
    pub fn sweep(
        &mut self,
        brush: egui::Rect,
        boxes: impl Iterator<Item = (usize, egui::Rect)>,
        additive: bool,
    ) {
        // Sorted, so the primary after a sweep is always the last actor in
        // the list rather than whichever box the world happened to yield
        // first.
        let mut covered: Vec<usize> = boxes
            .filter(|(_, rect)| brush.intersects(*rect))
            .map(|(index, _)| index)
            .collect();
        covered.sort_unstable();
        if !additive {
            self.0.clear();
        }
        for index in covered {
            self.toggle(index);
        }
    }

    /// Forgets the indices a scene that has just been reloaded no longer
    /// has. Without this a selection would go on pointing at whatever now
    /// stands where the old actors did.
    pub fn retain(&mut self, count: usize) {
        self.0.retain(|index| *index < count);
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    /// The actor everything else is judged against: the properties
    /// window's subject, and the one whose numbers it shows.
    pub fn primary(&self) -> Option<usize> {
        self.0.last().copied()
    }

    pub fn contains(&self, index: usize) -> bool {
        self.0.contains(&index)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The selection as a slice, in pick order.
    pub fn picked(&self) -> &[usize] {
        &self.0
    }
}

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

/// Draws a box around every selected actor. Gizmos are drawn without a
/// depth test, so the selection shows through the background card — which
/// is the whole difficulty with a scene this flat and this far away.
///
/// The primary is drawn at full strength and the rest dimmer, so the one
/// the properties window is about is obvious without reading the list.
pub fn draw_selection(
    selected: Res<Selected>,
    standing: Query<(&EditorActor, &Transform)>,
    mut gizmos: Gizmos,
) {
    let primary = selected.primary();
    for (actor, transform) in standing.iter() {
        let index = actor.index;
        if !selected.contains(index) {
            continue;
        }
        let color = if Some(index) == primary {
            Color::srgb(1.0, 0.65, 0.15)
        } else {
            Color::srgba(1.0, 0.65, 0.15, 0.55)
        };
        actor_box_edges(&mut gizmos, transform.translation, color);
    }
}

/// The edges of the box an actor is picked within, which is also the box
/// its selection is drawn as: one shape for both, so a click lands where
/// the outline is.
fn actor_box_edges(gizmos: &mut Gizmos, base: Vec3, color: Color) {
    let corners = box_corners(base);
    // The four uprights, the ring at the feet, the ring at the head: twelve
    // lines is the least that reads as a box.
    let edges = (0..4)
        .flat_map(|at| [(corners[at], corners[at + 4])])
        .chain((0..4).flat_map(|at| [(corners[at], corners[(at + 1) % 4])]))
        .chain((0..4).flat_map(|at| {
            [(
                corners[at + 4],
                corners[4 + (at + 1) % 4],
            )]
        }));
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

    /// A rectangle ten by ten, at a spot.
    fn rect(x: f32, y: f32) -> egui::Rect {
        egui::Rect::from_min_max(egui::pos2(x, y), egui::pos2(x + 10.0, y + 10.0))
    }

    #[test]
    fn a_plain_pick_leaves_one_actor_selected() {
        let mut selected = Selected::default();
        selected.only(0);
        selected.only(2);
        assert_eq!(selected.picked(), [2]);
        assert_eq!(selected.primary(), Some(2));
    }

    #[test]
    fn shift_picking_joins_and_leaves_the_others_alone() {
        let mut selected = Selected::default();
        selected.only(0);
        selected.toggle(2);
        assert_eq!(selected.picked(), [0, 2]);
        assert!(selected.contains(0));
        // Picking the primary again takes it back out.
        selected.toggle(0);
        assert_eq!(selected.picked(), [2]);
    }

    #[test]
    fn the_primary_is_whoever_was_picked_last() {
        // Which actor the properties window is about, and which one a group
        // moves around, has to survive picking out of order.
        let mut selected = Selected::default();
        selected.only(3);
        selected.toggle(1);
        assert_eq!(selected.primary(), Some(1));
        selected.toggle(3);
        assert_eq!(selected.primary(), Some(1));
    }

    #[test]
    fn picking_the_same_actor_twice_ends_with_nothing() {
        let mut selected = Selected::default();
        selected.toggle(1);
        assert_eq!(selected.picked(), [1]);
        selected.toggle(1);
        assert!(selected.is_empty());
    }

    #[test]
    fn a_sweep_takes_what_it_covers_and_leaves_the_rest() {
        let boxes = [(0, rect(0.0, 0.0)), (1, rect(40.0, 40.0)), (2, rect(80.0, 80.0))];
        let mut selected = Selected::default();
        // A brush across the first two, stopping short of the third.
        let brush = egui::Rect::from_min_max(egui::pos2(-5.0, -5.0), egui::pos2(55.0, 55.0));
        selected.sweep(brush, boxes.into_iter(), false);
        assert_eq!(selected.picked(), [0, 1]);
    }

    #[test]
    fn a_sweep_replaces_what_was_picked_unless_shift_is_held() {
        let boxes = [(0, rect(0.0, 0.0)), (1, rect(40.0, 40.0))];
        let brush = egui::Rect::from_min_max(egui::pos2(-5.0, -5.0), egui::pos2(15.0, 15.0));
        let mut replacing = Selected::default();
        replacing.only(1);
        replacing.sweep(brush, boxes.into_iter(), false);
        assert_eq!(replacing.picked(), [0]);

        let mut adding = Selected::default();
        adding.only(1);
        adding.sweep(brush, boxes.into_iter(), true);
        assert_eq!(adding.picked(), [1, 0]);
    }

    #[test]
    fn a_shift_sweep_over_something_already_picked_takes_it_back_out() {
        let mut selected = Selected::default();
        selected.only(2);
        selected.toggle(0);
        let boxes = [(0, rect(0.0, 0.0)), (2, rect(80.0, 80.0))];
        // A brush over the first box only. 2 was picked before the sweep
        // and the brush misses it, so it stays; the one the brush does
        // cover was already picked too, and a shift-sweep takes it back
        // out rather than leaving it in.
        selected.sweep(rect(-5.0, -5.0), boxes.into_iter(), true);
        assert_eq!(selected.picked(), [2]);
    }

    #[test]
    fn a_reloaded_scene_takes_the_actors_it_no_longer_has() {
        // A selection left pointing past the end of the list would describe
        // whatever now stands where the old actors did.
        let mut selected = Selected::default();
        selected.only(0);
        selected.toggle(5);
        selected.retain(3);
        assert_eq!(selected.picked(), [0]);
    }

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

    #[test]
    fn the_box_stands_on_the_ground_it_was_given() {
        // The box is drawn at an actor's snapped height, and the height is
        // a number that has to survive being carried in the base twice: a
        // corner is the base plus an offset, and the base already holds the
        // height. This went out once as double the ground.
        let at = Vec3::new(-30.0, 6.598, -105.0);
        let corners = box_corners(at);
        let (low, high) = corners
            .iter()
            .map(|corner| corner.y)
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), y| {
                (low.min(y), high.max(y))
            });
        assert!((low - (at.y + 0.02)).abs() < 1e-6, "{low}");
        assert!((high - (at.y + BOX_HEIGHT)).abs() < 1e-6, "{high}");
    }

    #[test]
    fn the_box_holds_the_whole_ground_spot() {
        let at = Vec3::new(-30.0, 6.598, -105.0);
        let corners = box_corners(at);
        let (low_x, high_x) = corners
            .iter()
            .map(|corner| corner.x)
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), x| {
                (low.min(x), high.max(x))
            });
        let (low_z, high_z) = corners
            .iter()
            .map(|corner| corner.z)
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), z| {
                (low.min(z), high.max(z))
            });
        // A body-sized square either side of it, and its feet inside it.
        assert!((low_x - (at.x - BOX_HALF_WIDTH)).abs() < 1e-6, "{low_x}");
        assert!((high_x - (at.x + BOX_HALF_WIDTH)).abs() < 1e-6, "{high_x}");
        assert!((low_z - (at.z - BOX_HALF_WIDTH)).abs() < 1e-6, "{low_z}");
        assert!((high_z - (at.z + BOX_HALF_WIDTH)).abs() < 1e-6, "{high_z}");
    }
}
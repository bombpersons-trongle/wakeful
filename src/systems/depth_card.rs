//! Building finished depth cards once their images land.
//!
//! The mesh itself is `crate::depth_card_mesh` — the geometry half,
//! shared with the editor. What lives here is the game's half: a pending
//! component per card, the system that converts it once the images
//! arrive, and parenting under the scene graphics root like any other
//! scene content.

use bevy::prelude::*;

use crate::depth_card_mesh;
use crate::scene::CameraPose;
use crate::systems::scene::SceneGraphics;

/// A background card still waiting for its depth image to load; the
/// builder system converts it into the finished mesh entity.
#[derive(Component)]
pub(crate) struct DepthCardPending {
    pub(crate) pose: CameraPose,
    pub(crate) depth_range: f32,
    pub(crate) depth: Handle<Image>,
    pub(crate) background: Handle<Image>,
}

/// A finished background card. Marks the entity for scene-teardown
/// cleanup: a card must never outlive the scene whose map built it.
#[derive(Component)]
pub(crate) struct DepthCard;

/// Builds pending cards once their depth and background images land,
/// parenting the finished card under the scene graphics root like any
/// other scene content.
pub(crate) fn build_pending_cards(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    images: Res<Assets<Image>>,
    graphics: Res<SceneGraphics>,
    pending: Query<(Entity, &DepthCardPending)>,
) {
    for (entity, card) in &pending {
        let Some(depth) = images.get(&card.depth) else {
            continue;
        };
        if images.get(&card.background).is_none() {
            continue;
        }
        let mesh = match depth_card_mesh::build(depth, &card.pose, card.depth_range) {
            Some(mesh) => mesh,
            None => {
                warn!("depth card: unsupported depth format, skipping");
                continue;
            }
        };
        commands.entity(entity).remove::<DepthCardPending>();
        commands.entity(graphics.0).add_child(entity);
        commands.entity(entity).insert((
            DepthCard,
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color_texture: Some(card.background.clone()),
                unlit: true, // the background is pre-lit paint
                ..default()
            })),
        ));
    }
}

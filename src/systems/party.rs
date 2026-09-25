//! The party: the world's roster of characters and who leads the field.
//!
//! The party starts empty — a world script populates it (see
//! `assets/scripts/world/party.rhai`). The player entity is a view of
//! the leader: whenever the leader changes, the field body swaps to
//! that member's model, consistently across scenes. An empty or
//! leader-less party means the placeholder capsule.

use std::collections::BTreeMap;

use crate::scripts::PartyCommand;
use crate::systems::animation::{CharacterAnimations, CharacterAnimator, PendingAnimations};
use crate::systems::player::{PlaceholderBody, spawn_placeholder_body};
use crate::systems::scene::gltf_asset_path;
use crate::{Player, PlayerModel};
use bevy::gltf::Gltf;
use bevy::prelude::*;

/// One playable character: identity plus the model that represents it.
#[derive(Clone, Debug)]
pub(crate) struct Member {
    pub(crate) name: String,
    pub(crate) model: String,
}

/// The world's roster of characters and the current field leader.
#[derive(Resource, Default, Debug)]
pub(crate) struct Party {
    members: BTreeMap<String, Member>,
    leader: Option<String>,
    /// The leader whose model was last queued for the field; the guard
    /// that keeps the sync system from re-queueing every frame.
    applied: Option<String>,
}

/// Marks the player's currently attached party model; despawned when
/// the leader changes.
#[derive(Component)]
pub(crate) struct FieldBody;

impl Party {
    /// Applies script-requested roster changes, in call order.
    /// Removing the leader reverts the field to the capsule; leading a
    /// member that doesn't exist yet is legal and takes effect when
    /// they're added.
    pub(crate) fn apply(&mut self, commands: &[PartyCommand]) {
        for command in commands {
            match command {
                PartyCommand::Add { id, name, model } => {
                    self.members.insert(
                        id.clone(),
                        Member {
                            name: name.clone(),
                            model: model.clone(),
                        },
                    );
                }
                PartyCommand::Remove { id } => {
                    self.members.remove(id);
                    // Clearing the leader deliberately leaves `applied`
                    // alone: the sync system sees the mismatch and
                    // reverts the field to the capsule itself.
                    if self.leader.as_deref() == Some(id) {
                        self.leader = None;
                    }
                }
                PartyCommand::Leader { id } => self.leader = Some(id.clone()),
            }
        }
    }
}

/// Queues the leader's model for the field, reverting to the capsule
/// when there's no resolvable leader. Runs every frame; the `applied`
/// guard makes it a no-op unless the party changed.
pub(crate) fn sync_player_model(
    mut commands: Commands,
    server: Res<AssetServer>,
    mut party: ResMut<Party>,
    players: Query<Entity, With<Player>>,
    field_bodies: Query<(Entity, &ChildOf), With<FieldBody>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let want = party
        .leader
        .clone()
        .filter(|id| party.members.contains_key(id));
    if party.applied == want {
        return;
    }
    let Ok(player) = players.single() else {
        return;
    };
    party.applied = want.clone();
    match want {
        Some(id) => {
            let member = &party.members[&id];
            debug!("field body: {} ({})", member.name, member.model);
            commands.insert_resource(PlayerModel(server.load(gltf_asset_path(&member.model))));
        }
        None => {
            commands.remove_resource::<PlayerModel>();
            for (body, child_of) in &field_bodies {
                if child_of.parent() == player {
                    commands.entity(body).despawn();
                }
            }
            spawn_placeholder_body(&mut commands, player, &mut meshes, &mut materials);
        }
    }
}

/// Attaches the queued leader model once its glTF has loaded, replacing
/// whatever body the player currently wears. The old animation wiring
/// is dropped so the new model's clips wire up fresh.
pub(crate) fn attach_player_model(
    mut commands: Commands,
    model: Option<Res<PlayerModel>>,
    gltfs: Res<Assets<Gltf>>,
    mut players: Query<Entity, With<Player>>,
    placeholders: Query<(Entity, &ChildOf), With<PlaceholderBody>>,
    old_models: Query<(Entity, &ChildOf), With<FieldBody>>,
) {
    let Some(model) = model else {
        return;
    };
    let Some(gltf) = gltfs.get(&model.0) else {
        return;
    };
    // The file's default scene; the first one if the glTF declares none.
    let Some(scene) = gltf
        .default_scene
        .clone()
        .or_else(|| gltf.scenes.first().cloned())
    else {
        return;
    };
    let Ok(player) = players.single_mut() else {
        return;
    };
    for (body, child_of) in placeholders.iter().chain(&old_models) {
        if child_of.parent() == player {
            commands.entity(body).despawn();
        }
    }
    commands
        .entity(player)
        .insert(PendingAnimations(model.0.clone()));
    commands
        .entity(player)
        .remove::<(CharacterAnimations, CharacterAnimator)>()
        .with_child((FieldBody, WorldAssetRoot(scene), Transform::default()));
    commands.remove_resource::<PlayerModel>();
}

#[cfg(test)]
mod tests {
    use bevy::asset::{AssetServer, AssetServerMode, UnapprovedPathMode, io::AssetSourceBuilders};
    use bevy::ecs::system::RunSystemOnce;
    use bevy::tasks::{ComputeTaskPool, IoTaskPool, TaskPool};
    use std::sync::Arc;

    use super::*;

    fn test_asset_server() -> AssetServer {
        let mut builders = AssetSourceBuilders::default();
        builders.init_default_source("assets", None);
        let sources = Arc::new(builders.build_sources(false, false));
        AssetServer::new(
            sources,
            AssetServerMode::Unprocessed,
            false,
            UnapprovedPathMode::Forbid,
        )
    }

    fn empty_gltf() -> Gltf {
        Gltf {
            scenes: vec![Handle::default()],
            named_scenes: Default::default(),
            meshes: vec![],
            named_meshes: Default::default(),
            materials: vec![],
            named_materials: Default::default(),
            nodes: vec![],
            named_nodes: Default::default(),
            skins: vec![],
            named_skins: Default::default(),
            default_scene: Some(Handle::default()),
            animations: vec![],
            named_animations: Default::default(),
            source: None,
        }
    }

    #[test]
    fn roster_changes_apply_in_call_order() {
        let mut party = Party::default();
        party.apply(&[
            PartyCommand::Add {
                id: "a".into(),
                name: "A".into(),
                model: "a.glb".into(),
            },
            PartyCommand::Leader { id: "a".into() },
            PartyCommand::Add {
                id: "b".into(),
                name: "B".into(),
                model: "b.glb".into(),
            },
            PartyCommand::Leader { id: "b".into() },
            PartyCommand::Remove { id: "a".into() },
        ]);
        assert_eq!(party.leader.as_deref(), Some("b"));
        assert_eq!(party.members.len(), 1);

        // Removing the leader reverts to a leader-less (capsule) party.
        party.apply(&[PartyCommand::Remove { id: "b".into() }]);
        assert_eq!(party.leader, None);
    }

    /// A world with one player wearing the cone, plus the asset
    /// scaffolding the sync/attach systems need.
    fn body_world() -> World {
        let mut world = World::new();
        // `server.load` queues its file read on Bevy's task pools, which
        // a bare test world doesn't set up.
        IoTaskPool::get_or_init(TaskPool::new);
        ComputeTaskPool::get_or_init(TaskPool::new);
        world.insert_resource(Party::default());
        let server = test_asset_server();
        let gltfs = Assets::<Gltf>::default();
        server.register_asset(&gltfs);
        world.insert_resource(server);
        world.insert_resource(gltfs);
        world.insert_resource(Assets::<Mesh>::default());
        world.insert_resource(Assets::<StandardMaterial>::default());
        let player = world.spawn(Player).id();
        world.resource_scope(|world, mut meshes: Mut<Assets<Mesh>>| {
            world.resource_scope(|world, mut materials: Mut<Assets<StandardMaterial>>| {
                let mut commands = world.commands();
                spawn_placeholder_body(&mut commands, player, &mut meshes, &mut materials);
            });
        });
        world.flush();
        world
    }

    /// Pretends the queued model finished loading, then attaches it.
    fn load_and_attach(world: &mut World) {
        world.resource_scope(|world, mut gltfs: Mut<Assets<Gltf>>| {
            let handle = gltfs.add(empty_gltf());
            world.insert_resource(PlayerModel(handle));
        });
        world.run_system_once(attach_player_model).unwrap();
        world.flush();
    }

    fn field_body(world: &mut World, player: Entity) -> Option<Entity> {
        world
            .query_filtered::<(Entity, &ChildOf), With<FieldBody>>()
            .iter(world)
            .find(|(_, child_of)| child_of.parent() == player)
            .map(|(entity, _)| entity)
    }

    fn player_of(world: &mut World) -> Entity {
        world
            .query_filtered::<Entity, With<Player>>()
            .single(world)
            .unwrap()
    }

    #[test]
    fn the_leader_drives_the_field_body() {
        let mut world = body_world();
        world.resource_scope(|_world, mut party: Mut<Party>| {
            party.apply(&[
                PartyCommand::Add {
                    id: "hero".into(),
                    name: "Hero".into(),
                    model: "models/character.glb".into(),
                },
                PartyCommand::Leader { id: "hero".into() },
            ]);
        });
        world.run_system_once(sync_player_model).unwrap();
        world.flush();
        // Queued for load; the cone stays until the model arrives.
        assert!(world.get_resource::<PlayerModel>().is_some());
        let player = player_of(&mut world);
        assert!(field_body(&mut world, player).is_none());

        load_and_attach(&mut world);
        assert!(world.get_resource::<PlayerModel>().is_none());
        let model = field_body(&mut world, player).expect("model attached");
        assert!(
            world
                .query_filtered::<Entity, With<PlaceholderBody>>()
                .single(&world)
                .is_err(),
            "cone replaced"
        );
        assert!(world.get::<PendingAnimations>(player).is_some());

        // Leader change: the next attach replaces the body.
        world.resource_scope(|_world, mut party: Mut<Party>| {
            party.apply(&[
                PartyCommand::Add {
                    id: "pip".into(),
                    name: "Pip".into(),
                    model: "models/pip.glb".into(),
                },
                PartyCommand::Leader { id: "pip".into() },
            ]);
        });
        world.run_system_once(sync_player_model).unwrap();
        world.flush();
        load_and_attach(&mut world);

        let player = player_of(&mut world);
        let new_model = field_body(&mut world, player).expect("replacement attached");
        assert_ne!(model, new_model);
    }

    #[test]
    fn losing_the_leader_reverts_to_the_capsule() {
        let mut world = body_world();
        world.resource_scope(|_world, mut party: Mut<Party>| {
            party.apply(&[
                PartyCommand::Add {
                    id: "hero".into(),
                    name: "Hero".into(),
                    model: "models/character.glb".into(),
                },
                PartyCommand::Leader { id: "hero".into() },
            ]);
        });
        world.run_system_once(sync_player_model).unwrap();
        world.flush();
        load_and_attach(&mut world);

        // Remove the leader: back to the cone, model gone, queue dropped.
        world.resource_scope(|_world, mut party: Mut<Party>| {
            party.apply(&[PartyCommand::Remove { id: "hero".into() }]);
        });
        world.run_system_once(sync_player_model).unwrap();
        world.flush();

        let player = player_of(&mut world);
        assert!(world.get_resource::<PlayerModel>().is_none());
        assert!(field_body(&mut world, player).is_none());
        assert!(
            world
                .query_filtered::<Entity, With<PlaceholderBody>>()
                .single(&world)
                .is_ok(),
            "capsule restored"
        );
    }
}

//! Debug screenshots: touch a request file, get the game frame back.
//!
//! Gated on `cfg(debug_assertions)`: present in every dev build (so a
//! running game can always be inspected), absent from release. The
//! "API" is the filesystem: a `shot-<nonce>.request` file in `.debug/`
//! asks for a capture of the clean 320x240 game image, and the PNG
//! lands as `shot-<nonce>.png` a frame or two later. No network, no
//! ports; the readback uses Bevy's own `gpu_readback` (the `Screenshot`
//! component's image-target path ships frames of uninitialized data in
//! 0.19, so it can't be used here).

use std::path::{Path, PathBuf};

use bevy::asset::RenderAssetUsages;
use bevy::ecs::entity::Entities;
use bevy::image::Image;
use bevy::prelude::*;
use bevy::render::gpu_readback::{Readback, ReadbackComplete};
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::screen::{GAME_HEIGHT, GAME_WIDTH, GameImage};

/// Where request and response files live, relative to the repo.
const SHOT_DIR: &str = ".debug";

/// The folder requests are scanned from.
#[derive(Resource, Clone)]
pub struct ShotDir(PathBuf);

pub fn setup(mut commands: Commands) {
    let dir = PathBuf::from(SHOT_DIR);
    let _ = std::fs::create_dir_all(&dir);
    commands.insert_resource(ShotDir(dir));
}

/// Finds the pending `<prefix><name>.request` files under `dir`.
fn scan_requests(dir: &Path, prefix: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<_> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            name.strip_prefix(prefix)?
                .strip_suffix(".request")
                .map(str::to_owned)
        })
        .collect();
    names.sort();
    names
}

fn scan_shot_requests(dir: &Path) -> Vec<String> {
    scan_requests(dir, "shot-")
}

/// Serves every pending request: read the clean game image back from
/// the GPU, delete the request. The PNG lands when the readback
/// completes, a frame or two later.
#[allow(clippy::too_many_arguments)]
/// The render-side queries of the state dump, bundled to stay under
/// the system parameter limit.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct RenderDumpParams<'w, 's> {
    cameras:
        Query<'w, 's, (&'static Transform, &'static Projection, &'static Camera), With<Camera3d>>,
    meshes: Query<
        'w,
        's,
        (
            &'static GlobalTransform,
            &'static Mesh3d,
            &'static Visibility,
            &'static InheritedVisibility,
            Option<&'static ChildOf>,
        ),
    >,
    visibilities: Query<'w, 's, &'static Visibility>,
    actormodels: Query<'w, 's, &'static crate::systems::actor::ActorModel>,
    anim_players: Query<'w, 's, (), With<bevy::animation::prelude::AnimationPlayer>>,
    graph_handles: Query<'w, 's, &'static bevy::animation::prelude::AnimationGraphHandle>,
    anim_transitions: Query<'w, 's, &'static bevy::animation::prelude::AnimationTransitions>,
    actors: Query<
        'w,
        's,
        (
            &'static GlobalTransform,
            &'static Visibility,
            &'static InheritedVisibility,
        ),
        With<crate::systems::actor::Actor>,
    >,
}

#[allow(clippy::too_many_arguments)]
pub fn check_requests<'w, 's>(
    mut commands: Commands,
    dir: Option<Res<ShotDir>>,
    game_image: Option<Res<GameImage>>,
    state: Option<Res<crate::world_state::WorldState>>,
    battle_handle: Option<Res<crate::battle::BattleHandle>>,
    battle: Option<Res<crate::battle::Battle>>,
    graphics: Option<Res<crate::systems::scene::SceneGraphics>>,
    grounds: Query<(Entity, &Visibility), With<crate::Ground>>,
    render: RenderDumpParams<'w, 's>,
    mut characters: Query<(
        Entity,
        &crate::systems::animation::CharacterAnimations,
        &mut crate::systems::animation::CharacterAnimator,
    )>,
    mut anim_players: Query<&mut bevy::animation::AnimationPlayer>,
    visibilities: Query<&Visibility>,
    transforms: Query<&Transform>,
    entities: &Entities,
    childrens: Query<&Children>,
    pending_animations: Query<(Entity, &crate::systems::animation::PendingAnimations)>,
) {
    let (Some(dir), Some(game_image)) = (dir, game_image) else {
        return;
    };
    // `dump-state-<nonce>.request`: write the world state out as text,
    // so store persistence can be verified across scene changes.
    for nonce in scan_requests(&dir.0, "dump-state-") {
        let request = dir.0.join(format!("dump-state-{nonce}.request"));
        if std::fs::remove_file(&request).is_err() {
            continue;
        }
        let out = dir.0.join(format!("state-{nonce}.txt"));
        if let Some(state) = &state {
            let shared = state.shared();
            let shared = shared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let scripts = state.debug_stores();
            let scripts = scripts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut lines = vec![format!("shared: {shared:?}")];
            if let Some(handle) = &battle_handle {
                lines.push(format!(
                    "battle-handle: active={} requests={}",
                    handle.active(),
                    handle.pending_requests()
                ));
            }
            if let Some(graphics) = &graphics {
                let grounds: Vec<String> = grounds
                    .iter()
                    .map(|(e, v)| format!("{} v={:?}", e.index(), *v))
                    .collect();
                lines.push(format!(
                    "graphics_root: {} grounds=[{}]",
                    graphics.0.index(),
                    grounds.join(", ")
                ));
            }
            let active = render
                .cameras
                .iter()
                .find(|(_, _, camera)| camera.is_active)
                .map(|(transform, projection, _)| (transform, projection));
            if let Some((transform, Projection::Perspective(perspective))) = active {
                let (yaw, pitch, _) = transform.rotation.to_euler(EulerRot::YXZ);
                lines.push(format!(
                    "camera: at {:?} yaw={:.2} pitch={:.2} fov={:.3}",
                    transform.translation, yaw, pitch, perspective.fov
                ));
            }
            let mut mesh_spots: Vec<String> = render
                .meshes
                .iter()
                .filter(|(t, _, _, _, _)| t.translation().y < 15.0)
                .map(|(t, _, v, iv, parent)| {
                    format!(
                        "{:.1} v={} iv={} parent={:?} pvis={:?}",
                        t.translation(),
                        *v == Visibility::Visible,
                        iv.get(),
                        parent.map(|c| c.parent().index()),
                        parent
                            .and_then(|c| render.visibilities.get(c.parent()).ok())
                            .is_some_and(|p| *p == Visibility::Visible),
                    )
                })
                .collect();
            mesh_spots.sort();
            lines.push(format!("low_meshes: {}", mesh_spots.join(", ")));
            for (transform, visibility, inherited) in &render.actors {
                lines.push(format!(
                    "actor: at {:?} v={} iv={}",
                    transform.translation(),
                    *visibility == Visibility::Visible,
                    inherited.get()
                ));
            }
            if let Some(battle) = &battle {
                lines.push(format!(
                    "arena: {:?} children={:?}",
                    battle.arena.index(),
                    childrens.get(battle.arena).map(|ch| ch.len())
                ));
                lines.push(format!(
                    "battle: participants={:?} actions={:?}",
                    battle
                        .participants
                        .iter()
                        .map(|c| (
                            c.id.as_str(),
                            c.entity.index(),
                            render.actormodels.get(c.entity).is_ok(),
                            childrens.get(c.entity).ok().map(|ch| ch.len()),
                            childrens
                                .get(c.entity)
                                .ok()
                                .and_then(|ch| ch.first())
                                .and_then(|mc| {
                                    childrens.get(*mc).ok().map(|scene_root| {
                                        (
                                            scene_root.len(),
                                            scene_root.first().and_then(|n| {
                                                childrens.get(*n).ok().map(|g| g.len())
                                            }),
                                        )
                                    })
                                }),
                            entities.contains(c.entity),
                        ))
                        .collect::<Vec<_>>(),
                    battle.actions.keys().collect::<Vec<_>>() // {:#?}
                ));
            }
            for (k, v) in scripts.iter() {
                lines.push(format!("{k}: {:?}", v.lock().unwrap()));
            }
            for (entity, _) in pending_animations.iter() {
                lines.push(format!(
                    "pending {entity:?}: children={:?}",
                    childrens.get(entity).map(|c| c.len())
                ));
            }
            for (entity, animations, animator) in &mut characters {
                lines.push(format!(
                    "anim {:?}: {:?} {}",
                    entity.index(),
                    animations.debug_clips(),
                    animator.debug()
                ));
                if let Some(player_entity) = animator.debug_player() {
                    lines.push(format!(
                        "  player {:?}: alive={} anim_player={} graph={} transitions={}",
                        player_entity.index(),
                        entities.contains(player_entity),
                        render.anim_players.contains(player_entity),
                        render.graph_handles.contains(player_entity),
                        render.anim_transitions.contains(player_entity),
                    ));
                }
                if let Some(root) = animator.debug_root() {
                    let scene_root = childrens
                        .get(root)
                        .ok()
                        .and_then(|ch| ch.first())
                        .and_then(|mc| childrens.get(*mc).ok());
                    lines.push(format!(
                        "  root {root:?}: appeared={} {:?} at {:?} children={:?} pending={} scene_root_children={:?} first_node_children={:?}",
                        animator.debug_appeared(),
                        visibilities.get(root),
                        transforms.get(root).map(|t| t.translation),
                        childrens.get(root).map(|c| c.len()),
                        pending_animations.contains(root),
                        scene_root.map(|r| r.len()),
                        scene_root
                            .and_then(|r| r.first())
                            .and_then(|n| {
                                childrens.get(*n).ok().map(|g| {
                                    (
                                        g.len(),
                                        g.first().and_then(|h| childrens.get(*h).ok().map(|gg| gg.len())),
                                    )
                                })
                            }),
                    ));
                }
                if let Some(player_entity) = animator.debug_player()
                    && let Ok(player) = anim_players.get_mut(player_entity)
                {
                    for (node, animation) in player.playing_animations() {
                        lines.push(format!(
                            "  player {:?} node#{}: seek={:?} finished={} paused={} weight={}",
                            player_entity.index(),
                            node.index(),
                            animation.seek_time(),
                            animation.is_finished(),
                            animation.is_paused(),
                            animation.weight(),
                        ));
                    }
                }
            }
            let text = lines.join("\n");
            let _ = std::fs::write(&out, text);
        }
    }
    for nonce in scan_shot_requests(&dir.0) {
        let request = dir.0.join(format!("shot-{nonce}.request"));
        match std::fs::remove_file(&request) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Another request file won the race; it'll serve next frame.
                continue;
            }
            Err(e) => {
                warn!("screenshot request {nonce} could not be cleared: {e}");
                continue;
            }
        }
        let path = dir.0.join(format!("shot-{nonce}.png"));
        commands
            .spawn(Readback::texture(game_image.0.clone()))
            .observe(
                move |trigger: On<ReadbackComplete>, mut commands: Commands| {
                    // The readback repeats every frame until the
                    // component comes off; one shot per request.
                    commands.entity(trigger.entity).remove::<Readback>();
                    save_png(&path, &trigger.data);
                },
            );
    }
}

fn save_png(path: &Path, data: &[u8]) {
    let expected = (GAME_WIDTH * GAME_HEIGHT * 4) as usize;
    if data.len() != expected {
        warn!(
            "screenshot readback was {} bytes, expected {expected}; not saving",
            data.len()
        );
        return;
    }
    let image = Image::new(
        Extent3d {
            width: GAME_WIDTH,
            height: GAME_HEIGHT,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data.to_vec(),
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::MAIN_WORLD,
    );
    let result = image
        .try_into_dynamic()
        .map_err(|e| e.to_string())
        .and_then(|dynamic| {
            dynamic
                .to_rgb8()
                .save_with_format(path.to_string_lossy().as_ref(), image::ImageFormat::Png)
                .map_err(|e| e.to_string())
        });
    match result {
        Ok(()) => info!("screenshot saved to {}", path.display()),
        Err(e) => warn!("screenshot could not be saved: {e}"),
    }
}

/// Serves `tap-<action>`, `hold-<action>`, and `release-<action>`
/// request files against the injected-input layer: taps fire for one
/// tick, holds press until the matching release (with press/release
/// edges on transition). Unknown actions warn once and are consumed.
pub fn check_input_requests(
    dir: Option<Res<ShotDir>>,
    injected: Option<ResMut<crate::input::InjectedInputs>>,
) {
    let (Some(dir), Some(mut injected)) = (dir, injected) else {
        return;
    };
    injected.just_pressed.clear();
    injected.just_released.clear();

    for action in scan_requests(&dir.0, "hold-") {
        let request = dir.0.join(format!("hold-{action}.request"));
        if consume(&request).is_err() {
            continue;
        }
        match crate::input::PadButton::from_config(&action) {
            Some(button) => {
                if injected.held.insert(button) {
                    injected.just_pressed.insert(button);
                }
            }
            None => warn!("input request names unknown action '{action}'"),
        }
    }
    for action in scan_requests(&dir.0, "release-") {
        let request = dir.0.join(format!("release-{action}.request"));
        if consume(&request).is_err() {
            continue;
        }
        match crate::input::PadButton::from_config(&action) {
            Some(button) => {
                if injected.held.remove(&button) {
                    injected.just_released.insert(button);
                }
            }
            None => warn!("input request names unknown action '{action}'"),
        }
    }
    // Taps last, so a hold+tap in the same tick still reads an edge.
    for action in scan_requests(&dir.0, "tap-") {
        let request = dir.0.join(format!("tap-{action}.request"));
        if consume(&request).is_err() {
            continue;
        }
        match crate::input::PadButton::from_config(&action) {
            Some(button) => {
                injected.just_pressed.insert(button);
            }
            None => warn!("input request names unknown action '{action}'"),
        }
    }
}

/// Deletes a consumed request file; `Err` means another scanner pass
/// already took it this frame.
fn consume(request: &Path) -> std::io::Result<()> {
    std::fs::remove_file(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::PadButton;
    use bevy::ecs::system::RunSystemOnce;

    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wakeful-shot-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn scan_finds_request_nonces_in_order_and_ignores_the_rest() {
        let dir = scratch_dir("scan");
        std::fs::write(dir.join("shot-300.request"), "").unwrap();
        std::fs::write(dir.join("shot-100.request"), "").unwrap();
        std::fs::write(dir.join("shot-200.png"), "").unwrap();
        std::fs::write(dir.join("unrelated.txt"), "").unwrap();

        assert_eq!(
            scan_requests(&dir, "shot-"),
            vec!["100".to_owned(), "300".to_owned()]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_folder_scans_as_empty() {
        assert!(scan_requests(Path::new("/nonexistent/wakeful-shots"), "shot-").is_empty());
    }

    #[test]
    fn input_requests_drive_the_injected_layer() {
        let dir = scratch_dir("input");
        let mut world = World::new();
        world.insert_resource(ShotDir(dir.clone()));
        world.init_resource::<crate::input::InjectedInputs>();

        std::fs::write(dir.join("tap-triangle.request"), "").unwrap();
        std::fs::write(dir.join("hold-dpad_up.request"), "").unwrap();
        std::fs::write(dir.join("bogus-what.request"), "").unwrap();
        world.run_system_once(check_input_requests).unwrap();

        let injected = world.resource::<crate::input::InjectedInputs>();
        assert!(injected.just_pressed.contains(&PadButton::Triangle));
        assert!(injected.held.contains(&PadButton::DPadUp));

        // Edges clear on the next scan; the hold persists. A release
        // with no matching hold is a silent no-op.
        world.run_system_once(check_input_requests).unwrap();
        let injected = world.resource::<crate::input::InjectedInputs>();
        assert!(injected.just_pressed.is_empty());
        assert!(injected.held.contains(&PadButton::DPadUp));

        std::fs::write(dir.join("release-dpad_up.request"), "").unwrap();
        world.run_system_once(check_input_requests).unwrap();
        let injected = world.resource::<crate::input::InjectedInputs>();
        assert!(!injected.held.contains(&PadButton::DPadUp));
        assert!(injected.just_released.contains(&PadButton::DPadUp));

        std::fs::remove_dir_all(&dir).unwrap();
    }
}

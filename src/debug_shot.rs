//! Debug tooling behind the MCP server: the executor that turns tool
//! calls into game effects.
//!
//! Gated on `cfg(debug_assertions)`: present in every dev build, absent
//! from release. [`serve_debug_commands`] drains the
//! [`DebugCommands`](crate::debug_server::DebugCommands) queue at the
//! head of each fixed tick — screenshots via Bevy's `gpu_readback` (the
//! `Screenshot` component's image-target path ships frames of
//! uninitialized data in 0.19, so it can't be used here), input through
//! the injected layer, state dumps as text, and `eval` as one-shot
//! world-tier rhai.

use std::path::{Path, PathBuf};

use bevy::asset::RenderAssetUsages;
use bevy::ecs::entity::Entities;
use bevy::image::Image;
use bevy::prelude::*;
use bevy::render::gpu_readback::{Readback, ReadbackComplete};
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::debug_server::{DebugKind, DebugReply};
use crate::screen::{GAME_HEIGHT, GAME_WIDTH, GameImage};

/// Where request and response files live, relative to the repo.
const SHOT_DIR: &str = ".debug";

/// The folder requests are scanned from.
#[derive(Resource, Clone)]
pub struct ShotDir(PathBuf);


pub fn setup(mut commands: Commands) {
    let dir = PathBuf::from(SHOT_DIR);
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::create_dir_all(dir.join("shots"));
    commands.insert_resource(ShotDir(dir));
}

/// The resources the `eval` console binds against: the same handles a
/// world script sees (input, UI, shared stores, battle, scene ops),
/// plus the party so mutator calls apply immediately.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ConsoleEnvParams<'w> {
    input: Res<'w, crate::input::InputManager>,
    ui: Res<'w, crate::systems::ui::UiApi>,
    state: Res<'w, crate::world_state::WorldState>,
    battle_handle: Res<'w, crate::battle::BattleHandle>,
    world: Res<'w, crate::scripts::WorldCommands>,
    party: ResMut<'w, crate::systems::party::Party>,
}

/// Render-side queries for the state dump, bundled to stay under the
/// system parameter limit.
/// Render-side queries for the state dump, bundled to stay under the
/// system parameter limit.
type DumpQueries<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static crate::systems::animation::CharacterAnimations,
        &'static mut crate::systems::animation::CharacterAnimator,
    ),
>;

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

/// Loose scene peeks for the dump (visibilities/transforms/children
/// plus the entity list), bundled out of the executor's parameter
/// count.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ScenePeekParams<'w, 's> {
    visibilities: Query<'w, 's, &'static Visibility>,
    transforms: Query<'w, 's, &'static Transform>,
    childrens: Query<'w, 's, &'static Children>,
}

/// The queue and its side resources, bundled out of the executor's
/// parameter count.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct DebugRunParams<'w> {
    dir: Option<Res<'w, ShotDir>>,
    game_image: Option<Res<'w, GameImage>>,
    queue: Option<Res<'w, crate::debug_server::DebugCommands>>,
    injected: Option<ResMut<'w, crate::input::InjectedInputs>>,
}

/// Drains the debug queue at the head of the tick: screenshots via
/// `gpu_readback` (the reply lands when the readback completes, a
/// frame or two later), input through the injected layer, state dumps
/// and `eval` inline. Injected edges clear here every tick, so a tap
/// queued this tick reaches this tick's `aggregate_inputs`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn serve_debug_commands(
    mut commands: Commands,
    mut run: DebugRunParams,
    mut console: ConsoleEnvParams,
    battle: Option<Res<crate::battle::Battle>>,
    battle_handle: Option<Res<crate::battle::BattleHandle>>,
    graphics: Option<Res<crate::systems::scene::SceneGraphics>>,
    grounds: Query<(Entity, &Visibility), With<crate::Ground>>,
    render: RenderDumpParams,
    mut characters: DumpQueries,
    mut anim_players: Query<&mut bevy::animation::AnimationPlayer>,
    peek: ScenePeekParams,
    pending_animations: Query<(Entity, &crate::systems::animation::PendingAnimations)>,
    entities: &Entities,
) {
    let Some(injected) = &mut run.injected else {
        return;
    };
    injected.just_pressed.clear();
    injected.just_released.clear();
    let Some(queue) = &run.queue else {
        return;
    };
    for command in queue.take() {
        let kind = command.kind;
        let (hold, release) = (
            matches!(kind, DebugKind::Hold(_)),
            matches!(kind, DebugKind::Release(_)),
        );
        match kind {
            DebugKind::Tap(action) | DebugKind::Hold(action) | DebugKind::Release(action) => {
                let Some(button) = crate::input::PadButton::from_config(&action) else {
                    let _ = command.reply.send(Err(format!("unknown action '{action}'")));
                    continue;
                };
                let injected = &mut **injected;
                if hold {
                    if injected.held.insert(button) {
                        injected.just_pressed.insert(button);
                    }
                } else if release {
                    if injected.held.remove(&button) {
                        injected.just_released.insert(button);
                    }
                } else {
                    injected.just_pressed.insert(button);
                }
                let _ = command.reply.send(Ok(DebugReply::Text(format!("{action} ok"))));
            }
            DebugKind::Screenshot => {
                let (Some(game_image), Some(dir)) = (run.game_image.as_ref(), run.dir.as_ref())
                else {
                    continue;
                };
                let secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or_default();
                let path = dir.0.join("shots").join(format!("shot-{secs}.png"));
                let reply = command.reply.clone();
                commands
                    .spawn(Readback::texture(game_image.0.clone()))
                    .observe(
                        move |trigger: On<ReadbackComplete>, mut commands: Commands| {
                            // The readback repeats every frame until the
                            // component comes off; one shot per request.
                            commands.entity(trigger.entity).remove::<Readback>();
                            match encode_png(&trigger.data, &path) {
                                Some(png) => {
                                    let _ = reply.send(Ok(DebugReply::Png(png)));
                                }
                                None => {
                                    let _ = reply.send(Err("the readback was the wrong size"
                                        .to_owned()));
                                }
                            }
                        },
                    );
            }
            DebugKind::State => {
                let text = state_dump(
                    &console.state, &battle_handle, &battle, &graphics, &grounds, &render,
                    &mut characters, &mut anim_players, &peek, &pending_animations, entities,
                );
                let _ = command.reply.send(Ok(DebugReply::Text(text)));
            }
            DebugKind::Eval(code) => {
                let reply = run_console(&code, &mut console);
                let _ = command.reply.send(reply.map(DebugReply::Text));
            }
        }
    }
}

/// Runs one console snippet: wrapped with the shared-library imports,
/// compiled against the live world-tier environment, executed, and its
/// last expression stringified. Party mutator calls apply immediately;
/// scene operations land in the shared channel for the drain.
fn run_console(
    code: &str,
    console: &mut ConsoleEnvParams,
) -> Result<String, String> {
    let source = format!(
        "import \"lib/roster\" as r;\nimport \"lib/floats\" as floats;\nfn console_entry() {{\n{code}\n}}\n"
    );
    let env = crate::scripts::ScriptEnv::new(
        console.input.handle(),
        console.ui.clone(),
        console.state.clone(),
        console.battle_handle.clone(),
        console.world.clone(),
    );
    let runtime = crate::scripts::WorldScript::compile_with_handle(&source, env).map_err(|e| {
        format!(
            "compile: {e} [root={} lib_exists={}]",
            crate::assets::assets_root().join("scripts").display(),
            crate::assets::assets_root()
                .join("scripts/lib/roster.rhai")
                .exists()
        )
    })?;
    let (value, changes) = runtime.call_console("console_entry").map_err(|e| {
        format!(
            "{e} [root={} lib_exists={}]",
            crate::assets::assets_root().join("scripts").display(),
            crate::assets::assets_root()
                .join("scripts/lib/roster.rhai")
                .exists()
        )
    })?;
    console.party.apply(&changes);
    Ok(format!("{value}"))
}

/// Renders the world state as text: shared stores, battle internals,
/// the active camera, actors, and the animation drivers.
#[allow(clippy::too_many_arguments)]
fn state_dump<'w, 's>(
    state: &Res<crate::world_state::WorldState>,
    battle_handle: &Option<Res<crate::battle::BattleHandle>>,
    battle: &Option<Res<crate::battle::Battle>>,
    graphics: &Option<Res<crate::systems::scene::SceneGraphics>>,
    grounds: &Query<(Entity, &Visibility), With<crate::Ground>>,
    render: &RenderDumpParams<'w, 's>,
    characters: &mut DumpQueries,
    anim_players: &mut Query<&mut bevy::animation::AnimationPlayer>,
    peek: &ScenePeekParams<'w, 's>,
    pending_animations: &Query<(Entity, &crate::systems::animation::PendingAnimations)>,
    entities: &Entities,
) -> String {
    let mut lines = Vec::new();
    {
        let shared = state.shared();
        let shared = shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let scripts = state.debug_stores();
        let scripts = scripts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        lines.push(format!("shared: {shared:?}"));
        if let Some(handle) = battle_handle {
            lines.push(format!(
                "battle-handle: active={} requests={}",
                handle.active(),
                handle.pending_requests()
            ));
        }
        if let Some(graphics) = graphics {
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
                        .and_then(|c| peek.visibilities.get(c.parent()).ok())
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
        if let Some(battle) = battle {
            lines.push(format!(
                "arena: {:?} children={:?}",
                battle.arena.index(),
                peek.childrens.get(battle.arena).map(|ch| ch.len())
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
                        peek.childrens.get(c.entity).ok().map(|ch| ch.len()),
                        peek.childrens
                            .get(c.entity)
                            .ok()
                            .and_then(|ch| ch.first())
                            .and_then(|mc| {
                                peek.childrens.get(*mc).ok().map(|scene_root| {
                                    (
                                        scene_root.len(),
                                        scene_root.first().and_then(|n| {
                                            peek.childrens.get(*n).ok().map(|g| g.len())
                                        }),
                                    )
                                })
                            }),
                        entities.contains(c.entity),
                    ))
                    .collect::<Vec<_>>(),
                    battle.actions.keys().collect::<Vec<_>>()
                ));
        }
        for (k, v) in scripts.iter() {
            lines.push(format!("{k}: {:?}", v.lock().unwrap()));
        }
        for (entity, _) in pending_animations.iter() {
            lines.push(format!(
                "pending {entity:?}: children={:?}",
                peek.childrens.get(entity).map(|c| c.len())
            ));
        }
        for (entity, animations, animator) in &mut *characters {
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
                let scene_root = peek
                    .childrens
                    .get(root)
                    .ok()
                    .and_then(|ch| ch.first())
                    .and_then(|mc| peek.childrens.get(*mc).ok());
                lines.push(format!(
                    "  root {root:?}: appeared={} {:?} at {:?} children={:?} pending={} scene_root_children={:?} first_node_children={:?}",
                    animator.debug_appeared(),
                    peek.visibilities.get(root),
                    peek.transforms.get(root).map(|t| t.translation),
                    peek.childrens.get(root).map(|c| c.len()),
                    pending_animations.contains(root),
                    scene_root.map(|r| r.len()),
                    scene_root
                        .and_then(|r| r.first())
                        .and_then(|n| {
                            peek.childrens.get(*n).ok().map(|g| {
                                (
                                    g.len(),
                                    g.first()
                                        .and_then(|h| peek.childrens.get(*h).ok().map(|gg| gg.len())),
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
    }
    lines.join("\n")
}

/// Encodes the readback as PNG, saves it under `.debug/shots/`, and
/// hands the bytes back for the MCP image reply.
fn encode_png(data: &[u8], path: &Path) -> Option<Vec<u8>> {
    let expected = (GAME_WIDTH * GAME_HEIGHT * 4) as usize;
    if data.len() != expected {
        warn!(
            "screenshot readback was {} bytes, expected {expected}; not saving",
            data.len()
        );
        return None;
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
    let dynamic = image.try_into_dynamic().ok()?.to_rgb8();
    let mut buffer = std::io::Cursor::new(Vec::new());
    dynamic
        .write_to(&mut buffer, image::ImageFormat::Png)
        .ok()?;
    let bytes = buffer.into_inner();
    std::fs::create_dir_all(path.parent()?).ok();
    std::fs::write(path, &bytes).ok()?;
    info!("screenshot saved to {}", path.display());
    Some(bytes)
}


#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce as _;
    use crate::debug_server::DebugCommand;
    use crate::input::PadButton;

    fn world_with_debug() -> World {
        let mut world = World::new();
        world.insert_resource(ShotDir(PathBuf::from(".debug")));
        world.insert_resource(crate::debug_server::DebugCommands::default());
        world.init_resource::<crate::input::InjectedInputs>();
        world.insert_resource(crate::input::InputManager::standard());
        world.insert_resource(crate::systems::ui::UiApi::new());
        world.insert_resource(crate::world_state::WorldState::default());
        world.insert_resource(crate::battle::BattleHandle::new());
        world.insert_resource(crate::scripts::WorldCommands::default());
        world.insert_resource(crate::systems::party::Party::default());
        world
    }

    fn send(world: &mut World, kind: DebugKind) -> Result<DebugReply, String> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        world
            .resource::<crate::debug_server::DebugCommands>()
            .push(DebugCommand { kind, reply: tx });
        world.run_system_once(serve_debug_commands).unwrap();
        rx.try_recv()
            .expect("the executor answers within the same tick")
    }

    #[test]
    fn taps_reach_the_injected_layer_for_one_tick() {
        let mut world = world_with_debug();

        let reply = send(
            &mut world,
            DebugKind::Tap("triangle".to_owned()),
        )
        .unwrap();
        assert!(matches!(reply, DebugReply::Text(_)));
        let injected = world.resource::<crate::input::InjectedInputs>();
        assert!(injected.just_pressed.contains(&PadButton::Triangle));

        // Edges clear on the next tick; nothing lingers.
        send(&mut world, DebugKind::State).unwrap();
        let injected = world.resource::<crate::input::InjectedInputs>();
        assert!(injected.just_pressed.is_empty());
    }

    #[test]
    fn holds_press_until_released_and_unknown_actions_err() {
        let mut world = world_with_debug();

        send(&mut world, DebugKind::Hold("dpad_up".to_owned())).unwrap();
        send(&mut world, DebugKind::State).unwrap();
        assert!(world
            .resource::<crate::input::InjectedInputs>()
            .held
            .contains(&PadButton::DPadUp));

        let error = send(
            &mut world,
            DebugKind::Release("bogus_button".to_owned()),
        )
        .unwrap_err();
        assert!(error.contains("bogus_button"));

        send(&mut world, DebugKind::Release("dpad_up".to_owned())).unwrap();
        assert!(!world
            .resource::<crate::input::InjectedInputs>()
            .held
            .contains(&PadButton::DPadUp));
    }

    #[test]
    fn eval_runs_against_the_shared_store() {
        let mut world = world_with_debug();

        let reply = send(
            &mut world,
            DebugKind::Eval("remember_global(\"smoke\", 42.0); 42.0".to_owned()),
        )
        .unwrap();
        let DebugReply::Text(text) = reply else {
            panic!("eval replies with text");
        };
        assert_eq!(text, "42.0", "the last expression is the result");

        let shared = world
            .resource::<crate::world_state::WorldState>()
            .shared();
        let shared = shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            shared.get("smoke").and_then(|v| v.clone().try_cast::<f64>()),
            Some(42.0),
            "the console wrote the shared store"
        );
    }

    #[test]
    fn eval_warps_land_in_the_scene_channel() {
        let mut world = world_with_debug();

        send(
            &mut world,
            DebugKind::Eval(
                "warp_to(\"scenes/devroom.scene\", 1.0, 2.0)".to_owned(),
            ),
        )
        .unwrap();

        let requests = world
            .resource::<crate::scripts::WorldCommands>()
            .take();
        assert!(
            requests.iter().any(|r| matches!(
                r,
                crate::scripts::WorldRequest::Warp { scene, .. }
                    if scene == "scenes/devroom.scene"
            )),
            "the console's warp rides the shared channel: {requests:?}"
        );
    }

    #[test]
    fn eval_surfaces_compile_and_runtime_errors() {
        let mut world = world_with_debug();

        let error = send(
            &mut world,
            DebugKind::Eval("let mut broken = 1;".to_owned()),
        )
        .unwrap_err();
        assert!(error.contains("compile"), "{error}");

        let error = send(
            &mut world,
            DebugKind::Eval("bogus_fn()".to_owned()),
        )
        .unwrap_err();
        assert!(error.to_lowercase().contains("bogus_fn"), "{error}");
    }
}


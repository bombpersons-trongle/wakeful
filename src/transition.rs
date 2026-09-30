//! The transition: the screen freezes under an animated cover while
//! the world changes underneath it.
//!
//! A transition runs in three phases. `Out`: the cover closes over the
//! outgoing view — a plain black fade, or the battle swirl, which
//! samples a frozen copy of the last frame. The world needs no help
//! freezing here: every motion system is gated on its own game state,
//! and the transition is a state of its own. `Covered`: the screen is
//! fully black; the game state flips to the requested target and the
//! new context stages itself behind the cover (its `OnEnter` hooks).
//! `In`: the cover fades away onto the live result.
//!
//! That ordering is the machine's whole safety story: the next state's
//! content does not exist until the cover is closed, so the swirl's
//! frozen frame can only ever hold the outgoing view, and staging is
//! never on screen. Consumers never touch this module — they hook
//! `OnEnter` of their own state.

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use bevy::render::gpu_readback::{Readback, ReadbackComplete};
use bevy::render::render_resource::{AsBindGroup, Extent3d, ShaderType, TextureDimension, TextureFormat};
use bevy::shader::ShaderRef;
use bevy::sprite_render::{AlphaMode2d, Material2d};

use crate::game_state::GameState;
use crate::screen::{GAME_HEIGHT, GAME_WIDTH, UI_LAYER};
use crate::systems::bubble::screen_to_world;

/// Seconds for the cover to close over the outgoing view.
const OUT_SECS: f32 = 1.5;
/// Seconds the cover holds fully black while the new context stages.
const HOLD_SECS: f32 = 0.1;
/// Seconds for the cover to fade away onto the new view.
const IN_SECS: f32 = 0.5;
/// How long the swirl may wait for its frame readback before giving up
/// and finishing as a plain fade — a transition that never closes
/// freezes the game forever.
const CAPTURE_TIMEOUT_SECS: f32 = 2.0;

/// What the cover does on the way out. Both effects end fully black;
/// only the journey differs. The swirl needs the frozen frame, so it
/// starts when the readback lands; the fade needs nothing and starts
/// immediately.
#[derive(Clone, Copy, PartialEq, Default, Debug)]
pub(crate) enum Effect {
    #[default]
    Fade,
    Swirl,
}

/// The machine's phases, in order.
#[derive(Clone, Copy, PartialEq, Default, Debug)]
pub(crate) enum Phase {
    #[default]
    Idle,
    /// The cover is closing over the outgoing view.
    Out,
    /// Fully black: the state flips as the phase begins, and the hold
    /// absorbs the new context's staging frames.
    Covered,
    /// The cover fades away onto the new view.
    In,
}

/// The transition machine. Script requests call [`TransitionState::begin`];
/// [`drive_transition`] animates it and performs the covered flip.
#[derive(Resource, Default)]
pub(crate) struct TransitionState {
    /// Where the game goes when the cover closes; `None` while idle.
    target: Option<GameState>,
    effect: Effect,
    phase: Phase,
    /// Cover strength: 0 = clear, 1 = fully black.
    progress: f32,
    /// Countdown of the covered hold.
    hold: f32,
    /// The swirl's frame readback: whether it is queued, whether the
    /// frame has landed, and how long it has been waited on.
    capture_queued: bool,
    frame_ready: bool,
    capture_wait: f32,
}

impl TransitionState {
    /// Starts a transition to `target`. Refused (returns false) while
    /// one is already running — a second script request mid-flight
    /// must not derail the first.
    pub(crate) fn begin(&mut self, target: GameState, effect: Effect) -> bool {
        if self.phase != Phase::Idle {
            return false;
        }
        *self = Self {
            target: Some(target),
            effect,
            phase: Phase::Out,
            ..default()
        };
        true
    }
}

/// The cover's material: one fullscreen quad drawn by the UI camera
/// above everything. Mode 1 (the swirl) samples the frozen frame,
/// magnifying about the center while the cover closes; mode 0 is the
/// plain cover — black at the progress opacity.
#[derive(Asset, AsBindGroup, Clone, TypePath, Debug)]
pub(crate) struct TransitionMaterial {
    #[uniform(0)]
    effect: EffectUniform,
    #[texture(1)]
    #[sampler(2)]
    frozen: Handle<Image>,
}

#[derive(ShaderType, Clone, Copy, Debug, Default)]
struct EffectUniform {
    mode: u32,
    progress: f32,
}

impl Material2d for TransitionMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/transition.wgsl".into()
    }

    // The swirl renders the opaque frozen frame while the plain cover
    // fades by alpha: blending serves both.
    fn alpha_mode(&self) -> AlphaMode2d {
        AlphaMode2d::Blend
    }
}

/// Handle of the cover's material; the frame capture swaps its texture.
#[derive(Resource)]
pub(crate) struct OverlayMaterial(pub(crate) Handle<TransitionMaterial>);

/// Marks the fullscreen cover quad, spawned once at boot.
#[derive(Component)]
pub(crate) struct TransitionQuad;

/// Spawns the cover quad once at startup; visibility and material are
/// the machine's to drive.
pub(crate) fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<TransitionMaterial>>,
    bubbles: Res<crate::systems::bubble::BubbleAssets>,
) {
    // A 1x1 black placeholder so the material always has a texture;
    // the swirl's capture swaps it.
    let placeholder = images.add(black_image());
    let material = materials.add(TransitionMaterial {
        effect: default(),
        frozen: placeholder.clone(),
    });
    commands.insert_resource(OverlayMaterial(material.clone()));
    // Above every other UI quad (the battle's own fade cover sits at
    // z 1.5): the transition is the topmost thing in the game. The
    // extra size hides edge sampling at the swirl's zoomed UVs.
    let center = screen_to_world(Vec2::new(GAME_WIDTH as f32 / 2.0, GAME_HEIGHT as f32 / 2.0));
    commands.spawn((
        TransitionQuad,
        Mesh2d(bubbles.rect.clone()),
        MeshMaterial2d(material),
        Visibility::Hidden,
        Transform::from_translation(center.extend(2.0)).with_scale(Vec3::new(340.0, 260.0, 1.0)),
        RenderLayers::layer(UI_LAYER),
    ));
}

fn black_image() -> Image {
    Image::new_fill(
        Extent3d::default(),
        TextureDimension::D2,
        &[0, 0, 0, 255],
        TextureFormat::Rgba8UnormSrgb,
        // RENDER_WORLD or the GPU never sees the texture and the
        // material's bind group silently fails to prepare — the quad
        // draws nothing, with no error anywhere.
        RenderAssetUsages::default(),
    )
}

/// Queues the swirl's frame readback: the first Transition frame, while
/// the outgoing view is still the only thing that has ever been
/// rendered. The observer hands the frame back as the cover's texture.
pub(crate) fn capture(
    mut commands: Commands,
    game_image: Res<crate::screen::GameImage>,
    mut state: ResMut<TransitionState>,
) {
    if state.phase != Phase::Out
        || state.effect != Effect::Swirl
        || state.capture_queued
        || state.frame_ready
    {
        return;
    }
    state.capture_queued = true;
    commands
        .spawn(Readback::texture(game_image.0.clone()))
        .observe(on_frame_readback);
}

/// The readback's payoff: the frame becomes the cover's texture, and
/// the machine may start closing the cover over it. The `Readback`
/// component goes either way — a leftover one would re-read every
/// frame.
fn on_frame_readback(
    trigger: On<ReadbackComplete>,
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<TransitionMaterial>>,
    overlay: Res<OverlayMaterial>,
    mut state: ResMut<TransitionState>,
) {
    commands.entity(trigger.entity).remove::<Readback>();
    ingest_frame(
        &trigger.data,
        &mut images,
        &mut materials,
        &overlay.0,
        &mut state,
    );
}

/// Consumes one captured frame. A correctly-sized frame is swapped in
/// as the cover texture and the swirl may start; anything else warns
/// and downgrades to a plain fade.
fn ingest_frame(
    data: &[u8],
    images: &mut Assets<Image>,
    materials: &mut Assets<TransitionMaterial>,
    overlay: &Handle<TransitionMaterial>,
    state: &mut TransitionState,
) {
    let expected = (GAME_WIDTH * GAME_HEIGHT * 4) as usize;
    if data.len() != expected {
        warn!("transition: capture came back {} bytes", data.len());
        state.effect = Effect::Fade;
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
        RenderAssetUsages::default(),
    );
    if let Some(mut material) = materials.get_mut(overlay) {
        material.frozen = images.add(image);
    }
    state.frame_ready = true;
}

/// The machine's heartbeat: animates the cover, performs the covered
/// state flip, and feeds the overlay's material. Not state-gated — the
/// In phase outlives the flip, running over the new state's view.
pub(crate) fn drive_transition(
    mut state: ResMut<TransitionState>,
    mut next_state: ResMut<NextState<GameState>>,
    time: Res<Time>,
    mut materials: ResMut<Assets<TransitionMaterial>>,
    overlay: Res<OverlayMaterial>,
    mut quad: Query<&mut Visibility, With<TransitionQuad>>,
) {
    let dt = time.delta_secs();
    match state.phase {
        Phase::Idle => {}
        Phase::Out => {
            if state.effect == Effect::Swirl && !state.frame_ready {
                state.capture_wait += dt;
                if state.capture_wait >= CAPTURE_TIMEOUT_SECS {
                    warn!("transition: frame capture timed out; finishing as a fade");
                    state.effect = Effect::Fade;
                }
            } else {
                state.progress = (state.progress + dt / OUT_SECS).min(1.0);
                if state.progress >= 1.0 {
                    // The cover is opaque: the world becomes the
                    // target's, and the hold absorbs its staging.
                    let target = state.target.take().expect("a running transition has a target");
                    next_state.set(target);
                    state.hold = HOLD_SECS;
                    state.phase = Phase::Covered;
                }
            }
        }
        Phase::Covered => {
            state.hold -= dt;
            if state.hold <= 0.0 {
                state.phase = Phase::In;
            }
        }
        Phase::In => {
            state.progress = (state.progress - dt / IN_SECS).max(0.0);
            if state.progress <= 0.0 {
                *state = TransitionState::default();
            }
        }
    }

    // The overlay: on screen once it has something to show (a fade
    // needs no frame; the swirl waits for the capture), gone once the
    // machine rests. Mode 1 is the swirl's opening act only — the
    // covered hold and the reveal are a plain black cover.
    let visible = state.phase != Phase::Idle && (state.effect == Effect::Fade || state.frame_ready);
    let mode = u32::from(state.phase == Phase::Out && state.effect == Effect::Swirl);
    for mut visibility in &mut quad {
        *visibility = if visible {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
    if let Some(mut material) = materials.get_mut(&overlay.0) {
        material.effect = EffectUniform {
            mode,
            progress: state.progress,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use std::time::Duration;

    /// The resources and the cover quad `drive_transition` touches.
    fn harness() -> World {
        let mut world = World::new();
        world.init_resource::<TransitionState>();
        world.init_resource::<NextState<GameState>>();
        world.init_resource::<Assets<Image>>();
        world.init_resource::<Assets<TransitionMaterial>>();
        let frozen = world.resource_mut::<Assets<Image>>().add(black_image());
        let material = world
            .resource_mut::<Assets<TransitionMaterial>>()
            .add(TransitionMaterial {
                effect: default(),
                frozen,
            });
        world.insert_resource(OverlayMaterial(material));
        world.spawn((TransitionQuad, Visibility::Hidden));
        world
    }

    /// Advances time by `secs`, then runs one machine tick.
    fn step(world: &mut World, secs: f32) {
        let mut time: Time = Time::default();
        time.advance_by(Duration::from_secs_f32(secs));
        world.insert_resource(time);
        world.run_system_once(drive_transition).unwrap();
    }

    fn quad_visibility(world: &mut World) -> Visibility {
        let mut query = world.query::<&Visibility>();
        *query
            .iter(world)
            .next()
            .expect("the harness spawns the cover quad")
    }

    /// Starts a transition, failing the test if the machine refuses.
    fn begin(world: &mut World, target: GameState, effect: Effect) {
        assert!(
            world.resource_mut::<TransitionState>().begin(target, effect),
            "the machine starts idle"
        );
    }

    #[test]
    fn a_fade_closes_then_flips_and_reopens() {
        let mut world = harness();
        begin(&mut world, GameState::Battle, Effect::Fade);

        // Almost closed: still outgoing.
        step(&mut world, OUT_SECS * 0.9);
        let state = world.resource::<TransitionState>();
        assert_eq!(state.phase, Phase::Out);
        assert_eq!(quad_visibility(&mut world), Visibility::Visible);

        // Closed: the state flips behind the cover, the hold begins.
        step(&mut world, OUT_SECS * 0.2);
        let state = world.resource::<TransitionState>();
        assert_eq!(state.phase, Phase::Covered);
        assert_eq!(state.progress, 1.0);
        assert!(matches!(
            *world.resource::<NextState<GameState>>(),
            NextState::Pending(GameState::Battle)
        ));

        // Held, then revealed, then idle with nothing left behind.
        // (One phase advances per tick.)
        step(&mut world, HOLD_SECS);
        assert_eq!(world.resource::<TransitionState>().phase, Phase::In);
        step(&mut world, IN_SECS * 0.5);
        let state = world.resource::<TransitionState>();
        assert_eq!(state.phase, Phase::In);
        assert_eq!(state.progress, 0.5);
        step(&mut world, IN_SECS);
        let state = world.resource::<TransitionState>();
        assert_eq!(state.phase, Phase::Idle);
        assert_eq!(state.target, None);
        assert_eq!(quad_visibility(&mut world), Visibility::Hidden);
    }

    #[test]
    fn a_swirl_waits_for_the_frozen_frame() {
        let mut world = harness();
        begin(&mut world, GameState::Battle, Effect::Swirl);

        // A whole out-leg passes: the frame has not landed, so the
        // cover neither closes nor shows itself.
        step(&mut world, OUT_SECS);
        let state = world.resource::<TransitionState>();
        assert_eq!(state.phase, Phase::Out);
        assert_eq!(state.progress, 0.0);
        assert_eq!(quad_visibility(&mut world), Visibility::Hidden);

        // The frame lands (the capture observer sets this): the swirl
        // runs and the machine completes normally.
        world.resource_mut::<TransitionState>().frame_ready = true;
        step(&mut world, OUT_SECS);
        let state = world.resource::<TransitionState>();
        assert_eq!(state.phase, Phase::Covered);
        assert_eq!(quad_visibility(&mut world), Visibility::Visible);
    }

    #[test]
    fn a_stuck_capture_falls_back_to_a_fade() {
        let mut world = harness();
        begin(&mut world, GameState::Battle, Effect::Swirl);

        // Past the timeout with no frame: the effect downgrades to a
        // fade, which needs no texture and is on screen immediately.
        step(&mut world, CAPTURE_TIMEOUT_SECS + 0.1);
        let state = world.resource::<TransitionState>();
        assert_eq!(state.effect, Effect::Fade);
        assert_eq!(state.phase, Phase::Out);
        assert_eq!(quad_visibility(&mut world), Visibility::Visible);

        // And the fade still closes and flips.
        step(&mut world, OUT_SECS);
        assert_eq!(world.resource::<TransitionState>().phase, Phase::Covered);
    }

    #[test]
    fn a_second_request_while_busy_is_refused() {
        let mut world = harness();
        begin(&mut world, GameState::Battle, Effect::Fade);
        let mut state = world.resource_mut::<TransitionState>();
        assert!(!state.begin(GameState::Scene, Effect::Fade));
        assert_eq!(state.target, Some(GameState::Battle));
    }

    #[test]
    fn the_frame_readback_becomes_the_cover_texture() {
        let mut world = harness();
        let placeholder = world
            .resource::<Assets<TransitionMaterial>>()
            .get(&world.resource::<OverlayMaterial>().0)
            .unwrap()
            .frozen
            .clone();
        begin(&mut world, GameState::Battle, Effect::Swirl);

        // The machine ingests what the capture observer hands it.
        let expected = (GAME_WIDTH * GAME_HEIGHT * 4) as usize;
        let overlay = world.resource::<OverlayMaterial>().0.clone();
        world.resource_scope(|world, mut images: Mut<Assets<Image>>| {
            world.resource_scope(|world, mut materials: Mut<Assets<TransitionMaterial>>| {
                let mut state = world.resource_mut::<TransitionState>();
                ingest_frame(
                    &vec![7; expected],
                    &mut images,
                    &mut materials,
                    &overlay,
                    &mut state,
                );
            });
        });

        let state = world.resource::<TransitionState>();
        assert!(state.frame_ready);
        let material = world
            .resource::<Assets<TransitionMaterial>>()
            .get(&world.resource::<OverlayMaterial>().0)
            .unwrap();
        assert_ne!(material.frozen, placeholder, "the frame was swapped in");
    }

    #[test]
    fn a_malformed_capture_falls_back_to_a_fade() {
        let mut world = harness();
        begin(&mut world, GameState::Battle, Effect::Swirl);

        let overlay = world.resource::<OverlayMaterial>().0.clone();
        world.resource_scope(|world, mut images: Mut<Assets<Image>>| {
            world.resource_scope(|world, mut materials: Mut<Assets<TransitionMaterial>>| {
                let mut state = world.resource_mut::<TransitionState>();
                ingest_frame(&[0; 4], &mut images, &mut materials, &overlay, &mut state);
            });
        });

        let state = world.resource::<TransitionState>();
        assert_eq!(state.effect, Effect::Fade);
        assert!(!state.frame_ready);
    }

    #[test]
    fn the_swirl_queues_exactly_one_readback_and_fades_queue_none() {
        let mut world = harness();
        let image = world.resource_mut::<Assets<Image>>().add(black_image());
        world.insert_resource(crate::screen::GameImage(image));

        begin(&mut world, GameState::Battle, Effect::Swirl);
        world.run_system_once(capture).unwrap();
        world.run_system_once(capture).unwrap();
        let queued = world.query::<&Readback>().iter(&world).count();
        assert_eq!(queued, 1, "the second tick must not re-queue");

        // A fade never queues: it needs no frame.
        world.flush();
        *world.resource_mut::<TransitionState>() = TransitionState::default();
        begin(&mut world, GameState::Scene, Effect::Fade);
        world.run_system_once(capture).unwrap();
        let queued = world.query::<&Readback>().iter(&world).count();
        assert_eq!(queued, 1, "the swirl's readback is still the only one");
    }
}

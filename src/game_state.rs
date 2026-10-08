//! The game's top-level state: the boot menu, playing a scene,
//! transitioning between contexts (a battle starting, a battle ending
//! — scene warps later), or fighting a battle.
//!
//! Systems declare which state they belong to with `run_if(in_state(..))`
//! — the world freezes around a transition and a battle without any
//! ad-hoc checks — and each module suspends and restores its own
//! entities at the covered point of a transition, behind an opaque
//! screen. Battle.rs, scene.rs, and transition.rs share this vocabulary
//! and nothing else.

use bevy::prelude::*;

#[derive(States, Clone, PartialEq, Eq, Debug, Hash, Default)]
pub(crate) enum GameState {
    /// The boot screen: New Game / Continue. No scene is loaded yet.
    StartMenu,
    #[default]
    Scene,
    Transition,
    Battle,
}

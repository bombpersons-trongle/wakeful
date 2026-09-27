//! The world state: persistent key/value stores scripts can rely on.
//!
//! Three layers, all game-lifetime and all future save-file material:
//!
//! - the **shared store**: one map every script can read and write
//!   (`remember_global` / `recall_global`) — chest flags, quest steps;
//! - **per-identity stores**: an actor's private map, keyed by a
//!   game-wide id (`"actor::merchant"`) when the actor opts into
//!   sharing, or scene-scoped (`"{scene}::{id}"`) when it doesn't;
//!   id-less scripts fall back to their script path;
//! - everything is plain data, so serializing [`WorldState`] *is* the
//!   save file when that idea lands.

use bevy::prelude::*;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use rhai::Dynamic;

/// A script-writable map shared behind a handle.
pub type SharedMap = Arc<Mutex<BTreeMap<String, Dynamic>>>;

fn empty_map() -> SharedMap {
    Arc::new(Mutex::new(BTreeMap::new()))
}

/// The world's persistent state, owned by the game and shared into
/// script engines as handles.
#[derive(Resource, Clone, Default)]
pub struct WorldState {
    shared: SharedMap,
    /// Identity key -> that script instance's private store.
    scripts: Arc<Mutex<BTreeMap<String, SharedMap>>>,
}

impl WorldState {
    /// The store every script shares.
    pub fn shared(&self) -> SharedMap {
        self.shared.clone()
    }

    /// The private store for `key`, created on first use. The same key
    /// always yields the same map — that's the whole persistence
    /// contract.
    pub fn store_for(&self, key: &str) -> SharedMap {
        let mut scripts = self.scripts.lock().unwrap_or_else(PoisonError::into_inner);
        scripts
            .entry(key.to_owned())
            .or_insert_with(empty_map)
            .clone()
    }

    /// Read-only handle to the per-identity store registry, for debug
    /// dumps.
    #[cfg(debug_assertions)]
    pub fn debug_stores(&self) -> Arc<Mutex<BTreeMap<String, SharedMap>>> {
        self.scripts.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_for_is_stable_per_key_and_isolated_between_keys() {
        let state = WorldState::default();
        let chest1 = state.store_for("scene::chest1");
        chest1.lock().unwrap().insert("opened".into(), true.into());

        let again = state.store_for("scene::chest1");
        assert_eq!(
            again
                .lock()
                .unwrap()
                .get("opened")
                .and_then(|v| v.as_bool().ok()),
            Some(true)
        );

        let chest2 = state.store_for("scene::chest2");
        assert!(chest2.lock().unwrap().is_empty(), "isolated");
    }

    #[test]
    fn the_shared_store_is_one_map_for_all_callers() {
        let state = WorldState::default();
        state
            .shared()
            .lock()
            .unwrap()
            .insert("met".into(), 1.into());
        assert_eq!(
            state
                .shared()
                .lock()
                .unwrap()
                .get("met")
                .and_then(|v| v.as_int().ok()),
            Some(1)
        );
    }
}

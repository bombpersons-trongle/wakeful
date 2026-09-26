//! Game systems, grouped by concern. `main.rs` wires these into the app
//! and owns the shared components and resources they operate on.

pub mod actor;
pub mod animation;
pub mod bubble;
pub mod camera;
pub mod debug_draw;
pub mod input;
pub mod party;
pub mod player;
pub mod scene;
pub mod teleport;
pub mod ui;
pub mod world;
pub mod world_script;

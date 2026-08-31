//! Game systems, grouped by concern. `main.rs` wires these into the app
//! and owns the shared components and resources they operate on.

pub mod camera;
pub mod debug_draw;
pub mod input;
pub mod player;
pub mod scene;
pub mod world;

//! The shared core of wakeful: the scene file, the walkable ground it
//! points at, the assets-root lookup, and the geometry a baked plate
//! becomes.
//!
//! Two binaries sit on this. The game (`src/main.rs`) is one: it plays
//! scenes, moves a player through them, runs battles. The editor
//! (`src/bin/editor/`) is the other: it opens the same scene files and
//! shows them. Neither owns the format, so anything both must agree on
//! lives here rather than in either app.

pub mod assets;
pub mod depth_card_mesh;
pub mod scene;
pub mod scene_merge;
pub mod walkmesh;

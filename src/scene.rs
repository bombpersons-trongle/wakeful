//! Scene definitions: the data behind one "room" of the game — background
//! image, fixed camera pose, walkable area, and the player's character
//! model. Loaded from RON files in `assets/scenes/`.

use bevy::asset::Asset;
use bevy::math::Vec2;
use bevy::reflect::TypePath;
use serde::Deserialize;

#[derive(Asset, TypePath, Deserialize)]
pub struct Scene {
    /// Path to the background image, relative to `assets/`. Expected to be
    /// the game's virtual resolution (640x480), like a pre-rendered FF7 room.
    pub background: Option<String>,
    pub camera: CameraPose,
    pub walkable: Option<WalkableGrid>,
    /// Path to the player's glTF scene, e.g. `models/hero.gltf#Scene0`.
    /// When absent, the player is the placeholder capsule.
    pub character_model: Option<String>,
}

#[derive(Deserialize)]
pub struct CameraPose {
    pub position: [f32; 3],
    pub target: [f32; 3],
    pub fov_degrees: f32,
}

#[derive(Deserialize)]
pub struct WalkableGrid {
    /// World XZ position of the corner of cell `[0][0]`.
    pub origin: [f32; 2],
    pub cell_size: f32,
    pub cols: usize,
    pub rows: usize,
    /// Row-major walkable flags, starting at `origin`, +X along columns,
    /// +Z along rows.
    pub cells: Vec<bool>,
}

impl WalkableGrid {
    /// Whether the cell containing the world position is walkable.
    /// Positions outside the grid are never walkable.
    pub fn is_walkable(&self, x: f32, z: f32) -> bool {
        let Some((col, row)) = self.cell_at(x, z) else {
            return false;
        };
        self.cells
            .get(row * self.cols + col)
            .copied()
            .unwrap_or(false)
    }

    /// Grid column/row for a world position, if it falls inside the grid.
    fn cell_at(&self, x: f32, z: f32) -> Option<(usize, usize)> {
        if self.cell_size <= 0.0 {
            return None;
        }
        let col = ((x - self.origin[0]) / self.cell_size).floor();
        let row = ((z - self.origin[1]) / self.cell_size).floor();
        if col < 0.0 || row < 0.0 {
            return None;
        }
        let (col, row) = (col as usize, row as usize);
        if col >= self.cols || row >= self.rows {
            return None;
        }
        Some((col, row))
    }

    /// Restricts a desired movement so the result stays on walkable cells.
    ///
    /// Tries the full move first, then each axis alone, so the character
    /// slides along walls of blocked cells instead of sticking to them.
    pub fn constrain(&self, from: Vec2, to: Vec2) -> Vec2 {
        if self.is_walkable(to.x, to.y) {
            return to;
        }
        if self.is_walkable(to.x, from.y) {
            return Vec2::new(to.x, from.y);
        }
        if self.is_walkable(from.x, to.y) {
            return Vec2::new(from.x, to.y);
        }
        from
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> WalkableGrid {
        WalkableGrid {
            origin: [-2.0, -2.0],
            cell_size: 1.0,
            cols: 2,
            rows: 2,
            // only cell [0][0] (top-left) is walkable
            cells: [true, false, false, false].to_vec(),
        }
    }

    #[test]
    fn reports_walkable_cells() {
        let grid = grid();
        assert!(grid.is_walkable(-1.5, -1.5));
    }

    #[test]
    fn reports_blocked_cells() {
        let grid = grid();
        assert!(!grid.is_walkable(-0.5, -1.5));
        assert!(!grid.is_walkable(-1.5, -0.5));
        assert!(!grid.is_walkable(-0.5, -0.5));
    }

    #[test]
    fn outside_the_grid_is_never_walkable() {
        let grid = grid();
        assert!(!grid.is_walkable(50.0, 50.0));
        assert!(!grid.is_walkable(-3.0, -1.5));
        assert!(!grid.is_walkable(-1.5, -3.0));
    }

    #[test]
    fn positions_on_cell_edges_floor_consistently() {
        let grid = grid();
        // exactly on the boundary between cell 0 and cell 1 belongs to cell 1
        assert!(!grid.is_walkable(-1.0, -1.5));
    }

    #[test]
    fn constrain_keeps_free_moves() {
        let grid = grid();
        let from = Vec2::new(-1.5, -1.5);
        let to = Vec2::new(-1.4, -1.4);
        assert_eq!(grid.constrain(from, to), to);
    }

    #[test]
    fn constrain_slides_along_blocked_cells() {
        let grid = grid();
        // moving right into a blocked cell slides on Z, the free axis
        let from = Vec2::new(-1.5, -1.5);
        let to = Vec2::new(-0.5, -1.4);
        assert_eq!(grid.constrain(from, to), Vec2::new(-1.5, -1.4));
    }

    #[test]
    fn constrain_stops_when_surrounded() {
        let grid = grid();
        // diagonal target, X neighbor, and Z neighbor are all blocked
        let from = Vec2::new(-1.5, -1.5);
        let to = Vec2::new(-0.5, -0.4);
        assert_eq!(grid.constrain(from, to), from);
    }

    #[test]
    fn parses_scene_ron() {
        let src = r#"(
            background: Some("backgrounds/room1.png"),
            camera: (position: (0.0, 6.0, 9.0), target: (0.0, 0.0, 0.0), fov_degrees: 45.0),
            walkable: Some((
                origin: (-2.0, -2.0),
                cell_size: 1.0,
                cols: 2,
                rows: 2,
                cells: [true, false, false, true],
            )),
            character_model: None,
        )"#;
        let scene: Scene = ron::from_str(src).unwrap();
        assert_eq!(scene.camera.position, [0.0, 6.0, 9.0]);
        assert_eq!(scene.camera.fov_degrees, 45.0);
        let grid = scene.walkable.unwrap();
        assert!(grid.is_walkable(-1.5, -1.5));
        assert!(!grid.is_walkable(-0.5, -1.5));
    }

    #[test]
    fn ships_a_valid_devroom_scene() {
        // The file the game loads at startup must stay parseable, and its
        // spawn point (world origin) must remain walkable.
        let src = include_str!("../assets/scenes/devroom.scene");
        let scene: Scene = ron::from_str(src).unwrap();
        assert!(
            scene
                .walkable
                .expect("dev room needs a walkable grid")
                .is_walkable(0.0, 0.0)
        );
    }
}

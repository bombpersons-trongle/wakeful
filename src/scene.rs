//! Scene definitions: the data behind one "room" of the game — background
//! image, fixed camera pose, walkable area, and the player's character
//! model. Loaded from RON files in `assets/scenes/`.

use bevy::asset::Asset;
use bevy::math::Vec2;
use bevy::reflect::TypePath;
use serde::{Deserialize, Serialize};

#[derive(Asset, TypePath, Deserialize, Serialize)]
pub struct Scene {
    /// Path to the background image, relative to `assets/`. Expected to be
    /// the game's virtual resolution (320x240), like a pre-rendered FF7 room.
    pub background: Option<String>,
    pub camera: CameraPose,
    pub walkable: Option<WalkableGrid>,
    /// Path to the player's glTF model, e.g. `models/hero.glb`. The file's
    /// default scene is spawned; a `#SceneN` suffix, if present, is ignored.
    /// When absent, the player is the placeholder capsule.
    pub character_model: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Copy)]
pub struct CameraPose {
    pub position: [f32; 3],
    pub target: [f32; 3],
    pub fov_degrees: f32,
}

#[derive(Deserialize, Serialize)]
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

    /// Restricts a desired movement so the character (a circle of `radius`
    /// around its center) stays entirely on walkable cells.
    ///
    /// Tries the full move first, then each axis alone, so the character
    /// slides along walls of blocked cells instead of sticking to them.
    /// Radius `<= 0` constrains the center point alone.
    pub fn constrain(&self, from: Vec2, to: Vec2, radius: f32) -> Vec2 {
        if self.is_circle_walkable(to.x, to.y, radius) {
            return to;
        }
        if self.is_circle_walkable(to.x, from.y, radius) {
            return Vec2::new(to.x, from.y);
        }
        if self.is_circle_walkable(from.x, to.y, radius) {
            return Vec2::new(from.x, to.y);
        }
        from
    }

    /// Whether a circle of `radius` at the position lies entirely on
    /// walkable cells; cells outside the grid count as blocked. Radius
    /// `<= 0` reduces to the point test. Touching a blocked cell exactly
    /// still counts as walkable, so characters can rest against walls.
    pub fn is_circle_walkable(&self, x: f32, z: f32, radius: f32) -> bool {
        if self.cell_size <= 0.0 {
            return false;
        }
        if radius <= 0.0 {
            return self.is_walkable(x, z);
        }
        // Only cells overlapping the circle's bounding box can touch the
        // circle, so the distance test runs against those alone.
        let min_col = ((x - radius - self.origin[0]) / self.cell_size).floor() as i64;
        let max_col = ((x + radius - self.origin[0]) / self.cell_size).floor() as i64;
        let min_row = ((z - radius - self.origin[1]) / self.cell_size).floor() as i64;
        let max_row = ((z + radius - self.origin[1]) / self.cell_size).floor() as i64;
        for row in min_row..=max_row {
            for col in min_col..=max_col {
                let blocked = col < 0
                    || row < 0
                    || col as usize >= self.cols
                    || row as usize >= self.rows
                    || !self.cells[row as usize * self.cols + col as usize];
                if !blocked {
                    continue;
                }
                // Distance from the center to the closest point of the
                // cell's rect; overlap means the circle leaves the
                // walkable area.
                let min_x = self.origin[0] + col as f32 * self.cell_size;
                let min_z = self.origin[1] + row as f32 * self.cell_size;
                let dx = x - x.clamp(min_x, min_x + self.cell_size);
                let dz = z - z.clamp(min_z, min_z + self.cell_size);
                if dx * dx + dz * dz < radius * radius {
                    return false;
                }
            }
        }
        true
    }

    /// Sets the walkable flag of the cell containing the world position.
    /// Returns false (leaving the grid unchanged) if it lies outside.
    pub fn set_walkable(&mut self, x: f32, z: f32, walkable: bool) -> bool {
        let Some((col, row)) = self.cell_at(x, z) else {
            return false;
        };
        self.cells[row * self.cols + col] = walkable;
        true
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
        assert_eq!(grid.constrain(from, to, 0.0), to);
    }

    #[test]
    fn constrain_slides_along_blocked_cells() {
        let grid = grid();
        // moving right into a blocked cell slides on Z, the free axis
        let from = Vec2::new(-1.5, -1.5);
        let to = Vec2::new(-0.5, -1.4);
        assert_eq!(grid.constrain(from, to, 0.0), Vec2::new(-1.5, -1.4));
    }

    #[test]
    fn constrain_stops_when_surrounded() {
        let grid = grid();
        // diagonal target, X neighbor, and Z neighbor are all blocked
        let from = Vec2::new(-1.5, -1.5);
        let to = Vec2::new(-0.5, -0.4);
        assert_eq!(grid.constrain(from, to, 0.0), from);
    }

    #[test]
    fn constrain_rejects_moves_that_overhang_the_region() {
        // The center point of `to` is walkable, but a body of radius 0.4
        // around it would stick out past the grid's far edge.
        let mut grid = grid();
        grid.cols = 5;
        grid.rows = 5;
        grid.origin = [-2.5, -2.5];
        grid.cells = vec![true; 25];
        let from = Vec2::new(0.0, 0.0);
        let to = Vec2::new(0.0, -2.15);
        assert_eq!(grid.constrain(from, to, 0.4), from);
        // Close enough to keep the whole body inside: the move passes.
        let near_edge = Vec2::new(0.0, -2.05);
        assert_eq!(grid.constrain(from, near_edge, 0.4), near_edge);
    }

    #[test]
    fn a_body_fits_through_one_cell_wide_corridors() {
        // Only the middle row is walkable; a radius-0.4 body passes down
        // its centerline without touching the blocked rows.
        let mut grid = grid();
        grid.cols = 3;
        grid.rows = 3;
        grid.origin = [-1.5, -1.5];
        grid.cells = vec![false, false, false, true, true, true, false, false, false];
        let from = Vec2::new(-1.0, 0.0);
        let to = Vec2::new(1.0, 0.0);
        assert_eq!(grid.constrain(from, to, 0.4), to);
    }

    #[test]
    fn a_body_rests_against_blocked_cells() {
        // Moving toward a blocked cell stops where the body touches it.
        let mut grid = grid();
        grid.cols = 3;
        grid.rows = 1;
        grid.origin = [-1.5, -0.5];
        grid.cells = vec![true, true, false];
        let from = Vec2::new(-1.0, 0.0);
        let to = Vec2::new(0.5, 0.0);
        // The blocked cell spans x [0.5, 1.5]; the body's edge rests at its
        // boundary, so the center stops 0.4 short of it.
        assert_eq!(grid.constrain(from, to, 0.4), from);
        let resting = Vec2::new(0.1, 0.0);
        assert_eq!(grid.constrain(from, resting, 0.4), resting);
    }

    #[test]
    fn set_walkable_flips_the_cell_under_a_position() {
        let mut grid = grid();
        assert!(!grid.is_walkable(-0.5, -1.5));
        assert!(grid.set_walkable(-0.5, -1.5, true));
        assert!(grid.is_walkable(-0.5, -1.5));
        assert!(grid.set_walkable(-0.5, -1.5, false));
        assert!(!grid.is_walkable(-0.5, -1.5));
    }

    #[test]
    fn set_walkable_ignores_outside_positions() {
        let mut grid = grid();
        assert!(!grid.set_walkable(50.0, 50.0, true));
        assert_eq!(grid.cells, [true, false, false, false]);
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

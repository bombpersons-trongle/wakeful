//! Development aids for visualizing scene data.

use bevy::math::Isometry3d;
use bevy::prelude::*;

use crate::CurrentScene;
use crate::scene::{Scene, Teleporter, WalkableGrid};

/// The overlay is drawn just above the ground so the rects don't z-fight
/// with it.
const DEBUG_GRID_Y: f32 = 0.02;
const DEBUG_WALKABLE_COLOR: Color = Color::srgba(0.25, 0.9, 0.35, 0.4);
const DEBUG_BLOCKED_COLOR: Color = Color::srgba(0.9, 0.25, 0.2, 0.16);
/// Each cell's outline is drawn at this fraction of the cell size, leaving
/// a gap between neighboring rects. At full size adjacent outlines coincide
/// and the later-drawn rect overpaints the shared edge, hiding blocked
/// cells' red under walkable green.
const DEBUG_GRID_RECT_INSET: f32 = 0.9;
/// Teleporter outlines sit above the grid rects so both stay visible.
const DEBUG_TELEPORT_Y: f32 = 0.04;
const DEBUG_TELEPORT_COLOR: Color = Color::srgba(0.95, 0.55, 0.15, 0.6);

/// Toggled with F2: outlines the scene's walkable grid and teleporter
/// triggers on the ground so movement bounds and transition zones are
/// visible while testing.
pub fn debug_draw_walkables(
    keys: Res<ButtonInput<KeyCode>>,
    scenes: Res<Assets<Scene>>,
    current: Option<Res<CurrentScene>>,
    mut gizmos: Gizmos,
    mut enabled: Local<bool>,
) {
    if keys.just_pressed(KeyCode::F2) {
        *enabled = !*enabled;
    }
    if !*enabled {
        return;
    }
    let Some(scene) = current.as_ref().and_then(|c| scenes.get(&c.handle)) else {
        return;
    };
    if let Some(grid) = &scene.walkable {
        draw_walkable_grid(&mut gizmos, grid);
    }
    draw_teleporters(&mut gizmos, &scene.teleporters);
}

/// Draws one rect per cell: bright for walkable, dim for blocked.
pub fn draw_walkable_grid(gizmos: &mut Gizmos, grid: &WalkableGrid) {
    let rotation = Quat::from_rotation_x(-core::f32::consts::FRAC_PI_2);
    for row in 0..grid.rows {
        for col in 0..grid.cols {
            let walkable = grid
                .cells
                .get(row * grid.cols + col)
                .copied()
                .unwrap_or(false);
            let x = grid.origin[0] + (col as f32 + 0.5) * grid.cell_size;
            let z = grid.origin[1] + (row as f32 + 0.5) * grid.cell_size;
            gizmos.rect(
                Isometry3d::new(Vec3::new(x, DEBUG_GRID_Y, z), rotation),
                Vec2::splat(grid.cell_size * DEBUG_GRID_RECT_INSET),
                if walkable {
                    DEBUG_WALKABLE_COLOR
                } else {
                    DEBUG_BLOCKED_COLOR
                },
            );
        }
    }
}

/// Draws one rect per teleporter trigger.
pub fn draw_teleporters(gizmos: &mut Gizmos, teleporters: &[Teleporter]) {
    let rotation = Quat::from_rotation_x(-core::f32::consts::FRAC_PI_2);
    for teleporter in teleporters {
        gizmos.rect(
            Isometry3d::new(
                Vec3::new(
                    teleporter.position[0],
                    DEBUG_TELEPORT_Y,
                    teleporter.position[1],
                ),
                rotation,
            ),
            Vec2::new(teleporter.size[0], teleporter.size[1]),
            DEBUG_TELEPORT_COLOR,
        );
    }
}

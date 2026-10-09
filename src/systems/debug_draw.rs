//! Development aids for visualizing scene data.

use bevy::math::Isometry3d;
use bevy::prelude::*;

use crate::CurrentScene;
use crate::scene::{Scene, Teleporter};
use crate::walkmesh::WalkMesh;

/// The walk-mesh overlay hovers just above the mesh so the lines don't
/// z-fight with it.
const DEBUG_MESH_Y: f32 = 0.02;
const DEBUG_WALKABLE_COLOR: Color = Color::srgba(0.25, 0.9, 0.35, 0.4);
/// Teleporter outlines sit above the mesh lines so both stay visible.
const DEBUG_TELEPORT_Y: f32 = 0.04;
const DEBUG_TELEPORT_COLOR: Color = Color::srgba(0.95, 0.55, 0.15, 0.6);

/// Toggled with F2: outlines the scene's walk mesh and teleporter triggers
/// so movement bounds and transition zones are visible while testing.
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
    if let Some(mesh) = &scene.walk_mesh {
        draw_walk_mesh(&mut gizmos, mesh);
    }
    draw_teleporters(&mut gizmos, &scene.teleporters);
}

/// Draws every triangle as a line loop, so the walkable surface and its
/// edges are visible where they actually are in the world.
pub fn draw_walk_mesh(gizmos: &mut Gizmos, mesh: &WalkMesh) {
    let lift = |v: [f32; 3]| Vec3::new(v[0], v[1] + DEBUG_MESH_Y, v[2]);
    for triangle in &mesh.triangles {
        let points: Vec<Vec3> = triangle
            .iter()
            .filter_map(|i| mesh.vertices.get(*i as usize))
            .map(|v| lift(*v))
            .collect();
        if points.len() == 3 {
            gizmos.line(points[0], points[1], DEBUG_WALKABLE_COLOR);
            gizmos.line(points[1], points[2], DEBUG_WALKABLE_COLOR);
            gizmos.line(points[2], points[0], DEBUG_WALKABLE_COLOR);
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

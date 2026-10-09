//! Development aids for visualizing scene data.

use bevy::math::Isometry3d;
use bevy::prelude::*;

use crate::CurrentScene;
use crate::scene::{Scene, Teleporter};

/// Teleporter outlines sit above the mesh lines so both stay visible.
const DEBUG_TELEPORT_Y: f32 = 0.04;
const DEBUG_TELEPORT_COLOR: Color = Color::srgba(0.95, 0.55, 0.15, 0.6);

/// Toggled with F2: outlines the scene's walk mesh and teleporter triggers
/// so movement bounds and transition zones are visible while testing.
/// The editor draws the mesh unconditionally instead — `walkmesh::draw`.
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
        crate::walkmesh::draw(&mut gizmos, mesh);
    }
    draw_teleporters(&mut gizmos, &scene.teleporters);
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

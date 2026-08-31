//! Static world setup: the placeholder ground and lighting.

use bevy::prelude::*;

/// Size of the placeholder ground plane.
const GROUND_SIZE: f32 = 30.0;
const GROUND_COLOR: Color = Color::srgb(0.23, 0.21, 0.28);

pub fn spawn_world(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Placeholder floor; a pre-rendered background image arrives with real art.
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(GROUND_SIZE, GROUND_SIZE))),
        MeshMaterial3d(materials.add(StandardMaterial::from_color(GROUND_COLOR))),
    ));

    commands.spawn((
        DirectionalLight::default(),
        Transform::from_xyz(4.0, 8.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

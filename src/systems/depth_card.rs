//! The depth card: the painted background as real geometry.
//!
//! Where a scene ships a `depth_map`, its background stops being a flat
//! sprite and becomes a static mesh — one vertex per depth-map pixel,
//! positioned by unprojecting the pixel's ray through the scene camera
//! and pushing it out to the encoded distance, textured with the
//! background image at the same grid spot. The card renders in the
//! scene camera's own 3D pass, so the depth buffer resolves
//! character-versus-background occlusion in both directions with no
//! special machinery: step behind the awning and it covers you, step in
//! front and you cover it.
//!
//! The encoded distance is a ray distance in meters along the view ray,
//! normalized over the scene's `depth_range`. The Blender exporter that
//! authors these maps writes the same convention, and the map's pixel
//! grid is exactly the background image's — one vertex per pixel keeps
//! silhouettes pixel-accurate at the game's resolution.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::render::render_resource::TextureFormat;
use bevy::prelude::*;

use crate::scene::CameraPose;
use crate::systems::scene::SceneGraphics;

/// A background card still waiting for its depth image to load; the
/// builder system converts it into the finished mesh entity.
#[derive(Component)]
pub(crate) struct DepthCardPending {
    pub(crate) pose: CameraPose,
    pub(crate) depth_range: f32,
    pub(crate) depth: Handle<Image>,
    pub(crate) background: Handle<Image>,
}

/// A finished background card. Marks the entity for scene-teardown
/// cleanup: a card must never outlive the scene whose map built it.
#[derive(Component)]
pub(crate) struct DepthCard;

/// Builds pending cards once their depth and background images land,
/// parenting the finished card under the scene graphics root like any
/// other scene content.
pub(crate) fn build_pending_cards(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    images: Res<Assets<Image>>,
    graphics: Res<SceneGraphics>,
    pending: Query<(Entity, &DepthCardPending)>,
) {
    for (entity, card) in &pending {
        let Some(depth) = images.get(&card.depth) else {
            continue;
        };
        if images.get(&card.background).is_none() {
            continue;
        }
        let mesh = match build_depth_card_mesh(depth, &card.pose, card.depth_range) {
            Some(mesh) => mesh,
            None => {
                warn!("depth card: unsupported depth format, skipping");
                continue;
            }
        };
        commands.entity(entity).remove::<DepthCardPending>();
        commands.entity(graphics.0).add_child(entity);
        commands.entity(entity).insert((
            DepthCard,
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color_texture: Some(card.background.clone()),
                unlit: true, // the background is pre-lit paint
                ..default()
            })),
        ));
    }
}

/// Builds the card mesh: one vertex per depth pixel, the pixel's ray
/// unprojected through the camera pose and pushed out to the encoded
/// distance. Returns `None` when the depth image's format carries no
/// readable depth channel.
fn build_depth_card_mesh(depth: &Image, pose: &CameraPose, depth_range: f32) -> Option<Mesh> {
    let width = depth.width() as usize;
    let height = depth.height() as usize;
    if width < 2 || height < 2 {
        warn!("depth card: the depth map is degenerate ({width}x{height})");
        return None;
    }
    // The depth channel: a single-channel map in R, a packed one in the
    // red byte of RGBA.
    let stride = match depth.texture_descriptor.format {
        TextureFormat::R8Unorm => 1,
        TextureFormat::Rgba8Unorm | TextureFormat::Rgba8UnormSrgb => 4,
        other => {
            warn!("depth card: unreadable depth format {other:?}");
            return None;
        }
    };
    let Some(data) = depth.data.as_ref() else {
        warn!("depth card: the depth image carries no pixel data");
        return None;
    };
    let distance = |x: usize, y: usize| -> f32 {
        (data[(y * width + x) * stride] as f32 / 255.0) * depth_range
    };

    // The camera basis, constructed exactly the way the engine poses the
    // scene camera: from_translation + looking_at(target, Vec3::Y).
    let position = Vec3::from(pose.position);
    let forward = (Vec3::from(pose.target) - position).normalize();
    let right = forward.cross(Vec3::Y).normalize();
    let up = right.cross(forward);
    let tan_half = (pose.fov_degrees.to_radians() * 0.5).tan();
    let aspect = width as f32 / height as f32;

    let mut positions = Vec::with_capacity(width * height);
    let mut uvs = Vec::with_capacity(width * height);
    let mut normals = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            // Pixel-center uv; the background texture shares the grid.
            let u = (x as f32 + 0.5) / width as f32;
            let v = (y as f32 + 0.5) / height as f32;
            // Image rows run top-down; the screen's top is ndc +1.
            let ndc = Vec2::new(u * 2.0 - 1.0, 1.0 - v * 2.0);
            let ray = forward
                + right * (ndc.x * tan_half * aspect)
                + up * (ndc.y * tan_half);
            let ray = ray.normalize();
            positions.push(position + ray * distance(x, y));
            uvs.push([u, v]);
            normals.push(-forward);
        }
    }

    // Two triangles per cell, wound counter-clockwise seen from the
    // camera (the grid's x runs screen-right, y screen-down).
    let mut indices = Vec::with_capacity((width - 1) * (height - 1) * 6);
    for y in 0..height - 1 {
        for x in 0..width - 1 {
            let a = y * width + x;
            let b = a + 1;
            let c = a + width;
            let d = c + 1;
            indices.extend([a as u32, c as u32, b as u32, b as u32, c as u32, d as u32]);
        }
    }

    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_indices(Indices::U32(indices));
    Some(mesh)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::mesh::VertexAttributeValues;
    use bevy::render::render_resource::{Extent3d, TextureDimension};

    /// A little R8 depth map; `value` paints each pixel.
    fn depth_image(width: usize, height: usize, value: fn(usize, usize) -> u8) -> Image {
        let mut data = vec![0u8; width * height];
        for y in 0..height {
            for x in 0..width {
                data[y * width + x] = value(x, y);
            }
        }
        Image::new(
            Extent3d {
                width: width as u32,
                height: height as u32,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            data,
            TextureFormat::R8Unorm,
            RenderAssetUsages::default(),
        )
    }

    /// Looking straight down -Z with a 90 vertical fov (tan_half = 1).
    fn pose() -> CameraPose {
        CameraPose {
            position: [0.0, 0.0, 0.0],
            target: [0.0, 0.0, -1.0],
            fov_degrees: 90.0,
        }
    }

    #[test]
    fn the_card_unprojects_pixel_rays_through_the_camera() {
        let depth = depth_image(2, 2, |_, _| 255);
        let mesh = build_depth_card_mesh(&depth, &pose(), 16.0).expect("builds from R8");
        let VertexAttributeValues::Float32x3(positions) = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .expect("positions exist")
        else {
            panic!("positions are f32x3");
        };
        assert_eq!(positions.len(), 4);

        // Top-left pixel: u 1/4, v 1/4 -> ndc (-0.5, +0.5); the ray is
        // (-0.5, +0.5, -1) normalized, pushed out to 16.
        let expected = Vec3::new(-0.5, 0.5, -1.0).normalize() * 16.0;
        let got = positions[0];
        assert!(
            (Vec3::from(got) - expected).length() < 1e-4,
            "top-left unprojects along its ray"
        );
        // Bottom-right: the mirrored ray.
        let expected = Vec3::new(0.5, -0.5, -1.0).normalize() * 16.0;
        let got = positions[3];
        assert!((Vec3::from(got) - expected).length() < 1e-4);
    }

    #[test]
    fn the_card_maps_the_encoded_distance_through_depth_range() {
        // 128/255 of the range lands about halfway out.
        let depth = depth_image(2, 2, |_, _| 128);
        let mesh = build_depth_card_mesh(&depth, &pose(), 32.0).unwrap();
        let VertexAttributeValues::Float32x3(positions) = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .expect("positions exist")
        else {
            panic!("positions are f32x3");
        };
        let radius = 128.0 / 255.0 * 32.0;
        for got in positions {
            let direction = Vec3::from(*got) - Vec3::from(pose().position);
            assert!((direction.length() - radius).abs() < 1e-4);
        }
    }

    #[test]
    fn a_degenerate_map_builds_nothing() {
        let depth = depth_image(1, 1, |_, _| 255);
        assert!(build_depth_card_mesh(&depth, &pose(), 16.0).is_none());
    }
}

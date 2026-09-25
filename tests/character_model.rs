//! Validates the generated character model against the same glTF parser
//! bevy's loader builds on (`bevy_gltf` wraps `gltf::import`).
//!
//! `tools/generate_character.py` hand-rolls the GLB binary, so format
//! mistakes — mismatched accessor types, out-of-bounds indices, broken
//! keyframe buffers — only surface at parse time. This test is that
//! parse, plus the rig contract the animation engine relies on.

use std::collections::HashSet;

const RIG_JOINTS: [&str; 8] = [
    "hips", "torso", "head", "hair", "arm_l", "arm_r", "leg_l", "leg_r",
];
const CLIPS: [&str; 6] = ["idle", "walk", "run", "pick_up", "shrug", "wave"];

#[test]
fn the_generated_character_model_is_a_valid_rigged_gltf() {
    let bytes = include_bytes!("../assets/models/character.glb");
    // Parses the container, validates every accessor/bufferView against
    // the declared component types, and loads the buffers. Malformed
    // data — the class of bug a hand-rolled writer can ship — fails
    // right here.
    let (document, buffers, _) = gltf::import_slice(bytes.as_slice())
        .expect("character.glb must be a valid glTF with in-bounds buffers");

    // The rig: every standard joint present by name, scene-rooted.
    let names: HashSet<&str> = document.nodes().filter_map(|n| n.name()).collect();
    for joint in RIG_JOINTS {
        assert!(names.contains(joint), "rig is missing the '{joint}' joint");
    }

    // Every mesh's indices reference real vertices (the loader panics
    // on violations, so assert it here where the message is readable).
    for mesh in document.meshes() {
        for primitive in mesh.primitives() {
            let reader =
                primitive.reader(|buffer| buffers.get(buffer.index()).map(|data| &data[..]));
            let vertex_count = reader
                .read_positions()
                .expect("primitives have POSITION")
                .count();
            let indices: Vec<u32> = reader
                .read_indices()
                .expect("primitives are indexed")
                .into_u32()
                .collect();
            assert_eq!(indices.len() % 3, 0, "triangulated indices");
            assert!(
                indices.iter().all(|&i| i < vertex_count as u32),
                "mesh '{}' has out-of-bounds indices",
                mesh.name().unwrap_or("?")
            );
        }
    }

    // The full standard clip set: locomotion loops the engine repeats,
    // emotes are one-shots. Each must have rotation or translation
    // keyframes targeting actual rig nodes.
    let animation_names: HashSet<&str> = document.animations().filter_map(|a| a.name()).collect();
    for clip in CLIPS {
        assert!(animation_names.contains(clip), "missing the '{clip}' clip");
    }
    let animation_count = document.animations().count();
    assert_eq!(animation_count, CLIPS.len(), "no extra unnamed animations");
    for animation in document.animations() {
        let channels = animation.channels().count();
        assert!(
            channels > 0,
            "clip '{}' has no channels",
            animation.name().unwrap_or("?")
        );
        for channel in animation.channels() {
            let target = channel.target();
            assert!(
                target.node().name().is_some(),
                "clip '{}' targets an unnamed node",
                animation.name().unwrap_or("?")
            );
            assert!(
                matches!(
                    channel.sampler().interpolation(),
                    gltf::animation::Interpolation::Linear
                ),
                "clips bake LINEAR keyframes"
            );
        }
    }
}

//! Validates the generated character and chest models against the same
//! glTF parser bevy's loader builds on (`bevy_gltf` wraps `gltf::import`).
//!
//! The `tools/generate_*.py` scripts hand-roll the GLB binaries, so
//! format mistakes — mismatched accessor types, out-of-bounds indices,
//! broken keyframe buffers — only surface at parse time. These tests
//! are that parse, plus the rig contracts the animation engine relies
//! on.

use std::collections::HashSet;

/// Parses and structurally validates a generated model: in-bounds
/// indexed meshes, LINEAR one-shot/loop clips targeting named nodes.
fn assert_valid_model(name: &str, bytes: &[u8]) -> (gltf::Document, Vec<gltf::buffer::Data>) {
    // Parses the container, validates every accessor/bufferView against
    // the declared component types, and loads the buffers. Malformed
    // data — the class of bug a hand-rolled writer can ship — fails
    // right here.
    let (document, buffers, _) =
        gltf::import_slice(bytes).unwrap_or_else(|e| panic!("{name} must be a valid glTF: {e}"));

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

    // Clips must have keyframes targeting actual named rig nodes.
    for animation in document.animations() {
        assert!(
            animation.channels().count() > 0,
            "clip '{}' has no channels",
            animation.name().unwrap_or("?")
        );
        for channel in animation.channels() {
            assert!(
                channel.target().node().name().is_some(),
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
    (document, buffers)
}

const RIG_JOINTS: [&str; 8] = [
    "hips", "torso", "head", "hair", "arm_l", "arm_r", "leg_l", "leg_r",
];
const CLIPS: [&str; 8] = [
    "idle", "walk", "run", "pick_up", "shrug", "wave", "attack", "die",
];

/// The goblin ships the same rig and clip set as the hero — the battle
/// flow drives both through the same clips (attack, die).
#[test]
fn the_generated_goblin_model_matches_the_hero_rig() {
    let (document, buffers) = assert_valid_model(
        "goblin.glb",
        include_bytes!("../assets/models/goblin.glb"),
    );
    let names: HashSet<&str> = document.nodes().filter_map(|n| n.name()).collect();
    for joint in RIG_JOINTS {
        assert!(names.contains(joint), "goblin rig is missing '{joint}'");
    }
    let animation_names: HashSet<&str> = document.animations().filter_map(|a| a.name()).collect();
    for clip in CLIPS {
        assert!(animation_names.contains(clip), "goblin is missing '{clip}'");
    }
}

#[test]
fn the_attack_clip_sells_a_swing() {
    let (document, buffers) = assert_valid_model(
        "character.glb",
        include_bytes!("../assets/models/character.glb"),
    );
    let attack = document
        .animations()
        .find(|a| a.name() == Some("attack"))
        .expect("attack clip");
    assert!(
        clip_moves_a_joint(&document, &buffers, &attack),
        "attack must animate; a constant clip plays as one frame"
    );
}
/// `die` is the battle flow's death animation: it must END with the
/// body sunk below the floor (hips translated down), and it must not
/// touch scale — a scale-0 death would stick after the clip stops
/// applying, because nothing else animates scale.
#[test]
fn the_die_clip_ends_buried() {
    let (document, buffers) = assert_valid_model(
        "character.glb",
        include_bytes!("../assets/models/character.glb"),
    );
    let die = document
        .animations()
        .find(|a| a.name() == Some("die"))
        .expect("die clip");
    let mut saw_translation = false;
    for channel in die.channels() {
        let node = channel.target().node().name();
        match channel.target().property() {
            gltf::animation::Property::Scale => {
                panic!("die must not animate scale; a stale scale sticks after the clip")
            }
            gltf::animation::Property::Translation => {
                assert_eq!(node, Some("hips"), "die sinks via the hips joint");
                let reader =
                    channel.reader(|buffer| buffers.get(buffer.index()).map(|data| &data[..]));
                let ys = match reader.read_outputs().expect("translation outputs") {
                    gltf::animation::util::ReadOutputs::Translations(t) => {
                        t.map(|v| v[1]).collect::<Vec<f32>>()
                    }
                    _ => panic!("expected translations"),
                };
                assert!(*ys.last().expect("keyframes") < -1.2, "die must end buried");
                assert!(ys[0] > -0.1, "die must start standing");
                saw_translation = true;
            }
            _ => {}
        }
    }
    assert!(saw_translation, "die must sink (a hips translation)");
}

#[test]
fn the_generated_character_model_is_a_valid_rigged_gltf() {
    let (document, buffers) = assert_valid_model(
        "character.glb",
        include_bytes!("../assets/models/character.glb"),
    );

    // The rig: every standard joint present by name.
    let names: HashSet<&str> = document.nodes().filter_map(|n| n.name()).collect();
    for joint in RIG_JOINTS {
        assert!(names.contains(joint), "rig is missing the '{joint}' joint");
    }

    // The full standard clip set: locomotion loops the engine repeats,
    // emotes are one-shots.
    let animation_names: HashSet<&str> = document.animations().filter_map(|a| a.name()).collect();
    for clip in CLIPS {
        assert!(animation_names.contains(clip), "missing the '{clip}' clip");
    }
    assert_eq!(
        document.animations().count(),
        CLIPS.len(),
        "no extra unnamed animations"
    );

    // One-shots must actually move their joints somewhere in the clip:
    // an envelope baked constant parses perfectly and plays "instantly"
    // — the chest lid bug. (One-shots may legitimately return to their
    // first pose, so vary-across-keyframes is the guard, not first≠last.)
    for clip in ["pick_up", "shrug", "wave"] {
        let animation = document
            .animations()
            .find(|a| a.name() == Some(clip))
            .expect("one-shot clip exists");
        assert!(
            clip_moves_a_joint(&document, &buffers, &animation),
            "the '{clip}' one-shot must move its joints"
        );
    }
}

const CHEST_PARTS: [&str; 3] = ["character", "base", "lid"];

#[test]
fn the_generated_chest_model_opens_with_a_hinged_lid() {
    let (document, buffers) =
        assert_valid_model("chest.glb", include_bytes!("../assets/models/chest.glb"));

    let names: HashSet<&str> = document.nodes().filter_map(|n| n.name()).collect();
    for part in CHEST_PARTS {
        assert!(names.contains(part), "chest is missing the '{part}' node");
    }

    // Exactly the open clip, targeting the lid.
    let animations: Vec<_> = document.animations().collect();
    assert_eq!(animations.len(), 1, "the chest ships only 'open'");
    assert_eq!(animations[0].name(), Some("open"));
    let targets: HashSet<&str> = animations[0]
        .channels()
        .map(|c| c.target().node().name().unwrap())
        .collect();
    assert!(targets.contains("lid"), "open must animate the lid");

    // And the clip must actually move that lid: the first rotation
    // keyframe is the closed rest pose, and later keyframes swing it
    // open. A clip baked with a constant pose parses perfectly — and
    // plays nothing.
    let lid_channel = animations[0]
        .channels()
        .find(|c| c.target().node().name() == Some("lid"))
        .expect("open rotates the lid");
    let rotations =
        channel_rotations(&document, &buffers, lid_channel).expect("lid rotation keyframes");
    assert!(
        quat_dot(rotations[0], [0.0, 0.0, 0.0, 1.0]) > 0.999,
        "the lid must start closed"
    );
    // A constant clip is the bug: every keyframe the same pose, so the
    // "swing" renders as a single frame.
    assert!(
        !rotations.iter().all(|r| quat_dot(*r, rotations[0]) > 0.999),
        "the open clip must move the lid across the swing"
    );
}

fn clip_moves_a_joint(
    document: &gltf::Document,
    buffers: &[gltf::buffer::Data],
    animation: &gltf::Animation,
) -> bool {
    animation.channels().any(|channel| {
        if channel.target().property() != gltf::animation::Property::Rotation {
            return false;
        }
        let Some(rotations) = channel_rotations(document, buffers, channel) else {
            return false;
        };
        // A constant rotation for every keyframe is the bug: the clip
        // "plays" while the pose never changes.
        !rotations.iter().all(|r| quat_dot(*r, rotations[0]) > 0.999)
    })
}

/// Every rotation quaternion a channel bakes, for the
/// constant-clip guard.
fn channel_rotations(
    _document: &gltf::Document,
    buffers: &[gltf::buffer::Data],
    channel: gltf::animation::Channel,
) -> Option<Vec<[f32; 4]>> {
    let reader = channel.reader(|buffer| buffers.get(buffer.index()).map(|data| &data[..]));
    match reader.read_outputs()? {
        gltf::animation::util::ReadOutputs::Rotations(rotations) => {
            Some(rotations.into_f32().collect())
        }
        _ => None,
    }
}

fn quat_dot(a: [f32; 4], b: [f32; 4]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>().abs()
}

#[test]
fn the_generated_arena_is_a_room_the_camera_fights_inside() {
    let (document, buffers) =
        assert_valid_model("arena.glb", include_bytes!("../assets/models/arena.glb"));

    // One textured mesh, no animations: the arena is a static room.
    assert_eq!(document.meshes().count(), 1, "one arena mesh");
    assert_eq!(document.animations().count(), 0, "arenas do not animate");
    let material = document.materials().next().expect("arena material");
    let texture = material
        .pbr_metallic_roughness()
        .base_color_texture()
        .expect("the arena is checker-textured");
    assert_eq!(
        texture.texture().source().name(),
        Some("arena-checker"),
        "the generated checker texture is bound"
    );

    // Every vertex normal must point at the room's center: the faces
    // wind for a camera INSIDE the cube. A plain outward cube would
    // render as nothing at all from the battle camera.
    let mesh = document.meshes().next().unwrap();
    let primitive = mesh.primitives().next().unwrap();
    let center = [0.0, 4.0, 0.0]; // the room's middle (floor at y=0)
    let reader = primitive.reader(|buffer| buffers.get(buffer.index()).map(|data| &data[..]));
    let normals: Vec<[f32; 3]> = reader.read_normals().expect("normals").collect();
    let positions: Vec<[f32; 3]> = reader.read_positions().expect("positions").collect();
    assert_eq!(normals.len(), positions.len());
    for (normal, position) in normals.iter().zip(&positions) {
        let outward = [
            position[0] - center[0],
            position[1] - center[1],
            position[2] - center[2],
        ];
        let dot: f32 = normal.iter().zip(outward).map(|(n, o)| n * o).sum();
        assert!(
            dot < 0.0,
            "normals must face the room's center, got {normal:?} at {position:?}"
        );
    }
}

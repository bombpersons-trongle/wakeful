//! Debug screenshots: touch a request file, get the game frame back.
//!
//! Gated on `cfg(debug_assertions)`: present in every dev build (so a
//! running game can always be inspected), absent from release. The
//! "API" is the filesystem: a `shot-<nonce>.request` file in `.debug/`
//! asks for a capture of the clean 320x240 game image, and the PNG
//! lands as `shot-<nonce>.png` a frame or two later. No network, no
//! ports; the readback uses Bevy's own `gpu_readback` (the `Screenshot`
//! component's image-target path ships frames of uninitialized data in
//! 0.19, so it can't be used here).

use std::path::{Path, PathBuf};

use bevy::asset::RenderAssetUsages;
use bevy::image::Image;
use bevy::prelude::*;
use bevy::render::gpu_readback::{Readback, ReadbackComplete};
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::screen::{GAME_HEIGHT, GAME_WIDTH, GameImage};

/// Where request and response files live, relative to the repo.
const SHOT_DIR: &str = ".debug";

/// The folder requests are scanned from.
#[derive(Resource, Clone)]
pub struct ShotDir(PathBuf);

pub fn setup(mut commands: Commands) {
    let dir = PathBuf::from(SHOT_DIR);
    let _ = std::fs::create_dir_all(&dir);
    commands.insert_resource(ShotDir(dir));
}

/// Finds the pending `<prefix><name>.request` files under `dir`.
fn scan_requests(dir: &Path, prefix: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<_> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            name.strip_prefix(prefix)?
                .strip_suffix(".request")
                .map(str::to_owned)
        })
        .collect();
    names.sort();
    names
}

fn scan_shot_requests(dir: &Path) -> Vec<String> {
    scan_requests(dir, "shot-")
}

/// Serves every pending request: read the clean game image back from
/// the GPU, delete the request. The PNG lands when the readback
/// completes, a frame or two later.
pub fn check_requests(
    mut commands: Commands,
    dir: Option<Res<ShotDir>>,
    game_image: Option<Res<GameImage>>,
) {
    let (Some(dir), Some(game_image)) = (dir, game_image) else {
        return;
    };
    for nonce in scan_shot_requests(&dir.0) {
        let request = dir.0.join(format!("shot-{nonce}.request"));
        match std::fs::remove_file(&request) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Another request file won the race; it'll serve next frame.
                continue;
            }
            Err(e) => {
                warn!("screenshot request {nonce} could not be cleared: {e}");
                continue;
            }
        }
        let path = dir.0.join(format!("shot-{nonce}.png"));
        commands
            .spawn(Readback::texture(game_image.0.clone()))
            .observe(
                move |trigger: On<ReadbackComplete>, mut commands: Commands| {
                    // The readback repeats every frame until the
                    // component comes off; one shot per request.
                    commands.entity(trigger.entity).remove::<Readback>();
                    save_png(&path, &trigger.data);
                },
            );
    }
}

fn save_png(path: &Path, data: &[u8]) {
    let expected = (GAME_WIDTH * GAME_HEIGHT * 4) as usize;
    if data.len() != expected {
        warn!(
            "screenshot readback was {} bytes, expected {expected}; not saving",
            data.len()
        );
        return;
    }
    let image = Image::new(
        Extent3d {
            width: GAME_WIDTH,
            height: GAME_HEIGHT,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data.to_vec(),
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::MAIN_WORLD,
    );
    let result = image
        .try_into_dynamic()
        .map_err(|e| e.to_string())
        .and_then(|dynamic| {
            dynamic
                .to_rgb8()
                .save_with_format(path.to_string_lossy().as_ref(), image::ImageFormat::Png)
                .map_err(|e| e.to_string())
        });
    match result {
        Ok(()) => info!("screenshot saved to {}", path.display()),
        Err(e) => warn!("screenshot could not be saved: {e}"),
    }
}

/// Serves `tap-<action>`, `hold-<action>`, and `release-<action>`
/// request files against the injected-input layer: taps fire for one
/// tick, holds press until the matching release (with press/release
/// edges on transition). Unknown actions warn once and are consumed.
pub fn check_input_requests(
    dir: Option<Res<ShotDir>>,
    injected: Option<ResMut<crate::input::InjectedInputs>>,
) {
    let (Some(dir), Some(mut injected)) = (dir, injected) else {
        return;
    };
    injected.just_pressed.clear();
    injected.just_released.clear();

    for action in scan_requests(&dir.0, "hold-") {
        let request = dir.0.join(format!("hold-{action}.request"));
        if consume(&request).is_err() {
            continue;
        }
        match crate::input::PadButton::from_config(&action) {
            Some(button) => {
                if injected.held.insert(button) {
                    injected.just_pressed.insert(button);
                }
            }
            None => warn!("input request names unknown action '{action}'"),
        }
    }
    for action in scan_requests(&dir.0, "release-") {
        let request = dir.0.join(format!("release-{action}.request"));
        if consume(&request).is_err() {
            continue;
        }
        match crate::input::PadButton::from_config(&action) {
            Some(button) => {
                if injected.held.remove(&button) {
                    injected.just_released.insert(button);
                }
            }
            None => warn!("input request names unknown action '{action}'"),
        }
    }
    // Taps last, so a hold+tap in the same tick still reads an edge.
    for action in scan_requests(&dir.0, "tap-") {
        let request = dir.0.join(format!("tap-{action}.request"));
        if consume(&request).is_err() {
            continue;
        }
        match crate::input::PadButton::from_config(&action) {
            Some(button) => {
                injected.just_pressed.insert(button);
            }
            None => warn!("input request names unknown action '{action}'"),
        }
    }
}

/// Deletes a consumed request file; `Err` means another scanner pass
/// already took it this frame.
fn consume(request: &Path) -> std::io::Result<()> {
    std::fs::remove_file(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::PadButton;
    use bevy::ecs::system::RunSystemOnce;

    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wakeful-shot-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn scan_finds_request_nonces_in_order_and_ignores_the_rest() {
        let dir = scratch_dir("scan");
        std::fs::write(dir.join("shot-300.request"), "").unwrap();
        std::fs::write(dir.join("shot-100.request"), "").unwrap();
        std::fs::write(dir.join("shot-200.png"), "").unwrap();
        std::fs::write(dir.join("unrelated.txt"), "").unwrap();

        assert_eq!(
            scan_requests(&dir, "shot-"),
            vec!["100".to_owned(), "300".to_owned()]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_folder_scans_as_empty() {
        assert!(scan_requests(Path::new("/nonexistent/wakeful-shots"), "shot-").is_empty());
    }

    #[test]
    fn input_requests_drive_the_injected_layer() {
        let dir = scratch_dir("input");
        let mut world = World::new();
        world.insert_resource(ShotDir(dir.clone()));
        world.init_resource::<crate::input::InjectedInputs>();

        std::fs::write(dir.join("tap-triangle.request"), "").unwrap();
        std::fs::write(dir.join("hold-dpad_up.request"), "").unwrap();
        std::fs::write(dir.join("bogus-what.request"), "").unwrap();
        world.run_system_once(check_input_requests).unwrap();

        let injected = world.resource::<crate::input::InjectedInputs>();
        assert!(injected.just_pressed.contains(&PadButton::Triangle));
        assert!(injected.held.contains(&PadButton::DPadUp));

        // Edges clear on the next scan; the hold persists. A release
        // with no matching hold is a silent no-op.
        world.run_system_once(check_input_requests).unwrap();
        let injected = world.resource::<crate::input::InjectedInputs>();
        assert!(injected.just_pressed.is_empty());
        assert!(injected.held.contains(&PadButton::DPadUp));

        std::fs::write(dir.join("release-dpad_up.request"), "").unwrap();
        world.run_system_once(check_input_requests).unwrap();
        let injected = world.resource::<crate::input::InjectedInputs>();
        assert!(!injected.held.contains(&PadButton::DPadUp));
        assert!(injected.just_released.contains(&PadButton::DPadUp));

        std::fs::remove_dir_all(&dir).unwrap();
    }
}

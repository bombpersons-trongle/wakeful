//! Locating the assets folder.

use std::path::PathBuf;

/// The assets folder name, relative to the repo (or however the
/// executable was launched) so file access works no matter how the game
/// is started. Shared by script loading, config loading, and the editor,
/// which runs as its own binary against the same folder.
pub const ASSETS_DIR: &str = "assets";

pub fn assets_root() -> PathBuf {
    if let Some(root) = std::env::var_os("BEVY_ASSET_ROOT") {
        return PathBuf::from(root);
    }
    if let Some(root) = std::env::var_os("CARGO_MANIFEST_DIR") {
        return PathBuf::from(root).join(ASSETS_DIR);
    }
    // Launched as `target/debug/wakeful`: walk up from the executable
    // until a folder that actually contains `assets` shows up (the
    // repo root, when running from a checkout).
    let mut dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.to_path_buf()));
    for _ in 0..4 {
        match dir {
            Some(here) if here.join(ASSETS_DIR).is_dir() => return here.join(ASSETS_DIR),
            Some(here) => dir = here.parent().map(|p| p.to_path_buf()),
            None => break,
        }
    }
    PathBuf::from(ASSETS_DIR)
}

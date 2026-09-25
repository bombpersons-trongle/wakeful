//! Locating the assets folder.

use std::path::PathBuf;

/// The assets folder name, relative to the repo (or however the
/// executable was launched) so file access works no matter how the game
/// is started. Shared by script loading, config loading, and the editor.
pub(crate) const ASSETS_DIR: &str = "assets";

pub(crate) fn assets_root() -> PathBuf {
    if let Some(root) = std::env::var_os("BEVY_ASSET_ROOT") {
        return PathBuf::from(root);
    }
    if let Some(root) = std::env::var_os("CARGO_MANIFEST_DIR") {
        return PathBuf::from(root).join(ASSETS_DIR);
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(ASSETS_DIR)))
        .unwrap_or_else(|| PathBuf::from(ASSETS_DIR))
}

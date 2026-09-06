//! CRT-style display simulation, plus the user-tunable display
//! settings both fullscreen effects read from `assets/ui.ron`.
//!
//! [`CrtMaterial`] runs on the present camera — after the finished
//! game image is blitted to the window — so scanlines, the RGB mask,
//! and the vignette cover the upscaled picture like a tube showing a
//! 240p signal. The effects are periodic in virtual-pixel space (see
//! the shader docs), so window resizes can't distort the pattern.
//!
//! [`DisplaySettings`] is the parsed `ui.ron` section;
//! [`sync_display_effects`] carries it into the live materials. Barrel
//! curvature is deliberately absent: it would need window-size math
//! and would desync the editor's cursor picking.
//!
//! `snapshot_mac_os` includes this module only because `screen.rs`
//! references it; the present camera doesn't exist there.

use std::path::Path;

use bevy::core_pipeline::fullscreen_material::FullscreenMaterial;
use bevy::core_pipeline::tonemapping::tonemapping;
use bevy::core_pipeline::{Core2d, Core2dSystems};
use bevy::ecs::schedule::{IntoScheduleConfigs, ScheduleConfigs, ScheduleLabel};
use bevy::ecs::system::BoxedSystem;
use bevy::prelude::*;
use bevy::render::extract_component::ExtractComponent;
use bevy::render::render_resource::ShaderType;
use bevy::shader::ShaderRef;
use serde::Deserialize;

use crate::dither::{self, DitherPostProcess};

/// Parses RON with `IMPLICIT_SOME` so `Option` sections can be written
/// plainly — `crt: (scanline: 0.5)` instead of `Some((...))`.
fn parse_config(text: &str) -> Result<DisplayConfig, ron::error::SpannedError> {
    let options =
        ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
    options.from_str(text)
}

/// Tuned default CRT intensity: visible structure without crushing
/// brightness. Also the per-field fallbacks for partial ui.ron sections.
const TUNED_SCANLINE: f32 = 0.35;
const TUNED_MASK: f32 = 0.25;
const TUNED_VIGNETTE: f32 = 0.30;

/// CRT display simulation for the window frame: scanline depth, RGB
/// mask depth, and vignette strength. All-zero strengths render the
/// frame untouched — that is how "CRT off" is expressed.
#[derive(Component, ExtractComponent, Clone, Copy, Debug, ShaderType, Default, PartialEq)]
pub(crate) struct CrtMaterial {
    pub scanline: f32,
    pub mask: f32,
    pub vignette: f32,
}

impl FullscreenMaterial for CrtMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/crt.wgsl".into()
    }

    // The present camera is a Camera2d, so the pass must run in the 2d
    // graph; the default Core3d schedule never touches 2d views (see
    // dither.rs for the full story).
    fn schedule() -> impl ScheduleLabel + Clone {
        Core2d
    }

    fn schedule_configs(system: ScheduleConfigs<BoxedSystem>) -> ScheduleConfigs<BoxedSystem> {
        system
            .in_set(Core2dSystems::PostProcess)
            .before(tonemapping)
    }
}

/// Gentle default CRT settings.
pub(crate) fn tuned_crt() -> CrtMaterial {
    CrtMaterial {
        scanline: TUNED_SCANLINE,
        mask: TUNED_MASK,
        vignette: TUNED_VIGNETTE,
    }
}

/// Live display-effect settings parsed from `assets/ui.ron`;
/// [`sync_display_effects`] carries them into the fullscreen materials.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub(crate) struct DisplaySettings {
    pub dither_enabled: bool,
    pub crt_enabled: bool,
    pub scanline: f32,
    pub mask: f32,
    pub vignette: f32,
}

impl Default for DisplaySettings {
    fn default() -> Self {
        Self {
            dither_enabled: true,
            crt_enabled: true,
            scanline: TUNED_SCANLINE,
            mask: TUNED_MASK,
            vignette: TUNED_VIGNETTE,
        }
    }
}

fn default_enabled() -> bool {
    true
}

/// The `dither` section of ui.ron.
#[derive(Deserialize, Default, Debug)]
struct DitherSection {
    #[serde(default = "default_enabled")]
    enabled: bool,
}

/// The `crt` section of ui.ron.
#[derive(Deserialize, Debug)]
struct CrtSection {
    #[serde(default = "default_enabled")]
    enabled: bool,
    #[serde(default = "default_scanline")]
    scanline: f32,
    #[serde(default = "default_mask")]
    mask: f32,
    #[serde(default = "default_vignette")]
    vignette: f32,
}

fn default_scanline() -> f32 {
    TUNED_SCANLINE
}

fn default_mask() -> f32 {
    TUNED_MASK
}

fn default_vignette() -> f32 {
    TUNED_VIGNETTE
}

/// The display parts of `assets/ui.ron`. Everything defaults, so the
/// file may carry only the bubble section — and unknown fields are
/// ignored, which is what lets bubble.rs read the same file.
#[derive(Deserialize, Default, Debug)]
struct DisplayConfig {
    #[serde(default)]
    dither: Option<DitherSection>,
    #[serde(default)]
    crt: Option<CrtSection>,
}

impl From<CrtSection> for CrtMaterial {
    fn from(section: CrtSection) -> Self {
        if section.enabled {
            CrtMaterial {
                scanline: section.scanline,
                mask: section.mask,
                vignette: section.vignette,
            }
        } else {
            Self::default()
        }
    }
}

impl DisplaySettings {
    /// Loads `path`, falling back to the defaults when the file is
    /// missing and warning when it exists but is broken.
    pub(crate) fn from_file(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        match parse_config(&text) {
            Ok(config) => Self::from_config(config),
            Err(e) => {
                warn!(
                    "{} is not a valid UI config, display effects use the defaults: {e}",
                    path.display()
                );
                Self::default()
            }
        }
    }

    fn from_config(config: DisplayConfig) -> Self {
        let mut settings = Self::default();
        if let Some(dither) = config.dither {
            settings.dither_enabled = dither.enabled;
        }
        if let Some(crt) = config.crt {
            settings.crt_enabled = crt.enabled;
            settings.scanline = crt.scanline;
            settings.mask = crt.mask;
            settings.vignette = crt.vignette;
        }
        settings
    }
}

/// Applies [`DisplaySettings`] to the live fullscreen passes: dither
/// strength 0 when disabled, CRT strengths zeroed when disabled.
/// Mutates only on mismatch, so an unchanged frame costs two
/// comparisons. Comparing instead of relying on change detection keeps
/// this working in bare-world tests (see `BubbleAssets.applied`).
pub(crate) fn sync_display_effects(
    settings: Res<DisplaySettings>,
    mut dither: Query<&mut DitherPostProcess>,
    mut crt: Query<&mut CrtMaterial>,
) {
    let mut wanted = dither::tuned();
    if !settings.dither_enabled {
        wanted.dither_strength = 0.0;
    }
    for mut pass in &mut dither {
        if *pass != wanted {
            *pass = wanted;
        }
    }

    let wanted_crt = if settings.crt_enabled {
        CrtMaterial {
            scanline: settings.scanline,
            mask: settings.mask,
            vignette: settings.vignette,
        }
    } else {
        CrtMaterial::default()
    };
    for mut pass in &mut crt {
        if *pass != wanted_crt {
            *pass = wanted_crt;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use std::path::PathBuf;

    fn temp_config(name: &str, body: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("wakeful-display-{name}.ron"));
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn defaults_are_on_with_tuned_strengths() {
        let s = DisplaySettings::default();
        assert!(s.dither_enabled);
        assert!(s.crt_enabled);
        assert_eq!(
            tuned_crt(),
            CrtMaterial {
                scanline: s.scanline,
                mask: s.mask,
                vignette: s.vignette,
            }
        );
    }

    #[test]
    fn missing_file_falls_back_to_defaults() {
        let path = Path::new("/nonexistent/wakeful/ui.ron");
        assert_eq!(DisplaySettings::from_file(path), DisplaySettings::default());
    }

    #[test]
    fn broken_file_falls_back_to_defaults() {
        let path = temp_config("broken", "not ron at all");
        assert_eq!(
            DisplaySettings::from_file(&path),
            DisplaySettings::default()
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn bubble_only_config_leaves_display_defaults() {
        // Old configs (or bubble-only experiments) must keep parsing.
        let path = temp_config("bubble-only", "(bubble: ())");
        assert_eq!(
            DisplaySettings::from_file(&path),
            DisplaySettings::default()
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn sections_are_read_independently() {
        let path = temp_config(
            "dither-off",
            "(dither: (enabled: false), crt: (enabled: true, scanline: 0.5, mask: 0.1, vignette: 0.2))",
        );
        let s = DisplaySettings::from_file(&path);
        assert!(!s.dither_enabled);
        assert!(s.crt_enabled);
        assert_eq!(s.scanline, 0.5);
        assert_eq!(s.mask, 0.1);
        assert_eq!(s.vignette, 0.2);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn partial_section_fills_from_tuned_defaults() {
        let path = temp_config("partial-crt", "(crt: (scanline: 0.6))");
        let s = DisplaySettings::from_file(&path);
        assert!(s.crt_enabled);
        assert_eq!(s.scanline, 0.6);
        assert_eq!(s.mask, TUNED_MASK);
        assert_eq!(s.vignette, TUNED_VIGNETTE);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn sync_zeroes_the_dither_when_disabled() {
        let mut world = World::new();
        world.insert_resource(DisplaySettings {
            dither_enabled: false,
            ..Default::default()
        });
        let camera = world.spawn(DitherPostProcess::default()).id();
        world.run_system_once(sync_display_effects).unwrap();

        let pass = world.get::<DitherPostProcess>(camera).unwrap();
        assert_eq!(pass.dither_strength, 0.0);
        // Quantization depth stays at its tuned value; the shader's
        // early-out is what makes off mean fully off.
        assert_eq!(pass.color_steps, dither::tuned().color_steps);
    }

    #[test]
    fn sync_writes_crt_strengths_from_settings() {
        let mut world = World::new();
        world.insert_resource(DisplaySettings {
            scanline: 0.5,
            mask: 0.1,
            vignette: 0.2,
            ..Default::default()
        });
        let camera = world.spawn(CrtMaterial::default()).id();
        world.run_system_once(sync_display_effects).unwrap();

        let pass = world.get::<CrtMaterial>(camera).unwrap();
        assert_eq!(
            *pass,
            CrtMaterial {
                scanline: 0.5,
                mask: 0.1,
                vignette: 0.2,
            }
        );
    }

    #[test]
    fn sync_renders_crt_untouched_when_disabled() {
        let mut world = World::new();
        world.insert_resource(DisplaySettings {
            crt_enabled: false,
            ..Default::default()
        });
        let camera = world.spawn(tuned_crt()).id();
        world.run_system_once(sync_display_effects).unwrap();

        assert_eq!(
            *world.get::<CrtMaterial>(camera).unwrap(),
            CrtMaterial::default()
        );
    }
}

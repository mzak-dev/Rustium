use std::fs;
use std::path::PathBuf;

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::mcp::McpSettings;

/// Interface language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) enum Language {
    #[default]
    English,
    Polish,
}

/// Accent color for the active tool, selection and primary buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) enum Accent {
    #[default]
    RustOrange,
    Cyan,
    Blue,
    Lime,
}

impl Accent {
    pub(crate) const ALL: [Self; 4] = [Self::RustOrange, Self::Cyan, Self::Blue, Self::Lime];

    pub(crate) fn rgb(self) -> [u8; 3] {
        match self {
            Self::RustOrange => [0xee, 0x6c, 0x3e],
            Self::Cyan => [0x35, 0xd0, 0xc8],
            Self::Blue => [0x7c, 0x9c, 0xf5],
            Self::Lime => [0xb6, 0xe3, 0x6a],
        }
    }

    pub(crate) fn message_key(self) -> &'static str {
        match self {
            Self::RustOrange => "accent-rust-orange",
            Self::Cyan => "accent-cyan",
            Self::Blue => "accent-blue",
            Self::Lime => "accent-lime",
        }
    }
}

/// How much the interface animates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) enum Motion {
    #[default]
    Full,
    Reduced,
    Off,
}

/// How orbiting treats the up axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) enum OrbitStyle {
    /// Keeps the model upright.
    #[default]
    Turntable,
    /// Allows looking over the poles.
    Free,
}

/// Graphics API, applied after a restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) enum Backend {
    #[default]
    Automatic,
    Vulkan,
    DirectX12,
}

/// Frame pacing when vertical sync is off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) enum FrameLimit {
    #[default]
    MatchDisplay,
    Fps60,
    Fps144,
    Unlimited,
}

/// Everything the Settings window edits; saved as TOML in the config directory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is one on/off switch in the Settings window"
)]
pub(crate) struct Settings {
    pub(crate) language: Language,
    pub(crate) show_start_screen: bool,
    pub(crate) default_voxel_size_cm: f32,
    pub(crate) autosave: bool,
    pub(crate) autosave_minutes: u32,
    pub(crate) undo_steps: u32,
    pub(crate) accent: Accent,
    pub(crate) ui_scale_percent: u32,
    pub(crate) translucent_panels: bool,
    pub(crate) motion: Motion,
    pub(crate) shortcut_letters: bool,
    pub(crate) tooltip_delay_seconds: f32,
    pub(crate) orbit_style: OrbitStyle,
    pub(crate) background: [u8; 3],
    pub(crate) show_grid: bool,
    pub(crate) show_volume_bounds: bool,
    pub(crate) field_of_view_degrees: f32,
    pub(crate) msaa_samples: u32,
    pub(crate) invert_zoom: bool,
    pub(crate) backend: Backend,
    pub(crate) vsync: bool,
    pub(crate) frame_limit: FrameLimit,
    pub(crate) mcp: McpSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: Language::English,
            show_start_screen: true,
            default_voxel_size_cm: 25.0,
            autosave: true,
            autosave_minutes: 5,
            undo_steps: 200,
            accent: Accent::RustOrange,
            ui_scale_percent: 100,
            translucent_panels: true,
            motion: Motion::Full,
            shortcut_letters: true,
            tooltip_delay_seconds: 0.3,
            orbit_style: OrbitStyle::Turntable,
            background: [0x1a, 0x1d, 0x22],
            show_grid: true,
            show_volume_bounds: true,
            field_of_view_degrees: 45.0,
            msaa_samples: 4,
            invert_zoom: false,
            backend: Backend::Automatic,
            vsync: true,
            frame_limit: FrameLimit::MatchDisplay,
            mcp: McpSettings::default(),
        }
    }
}

impl Settings {
    /// Reads the saved settings, falling back to defaults when the file is missing or unreadable.
    pub(crate) fn load() -> Self {
        let path = settings_path();
        let Ok(text) = fs::read_to_string(&path) else {
            return Self::default();
        };
        toml::from_str(&text).unwrap_or_else(|error| {
            warn!(path = %path.display(), %error, "settings file is invalid, using defaults");
            Self::default()
        })
    }

    /// Writes the settings; failures are logged because losing preferences must not interrupt editing.
    pub(crate) fn save(&self) {
        let path = settings_path();
        let result = toml::to_string_pretty(self)
            .map_err(|error| error.to_string())
            .and_then(|text| {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                }
                fs::write(&path, text).map_err(|error| error.to_string())
            });
        if let Err(error) = result {
            warn!(path = %path.display(), %error, "cannot save settings");
        }
    }

    /// Resets the fields shown in one Settings section.
    pub(crate) fn reset_section(&mut self, section: crate::settings_window::Section) {
        use crate::settings_window::Section;
        let defaults = Self::default();
        match section {
            Section::General => {
                self.language = defaults.language;
                self.show_start_screen = defaults.show_start_screen;
                self.default_voxel_size_cm = defaults.default_voxel_size_cm;
                self.autosave = defaults.autosave;
                self.autosave_minutes = defaults.autosave_minutes;
                self.undo_steps = defaults.undo_steps;
            }
            Section::Interface => {
                self.accent = defaults.accent;
                self.ui_scale_percent = defaults.ui_scale_percent;
                self.translucent_panels = defaults.translucent_panels;
                self.motion = defaults.motion;
                self.shortcut_letters = defaults.shortcut_letters;
                self.tooltip_delay_seconds = defaults.tooltip_delay_seconds;
            }
            Section::Viewport => {
                self.orbit_style = defaults.orbit_style;
                self.background = defaults.background;
                self.show_grid = defaults.show_grid;
                self.show_volume_bounds = defaults.show_volume_bounds;
                self.field_of_view_degrees = defaults.field_of_view_degrees;
                self.msaa_samples = defaults.msaa_samples;
            }
            Section::Input => self.invert_zoom = defaults.invert_zoom,
            Section::Performance => {
                self.backend = defaults.backend;
                self.vsync = defaults.vsync;
                self.frame_limit = defaults.frame_limit;
            }
            Section::Mcp => self.mcp = defaults.mcp,
        }
    }
}

fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("dev", "mzak", "Rustium")
}

fn settings_path() -> PathBuf {
    project_dirs().map_or_else(
        || PathBuf::from("settings.toml"),
        |dirs| dirs.config_dir().join("settings.toml"),
    )
}

/// Local data directory for logs and recovery files.
pub(crate) fn data_dir() -> PathBuf {
    project_dirs().map_or_else(|| PathBuf::from("."), |dirs| dirs.data_local_dir().to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_through_toml() {
        let settings = Settings {
            language: Language::Polish,
            msaa_samples: 8,
            ..Settings::default()
        };
        let text = toml::to_string_pretty(&settings).unwrap();
        assert_eq!(toml::from_str::<Settings>(&text).unwrap(), settings);
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let settings: Settings = toml::from_str("vsync = false").unwrap();
        assert!(!settings.vsync);
        assert_eq!(settings.undo_steps, 200);
    }
}

#[cfg(not(target_arch = "wasm32"))]
use std::fs;
use std::path::PathBuf;

#[cfg(not(target_arch = "wasm32"))]
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::keymap::Keymap;
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

/// How voxels appear and vanish when they are edited (Motion Full only; Reduced always flashes, Off shows nothing).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) enum EditAnimation {
    /// See-through cells while the stroke runs, hardened one after another on release.
    #[default]
    Ghost,
    /// Cells grow from 60 % with a slight overshoot and shrink away.
    Pop,
    /// Cells grow out of the face they were placed on and sink back into it.
    FromWall,
    /// Like Pop, spreading out from where the gesture started.
    Wave,
    /// The change shows at once, with a fading highlight.
    Flash,
}

impl EditAnimation {
    pub(crate) const ALL: [Self; 5] = [Self::Ghost, Self::Pop, Self::FromWall, Self::Wave, Self::Flash];

    pub(crate) fn message_key(self) -> &'static str {
        match self {
            Self::Ghost => "edit-animation-ghost",
            Self::Pop => "edit-animation-pop",
            Self::FromWall => "edit-animation-from-wall",
            Self::Wave => "edit-animation-wave",
            Self::Flash => "edit-animation-flash",
        }
    }
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

impl From<Backend> for rustium_render::GraphicsBackend {
    fn from(backend: Backend) -> Self {
        match backend {
            Backend::Automatic => Self::Automatic,
            Backend::Vulkan => Self::Vulkan,
            Backend::DirectX12 => Self::DirectX12,
        }
    }
}

impl From<rustium_render::GraphicsBackend> for Backend {
    fn from(backend: rustium_render::GraphicsBackend) -> Self {
        match backend {
            rustium_render::GraphicsBackend::Automatic => Self::Automatic,
            rustium_render::GraphicsBackend::Vulkan => Self::Vulkan,
            rustium_render::GraphicsBackend::DirectX12 => Self::DirectX12,
        }
    }
}

/// Which traversal the path tracer uses; applied immediately.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) enum RayTracingMode {
    /// Hardware ray query when the GPU supports it.
    #[default]
    Automatic,
    /// Software DDA, runs on every GPU.
    Software,
    /// Hardware ray query; software where unsupported.
    Hardware,
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
    pub(crate) edit_animation: EditAnimation,
    pub(crate) shortcut_letters: bool,
    /// Draw tool-aware cursors over the 3D view, node canvas and render frame instead of system ones.
    pub(crate) custom_cursors: bool,
    pub(crate) tooltip_delay_seconds: f32,
    pub(crate) orbit_style: OrbitStyle,
    pub(crate) background: [u8; 3],
    pub(crate) show_grid: bool,
    pub(crate) show_volume_bounds: bool,
    pub(crate) field_of_view_degrees: f32,
    pub(crate) msaa_samples: u32,
    pub(crate) invert_zoom: bool,
    pub(crate) backend: Backend,
    /// Name of the chosen graphics device; `None` is Automatic.
    pub(crate) gpu_adapter: Option<String>,
    pub(crate) ray_tracing: RayTracingMode,
    pub(crate) vsync: bool,
    pub(crate) frame_limit: FrameLimit,
    pub(crate) keymap: Keymap,
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
            edit_animation: EditAnimation::Ghost,
            shortcut_letters: true,
            custom_cursors: true,
            tooltip_delay_seconds: 0.3,
            orbit_style: OrbitStyle::Turntable,
            background: [0x1a, 0x1d, 0x22],
            show_grid: true,
            show_volume_bounds: true,
            field_of_view_degrees: 45.0,
            msaa_samples: 4,
            invert_zoom: false,
            backend: Backend::Automatic,
            gpu_adapter: None,
            ray_tracing: RayTracingMode::Automatic,
            vsync: true,
            frame_limit: FrameLimit::MatchDisplay,
            keymap: Keymap::default(),
            mcp: McpSettings::default(),
        }
    }
}

impl Settings {
    /// Reads the saved settings, falling back to defaults when the file is missing or unreadable.
    pub(crate) fn load() -> Self {
        let Some(text) = read_settings_text() else {
            return Self::default();
        };
        toml::from_str(&text).unwrap_or_else(|error| {
            warn!(%error, "settings are invalid, using defaults");
            Self::default()
        })
    }

    /// Writes the settings; failures are logged because losing preferences must not interrupt editing.
    pub(crate) fn save(&self) {
        let result = toml::to_string_pretty(self)
            .map_err(|error| error.to_string())
            .and_then(|text| write_settings_text(&text));
        if let Err(error) = result {
            warn!(%error, "cannot save settings");
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
                self.edit_animation = defaults.edit_animation;
                self.shortcut_letters = defaults.shortcut_letters;
                self.custom_cursors = defaults.custom_cursors;
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
            Section::Input => {
                self.invert_zoom = defaults.invert_zoom;
                self.keymap = defaults.keymap;
            }
            Section::Performance => {
                self.backend = defaults.backend;
                self.gpu_adapter = defaults.gpu_adapter;
                self.ray_tracing = defaults.ray_tracing;
                self.vsync = defaults.vsync;
                self.frame_limit = defaults.frame_limit;
            }
            Section::Mcp => self.mcp = defaults.mcp,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("dev", "mzak", "Rustium")
}

#[cfg(not(target_arch = "wasm32"))]
fn settings_path() -> PathBuf {
    project_dirs().map_or_else(
        || PathBuf::from("settings.toml"),
        |dirs| dirs.config_dir().join("settings.toml"),
    )
}

/// The text of the settings file, if there is one.
#[cfg(not(target_arch = "wasm32"))]
fn read_settings_text() -> Option<String> {
    fs::read_to_string(settings_path()).ok()
}

#[cfg(not(target_arch = "wasm32"))]
fn write_settings_text(text: &str) -> Result<(), String> {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    fs::write(&path, text).map_err(|error| format!("{}: {error}", path.display()))
}

/// Where the browser keeps the settings: the same TOML as the desktop's settings file, in local storage.
#[cfg(target_arch = "wasm32")]
const SETTINGS_KEY: &str = "rustium.settings";

#[cfg(target_arch = "wasm32")]
fn read_settings_text() -> Option<String> {
    crate::web::storage_get(SETTINGS_KEY)
}

#[cfg(target_arch = "wasm32")]
fn write_settings_text(text: &str) -> Result<(), String> {
    crate::web::storage_set(SETTINGS_KEY, text).map_err(|report| format!("{report:#}"))
}

/// Local data directory for logs and recovery files.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn data_dir() -> PathBuf {
    project_dirs().map_or_else(|| PathBuf::from("."), |dirs| dirs.data_local_dir().to_path_buf())
}

/// The browser has no file system: nothing is ever found or written under this directory.
#[cfg(target_arch = "wasm32")]
pub(crate) fn data_dir() -> PathBuf {
    PathBuf::from("/rustium")
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
    fn custom_cursors_default_on_and_persist() {
        assert!(Settings::default().custom_cursors);
        let settings: Settings = toml::from_str("vsync = false").unwrap();
        assert!(settings.custom_cursors);
        let off = Settings {
            custom_cursors: false,
            ..Settings::default()
        };
        let text = toml::to_string_pretty(&off).unwrap();
        assert!(!toml::from_str::<Settings>(&text).unwrap().custom_cursors);
    }

    #[test]
    fn resetting_interface_restores_custom_cursors() {
        let mut settings = Settings {
            custom_cursors: false,
            ..Settings::default()
        };
        settings.reset_section(crate::settings_window::Section::Interface);
        assert!(settings.custom_cursors);
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let settings: Settings = toml::from_str("vsync = false").unwrap();
        assert!(!settings.vsync);
        assert_eq!(settings.undo_steps, 200);
    }
}

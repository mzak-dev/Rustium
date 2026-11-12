use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

/// A group of MCP tools the user can switch on or off in the MCP tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    /// Scene info, Volume, voxel queries, the Palette and Materials.
    ReadModel,
    /// Setting and erasing voxels, applying a Brush.
    EditVoxels,
    /// Changing Palette colors and Materials.
    EditPalette,
    /// Running Luau scripts.
    RunScripts,
    /// Starting renders.
    StartRenders,
}

impl Permission {
    /// Every permission, in the order of the MCP tab.
    pub const ALL: [Self; 5] = [
        Self::ReadModel,
        Self::EditVoxels,
        Self::EditPalette,
        Self::RunScripts,
        Self::StartRenders,
    ];

    /// The name shown to MCP clients when the permission is off.
    pub fn label(self) -> &'static str {
        match self {
            Self::ReadModel => "Read model",
            Self::EditVoxels => "Edit voxels",
            Self::EditPalette => "Edit palette & materials",
            Self::RunScripts => "Run scripts",
            Self::StartRenders => "Start renders",
        }
    }

    fn bit(self) -> u8 {
        1 << (self as u8)
    }
}

/// Which tool groups are switched on; saved with the settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is one permission switch in the MCP tab"
)]
pub struct Permissions {
    /// Scene info, Volume, voxel queries, the Palette and Materials.
    pub read_model: bool,
    /// Setting and erasing voxels, applying a Brush.
    pub edit_voxels: bool,
    /// Changing Palette colors and Materials.
    pub edit_palette: bool,
    /// Running Luau scripts.
    pub run_scripts: bool,
    /// Starting renders.
    pub start_renders: bool,
}

impl Default for Permissions {
    /// Reading and editing are on; scripts and renders are off until the user allows them.
    fn default() -> Self {
        Self {
            read_model: true,
            edit_voxels: true,
            edit_palette: true,
            run_scripts: false,
            start_renders: false,
        }
    }
}

impl Permissions {
    /// Whether `permission` is on.
    pub fn allows(self, permission: Permission) -> bool {
        match permission {
            Permission::ReadModel => self.read_model,
            Permission::EditVoxels => self.edit_voxels,
            Permission::EditPalette => self.edit_palette,
            Permission::RunScripts => self.run_scripts,
            Permission::StartRenders => self.start_renders,
        }
    }

    /// The switch for `permission`, for the interface to toggle.
    pub fn flag_mut(&mut self, permission: Permission) -> &mut bool {
        match permission {
            Permission::ReadModel => &mut self.read_model,
            Permission::EditVoxels => &mut self.edit_voxels,
            Permission::EditPalette => &mut self.edit_palette,
            Permission::RunScripts => &mut self.run_scripts,
            Permission::StartRenders => &mut self.start_renders,
        }
    }
}

/// The permissions as the server thread sees them: the interface updates them, every tool call reads them.
#[derive(Debug, Clone, Default)]
pub struct SharedPermissions(Arc<AtomicU8>);

impl SharedPermissions {
    /// Shared permissions starting from `permissions`.
    pub fn new(permissions: Permissions) -> Self {
        let shared = Self::default();
        shared.set(permissions);
        shared
    }

    /// Replaces all permissions; calls already running are not interrupted.
    pub fn set(&self, permissions: Permissions) {
        let bits = Permission::ALL
            .into_iter()
            .filter(|permission| permissions.allows(*permission))
            .fold(0, |bits, permission| bits | permission.bit());
        self.0.store(bits, Ordering::Relaxed);
    }

    /// Whether `permission` is on right now.
    pub fn allows(&self, permission: Permission) -> bool {
        self.0.load(Ordering::Relaxed) & permission.bit() != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_permissions_follow_the_last_set() {
        let shared = SharedPermissions::new(Permissions::default());
        assert!(shared.allows(Permission::EditVoxels));
        assert!(!shared.allows(Permission::RunScripts));
        let mut next = Permissions::default();
        *next.flag_mut(Permission::EditVoxels) = false;
        *next.flag_mut(Permission::RunScripts) = true;
        shared.set(next);
        assert!(!shared.allows(Permission::EditVoxels));
        assert!(shared.allows(Permission::RunScripts));
        assert!(shared.allows(Permission::ReadModel));
    }

    #[test]
    fn missing_settings_fields_use_defaults() {
        let parsed: Permissions = serde_json::from_str(r#"{"run_scripts": true}"#).unwrap();
        assert!(parsed.run_scripts && parsed.read_model && !parsed.start_renders);
    }
}

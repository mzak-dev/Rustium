//! Operating-system integration for Rustium.
//!
//! On Windows the frameless main window answers `WM_NCHITTEST` so the maximize button opens Snap Layouts on
//! Windows 11 and the top strip behaves like a native title bar. On every other platform [`Platform`] does
//! nothing. This is the only crate in the workspace that contains `unsafe` code.

mod hit;
#[cfg(windows)]
mod win32;

pub use hit::{Edge, HitRegions, HitZone, Rect};
use raw_window_handle::HasWindowHandle;

/// Handle to the platform hooks installed on one window; dropping it removes them.
#[derive(Debug)]
pub struct Platform {
    #[cfg(windows)]
    inner: win32::Subclass,
}

impl Platform {
    /// Installs the hooks on `window`. Returns `None` where there is nothing to install, or when the
    /// window handle is not usable. Must be called on the thread that owns the window.
    pub fn attach(window: &impl HasWindowHandle) -> Option<Self> {
        #[cfg(windows)]
        {
            win32::Subclass::install(window).map(|inner| Self { inner })
        }
        #[cfg(not(windows))]
        {
            let _ = window;
            None
        }
    }

    /// Tells the platform layer where the window's regions are. Call it every frame; unchanged values are cheap.
    #[cfg_attr(not(windows), expect(clippy::unused_self, reason = "nothing to update off Windows"))]
    pub fn update(&self, regions: &HitRegions) {
        #[cfg(windows)]
        self.inner.update(regions);
        #[cfg(not(windows))]
        let _ = regions;
    }
}

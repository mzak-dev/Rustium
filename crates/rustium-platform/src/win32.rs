//! Window subclass that answers `WM_NCHITTEST` for the frameless main window.
//!
//! Untested at runtime so far: written and type-checked on Linux only.

use std::marker::PhantomData;
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{InvalidateRect, ScreenToClient};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT, HTCAPTION, HTLEFT, HTMAXBUTTON, HTRIGHT, HTTOP, HTTOPLEFT, HTTOPRIGHT,
    IsZoomed, PostMessageW, SC_MAXIMIZE, SC_RESTORE, WM_NCDESTROY, WM_NCHITTEST, WM_NCLBUTTONDBLCLK, WM_NCLBUTTONDOWN,
    WM_NCLBUTTONUP, WM_NCMOUSELEAVE, WM_NCMOUSEMOVE, WM_SYSCOMMAND,
};

use crate::hit::{Edge, HitRegions, HitZone};

/// Identifies our subclass on the window.
const SUBCLASS_ID: usize = 0x5255_5354;

/// State shared between the app (writer) and the window procedure (reader); both run on the window's thread.
#[derive(Debug, Default)]
struct Shared {
    regions: Mutex<HitRegions>,
    /// The pointer is over the maximize button (a non-client area, so the client never sees it).
    maximize_hovered: AtomicBool,
    /// The left button went down on the maximize button and has not been released yet.
    maximize_pressed: AtomicBool,
}

/// The installed subclass. Dropping it removes the hook.
#[derive(Debug)]
pub(crate) struct Subclass {
    hwnd: HWND,
    shared: Arc<Shared>,
    /// The window procedure must be removed on the window's own thread.
    _not_send: PhantomData<*const ()>,
}

impl Subclass {
    pub(crate) fn install(window: &impl HasWindowHandle) -> Option<Self> {
        let handle = window.window_handle().ok()?;
        let RawWindowHandle::Win32(win32) = handle.as_raw() else {
            return None;
        };
        let hwnd = win32.hwnd.get() as HWND;
        let shared = Arc::new(Shared::default());
        // The window procedure owns one strong reference, released when the subclass is removed.
        let data = Arc::into_raw(Arc::clone(&shared)) as usize;
        // SAFETY: `hwnd` is the live window handle of the caller's window and this runs on its thread;
        // `subclass_proc` matches the SUBCLASSPROC signature and `data` stays valid until the subclass is removed.
        let installed = unsafe { SetWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID, data) };
        if installed == 0 {
            tracing::warn!("cannot subclass the window; Snap Layouts are unavailable");
            // SAFETY: `data` came from `Arc::into_raw` above and was not handed to a window.
            drop(unsafe { Arc::from_raw(data as *const Shared) });
            return None;
        }
        Some(Self {
            hwnd,
            shared,
            _not_send: PhantomData,
        })
    }

    pub(crate) fn maximize_hovered(&self) -> bool {
        self.shared.maximize_hovered.load(Ordering::Relaxed)
    }

    pub(crate) fn update(&self, regions: &HitRegions) {
        let Ok(mut current) = self.shared.regions.lock() else {
            return;
        };
        if *current != *regions {
            current.clone_from(regions);
        }
    }
}

impl Drop for Subclass {
    fn drop(&mut self) {
        // SAFETY: same window and thread as `install`; the call fails harmlessly if the window is already gone.
        let removed = unsafe { RemoveWindowSubclass(self.hwnd, Some(subclass_proc), SUBCLASS_ID) };
        if removed != 0 {
            // The window procedure's reference is no longer reachable; release it here.
            let data = Arc::as_ptr(&self.shared);
            // SAFETY: the strong count owned by the removed subclass was created with `Arc::into_raw` on this
            // same allocation, and nothing else releases it once removal succeeded.
            unsafe { Arc::decrement_strong_count(data) };
        }
    }
}

/// Window procedure hook. Never unwinds into Windows.
///
/// # Safety
///
/// Called by Windows with a valid `hwnd` and the `data` registered in [`Subclass::install`].
unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    match message {
        WM_NCHITTEST => {
            let answer = panic::catch_unwind(AssertUnwindSafe(|| hit_test(hwnd, lparam, data)));
            match answer {
                Ok(Some(result)) => return result,
                Ok(None) => {}
                Err(_) => tracing::error!("panic while answering WM_NCHITTEST"),
            }
        }
        WM_NCMOUSEMOVE | WM_NCMOUSELEAVE | WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK | WM_NCLBUTTONUP => {
            let handled = panic::catch_unwind(AssertUnwindSafe(|| {
                maximize_button_message(hwnd, message, wparam, data)
            }));
            match handled {
                Ok(true) => return 0,
                Ok(false) => {}
                Err(_) => tracing::error!("panic while handling a maximize button message"),
            }
        }
        WM_NCDESTROY => {
            // SAFETY: called from the window's own procedure with the registration made in `install`.
            let removed = unsafe { RemoveWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID) };
            if removed != 0 {
                // SAFETY: `data` is the pointer from `Arc::into_raw`; removal succeeded, so this is its last use.
                unsafe { Arc::decrement_strong_count(data as *const Shared) };
            }
        }
        _ => {}
    }
    // SAFETY: forwards the unchanged message of this very call to the next procedure in the chain.
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

/// Handles the non-client mouse messages over the maximize button, which Windows sends there instead of the client
/// area: the button highlights and still maximizes or restores when clicked. Returns `true` when the message is used
/// up.
fn maximize_button_message(hwnd: HWND, message: u32, wparam: WPARAM, data: usize) -> bool {
    // SAFETY: `data` is the pointer from `Arc::into_raw` in `install`, alive until the subclass is removed.
    let shared = unsafe { &*(data as *const Shared) };
    let over_button = u32::try_from(wparam).is_ok_and(|code| code == HTMAXBUTTON);
    match message {
        WM_NCMOUSEMOVE if over_button => {
            if !shared.maximize_hovered.swap(true, Ordering::Relaxed) {
                let mut track = TRACKMOUSEEVENT {
                    cbSize: u32::try_from(size_of::<TRACKMOUSEEVENT>()).unwrap_or(0),
                    dwFlags: TME_LEAVE | TME_NONCLIENT,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                // SAFETY: `track` is a fully initialised TRACKMOUSEEVENT for this window.
                unsafe { TrackMouseEvent(&raw mut track) };
                redraw(hwnd);
            }
            false
        }
        WM_NCMOUSEMOVE | WM_NCMOUSELEAVE => {
            // Moved to another part of the window, or left it.
            let was_hovered = shared.maximize_hovered.swap(false, Ordering::Relaxed);
            shared.maximize_pressed.store(false, Ordering::Relaxed);
            if was_hovered {
                redraw(hwnd);
            }
            false
        }
        // The second press of a quick double click arrives as a double click message; handling it like a press keeps
        // the default procedure from running its own maximize logic after ours.
        WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK if over_button => {
            shared.maximize_pressed.store(true, Ordering::Relaxed);
            true
        }
        WM_NCLBUTTONUP if over_button => {
            if !shared.maximize_pressed.swap(false, Ordering::Relaxed) {
                return false;
            }
            // SAFETY: `hwnd` is the window this procedure runs for.
            let command = if unsafe { IsZoomed(hwnd) } != 0 {
                SC_RESTORE
            } else {
                SC_MAXIMIZE
            };
            // SAFETY: posting a system command to our own window; the handler runs after this procedure returns.
            unsafe { PostMessageW(hwnd, WM_SYSCOMMAND, command as WPARAM, 0) };
            true
        }
        _ => false,
    }
}

/// Asks for a repaint so the interface can show the button's new state.
fn redraw(hwnd: HWND) {
    // SAFETY: `hwnd` is the window this procedure runs for; a null rectangle invalidates the whole client area.
    unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
}

/// The hit-test answer, or `None` to leave the decision to the default procedure.
fn hit_test(hwnd: HWND, lparam: LPARAM, data: usize) -> Option<LRESULT> {
    // SAFETY: `data` is the pointer from `Arc::into_raw` in `install`, alive until the subclass is removed,
    // and only shared access is needed.
    let shared = unsafe { &*(data as *const Shared) };
    let regions = shared.regions.try_lock().ok()?;
    // The low and high words are signed screen coordinates.
    let mut point = POINT {
        x: signed_word(lparam),
        y: signed_word(lparam >> 16),
    };
    // SAFETY: `hwnd` is the window this procedure runs for and `point` is a valid, writable POINT.
    if unsafe { ScreenToClient(hwnd, &raw mut point) } == 0 {
        return None;
    }
    // SAFETY: `hwnd` is the window this procedure runs for.
    let maximized = unsafe { IsZoomed(hwnd) } != 0;
    let zone = regions.classify(point.x as f32, point.y as f32, maximized);
    let code = match zone {
        HitZone::Client => return None,
        HitZone::Caption => HTCAPTION,
        HitZone::MaximizeButton => HTMAXBUTTON,
        HitZone::Resize(Edge::Left) => HTLEFT,
        HitZone::Resize(Edge::Right) => HTRIGHT,
        HitZone::Resize(Edge::Top) => HTTOP,
        HitZone::Resize(Edge::Bottom) => HTBOTTOM,
        HitZone::Resize(Edge::TopLeft) => HTTOPLEFT,
        HitZone::Resize(Edge::TopRight) => HTTOPRIGHT,
        HitZone::Resize(Edge::BottomLeft) => HTBOTTOMLEFT,
        HitZone::Resize(Edge::BottomRight) => HTBOTTOMRIGHT,
    };
    LRESULT::try_from(code).ok()
}

/// The low 16 bits of `value` as a signed number, as `GET_X_LPARAM` does.
#[expect(clippy::cast_possible_truncation, reason = "keeping only the low word is the point")]
fn signed_word(value: LPARAM) -> i32 {
    i32::from(value as i16)
}

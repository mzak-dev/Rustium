# Frameless window with our own title-bar controls

The main window has no system decorations; minimize, maximize and close live in the HUD as in the mockups. Dragging uses winit's `drag_window` (the system move loop keeps Aero Snap), resizing uses `drag_resize_window` from a thin border band, and on Windows `with_undecorated_shadow(true)` keeps the drop shadow. Because winit covers all of that, there is no `rustium-platform` crate yet; it appears only if we need Win32 `WM_NCHITTEST` for Snap Layouts on hovering the maximize button.

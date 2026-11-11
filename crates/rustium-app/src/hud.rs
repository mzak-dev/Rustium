//! The viewport-first HUD from the Final mockups: floating glass islands over a full-window viewport.

mod corners;
mod hotbar;
mod inspector;
mod tool_options;
mod toolbar;
mod top_left;
mod top_right;
mod view_cube;
pub(crate) mod widgets;

use egui::{
    Align2, Context, CornerRadius, Id, InnerResponse, Key, Modifiers, Order, PointerButton, Pos2, Rect, Ui, Vec2,
};
use glam::Vec2 as GlamVec2;
use rustium_core::{Face, HitTarget, pick};
use winit::window::ResizeDirection;

use crate::camera::OrbitCamera;
use crate::editor::{Editor, Tool};
use crate::i18n::I18n;
use crate::settings::{Motion, Settings};
use crate::settings_window::Section;

/// Tabs of the right-hand inspector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InspectorTab {
    Scene,
    History,
    Mcp,
}

/// Interface state that is not part of the document.
#[derive(Debug)]
pub(crate) struct UiState {
    pub(crate) started_at: Option<f64>,
    pub(crate) inspector_open: bool,
    pub(crate) inspector_tab: InspectorTab,
    pub(crate) palette_open: bool,
    pub(crate) splash_open: bool,
    pub(crate) splash_opened_at: Option<f64>,
    pub(crate) settings_section: Option<Section>,
    pub(crate) settings_search: String,
}

impl UiState {
    pub(crate) fn new(show_splash: bool) -> Self {
        Self {
            started_at: None,
            inspector_open: true,
            inspector_tab: InspectorTab::Scene,
            palette_open: false,
            splash_open: show_splash,
            splash_opened_at: None,
            settings_section: None,
            settings_search: String::new(),
        }
    }

    /// Whether a modal (start screen or Settings) covers the HUD.
    pub(crate) fn modal_open(&self) -> bool {
        self.splash_open || self.settings_section.is_some()
    }
}

/// Things the HUD asks the application to do after the frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AppCommand {
    NewScene,
    Quit,
    Minimize,
    ToggleMaximize,
    DragWindow,
    DragResize(ResizeDirection),
    FrameModel,
    HistoryJump(usize),
    SettingsChanged,
}

/// One glass panel: the rect in points and its corner radius.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Glass {
    pub(crate) rect: Rect,
    pub(crate) radius: f32,
}

/// Everything the HUD reads and changes during one frame.
pub(crate) struct Hud<'frame> {
    pub(crate) editor: &'frame mut Editor,
    pub(crate) camera: &'frame mut OrbitCamera,
    pub(crate) settings: &'frame mut Settings,
    pub(crate) ui: &'frame mut UiState,
    pub(crate) i18n: &'frame I18n,
    pub(crate) logo: &'frame egui::TextureHandle,
    pub(crate) maximized: bool,
    pub(crate) adapter_name: &'frame str,
    pub(crate) sample_counts: &'frame [u32],
    pub(crate) glass: Vec<Glass>,
    pub(crate) commands: Vec<AppCommand>,
}

impl std::fmt::Debug for Hud<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hud")
            .field("commands", &self.commands)
            .finish_non_exhaustive()
    }
}

impl Hud<'_> {
    pub(crate) fn tr(&self, key: &str) -> String {
        self.i18n.tr(key)
    }

    pub(crate) fn accent(&self) -> egui::Color32 {
        crate::theme::accent(self.settings.accent)
    }

    pub(crate) fn send(&mut self, command: AppCommand) {
        self.commands.push(command);
    }
}

/// Where an island sits on screen.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Place {
    Anchor(Align2, Vec2),
    Fixed(Pos2),
}

/// Entry animation: the island slides in from `from` (points) after `delay` seconds.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Intro {
    pub(crate) delay: f32,
    pub(crate) from: Vec2,
}

/// Draws one floating glass island and records it for the backdrop blur.
pub(crate) fn island<R>(
    ctx: &Context,
    hud: &mut Hud<'_>,
    id: &str,
    place: Place,
    intro: Intro,
    frame: egui::Frame,
    add_contents: impl FnOnce(&mut Ui, &mut Hud<'_>) -> R,
) -> InnerResponse<R> {
    let progress = intro_progress(ctx, hud, intro.delay);
    let offset = match hud.settings.motion {
        Motion::Full => intro.from * (1.0 - ease_out(progress)),
        Motion::Reduced | Motion::Off => Vec2::ZERO,
    };
    let area = egui::Area::new(Id::unique(id))
        .order(Order::Middle)
        .constrain(false)
        .sense(egui::Sense::click_and_drag());
    let area = match place {
        Place::Anchor(align, position) => area.anchor(align, position + offset),
        Place::Fixed(position) => area.fixed_pos(position + offset),
    };
    let response = area.show(ctx, |ui| {
        ui.multiply_opacity(ease_out(progress));
        frame.show(ui, |ui| add_contents(ui, hud)).inner
    });
    let radius = f32::from(
        frame
            .corner_radius
            .nw
            .max(frame.corner_radius.ne)
            .max(frame.corner_radius.se),
    );
    hud.glass.push(Glass {
        rect: response.response.rect,
        radius,
    });
    response
}

fn intro_progress(ctx: &Context, hud: &Hud<'_>, delay: f32) -> f32 {
    if hud.settings.motion == Motion::Off {
        return 1.0;
    }
    let now = ctx.input(|input| input.time);
    let started = hud.ui.started_at.unwrap_or(now);
    let elapsed = seconds_between(started, now) - delay;
    let progress = (elapsed / 0.55).clamp(0.0, 1.0);
    if progress < 1.0 {
        ctx.request_repaint();
    }
    progress
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "UI animation times are seconds since start, far inside f32 precision"
)]
pub(crate) fn seconds_between(start: f64, end: f64) -> f32 {
    (end - start) as f32
}

pub(crate) fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

/// Draws the whole HUD and handles viewport input for one frame.
pub(crate) fn draw(ctx: &Context, hud: &mut Hud<'_>) {
    if hud.ui.started_at.is_none() {
        hud.ui.started_at = Some(ctx.input(|input| input.time));
    }
    viewport_input(ctx, hud);
    top_left::show(ctx, hud);
    tool_options::show(ctx, hud);
    top_right::show(ctx, hud);
    toolbar::show(ctx, hud);
    inspector::show(ctx, hud);
    view_cube::show(ctx, hud);
    hotbar::show(ctx, hud);
    corners::show(ctx, hud);
    if !hud.ui.modal_open() {
        shortcuts(ctx, hud);
    }
}

/// Width of the band along the window border that resizes the frameless window.
const RESIZE_BAND: f32 = 6.0;

/// The full-window background: picking, brush strokes, camera navigation and window resizing.
fn viewport_input(ctx: &Context, hud: &mut Hud<'_>) {
    let screen = ctx.content_rect();
    let response = egui::Area::new(Id::unique("viewport"))
        .order(Order::Background)
        .fixed_pos(screen.min)
        .sense(egui::Sense::click_and_drag())
        .show(ctx, |ui| {
            ui.set_min_size(screen.size());
        })
        .response;

    let pointer = ctx.input(|input| input.pointer.hover_pos());
    let resize = pointer
        .filter(|_| !hud.maximized && response.hovered())
        .and_then(|position| resize_direction(screen, position));
    if let Some(direction) = resize {
        ctx.set_cursor_icon(resize_cursor(direction));
        if ctx.input(|input| input.pointer.primary_pressed()) {
            hud.send(AppCommand::DragResize(direction));
            return;
        }
    }

    // Title-bar zone: the strip from the top edge down to the bottom of the top islands moves the window.
    let in_title_zone = pointer.is_some_and(|position| position.y - screen.min.y < MARGIN + TOP_BAR_HEIGHT);
    if in_title_zone && response.hovered() && !hud.editor.is_stroking() {
        hud.editor.hover = None;
        if response.double_clicked() {
            hud.send(AppCommand::ToggleMaximize);
        } else if ctx.input(|input| input.pointer.primary_pressed()) {
            hud.send(AppCommand::DragWindow);
        }
        return;
    }

    // Hover picking.
    let aspect = screen.width() / screen.height().max(1.0);
    hud.editor.hover = None;
    let mut focus = hud.camera.target;
    if let Some(position) = pointer.filter(|_| response.hovered() || hud.editor.is_stroking()) {
        let ndc = GlamVec2::new(
            (position.x - screen.min.x) / screen.width() * 2.0 - 1.0,
            1.0 - (position.y - screen.min.y) / screen.height() * 2.0,
        );
        let ray = hud.camera.ray(ndc, aspect);
        hud.editor.hover = pick(hud.editor.scene.shape(), ray, 4000.0);
        if let Some(hit) = hud.editor.hover {
            focus = match hit.target {
                HitTarget::Voxel => hit.voxel.as_vec3() + 0.5,
                HitTarget::Ground => (hit.voxel + glam::IVec3::Y).as_vec3() + glam::Vec3::new(0.5, 0.0, 0.5),
            };
        }
    }

    // Brush strokes with the primary button.
    let (pressed, down, released) = ctx.input(|input| {
        (
            input.pointer.primary_pressed(),
            input.pointer.primary_down(),
            input.pointer.primary_released(),
        )
    });
    if pressed
        && response.hovered()
        && hud.editor.tool.is_available()
        && let Some(hit) = hud.editor.hover
    {
        hud.editor.press(hit);
    } else if down
        && hud.editor.is_stroking()
        && let Some(hit) = hud.editor.hover
    {
        hud.editor.drag(hit);
    }
    if released || (!down && hud.editor.is_stroking()) {
        hud.editor.release();
    }

    // Camera: middle drag orbits, Shift + middle drag pans, the wheel zooms towards the cursor.
    if response.dragged_by(PointerButton::Middle) {
        let delta = response.drag_delta();
        if ctx.input(|input| input.modifiers.shift) {
            hud.camera.pan(GlamVec2::new(delta.x, delta.y), screen.height());
        } else {
            hud.camera
                .orbit(GlamVec2::new(delta.x, delta.y), hud.settings.orbit_style);
        }
    }
    if response.hovered() {
        let scroll = ctx.input(|input| input.smooth_scroll_delta.y);
        if scroll.abs() > f32::EPSILON {
            let direction = if hud.settings.invert_zoom { -1.0 } else { 1.0 };
            hud.camera.zoom_towards(scroll / 60.0 * direction, focus);
        }
    }
}

fn resize_direction(screen: Rect, position: Pos2) -> Option<ResizeDirection> {
    let west = position.x - screen.min.x < RESIZE_BAND;
    let east = screen.max.x - position.x < RESIZE_BAND;
    let north = position.y - screen.min.y < RESIZE_BAND;
    let south = screen.max.y - position.y < RESIZE_BAND;
    match (north, south, west, east) {
        (true, _, true, _) => Some(ResizeDirection::NorthWest),
        (true, _, _, true) => Some(ResizeDirection::NorthEast),
        (_, true, true, _) => Some(ResizeDirection::SouthWest),
        (_, true, _, true) => Some(ResizeDirection::SouthEast),
        (true, ..) => Some(ResizeDirection::North),
        (_, true, ..) => Some(ResizeDirection::South),
        (_, _, true, _) => Some(ResizeDirection::West),
        (_, _, _, true) => Some(ResizeDirection::East),
        _ => None,
    }
}

fn resize_cursor(direction: ResizeDirection) -> egui::CursorIcon {
    match direction {
        ResizeDirection::North | ResizeDirection::South => egui::CursorIcon::ResizeVertical,
        ResizeDirection::East | ResizeDirection::West => egui::CursorIcon::ResizeHorizontal,
        ResizeDirection::NorthWest | ResizeDirection::SouthEast => egui::CursorIcon::ResizeNwSe,
        ResizeDirection::NorthEast | ResizeDirection::SouthWest => egui::CursorIcon::ResizeNeSw,
    }
}

/// Keyboard shortcuts from Settings → Input.
fn shortcuts(ctx: &Context, hud: &mut Hud<'_>) {
    const SLOT_KEYS: [Key; 10] = [
        Key::Num1,
        Key::Num2,
        Key::Num3,
        Key::Num4,
        Key::Num5,
        Key::Num6,
        Key::Num7,
        Key::Num8,
        Key::Num9,
        Key::Num0,
    ];
    if ctx.egui_wants_keyboard_input() {
        return;
    }
    let undo = ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::Z));
    let redo = ctx.input_mut(|input| {
        input.consume_key(Modifiers::COMMAND, Key::Y)
            || input.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z)
    });
    if undo {
        hud.editor.undo();
    }
    if redo {
        hud.editor.redo();
    }
    if ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::N)) {
        hud.send(AppCommand::NewScene);
    }
    if ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::Comma)) {
        hud.ui.settings_section = Some(Section::General);
    }
    if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Home)) {
        hud.send(AppCommand::FrameModel);
    }
    for tool in Tool::ALL {
        if tool.is_available() && ctx.input_mut(|input| input.consume_key(Modifiers::NONE, tool.key())) {
            hud.editor.tool = tool;
        }
    }
    for (slot, key) in SLOT_KEYS.into_iter().enumerate() {
        if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, key)) {
            hud.editor.active_slot = slot;
        }
    }
}

/// Snap target for a click on a view cube face.
pub(crate) fn snap(hud: &mut Hud<'_>, face: Face) {
    hud.camera.snap(face);
}

/// A rounded rect radius with only selected corners rounded.
pub(crate) fn corner(top_left: u8, top_right: u8, bottom_right: u8, bottom_left: u8) -> CornerRadius {
    CornerRadius {
        nw: top_left,
        ne: top_right,
        se: bottom_right,
        sw: bottom_left,
    }
}

pub(crate) const MARGIN: f32 = 12.0;
pub(crate) const INSPECTOR_WIDTH: f32 = 296.0;
pub(crate) const TOP_BAR_HEIGHT: f32 = 44.0;

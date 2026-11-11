//! The viewport-first HUD from the Final mockups: floating glass islands over a full-window viewport.

mod corners;
mod hotbar;
mod inspector;
mod materials_preview;
mod prompt;
mod render_bottom;
mod render_inspector;
mod render_tools;
mod render_viewport;
mod scene_tree;
mod toast;
mod tool_options;
mod toolbar;
mod top_left;
mod top_right;
mod view_cube;
mod viewport;
pub(crate) mod widgets;

use std::collections::HashSet;
use std::path::PathBuf;

use egui::{Align2, Context, CornerRadius, Id, InnerResponse, Key, Modifiers, Order, Pos2, Rect, Ui, Vec2};
pub(crate) use prompt::Prompt;
use rustium_core::{Face, NodeId};
pub(crate) use toast::Toast;
use winit::window::ResizeDirection;

use crate::camera::OrbitCamera;
use crate::editor::{Editor, Tool};
use crate::i18n::I18n;
use crate::mcp::McpStatus;
use crate::recent::RecentItem;
use crate::render_mode::RenderState;
use crate::settings::{Motion, Settings};
use crate::settings_window::Section;

/// The app's modes, switched in the top-left island.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Editing voxels with the raster viewport.
    Model,
    /// Path-traced preview and image output.
    Render,
}

/// Tabs of the right-hand inspector in Model mode.
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
    pub(crate) mode: Mode,
    pub(crate) render: RenderState,
    pub(crate) inspector_open: bool,
    pub(crate) inspector_tab: InspectorTab,
    pub(crate) palette_open: bool,
    pub(crate) splash_open: bool,
    pub(crate) splash_opened_at: Option<f64>,
    pub(crate) settings_section: Option<Section>,
    pub(crate) settings_search: String,
    /// Where the Tool wheel is open, in points.
    pub(crate) tool_wheel: Option<Pos2>,
    /// Where a marquee drag started, in points.
    pub(crate) marquee: Option<Pos2>,
    /// Groups collapsed in the Scene tree.
    pub(crate) collapsed: HashSet<NodeId>,
    /// The Node being renamed in the Scene tree and the text typed so far.
    pub(crate) renaming: Option<(NodeId, String)>,
    /// A short message under the top bar, e.g. the result of a script run.
    pub(crate) toast: Option<Toast>,
    /// A question that blocks the interface until answered.
    pub(crate) prompt: Option<Prompt>,
    /// Recently opened files for the start screen.
    pub(crate) recent: Vec<RecentItem>,
    /// File name of the open document; empty while it has never been saved.
    pub(crate) document_name: String,
}

impl UiState {
    pub(crate) fn new(show_splash: bool) -> Self {
        Self {
            started_at: None,
            mode: Mode::Model,
            render: RenderState::new(&OrbitCamera::default()),
            inspector_open: true,
            inspector_tab: InspectorTab::Scene,
            palette_open: false,
            splash_open: show_splash,
            splash_opened_at: None,
            settings_section: None,
            settings_search: String::new(),
            tool_wheel: None,
            marquee: None,
            collapsed: HashSet::new(),
            renaming: None,
            toast: None,
            prompt: None,
            recent: Vec::new(),
            document_name: String::new(),
        }
    }

    /// Whether a modal (start screen or Settings) covers the HUD.
    pub(crate) fn modal_open(&self) -> bool {
        self.splash_open || self.settings_section.is_some() || self.prompt.is_some()
    }
}

/// Things the HUD asks the application to do after the frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AppCommand {
    /// Start an empty Scene (asks first when there are unsaved changes).
    NewScene,
    /// Open this file, or ask for one (asks first when there are unsaved changes).
    Open(Option<PathBuf>),
    /// Save to the document's file, or ask for one.
    Save,
    /// Save to a file chosen in a dialog.
    SaveAs,
    /// Import a MagicaVoxel file as a new Scene.
    ImportVox,
    /// Export the Scene as a MagicaVoxel file.
    ExportVox,
    /// Replace Palette colors from a `.pal` or PNG file.
    ImportPalette,
    /// Export the Palette as a `.pal` file.
    ExportPalette,
    /// Answers of the "save changes?" and recovery prompts.
    PromptSave,
    PromptDiscard,
    PromptCancel,
    RecoveryRestore,
    RecoveryDiscard,
    /// Leave the application (asks first when there are unsaved changes).
    Quit,
    Minimize,
    ToggleMaximize,
    DragWindow,
    DragResize(ResizeDirection),
    FrameModel,
    HistoryJump(usize),
    SettingsChanged,
    /// Save the traced image through a file dialog; with the MP4 format it renders a turntable video instead.
    SaveImage,
    /// Render every Shot to a file in a folder chosen in a dialog.
    QueueShots,
    /// Stop the running render job.
    CancelJob,
    /// Throw away accumulated samples and trace again.
    RestartRender,
    /// Run a Luau script chosen in a file dialog.
    RunScript,
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
    pub(crate) mcp_status: &'frame McpStatus,
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
    if hud.ui.mode == Mode::Render {
        render_viewport::layout(ctx, hud);
    }
    viewport_input(ctx, hud);
    top_left::show(ctx, hud);
    top_right::show(ctx, hud);
    inspector::show(ctx, hud);
    toast::show(ctx, hud);
    match hud.ui.mode {
        Mode::Model => {
            tool_options::show(ctx, hud);
            toolbar::show(ctx, hud);
            view_cube::show(ctx, hud);
            hotbar::show(ctx, hud);
            corners::show(ctx, hud);
        }
        Mode::Render => {
            render_tools::options(ctx, hud);
            render_tools::toolbar(ctx, hud);
            render_bottom::shots(ctx, hud);
            render_bottom::corners(ctx, hud);
        }
    }
    if !hud.ui.modal_open() {
        shortcuts(ctx, hud);
    }
    prompt::show(ctx, hud);
}

/// Puts the MCP client configuration snippet on the clipboard and says so.
pub(crate) fn copy_client_config(ctx: &Context, hud: &mut Hud<'_>) {
    ctx.copy_text(hud.settings.mcp.client_config());
    hud.ui.toast = Some(Toast::new(hud.tr("mcp-copied"), false));
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
            if hud.ui.mode == Mode::Render {
                render_viewport::paint(ui, hud);
            }
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

    if hud.ui.mode == Mode::Render {
        render_viewport::input(ctx, hud, &response, pointer);
        return;
    }
    viewport::interact(ctx, hud, &response, screen, pointer);
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
    if ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::O)) {
        hud.send(AppCommand::Open(None));
    }
    // Ctrl Shift S first: Ctrl S would also match while Shift is held.
    if ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::S)) {
        hud.send(AppCommand::SaveAs);
    } else if ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::S)) {
        hud.send(AppCommand::Save);
    }
    if ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::Comma)) {
        hud.ui.settings_section = Some(Section::General);
    }
    if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Home)) {
        hud.send(AppCommand::FrameModel);
    }
    if hud.ui.mode == Mode::Render {
        render_viewport::shortcuts(ctx, hud);
        return;
    }
    for tool in Tool::ALL {
        if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, tool.key())) {
            hud.editor.tool = tool;
        }
    }
    focus_keys(ctx, hud);
    for (slot, key) in SLOT_KEYS.into_iter().enumerate() {
        if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, key)) {
            hud.editor.active_slot = slot;
        }
    }
}

/// Enter steps into the selected Node, Esc clears the voxel selection and then steps out, Delete removes Nodes.
fn focus_keys(ctx: &Context, hud: &mut Hud<'_>) {
    if hud.ui.renaming.is_some() {
        return;
    }
    if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Enter)) {
        let target = hud
            .editor
            .node_selection
            .last()
            .copied()
            .or_else(|| hud.editor.hover.and_then(|hit| hit.node))
            .and_then(|node| hud.editor.scene.child_toward(hud.editor.focus, node).or(Some(node)));
        if let Some(target) = target {
            hud.editor.step_in(target);
        }
    }
    if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape)) {
        if !hud.editor.selection.is_empty() {
            hud.editor.selection = rustium_core::Selection::new();
        } else if !hud.editor.node_selection.is_empty() && !hud.editor.focus_is_model() {
            hud.editor.node_selection.clear();
        } else {
            hud.editor.step_out();
        }
    }
    if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Delete)) {
        for node in hud.editor.node_selection.clone() {
            hud.editor.execute(&rustium_core::Command::Delete { node });
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

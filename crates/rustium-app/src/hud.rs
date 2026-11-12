//! The viewport-first HUD from the Final mockups: floating glass islands over a full-window viewport.

mod corners;
mod cursors;
mod effects;
mod effects_bars;
mod effects_canvas;
mod effects_inspector;
mod hotbar;
mod inspector;
mod prompt;
mod render_bottom;
mod render_inspector;
mod render_tools;
mod render_viewport;
mod scene_tree;
mod toast;
mod tool_options;
mod tool_wheel;
mod toolbar;
mod top_left;
mod top_right;
mod view_cube;
mod viewport;
pub(crate) mod widgets;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

pub(crate) use cursors::CursorUi;
pub(crate) use effects::{EffectsState, GraphTool, PreviewShape};
pub(crate) use effects_bars::preview_view_rect;
use egui::{Align2, Color32, Context, CornerRadius, Id, InnerResponse, Order, Pos2, Rect, Ui, Vec2};
pub(crate) use prompt::Prompt;
use rustium_core::{Face, NodeId};
use rustium_render::TraceEngine;
pub(crate) use toast::Toast;
use winit::window::ResizeDirection;

use crate::camera::OrbitCamera;
use crate::editor::Editor;
use crate::i18n::I18n;
use crate::keymap::{Action, Scope};
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
    /// The Material node editor.
    Effects,
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
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent open/closed flags of separate panels"
)]
pub(crate) struct UiState {
    pub(crate) started_at: Option<f64>,
    pub(crate) mode: Mode,
    pub(crate) render: RenderState,
    pub(crate) effects: EffectsState,
    pub(crate) inspector_open: bool,
    pub(crate) inspector_tab: InspectorTab,
    pub(crate) palette_open: bool,
    pub(crate) splash_open: bool,
    pub(crate) splash_opened_at: Option<f64>,
    pub(crate) settings_section: Option<Section>,
    pub(crate) settings_search: String,
    /// The Interface scale shown while its slider is dragged; applied on release.
    pub(crate) scale_draft: Option<u32>,
    /// Whether a text field had keyboard focus at the end of the previous frame.
    pub(crate) text_focus_before: bool,
    /// Where the Tool wheel is open, in points.
    pub(crate) tool_wheel: Option<Pos2>,
    /// Where the right button went down over the viewport, until it is released.
    pub(crate) wheel_press: Option<Pos2>,
    /// The primary press in progress began while a text field had focus, so it must not reach the Tools.
    pub(crate) press_swallowed: bool,
    /// Viewport cursor bookkeeping.
    pub(crate) cursor: CursorUi,
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
    /// Where the maximize button was drawn last frame, in points; Windows shows Snap Layouts over it.
    pub(crate) maximize_button: Option<Rect>,
    /// Settings → Input: rebinding, search and file dialogs.
    pub(crate) keymap_ui: crate::settings_input::KeymapUi,
    /// Where each island was drawn last frame, by id, so centre islands can keep clear of the corner ones.
    pub(crate) island_rects: HashMap<String, Rect>,
    /// How compact each shrinking island is drawn.
    pub(crate) fit: HashMap<String, Fit>,
}

impl UiState {
    pub(crate) fn new(show_splash: bool) -> Self {
        Self {
            started_at: None,
            mode: Mode::Model,
            render: RenderState::new(&OrbitCamera::default()),
            effects: EffectsState::new(),
            inspector_open: true,
            inspector_tab: InspectorTab::Scene,
            palette_open: false,
            splash_open: show_splash,
            splash_opened_at: None,
            settings_section: None,
            settings_search: String::new(),
            scale_draft: None,
            text_focus_before: false,
            tool_wheel: None,
            wheel_press: None,
            press_swallowed: false,
            cursor: CursorUi::default(),
            marquee: None,
            collapsed: HashSet::new(),
            renaming: None,
            toast: None,
            prompt: None,
            recent: Vec::new(),
            document_name: String::new(),
            maximize_button: None,
            keymap_ui: crate::settings_input::KeymapUi::default(),
            island_rects: HashMap::new(),
            fit: HashMap::new(),
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
    pub(crate) adapters: &'frame [rustium_render::AdapterChoice],
    /// The traversal the viewport's path tracer runs, and whether the GPU could run the hardware one.
    pub(crate) trace_engine: TraceEngine,
    pub(crate) ray_query_supported: bool,
    pub(crate) sample_counts: &'frame [u32],
    pub(crate) mcp_status: &'frame McpStatus,
    pub(crate) material_status: &'frame rustium_render::CompileStatus,
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
    // The backdrop blur only samples the 3D view; over the node canvas (drawn by egui) glass would let node text
    // show through, so Effects islands are fully opaque.
    let frame = if hud.ui.mode == Mode::Effects && frame.fill == crate::theme::PANEL {
        frame.fill(Color32::from_rgba_unmultiplied(17, 19, 23, 255))
    } else {
        frame
    };
    let offset = match hud.settings.motion {
        Motion::Full => intro.from * (1.0 - ease_out(progress)),
        Motion::Reduced | Motion::Off => Vec2::ZERO,
    };
    let area = egui::Area::new(Id::unique(id))
        .order(Order::Middle)
        .constrain(false)
        .sense(egui::Sense::click_and_drag());
    let place = placed_by_known_width(ctx, hud, id, place);
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
    let rect = response.response.rect;
    hud.glass.push(Glass { rect, radius });
    hud.ui.island_rects.insert(id.to_owned(), rect);
    if let Some(fit) = hud.ui.fit.get_mut(id) {
        fit.widths[usize::from(fit.level)] = rect.width();
    }
    response
}

/// An island anchored at the horizontal centre is placed from the width its current layout measured before, instead of
/// the size the area had last frame: after its content changes (Pause to Render, a Shot added) the anchor would
/// otherwise centre the old width for one frame.
fn placed_by_known_width(ctx: &Context, hud: &Hud<'_>, id: &str, place: Place) -> Place {
    let Place::Anchor(align, offset) = place else {
        return place;
    };
    let (Some(fit), Some(previous)) = (hud.ui.fit.get(id), hud.ui.island_rects.get(id)) else {
        return place;
    };
    let width = fit.widths[usize::from(fit.level)];
    if align.x() != egui::Align::Center || width == 0.0 {
        return place;
    }
    let screen = ctx.content_rect();
    let top = match align.y() {
        egui::Align::Min => screen.top(),
        egui::Align::Center => screen.center().y - previous.height() / 2.0,
        egui::Align::Max => screen.bottom() - previous.height(),
    };
    Place::Fixed(Pos2::new(screen.center().x - width / 2.0, top) + offset)
}

/// Space kept between islands that sit side by side.
pub(crate) const ISLAND_GAP: f32 = 8.0;
/// The narrowest a bottom-centre island gets; the bottom corner islands leave this much room.
const CENTRE_MIN: f32 = 385.0;
/// Number of layouts an island can shrink through, from 0 (full) up.
pub(crate) const FIT_LEVELS: usize = 5;

/// How compact an island is drawn and how wide each layout turned out to be.
#[derive(Debug, Default)]
pub(crate) struct Fit {
    level: u8,
    widths: [f32; FIT_LEVELS],
    key: u64,
    /// Widths measured under other keys, so going back to a layout (a Render status that comes and goes) does not
    /// measure every level again.
    remembered: Vec<(u64, [f32; FIT_LEVELS])>,
}

impl Fit {
    /// Moves to the widths measured under `key`: the ones remembered from an earlier visit, else none.
    fn switch_key(&mut self, key: u64) {
        if self.key == key {
            return;
        }
        if self.widths.iter().any(|width| *width != 0.0) {
            self.remembered.retain(|(known, _)| *known != self.key);
            self.remembered.push((self.key, self.widths));
        }
        // Few distinct keys occur in practice; the oldest memories go first.
        if self.remembered.len() > 8 {
            self.remembered.remove(0);
        }
        self.key = key;
        self.widths = self
            .remembered
            .iter()
            .find(|(known, _)| *known == key)
            .map_or([0.0; FIT_LEVELS], |(_, widths)| *widths);
    }
}

/// A hash of what decides the width of an island (the Tool, the language), to forget stale measurements.
pub(crate) fn fit_key(parts: &[&dyn std::fmt::Debug]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for part in parts {
        format!("{part:?}").hash(&mut hasher);
    }
    hasher.finish()
}

/// The layout an island must use to fit in `budget` points: the first of the levels `0..=max` whose measured width
/// fits. Widths are measured when an island is drawn (see [`island`]), so a level that was never drawn is tried
/// next frame. Call it before drawing the island with the same `id`.
pub(crate) fn fit_level(ctx: &Context, hud: &mut Hud<'_>, id: &str, budget: f32, key: u64, max: u8) -> u8 {
    let fit = hud.ui.fit.entry(id.to_owned()).or_default();
    fit.switch_key(key);
    let mut chosen = max;
    for level in 0..=max {
        let width = fit.widths[usize::from(level)];
        if width == 0.0 || width <= budget {
            chosen = level;
            break;
        }
    }
    fit.level = chosen;
    if fit.widths[usize::from(chosen)] == 0.0 {
        ctx.request_repaint();
    }
    chosen
}

/// Width available to an island centred in the window between the islands `left` and `right`, as drawn last frame.
pub(crate) fn centre_budget(ctx: &Context, hud: &Hud<'_>, left: &str, right: &str) -> f32 {
    let centre = ctx.content_rect().center().x;
    match (hud.ui.island_rects.get(left), hud.ui.island_rects.get(right)) {
        (Some(left), Some(right)) => 2.0 * (centre - left.right()).min(right.left() - centre) - 2.0 * ISLAND_GAP,
        _ => f32::MAX,
    }
}

/// Width a bottom corner island may take so the bottom-centre island still fits between the two.
pub(crate) fn corner_budget(ctx: &Context) -> f32 {
    (ctx.content_rect().width() - CENTRE_MIN) / 2.0 - ISLAND_GAP
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
    // egui hands keyboard focus to the first widget on Tab while the frame is built; remember whether the key was
    // meant for a shortcut before that happens.
    // Escape already took focus off a text field when the frame starts, so the previous frame's state counts too.
    // The Add node popup takes the keys typed right after Tab, before its search field has focus.
    let text_focus = ctx.egui_wants_keyboard_input()
        || hud.ui.text_focus_before
        || (hud.ui.mode == Mode::Effects && hud.ui.effects.add_popup.is_some());
    let tab_shortcut =
        hud.ui.mode == Mode::Effects && !text_focus && ctx.input(|input| input.key_pressed(egui::Key::Tab));
    // A popup, menu, the Tool wheel or a text field that is open when the frame starts owns the Escape key.
    let escape_owned = text_focus || escape_owner_open(ctx, hud);
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
        Mode::Effects => effects::draw(ctx, hud),
        Mode::Render => {
            render_tools::options(ctx, hud);
            render_tools::toolbar(ctx, hud);
            render_bottom::shots(ctx, hud);
            render_bottom::corners(ctx, hud);
        }
    }
    if escape_owned && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        // Whatever closed on Escape is drawn one frame later; make sure that frame happens.
        ctx.request_repaint();
    }
    // Keys typed into a text field that loses focus in this very frame (Enter, Escape) are not shortcuts either.
    if !hud.ui.modal_open() && !text_focus {
        if tab_shortcut {
            ctx.memory_mut(egui::Memory::stop_text_input);
        }
        shortcuts(ctx, hud);
    }
    prompt::show(ctx, hud);
    cursors::update(ctx, hud);
    hud.ui.text_focus_before = ctx.egui_wants_keyboard_input();
}

/// Whether something that closes on Escape is open: an egui popup or menu, the Tool wheel (any mode), a Scene tree
/// rename or one of the Effects popups.
fn escape_owner_open(ctx: &Context, hud: &Hud<'_>) -> bool {
    egui::Popup::is_any_open(ctx)
        || (hud.ui.mode == Mode::Model && hud.ui.palette_open)
        || (hud.ui.mode == Mode::Effects
            && hud.ui.effects.assign_open
            && hud.ui.effects.tab == effects::EffectsTab::Material)
        || hud.ui.tool_wheel.is_some()
        || hud.ui.renaming.is_some()
        || hud.ui.effects.add_popup.is_some()
        || hud.ui.effects.frame_label.is_some()
        || hud.ui.effects.color_popup.is_some()
}

/// Puts the MCP client configuration snippet on the clipboard and says so.
pub(crate) fn copy_client_config(ctx: &Context, hud: &mut Hud<'_>) {
    ctx.copy_text(hud.settings.mcp.client_config());
    hud.ui.toast = Some(Toast::new(hud.tr("mcp-copied"), false));
}

/// Pointer travel (points) below which a right press and release count as a click.
const WHEEL_SLOP: f32 = 8.0;

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
            } else if hud.ui.mode == Mode::Effects {
                let hole = effects_bars::preview_view_rect(screen);
                effects_canvas::paint(ui, hud, screen, hole);
            }
        })
        .response;

    let pointer = ctx.input(|input| input.pointer.hover_pos());
    // The window's edges and title strip belong to the desktop window; a page has neither.
    let resize = pointer
        .filter(|_| crate::DESKTOP && !hud.maximized && response.hovered())
        .and_then(|position| resize_direction(screen, position));
    if let Some(direction) = resize {
        ctx.set_cursor_icon(resize_cursor(direction));
        if ctx.input(|input| input.pointer.primary_pressed()) {
            hud.send(AppCommand::DragResize(direction));
            return;
        }
    }

    // Title-bar zone: the strip from the top edge down to the bottom of the top islands moves the window.
    let in_title_zone =
        crate::DESKTOP && pointer.is_some_and(|position| position.y - screen.min.y < MARGIN + TOP_BAR_HEIGHT);
    if in_title_zone && response.hovered() && !hud.editor.is_stroking() {
        hud.editor.hover = None;
        if response.double_clicked() {
            hud.send(AppCommand::ToggleMaximize);
        } else if ctx.input(|input| input.pointer.primary_pressed()) {
            hud.send(AppCommand::DragWindow);
        }
        return;
    }

    hud.ui.cursor.pointer = pointer.filter(|_| resize.is_none() && (response.contains_pointer() || response.dragged()));
    // The Tool wheel of the current mode owns the pointer while it is open.
    if let Some(centre) = hud.ui.tool_wheel {
        hud.editor.clear_hover();
        tool_wheel::show(ctx, hud, centre);
        return;
    }
    let busy = hud.editor.is_stroking() || hud.ui.effects.gesture.is_some() || egui::Popup::is_any_open(ctx);
    open_wheel_on_right_click(ctx, hud, &response, pointer, busy);

    // A press that began while a text field had focus only commits the field: the Tool must not act on it.
    let (pressed, down, released) = ctx.input(|input| {
        (
            input.pointer.primary_pressed(),
            input.pointer.primary_down(),
            input.pointer.primary_released(),
        )
    });
    if pressed {
        hud.ui.press_swallowed = hud.ui.text_focus_before;
    } else if !down && !released {
        hud.ui.press_swallowed = false;
    }
    if hud.ui.press_swallowed {
        hud.editor.clear_hover();
        return;
    }

    if hud.ui.mode == Mode::Render {
        render_viewport::input(ctx, hud, &response, pointer);
        return;
    }
    if hud.ui.mode == Mode::Effects {
        hud.editor.clear_hover();
        effects_canvas::input(ctx, hud, &response, pointer);
        return;
    }
    viewport::interact(ctx, hud, &response, screen, pointer);
}

/// Opens the Tool wheel when the right button is released near where it went down, however long it was held.
fn open_wheel_on_right_click(
    ctx: &Context,
    hud: &mut Hud<'_>,
    response: &egui::Response,
    pointer: Option<Pos2>,
    busy: bool,
) {
    let (pressed, released) = ctx.input(|input| {
        (
            input.pointer.button_pressed(egui::PointerButton::Secondary),
            input.pointer.button_released(egui::PointerButton::Secondary),
        )
    });
    if pressed && response.contains_pointer() && !busy {
        hud.ui.wheel_press = pointer;
    }
    if released
        && let Some(origin) = hud.ui.wheel_press.take()
        && !busy
        && pointer.is_none_or(|position| position.distance(origin) < WHEEL_SLOP)
    {
        hud.ui.tool_wheel = Some(origin);
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
    if ctx.egui_wants_keyboard_input() {
        return;
    }
    let scope = match hud.ui.mode {
        Mode::Render => Scope::Render,
        Mode::Effects => Scope::Effects,
        Mode::Model => Scope::Model,
    };
    if scope == Scope::Effects {
        effects::extra_keys(ctx, hud);
    }
    let pressed = ctx.input_mut(|input| hud.settings.keymap.take_pressed(input, scope));
    if scope == Scope::Effects && pressed.contains(&Action::AddNode) {
        // Letters typed in the same frame as Tab belong to the node search, not to the graph tool shortcuts.
        effects::shortcut(ctx, hud, Action::AddNode);
        let typed: String = ctx.input(|input| {
            input
                .events
                .iter()
                .filter_map(|event| match event {
                    egui::Event::Text(text) => Some(text.as_str()),
                    _ => None,
                })
                .collect()
        });
        if let Some(popup) = hud.ui.effects.add_popup.as_mut() {
            popup.query.push_str(&typed);
        }
        return;
    }
    for action in pressed {
        match action {
            Action::Undo => hud.editor.undo(),
            Action::Redo => hud.editor.redo(),
            Action::NewScene => hud.send(AppCommand::NewScene),
            Action::OpenFile => hud.send(AppCommand::Open(None)),
            Action::SaveFile => hud.send(AppCommand::Save),
            Action::SaveFileAs => hud.send(AppCommand::SaveAs),
            Action::OpenSettings => hud.ui.settings_section = Some(Section::General),
            Action::FrameModel => hud.send(AppCommand::FrameModel),
            Action::Tool(tool) => hud.editor.tool = tool,
            Action::Slot(slot) => hud.editor.active_slot = usize::from(slot),
            Action::StepIn | Action::StepOut | Action::DeleteNodes => focus_action(hud, action),
            Action::GraphTool(_)
            | Action::AddNode
            | Action::FrameNodes
            | Action::FitGraph
            | Action::DeleteGraphSelection
            | Action::DuplicateNodes
            | Action::SelectAllNodes
            | Action::ClearNodeSelection
            | Action::MaterialSlot(_) => effects::shortcut(ctx, hud, action),
            Action::RenderTool(_) | Action::RenderStart | Action::ClearRenderRegion => {
                render_viewport::shortcut(hud, action);
            }
        }
    }
}

/// Step in selects the Node under the selection or cursor, step out clears the voxel selection and then steps
/// out of the focused Node, and delete removes the selected Nodes.
fn focus_action(hud: &mut Hud<'_>, action: Action) {
    if hud.ui.renaming.is_some() {
        return;
    }
    match action {
        Action::StepIn => {
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
        Action::StepOut => {
            if !hud.editor.selection.is_empty() {
                hud.editor.selection = rustium_core::Selection::new();
            } else if !hud.editor.node_selection.is_empty() && !hud.editor.focus_is_model() {
                hud.editor.node_selection.clear();
            } else {
                hud.editor.step_out();
            }
        }
        Action::DeleteNodes => {
            for node in hud.editor.node_selection.clone() {
                hud.editor.execute(&rustium_core::Command::Delete { node });
            }
        }
        _ => {}
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

#[cfg(test)]
mod fit_tests {
    use super::*;

    #[test]
    fn widths_come_back_when_an_earlier_key_returns() {
        let mut fit = Fit::default();
        fit.switch_key(1);
        fit.widths = [300.0, 200.0, 0.0, 0.0, 0.0];
        fit.switch_key(2);
        assert_eq!(fit.widths, [0.0; FIT_LEVELS], "a new key has nothing measured");
        fit.widths = [320.0, 0.0, 0.0, 0.0, 0.0];
        fit.switch_key(1);
        assert_eq!(fit.widths, [300.0, 200.0, 0.0, 0.0, 0.0], "no level is measured again");
        fit.switch_key(2);
        assert_eq!(fit.widths[0], 320.0);
    }
}

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use color_eyre::eyre::{Result, WrapErr, eyre};
use egui::ViewportId;
use fluent_bundle::FluentValue;
use glam::{IVec3, Vec3};
use rustium_core::{ChunkMesh, Volume, mesh_chunk};
use rustium_platform::{HitRegions, Platform};
use rustium_render::{
    CameraUniform, GlassRect, GpuContext, GpuPreferences, GraphicsBackend, LineVertex, ModelDraw, RayTracing,
    TraceFrame, ViewportFrame, ViewportRenderer,
};
use tracing::{error, info, warn};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::window::{Icon, Window, WindowId};

use crate::camera::OrbitCamera;
use crate::editor::Editor;
use crate::effects_preview::{self, Preview};
use crate::files::{self, Document, Io, Pending};
use crate::hud::{self, AppCommand, Hud, Mode, PreviewShape, Toast, UiState};
use crate::i18n::I18n;
use crate::mcp::McpHost;
use crate::recent::{self, RecentFile};
use crate::render_job::{self, Destination, JobSpec, Output, OutputKind};
use crate::render_mode::{Backdrop, ImageFormat};
use crate::render_output::{self, SaveOutcome};
use crate::settings::{Backend, FrameLimit, RayTracingMode, Settings};
use crate::{settings_window, splash, theme};

mod documents;

use documents::PromptAnswer;

const LOGO_PNG: &[u8] = include_bytes!("../assets/logo.png");

/// Events other threads send to the event loop through its `EventLoopProxy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UserEvent {
    /// The MCP server queued requests for the UI thread.
    Automation,
    /// The IO thread finished a job.
    Io,
}

/// The winit application: owns the settings and, once resumed, the running editor.
pub(crate) struct App {
    settings: Settings,
    proxy: EventLoopProxy<UserEvent>,
    running: Option<Running>,
    error: Option<color_eyre::Report>,
    /// A file named on the command line, opened at startup.
    open_path: Option<PathBuf>,
}

impl App {
    pub(crate) fn new(settings: Settings, proxy: EventLoopProxy<UserEvent>, open_path: Option<PathBuf>) -> Self {
        Self {
            settings,
            proxy,
            running: None,
            error: None,
            open_path,
        }
    }

    /// The error that stopped the event loop, if any.
    pub(crate) fn into_result(self) -> Result<()> {
        self.error.map_or(Ok(()), Err)
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, report: color_eyre::Report) {
        error!(error = ?report, "fatal error");
        self.error = Some(report);
        event_loop.exit();
    }
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("running", &self.running.is_some())
            .finish_non_exhaustive()
    }
}

/// What a render job produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobKind {
    Turntable,
    Queue,
}

/// Everything that exists while the window is open.
struct Running {
    window: Arc<Window>,
    gpu: GpuContext,
    viewport: ViewportRenderer,
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
    logo: egui::TextureHandle,
    editor: Editor,
    camera: OrbitCamera,
    ui: UiState,
    i18n: I18n,
    mesh: ChunkMesh,
    preview: Preview,
    sample_counts: Vec<u32>,
    adapter_name: String,
    next_frame: Option<Instant>,
    /// Mode and Editor revision the path tracer's scene upload matches.
    last_mode: Mode,
    trace_revision: Option<u64>,
    save_results: Receiver<SaveOutcome>,
    save_sender: mpsc::Sender<SaveOutcome>,
    mcp: McpHost,
    io: Io,
    doc: Document,
    /// The document and revision whose unsaved changes the user agreed to lose for the file being opened.
    accepted_loss: Option<(u64, u64)>,
    recent: Vec<RecentFile>,
    splash_was_open: bool,
    title: String,
    /// Win32 hit-testing for Snap Layouts; `None` off Windows.
    platform: Option<Platform>,
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.running.is_some() {
            return;
        }
        match Running::new(event_loop, &self.settings, &self.proxy, self.open_path.take()) {
            Ok(running) => self.running = Some(running),
            Err(report) => self.fail(event_loop, report),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _window_id: WindowId, event: WindowEvent) {
        let Some(running) = &mut self.running else {
            return;
        };
        let response = running.egui_state.on_window_event(&running.window, &event);
        match event {
            WindowEvent::CloseRequested => {
                if running.run_command(AppCommand::Quit, &mut self.settings) {
                    event_loop.exit();
                } else {
                    // The "unsaved changes" prompt has to appear.
                    running.window.request_redraw();
                }
            }
            WindowEvent::Resized(size) => {
                running.gpu.resize(size.width, size.height);
                running.window.request_redraw();
            }
            WindowEvent::RedrawRequested => match running.redraw(&mut self.settings) {
                Ok(true) => event_loop.exit(),
                Ok(false) => {}
                Err(report) => self.fail(event_loop, report),
            },
            _ => {}
        }
        if response.repaint
            && let Some(running) = &self.running
        {
            running.window.request_redraw();
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        let Some(running) = &mut self.running else {
            return;
        };
        match event {
            UserEvent::Automation => {
                running.mcp.drain(&mut running.editor);
                running.window.request_redraw();
            }
            UserEvent::Io => {
                if running.handle_io(&self.settings) {
                    event_loop.exit();
                }
                running.window.request_redraw();
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(running) = &mut self.running else {
            return;
        };
        running.autosave_tick(&self.settings);
        let wake = [running.next_frame, running.next_autosave(&self.settings)]
            .into_iter()
            .flatten()
            .min();
        match wake {
            Some(when) if when <= Instant::now() => {
                running.window.request_redraw();
                event_loop.set_control_flow(ControlFlow::Wait);
            }
            Some(when) => event_loop.set_control_flow(ControlFlow::WaitUntil(when)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }
}

impl Running {
    fn new(
        event_loop: &ActiveEventLoop,
        settings: &Settings,
        proxy: &EventLoopProxy<UserEvent>,
        open_path: Option<PathBuf>,
    ) -> Result<Self> {
        let (logo_rgba, logo_size) = decode_png(LOGO_PNG).wrap_err("cannot decode the logo")?;
        let attributes = Window::default_attributes()
            .with_title("Rustium")
            .with_decorations(false)
            .with_inner_size(LogicalSize::new(1440.0, 900.0))
            .with_min_inner_size(LogicalSize::new(960.0, 640.0))
            .with_window_icon(Icon::from_rgba(logo_rgba.clone(), logo_size.0, logo_size.1).ok());
        #[cfg(windows)]
        let attributes = {
            use winit::platform::windows::WindowAttributesExtWindows;
            attributes.with_undecorated_shadow(true)
        };
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .wrap_err("cannot create the window")?,
        );
        let platform = Platform::attach(&*window);
        let size = window.inner_size();
        let preferences = GpuPreferences {
            backend: match settings.backend {
                Backend::Automatic => GraphicsBackend::Automatic,
                Backend::Vulkan => GraphicsBackend::Vulkan,
                Backend::DirectX12 => GraphicsBackend::DirectX12,
            },
        };
        let mut gpu = pollster::block_on(GpuContext::new(
            Arc::clone(&window),
            size.width,
            size.height,
            preferences,
        ))
        .wrap_err("cannot open the GPU")?;
        gpu.set_vsync(settings.vsync);
        let sample_counts = gpu.supported_sample_counts(wgpu::TextureFormat::Rgba8UnormSrgb);
        let samples = best_sample_count(&sample_counts, settings.msaa_samples);
        let mut viewport = ViewportRenderer::new(&gpu, samples);
        viewport.set_ray_tracing(&gpu, ray_tracing(settings.ray_tracing));
        let adapter_name = gpu.adapter_info().name;
        info!(samples, "viewport ready");

        let egui_ctx = egui::Context::default();
        theme::install_fonts(&egui_ctx);
        theme::apply_style(&egui_ctx, settings);
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            ViewportId::ROOT,
            &window,
            Some(scale_factor(&window)),
            Some(winit::window::Theme::Dark),
            Some(gpu.device().limits().max_texture_dimension_2d as usize),
        );
        let egui_renderer = egui_wgpu::Renderer::new(
            gpu.device(),
            gpu.surface_format(),
            egui_wgpu::RendererOptions::default(),
        );
        let small_logo = downscale(&logo_rgba, logo_size, 4);
        let logo_image = egui::ColorImage::from_rgba_unmultiplied(
            [(logo_size.0 / 4) as usize, (logo_size.1 / 4) as usize],
            &small_logo,
        );
        let logo = egui_ctx.load_texture("logo", logo_image, egui::TextureOptions::LINEAR);

        let editor = Editor::new(settings.default_voxel_size_cm / 100.0, settings.undo_steps as usize);
        let (save_sender, save_results) = mpsc::channel();
        let waker_proxy = proxy.clone();
        let mcp = McpHost::new(
            &settings.mcp,
            Arc::new(move || {
                // A closed event loop means the editor is exiting; the request will be refused anyway.
                let _ = waker_proxy.send_event(UserEvent::Automation);
            }),
        );
        let camera = OrbitCamera {
            field_of_view_degrees: settings.field_of_view_degrees,
            ..OrbitCamera::default()
        };
        let io_proxy = proxy.clone();
        let io = Io::new(Arc::new(move || {
            let _ = io_proxy.send_event(UserEvent::Io);
        }));
        let recent_files = recent::load();
        let mut ui = UiState::new(settings.show_start_screen && open_path.is_none());
        ui.recent = recent::check(recent_files.clone());
        ui.prompt = startup_recovery(&recent_files).map(hud::Prompt::Recovery);
        if let Some(path) = open_path {
            io.submit(files::IoJob::Load { path });
        }
        let doc = Document::new(editor.revision);
        Ok(Self {
            window,
            gpu,
            viewport,
            egui_ctx,
            egui_state,
            egui_renderer,
            logo,
            editor,
            camera,
            ui,
            i18n: I18n::new(settings.language),
            mesh: ChunkMesh::default(),
            preview: Preview::default(),
            sample_counts,
            adapter_name,
            next_frame: None,
            last_mode: Mode::Model,
            trace_revision: None,
            save_results,
            save_sender,
            mcp,
            io,
            doc,
            accepted_loss: None,
            recent: recent_files,
            splash_was_open: false,
            title: String::new(),
            platform,
        })
    }

    /// Reports the title strip, the controls inside it and the maximize button to the platform layer.
    fn update_platform(&self, glass: &[hud::Glass], pixels_per_point: f32) {
        let Some(platform) = &self.platform else {
            return;
        };
        let to_pixels = |rect: egui::Rect| rustium_platform::Rect {
            left: rect.min.x * pixels_per_point,
            top: rect.min.y * pixels_per_point,
            right: rect.max.x * pixels_per_point,
            bottom: rect.max.y * pixels_per_point,
        };
        let size = self.window.inner_size();
        let strip = hud::MARGIN + hud::TOP_BAR_HEIGHT;
        let regions = HitRegions {
            size: (size.width as f32, size.height as f32),
            resize_border: 6.0 * pixels_per_point,
            caption: Some(rustium_platform::Rect {
                left: 0.0,
                top: 0.0,
                right: size.width as f32,
                bottom: strip * pixels_per_point,
            }),
            controls: glass
                .iter()
                .map(|glass| glass.rect)
                .filter(|rect| rect.min.y < strip)
                .map(to_pixels)
                .collect(),
            maximize_button: self.ui.maximize_button.map(to_pixels),
        };
        platform.update(&regions);
    }

    /// Runs the interface, applies its commands and draws one frame. Returns `true` to quit.
    fn redraw(&mut self, settings: &mut Settings) -> Result<bool> {
        self.update_title();
        self.refresh_recent_on_splash();
        if self.handle_io(settings) {
            return Ok(true);
        }
        let raw_input = self.egui_state.take_egui_input(&self.window);
        let ctx = self.egui_ctx.clone();
        let maximized = self.window.is_maximized();
        let mut glass = Vec::new();
        let mut commands = Vec::new();
        let mut output = ctx.run_pass(raw_input, |ctx| {
            let mut hud = Hud {
                editor: &mut self.editor,
                camera: &mut self.camera,
                settings,
                ui: &mut self.ui,
                i18n: &self.i18n,
                logo: &self.logo,
                maximized,
                adapter_name: &self.adapter_name,
                trace_engine: self.viewport.trace_engine(),
                ray_query_supported: self.gpu.supports_ray_query(),
                sample_counts: &self.sample_counts,
                mcp_status: self.mcp.status(),
                material_status: self.viewport.material_status(),
                glass: Vec::new(),
                commands: Vec::new(),
            };
            hud::draw(ctx, &mut hud);
            splash::show(ctx, &mut hud);
            settings_window::show(ctx, &mut hud);
            glass = std::mem::take(&mut hud.glass);
            commands = std::mem::take(&mut hud.commands);
        });
        self.egui_state
            .handle_platform_output(&self.window, output.platform_output);

        for command in commands {
            if self.run_command(command, settings) {
                return Ok(true);
            }
        }
        self.upload_dirty_chunks();
        self.sync_render();

        let repaint_delay = output
            .viewport_output
            .get(&ViewportId::ROOT)
            .map_or(Duration::MAX, |viewport| viewport.repaint_delay);
        let compiling = *self.viewport.material_status() == rustium_render::CompileStatus::Compiling;
        let settling = self.ui.mode != Mode::Render && self.viewport.is_settling();
        let repaint_delay =
            if compiling || settling || (self.ui.mode == Mode::Render && self.ui.render.is_accumulating()) {
                Duration::ZERO
            } else if self.ui.render.job.is_some() {
                repaint_delay.min(Duration::from_millis(200))
            } else {
                repaint_delay
            };
        self.next_frame = Instant::now().checked_add(repaint_delay.max(frame_interval(settings)));

        let pixels_per_point = output.pixels_per_point;
        let paint_jobs = self
            .egui_ctx
            .tessellate(std::mem::take(&mut output.shapes), pixels_per_point);
        // Texture uploads must happen every pass, even when this frame is skipped.
        let mut textures = std::mem::take(&mut output.textures_delta);
        for (id, deltas) in textures.set.drain() {
            for delta in &deltas {
                self.egui_renderer
                    .update_texture(self.gpu.device(), self.gpu.queue(), id, delta);
            }
        }
        let freed: Vec<egui::TextureId> = textures.free.drain().collect();
        textures.clear();
        let Some(frame) = self.gpu.acquire_frame().wrap_err("cannot acquire a frame")? else {
            for id in &freed {
                self.egui_renderer.free_texture(id);
            }
            return Ok(false);
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let srgb_view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.gpu.surface_srgb_format()),
            ..Default::default()
        });
        let mut encoder = self
            .gpu
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame encoder"),
            });

        self.update_platform(&glass, pixels_per_point);

        let lines = if self.ui.mode == Mode::Effects {
            Vec::new()
        } else {
            self.overlay_lines(settings)
        };
        let glass_rects: Vec<GlassRect> = glass
            .iter()
            .map(|glass| GlassRect {
                min: (glass.rect.min.to_vec2() * pixels_per_point).into(),
                max: (glass.rect.max.to_vec2() * pixels_per_point).into(),
                radius: glass.radius * pixels_per_point,
            })
            .collect();
        let background = settings
            .background
            .map(|channel| (f32::from(channel) / 255.0).powf(2.2));
        let effects_view = (self.ui.mode == Mode::Effects).then(|| self.effects_view(pixels_per_point));
        let models = match &effects_view {
            Some(_) if self.ui.effects.preview != PreviewShape::Model => Preview::draws(),
            _ => self.model_draws(),
        };
        let frame_input = ViewportFrame {
            models: &models,
            camera: effects_view.unwrap_or_else(|| self.camera_uniform()),
            background,
            show_grid: settings.show_grid && self.ui.mode != Mode::Effects,
            lines: &lines,
            glass: &glass_rects,
            blur: settings.translucent_panels,
            trace: self.trace_frame(pixels_per_point),
        };
        self.viewport.render(&self.gpu, &mut encoder, &srgb_view, &frame_input);
        if frame_input.trace.is_some() {
            self.ui
                .render
                .update_progress(self.viewport.trace_progress(), Instant::now());
        }

        let (width, height) = self.gpu.size();
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [width, height],
            pixels_per_point,
        };
        let user_buffers =
            self.egui_renderer
                .update_buffers(self.gpu.device(), self.gpu.queue(), &mut encoder, &paint_jobs, &screen);
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("interface pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                })
                .forget_lifetime();
            self.egui_renderer.render(&mut pass, &paint_jobs, &screen);
        }
        self.gpu
            .queue()
            .submit(user_buffers.into_iter().chain(std::iter::once(encoder.finish())));
        self.gpu.queue().present(frame);
        for id in &freed {
            self.egui_renderer.free_texture(id);
        }
        Ok(false)
    }

    /// Applies one HUD command; returns `true` when the app should quit.
    fn run_command(&mut self, command: AppCommand, settings: &mut Settings) -> bool {
        match command {
            AppCommand::Quit => return self.request(Pending::Quit, settings),
            AppCommand::NewScene => return self.request(Pending::New, settings),
            AppCommand::Open(path) => return self.request(Pending::Open(path), settings),
            AppCommand::ImportVox => return self.request(Pending::ImportVox, settings),
            AppCommand::Save => self.save(None, false),
            AppCommand::SaveAs => self.save(None, true),
            AppCommand::ExportVox => self.export_vox(),
            AppCommand::ImportPalette => self.io.pick_import_palette(self.i18n.tr("dialog-import-palette")),
            AppCommand::ExportPalette => self.export_palette(),
            AppCommand::PromptSave | AppCommand::RecoveryRestore => {
                return self.answer_prompt(PromptAnswer::Primary, settings);
            }
            AppCommand::PromptDiscard | AppCommand::RecoveryDiscard => {
                return self.answer_prompt(PromptAnswer::Secondary, settings);
            }
            AppCommand::PromptCancel => return self.answer_prompt(PromptAnswer::Cancel, settings),
            AppCommand::Minimize => self.window.set_minimized(true),
            AppCommand::ToggleMaximize => self.window.set_maximized(!self.window.is_maximized()),
            AppCommand::DragWindow => {
                if let Err(error) = self.window.drag_window() {
                    tracing::warn!(%error, "cannot drag the window");
                }
            }
            AppCommand::DragResize(direction) => {
                if let Err(error) = self.window.drag_resize_window(direction) {
                    tracing::warn!(%error, "cannot resize the window");
                }
            }
            AppCommand::FrameModel => self.camera.frame(self.editor.scene.world_volume(self.editor.focus)),
            AppCommand::HistoryJump(target) => {
                while self.editor.history.done().count() > target && self.editor.history.can_undo() {
                    self.editor.undo();
                }
                while self.editor.history.done().count() < target && self.editor.history.can_redo() {
                    self.editor.redo();
                }
            }
            AppCommand::SettingsChanged => self.apply_settings(settings),
            AppCommand::SaveImage => {
                if self.ui.render.format == ImageFormat::Mp4 {
                    self.start_job(JobKind::Turntable);
                } else {
                    self.save_image();
                }
            }
            AppCommand::QueueShots => self.start_job(JobKind::Queue),
            AppCommand::CancelJob => {
                if let Some(job) = &self.ui.render.job {
                    job.cancel();
                }
            }
            AppCommand::RestartRender => self.viewport.restart_trace(),
            AppCommand::RunScript => self.run_script_file(),
        }
        false
    }

    fn apply_settings(&mut self, settings: &Settings) {
        theme::apply_style(&self.egui_ctx, settings);
        self.i18n.set_language(settings.language);
        self.gpu.set_vsync(settings.vsync);
        let samples = best_sample_count(&self.sample_counts, settings.msaa_samples);
        self.viewport.set_sample_count(&self.gpu, samples);
        if self
            .viewport
            .set_ray_tracing(&self.gpu, ray_tracing(settings.ray_tracing))
        {
            // The new tracer holds no Scene yet.
            self.trace_revision = None;
        }
        self.camera.field_of_view_degrees = settings.field_of_view_degrees;
        self.editor.history.set_limit(settings.undo_steps as usize);
        self.mcp.apply(&settings.mcp);
        settings.save();
    }

    /// Main menu → Run script: pick a `.luau` file, run it on the Focus Model as one History step and report.
    fn run_script_file(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title(self.i18n.tr("menu-run-script"))
            .add_filter("Luau", &["luau", "lua"])
            .pick_file()
        else {
            return;
        };
        let result = std::fs::read_to_string(&path)
            .map_err(|error| error.to_string())
            .and_then(|source| {
                let model = self.editor.focus;
                self.editor
                    .run_script(model, &source, &rustium_script::Limits::default())
                    .map_err(|error| error.to_string())
            });
        let toast = match result {
            Ok(run) => {
                info!(path = %path.display(), changed = run.changed, "script finished");
                let mut text = self
                    .i18n
                    .tr(if run.changed { "script-done" } else { "script-no-change" });
                for line in run.log.iter().take(6) {
                    text.push('\n');
                    text.push_str(line);
                }
                Toast::new(text, false)
            }
            Err(error) => {
                warn!(path = %path.display(), %error, "script failed");
                let text = self
                    .i18n
                    .tr_args("script-failed", &[("error", FluentValue::from(error))]);
                Toast::new(text, true)
            }
        };
        self.ui.toast = Some(toast);
    }

    fn upload_dirty_chunks(&mut self) {
        let placements = self.editor.scene.placements();
        let (reset, chunks) = self.editor.take_dirty(&placements);
        if reset {
            self.viewport.clear_chunks();
            self.preview.invalidate();
        }
        for (shape_id, coord) in chunks {
            let Some(shape) = self.editor.scene.shape(shape_id) else {
                continue;
            };
            mesh_chunk(shape, coord, &mut self.mesh);
            self.viewport.upload_chunk(&self.gpu, shape_id, coord, &self.mesh);
        }
    }

    /// Keeps the GPU copies the renderers read in step with the Editor and the mode.
    fn sync_render(&mut self) {
        // A Cube or Ball preview points one Palette Color at the edited Material in the GPU copy only.
        let wanted = (self.ui.mode == Mode::Effects && self.ui.effects.preview != PreviewShape::Model)
            .then_some(self.ui.effects.graph);
        let mut dirty = self.editor.take_palette_dirty();
        if wanted != self.preview.applied_graph {
            self.preview.applied_graph = wanted;
            dirty = true;
        }
        if dirty {
            let scratch = wanted.map(|graph| Preview::scene_with_material(&self.editor.scene, graph));
            let scene = scratch.as_ref().unwrap_or(&self.editor.scene);
            self.viewport
                .set_materials(&self.gpu, scene.palette(), self.editor.scene.material_graphs());
        }
        if wanted.is_some() {
            let shape = self.ui.effects.preview;
            // Uploading marks the image as edited (and restarts temporal accumulation), so only do it on a change.
            if let Some((id, meshes)) = self.preview.take_upload(shape, &self.editor.scene) {
                for (coord, mesh) in meshes {
                    self.viewport.upload_chunk(&self.gpu, id, *coord, mesh);
                }
            }
        } else {
            self.preview.invalidate();
        }
        self.viewport.poll_materials(&self.gpu);
        let mode = self.ui.mode;
        if mode == Mode::Render {
            if self.last_mode != Mode::Render || self.trace_revision != Some(self.editor.revision) {
                self.viewport.upload_trace_scene(&self.gpu, &self.editor.scene);
                self.trace_revision = Some(self.editor.revision);
            }
            self.ui.render.sync_active_shot(&self.camera);
            self.poll_saves();
            self.poll_job();
        }
        self.last_mode = mode;
    }

    /// The path tracer's input for this frame, or `None` outside Render mode.
    fn trace_frame(&self, pixels_per_point: f32) -> Option<TraceFrame> {
        if self.ui.mode == Mode::Render {
            self.ui
                .render
                .trace_frame(&self.camera, pixels_per_point, self.editor.scene.voxel_size())
        } else {
            None
        }
    }

    /// Reads the traced image back and hands it to the save dialog thread.
    fn save_image(&mut self) {
        match self.viewport.read_trace_image(&self.gpu) {
            Ok(image) => {
                let render = &self.ui.render;
                render_output::save_with_dialog(
                    image,
                    render.format,
                    render.backdrop == Backdrop::Transparent,
                    render.exposure(),
                    self.save_sender.clone(),
                );
            }
            Err(error) => {
                error!(%error, "cannot read the image back");
                self.ui.render.message = Some(self.i18n.tr("save-failed-readback"));
            }
        }
    }

    fn poll_saves(&mut self) {
        while let Ok(outcome) = self.save_results.try_recv() {
            self.report(outcome);
        }
    }

    /// Starts a turntable or queue job on a worker thread, unless one is already running.
    fn start_job(&mut self, kind: JobKind) {
        if self.ui.render.job.is_some() {
            return;
        }
        let render = &self.ui.render;
        let voxel_size = self.editor.scene.voxel_size();
        let still_format = if render.format == ImageFormat::Exr {
            ImageFormat::Exr
        } else {
            ImageFormat::Png
        };
        let outputs = match kind {
            JobKind::Turntable => vec![Output {
                name: "turntable".to_owned(),
                kind: OutputKind::Video,
                frames: render.turntable_frames(&self.camera, render.lens_mm, render.size, voxel_size),
            }],
            JobKind::Queue => render
                .shots
                .iter()
                .enumerate()
                .map(|(index, shot)| {
                    let name = format!("shot-{}", index + 1);
                    if render.format == ImageFormat::Mp4 {
                        let frames = render.turntable_frames(&shot.camera, shot.lens_mm, shot.size, voxel_size);
                        Output {
                            name,
                            kind: OutputKind::Video,
                            frames,
                        }
                    } else {
                        let frame = render.frame_for(&shot.camera, shot.lens_mm, shot.size, voxel_size);
                        Output {
                            name,
                            kind: OutputKind::Still(still_format),
                            frames: vec![frame],
                        }
                    }
                })
                .collect(),
        };
        let spec = JobSpec {
            outputs,
            destination: match kind {
                JobKind::Turntable => Destination::File,
                JobKind::Queue => Destination::Folder,
            },
            exposure: render.exposure(),
            transparent: render.backdrop == Backdrop::Transparent,
        };
        let renderer = self.viewport.offline_renderer(&self.gpu);
        let scene = self.editor.scene.clone();
        self.ui.render.message = None;
        self.ui.render.job = Some(render_job::spawn(spec, renderer, scene));
    }

    /// Picks up the result of the running render job.
    fn poll_job(&mut self) {
        let Some(outcome) = self.ui.render.job.as_ref().and_then(render_job::JobHandle::poll) else {
            return;
        };
        self.ui.render.job = None;
        self.report(outcome);
    }

    /// Shows the result of a save or job under the Output tab, and as a toast for the cases that need attention.
    fn report(&mut self, outcome: SaveOutcome) {
        let (message, warning) = match outcome {
            SaveOutcome::Saved(path) => (
                Some(
                    self.i18n
                        .tr_args("save-done", &[("path", FluentValue::from(path.display().to_string()))]),
                ),
                false,
            ),
            SaveOutcome::Batch { folder, count } => (
                Some(self.i18n.tr_args(
                    "queue-done",
                    &[
                        ("count", FluentValue::from(count)),
                        ("path", FluentValue::from(folder.display().to_string())),
                    ],
                )),
                false,
            ),
            SaveOutcome::Sequence(directory) => (
                Some(self.i18n.tr_args(
                    "video-no-ffmpeg",
                    &[("path", FluentValue::from(directory.display().to_string()))],
                )),
                true,
            ),
            SaveOutcome::Cancelled => (None, false),
            SaveOutcome::Failed(reason) => (
                Some(
                    self.i18n
                        .tr_args("save-failed", &[("reason", FluentValue::from(reason))]),
                ),
                true,
            ),
        };
        if warning && let Some(text) = &message {
            self.ui.toast = Some(Toast::new(text.clone(), true));
        }
        self.ui.render.message = message;
    }

    /// The Models to draw; everything outside the Focus is dimmed.
    fn model_draws(&self) -> Vec<ModelDraw> {
        let scene = &self.editor.scene;
        let focus = self.editor.focus;
        scene
            .placements()
            .into_iter()
            .map(|placement| ModelDraw {
                dim: if scene.is_ancestor_or_self(focus, placement.node) {
                    0.0
                } else {
                    1.0
                },
                placement,
            })
            .collect()
    }

    /// Camera of the Effects preview: the Scene or the Cube and Ball squeezed into the preview rectangle.
    fn effects_view(&self, pixels_per_point: f32) -> CameraUniform {
        let (width, height) = self.gpu.size();
        let screen = egui::vec2(width as f32 / pixels_per_point, height as f32 / pixels_per_point);
        let rect = hud::preview_view_rect(egui::Rect::from_min_size(egui::Pos2::ZERO, screen));
        let camera = effects_preview::camera_for(self.ui.effects.preview, &self.editor.scene);
        effects_preview::uniform(&camera, rect, screen)
    }

    fn camera_uniform(&self) -> CameraUniform {
        let (width, height) = self.gpu.size();
        let aspect = width as f32 / height.max(1) as f32;
        let eye = self.camera.eye();
        let major = (1.0 / self.editor.scene.voxel_size()).round().max(1.0);
        let extent = (self.camera.distance * 4.0).max(64.0);
        let centre = (self.camera.target / major).round() * major;
        CameraUniform {
            view_proj: self.camera.view_proj(aspect).to_cols_array_2d(),
            eye: eye.extend(1.0).to_array(),
            sun: Vec3::new(0.45, 0.8, 0.35).normalize().extend(0.0).to_array(),
            grid: [major, extent, centre.x, centre.z],
        }
    }

    fn overlay_lines(&self, settings: &Settings) -> Vec<LineVertex> {
        let editor = &self.editor;
        let mut lines = Vec::new();
        let linear = |rgb: [u8; 3]| rgb.map(|channel| (f32::from(channel) / 255.0).powf(2.2));
        let [r, g, b] = linear(settings.accent.rgb());
        if settings.show_volume_bounds
            && let Some(volume) = editor.scene.world_volume(editor.focus)
        {
            push_box(&mut lines, volume, 0.0, [1.0, 1.0, 1.0, 0.22]);
        }
        for volume in editor.selection_boxes() {
            push_box(&mut lines, volume, 0.02, [1.0, 1.0, 1.0, 0.9]);
        }
        for quad in editor.mirror_planes() {
            for index in 0..4 {
                for point in [quad[index], quad[(index + 1) % 4]] {
                    lines.push(LineVertex {
                        position: point.to_array(),
                        color: [r, g, b, 0.55],
                    });
                }
            }
        }
        if let Some((start, end, _)) = editor.distance() {
            let centre = |cell: IVec3| cell.as_vec3() + Vec3::splat(0.5);
            push_box(&mut lines, Volume::point(start), 0.02, [r, g, b, 1.0]);
            push_box(&mut lines, Volume::point(end), 0.02, [r, g, b, 1.0]);
            for point in [centre(start), centre(end)] {
                lines.push(LineVertex {
                    position: point.to_array(),
                    color: [r, g, b, 1.0],
                });
            }
        }
        if let Some(bounds) = editor.hover_bounds() {
            push_box(&mut lines, bounds, 0.01, [r, g, b, 1.0]);
        }
        lines
    }
}

/// A recovery copy left by a session that did not end cleanly: the unsaved Scene's, else the newest file's.
fn startup_recovery(recent: &[RecentFile]) -> Option<files::RecoveryOffer> {
    files::find_recovery(None).or_else(|| recent.iter().find_map(|file| files::find_recovery(Some(&file.path))))
}

fn push_box(lines: &mut Vec<LineVertex>, volume: Volume, inflate: f32, color: [f32; 4]) {
    let min = volume.min.as_vec3() - inflate;
    let max = (volume.max + IVec3::ONE).as_vec3() + inflate;
    let corner = |index: usize| {
        Vec3::new(
            if index & 1 == 0 { min.x } else { max.x },
            if index & 2 == 0 { min.y } else { max.y },
            if index & 4 == 0 { min.z } else { max.z },
        )
    };
    for (a, b) in [
        (0, 1),
        (2, 3),
        (4, 5),
        (6, 7),
        (0, 2),
        (1, 3),
        (4, 6),
        (5, 7),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ] {
        lines.push(LineVertex {
            position: corner(a).to_array(),
            color,
        });
        lines.push(LineVertex {
            position: corner(b).to_array(),
            color,
        });
    }
}

/// Minimum time between frames when vertical sync is off and a frame limit is set.
fn frame_interval(settings: &Settings) -> Duration {
    if settings.vsync {
        return Duration::ZERO;
    }
    match settings.frame_limit {
        FrameLimit::Fps60 => Duration::from_micros(16_667),
        FrameLimit::Fps144 => Duration::from_micros(6_944),
        FrameLimit::MatchDisplay | FrameLimit::Unlimited => Duration::ZERO,
    }
}

/// The renderer's name for the Settings choice.
fn ray_tracing(mode: RayTracingMode) -> RayTracing {
    match mode {
        RayTracingMode::Automatic => RayTracing::Auto,
        RayTracingMode::Software => RayTracing::Software,
        RayTracingMode::Hardware => RayTracing::Hardware,
    }
}

fn best_sample_count(supported: &[u32], requested: u32) -> u32 {
    supported
        .iter()
        .copied()
        .filter(|count| *count <= requested)
        .max()
        .unwrap_or(1)
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "display scale factors are small numbers that fit in f32"
)]
fn scale_factor(window: &Window) -> f32 {
    window.scale_factor() as f32
}

fn decode_png(bytes: &[u8]) -> Result<(Vec<u8>, (u32, u32))> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
    let mut reader = decoder.read_info()?;
    let mut buffer = vec![0; reader.output_buffer_size().ok_or_else(|| eyre!("logo is too large"))?];
    let info = reader.next_frame(&mut buffer)?;
    buffer.truncate(info.buffer_size());
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err(eyre!(
            "logo must be 8-bit RGBA, found {:?} {:?}",
            info.color_type,
            info.bit_depth
        ));
    }
    Ok((buffer, (info.width, info.height)))
}

/// Box-filters an RGBA image by `factor` so the small logo does not shimmer.
fn downscale(rgba: &[u8], size: (u32, u32), factor: u32) -> Vec<u8> {
    let (width, height) = (size.0 / factor, size.1 / factor);
    let mut out = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let mut sum = [0_u32; 4];
            for dy in 0..factor {
                for dx in 0..factor {
                    let index = (((y * factor + dy) * size.0 + x * factor + dx) * 4) as usize;
                    for (channel, total) in sum.iter_mut().enumerate() {
                        *total += u32::from(rgba.get(index + channel).copied().unwrap_or(0));
                    }
                }
            }
            let samples = factor * factor;
            out.extend(sum.iter().map(|total| u8::try_from(total / samples).unwrap_or(u8::MAX)));
        }
    }
    out
}

use std::sync::Arc;
use std::time::{Duration, Instant};

use color_eyre::eyre::{Result, WrapErr, eyre};
use egui::ViewportId;
use glam::{IVec3, Vec3};
use rustium_core::{ChunkMesh, Volume, mesh_chunk};
use rustium_render::{
    CameraUniform, GlassRect, GpuContext, GpuPreferences, GraphicsBackend, LineVertex, ViewportFrame, ViewportRenderer,
};
use tracing::{error, info};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::window::{Icon, Window, WindowId};

use crate::camera::OrbitCamera;
use crate::editor::Editor;
use crate::hud::{self, AppCommand, Hud, UiState};
use crate::i18n::I18n;
use crate::settings::{Backend, FrameLimit, Settings};
use crate::{settings_window, splash, theme};

const LOGO_PNG: &[u8] = include_bytes!("../assets/logo.png");

/// The winit application: owns the settings and, once resumed, the running editor.
pub(crate) struct App {
    settings: Settings,
    running: Option<Running>,
    error: Option<color_eyre::Report>,
}

impl App {
    pub(crate) fn new(settings: Settings) -> Self {
        Self {
            settings,
            running: None,
            error: None,
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
    sample_counts: Vec<u32>,
    adapter_name: String,
    next_frame: Option<Instant>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.running.is_some() {
            return;
        }
        match Running::new(event_loop, &self.settings) {
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
            WindowEvent::CloseRequested => event_loop.exit(),
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

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(running) = &self.running else {
            return;
        };
        match running.next_frame {
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
    fn new(event_loop: &ActiveEventLoop, settings: &Settings) -> Result<Self> {
        let (logo_rgba, logo_size) = decode_png(LOGO_PNG).wrap_err("cannot decode the logo")?;
        let mut attributes = Window::default_attributes()
            .with_title("Rustium")
            .with_decorations(false)
            .with_inner_size(LogicalSize::new(1440.0, 900.0))
            .with_min_inner_size(LogicalSize::new(960.0, 640.0))
            .with_window_icon(Icon::from_rgba(logo_rgba.clone(), logo_size.0, logo_size.1).ok());
        #[cfg(windows)]
        {
            use winit::platform::windows::WindowAttributesExtWindows;
            attributes = attributes.with_undecorated_shadow(true);
        }
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .wrap_err("cannot create the window")?,
        );
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
        let viewport = ViewportRenderer::new(&gpu, samples);
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
        viewport.set_palette(&gpu, editor.scene.palette());
        let camera = OrbitCamera {
            field_of_view_degrees: settings.field_of_view_degrees,
            ..OrbitCamera::default()
        };
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
            ui: UiState::new(settings.show_start_screen),
            i18n: I18n::new(settings.language),
            mesh: ChunkMesh::default(),
            sample_counts,
            adapter_name,
            next_frame: None,
        })
    }

    /// Runs the interface, applies its commands and draws one frame. Returns `true` to quit.
    fn redraw(&mut self, settings: &mut Settings) -> Result<bool> {
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
                sample_counts: &self.sample_counts,
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

        let repaint_delay = output
            .viewport_output
            .get(&ViewportId::ROOT)
            .map_or(Duration::MAX, |viewport| viewport.repaint_delay);
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

        let lines = self.overlay_lines(settings);
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
        let frame_input = ViewportFrame {
            camera: self.camera_uniform(),
            background,
            show_grid: settings.show_grid,
            lines: &lines,
            glass: &glass_rects,
            blur: settings.translucent_panels,
        };
        self.viewport.render(&self.gpu, &mut encoder, &srgb_view, &frame_input);

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
            AppCommand::Quit => return true,
            AppCommand::NewScene => {
                self.editor
                    .new_scene(settings.default_voxel_size_cm / 100.0, settings.undo_steps as usize);
                self.viewport.clear_chunks();
                self.viewport.set_palette(&self.gpu, self.editor.scene.palette());
                self.camera.frame(None);
            }
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
            AppCommand::FrameModel => self.camera.frame(self.editor.scene.shape().volume()),
            AppCommand::HistoryJump(target) => {
                while self.editor.history.done().count() > target && self.editor.history.can_undo() {
                    self.editor.undo();
                }
                while self.editor.history.done().count() < target && self.editor.history.can_redo() {
                    self.editor.redo();
                }
            }
            AppCommand::SettingsChanged => self.apply_settings(settings),
        }
        false
    }

    fn apply_settings(&mut self, settings: &Settings) {
        theme::apply_style(&self.egui_ctx, settings);
        self.i18n.set_language(settings.language);
        self.gpu.set_vsync(settings.vsync);
        let samples = best_sample_count(&self.sample_counts, settings.msaa_samples);
        self.viewport.set_sample_count(&self.gpu, samples);
        self.camera.field_of_view_degrees = settings.field_of_view_degrees;
        self.editor.history.set_limit(settings.undo_steps as usize);
        settings.save();
    }

    fn upload_dirty_chunks(&mut self) {
        let (reset, chunks) = self.editor.take_dirty();
        if reset {
            self.viewport.clear_chunks();
        }
        for coord in chunks {
            mesh_chunk(self.editor.scene.shape(), coord, &mut self.mesh);
            self.viewport.upload_chunk(&self.gpu, coord, &self.mesh);
        }
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
        let mut lines = Vec::new();
        if settings.show_volume_bounds
            && let Some(volume) = self.editor.scene.shape().volume()
        {
            push_box(&mut lines, volume, 0.0, [1.0, 1.0, 1.0, 0.22]);
        }
        if let Some(bounds) = self.editor.hover_bounds() {
            let [r, g, b] = settings
                .accent
                .rgb()
                .map(|channel| (f32::from(channel) / 255.0).powf(2.2));
            push_box(&mut lines, bounds, 0.01, [r, g, b, 1.0]);
        }
        lines
    }
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

//! Rebuilding the pipelines that run `evaluate_material` without stalling the frame.
//!
//! A change to a Material's shape produces new WGSL. The new raster and path tracer pipelines are built on a worker
//! thread (`wgpu::Device` is shareable; the driver's shader compile is the slow part) and swapped in between frames
//! when they are ready. The old pipelines keep drawing meanwhile, and a shader that fails to compile leaves them in
//! place and is reported through [`CompileStatus::Error`]. A change to a number only rewrites the parameter buffer.

use std::sync::mpsc;
use std::thread;

use bytemuck::Zeroable;
use wgpu::naga;

use crate::material_compiler::{CompileError, CompiledMaterials, assemble};
use crate::materials::{GpuMaterialTable, MaterialBuffers};
use crate::path_tracer::{TraceEngine, compute_shader_source, create_compute_pipeline};
use crate::viewport_renderer::{create_raster_pair, raster_shader_source};

/// How far the Material shaders are from matching the Scene's Material graphs (the "Compiled" chip).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompileStatus {
    /// The pipelines run exactly the Scene's graphs.
    Compiled,
    /// New pipelines are being built; the previous ones still draw.
    Compiling,
    /// The last change did not compile; the previous pipelines still draw. Holds the compiler's message.
    Error(String),
}

/// What a finished build delivers.
pub(crate) struct Built {
    pub(crate) raster: wgpu::RenderPipeline,
    pub(crate) raster_mirrored: wgpu::RenderPipeline,
    pub(crate) compute: wgpu::ComputePipeline,
}

/// Everything a worker needs to build pipelines (all cheap handles).
#[derive(Debug, Clone)]
pub(crate) struct BuildContext {
    pub(crate) device: wgpu::Device,
    pub(crate) raster_layout: wgpu::PipelineLayout,
    pub(crate) compute_layout: wgpu::PipelineLayout,
    pub(crate) sample_count: u32,
    /// The traversal the compute pipeline is composed with.
    pub(crate) engine: TraceEngine,
}

struct Message {
    generation: u64,
    result: Result<Built, String>,
}

#[derive(Debug)]
struct Pending {
    generation: u64,
    generated: String,
    params: Vec<f32>,
    /// The color table that goes with `generated`: its Material indices only make sense for that shader.
    table: GpuMaterialTable,
}

/// Tracks which generated source the live pipelines run and builds replacements in the background.
#[derive(Debug)]
pub(crate) struct MaterialCompiler {
    buffers: MaterialBuffers,
    live: String,
    /// The color table the live shader runs with.
    live_table: GpuMaterialTable,
    /// Source that failed to build, so changing only numbers does not build it again.
    failed: Option<String>,
    status: CompileStatus,
    pending: Option<Pending>,
    sender: mpsc::Sender<Message>,
    receiver: mpsc::Receiver<Message>,
    next_generation: u64,
}

impl MaterialCompiler {
    /// `live` is the generated source the initial pipelines were built from.
    pub(crate) fn new(buffers: MaterialBuffers, live: String) -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            buffers,
            live,
            live_table: GpuMaterialTable::zeroed(),
            failed: None,
            status: CompileStatus::Compiled,
            pending: None,
            sender,
            receiver,
            next_generation: 0,
        }
    }

    pub(crate) fn status(&self) -> &CompileStatus {
        &self.status
    }

    /// The generated source of the pipelines that are drawing now.
    pub(crate) fn live_source(&self) -> String {
        assemble(&self.live)
    }

    /// Takes the newly compiled graphs: rewrites the parameter buffer when the shader is unchanged, starts a build
    /// when it is not.
    pub(crate) fn request(
        &mut self,
        queue: &wgpu::Queue,
        context: &BuildContext,
        compiled: Result<CompiledMaterials, CompileError>,
        table: &GpuMaterialTable,
    ) {
        // Until a new shader is live the table keeps pointing at the Materials the live shader knows, so Colors can
        // change at once without any of them landing on the wrong Material.
        let interim = (*table).with_graphs_of(&self.live_table);
        let compiled = match compiled {
            Ok(compiled) => compiled,
            Err(error) => {
                self.pending = None;
                self.status = CompileStatus::Error(error.to_string());
                self.buffers.write_table(queue, &interim);
                return;
            }
        };
        if compiled.generated == self.live {
            self.pending = None;
            self.failed = None;
            self.buffers.write_params(queue, &compiled.params);
            self.live_table = *table;
            self.buffers.write_table(queue, table);
            self.status = CompileStatus::Compiled;
            return;
        }
        self.buffers.write_table(queue, &interim);
        if let Some(pending) = self.pending.as_mut().filter(|p| p.generated == compiled.generated) {
            pending.params = compiled.params;
            pending.table = *table;
            self.status = CompileStatus::Compiling;
        } else if self.failed.as_deref() == Some(compiled.generated.as_str()) {
            // This exact source already failed; only its numbers changed, so building it again fails again.
            self.pending = None;
        } else {
            self.start(context, compiled.generated, compiled.params, table);
        }
    }

    pub(crate) fn restart_pending(&mut self, context: &BuildContext) {
        if let Some(pending) = self.pending.take() {
            self.start(context, pending.generated, pending.params, &pending.table);
        }
    }

    fn start(&mut self, context: &BuildContext, generated: String, params: Vec<f32>, table: &GpuMaterialTable) {
        self.next_generation += 1;
        let generation = self.next_generation;
        let source = assemble(&generated);
        self.pending = Some(Pending {
            generation,
            generated,
            params,
            table: *table,
        });
        self.failed = None;
        self.status = CompileStatus::Compiling;
        let context = context.clone();
        let sender = self.sender.clone();
        let spawned = thread::Builder::new()
            .name("material-compile".to_owned())
            .spawn(move || {
                let result = build(&context, &source);
                let _ = sender.send(Message { generation, result });
            });
        if let Err(error) = spawned {
            self.pending = None;
            self.status = CompileStatus::Error(format!("cannot start the shader compiler thread: {error}"));
        }
    }

    /// Collects a finished build. When one is ready its constants are uploaded and the pipelines are returned for
    /// the caller to swap in at once, so shader and constants always change together.
    pub(crate) fn poll(&mut self, queue: &wgpu::Queue) -> Option<Built> {
        let mut built = None;
        while let Ok(message) = self.receiver.try_recv() {
            if self
                .pending
                .as_ref()
                .is_none_or(|pending| pending.generation != message.generation)
            {
                continue; // superseded while it was building
            }
            let Some(pending) = self.pending.take() else { continue };
            match message.result {
                Ok(pipelines) => {
                    self.buffers.write_params(queue, &pending.params);
                    self.buffers.write_table(queue, &pending.table);
                    self.live_table = pending.table;
                    self.live = pending.generated;
                    self.failed = None;
                    self.status = CompileStatus::Compiled;
                    built = Some(pipelines);
                }
                Err(error) => {
                    self.failed = Some(pending.generated);
                    self.status = CompileStatus::Error(error);
                }
            }
        }
        built
    }
}

/// Validates the shaders with naga (for readable messages), then builds the pipelines inside an error scope.
fn build(context: &BuildContext, material_source: &str) -> Result<Built, String> {
    validate(
        "raster",
        &raster_shader_source(material_source),
        naga::valid::Capabilities::default(),
    )?;
    let capabilities = match context.engine {
        TraceEngine::Software => naga::valid::Capabilities::default(),
        TraceEngine::Hardware => naga::valid::Capabilities::default() | naga::valid::Capabilities::RAY_QUERY,
    };
    validate(
        "path tracer",
        &compute_shader_source(material_source, context.engine),
        capabilities,
    )?;
    let scope = context.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let (raster, raster_mirrored) = create_raster_pair(
        &context.device,
        &context.raster_layout,
        context.sample_count,
        material_source,
    );
    let compute = create_compute_pipeline(
        &context.device,
        &context.compute_layout,
        material_source,
        context.engine,
    );
    if let Some(error) = pollster::block_on(scope.pop()) {
        return Err(error.to_string());
    }
    Ok(Built {
        raster,
        raster_mirrored,
        compute,
    })
}

/// Parses and validates `source`; the error is the compiler's message with the offending line.
pub(crate) fn validate(name: &str, source: &str, capabilities: naga::valid::Capabilities) -> Result<(), String> {
    let module =
        naga::front::wgsl::parse_str(source).map_err(|error| format!("{name}: {}", error.emit_to_string(source)))?;
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), capabilities)
        .validate(&module)
        .map_err(|error| format!("{name}: {}", error.emit_to_string(source)))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use rustium_core::{MaterialGraph, SampleMaterial};

    use super::*;
    use crate::gpu_context::GraphicsBackend;
    use crate::material_compiler::{compile_materials, default_compiled};
    use crate::path_tracer::{PathTracer, RayTracing};
    use crate::viewport_renderer::create_scene_layout;

    struct Harness {
        device: wgpu::Device,
        queue: wgpu::Queue,
        compiler: MaterialCompiler,
        context: BuildContext,
    }

    fn harness() -> Option<Harness> {
        // The backends the app opens, so a machine with only DirectX 12 skips instead of failing.
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
        descriptor.backends = GraphicsBackend::Automatic.backends();
        let instance = wgpu::Instance::new(descriptor);
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
        let buffers = MaterialBuffers::new(&device);
        let tracer = PathTracer::new(
            &device,
            &buffers,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            RayTracing::Software,
        );
        let scene_layout = create_scene_layout(&device);
        let raster_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("test raster layout"),
            bind_group_layouts: &[Some(&scene_layout)],
            immediate_size: 0,
        });
        let context = BuildContext {
            device: device.clone(),
            raster_layout,
            compute_layout: tracer.compute_pipeline_layout().clone(),
            sample_count: 1,
            engine: TraceEngine::Software,
        };
        let compiler = MaterialCompiler::new(buffers, default_compiled().generated);
        Some(Harness {
            device,
            queue,
            compiler,
            context,
        })
    }

    fn wait_for_build(harness: &mut Harness) -> Option<Built> {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(120) {
            if let Some(built) = harness.compiler.poll(&harness.queue) {
                return Some(built);
            }
            if *harness.compiler.status() != CompileStatus::Compiling {
                return None;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        None
    }

    #[test]
    fn a_new_shape_builds_in_the_background_and_a_new_number_does_not() {
        let Some(mut harness) = harness() else {
            eprintln!("no GPU adapter, skipping");
            return;
        };
        let mut graphs = vec![MaterialGraph::default_material(), SampleMaterial::Stone.build()];
        let compiled = compile_materials(&graphs);
        harness
            .compiler
            .request(&harness.queue, &harness.context, compiled, &GpuMaterialTable::zeroed());
        assert_eq!(*harness.compiler.status(), CompileStatus::Compiling);
        assert!(wait_for_build(&mut harness).is_some(), "the build finishes");
        assert_eq!(*harness.compiler.status(), CompileStatus::Compiled);

        // Same shape, different number: no build at all.
        graphs[1].casts_shadow = false;
        let compiled = compile_materials(&graphs);
        harness
            .compiler
            .request(&harness.queue, &harness.context, compiled, &GpuMaterialTable::zeroed());
        assert_eq!(*harness.compiler.status(), CompileStatus::Compiled);
        assert!(harness.compiler.poll(&harness.queue).is_none());
    }

    #[test]
    fn a_superseded_build_is_dropped_and_undoing_to_the_live_shader_cancels_it() {
        let Some(mut harness) = harness() else {
            eprintln!("no GPU adapter, skipping");
            return;
        };
        let stone = vec![MaterialGraph::default_material(), SampleMaterial::Stone.build()];
        harness.compiler.request(
            &harness.queue,
            &harness.context,
            compile_materials(&stone),
            &GpuMaterialTable::zeroed(),
        );
        // Back to the shader that is live before the build finishes.
        let default = vec![MaterialGraph::default_material()];
        harness.compiler.request(
            &harness.queue,
            &harness.context,
            compile_materials(&default),
            &GpuMaterialTable::zeroed(),
        );
        assert_eq!(*harness.compiler.status(), CompileStatus::Compiled);
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            harness.compiler.poll(&harness.queue).is_none(),
            "the old build is ignored"
        );
        assert_eq!(*harness.compiler.status(), CompileStatus::Compiled);
    }

    #[test]
    fn a_shader_that_does_not_compile_is_reported_and_keeps_the_old_one() {
        let Some(harness) = harness() else {
            eprintln!("no GPU adapter, skipping");
            return;
        };
        let broken = format!("{}fn evaluate_material(", default_compiled().shader_source());
        let error = build(&harness.context, &broken).err().unwrap();
        assert!(error.contains("raster"), "{error}");
        let _ = &harness.device;
    }

    #[test]
    fn numbers_changing_on_a_shader_that_failed_do_not_rebuild_it() {
        let Some(mut harness) = harness() else {
            eprintln!("no GPU adapter, skipping");
            return;
        };
        let graphs = vec![MaterialGraph::default_material(), SampleMaterial::Stone.build()];
        let compiled = compile_materials(&graphs).unwrap();
        harness.compiler.failed = Some(compiled.generated.clone());
        harness.compiler.status = CompileStatus::Error("boom".to_owned());
        harness.compiler.request(
            &harness.queue,
            &harness.context,
            Ok(compiled),
            &GpuMaterialTable::zeroed(),
        );
        assert!(harness.compiler.pending.is_none(), "no build was started");
        assert!(matches!(harness.compiler.status(), CompileStatus::Error(_)));
    }

    #[test]
    fn the_interim_table_keeps_the_materials_the_live_shader_knows() {
        let mut live = GpuMaterialTable::zeroed();
        live.colors[3].graph = 1;
        let mut wanted = GpuMaterialTable::zeroed();
        wanted.colors[3].graph = 2;
        wanted.colors[3].albedo = [0.5, 0.5, 0.5];
        let interim = wanted.with_graphs_of(&live);
        assert_eq!(interim.colors[3].graph, 1);
        assert_eq!(interim.colors[3].albedo, [0.5, 0.5, 0.5]);
    }

    #[test]
    fn a_graph_the_compiler_rejects_shows_as_an_error_until_fixed() {
        let Some(mut harness) = harness() else {
            eprintln!("no GPU adapter, skipping");
            return;
        };
        let compiled = Err(crate::material_compiler::CompileError::TooManyParameters(1));
        harness
            .compiler
            .request(&harness.queue, &harness.context, compiled, &GpuMaterialTable::zeroed());
        assert!(matches!(harness.compiler.status(), CompileStatus::Error(_)));
        let compiled = compile_materials(&[MaterialGraph::default_material()]);
        harness
            .compiler
            .request(&harness.queue, &harness.context, compiled, &GpuMaterialTable::zeroed());
        assert_eq!(*harness.compiler.status(), CompileStatus::Compiled);
    }
}

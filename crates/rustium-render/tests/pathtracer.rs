//! Renders tiny scenes headless on the real GPU and checks what the path tracer produces. Skipped (with a note)
//! when the machine has no adapter. Set `RUSTIUM_DUMP_DIR` to also write the tonemapped images as PNG files.
#![expect(
    clippy::unwrap_used,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::many_single_char_names,
    clippy::print_stderr,
    reason = "test code"
)]

use glam::{IVec3, Mat4, Vec3};
use rustium_core::{
    Brush, ColorId, Command, Face, GraphEdit, GraphNodeKind, HitTarget, InputValue, Mirror, NodeId, Orientation,
    ParamValue, RayHit, SampleMaterial, Scene, SurfaceConstants, Transform, VoxelTool, apply,
};
use rustium_render::{
    Ground, MaterialBuffers, PathTracer, RayTracing, RenderPass, RenderStyle, Sky, ThinLens, TraceEngine, TraceFrame,
    TraceImage,
};

const SIZE: u32 = 96;
/// The app's default exposure, -1 EV.
const EXPOSURE: f32 = 0.5;

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
}

fn gpu() -> Option<Gpu> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    let (device, queue) = pollster::block_on(rustium_render::request_device(&adapter)).ok()?;
    Some(Gpu { device, queue })
}

fn id(number: u8) -> ColorId {
    ColorId::new(number).unwrap()
}

fn first_model(scene: &Scene) -> NodeId {
    scene.node(NodeId::ROOT).unwrap().children()[0]
}

/// Fills a box of the first Model.
fn fill(scene: &mut Scene, min: IVec3, max: IVec3, color: ColorId) {
    let model = first_model(scene);
    fill_model(scene, model, min, max, color);
}

fn fill_model(scene: &mut Scene, model: NodeId, min: IVec3, max: IVec3, color: ColorId) {
    for y in min.y..=max.y {
        for z in min.z..=max.z {
            for x in min.x..=max.x {
                let command = Command::ApplyBrush {
                    model,
                    tool: VoxelTool::Attach,
                    brush: Brush::default(),
                    hit: RayHit {
                        voxel: IVec3::new(x, y - 1, z),
                        face: Face::PosY,
                        target: HitTarget::Ground,
                    },
                    end: None,
                    color,
                    mirror: Mirror::default(),
                };
                apply(scene, &command).unwrap();
            }
        }
    }
}

/// Creates a material graph with `surface` and points Color `color` at it.
fn shade(scene: &mut Scene, color: u8, surface: SurfaceConstants) {
    let graph = u32::try_from(scene.material_graphs().len()).unwrap();
    let create = Command::CreateGraph {
        name: format!("Test {graph}"),
        surface,
    };
    apply(scene, &create).unwrap();
    apply(scene, &Command::AssignGraph { id: id(color), graph }).unwrap();
}

struct Camera {
    view_proj: Mat4,
}

impl Camera {
    fn looking_at(eye: Vec3, target: Vec3) -> Self {
        let view = glam::camera::rh::view::look_at_mat4(eye, target, Vec3::Y);
        let projection = glam::camera::rh::proj::directx::perspective(45_f32.to_radians(), 1.0, 0.1, 500.0);
        Self {
            view_proj: projection * view,
        }
    }

    /// The pixel a world point lands on.
    fn pixel(&self, point: Vec3) -> (u32, u32) {
        let clip = self.view_proj.project_point3(point);
        let x = ((clip.x * 0.5 + 0.5) * SIZE as f32)
            .round()
            .clamp(0.0, SIZE as f32 - 1.0);
        let y = ((0.5 - clip.y * 0.5) * SIZE as f32)
            .round()
            .clamp(0.0, SIZE as f32 - 1.0);
        (x as u32, y as u32)
    }
}

struct Setup {
    sky: Sky,
    ground: Ground,
    sun_height: f32,
    samples: u32,
    style: RenderStyle,
    denoise: bool,
    lens: Option<ThinLens>,
    pass: RenderPass,
    /// A pass to switch to once the samples are done, without restarting.
    switch_to: Option<RenderPass>,
    region: Option<[u32; 4]>,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            sky: Sky::Clear,
            ground: Ground::None,
            sun_height: 45.0,
            samples: 16,
            style: RenderStyle::Traced,
            denoise: false,
            lens: None,
            pass: RenderPass::Beauty,
            switch_to: None,
            region: None,
        }
    }
}

/// Renders with the software tracer.
fn render(gpu: &Gpu, scene: &Scene, camera: &Camera, setup: &Setup) -> TraceImage {
    render_with(gpu, scene, camera, setup, RayTracing::Software, None)
}

/// Renders `scene` with the given traversal; `before` is uploaded first, as an earlier state of the Scene.
fn render_with(
    gpu: &Gpu,
    scene: &Scene,
    camera: &Camera,
    setup: &Setup,
    ray_tracing: RayTracing,
    before: Option<&Scene>,
) -> TraceImage {
    let buffers = MaterialBuffers::new(&gpu.device);
    buffers.write_colors(&gpu.queue, scene.palette(), scene.material_graphs());
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut tracer = PathTracer::new(&gpu.device, &buffers, format, ray_tracing);
    tracer.set_time_budget(false);
    if ray_tracing == RayTracing::Hardware && has_ray_query(gpu) {
        assert_eq!(
            tracer.engine(),
            TraceEngine::Hardware,
            "the hardware engine is the one under test"
        );
    }
    tracer
        .set_materials(&gpu.device, &gpu.queue, scene.material_graphs())
        .unwrap();
    if let Some(before) = before {
        tracer.upload_scene(&gpu.device, &gpu.queue, before);
    }
    tracer.upload_scene(&gpu.device, &gpu.queue, scene);
    let target = gpu
        .device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("test target"),
            size: wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default());
    let frame = TraceFrame {
        inv_view_proj: camera.view_proj.inverse().to_cols_array_2d(),
        size: (SIZE, SIZE),
        samples: setup.samples,
        bounces: 5,
        sun_turn_degrees: 135.0,
        sun_height_degrees: setup.sun_height,
        sky: setup.sky,
        ground: setup.ground,
        transparent: false,
        region: setup.region,
        running: true,
        frame_rect: [0.0, 0.0, SIZE as f32, SIZE as f32],
        exposure: EXPOSURE,
        style: setup.style,
        denoise: setup.denoise,
        lens: setup.lens,
        pass: setup.pass,
    };
    for _ in 0..setup.samples {
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("test") });
        tracer.encode(&gpu.device, &gpu.queue, &mut encoder, &target, &frame);
        gpu.queue.submit(std::iter::once(encoder.finish()));
    }
    assert_eq!(tracer.progress().samples, setup.samples);
    if let Some(pass) = setup.switch_to {
        let generation = tracer.progress().generation;
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("test") });
        tracer.encode(
            &gpu.device,
            &gpu.queue,
            &mut encoder,
            &target,
            &TraceFrame { pass, ..frame },
        );
        gpu.queue.submit(std::iter::once(encoder.finish()));
        let progress = tracer.progress();
        assert_eq!(progress.samples, setup.samples, "switching the pass keeps the samples");
        assert_eq!(progress.generation, generation, "switching the pass does not restart");
    }
    tracer.read_image(&gpu.device, &gpu.queue).unwrap()
}

fn luminance(image: &TraceImage, (x, y): (u32, u32)) -> f32 {
    let index = ((y * image.width + x) * 4) as usize;
    let [r, g, b] = [image.rgba[index], image.rgba[index + 1], image.rgba[index + 2]];
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// Mean luminance of the 7x7 pixels around `centre`, to average out path tracing noise.
fn area_luminance(image: &TraceImage, (cx, cy): (u32, u32)) -> f32 {
    let mut total = 0.0;
    for y in cy.saturating_sub(3)..=(cy + 3).min(image.height - 1) {
        for x in cx.saturating_sub(3)..=(cx + 3).min(image.width - 1) {
            total += luminance(image, (x, y));
        }
    }
    total / 49.0
}

/// ACES filmic curve plus the sRGB transfer function: what the screen shows for a linear value, in `0..=1`.
fn display(linear: f32) -> f32 {
    let aces = ((linear * (2.51 * linear + 0.03)) / (linear * (2.43 * linear + 0.59) + 0.14)).clamp(0.0, 1.0);
    let mapped = aces + (linear - aces) * (-linear / 0.04).exp();
    if mapped <= 0.003_130_8 {
        mapped * 12.92
    } else {
        1.055 * mapped.powf(1.0 / 2.4) - 0.055
    }
}

fn rgb(image: &TraceImage, (x, y): (u32, u32)) -> [f32; 3] {
    let index = ((y * image.width + x) * 4) as usize;
    [image.rgba[index], image.rgba[index + 1], image.rgba[index + 2]]
}

fn dump(name: &str, image: &TraceImage) {
    let Ok(dir) = std::env::var("RUSTIUM_DUMP_DIR") else {
        return;
    };
    let tonemap = |value: f32| {
        let x = value * EXPOSURE;
        let aces = ((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14)).clamp(0.0, 1.0);
        let mapped = aces + (x - aces) * (-x / 0.04).exp();
        let srgb = if mapped <= 0.003_130_8 {
            mapped * 12.92
        } else {
            1.055 * mapped.powf(1.0 / 2.4) - 0.055
        };
        (srgb * 255.0).round() as u8
    };
    let mut bytes = Vec::new();
    for texel in image.rgba.chunks(4) {
        bytes.extend([tonemap(texel[0]), tonemap(texel[1]), tonemap(texel[2]), 255]);
    }
    let file = std::fs::File::create(std::path::Path::new(&dir).join(format!("{name}.png"))).unwrap();
    let mut encoder = png::Encoder::new(file, image.width, image.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().unwrap().write_image_data(&bytes).unwrap();
}

macro_rules! gpu_or_skip {
    () => {
        match gpu() {
            Some(gpu) => gpu,
            None => {
                eprintln!("no GPU adapter, skipping");
                return;
            }
        }
    };
}

#[test]
fn an_empty_scene_shows_the_sky() {
    let gpu = gpu_or_skip!();
    let scene = Scene::new();
    let camera = Camera::looking_at(Vec3::new(0.0, 2.0, 10.0), Vec3::new(0.0, 4.0, 0.0));
    let setup = Setup {
        sky: Sky::Clear,
        ground: Ground::None,
        sun_height: 42.0,
        samples: 4,
        ..Setup::default()
    };
    let image = render(&gpu, &scene, &camera, &setup);
    dump("sky", &image);
    let [r, _, b] = rgb(&image, (SIZE / 2, 4));
    assert!(b > r, "clear sky is blue, got r={r} b={b}");
    assert!(luminance(&image, (SIZE / 2, 4)) > 0.1);
}

#[test]
fn a_cube_in_the_sun_casts_a_shadow_on_the_floor() {
    let gpu = gpu_or_skip!();
    let mut scene = Scene::new();
    fill(&mut scene, IVec3::new(-12, 0, -12), IVec3::new(12, 0, 12), id(3));
    fill(&mut scene, IVec3::new(-2, 1, -2), IVec3::new(1, 4, 1), id(37));
    let camera = Camera::looking_at(Vec3::new(14.0, 14.0, 20.0), Vec3::new(0.0, 2.0, 0.0));
    let setup = Setup {
        sky: Sky::Clear,
        ground: Ground::Studio,
        sun_height: 40.0,
        samples: 48,
        ..Setup::default()
    };
    let image = render(&gpu, &scene, &camera, &setup);
    dump("sun-cube", &image);
    // The sun comes from turn 135 degrees (+x, +z side); the shadow falls towards -x, -z.
    let lit = luminance(&image, camera.pixel(Vec3::new(5.0, 1.0, 5.0)));
    let shadowed = luminance(&image, camera.pixel(Vec3::new(-4.5, 1.0, -4.5)));
    assert!(
        lit > shadowed * 1.5,
        "lit {lit} should be clearly brighter than shadowed {shadowed}"
    );
    assert!(shadowed > 0.0, "the shadow is lit by the sky, not black");
    // After exposure and tonemapping the lit studio floor is a light grey (the mockup's ~#BFC3C7 is 0.75), not white,
    // and the shadow stays clearly visible.
    let shown = |linear: f32| display(linear * EXPOSURE);
    let (lit_shown, shadow_shown) = (shown(lit), shown(shadowed));
    assert!((0.6..0.88).contains(&lit_shown), "lit floor shows as {lit_shown}");
    assert!(
        shadow_shown < lit_shown - 0.15,
        "shadow {shadow_shown} against floor {lit_shown}"
    );
}

#[test]
fn an_emissive_voxel_lights_its_surroundings() {
    let gpu = gpu_or_skip!();
    let mut scene = Scene::new();
    fill(&mut scene, IVec3::new(-10, 0, -10), IVec3::new(10, 0, 10), id(3));
    fill(&mut scene, IVec3::new(0, 1, 0), IVec3::new(0, 1, 0), id(60));
    shade(
        &mut scene,
        60,
        SurfaceConstants {
            emission_color: [255, 200, 120],
            emission_strength: 20.0,
            ..SurfaceConstants::MATTE
        },
    );
    let camera = Camera::looking_at(Vec3::new(0.0, 12.0, 14.0), Vec3::new(0.0, 0.0, 0.0));
    let setup = Setup {
        sky: Sky::Night,
        ground: Ground::Studio,
        sun_height: 20.0,
        samples: 64,
        ..Setup::default()
    };
    let image = render(&gpu, &scene, &camera, &setup);
    dump("emissive", &image);
    let near = area_luminance(&image, camera.pixel(Vec3::new(2.5, 1.0, 0.5)));
    let far = area_luminance(&image, camera.pixel(Vec3::new(8.5, 1.0, 0.5)));
    assert!(
        near > far * 2.0,
        "floor next to the light ({near}) should beat the far floor ({far})"
    );
    let light = luminance(&image, camera.pixel(Vec3::new(0.5, 1.5, 0.5)));
    assert!(light > 5.0, "the emissive voxel itself is bright, got {light}");
}

#[test]
fn glass_and_metal_render_without_blowing_up() {
    let gpu = gpu_or_skip!();
    let mut scene = Scene::new();
    fill(&mut scene, IVec3::new(-8, 0, -8), IVec3::new(8, 0, 8), id(8));
    fill(&mut scene, IVec3::new(-5, 1, -1), IVec3::new(-3, 3, 1), id(1));
    fill(&mut scene, IVec3::new(-1, 1, -1), IVec3::new(1, 3, 1), id(30));
    shade(
        &mut scene,
        1,
        SurfaceConstants {
            transmission: 1.0,
            roughness: 0.0,
            ..SurfaceConstants::MATTE
        },
    );
    shade(
        &mut scene,
        30,
        SurfaceConstants {
            metallic: 1.0,
            roughness: 0.15,
            ..SurfaceConstants::MATTE
        },
    );
    let camera = Camera::looking_at(Vec3::new(2.0, 8.0, 16.0), Vec3::new(0.0, 1.5, 0.0));
    let setup = Setup {
        sky: Sky::Clear,
        ground: Ground::Studio,
        sun_height: 45.0,
        samples: 64,
        ..Setup::default()
    };
    let image = render(&gpu, &scene, &camera, &setup);
    dump("colors", &image);
    assert!(
        image.rgba.iter().all(|value| value.is_finite()),
        "no NaN or infinity in the image"
    );
    let glass = area_luminance(&image, camera.pixel(Vec3::new(-4.0, 2.0, 1.0)));
    assert!(glass > 0.02, "clear glass shows what is behind it, got {glass}");
}

/// An L-shaped Model, a turned and moved Instance of it, a mirrored Instance and a hidden Model; every voxel glows.
fn instanced_scene() -> (Scene, Camera) {
    let mut scene = Scene::new();
    // Every voxel glows, so "lit" simply means "there is a voxel here".
    shade(
        &mut scene,
        60,
        SurfaceConstants {
            emission_strength: 8.0,
            ..SurfaceConstants::MATTE
        },
    );
    // An L: a bar of three along +X with one voxel on top of its first cell.
    let a = first_model(&scene);
    fill_model(&mut scene, a, IVec3::new(0, 1, 0), IVec3::new(2, 1, 0), id(60));
    fill_model(&mut scene, a, IVec3::new(0, 2, 0), IVec3::new(0, 2, 0), id(60));

    // An Instance of it, turned a quarter about Y and moved 10 voxels along X.
    apply(&mut scene, &Command::DuplicateAsInstance { node: a }).unwrap();
    let children = scene.node(NodeId::ROOT).unwrap().children().to_vec();
    let instance = children.into_iter().find(|child| *child != a).unwrap();
    assert_eq!(
        scene.model_shape(a).unwrap().0,
        scene.model_shape(instance).unwrap().0,
        "an Instance shares its Shape"
    );
    let transform = Transform {
        orientation: Orientation::rotation(1, 1),
        offset: IVec3::new(10, 0, 0),
    };
    apply(
        &mut scene,
        &Command::SetTransform {
            node: instance,
            transform,
        },
    )
    .unwrap();

    // A third Model that is filled and then hidden must not show up.
    apply(
        &mut scene,
        &Command::NewModel {
            parent: NodeId::ROOT,
            name: "Hidden".to_owned(),
        },
    )
    .unwrap();
    let hidden = scene.node(NodeId::ROOT).unwrap().children()[0];
    fill_model(&mut scene, hidden, IVec3::new(4, 1, -6), IVec3::new(6, 1, -4), id(60));
    apply(
        &mut scene,
        &Command::SetHidden {
            node: hidden,
            hidden: true,
        },
    )
    .unwrap();

    // A second Instance, mirrored across X and moved behind the first, shares the Shape through the same BLAS.
    apply(&mut scene, &Command::DuplicateAsInstance { node: a }).unwrap();
    let mirrored = scene
        .node(NodeId::ROOT)
        .unwrap()
        .children()
        .iter()
        .copied()
        .find(|child| *child != a && *child != instance && *child != hidden)
        .unwrap();
    apply(
        &mut scene,
        &Command::SetTransform {
            node: mirrored,
            transform: Transform {
                orientation: Orientation::mirror(0),
                offset: IVec3::new(-3, 0, -8),
            },
        },
    )
    .unwrap();

    let camera = Camera::looking_at(Vec3::new(5.0, 28.0, 6.0), Vec3::new(5.0, 0.0, -1.0));
    (scene, camera)
}

#[test]
fn models_are_placed_by_their_transforms_and_hidden_ones_are_skipped() {
    let gpu = gpu_or_skip!();
    let (scene, camera) = instanced_scene();
    let setup = Setup {
        sky: Sky::Clear,
        ground: Ground::None,
        sun_height: 45.0,
        samples: 16,
        ..Setup::default()
    };
    let image = render(&gpu, &scene, &camera, &setup);
    dump("models", &image);
    let lit = |x: f32, y: f32, z: f32| luminance(&image, camera.pixel(Vec3::new(x, y, z)));
    // The original L lies along +X from the origin.
    assert!(lit(1.5, 1.5, 0.5) > 3.0, "the original model is drawn");
    // The instance's bar runs along -Z from x = 10.5: its far end is only there if the rotation was applied.
    assert!(
        lit(10.5, 1.5, -2.5) > 3.0,
        "the rotated instance is drawn where its transform puts it"
    );
    assert!(
        lit(12.5, 1.5, 0.5) < 1.5,
        "nothing sits where an unrotated instance would be"
    );
    assert!(lit(5.5, 1.5, -4.5) < 1.5, "the hidden model is skipped");
}

/// A two-colour showpiece used by the style tests: a floor and two cubes of different Colors.
fn showpiece() -> (Scene, Camera) {
    let mut scene = Scene::new();
    fill(&mut scene, IVec3::new(-12, 0, -12), IVec3::new(12, 0, 12), id(3));
    fill(&mut scene, IVec3::new(-6, 1, -2), IVec3::new(-3, 4, 1), id(37));
    fill(&mut scene, IVec3::new(2, 1, -2), IVec3::new(5, 4, 1), id(8));
    let camera = Camera::looking_at(Vec3::new(14.0, 14.0, 20.0), Vec3::new(0.0, 2.0, 0.0));
    (scene, camera)
}

/// The pixel on the top face of a cube, `x` along its width.
fn top_of(camera: &Camera, x: f32) -> (u32, u32) {
    camera.pixel(Vec3::new(x, 5.0, -0.5))
}

#[test]
fn clay_makes_every_color_the_same_grey() {
    let gpu = gpu_or_skip!();
    let (scene, camera) = showpiece();
    let setup = Setup {
        ground: Ground::Studio,
        samples: 32,
        ..Setup::default()
    };
    let traced = render(&gpu, &scene, &camera, &setup);
    let clay = render(
        &gpu,
        &scene,
        &camera,
        &Setup {
            style: RenderStyle::Clay,
            ..setup
        },
    );
    dump("style-clay", &clay);
    let (left, right) = (top_of(&camera, -4.5), top_of(&camera, 3.5));
    let traced_ratio = area_luminance(&traced, left) / area_luminance(&traced, right);
    let clay_ratio = area_luminance(&clay, left) / area_luminance(&clay, right);
    assert!(
        (traced_ratio - 1.0).abs() > 0.1,
        "the two Colors differ when traced: {traced_ratio}"
    );
    assert!(
        (clay_ratio - 1.0).abs() < 0.08,
        "clay lights both tops alike, ratio {clay_ratio}"
    );
}

#[test]
fn toon_has_flat_bands_and_dark_outlines() {
    let gpu = gpu_or_skip!();
    let (scene, camera) = showpiece();
    let toon = render(
        &gpu,
        &scene,
        &camera,
        &Setup {
            samples: 8,
            style: RenderStyle::Toon,
            ..Setup::default()
        },
    );
    dump("style-toon", &toon);
    // Walking along the row through the left cube crosses sky, outline, face and outline again.
    let (_, row) = camera.pixel(Vec3::new(-4.5, 2.5, -0.5));
    let sky = luminance(&toon, (2, row));
    let darkest = (0..SIZE).map(|x| luminance(&toon, (x, row))).fold(f32::MAX, f32::min);
    assert!(sky > 0.2, "sky is bright, got {sky}");
    assert!(darkest < 0.03, "an outline is nearly black, darkest pixel {darkest}");
    // A flat face is one band: neighbouring pixels on the top of the cube agree.
    let (x, y) = top_of(&camera, -4.5);
    let (a, b) = (luminance(&toon, (x, y)), luminance(&toon, (x + 1, y)));
    assert!((a - b).abs() < 1e-3, "flat band, got {a} and {b}");
}

#[test]
fn pixel_style_repeats_each_block() {
    let gpu = gpu_or_skip!();
    let (scene, camera) = showpiece();
    let pixel = render(
        &gpu,
        &scene,
        &camera,
        &Setup {
            ground: Ground::Studio,
            samples: 8,
            style: RenderStyle::Pixel,
            ..Setup::default()
        },
    );
    dump("style-pixel", &pixel);
    // The 96 px test image uses 2 px blocks.
    for y in (0..SIZE).step_by(2) {
        for x in (0..SIZE).step_by(2) {
            assert_eq!(rgb(&pixel, (x, y)), rgb(&pixel, (x + 1, y)), "block at {x},{y}");
            assert_eq!(rgb(&pixel, (x, y)), rgb(&pixel, (x, y + 1)), "block at {x},{y}");
        }
    }
}

#[test]
fn wire_draws_light_lines_on_dark() {
    let gpu = gpu_or_skip!();
    let (scene, camera) = showpiece();
    let wire = render(
        &gpu,
        &scene,
        &camera,
        &Setup {
            ground: Ground::Studio,
            samples: 8,
            style: RenderStyle::Wire,
            ..Setup::default()
        },
    );
    dump("style-wire", &wire);
    let count = |keep: &dyn Fn(f32) -> bool| {
        (0..SIZE * SIZE)
            .filter(|index| keep(luminance(&wire, (index % SIZE, index / SIZE))))
            .count()
    };
    let lines = count(&|value| value > 0.8);
    let dark = count(&|value| value < 0.1);
    assert!(lines > 20, "edges are drawn, found {lines} bright pixels");
    assert!(
        dark > (SIZE * SIZE) as usize / 2,
        "faces and background stay dark, found {dark}"
    );
}

#[test]
fn denoising_removes_noise_but_keeps_the_mean() {
    let gpu = gpu_or_skip!();
    let (scene, camera) = showpiece();
    let setup = Setup {
        ground: Ground::Studio,
        samples: 6,
        ..Setup::default()
    };
    let noisy = render(&gpu, &scene, &camera, &setup);
    let clean = render(&gpu, &scene, &camera, &Setup { denoise: true, ..setup });
    dump("denoise-off", &noisy);
    dump("denoise-on", &clean);
    // A patch of floor in the open sun.
    let (cx, cy) = camera.pixel(Vec3::new(8.0, 1.0, 6.0));
    let spread = |image: &TraceImage| {
        let mut values = Vec::new();
        for y in cy - 5..=cy + 5 {
            for x in cx - 5..=cx + 5 {
                values.push(luminance(image, (x, y)));
            }
        }
        let mean = values.iter().sum::<f32>() / values.len() as f32;
        let variance = values.iter().map(|value| (value - mean).powi(2)).sum::<f32>() / values.len() as f32;
        (mean, variance.sqrt())
    };
    let ((noisy_mean, noisy_spread), (clean_mean, clean_spread)) = (spread(&noisy), spread(&clean));
    assert!(
        clean_spread < noisy_spread * 0.6,
        "noise {noisy_spread} should drop clearly, got {clean_spread}"
    );
    assert!(
        (clean_mean / noisy_mean - 1.0).abs() < 0.1,
        "mean stays put: {noisy_mean} against {clean_mean}"
    );
    // The silhouette of the cube against the sky stays sharp.
    let sky_pixel = luminance(&clean, (2, 2));
    let cube = luminance(&clean, top_of(&camera, -4.5));
    assert!(
        (sky_pixel - cube).abs() > 0.1,
        "cube {cube} still differs from sky {sky_pixel}"
    );
}

#[test]
fn depth_of_field_blurs_what_is_off_the_focal_plane() {
    let gpu = gpu_or_skip!();
    let mut scene = Scene::new();
    // Two wide slabs, one at the focal distance and one twice as far.
    fill(&mut scene, IVec3::new(-8, 0, 0), IVec3::new(-3, 19, 0), id(37));
    fill(&mut scene, IVec3::new(3, 0, -30), IVec3::new(8, 19, -30), id(37));
    let eye = Vec3::new(0.0, 8.0, 30.0);
    let camera = Camera::looking_at(eye, Vec3::new(0.0, 8.0, 0.0));
    let sharp_setup = Setup {
        samples: 64,
        ..Setup::default()
    };
    let lens = ThinLens {
        eye: eye.to_array(),
        radius: 3.0,
        focus_distance: 30.0,
    };
    let sharp = render(&gpu, &scene, &camera, &sharp_setup);
    let blurred = render(
        &gpu,
        &scene,
        &camera,
        &Setup {
            lens: Some(lens),
            ..sharp_setup
        },
    );
    dump("dof", &blurred);
    // Steepest brightness change across a slab's vertical edge, relative to the edge's full contrast.
    let steepness = |image: &TraceImage, edge_x: f32| {
        let (px, py) = camera.pixel(Vec3::new(edge_x, 8.0, if edge_x < 0.0 { 0.0 } else { -30.0 }));
        let row: Vec<f32> = (px.saturating_sub(10)..=(px + 10).min(SIZE - 1))
            .map(|x| luminance(image, (x, py)))
            .collect();
        let contrast = row.iter().copied().fold(f32::MIN, f32::max) - row.iter().copied().fold(f32::MAX, f32::min);
        let step = row.windows(2).map(|pair| (pair[1] - pair[0]).abs()).fold(0.0, f32::max);
        assert!(contrast > 0.1, "the slab edge is visible, contrast {contrast}");
        step / contrast
    };
    let near_sharp = steepness(&blurred, -8.0);
    let far_blurred = steepness(&blurred, 9.0);
    let far_without_lens = steepness(&sharp, 9.0);
    assert!(
        near_sharp > far_blurred * 1.8,
        "the focused edge ({near_sharp}) is steeper than the defocused one ({far_blurred})"
    );
    assert!(
        far_without_lens > far_blurred * 1.5,
        "the pinhole keeps the far edge sharp ({far_without_lens} against {far_blurred})"
    );
}

#[test]
fn aov_passes_show_albedo_normals_and_depth() {
    let gpu = gpu_or_skip!();
    let (scene, camera) = showpiece();
    let pass = |pass| {
        render(
            &gpu,
            &scene,
            &camera,
            &Setup {
                samples: 4,
                pass,
                ..Setup::default()
            },
        )
    };
    let albedo = pass(RenderPass::Albedo);
    let normal = pass(RenderPass::Normal);
    let depth = pass(RenderPass::Depth);
    dump("pass-albedo", &albedo);
    assert!(albedo.display_ready);
    // The left cube is yellow (id 37): red and green well above blue, with no lighting in it.
    let [r, g, b] = rgb(&albedo, top_of(&camera, -4.5));
    assert!(r > b * 2.0 && g > b * 2.0, "albedo of the yellow cube: {r} {g} {b}");
    // Its top face points up, so the green channel (y) is 1.0 and x and z sit at 0.5.
    let [nx, ny, nz] = rgb(&normal, top_of(&camera, -4.5));
    assert!(
        ny > 0.99 && (nx - 0.5).abs() < 0.01 && (nz - 0.5).abs() < 0.01,
        "normal {nx} {ny} {nz}"
    );
    // The cube is nearer than the floor behind it, and the sky has no depth at all.
    let near = luminance(&depth, top_of(&camera, -4.5));
    let sky = luminance(&depth, (2, 2));
    assert!(near > sky + 0.1, "cube depth {near} against sky {sky}");
}

#[test]
fn switching_the_view_pass_keeps_the_accumulated_samples() {
    let gpu = gpu_or_skip!();
    let (scene, camera) = showpiece();
    for pass in [RenderPass::Albedo, RenderPass::Normal, RenderPass::Depth] {
        let direct = render(
            &gpu,
            &scene,
            &camera,
            &Setup {
                samples: 4,
                pass,
                ..Setup::default()
            },
        );
        let switched = render(
            &gpu,
            &scene,
            &camera,
            &Setup {
                samples: 4,
                switch_to: Some(pass),
                ..Setup::default()
            },
        );
        assert!(switched.display_ready);
        assert_eq!(switched.samples, 4);
        let (mean, _) = difference(&direct, &switched);
        assert!(
            mean < 1.0e-4,
            "{pass:?} after switching differs from a direct render by {mean}"
        );
    }
}

#[test]
fn offline_rendering_reaches_the_target_and_can_be_cancelled() {
    let gpu = gpu_or_skip!();
    let (scene, camera) = showpiece();
    let buffers = MaterialBuffers::new(&gpu.device);
    buffers.write_colors(&gpu.queue, scene.palette(), scene.material_graphs());
    let mut tracer = PathTracer::new(
        &gpu.device,
        &buffers,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        RayTracing::Software,
    );
    tracer
        .set_materials(&gpu.device, &gpu.queue, scene.material_graphs())
        .unwrap();
    tracer.upload_scene(&gpu.device, &gpu.queue, &scene);
    let frame = TraceFrame {
        inv_view_proj: camera.view_proj.inverse().to_cols_array_2d(),
        size: (SIZE, SIZE),
        samples: 10,
        bounces: 4,
        sun_turn_degrees: 135.0,
        sun_height_degrees: 45.0,
        sky: Sky::Clear,
        ground: Ground::Studio,
        transparent: false,
        region: None,
        running: true,
        frame_rect: [0.0, 0.0, SIZE as f32, SIZE as f32],
        exposure: EXPOSURE,
        style: RenderStyle::Traced,
        denoise: true,
        lens: None,
        pass: RenderPass::Beauty,
    };
    let mut reported = Vec::new();
    let image = tracer
        .render_to_image(&gpu.device, &gpu.queue, &frame, |done| {
            reported.push(done);
            true
        })
        .unwrap()
        .unwrap();
    assert_eq!(image.samples, 10);
    assert_eq!(reported.last(), Some(&10));
    assert!(luminance(&image, (SIZE / 2, SIZE / 2)) > 0.0);
    let cancelled = tracer
        .render_to_image(&gpu.device, &gpu.queue, &frame, |done| done < 4)
        .unwrap();
    assert!(cancelled.is_none(), "returning false from progress cancels the render");
}

/// Standard deviation of the luminance at the centre pixel of every voxel of a floor patch seen from above.
fn voxel_spread(gpu: &Gpu, material: Option<SampleMaterial>) -> f32 {
    let mut scene = Scene::new();
    fill(&mut scene, IVec3::new(-5, 0, -5), IVec3::new(4, 0, 4), id(3));
    if let Some(sample) = material {
        let graph = u32::try_from(scene.material_graphs().len()).unwrap();
        let add = Command::AddMaterial {
            graph: Box::new(sample.build()),
        };
        apply(&mut scene, &add).unwrap();
        apply(&mut scene, &Command::AssignGraph { id: id(3), graph }).unwrap();
    }
    let camera = Camera::looking_at(Vec3::new(0.0, 22.0, 0.01), Vec3::new(0.0, 0.0, 0.0));
    let setup = Setup {
        sky: Sky::Overcast,
        ground: Ground::None,
        sun_height: 70.0,
        samples: 200,
        ..Setup::default()
    };
    let image = render(gpu, &scene, &camera, &setup);
    dump(material.map_or("flat", SampleMaterial::label), &image);
    let mut values = Vec::new();
    for x in -4..4 {
        for z in -4..4 {
            values.push(luminance(
                &image,
                camera.pixel(Vec3::new(x as f32 + 0.5, 1.0, z as f32 + 0.5)),
            ));
        }
    }
    let mean = values.iter().sum::<f32>() / values.len() as f32;
    (values.iter().map(|value| (value - mean).powi(2)).sum::<f32>() / values.len() as f32).sqrt()
}

#[test]
fn a_per_voxel_noise_graph_varies_the_color_from_voxel_to_voxel() {
    let gpu = gpu_or_skip!();
    let flat = voxel_spread(&gpu, None);
    let slate = voxel_spread(&gpu, Some(SampleMaterial::RoofSlate));
    assert!(
        slate > flat * 1.4,
        "roof slate varies per voxel (spread {slate}), the default does not (spread {flat})"
    );
}

#[test]
fn a_gradient_graph_changes_the_color_along_its_axis() {
    let gpu = gpu_or_skip!();
    let mut scene = Scene::new();
    // A tall pillar seen from the side: the grass gradient lightens the colour towards its tip.
    fill(&mut scene, IVec3::new(-1, 0, -1), IVec3::new(1, 7, 1), id(3));
    let graph = u32::try_from(scene.material_graphs().len()).unwrap();
    let add = Command::AddMaterial {
        graph: Box::new(SampleMaterial::Grass.build()),
    };
    apply(&mut scene, &add).unwrap();
    apply(&mut scene, &Command::AssignGraph { id: id(3), graph }).unwrap();
    let camera = Camera::looking_at(Vec3::new(0.0, 4.0, 18.0), Vec3::new(0.0, 4.0, 0.0));
    let setup = Setup {
        sky: Sky::Overcast,
        ground: Ground::None,
        sun_height: 60.0,
        samples: 32,
        ..Setup::default()
    };
    let image = render(&gpu, &scene, &camera, &setup);
    dump("grass", &image);
    let low = rgb(&image, camera.pixel(Vec3::new(0.0, 0.5, 2.0)));
    let high = rgb(&image, camera.pixel(Vec3::new(0.0, 7.5, 2.0)));
    assert!(
        (high[1] / high[0].max(1e-3)) > (low[1] / low[0].max(1e-3)) * 1.05,
        "the tip is greener: low {low:?} high {high:?}"
    );
}

#[test]
fn a_model_space_gradient_moves_with_its_model() {
    let gpu = gpu_or_skip!();
    let mut scene = Scene::new();
    fill(&mut scene, IVec3::new(-1, 0, -1), IVec3::new(1, 7, 1), id(3));
    let graph = u32::try_from(scene.material_graphs().len()).unwrap();
    let add = Command::AddMaterial {
        graph: Box::new(SampleMaterial::Grass.build()),
    };
    apply(&mut scene, &add).unwrap();
    apply(&mut scene, &Command::AssignGraph { id: id(3), graph }).unwrap();
    // The pillar stands 30 voxels higher: in World space the whole gradient would be past its end.
    let model = first_model(&scene);
    apply(
        &mut scene,
        &Command::SetTransform {
            node: model,
            transform: Transform {
                orientation: Orientation::default(),
                offset: IVec3::new(0, 30, 0),
            },
        },
    )
    .unwrap();
    let camera = Camera::looking_at(Vec3::new(0.0, 34.0, 18.0), Vec3::new(0.0, 34.0, 0.0));
    let setup = Setup {
        sky: Sky::Overcast,
        ground: Ground::None,
        sun_height: 60.0,
        samples: 32,
        ..Setup::default()
    };
    let image = render(&gpu, &scene, &camera, &setup);
    dump("model-space", &image);
    let low = rgb(&image, camera.pixel(Vec3::new(0.0, 30.5, 2.0)));
    let high = rgb(&image, camera.pixel(Vec3::new(0.0, 37.5, 2.0)));
    assert!(
        (high[1] / high[0].max(1e-3)) > (low[1] / low[0].max(1e-3)) * 1.05,
        "the tip is greener in the Model's own space: low {low:?} high {high:?}"
    );
}

/// A patch of Roof slate (its Noise Vector is linked from Voxel color's Position) seen from above, the Model moved by
/// `offset` and the camera with it; `space` is the Noise node's Space choice (0 Model, 1 World).
fn slate_patch(gpu: &Gpu, offset: IVec3, space: u32) -> TraceImage {
    let mut scene = Scene::new();
    fill(&mut scene, IVec3::new(-5, 0, -5), IVec3::new(4, 0, 4), id(3));
    let graph = u32::try_from(scene.material_graphs().len()).unwrap();
    let mut slate = SampleMaterial::RoofSlate.build();
    let noise = slate
        .nodes()
        .iter()
        .find(|node| node.kind == GraphNodeKind::Noise)
        .unwrap()
        .id;
    slate
        .apply_edit(&GraphEdit::SetParam {
            node: noise,
            param: 3,
            value: ParamValue::Choice(space),
        })
        .unwrap();
    apply(&mut scene, &Command::AddMaterial { graph: Box::new(slate) }).unwrap();
    apply(&mut scene, &Command::AssignGraph { id: id(3), graph }).unwrap();
    let model = first_model(&scene);
    let transform = Transform {
        orientation: Orientation::default(),
        offset,
    };
    apply(&mut scene, &Command::SetTransform { node: model, transform }).unwrap();
    let shift = offset.as_vec3();
    let camera = Camera::looking_at(Vec3::new(0.0, 22.0, 0.01) + shift, shift);
    let setup = Setup {
        sky: Sky::Overcast,
        ground: Ground::None,
        sun_height: 70.0,
        samples: 64,
        ..Setup::default()
    };
    render(gpu, &scene, &camera, &setup)
}

#[test]
fn a_model_space_pattern_linked_to_the_voxel_position_moves_with_its_model() {
    let gpu = gpu_or_skip!();
    let home = slate_patch(&gpu, IVec3::ZERO, 0);
    let moved = slate_patch(&gpu, IVec3::new(13, 9, -7), 0);
    let (same, _) = difference(&home, &moved);
    let world = slate_patch(&gpu, IVec3::new(13, 9, -7), 1);
    let (changed, _) = difference(&home, &world);
    dump("slate-home", &home);
    dump("slate-moved", &moved);
    dump("slate-world", &world);
    assert!(
        same * 3.0 < changed,
        "in Model space the moved patch looks the same as at home ({same}); in World space it does not ({changed})"
    );
}

/// How far two images of the same scene are apart: the mean absolute difference per channel, and the share of pixels
/// that differ clearly (more than 0.25 in any channel).
fn difference(a: &TraceImage, b: &TraceImage) -> (f32, f32) {
    assert_eq!((a.width, a.height), (b.width, b.height));
    let mut total = 0.0;
    let mut off = 0_usize;
    for (left, right) in a.rgba.chunks(4).zip(b.rgba.chunks(4)) {
        let mut worst = 0.0_f32;
        for (x, y) in left.iter().zip(right) {
            total += (x - y).abs();
            worst = worst.max((x - y).abs());
        }
        off += usize::from(worst > 0.25);
    }
    (total / a.rgba.len() as f32, off as f32 / (a.rgba.len() / 4) as f32)
}

fn has_ray_query(gpu: &Gpu) -> bool {
    gpu.device.features().contains(wgpu::Features::EXPERIMENTAL_RAY_QUERY)
}

/// Renders `scene` with both traversals and checks that they agree. The random numbers are seeded per pixel and
/// sample, so only float rounding differs between the two. `before` is uploaded to the hardware tracer first, to
/// exercise rebuilding the structures. Skips without ray query.
fn assert_backends_agree(name: &str, gpu: &Gpu, scene: &Scene, camera: &Camera, setup: &Setup, before: Option<&Scene>) {
    if !has_ray_query(gpu) {
        eprintln!("no ray query on this adapter, skipping the hardware comparison");
        return;
    }
    let software = render_with(gpu, scene, camera, setup, RayTracing::Software, None);
    let hardware = render_with(gpu, scene, camera, setup, RayTracing::Hardware, before);
    dump(&format!("{name}-software"), &software);
    dump(&format!("{name}-hardware"), &hardware);
    assert!(hardware.rgba.iter().all(|value| value.is_finite()));
    let (mean, share) = difference(&software, &hardware);
    eprintln!("{name}: mean difference {mean:.5}, clearly different pixels {share:.4}");
    assert!(mean < 0.01, "{name}: mean difference {mean}");
    assert!(share < 0.01, "{name}: {share} of the pixels differ");
}

#[test]
fn the_engine_follows_the_preference_and_the_device() {
    let gpu = gpu_or_skip!();
    assert_eq!(RayTracing::Software.resolve(&gpu.device), TraceEngine::Software);
    let hardware = if has_ray_query(&gpu) {
        TraceEngine::Hardware
    } else {
        TraceEngine::Software
    };
    assert_eq!(RayTracing::Auto.resolve(&gpu.device), hardware);
    assert_eq!(RayTracing::Hardware.resolve(&gpu.device), hardware);
}

#[test]
fn hardware_matches_software_with_instances_mirrors_and_hidden_models() {
    let gpu = gpu_or_skip!();
    let (scene, camera) = instanced_scene();
    let setup = Setup {
        samples: 16,
        ..Setup::default()
    };
    assert_backends_agree("rq-instances", &gpu, &scene, &camera, &setup, None);
}

#[test]
fn hardware_matches_software_on_a_lit_scene_with_glass_and_metal() {
    let gpu = gpu_or_skip!();
    let mut scene = Scene::new();
    // A floor wider than one chunk, so rays cross chunk boundaries and chunks that are empty in between.
    fill(&mut scene, IVec3::new(-40, 0, -40), IVec3::new(40, 0, 40), id(8));
    fill(&mut scene, IVec3::new(-5, 1, -1), IVec3::new(-3, 3, 1), id(1));
    fill(&mut scene, IVec3::new(-1, 1, -1), IVec3::new(1, 3, 1), id(30));
    fill(&mut scene, IVec3::new(30, 1, 30), IVec3::new(33, 6, 33), id(37));
    shade(
        &mut scene,
        1,
        SurfaceConstants {
            transmission: 1.0,
            roughness: 0.0,
            ..SurfaceConstants::MATTE
        },
    );
    shade(
        &mut scene,
        30,
        SurfaceConstants {
            metallic: 1.0,
            roughness: 0.15,
            ..SurfaceConstants::MATTE
        },
    );
    let camera = Camera::looking_at(Vec3::new(2.0, 8.0, 16.0), Vec3::new(0.0, 1.5, 0.0));
    let setup = Setup {
        ground: Ground::Studio,
        samples: 32,
        ..Setup::default()
    };
    assert_backends_agree("rq-lit", &gpu, &scene, &camera, &setup, None);
}

#[test]
fn hardware_matches_software_in_the_styles() {
    let gpu = gpu_or_skip!();
    let (scene, camera) = showpiece();
    for (name, style) in [("toon", RenderStyle::Toon), ("wire", RenderStyle::Wire)] {
        let setup = Setup {
            ground: Ground::Studio,
            samples: 4,
            style,
            ..Setup::default()
        };
        assert_backends_agree(&format!("rq-{name}"), &gpu, &scene, &camera, &setup, None);
    }
}

#[test]
fn hardware_follows_the_scene_when_it_changes() {
    let gpu = gpu_or_skip!();
    let (before, camera) = showpiece();
    // A new chunk (a cube far from the others) and a Model that appears: the structures must be rebuilt.
    let (mut after, _) = showpiece();
    fill(&mut after, IVec3::new(40, 1, 40), IVec3::new(44, 5, 44), id(37));
    apply(
        &mut after,
        &Command::NewModel {
            parent: NodeId::ROOT,
            name: "Another".to_owned(),
        },
    )
    .unwrap();
    let another = after.node(NodeId::ROOT).unwrap().children()[0];
    fill_model(&mut after, another, IVec3::new(8, 1, -2), IVec3::new(10, 3, 0), id(8));
    let setup = Setup {
        ground: Ground::Studio,
        samples: 8,
        ..Setup::default()
    };
    assert_backends_agree("rq-rebuild", &gpu, &after, &camera, &setup, Some(&before));
}

/// `cargo test -p rustium-render --test pathtracer -- --ignored --nocapture benchmark`: times both traversals on a
/// larger scene (a field of towers over a wide floor) and prints milliseconds per sample.
#[test]
#[ignore = "benchmark, run on demand"]
fn benchmark_software_against_hardware() {
    let gpu = gpu_or_skip!();
    if !has_ray_query(&gpu) {
        eprintln!("no ray query on this adapter, skipping");
        return;
    }
    let mut scene = Scene::new();
    fill(&mut scene, IVec3::new(-96, 0, -96), IVec3::new(96, 0, 96), id(8));
    for tower in 0..120_i32 {
        let x = (tower * 37) % 170 - 85;
        let z = (tower * 53) % 170 - 85;
        let height = 4 + (tower * 7) % 28;
        fill(
            &mut scene,
            IVec3::new(x, 1, z),
            IVec3::new(x + 2, height, z + 2),
            id(37),
        );
    }
    let camera = Camera::looking_at(Vec3::new(40.0, 50.0, 110.0), Vec3::new(0.0, 6.0, 0.0));
    let buffers = MaterialBuffers::new(&gpu.device);
    buffers.write_colors(&gpu.queue, scene.palette(), scene.material_graphs());
    let frame = TraceFrame {
        inv_view_proj: camera.view_proj.inverse().to_cols_array_2d(),
        size: (SIZE * 2, SIZE * 2),
        samples: 32,
        bounces: 5,
        sun_turn_degrees: 135.0,
        sun_height_degrees: 45.0,
        sky: Sky::Clear,
        ground: Ground::Studio,
        transparent: false,
        region: None,
        running: true,
        frame_rect: [0.0; 4],
        exposure: EXPOSURE,
        style: RenderStyle::Traced,
        denoise: false,
        lens: None,
        pass: RenderPass::Beauty,
    };
    for (name, mode) in [("software", RayTracing::Software), ("hardware", RayTracing::Hardware)] {
        let mut tracer = PathTracer::new(&gpu.device, &buffers, wgpu::TextureFormat::Rgba8UnormSrgb, mode);
        tracer
            .set_materials(&gpu.device, &gpu.queue, scene.material_graphs())
            .unwrap();
        let started = std::time::Instant::now();
        tracer.upload_scene(&gpu.device, &gpu.queue, &scene);
        gpu.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let upload = started.elapsed().as_secs_f64() * 1000.0;
        // The first render compiles the pipeline's code on first use; time the second one.
        let _ = tracer
            .render_to_image(&gpu.device, &gpu.queue, &frame, |_| true)
            .unwrap();
        let started = std::time::Instant::now();
        let _ = tracer
            .render_to_image(&gpu.device, &gpu.queue, &frame, |_| true)
            .unwrap();
        let per_sample = started.elapsed().as_secs_f64() * 1000.0 / f64::from(frame.samples);
        eprintln!(
            "{name}: upload and build {upload:.1} ms, {per_sample:.1} ms per sample at {}x{}",
            SIZE * 2,
            SIZE * 2
        );
    }
}

/// Gives Color `color` a Material that is transparent haze: no reflection, `density` per voxel, `scattering` as albedo.
fn make_haze(scene: &mut Scene, color: u8, density: f32, scattering: [f32; 3]) -> u32 {
    shade(
        scene,
        color,
        SurfaceConstants {
            transmission: 1.0,
            roughness: 0.0,
            ior: 1.0,
            ..SurfaceConstants::MATTE
        },
    );
    let index = u32::try_from(scene.material_graphs().len()).unwrap() - 1;
    let add = Command::EditMaterial {
        index,
        edit: GraphEdit::AddNode {
            kind: GraphNodeKind::VolumeOutput,
            position: [0.0, 0.0],
        },
    };
    apply(scene, &add).unwrap();
    let node = scene.material_graphs()[index as usize].volume_output().unwrap().id;
    for (input, value) in [(0, InputValue::Float(density)), (1, InputValue::Color(scattering))] {
        let set = Command::EditMaterial {
            index,
            edit: GraphEdit::SetInput { node, input, value },
        };
        apply(scene, &set).unwrap();
    }
    index
}

fn bright_setup() -> Setup {
    Setup {
        sky: Sky::Clear,
        ground: Ground::None,
        sun_height: 45.0,
        samples: 48,
        ..Setup::default()
    }
}

fn box_camera() -> Camera {
    Camera::looking_at(Vec3::new(0.0, 4.0, 18.0), Vec3::new(0.0, 3.0, 0.0))
}

#[test]
fn a_glass_box_refracts_and_stays_plausibly_bright() {
    let gpu = gpu_or_skip!();
    let camera = box_camera();
    let setup = Setup {
        ground: Ground::Studio,
        ..bright_setup()
    };
    let build = |transmission: f32| {
        let mut scene = Scene::new();
        fill(&mut scene, IVec3::new(-3, 1, -3), IVec3::new(3, 6, 3), id(1));
        shade(
            &mut scene,
            1,
            SurfaceConstants {
                transmission,
                roughness: 0.0,
                ..SurfaceConstants::MATTE
            },
        );
        scene
    };
    let glass = render(&gpu, &build(1.0), &camera, &setup);
    dump("glass-box", &glass);
    let opaque = render(&gpu, &build(0.0), &camera, &setup);
    let empty = render(&gpu, &Scene::new(), &camera, &setup);
    let centre = camera.pixel(Vec3::new(0.0, 3.5, 3.5));
    let (through, solid, open) = (
        area_luminance(&glass, centre),
        area_luminance(&opaque, centre),
        area_luminance(&empty, centre),
    );
    assert!(
        (through - solid).abs() > 0.05 * open.max(solid),
        "glass {through} differs from opaque {solid}"
    );
    assert!(
        through > 0.05 * open && through < 3.0 * open.max(0.2),
        "glass {through} against open view {open}"
    );
    assert!(glass.rgba.iter().all(|value| value.is_finite()));
}

#[test]
fn glass_voxels_of_one_material_have_no_faces_between_them() {
    let gpu = gpu_or_skip!();
    let camera = box_camera();
    let setup = Setup {
        ground: Ground::Studio,
        ..bright_setup()
    };
    let build = |split: bool| {
        let mut scene = Scene::new();
        fill(&mut scene, IVec3::new(-3, 1, -3), IVec3::new(3, 6, 3), id(1));
        if split {
            fill(&mut scene, IVec3::new(-3, 1, -3), IVec3::new(3, 6, 0), id(2));
        }
        shade(
            &mut scene,
            1,
            SurfaceConstants {
                transmission: 1.0,
                roughness: 0.0,
                ..SurfaceConstants::MATTE
            },
        );
        let graph = u32::try_from(scene.material_graphs().len()).unwrap() - 1;
        apply(&mut scene, &Command::AssignGraph { id: id(2), graph }).unwrap();
        scene
    };
    let whole = render(&gpu, &build(false), &camera, &setup);
    let split = render(&gpu, &build(true), &camera, &setup);
    let centre = camera.pixel(Vec3::new(0.0, 3.5, 3.5));
    let (a, b) = (area_luminance(&whole, centre), area_luminance(&split, centre));
    // The two Colors have their own tints, so allow for that.
    assert!(b > a * 0.6 && b < a * 1.6, "split {b} against whole {a}");
}

#[test]
fn a_haze_box_dims_what_is_behind_it() {
    let gpu = gpu_or_skip!();
    let camera = box_camera();
    let setup = bright_setup();
    let mut scene = Scene::new();
    fill(&mut scene, IVec3::new(-3, 1, -3), IVec3::new(3, 6, 3), id(1));
    // Pure absorption: the box takes light away and adds none.
    make_haze(&mut scene, 1, 0.5, [0.0, 0.0, 0.0]);
    let hazy = render(&gpu, &scene, &camera, &setup);
    dump("haze-box", &hazy);
    let clear = render(&gpu, &Scene::new(), &camera, &setup);
    let centre = camera.pixel(Vec3::new(0.0, 3.5, 0.0));
    let (dim, open) = (area_luminance(&hazy, centre), area_luminance(&clear, centre));
    // Seven voxels of density 0.5 transmit exp(-3.5), about 3 percent.
    assert!(dim < open * 0.25, "haze {dim} against open sky {open}");
    assert!(hazy.rgba.iter().all(|value| value.is_finite()));
}

#[test]
fn scattering_haze_changes_the_view_through_it() {
    let gpu = gpu_or_skip!();
    let camera = box_camera();
    let setup = Setup {
        ground: Ground::Studio,
        ..bright_setup()
    };
    let mut scene = Scene::new();
    fill(&mut scene, IVec3::new(-3, 1, -3), IVec3::new(3, 6, 3), id(1));
    make_haze(&mut scene, 1, 0.4, [0.9, 0.95, 1.0]);
    let foggy = render(&gpu, &scene, &camera, &setup);
    dump("fog-box", &foggy);
    let empty = render(&gpu, &Scene::new(), &camera, &setup);
    let spot = camera.pixel(Vec3::new(0.0, 3.0, 0.0));
    let (in_fog, open) = (area_luminance(&foggy, spot), area_luminance(&empty, spot));
    assert!(
        (in_fog - open).abs() > 0.05 * open,
        "fog {in_fog} changes the view {open}"
    );
    assert!(foggy.rgba.iter().all(|value| value.is_finite()));
}

#[test]
fn a_material_that_does_not_cast_shadows_leaves_the_floor_lit() {
    let gpu = gpu_or_skip!();
    let camera = Camera::looking_at(Vec3::new(14.0, 14.0, 20.0), Vec3::new(0.0, 2.0, 0.0));
    let setup = Setup {
        ground: Ground::Studio,
        sun_height: 40.0,
        samples: 48,
        ..bright_setup()
    };
    let build = |casts: bool| {
        let mut scene = Scene::new();
        fill(&mut scene, IVec3::new(-12, 0, -12), IVec3::new(12, 0, 12), id(3));
        fill(&mut scene, IVec3::new(-2, 1, -2), IVec3::new(1, 4, 1), id(37));
        shade(&mut scene, 37, SurfaceConstants::MATTE);
        let index = u32::try_from(scene.material_graphs().len()).unwrap() - 1;
        let command = Command::SetCastsShadow {
            index,
            casts_shadow: casts,
        };
        apply(&mut scene, &command).unwrap();
        scene
    };
    let shadowed_spot = camera.pixel(Vec3::new(-4.0, 1.0, 3.0));
    let lit_spot = camera.pixel(Vec3::new(5.0, 1.0, 5.0));
    let with = render(&gpu, &build(true), &camera, &setup);
    let without = render(&gpu, &build(false), &camera, &setup);
    assert!(luminance(&with, shadowed_spot) * 1.5 < luminance(&with, lit_spot));
    dump("noshadow-with", &with);
    dump("noshadow-without", &without);
    let (dark, light) = (luminance(&without, shadowed_spot), luminance(&without, lit_spot));
    assert!(
        dark > light * 0.8,
        "without a shadow the floor stays lit: {dark} against {light}"
    );
}

#[test]
fn an_opaque_material_with_an_empty_volume_output_still_casts_a_shadow() {
    let gpu = gpu_or_skip!();
    let camera = Camera::looking_at(Vec3::new(14.0, 14.0, 20.0), Vec3::new(0.0, 2.0, 0.0));
    let setup = Setup {
        ground: Ground::Studio,
        sun_height: 40.0,
        samples: 48,
        ..bright_setup()
    };
    let mut scene = Scene::new();
    fill(&mut scene, IVec3::new(-12, 0, -12), IVec3::new(12, 0, 12), id(3));
    fill(&mut scene, IVec3::new(-2, 1, -2), IVec3::new(1, 4, 1), id(37));
    shade(&mut scene, 37, SurfaceConstants::MATTE);
    let index = u32::try_from(scene.material_graphs().len()).unwrap() - 1;
    let add = Command::EditMaterial {
        index,
        edit: GraphEdit::AddNode {
            kind: GraphNodeKind::VolumeOutput,
            position: [0.0, 0.0],
        },
    };
    apply(&mut scene, &add).unwrap();
    let image = render(&gpu, &scene, &camera, &setup);
    dump("volume-zero-shadow", &image);
    let shadowed_spot = camera.pixel(Vec3::new(-4.0, 1.0, 3.0));
    let lit_spot = camera.pixel(Vec3::new(5.0, 1.0, 5.0));
    assert!(
        luminance(&image, shadowed_spot) * 1.5 < luminance(&image, lit_spot),
        "an opaque voxel with a zero-density Volume output blocks the sun"
    );
}

#[test]
fn hardware_matches_software_with_haze_and_shadow_free_glass() {
    let gpu = gpu_or_skip!();
    let mut scene = Scene::new();
    fill(&mut scene, IVec3::new(-40, 0, -40), IVec3::new(40, 0, 40), id(8));
    fill(&mut scene, IVec3::new(-3, 1, -3), IVec3::new(3, 5, 3), id(1));
    fill(&mut scene, IVec3::new(30, 1, 30), IVec3::new(33, 6, 33), id(2));
    make_haze(&mut scene, 1, 0.3, [0.9, 0.95, 1.0]);
    shade(
        &mut scene,
        2,
        SurfaceConstants {
            transmission: 1.0,
            roughness: 0.0,
            ..SurfaceConstants::MATTE
        },
    );
    let glass = u32::try_from(scene.material_graphs().len()).unwrap() - 1;
    let command = Command::SetCastsShadow {
        index: glass,
        casts_shadow: false,
    };
    apply(&mut scene, &command).unwrap();
    let camera = Camera::looking_at(Vec3::new(8.0, 9.0, 18.0), Vec3::new(0.0, 2.0, 0.0));
    let setup = Setup {
        ground: Ground::Studio,
        samples: 16,
        ..Setup::default()
    };
    assert_backends_agree("rq-haze", &gpu, &scene, &camera, &setup, None);
}

#[test]
fn glass_resting_on_the_ground_shows_the_studio_floor_through_it() {
    let gpu = gpu_or_skip!();
    // Looking down at a glass block that stands on the floor: the rays leave through the bottom face right at the
    // ground plane and must meet the floor there, not fall through to the sky below the horizon.
    let camera = Camera::looking_at(Vec3::new(0.0, 22.0, 9.0), Vec3::new(0.0, 0.0, 0.0));
    let setup = Setup {
        ground: Ground::Studio,
        sun_height: 60.0,
        samples: 64,
        ..bright_setup()
    };
    let build = |with_glass: bool| {
        let mut scene = Scene::new();
        // A far floor voxel keeps the Scene non-empty; the glass block stands on the ground plane.
        fill(&mut scene, IVec3::new(8, 0, 8), IVec3::new(8, 0, 8), id(3));
        if with_glass {
            fill(&mut scene, IVec3::new(-2, 0, -2), IVec3::new(1, 2, 1), id(1));
            shade(
                &mut scene,
                1,
                SurfaceConstants {
                    transmission: 1.0,
                    roughness: 0.0,
                    ..SurfaceConstants::MATTE
                },
            );
            let index = u32::try_from(scene.material_graphs().len()).unwrap() - 1;
            apply(
                &mut scene,
                &Command::SetCastsShadow {
                    index,
                    casts_shadow: false,
                },
            )
            .unwrap();
        }
        scene
    };
    let spot = camera.pixel(Vec3::new(0.0, 3.0, 0.0));
    let glass = render(&gpu, &build(true), &camera, &setup);
    let bare = render(&gpu, &build(false), &camera, &setup);
    dump("glass-on-ground", &glass);
    let (through, open) = (area_luminance(&glass, spot), area_luminance(&bare, spot));
    assert!(
        through > open * 0.6,
        "the floor seen through grounded glass ({through}) is about as bright as the bare floor ({open})"
    );
    // The sky below the horizon is blue; the studio floor is neutral.
    let [r, _, b] = rgb(&glass, spot);
    assert!(r > b * 0.9, "no blue sky shows through the floor: r {r}, b {b}");
}

/// A tracer with the showpiece Scene uploaded, and a target to draw into.
fn showpiece_tracer(gpu: &Gpu, size: (u32, u32)) -> (PathTracer, wgpu::TextureView, Camera) {
    let (scene, camera) = showpiece();
    let buffers = MaterialBuffers::new(&gpu.device);
    buffers.write_colors(&gpu.queue, scene.palette(), scene.material_graphs());
    let mut tracer = PathTracer::new(
        &gpu.device,
        &buffers,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        RayTracing::Software,
    );
    tracer
        .set_materials(&gpu.device, &gpu.queue, scene.material_graphs())
        .unwrap();
    tracer.upload_scene(&gpu.device, &gpu.queue, &scene);
    tracer.set_time_budget(false);
    let target = gpu
        .device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("test target"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default());
    (tracer, target, camera)
}

fn showpiece_frame(camera: &Camera, size: (u32, u32)) -> TraceFrame {
    TraceFrame {
        inv_view_proj: camera.view_proj.inverse().to_cols_array_2d(),
        size,
        samples: 8,
        bounces: 4,
        sun_turn_degrees: 135.0,
        sun_height_degrees: 45.0,
        sky: Sky::Clear,
        ground: Ground::Studio,
        transparent: false,
        region: None,
        running: true,
        frame_rect: [0.0, 0.0, size.0 as f32, size.1 as f32],
        exposure: EXPOSURE,
        style: RenderStyle::Traced,
        denoise: false,
        lens: None,
        pass: RenderPass::Beauty,
    }
}

/// Encodes frames until the tracer has `frame.samples` samples and reads the picture back.
fn run_to_target(gpu: &Gpu, tracer: &mut PathTracer, target: &wgpu::TextureView, frame: &TraceFrame) -> TraceImage {
    for _ in 0..1000 {
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("test") });
        tracer.encode(&gpu.device, &gpu.queue, &mut encoder, target, frame);
        gpu.queue.submit(std::iter::once(encoder.finish()));
        if tracer.progress().samples >= frame.samples {
            break;
        }
    }
    tracer.read_image(&gpu.device, &gpu.queue).unwrap()
}

#[test]
fn the_render_region_keeps_the_picture_outside_it() {
    let gpu = gpu_or_skip!();
    let (mut tracer, target, camera) = showpiece_tracer(&gpu, (SIZE, SIZE));
    let frame = showpiece_frame(&camera, (SIZE, SIZE));
    let (left, right) = ((SIZE / 4, SIZE / 2), (SIZE * 3 / 4, SIZE / 2));
    // A new picture rendered in the left half only: the right half has nothing yet.
    let left_half = TraceFrame {
        region: Some([0, 0, SIZE / 2, SIZE]),
        ..frame
    };
    let first = run_to_target(&gpu, &mut tracer, &target, &left_half);
    dump("region-left", &first);
    assert!(luminance(&first, left) > 0.0);
    assert!(luminance(&first, right) < 1e-6, "nothing was traced outside the region");
    // Moving only the region to the right half keeps the left half and traces the right half.
    let right_half = TraceFrame {
        region: Some([SIZE / 2, 0, SIZE, SIZE]),
        ..frame
    };
    let second = run_to_target(&gpu, &mut tracer, &target, &right_half);
    dump("region-right", &second);
    assert_eq!(
        rgb(&first, left),
        rgb(&second, left),
        "pixels outside the region keep the earlier picture"
    );
    assert!(luminance(&second, right) > 0.0, "the new region is traced");
    // Changing the view restarts only the region too: what lies outside stays on screen, dimmed by the interface.
    let moved = TraceFrame {
        sun_height_degrees: 20.0,
        ..right_half
    };
    let third = run_to_target(&gpu, &mut tracer, &target, &moved);
    assert_eq!(
        rgb(&second, left),
        rgb(&third, left),
        "outside the region nothing is touched"
    );
    assert!(
        (luminance(&second, right) - luminance(&third, right)).abs() > 1e-4,
        "the region follows the new sun"
    );
}

#[test]
fn a_banded_sweep_covers_every_row() {
    let gpu = gpu_or_skip!();
    // 300 rows do not fit the first band, so one sample takes several frames.
    let (mut tracer, target, camera) = showpiece_tracer(&gpu, (64, 300));
    tracer.set_time_budget(true);
    let frame = TraceFrame {
        samples: 1,
        ..showpiece_frame(&camera, (64, 300))
    };
    let mut frames = 0;
    while tracer.progress().samples < 1 {
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("test") });
        tracer.encode(&gpu.device, &gpu.queue, &mut encoder, &target, &frame);
        gpu.queue.submit(std::iter::once(encoder.finish()));
        frames += 1;
        assert!(frames < 100, "the sweep ends");
    }
    assert!(frames >= 2, "took {frames} frames");
    let image = tracer.read_image(&gpu.device, &gpu.queue).unwrap();
    assert!(luminance(&image, (32, 2)) > 0.0 && luminance(&image, (32, 297)) > 0.0);
    assert!(image.rgba.iter().all(|value| value.is_finite()));
}

#[test]
fn an_image_bigger_than_the_budget_is_refused() {
    let gpu = gpu_or_skip!();
    assert!(rustium_render::check_image_size(&gpu.device, (1920, 1080)).is_ok());
    let error = rustium_render::check_image_size(&gpu.device, (u32::MAX, 16)).unwrap_err();
    assert!(matches!(error, rustium_render::Error::ImageTooLarge { .. }));
    let (_, max_bytes) = rustium_render::trace_image_limits(&gpu.device);
    let side = u32::try_from((max_bytes / 112).isqrt() + 64).unwrap();
    assert!(rustium_render::trace_image_bytes((side, side)) > max_bytes);
    assert!(rustium_render::check_image_size(&gpu.device, (side, side)).is_err());
}

#[test]
fn clay_draws_glass_as_a_solid_that_casts_a_shadow() {
    let gpu = gpu_or_skip!();
    let camera = Camera::looking_at(Vec3::new(14.0, 14.0, 20.0), Vec3::new(0.0, 2.0, 0.0));
    let mut scene = Scene::new();
    fill(&mut scene, IVec3::new(-12, 0, -12), IVec3::new(12, 0, 12), id(3));
    fill(&mut scene, IVec3::new(-2, 1, -2), IVec3::new(1, 4, 1), id(1));
    shade(
        &mut scene,
        1,
        SurfaceConstants {
            transmission: 1.0,
            roughness: 0.0,
            ..SurfaceConstants::MATTE
        },
    );
    let index = u32::try_from(scene.material_graphs().len()).unwrap() - 1;
    apply(
        &mut scene,
        &Command::SetCastsShadow {
            index,
            casts_shadow: false,
        },
    )
    .unwrap();
    let shadowed_spot = camera.pixel(Vec3::new(-4.0, 1.0, 3.0));
    let lit_spot = camera.pixel(Vec3::new(5.0, 1.0, 5.0));
    let setup = Setup {
        ground: Ground::Studio,
        sun_height: 40.0,
        samples: 48,
        ..bright_setup()
    };
    let traced = render(&gpu, &scene, &camera, &setup);
    let clay = render(
        &gpu,
        &scene,
        &camera,
        &Setup {
            style: RenderStyle::Clay,
            ..setup
        },
    );
    dump("clay-glass-shadow", &clay);
    assert!(
        luminance(&traced, shadowed_spot) > luminance(&traced, lit_spot) * 0.8,
        "traced glass that casts no shadow leaves the floor lit"
    );
    assert!(
        luminance(&clay, shadowed_spot) * 1.5 < luminance(&clay, lit_spot),
        "clay makes the glass solid, so it shades the floor like any other block"
    );
}

#[test]
fn the_depth_pass_spans_the_models_from_near_to_far() {
    let gpu = gpu_or_skip!();
    let (scene, camera) = showpiece();
    let depth = render(
        &gpu,
        &scene,
        &camera,
        &Setup {
            samples: 6,
            pass: RenderPass::Depth,
            ground: Ground::Studio,
            ..Setup::default()
        },
    );
    let near = luminance(&depth, camera.pixel(Vec3::new(9.0, 0.0, 9.0)));
    let far = luminance(&depth, camera.pixel(Vec3::new(-9.0, 0.0, -9.0)));
    assert!((0.0..=1.0).contains(&near) && (0.0..=1.0).contains(&far));
    assert!(
        near > far + 0.3,
        "the near floor corner ({near}) is much brighter than the far one ({far}): the range spans the Models"
    );
}

/// Size of the images of the interactive preview tests: taller than the first band of a sweep, so a changed view
/// cannot be traced in one frame.
const PREVIEW_SIZE: u32 = 256;

fn preview_frame(camera: &Camera, samples: u32) -> TraceFrame {
    TraceFrame {
        inv_view_proj: camera.view_proj.inverse().to_cols_array_2d(),
        size: (PREVIEW_SIZE, PREVIEW_SIZE),
        samples,
        bounces: 3,
        sun_turn_degrees: 135.0,
        sun_height_degrees: 45.0,
        sky: Sky::Clear,
        ground: Ground::None,
        transparent: false,
        region: None,
        running: true,
        frame_rect: [0.0, 0.0, PREVIEW_SIZE as f32, PREVIEW_SIZE as f32],
        exposure: EXPOSURE,
        style: RenderStyle::Traced,
        denoise: true,
        lens: None,
        pass: RenderPass::Beauty,
    }
}

/// The display brightness of every pixel the tracer drew into `target`.
fn picture_of(gpu: &Gpu, target: &wgpu::Texture) -> Vec<f32> {
    let row_bytes = PREVIEW_SIZE * 4;
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("test readback"),
        size: u64::from(row_bytes * PREVIEW_SIZE),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("test") });
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row_bytes),
                rows_per_image: Some(PREVIEW_SIZE),
            },
        },
        wgpu::Extent3d {
            width: PREVIEW_SIZE,
            height: PREVIEW_SIZE,
            depth_or_array_layers: 1,
        },
    );
    gpu.queue.submit(std::iter::once(encoder.finish()));
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |result| result.unwrap());
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let data = slice.get_mapped_range().unwrap();
    let mut picture = Vec::new();
    for pixel in data.as_chunks::<4>().0 {
        picture.push((f32::from(pixel[0]) + f32::from(pixel[1]) + f32::from(pixel[2])) / (3.0 * 255.0));
    }
    picture
}

/// Mean absolute difference of two pictures within one horizontal third.
fn third_distance(a: &[f32], b: &[f32], third: usize) -> f32 {
    let size = (PREVIEW_SIZE * PREVIEW_SIZE) as usize;
    let range = third * size / 3..(third + 1) * size / 3;
    let total: f32 = a[range.clone()]
        .iter()
        .zip(&b[range.clone()])
        .map(|(a, b)| (a - b).abs())
        .sum();
    total / range.len() as f32
}

fn preview_target(gpu: &Gpu) -> wgpu::Texture {
    gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("test target"),
        size: wgpu::Extent3d {
            width: PREVIEW_SIZE,
            height: PREVIEW_SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

fn encode_frame(gpu: &Gpu, tracer: &mut PathTracer, target: &wgpu::Texture, frame: &TraceFrame) {
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("test") });
    tracer.encode(&gpu.device, &gpu.queue, &mut encoder, &view, frame);
    gpu.queue.submit(std::iter::once(encoder.finish()));
}

#[test]
fn a_changed_view_never_shows_the_old_one_in_part_of_the_picture() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let (scene, _) = showpiece();
    // Looking down at the floor, then at the sky: every third of the picture differs clearly from the view of the cubes
    // and the floor.
    let camera_a = Camera::looking_at(Vec3::new(0.0, 30.0, 2.0), Vec3::new(0.0, 0.0, 0.0));
    let camera_b = Camera::looking_at(Vec3::new(14.0, 14.0, 20.0), Vec3::new(14.0, 60.0, -30.0));
    let buffers = MaterialBuffers::new(&gpu.device);
    buffers.write_colors(&gpu.queue, scene.palette(), scene.material_graphs());
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let new_tracer = || {
        let mut tracer = PathTracer::new(&gpu.device, &buffers, format, RayTracing::Software);
        tracer
            .set_materials(&gpu.device, &gpu.queue, scene.material_graphs())
            .unwrap();
        tracer.upload_scene(&gpu.device, &gpu.queue, &scene);
        tracer
    };

    // What the view B looks like when the whole picture is traced at once.
    let mut reference = new_tracer();
    reference.set_time_budget(false);
    let target = preview_target(&gpu);
    encode_frame(&gpu, &mut reference, &target, &preview_frame(&camera_b, 1));
    let wanted = picture_of(&gpu, &target);

    let mut tracer = new_tracer();
    let frame_a = preview_frame(&camera_a, 4);
    for _ in 0..40 {
        encode_frame(&gpu, &mut tracer, &target, &frame_a);
        if tracer.progress().samples >= 1 {
            break;
        }
    }
    let old = picture_of(&gpu, &target);
    for third in 0..3 {
        assert!(
            third_distance(&old, &wanted, third) > 0.05,
            "the two views must differ in third {third}"
        );
    }

    // Every frame after the camera moved, until the first sample of the new view is complete, shows view B all over.
    let frame_b = preview_frame(&camera_b, 4);
    for step in 0..40 {
        encode_frame(&gpu, &mut tracer, &target, &frame_b);
        let shown = picture_of(&gpu, &target);
        for third in 0..3 {
            assert!(
                third_distance(&shown, &wanted, third) < third_distance(&shown, &old, third),
                "frame {step}: third {third} still shows the old view"
            );
        }
        if tracer.progress().samples >= 1 {
            break;
        }
    }
    assert!(
        tracer.progress().samples >= 1,
        "the first sample of the new view completes"
    );
}

/// Save image, the Shot queue and turntables trace with a snapshot of the material buffers; the copy must be valid.
#[test]
fn snapshotting_the_material_buffers_raises_no_validation_error() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter; skipped");
        return;
    };
    let buffers = MaterialBuffers::new(&gpu.device);
    let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let _copy = buffers.snapshot(&gpu.device, &gpu.queue);
    let error = pollster::block_on(scope.pop());
    assert!(error.is_none(), "{error:?}");
}

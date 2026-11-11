//! Renders tiny scenes headless on the real GPU and checks what the path tracer produces. Skipped (with a note)
//! when the machine has no adapter. Set `RUSTIUM_DUMP_DIR` to also write the tonemapped images as PNG files.
#![expect(
    clippy::unwrap_used,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::many_single_char_names,
    reason = "test code"
)]

use glam::{IVec3, Mat4, Vec3};
use rustium_core::{
    Brush, ColorId, Command, Face, HitTarget, Mirror, NodeId, Orientation, RayHit, Scene, SurfaceConstants, Transform,
    VoxelTool, apply,
};
use rustium_render::{
    GpuMaterialTable, Ground, PathTracer, RenderPass, RenderStyle, Sky, ThinLens, TraceFrame, TraceImage,
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
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
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
        }
    }
}

fn render(gpu: &Gpu, scene: &Scene, camera: &Camera, setup: &Setup) -> TraceImage {
    let table = GpuMaterialTable::new(scene.palette(), scene.material_graphs());
    let materials = wgpu::util::DeviceExt::create_buffer_init(
        &gpu.device,
        &wgpu::util::BufferInitDescriptor {
            label: Some("test materials"),
            contents: bytemuck::bytes_of(&table),
            usage: wgpu::BufferUsages::UNIFORM,
        },
    );
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut tracer = PathTracer::new(&gpu.device, &materials, format);
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
        region: None,
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
    let mapped = ((linear * (2.51 * linear + 0.03)) / (linear * (2.43 * linear + 0.59) + 0.14)).clamp(0.0, 1.0);
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
        let mapped = ((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14)).clamp(0.0, 1.0);
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

#[test]
fn models_are_placed_by_their_transforms_and_hidden_ones_are_skipped() {
    let gpu = gpu_or_skip!();
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

    let camera = Camera::looking_at(Vec3::new(5.0, 28.0, 6.0), Vec3::new(5.0, 0.0, -1.0));
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
    assert!(lines > 40, "edges are drawn, found {lines} bright pixels");
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
fn offline_rendering_reaches_the_target_and_can_be_cancelled() {
    let gpu = gpu_or_skip!();
    let (scene, camera) = showpiece();
    let table = GpuMaterialTable::new(scene.palette(), scene.material_graphs());
    let materials = wgpu::util::DeviceExt::create_buffer_init(
        &gpu.device,
        &wgpu::util::BufferInitDescriptor {
            label: Some("test materials"),
            contents: bytemuck::bytes_of(&table),
            usage: wgpu::BufferUsages::UNIFORM,
        },
    );
    let mut tracer = PathTracer::new(&gpu.device, &materials, wgpu::TextureFormat::Rgba8UnormSrgb);
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

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
    Brush, Command, Face, HitTarget, MaterialId, Mirror, NodeId, Orientation, RayHit, Scene, SurfaceConstants,
    Transform, VoxelTool, apply,
};
use rustium_render::{GpuMaterialTable, Ground, PathTracer, Sky, TraceFrame, TraceImage};

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

fn id(number: u8) -> MaterialId {
    MaterialId::new(number).unwrap()
}

fn first_model(scene: &Scene) -> NodeId {
    scene.node(NodeId::ROOT).unwrap().children()[0]
}

/// Fills a box of the first Model.
fn fill(scene: &mut Scene, min: IVec3, max: IVec3, material: MaterialId) {
    let model = first_model(scene);
    fill_model(scene, model, min, max, material);
}

fn fill_model(scene: &mut Scene, model: NodeId, min: IVec3, max: IVec3, material: MaterialId) {
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
                    material,
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
    };
    let image = render(&gpu, &scene, &camera, &setup);
    dump("materials", &image);
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

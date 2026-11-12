//! Parses and validates every WGSL shader with naga, so shader mistakes fail `cargo test` instead of the app.
//! The Material shader is generated, so it is validated for the Default material and for every sample Material.

use rustium_core::{GraphEdit, GraphNodeKind, InputValue, MaterialGraph, SampleMaterial};
use rustium_render::compile_materials;
use wgpu::naga;

fn validate(name: &str, source: &str) {
    validate_with(name, source, naga::valid::Capabilities::default());
}

fn validate_with(name: &str, source: &str, capabilities: naga::valid::Capabilities) {
    let module =
        naga::front::wgsl::parse_str(source).unwrap_or_else(|error| panic!("{name}: {}", error.emit_to_string(source)));
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), capabilities)
        .validate(&module)
        .unwrap_or_else(|error| panic!("{name}: {}", error.emit_to_string(source)));
}

/// The Default material plus every sample Material, the way a Scene would hold them.
fn sample_graphs() -> Vec<MaterialGraph> {
    let mut graphs = vec![MaterialGraph::default_material()];
    graphs.extend(SampleMaterial::ALL.map(SampleMaterial::build));
    graphs
}

fn material_source(graphs: &[MaterialGraph]) -> String {
    compile_materials(graphs).expect("graphs compile").shader_source()
}

#[test]
fn the_material_module_is_valid_for_the_default_and_every_sample_material() {
    validate(
        "default material",
        &material_source(&[MaterialGraph::default_material()]),
    );
    validate("all samples", &material_source(&sample_graphs()));
    for sample in SampleMaterial::ALL {
        let graphs = [MaterialGraph::default_material(), sample.build()];
        validate(sample.label(), &material_source(&graphs));
    }
}

#[test]
fn every_node_kind_and_bypass_combination_compiles_to_valid_wgsl() {
    // One graph that holds every kind of node feeding the outputs, then the same with every node bypassed.
    let mut graph = MaterialGraph::blank("Everything");
    let surface = graph.surface_output().expect("surface").id;
    let mut previous: Option<(rustium_core::GraphNodeId, u8)> = None;
    for kind in GraphNodeKind::ALL {
        if kind.is_output() {
            continue;
        }
        let id = graph.next_node_id();
        graph
            .apply_edit(&GraphEdit::AddNode {
                kind,
                position: [0.0, 0.0],
            })
            .expect("add");
        // Chain whatever the node can read from the previous node, and every output into a Surface input.
        let spec = kind.spec();
        if let (Some((from, socket)), true) = (previous, !spec.inputs.is_empty()) {
            graph
                .apply_edit(&GraphEdit::Connect {
                    from: rustium_core::OutputRef { node: from, socket },
                    to: rustium_core::InputRef { node: id, socket: 0 },
                })
                .expect("chain");
        }
        for (index, _) in spec.outputs.iter().enumerate() {
            let socket = u8::try_from(index).expect("small");
            let target = u8::try_from((id.get() as usize + index) % 8).expect("small");
            if target != 0 {
                graph
                    .apply_edit(&GraphEdit::Connect {
                        from: rustium_core::OutputRef { node: id, socket },
                        to: rustium_core::InputRef {
                            node: surface,
                            socket: target,
                        },
                    })
                    .expect("to surface");
            }
        }
        if !spec.outputs.is_empty() {
            previous = Some((id, 0));
        }
    }
    let volume = graph.next_node_id();
    graph
        .apply_edit(&GraphEdit::AddNode {
            kind: GraphNodeKind::VolumeOutput,
            position: [0.0, 0.0],
        })
        .expect("volume");
    graph
        .apply_edit(&GraphEdit::SetInput {
            node: volume,
            input: 0,
            value: InputValue::Float(0.5),
        })
        .expect("density");
    validate(
        "everything",
        &material_source(&[MaterialGraph::default_material(), graph.clone()]),
    );

    for node in graph.nodes().iter().map(|node| node.id).collect::<Vec<_>>() {
        graph
            .apply_edit(&GraphEdit::SetBypass { node, bypass: true })
            .expect("bypass");
    }
    validate(
        "everything bypassed",
        &material_source(&[MaterialGraph::default_material(), graph]),
    );
}

#[test]
fn scene_shaders_are_valid_wgsl() {
    let material = material_source(&sample_graphs());
    let common = include_str!("../src/shaders/common.wgsl");
    validate(
        "raster",
        &format!("{material}{common}{}", include_str!("../src/shaders/raster.wgsl")),
    );
    for (name, source) in [
        ("grid", include_str!("../src/shaders/grid.wgsl")),
        ("lines", include_str!("../src/shaders/lines.wgsl")),
    ] {
        validate(name, &format!("{common}{source}"));
    }
}

#[test]
fn post_shaders_are_valid_wgsl() {
    let fullscreen = include_str!("../src/shaders/fullscreen.wgsl");
    validate(
        "blur",
        &format!("{fullscreen}{}", include_str!("../src/shaders/blur.wgsl")),
    );
    validate(
        "composite",
        &format!("{fullscreen}{}", include_str!("../src/shaders/composite.wgsl")),
    );
}

#[test]
fn path_tracer_shaders_are_valid_wgsl() {
    let body = format!(
        "{}{}{}",
        material_source(&sample_graphs()),
        include_str!("../src/shaders/pathtracer.wgsl"),
        include_str!("../src/shaders/trace_style.wgsl")
    );
    validate(
        "pathtracer (dda)",
        &format!("{body}{}", include_str!("../src/shaders/trace_dda.wgsl")),
    );
    validate_with(
        "pathtracer (ray query)",
        &format!(
            "enable wgpu_ray_query;\n{body}{}",
            include_str!("../src/shaders/trace_rq.wgsl")
        ),
        naga::valid::Capabilities::default() | naga::valid::Capabilities::RAY_QUERY,
    );
    validate("resolve", include_str!("../src/shaders/resolve.wgsl"));
    validate(
        "present",
        &format!(
            "{}{}",
            include_str!("../src/shaders/fullscreen.wgsl"),
            include_str!("../src/shaders/present.wgsl")
        ),
    );
}

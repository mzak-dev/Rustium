//! Parses and validates every WGSL shader with naga, so shader mistakes fail `cargo test` instead of the app.

use wgpu::naga;

fn validate(name: &str, source: &str) {
    let module =
        naga::front::wgsl::parse_str(source).unwrap_or_else(|error| panic!("{name}: {}", error.emit_to_string(source)));
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
    .validate(&module)
    .unwrap_or_else(|error| panic!("{name}: {error:?}"));
}

#[test]
fn scene_shaders_are_valid_wgsl() {
    let common = include_str!("../src/shaders/common.wgsl");
    for (name, source) in [
        ("raster", include_str!("../src/shaders/raster.wgsl")),
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

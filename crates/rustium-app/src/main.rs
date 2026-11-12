//! The `rustium` desktop binary; the editor itself lives in the `rustium_app` library.

#[cfg(not(target_arch = "wasm32"))]
fn main() -> color_eyre::eyre::Result<()> {
    rustium_app::run_native()
}

/// The browser build starts through `crates/rustium-web`, not through this binary.
#[cfg(target_arch = "wasm32")]
fn main() {}

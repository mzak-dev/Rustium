//! The browser build: the event loop on a `<canvas>`, browser storage, file pickers and downloads.
//!
//! `crates/rustium-web` installs the panic hook and then calls [`run`]. Everything else in this module is what the rest
//! of the editor uses in place of the desktop's files and dialogs.

use color_eyre::eyre::{Result, WrapErr, eyre};
use wasm_bindgen::JsCast;
use winit::event_loop::EventLoop;
use winit::platform::web::{EventLoopExtWebSys, WindowAttributesExtWebSys};
use winit::window::WindowAttributes;

use crate::{app, logging, settings};

/// Id of the `<canvas>` in `web/index.html` that the editor draws into.
const CANVAS_ID: &str = "rustium";

/// Id of the element in `web/index.html` that shows a fatal error instead of the editor.
const ERROR_ID: &str = "rustium-error";

/// Sends logs to the browser console and starts the editor in the page's `<canvas id="rustium">`. Returns at once: the
/// browser drives the event loop.
///
/// # Errors
///
/// Fails when logging or the event loop cannot be set up.
pub fn run() -> Result<()> {
    logging::init().wrap_err("cannot start logging")?;
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "starting Rustium in the browser");
    let event_loop = EventLoop::with_user_event()
        .build()
        .wrap_err("cannot create the event loop")?;
    let app = app::App::new(settings::Settings::load(), event_loop.create_proxy(), None);
    event_loop.spawn_app(app);
    Ok(())
}

/// Window attributes that draw into the page's canvas, which the page sizes with CSS.
pub(crate) fn canvas_attributes(attributes: WindowAttributes) -> Result<WindowAttributes> {
    let canvas = document()?
        .get_element_by_id(CANVAS_ID)
        .ok_or_else(|| eyre!("the page has no <canvas id=\"{CANVAS_ID}\">"))?
        .dyn_into::<web_sys::HtmlCanvasElement>()
        .map_err(|_| eyre!("#{CANVAS_ID} is not a <canvas>"))?;
    Ok(attributes
        .with_canvas(Some(canvas))
        // Browser shortcuts such as Ctrl+S must reach the editor.
        .with_prevent_default(true)
        .with_focusable(true))
}

/// Replaces the editor with `message`, for errors it cannot recover from (no WebGPU, no device).
pub(crate) fn show_fatal_error(message: &str) {
    let Ok(document) = document() else {
        return;
    };
    if let Some(element) = document.get_element_by_id(ERROR_ID) {
        element.set_text_content(Some(message));
        let _ = element.set_attribute("data-visible", "true");
    }
}

/// The value stored under `key` in the browser's local storage.
pub(crate) fn storage_get(key: &str) -> Option<String> {
    local_storage()?.get_item(key).ok().flatten()
}

/// Stores `value` under `key` in the browser's local storage.
///
/// # Errors
///
/// The browser refused (storage disabled or full).
pub(crate) fn storage_set(key: &str, value: &str) -> Result<()> {
    local_storage()
        .ok_or_else(|| eyre!("local storage is not available"))?
        .set_item(key, value)
        .map_err(|error| eyre!("the browser refused to store {key}: {error:?}"))
}

/// Hands `bytes` to the browser as a download named `file_name`.
///
/// # Errors
///
/// The browser could not create the file or the link that saves it.
pub(crate) fn download(file_name: &str, bytes: &[u8]) -> Result<()> {
    let document = document()?;
    let array = js_sys::Uint8Array::from(bytes);
    let parts = js_sys::Array::of1(&array);
    let options = web_sys::BlobPropertyBag::new();
    options.set_type("application/octet-stream");
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &options)
        .map_err(|error| eyre!("cannot create the file: {error:?}"))?;
    let url =
        web_sys::Url::create_object_url_with_blob(&blob).map_err(|error| eyre!("cannot link the file: {error:?}"))?;
    let anchor = document
        .create_element("a")
        .map_err(|error| eyre!("cannot create a link: {error:?}"))?
        .dyn_into::<web_sys::HtmlAnchorElement>()
        .map_err(|_| eyre!("<a> is not an anchor"))?;
    anchor.set_href(&url);
    anchor.set_download(file_name);
    anchor.click();
    // The click has started the download, which holds its own reference to the file.
    let _ = web_sys::Url::revoke_object_url(&url);
    Ok(())
}

/// Asks for one file with `extensions`; resolves to its name and contents, or `None` when the picker was dismissed.
pub(crate) async fn pick_file(
    title: String,
    filter: String,
    extensions: &'static [&'static str],
) -> Option<(String, Vec<u8>)> {
    let file = rfd::AsyncFileDialog::new()
        .set_title(title)
        .add_filter(filter, extensions)
        .pick_file()
        .await?;
    let bytes = file.read().await;
    Some((file.file_name(), bytes))
}

fn document() -> Result<web_sys::Document> {
    web_sys::window()
        .and_then(|window| window.document())
        .ok_or_else(|| eyre!("no browser document"))
}

fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

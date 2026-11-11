//! Saving the traced image: ACES-tonemapped 8-bit PNG or raw linear EXR, picked through a native file dialog.
//!
//! `rfd` is the one dialog crate used because it is a thin wrapper over the OS dialogs (on Windows just
//! `windows-sys`), so it adds no runtime and almost no build time. The dialog and the encoding run on a worker
//! thread so the interface keeps drawing.

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use color_eyre::eyre::{Result, WrapErr};
use rustium_render::TraceImage;
use tracing::{error, info};

use crate::render_mode::ImageFormat;

/// How a Save image request ended.
#[derive(Debug)]
pub(crate) enum SaveOutcome {
    Saved(PathBuf),
    /// A queue wrote `count` files into `folder`.
    Batch {
        folder: PathBuf,
        count: usize,
    },
    /// `ffmpeg` is missing, so the video's frames are PNG files in this folder.
    Sequence(PathBuf),
    Cancelled,
    Failed(String),
}

/// Narkowicz's ACES fit, matching `present.wgsl`, clamped to `0..=1`.
fn aces(value: f32) -> f32 {
    ((value * (2.51 * value + 0.03)) / (value * (2.43 * value + 0.59) + 0.14)).clamp(0.0, 1.0)
}

/// `0.0..=1.0` to a byte.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to 0..=1 and rounded before the cast"
)]
fn unit_to_u8(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Linear `0..=1` to the 8-bit sRGB transfer curve.
fn srgb8(linear: f32) -> u8 {
    let encoded = if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    };
    unit_to_u8(encoded)
}

/// Tonemaps the accumulated radiance (times `exposure`) to 8-bit sRGB: RGBA with straight alpha when `with_alpha`, else
/// RGB.
fn to_bytes(image: &TraceImage, with_alpha: bool, exposure: f32) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(image.rgba.len());
    for texel in image.rgba.as_chunks::<4>().0 {
        let [r, g, b, a] = [texel[0], texel[1], texel[2], texel[3]];
        // The accumulator holds premultiplied radiance; PNG wants straight color.
        let unpremultiply = if with_alpha && a > 1e-4 { 1.0 / a } else { 1.0 };
        bytes.extend([r, g, b].map(|channel| {
            let linear = channel * unpremultiply;
            // AOV passes are display colors already.
            srgb8(if image.display_ready {
                linear
            } else {
                aces(linear * exposure)
            })
        }));
        if with_alpha {
            bytes.push(unit_to_u8(a));
        }
    }
    bytes
}

/// The image as tightly packed 8-bit RGB, one frame of a video.
pub(crate) fn rgb_bytes(image: &TraceImage, exposure: f32) -> Vec<u8> {
    to_bytes(image, false, exposure)
}

/// Writes `image` as an 8-bit PNG, with an alpha channel when `with_alpha`.
pub(crate) fn write_png(path: &Path, image: &TraceImage, with_alpha: bool, exposure: f32) -> Result<()> {
    let file = BufWriter::new(File::create(path).wrap_err("cannot create the file")?);
    let mut encoder = png::Encoder::new(file, image.width, image.height);
    encoder.set_color(if with_alpha {
        png::ColorType::Rgba
    } else {
        png::ColorType::Rgb
    });
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().wrap_err("cannot write the PNG header")?;
    writer
        .write_image_data(&to_bytes(image, with_alpha, exposure))
        .wrap_err("cannot write the PNG data")?;
    writer.finish().wrap_err("cannot finish the PNG")
}

/// Writes `image` as a 32-bit float RGBA `OpenEXR` with the raw linear radiance (premultiplied alpha).
pub(crate) fn write_exr(path: &Path, image: &TraceImage) -> Result<()> {
    let width = image.width as usize;
    exr::prelude::write_rgba_file(path, width, image.height as usize, |x, y| {
        let index = (y * width + x) * 4;
        match image.rgba.get(index..index + 4) {
            Some([r, g, b, a]) => (*r, *g, *b, *a),
            _ => (0.0, 0.0, 0.0, 0.0),
        }
    })
    .wrap_err("cannot write the EXR")
}

/// Asks for a file name and writes `image` on a worker thread; the result arrives on `done`.
pub(crate) fn save_with_dialog(
    image: TraceImage,
    format: ImageFormat,
    transparent: bool,
    exposure: f32,
    done: Sender<SaveOutcome>,
) {
    let spawned = std::thread::Builder::new()
        .name("save-image".to_owned())
        .spawn(move || {
            let (label, extension) = match format {
                ImageFormat::Png | ImageFormat::Mp4 => ("PNG", "png"),
                ImageFormat::Exr => ("OpenEXR", "exr"),
            };
            let Some(path) = rfd::FileDialog::new()
                .add_filter(label, &[extension])
                .set_file_name(format!("render.{extension}"))
                .save_file()
            else {
                let _ = done.send(SaveOutcome::Cancelled);
                return;
            };
            let path = path.with_extension(extension);
            let result = match format {
                ImageFormat::Png | ImageFormat::Mp4 => write_png(&path, &image, transparent, exposure),
                ImageFormat::Exr => write_exr(&path, &image),
            };
            let outcome = match result {
                Ok(()) => {
                    info!(path = %path.display(), "saved image");
                    SaveOutcome::Saved(path)
                }
                Err(report) => {
                    error!(error = ?report, "cannot save the image");
                    SaveOutcome::Failed(format!("{report:#}"))
                }
            };
            let _ = done.send(outcome);
        });
    if let Err(error) = spawned {
        error!(%error, "cannot start the save thread");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(width: u32, height: u32) -> TraceImage {
        let mut rgba = Vec::new();
        for y in 0..height {
            for x in 0..width {
                rgba.extend([x as f32 / width as f32, y as f32 / height as f32, 0.25, 1.0]);
            }
        }
        TraceImage {
            width,
            height,
            samples: 8,
            rgba,
            display_ready: false,
        }
    }

    fn temp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rustium-test-{}-{name}", std::process::id()))
    }

    #[test]
    fn tonemapping_is_monotonic_and_stays_in_range() {
        let mut previous = 0;
        for step in 0..=80 {
            let value = srgb8(aces(step as f32 * 0.1));
            assert!(value >= previous);
            previous = value;
        }
        assert_eq!(srgb8(aces(0.0)), 0);
        assert_eq!(srgb8(aces(1000.0)), 255);
    }

    #[test]
    fn png_round_trips_size_and_pixels() {
        let image = gradient(8, 4);
        let path = temp("opaque.png");
        write_png(&path, &image, false, 1.0).unwrap();
        let mut reader = png::Decoder::new(std::io::BufReader::new(File::open(&path).unwrap()))
            .read_info()
            .unwrap();
        let mut buffer = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buffer).unwrap();
        assert_eq!((info.width, info.height), (8, 4));
        assert_eq!(info.color_type, png::ColorType::Rgb);
        assert_eq!(buffer[..info.buffer_size()], to_bytes(&image, false, 1.0)[..]);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn transparent_png_keeps_coverage_as_straight_alpha() {
        let image = TraceImage {
            width: 1,
            height: 1,
            samples: 1,
            // Half-covered pixel: premultiplied radiance 0.25 at alpha 0.5 is straight radiance 0.5.
            rgba: vec![0.25, 0.25, 0.25, 0.5],
            display_ready: false,
        };
        let bytes = to_bytes(&image, true, 1.0);
        assert_eq!(bytes[0], srgb8(aces(0.5)));
        assert_eq!(bytes[3], 128);
    }

    #[test]
    fn exr_round_trips_linear_radiance() {
        let image = gradient(8, 4);
        let path = temp("linear.exr");
        write_exr(&path, &image).unwrap();
        let loaded = exr::prelude::read_first_rgba_layer_from_file(
            &path,
            |resolution, _| vec![(0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32); resolution.width() * resolution.height()],
            |pixels, position, (r, g, b, a): (f32, f32, f32, f32)| {
                pixels[position.y() * 8 + position.x()] = (r, g, b, a);
            },
        )
        .unwrap();
        let pixels = &loaded.layer_data.channel_data.pixels;
        let expected = &image.rgba;
        for (index, pixel) in pixels.iter().enumerate() {
            assert!((pixel.0 - expected[index * 4]).abs() < 1e-3, "red at {index}");
            assert!((pixel.1 - expected[index * 4 + 1]).abs() < 1e-3, "green at {index}");
            assert!((pixel.3 - 1.0).abs() < 1e-6);
        }
        let _ = std::fs::remove_file(path);
    }
}

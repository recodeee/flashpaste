//! Auto-compress screenshots so they don't blow past Claude Code's
//! attachment limit.
//!
//! Claude Code rejects (or silently drops) image attachments above a few
//! megabytes. 4K-multimon PNGs routinely come in at 8–15 MB. Re-encoding
//! to WebP at the same visual fidelity gets us under the cap and keeps
//! transfer to the model snappy.
//!
//! The function lives here in `flashpaste-common` (rather than inside the
//! daemon) so anything that stages an image from a file can opt in
//! cheaply — the screenshot CLI, the daemon's inotify watcher, the
//! future MCP wrapper, etc.
//!
//! ## Policy
//!
//! 1. If the file is already ≤ `max_bytes`, return its bytes as-is. The
//!    common case (small PNG from PrtScr) avoids the decode/encode trip
//!    entirely.
//! 2. Otherwise: load via the `image` crate, downscale so the longest
//!    side is ≤ `max_dim` (preserving aspect ratio), re-encode as
//!    `image/webp` at quality 80. If the WebP encoder errors out (bad
//!    build, weird pixel format), fall back to JPEG quality 85.
//! 3. Caller decides what to do with the returned `(bytes, mime)` — most
//!    commonly write to a tmpfile and pass the new path on.
//!
//! ## Why WebP first
//!
//! At quality 80, WebP routinely produces 3–5× smaller files than PNG
//! and 1.5–2× smaller than JPEG quality 85, with no visible artefacts
//! on UI screenshots (sharp text, flat backgrounds — exactly the
//! content WebP handles best). JPEG is the fallback because (a) every
//! consumer understands it and (b) the `image` crate ships its own
//! native JPEG encoder so we can't lose the encoder to a missing system
//! library.
//!
//! ## Defaults
//!
//! * `max_bytes = 4 MB` — comfortably below Claude Code's attachment cap.
//! * `max_dim = 2400` — long-side. Big enough that UI screenshots stay
//!   readable; small enough that a 4K dual-monitor capture (~7680×2160)
//!   compresses to a sane size after the downscale.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use image::imageops::FilterType;
use image::{DynamicImage, ImageEncoder};

/// Sensible default: 4 MB. Claude Code's attachment cap sits a bit
/// higher than this, but we want headroom for HTTP framing and the
/// JSON-RPC wrapper around the bytes.
pub const DEFAULT_MAX_BYTES: usize = 4 * 1024 * 1024;

/// Sensible default: long-side 2400 px. Anything larger is downscaled
/// before encoding.
pub const DEFAULT_MAX_DIM: u32 = 2400;

/// Compress `path` for use as a Claude / agent attachment.
///
/// Returns `(bytes, mime)` where `mime` is `"image/png"` for the
/// pass-through branch and `"image/webp"` / `"image/jpeg"` for the
/// re-encode branches.
///
/// `max_bytes`: hard ceiling — files at or below this are returned
/// untouched. `max_dim`: longest side after downscale (only applied on
/// the re-encode branch).
pub fn compress_for_attach(
    path: &Path,
    max_bytes: usize,
    max_dim: u32,
) -> Result<(Vec<u8>, String)> {
    // Stat first; cheaper than reading the whole file if it's already
    // small.
    let meta = fs::metadata(path).with_context(|| format!("stat({})", path.display()))?;
    let size = meta.len() as usize;

    if size <= max_bytes {
        // Pass-through. Use the file's extension to pick a sensible
        // MIME — we don't sniff bytes because the caller already trusts
        // this path (it just came out of a screenshot tool).
        let bytes = fs::read(path).with_context(|| format!("read({})", path.display()))?;
        let mime = mime_for_path(path);
        return Ok((bytes, mime.to_string()));
    }

    // Slow path: decode → downscale → re-encode.
    let img = image::open(path).with_context(|| format!("decode {}", path.display()))?;

    let resized = downscale(&img, max_dim);

    match encode_webp(&resized) {
        Ok(bytes) => Ok((bytes, "image/webp".to_string())),
        Err(e) => {
            tracing::warn!(
                error = %e,
                "WebP encoder failed; falling back to JPEG quality 85"
            );
            let bytes = encode_jpeg(&resized, 85).context("JPEG fallback encoder")?;
            Ok((bytes, "image/jpeg".to_string()))
        }
    }
}

/// Convenience wrapper: read the env vars `FLASHPASTE_MAX_BYTES` /
/// `FLASHPASTE_MAX_DIM` (falling back to the [`DEFAULT_MAX_BYTES`] /
/// [`DEFAULT_MAX_DIM`] constants) and call [`compress_for_attach`].
pub fn compress_for_attach_env(path: &Path) -> Result<(Vec<u8>, String)> {
    let max_bytes = std::env::var("FLASHPASTE_MAX_BYTES")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(DEFAULT_MAX_BYTES);
    let max_dim = std::env::var("FLASHPASTE_MAX_DIM")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(DEFAULT_MAX_DIM);
    compress_for_attach(path, max_bytes, max_dim)
}

/// Return `img` resized so the longest side is `max_dim` (preserving
/// aspect ratio). If `img` is already within bounds, returns a clone —
/// avoids a redundant filter pass.
fn downscale(img: &DynamicImage, max_dim: u32) -> DynamicImage {
    let (w, h) = (img.width(), img.height());
    if w <= max_dim && h <= max_dim {
        return img.clone();
    }
    // `resize` preserves aspect ratio by fitting inside (max_dim,max_dim).
    // Lanczos3 is the right filter for a single resize: sharper than
    // Triangle and almost free at these dimensions (one shot, not a
    // multi-stage pyramid).
    img.resize(max_dim, max_dim, FilterType::Lanczos3)
}

fn encode_webp(img: &DynamicImage) -> Result<Vec<u8>> {
    // The `webp` crate operates on RGBA8 / RGB8 buffers. Convert to RGB8
    // first; WebP's lossy encoder doesn't need an alpha channel for
    // typical screenshots and dropping it saves ~25% on the encoded
    // size.
    let rgb = img.to_rgb8();
    let encoder = webp::Encoder::from_rgb(rgb.as_raw(), rgb.width(), rgb.height());
    // Quality 80.0 is the sweet spot for UI screenshots: aggressive
    // enough to halve the size vs PNG, conservative enough that text
    // antialiasing stays clean.
    let encoded = encoder.encode(80.0);
    Ok(encoded.to_vec())
}

fn encode_jpeg(img: &DynamicImage, quality: u8) -> Result<Vec<u8>> {
    let rgb = img.to_rgb8();
    let mut buf = Vec::with_capacity(rgb.as_raw().len() / 4);
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);
    encoder
        .write_image(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .context("JPEG encode")?;
    Ok(buf)
}

/// Map a path's extension to a MIME for the pass-through branch.
fn mime_for_path(path: &Path) -> &'static str {
    let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
        return "image/png";
    };
    match ext.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        _ => "image/png",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn pass_through_when_already_small() {
        let tmp = tempfile_path("small.png");
        // Minimal valid PNG (1×1 black pixel) — under any sane cap.
        let png: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x00, 0x00, 0x00,
            0x00, 0x3A, 0x7E, 0x9B, 0x55, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9C, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        {
            let mut f = std::fs::File::create(&tmp).unwrap();
            f.write_all(png).unwrap();
            f.sync_all().unwrap();
        }
        let (bytes, mime) =
            compress_for_attach(std::path::Path::new(&tmp), 1024 * 1024, 2400).unwrap();
        assert_eq!(mime, "image/png");
        assert_eq!(bytes, png);
        let _ = std::fs::remove_file(&tmp);
    }

    fn tempfile_path(name: &str) -> String {
        let dir = std::env::temp_dir();
        let pid = std::process::id();
        dir.join(format!("flashpaste-compress-test-{pid}-{name}"))
            .to_string_lossy()
            .into_owned()
    }

    /// Exercise the re-encode branch: synthesize a 1200×800 RGB PNG,
    /// write it to a tmpfile, then force compression by passing a
    /// `max_bytes` below the on-disk PNG size. Expect a WebP (or JPEG
    /// fallback) MIME on the result, and a byte stream smaller than
    /// the original.
    #[test]
    fn reencodes_when_above_cap() {
        let tmp = tempfile_path("big.png");
        let w = 1200u32;
        let h = 800u32;
        // Random-ish noise so PNG can't compress to ~zero bytes — we
        // want the original to actually exceed the cap.
        let mut buf = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h {
            for x in 0..w {
                buf.push((x ^ y) as u8);
                buf.push(((x.wrapping_mul(7)) ^ y) as u8);
                buf.push((x ^ y.wrapping_mul(11)) as u8);
            }
        }
        let img = image::RgbImage::from_raw(w, h, buf).expect("rgb buf shape");
        image::DynamicImage::ImageRgb8(img)
            .save_with_format(&tmp, image::ImageFormat::Png)
            .expect("save png");

        let original_size = std::fs::metadata(&tmp).unwrap().len() as usize;
        // Force the re-encode branch with a 1-byte cap.
        let (bytes, mime) = compress_for_attach(std::path::Path::new(&tmp), 1, 1024).unwrap();
        assert!(
            mime == "image/webp" || mime == "image/jpeg",
            "unexpected mime {mime}"
        );
        assert!(!bytes.is_empty(), "empty re-encoded output");
        // Re-encoded should be significantly smaller than the noisy PNG.
        assert!(
            bytes.len() < original_size,
            "re-encoded {} >= original {}",
            bytes.len(),
            original_size
        );
        let _ = std::fs::remove_file(&tmp);
    }
}

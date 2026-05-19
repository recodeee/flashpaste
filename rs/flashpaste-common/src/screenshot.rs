//! Find the latest screenshot in `~/Pictures/Screenshots/` within an age
//! window. Mirrors the bash:
//!
//! ```text
//!   find "$ss_dir" -maxdepth 1 -type f \( -iname '*.png' -o -iname '*.jpg' \
//!     -o -iname '*.jpeg' \) -mmin -1 -printf "%T@ %p\n" \
//!     | sort -nr | sed -n '1s/^[^ ]* //p'
//! ```
//!
//! …but in-process so there's no `find`/`sort`/`sed` fork (saves ~15ms on
//! a cold cache, ~3ms on a warm one).
//!
//! Returns `(path, age_seconds, mime)` — `mime` is `"image/png"` or
//! `"image/jpeg"`, picked by extension exactly like the bash script.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Newest screenshot in `dir` whose mtime is within `max_age_secs`
/// seconds of now. Returns `None` if the dir doesn't exist, contains no
/// matching files, or every match is too old.
pub fn find_latest(
    dir: &Path,
    max_age_secs: u64,
) -> Option<(PathBuf, u64, &'static str)> {
    let entries = fs::read_dir(dir).ok()?;
    let now = SystemTime::now();
    let mut best: Option<(SystemTime, PathBuf)> = None;
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_file() {
            continue;
        }
        let path = entry.path();
        if !is_image_ext(path.extension()) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(mtime) = meta.modified() else { continue };
        match &best {
            None => best = Some((mtime, path)),
            Some((best_mtime, _)) if mtime > *best_mtime => best = Some((mtime, path)),
            _ => {}
        }
    }
    let (mtime, path) = best?;
    let age = now.duration_since(mtime).ok()?.as_secs();
    if age > max_age_secs {
        return None;
    }
    let mime = mime_for_path(&path);
    Some((path, age, mime))
}

/// True if `ext` (case-insensitive) is `png`, `jpg`, or `jpeg`.
fn is_image_ext(ext: Option<&OsStr>) -> bool {
    let Some(ext) = ext else { return false };
    let Some(s) = ext.to_str() else { return false };
    matches!(s.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg")
}

/// Map a path's extension to an image MIME. Matches the bash case
/// statement: png → image/png, jpg/jpeg → image/jpeg, else default png.
pub fn mime_for_path(path: &Path) -> &'static str {
    let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
        return "image/png";
    };
    match ext.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        _ => "image/png",
    }
}

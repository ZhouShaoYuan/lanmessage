use std::path::PathBuf;

use chrono::Local;

use crate::ui::image_utils::get_screenshot_cache_dir;

/// Captures the primary monitor and saves full capture to a temp file.
/// Window minimize/restore is handled by the caller (main thread).
/// Returns (temp_path, screen_width, screen_height).
pub fn capture_full_screen() -> Option<(PathBuf, u32, u32)> {
    let monitors = xcap::Monitor::all().ok()?;
    let monitor = monitors.first()?;
    let img = monitor.capture_image().ok()?;
    let w = img.width();
    let h = img.height();

    let cache_dir = get_screenshot_cache_dir();
    let filename = format!(
        "screenshot_full_{}.png",
        Local::now().format("%Y%m%d_%H%M%S")
    );
    let path = cache_dir.join(&filename);

    if let Err(e) = img.save(&path) {
        log::error!("Failed to save full screenshot: {}", e);
        return None;
    }

    Some((path, w, h))
}

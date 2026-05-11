use std::path::{Path, PathBuf};

use eframe::egui;

pub fn is_image_file(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.ends_with(".png")
        || lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".bmp")
        || lower.ends_with(".gif")
        || lower.ends_with(".webp")
}

pub fn get_image_cache_dir() -> PathBuf {
    let base = dirs::cache_dir().unwrap_or_else(|| PathBuf::from("."));
    let dir = base.join("lanmessage").join("image_cache");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn get_screenshot_cache_dir() -> PathBuf {
    let base = dirs::cache_dir().unwrap_or_else(|| PathBuf::from("."));
    let dir = base.join("lanmessage").join("screenshots");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn load_image_as_color_image(path: &Path) -> Option<egui::ColorImage> {
    let rgba = image::open(path).ok()?.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_flat_samples().as_slice()))
}

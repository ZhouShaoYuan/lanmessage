#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use human_panic::setup_panic;
use log::info;

mod models;
mod util;
mod events;
mod ui;
mod core;
mod constants;
mod storage;

fn main() -> eframe::Result<()> {
    setup_panic!();

    env_logger::init();

    let icon = std::fs::read("resources/logo.png")
        .ok()
        .and_then(|bytes| eframe::icon_data::from_png_bytes(&bytes).ok())
        .map(std::sync::Arc::new);

    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_title("LAN Messenger")
        .with_inner_size([800.0, 600.0]);
    if let Some(icon) = icon {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "LAN Messenger",
        options,
        Box::new(|cc| {
            info!("starting up");
            // Load system Chinese font
            let mut fonts = eframe::egui::FontDefinitions::default();
            let font_candidates: Vec<std::path::PathBuf> = if cfg!(target_os = "windows") {
                let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
                let font_dir = std::path::Path::new(&windir).join("Fonts");
                vec![
                    font_dir.join("msyh.ttc"),      // Microsoft YaHei
                    font_dir.join("msyhbd.ttc"),    // Microsoft YaHei Bold
                    font_dir.join("simhei.ttf"),    // SimHei
                    font_dir.join("simsun.ttc"),    // SimSun
                ]
            } else if cfg!(target_os = "macos") {
                vec![
                    std::path::PathBuf::from("/System/Library/Fonts/PingFang.ttc"),
                    std::path::PathBuf::from("/System/Library/Fonts/STHeiti Light.ttc"),
                    std::path::PathBuf::from("/Library/Fonts/Arial Unicode.ttf"),
                ]
            } else {
                // Linux: common CJK font locations
                vec![
                    std::path::PathBuf::from("/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc"),
                    std::path::PathBuf::from("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc"),
                    std::path::PathBuf::from("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc"),
                    std::path::PathBuf::from("/usr/share/fonts/truetype/wqy/wqy-microhei.ttc"),
                    std::path::PathBuf::from("/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf"),
                ]
            };

            for font_path in &font_candidates {
                if let Ok(font_data) = std::fs::read(font_path) {
                    let font_name = font_path
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    info!("Loaded CJK font: {}", font_path.display());
                    fonts.font_data.insert(
                        font_name.clone(),
                        std::sync::Arc::new(eframe::egui::FontData::from_owned(font_data)),
                    );
                    fonts.families
                        .entry(eframe::egui::FontFamily::Proportional)
                        .or_default()
                        .insert(0, font_name.clone());
                    fonts.families
                        .entry(eframe::egui::FontFamily::Monospace)
                        .or_default()
                        .insert(0, font_name.clone());
                    cc.egui_ctx.set_fonts(fonts);
                    break;
                }
            }
            let (tx, rx) = async_channel::bounded(100);
            let socket = std::net::UdpSocket::bind(crate::constants::protocol::ADDR.as_str())
                .expect("couldn't bind socket");
            info!("udp server start listening! {:?}", crate::constants::protocol::ADDR.as_str());
            crate::events::model::model_run(socket.try_clone().unwrap(), tx);
            Ok(Box::new(crate::ui::app::IpMsgApp::new(rx)))
        }),
    )
}

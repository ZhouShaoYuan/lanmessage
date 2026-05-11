use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use async_channel::Receiver as AsyncReceiver;
use chrono::prelude::*;
use eframe::egui;
use local_ip_address::list_afinet_netifas; 

use crate::core::GLOBLE_SENDER;
use crate::models::event::{ModelEvent, UiEvent};
use crate::models::model::{FileInfo, ReceivedSimpleFileInfo, User};
use crate::models::message;
use crate::ui::chat_panel::{ChatMessage, ChatPanel, ReceivedFileEntry};
use crate::storage::history;
use crate::ui::image_utils;

enum FileDialogResult {
    SendFile { ip: String, file_info: FileInfo },
    SendFolder { ip: String, file_info: FileInfo },
    DownloadFile {
        ip: String,
        file: ReceivedSimpleFileInfo,
        save_path: PathBuf,
    },
    // 新增此 variant
    ScreenshotReady {
        ip: String,
        temp_path: PathBuf,
        screen_size: (u32, u32),
    },
}
/// Phases of the screenshot capture + region-selection flow,
/// all driven on the main thread in update().
enum ScreenshotPhase {
    /// Minimizing window, waiting before capture
    Minimizing { ip: String, started_at: Instant },
    /// Ready to start region selection
    Selection(ScreenshotState),
}

struct ScreenshotState {
    temp_path: PathBuf,
    texture: egui::TextureHandle,
    screen_size: (u32, u32),
    drag_start: Option<egui::Pos2>,
    drag_end: Option<egui::Pos2>,
    peer_ip: String,
}

const ACCENT: egui::Color32 = egui::Color32::from_rgb(81, 132, 99);
const ACCENT_LIGHT: egui::Color32 = egui::Color32::from_rgb(224, 247, 244);
const SENT_BUBBLE: egui::Color32 = egui::Color32::from_rgb(81, 132, 99);
const RECV_BUBBLE: egui::Color32 = egui::Color32::from_rgb(240, 240, 240);
const FILE_BUBBLE_SENT: egui::Color32 = egui::Color32::from_rgb(167,211,178);
const FILE_BUBBLE_RECV: egui::Color32 = egui::Color32::from_rgb(225, 235, 240);

pub struct IpMsgApp {
    ui_event_rx: AsyncReceiver<UiEvent>,
    users: Vec<User>,
    selected_user_idx: Option<usize>,
    footer_text: String,
    chats: HashMap<String, ChatPanel>,
    active_chat_ip: Option<String>,
    file_dialog_tx: Sender<FileDialogResult>,
    file_dialog_rx: Receiver<FileDialogResult>,
    image_preview_path: Option<String>,
    image_preview_just_opened: bool,
    image_preview_texture: Option<egui::TextureHandle>,
    texture_cache: HashMap<String, egui::TextureHandle>,
    screenshot_phase: Option<ScreenshotPhase>,
     // 新增：用于暂存后台线程捕获的截图数据，等待主线程加载纹理
    pending_screenshot: Option<(String, PathBuf, (u32, u32))>,
    local_ips: Vec<String>, 
}

impl IpMsgApp {
    pub fn new(rx: AsyncReceiver<UiEvent>) -> Self {
        let (fd_tx, fd_rx) = std::sync::mpsc::channel();
        // 获取本机 IP
        let local_ips = Self::get_local_ips();
        eprintln!("Detected Local IPs: {:?}", local_ips);
        GLOBLE_SENDER
            .send(ModelEvent::UserListSelected(String::from("未选择")))
            .unwrap();
        IpMsgApp {
            ui_event_rx: rx,
            users: Vec::new(),
            selected_user_idx: None,
            footer_text: String::from("-- 未选择 --"),
            chats: HashMap::new(),
            active_chat_ip: None,
            file_dialog_tx: fd_tx,
            file_dialog_rx: fd_rx,
            image_preview_path: None,
            image_preview_just_opened: false,
            image_preview_texture: None,
            texture_cache: HashMap::new(),
            screenshot_phase: None,
            pending_screenshot: None,
            local_ips,
        }
    }

    // 新增辅助函数：获取本机有效 IPv4 地址
    fn get_local_ips() -> Vec<String> {
        let mut ips = Vec::new();
        match list_afinet_netifas() {
            Ok(network_interfaces) => {
                for (_, ip) in network_interfaces.iter() {
                    if let std::net::IpAddr::V4(ipv4) = ip {
                        // 排除回环地址 (127.0.0.1) 和链路本地地址 (169.254.x.x)
                        if !ipv4.is_loopback() && !ipv4.is_link_local() && !ipv4.is_unspecified() {
                            ips.push(ipv4.to_string());
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("Failed to get local IPs: {}", e);
            }
        }
        // 去重，防止某些系统上报重复接口
        ips.sort();
        ips.dedup();
        ips
    }

    fn persist_panel_history(&self, peer_ip: &str) {
        if let Some(panel) = self.chats.get(peer_ip) {
            let msgs: Vec<_> = panel
                .history
                .iter()
                .map(history::SerializableChatMessage::from_chat_message)
                .collect();
            history::save_history(peer_ip, &msgs);
        }
    }

    fn process_ui_events(&mut self) -> bool {
        let mut any = false;
        while let Ok(event) = self.ui_event_rx.try_recv() {
            any = true;
            match event {
                UiEvent::UpdateUserListFooterStatus(text) => {
                    self.footer_text = format!("-- {} --", text);
                }
                UiEvent::UserListAddOne(user) => {
                    // let is_local = self.local_ips.contains(&user.ip);
                    
                    // if is_local {
                    //     // 如果是本机 IP，忽略
                    //     continue; 
                    // }
                    if !self.users.iter().any(|u| u.ip == user.ip) {
                        self.users.push(user);
                    }
                }
                UiEvent::UserListRemoveOne(ip) => {
                    self.users.retain(|u| u.ip != ip);
                }
                UiEvent::OpenOrReOpenChatWindow { name, ip } => {
                    if !self.chats.contains_key(&ip) {
                        let mut panel = ChatPanel::new(name, &ip);
                        panel.history = history::load_history(&ip)
                            .iter()
                            .map(|m| m.to_chat_message())
                            .collect();
                        self.chats.insert(ip.clone(), panel);
                    }
                    self.active_chat_ip = Some(ip);
                }
                UiEvent::OpenOrReOpenChatWindow1 { name, ip, .. } => {
                    if !self.chats.contains_key(&ip) {
                        let mut panel = ChatPanel::new(name, &ip);
                        panel.history = history::load_history(&ip)
                            .iter()
                            .map(|m| m.to_chat_message())
                            .collect();
                        self.chats.insert(ip.clone(), panel);
                    }
                    self.active_chat_ip = Some(ip);
                }
                UiEvent::CloseChatWindow(ip) => {
                    self.persist_panel_history(&ip);
                    self.chats.remove(&ip);
                    if self.active_chat_ip.as_ref() == Some(&ip) {
                        self.active_chat_ip = self.chats.keys().next().cloned();
                    }
                }
                UiEvent::DisplaySelfSendMsgInHis { to_ip, context, files } => {
                    if let Some(panel) = self.chats.get_mut(&to_ip) {
                        let (file_names, file_paths): (Vec<String>, Vec<String>) = files
                            .map(|s| {
                                s.file_info.iter()
                                    .map(|f| (f.name.clone(), f.file_name.to_string_lossy().to_string()))
                                    .unzip()
                            })
                            .unwrap_or_default();
                        panel.history.push(ChatMessage::Sent {
                            content: context.clone(),
                            time: Local::now().time(),
                            files: file_names,
                            file_paths,
                        });
                    }
                    self.persist_panel_history(&to_ip);
                }
                UiEvent::DisplayReceivedMsgInHis {
                    from_ip,
                    name,
                    context,
                    files,
                } => {
                    if !self.chats.contains_key(&from_ip) {
                        let mut panel = ChatPanel::new(&name, &from_ip);
                        panel.history = history::load_history(&from_ip)
                            .iter()
                            .map(|m| m.to_chat_message())
                            .collect();
                        self.chats.insert(from_ip.clone(), panel);
                    }
                    if let Some(panel) = self.chats.get_mut(&from_ip) {
                        let mut file_entries: Vec<ReceivedFileEntry> =
                            files.iter().map(ReceivedFileEntry::from_received).collect();

                        // Auto-download image files to cache
                        let cache_dir = image_utils::get_image_cache_dir();
                        for (i, f) in files.iter().enumerate() {
                            if image_utils::is_image_file(&f.name) {
                                let cache_path = cache_dir.join(&f.name);
                                if cache_path.exists() {
                                    file_entries[i].downloaded = true;
                                    file_entries[i].downloaded_path = Some(cache_path.to_string_lossy().to_string());
                                } else {
                                    let _ = GLOBLE_SENDER.send(ModelEvent::PutDownloadTaskInPool {
                                        file: f.clone(),
                                        save_base_path: cache_dir.clone(),
                                        download_ip: from_ip.clone(),
                                    });
                                }
                            }
                        }

                        panel.history.push(ChatMessage::Received {
                            name,
                            content: context,
                            time: Local::now().time(),
                            files: file_entries,
                        });
                    }
                    self.persist_panel_history(&from_ip);
                }
                UiEvent::RemoveInReceivedList {
                    packet_id,
                    file_id,
                    download_ip,
                } => {
                    if let Some(panel) = self.chats.get_mut(&download_ip) {
                        for msg in &mut panel.history {
                            if let ChatMessage::Received { files, .. } = msg {
                                for f in files {
                                    if f.packet_id == packet_id && f.file_id == file_id {
                                        f.downloaded = true;
                                        if image_utils::is_image_file(&f.name) {
                                            let cache_path = image_utils::get_image_cache_dir().join(&f.name);
                                            if cache_path.exists() {
                                                f.downloaded_path = Some(cache_path.to_string_lossy().to_string());
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    self.persist_panel_history(&download_ip);
                }
            }
        }
        any
    }

    fn process_file_dialog_results(&mut self) {
        while let Ok(result) = self.file_dialog_rx.try_recv() {
            match result {
                FileDialogResult::SendFile { ip, file_info } => {
                    if let Some(panel) = self.chats.get_mut(&ip) {
                        panel.pre_send_files.push(file_info);
                    }
                }
                FileDialogResult::SendFolder { ip, file_info } => {
                    if let Some(panel) = self.chats.get_mut(&ip) {
                        panel.pre_send_files.push(file_info);
                    }
                }
                FileDialogResult::DownloadFile { ip, file, save_path } => {
                    GLOBLE_SENDER
                        .send(ModelEvent::PutDownloadTaskInPool {
                            file,
                            save_base_path: save_path,
                            download_ip: ip,
                        })
                        .unwrap();
                }
                FileDialogResult::ScreenshotReady { ip, temp_path, screen_size } => {
                    if Path::new(&temp_path).exists() {
                        // self.screenshot_pending = Some((ip, temp_path, screen_size));
                        self.pending_screenshot = Some((ip, temp_path, screen_size));
                    }
                }
            }
        }
    }

    // ── Peer list with avatar circles ──────────────────────────────

    fn draw_peer_list(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("联系人")
                        .size(14.0)
                        .color(egui::Color32::from_rgb(100, 100, 100)),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let btn = egui::Button::new("刷新").small();
                    if ui.add(btn).clicked() {
                        self.local_ips = Self::get_local_ips();
                        crate::events::model::send_ipmsg_br_entry();
                    }
                });
            });
        });
        ui.add_space(4.0);
        ui.separator();

        let mut selected = self.selected_user_idx;
        for (i, user) in self.users.iter().enumerate() {
            let is_selected = selected == Some(i);
            let response = draw_peer_item(ui, user, is_selected);
            if response.clicked() {
                selected = Some(i);
                GLOBLE_SENDER
                    .send(ModelEvent::UserListSelected(user.ip.clone()))
                    .unwrap();
            }
            if response.double_clicked() {
                GLOBLE_SENDER
                    .send(ModelEvent::UserListDoubleClicked {
                        name: user.name.clone(),
                        ip: user.ip.clone(),
                    })
                    .unwrap();
            }
        }
        self.selected_user_idx = selected;

        ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(&self.footer_text)
                    .size(11.0)
                    .color(egui::Color32::GRAY),
            );
        });
    }

    // ── Image preview overlay ──────────────────────────────────────

    fn draw_image_preview(&mut self, ctx: &egui::Context) {
        let Some(ref path) = self.image_preview_path else { return };
        if !Path::new(path).exists() {
            self.image_preview_path = None;
            self.image_preview_texture = None;
            return;
        }

        let first_frame = self.image_preview_just_opened;
        if first_frame {
            self.image_preview_just_opened = false;
        }

        // Close on Escape (skip on first frame)
        if !first_frame && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.image_preview_path = None;
            self.image_preview_texture = None;
            return;
        }

        // Load image only once when path changes
        let needs_load = self.image_preview_texture.is_none();
        if needs_load {
            let Some(color_image) = image_utils::load_image_as_color_image(Path::new(path)) else {
                self.image_preview_path = None;
                return;
            };
            self.image_preview_texture = Some(ctx.load_texture("preview_full", color_image, egui::TextureOptions::LINEAR));
        }

        let texture = self.image_preview_texture.as_ref().unwrap();
        let img_size = texture.size_vec2();
        let screen_rect = ctx.screen_rect();
        let avail = screen_rect.size();
        let scale = (avail.x / img_size.x).min(avail.y / img_size.y);
        let display_size = img_size * scale;
        let img_center = screen_rect.center();

        // Full-screen backdrop Area (click to close, skip on first frame)
        let backdrop_area = egui::Area::new(egui::Id::new("preview_backdrop"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen_rect.min)
            .interactable(!first_frame);
        let backdrop_result = backdrop_area.show(ctx, |ui| {
            ui.allocate_exact_size(screen_rect.size(), egui::Sense::click());
            ui.painter().rect_filled(screen_rect, egui::Rounding::ZERO, egui::Color32::from_black_alpha(180));
        });
        if !first_frame && backdrop_result.response.clicked() {
            self.image_preview_path = None;
            self.image_preview_texture = None;
            return;
        }

        // Image layer
        let img_area = egui::Area::new(egui::Id::new("preview_image"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(img_center.x - display_size.x / 2.0, img_center.y - display_size.y / 2.0))
            .interactable(true);
        img_area.show(ctx, |ui| {
            ui.add(
                egui::Image::new(texture)
                    .fit_to_exact_size(display_size)
                    .rounding(egui::Rounding::same(4.0)),
            );
        });

        // Close button (X) top-right of image
        let close_pos = egui::pos2(
            img_center.x + display_size.x / 2.0 - 16.0,
            img_center.y - display_size.y / 2.0 + 4.0,
        );
        let close_area = egui::Area::new(egui::Id::new("preview_close_btn"))
            .order(egui::Order::Foreground)
            .fixed_pos(close_pos)
            .interactable(!first_frame);
        close_area.show(ctx, |ui| {
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::click());
            let bg = if resp.hovered() {
                egui::Color32::from_black_alpha(180)
            } else {
                egui::Color32::from_black_alpha(100)
            };
            ui.painter().circle_filled(rect.center(), 14.0, bg);
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "✕",
                egui::FontId::proportional(14.0),
                egui::Color32::WHITE,
            );
            if resp.clicked() {
                self.image_preview_path = None;
                self.image_preview_texture = None;
            }
        });
    }

    // ── Screenshot region selection ─────────────────────────────────

    fn exit_screenshot_mode(&mut self, ctx: &egui::Context) {
        self.screenshot_phase = None;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(true));
    }

    fn draw_screenshot_selection(&mut self, ctx: &egui::Context) {
        // 1. 获取物理像素与逻辑点的比例
        let pixels_per_point = ctx.pixels_per_point();
        let screen_rect = ctx.screen_rect();

        // Escape to cancel
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.exit_screenshot_mode(ctx);
            return;
        }

        // Extract state, then put it back after drawing
        let phase = self.screenshot_phase.take();
        let Some(ScreenshotPhase::Selection(mut state)) = phase else {
            self.exit_screenshot_mode(ctx);
            return;
        };
        // 2. 获取图片原始像素大小
        let img_size_px = state.texture.size_vec2(); // 这是物理像素尺寸
        // 计算在 egui 逻辑坐标系下，图片应该占用的尺寸，以实现 1:1 不缩放
        let img_size_points = img_size_px / pixels_per_point;

        // 定义绘制区域（从左上角开始，保持原始物理像素大小对应的逻辑大小）
        let display_rect = egui::Rect::from_min_size(screen_rect.min, img_size_points);

        let area = egui::Area::new(egui::Id::new("screenshot_overlay"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen_rect.min)
            .interactable(true)
            .anchor(egui::Align2::LEFT_TOP, egui::Vec2::ZERO);;

        let mut crop_result: Option<(PathBuf, String)> = None;

        area.show(ctx, |ui| {
            let (full_rect, resp) = ui.allocate_exact_size(screen_rect.size(), egui::Sense::drag());

            // Draw captured image as background
            ui.painter().image(
                state.texture.id(),
                display_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );

            // Dark overlay
            ui.painter().rect_filled(full_rect, egui::Rounding::ZERO, egui::Color32::from_black_alpha(100));

            // Handle drag
            if resp.drag_started() {
                state.drag_start = resp.interact_pointer_pos();
                state.drag_end = None;
            }
            if resp.dragged() {
                state.drag_end = resp.interact_pointer_pos();
            }

            // Draw selection rect
            if let (Some(start), Some(end)) = (state.drag_start, state.drag_end) {
                let sel = rect_from_two_points(start, end);
                // 限制选取框不超出图片显示范围
                let sel_clamped = sel.intersect(display_rect);

                // 在选取框内恢复原图（高亮效果）
                // 计算 uv：将逻辑坐标映射回 0.0 - 1.0 范围
                let uv_min = (sel_clamped.min - display_rect.min) / img_size_points;
                let uv_max = (sel_clamped.max - display_rect.min) / img_size_points;
                // Clear overlay inside selection (show original image)
                ui.painter().image(
                    state.texture.id(),
                    sel_clamped,
                    egui::Rect::from_min_max(uv_min.to_pos2(), uv_max.to_pos2()),
                    egui::Color32::WHITE,
                );

                // Selection border
                ui.painter().rect_stroke(sel_clamped, egui::Rounding::ZERO, egui::Stroke::new(2.0, ACCENT));

                // 显示像素大小提示
                let w_px = (sel_clamped.width() * pixels_per_point).round() as u32;
                let h_px = (sel_clamped.height() * pixels_per_point).round() as u32;
                let hint = format!("{} x {}", w_px, h_px);
                ui.painter().text(
                    egui::pos2(sel_clamped.min.x, sel_clamped.min.y - 14.0),
                    egui::Align2::LEFT_BOTTOM,
                    &hint,
                    egui::FontId::proportional(14.0),
                    egui::Color32::WHITE,
                );
            }

            // On drag stopped — crop
            if resp.drag_stopped() {
                if let (Some(start), Some(end)) = (state.drag_start, state.drag_end) {
                    let sel = rect_from_two_points(start, end).intersect(display_rect);
                    
                    if sel.width() > 2.0 && sel.height() > 2.0 {
                        // 关键：裁剪坐标必须换算回物理像素
                        let x = ((sel.min.x - display_rect.min.x) * pixels_per_point) as u32;
                        let y = ((sel.min.y - display_rect.min.y) * pixels_per_point) as u32;
                        let w = (sel.width() * pixels_per_point) as u32;
                        let h = (sel.height() * pixels_per_point) as u32;

                        if let Some(cropped_path) = crop_and_save(&state.temp_path, x, y, w, h) {
                            crop_result = Some((cropped_path, state.peer_ip.clone()));
                        }
                    }
                }
            }
        });

        if let Some((cropped_path, peer_ip)) = crop_result {
            if let Some(fi) = path_to_file_info(
                &cropped_path,
                crate::constants::protocol::IPMSG_FILE_REGULAR as u8,
            ) {
                if let Some(panel) = self.chats.get_mut(&peer_ip) {
                    panel.pre_send_files.push(fi);
                }
            }
            self.exit_screenshot_mode(ctx);
        } else {
            self.screenshot_phase = Some(ScreenshotPhase::Selection(state));
        }
    }

    // ── Chat area with tab bar ─────────────────────────────────────

    fn draw_chat_area(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.chats.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() / 2.5);
                ui.label(
                    egui::RichText::new("双击左侧用户开始聊天")
                        .size(16.0)
                        .color(egui::Color32::from_rgb(180, 180, 180)),
                );
            });
            return;
        }

        // Tab bar
        let ips: Vec<String> = self.chats.keys().cloned().collect();
        let mut active = self.active_chat_ip.clone();
        let mut close_ip: Option<String> = None;

        egui::Frame::none()
            .fill(egui::Color32::from_rgb(245, 245, 245))
            .inner_margin(egui::Margin::symmetric(8.0, 6.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for ip in &ips {
                        let panel = self.chats.get(ip).unwrap();
                        let is_active = active.as_ref() == Some(ip);
                        let tab_title = panel.peer_name.clone();

                        let (tab_fill, text_color) = if is_active {
                            (ACCENT, egui::Color32::WHITE)
                        } else {
                            (
                                egui::Color32::from_rgb(230, 230, 230),
                                egui::Color32::from_rgb(80, 80, 80),
                            )
                        };

                        let btn = egui::Frame::none()
                            .fill(tab_fill)
                            .rounding(egui::Rounding::same(6.0))
                            .inner_margin(egui::Margin::symmetric(10.0, 4.0))
                            .show(ui, |ui| {
                                ui.label(
                                    egui::RichText::new(&tab_title)
                                        .size(13.0)
                                        .color(text_color),
                                );
                            })
                            .response;

                        if btn.clicked() {
                            active = Some(ip.clone());
                        }

                        // Close button (X)
                        let x_resp = ui.add_sized(
                            [14.0, 14.0],
                            egui::Label::new(
                                egui::RichText::new("×")
                                    .size(12.0)
                                    .color(egui::Color32::from_rgb(150, 150, 150)),
                            )
                            .sense(egui::Sense::click()),
                        );
                        if x_resp.clicked() {
                            close_ip = Some(ip.clone());
                        }

                        ui.add_space(4.0);
                    }
                });
            });

        if let Some(ip) = close_ip {
            GLOBLE_SENDER
                .send(ModelEvent::ClickChatWindowCloseBtn { from_ip: ip })
                .unwrap();
        }
        self.active_chat_ip = active;

        ui.separator();

        // Chat content
        let active_ip = match &self.active_chat_ip {
            Some(ip) => ip.clone(),
            None => return,
        };

        if let Some(panel) = self.chats.get_mut(&active_ip) {
            let mut preview_clicked: Option<String> = None;
            let mut screenshot_request: Option<String> = None;
            draw_chat_panel(ui, ctx, panel, &self.file_dialog_tx, &mut preview_clicked, &mut self.texture_cache, &mut screenshot_request);
            // 处理截图请求
            if let Some(ip) = screenshot_request {
                // 1. 最小化窗口
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                // 2. 进入 Minimizing 状态，等待主循环处理捕获
                self.screenshot_phase = Some(ScreenshotPhase::Minimizing {
                    ip: ip.clone(),
                    started_at: Instant::now(),
                });
                self.start_screenshot_capture(ip);
            }
            if preview_clicked.is_some() {
                self.image_preview_path = preview_clicked;
                self.image_preview_just_opened = true;
            }
            // Evict old entries when cache grows too large
            if self.texture_cache.len() > 100 {
                self.texture_cache.clear();
            }
        }
    }

     fn start_screenshot_capture(&self, ip: String) {
        let tx = self.file_dialog_tx.clone();
        
        std::thread::spawn(move || {
            // 等待一小段时间让窗口完全最小化
            std::thread::sleep(std::time::Duration::from_millis(300));

            // 尝试捕获屏幕
            // 这里以 screenshots crate 为例，请根据实际使用的库调整
            let result = std::panic::catch_unwind(|| {
                // 伪代码：实际调用取决于你使用的截图库
                // let screen = screenshots::Screen::all().ok()?.get(0)?.capture_image()?;
                // let path = save_temp_image(&screen)?;
                // Some((path, (width, height)))
                
                // 由于没有具体的截图库上下文，这里提供一个基于 image 和原生绑定的通用思路
                // 或者使用 rdev/enigo 等底层库，但通常推荐使用专门的截图 crate
                
                // 假设有一个 helper 函数 capture_screen 返回 (PathBuf, (u32, u32))
                capture_screen_full()
            });

            if let Ok(Some((temp_path, screen_size))) = result {
                let _ = tx.send(FileDialogResult::ScreenshotReady {
                    ip,
                    temp_path,
                    screen_size,
                });
            } else {
                eprintln!("Failed to capture screenshot");
                // 可选：发送一个错误事件或恢复窗口
            }
        });
    }
}

// 辅助函数：捕获全屏并保存为临时文件
fn capture_screen_full() -> Option<(PathBuf, (u32, u32))> {
    use screenshots::Screen;
    use std::time::SystemTime;

    let screens = Screen::all().ok()?;
    let screen = screens.get(0)?;
    let image = screen.capture().ok()?;
    
    let width = image.width();
    let height = image.height();

    let cache_dir = crate::ui::image_utils::get_screenshot_cache_dir();
    let filename = format!("full_{}.png", SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).ok()?.as_millis());
    let path = cache_dir.join(filename);

    let png_data = image.to_png(None).ok()?;
    fs::write(&path, png_data).ok()?;

    Some((path, (width, height)))
}


// ── Draw a single peer item with avatar ───────────────────────────────

fn draw_peer_item(ui: &mut egui::Ui, user: &User, is_selected: bool) -> egui::Response {
    let desired_size = egui::vec2(ui.available_width(), 52.0);
    let (rect, response) = ui.allocate_at_least(desired_size, egui::Sense::click());

    let bg = if is_selected {
        ACCENT_LIGHT
    } else if response.hovered() {
        egui::Color32::from_rgb(245, 245, 245)
    } else {
        egui::Color32::TRANSPARENT
    };

    if bg != egui::Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, egui::Rounding::same(6.0), bg);
    }

    // Avatar circle
    let avatar_center = egui::pos2(rect.min.x + 24.0, rect.center().y);
    let avatar_color = avatar_color_for_name(&user.name);
    ui.painter().circle_filled(avatar_center, 16.0, avatar_color);

    let first_char = user.name.chars().next().unwrap_or('?').to_string();
    ui.painter().text(
        avatar_center,
        egui::Align2::CENTER_CENTER,
        &first_char,
        egui::FontId::proportional(14.0),
        egui::Color32::WHITE,
    );

    // Name and host text
    let text_x = rect.min.x + 48.0;
    let name_pos = egui::pos2(text_x, rect.center().y - 8.0);
    let host_pos = egui::pos2(text_x, rect.center().y + 8.0);

    let name_color = if is_selected {
        ACCENT
    } else {
        egui::Color32::from_rgb(50, 50, 50)
    };

    ui.painter().text(
        name_pos,
        egui::Align2::LEFT_CENTER,
        &user.name,
        egui::FontId::proportional(13.0),
        name_color,
    );
    ui.painter().text(
        host_pos,
        egui::Align2::LEFT_CENTER,
        &user.host,
        egui::FontId::proportional(11.0),
        egui::Color32::from_rgb(150, 150, 150),
    );

    // Online dot
    let dot_pos = egui::pos2(rect.max.x - 14.0, rect.center().y);
    ui.painter().circle_filled(dot_pos, 4.0, egui::Color32::from_rgb(76, 175, 80));

    response
}

fn avatar_color_for_name(name: &str) -> egui::Color32 {
    let colors = [
        egui::Color32::from_rgb(33, 150, 243),
        egui::Color32::from_rgb(156, 39, 176),
        egui::Color32::from_rgb(255, 87, 34),
        egui::Color32::from_rgb(0, 150, 136),
        egui::Color32::from_rgb(233, 30, 99),
        egui::Color32::from_rgb(63, 81, 181),
        egui::Color32::from_rgb(255, 152, 0),
    ];
    let hash = name.bytes().fold(0u32, |acc, b| acc.wrapping_add(b as u32));
    colors[(hash as usize) % colors.len()]
}

// ── Chat panel with message bubbles ───────────────────────────────────

fn draw_chat_panel(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    panel: &mut ChatPanel,
    fd_tx: &Sender<FileDialogResult>,
    preview_clicked: &mut Option<String>,
    texture_cache: &mut HashMap<String, egui::TextureHandle>,
    screenshot_request: &mut Option<String>,
) {
    // Reserve space for compose area: TextEdit(3 rows ~58px) + file buttons(~28px) + margins(16px) + spacing(~8px)
    let compose_height = 120.0;
    let messages_height = (ui.available_height() - compose_height).max(80.0);

    // ── Messages area (top, scrollable) ──
    egui::ScrollArea::vertical()
        .id_salt(format!("history_{}", panel.peer_ip))
        .max_height(messages_height)
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            ui.add_space(8.0);
            for (i, msg) in panel.history.iter().enumerate() {
                match msg {
                    ChatMessage::Sent {
                        content,
                        time,
                        files,
                        file_paths,
                    } => {
                        draw_sent_bubble(ui, ctx, content, time, files, file_paths, preview_clicked, texture_cache);
                    }
                    ChatMessage::Received {
                        name,
                        content,
                        time,
                        files,
                    } => {
                        draw_received_bubble(ui, name, content, time, files, i, &panel.peer_ip, fd_tx, ctx, preview_clicked, texture_cache);
                    }
                }
                ui.add_space(4.0);
            }
            // Pending send files info
            if !panel.pre_send_files.is_empty() {
                ui.horizontal(|ui| {
                    ui.add_space(ui.available_width() - 200.0);
                    egui::Frame::none()
                        .fill(FILE_BUBBLE_SENT)
                        .rounding(egui::Rounding::same(8.0))
                        .inner_margin(egui::Margin::same(8.0))
                        .show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.label(
                                    egui::RichText::new("📎 待发送文件:")
                                        .size(12.0)
                                        .color(egui::Color32::WHITE),
                                );
                                for f in &panel.pre_send_files {
                                    ui.label(
                                        egui::RichText::new(format!("  {} ({})", f.name, format_size(f.size)))
                                            .size(11.0)
                                            .color(egui::Color32::from_rgb(200, 230, 225)),
                                    );
                                }
                            });
                        });
                });
            }
        });

    ui.separator();

    // ── Compose area (bottom, fixed) ──
    egui::Frame::none()
        .fill(egui::Color32::from_rgb(248, 248, 248))
        .inner_margin(egui::Margin::same(8.0))
        .show(ui, |ui| {
            // Row 1: text input + send button
            ui.horizontal(|ui| {
                let text_width = ui.available_width() - 68.0;
                let response = ui.add(
                    egui::TextEdit::multiline(&mut panel.compose_text)
                        .desired_width(text_width.max(300.0))
                        .desired_rows(3)
                        .hint_text("输入消息..."),
                );

                // Ctrl+Enter to send
                if ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.ctrl) {
                    send_message(panel);
                    response.request_focus();
                }

                ui.vertical(|ui| {
                    ui.add_space(12.0);
                    let send_btn = ui.add_sized(
                        [56.0, 32.0],
                        egui::Button::new(
                            egui::RichText::new("发送").size(13.0).color(egui::Color32::WHITE),
                        )
                        .fill(ACCENT)
                        .rounding(egui::Rounding::same(6.0)),
                    );
                    if send_btn.clicked() {
                        send_message(panel);
                    }
                });
            });

            // Row 2: file buttons below the text input, left-aligned
            ui.horizontal(|ui| {
                ui.add_space(2.0);
                let ip = panel.peer_ip.clone();
                let tx = fd_tx.clone();
                let ctx_c = ctx.clone();
                if ui
                    .button(egui::RichText::new("📎").size(16.0))
                    .on_hover_text("选择文件")
                    .clicked()
                {
                    std::thread::spawn(move || {
                        if let Some(path) = rfd::FileDialog::new()
                            .set_title("打开文件")
                            .pick_file()
                        {
                            if let Some(fi) = path_to_file_info(
                                &path,
                                crate::constants::protocol::IPMSG_FILE_REGULAR as u8,
                            ) {
                                let _ = tx.send(FileDialogResult::SendFile {
                                    ip,
                                    file_info: fi,
                                });
                            }
                            ctx_c.request_repaint();
                        }
                    });
                }
                let ip = panel.peer_ip.clone();
                let tx = fd_tx.clone();
                let ctx_c = ctx.clone();
                if ui
                    .button(egui::RichText::new("📁").size(16.0))
                    .on_hover_text("选择文件夹")
                    .clicked()
                {
                    std::thread::spawn(move || {
                        if let Some(path) = rfd::FileDialog::new()
                            .set_title("打开文件夹")
                            .pick_folder()
                        {
                            if let Some(fi) = path_to_file_info(
                                &path,
                                crate::constants::protocol::IPMSG_FILE_DIR as u8,
                            ) {
                                let _ = tx.send(FileDialogResult::SendFolder {
                                    ip,
                                    file_info: fi,
                                });
                            }
                            ctx_c.request_repaint();
                        }
                    });
                }
                let ip = panel.peer_ip.clone();
                let _tx = fd_tx.clone();
                let _ctx_c = ctx.clone();
                if ui
                    .button(egui::RichText::new("📷").size(16.0))
                    .on_hover_text("截图")
                    .clicked()
                {
                    // Kick off the main-thread-driven screenshot flow
                    // ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                    // self.screenshot_phase = Some(ScreenshotPhase::Minimizing {
                    //     ip,
                    //     started_at: Instant::now(),
                    // });
                    // ctx.request_repaint();
                    
                    *screenshot_request = Some(ip);
                }
            });
        });
}

fn send_message(panel: &mut ChatPanel) {
    let context = panel.compose_text.trim().to_string();
    if context.is_empty() && panel.pre_send_files.is_empty() {
        return;
    }
    let ip = panel.peer_ip.clone();
    let files = panel.pre_send_files.clone();
    let (packet, share_file) = message::create_sendmsg(context.clone(), files, ip.clone());
    GLOBLE_SENDER
        .send(ModelEvent::SendOneMsg {
            to_ip: ip,
            packet,
            context,
            files: share_file,
        })
        .unwrap();
    panel.compose_text.clear();
    panel.pre_send_files.clear();
}

// ── Message bubbles ───────────────────────────────────────────────────

const THUMBNAIL_MAX_W: f32 = 200.0;
const THUMBNAIL_MAX_H: f32 = 150.0;

fn measure_text_width(ui: &mut egui::Ui, text: &str, font_id: egui::FontId) -> f32 {
    let galley = ui.fonts(|f| {
        f.layout_no_wrap(text.to_string(), font_id, egui::Color32::WHITE)
    });
    galley.size().x
}

fn get_or_load_thumbnail(
    ctx: &egui::Context,
    texture_cache: &mut HashMap<String, egui::TextureHandle>,
    key: &str,
    path: &Path,
) -> Option<egui::TextureHandle> {
    if let Some(handle) = texture_cache.get(key) {
        return Some(handle.clone());
    }
    let color_image = image_utils::load_image_as_color_image(path)?;
    let handle = ctx.load_texture(key, color_image, egui::TextureOptions::LINEAR);
    texture_cache.insert(key.to_string(), handle.clone());
    Some(handle)
}

fn render_image_thumbnail(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    texture_key: &str,
    file_path: &str,
    preview_clicked: &mut Option<String>,
    texture_cache: &mut HashMap<String, egui::TextureHandle>,
) -> bool {
    let path = Path::new(file_path);
    if !path.exists() { return false; }
    let Some(texture) = get_or_load_thumbnail(ctx, texture_cache, texture_key, path) else { return false };

    let original_size = texture.size_vec2();
    let scale = (THUMBNAIL_MAX_W / original_size.x)
        .min(THUMBNAIL_MAX_H / original_size.y)
        .min(1.0);
    let thumb_size = original_size * scale;

    let img_resp = ui.add(
        egui::Image::new(&texture)
            .fit_to_exact_size(thumb_size)
            .rounding(egui::Rounding::same(4.0))
            .sense(egui::Sense::click()),
    );

    if img_resp.clicked() {
        *preview_clicked = Some(file_path.to_string());
    }
    img_resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    true
}

fn draw_sent_bubble(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    content: &str,
    time: &NaiveTime,
    files: &[String],
    file_paths: &[String],
    preview_clicked: &mut Option<String>,
    texture_cache: &mut HashMap<String, egui::TextureHandle>,
) {
    let max_w = ui.available_width() * 0.65;
    let pad = 20.0;

    let content_w = if !content.is_empty() {
        measure_text_width(ui, content, egui::FontId::proportional(13.0))
    } else {
        0.0
    };
    let files_w: f32 = files
        .iter()
        .map(|f| measure_text_width(ui, &format!("📎 {}", f), egui::FontId::proportional(12.0)))
        .fold(0.0_f32, f32::max);
    let has_images = files.iter().enumerate().any(|(i, f)| {
        image_utils::is_image_file(f) && file_paths.get(i).map_or(false, |p| Path::new(p).exists())
    });
    let img_w = if has_images { THUMBNAIL_MAX_W + pad } else { 0.0 };
    let bubble_w = (content_w.max(files_w).max(img_w) + pad).min(max_w);

    ui.horizontal(|ui| {
        let push = (ui.available_width() - bubble_w - 8.0).max(8.0);
        ui.add_space(push);
        egui::Frame::none()
            .fill(SENT_BUBBLE)
            .rounding(egui::Rounding::same(10.0))
            .inner_margin(egui::Margin::same(10.0))
            .show(ui, |ui| {
                ui.set_max_width(bubble_w);
                ui.vertical(|ui| {
                    if !content.is_empty() {
                        ui.label(
                            egui::RichText::new(content)
                                .size(13.0)
                                .color(egui::Color32::WHITE),
                        );
                    }
                    for (i, f) in files.iter().enumerate() {
                        ui.add_space(2.0);
                        if image_utils::is_image_file(f) {
                            if let Some(fp) = file_paths.get(i) {
                                if Path::new(fp).exists() {
                                    let key = format!("sent_img_{}", i);
                                    render_image_thumbnail(ui, ctx, &key, fp, preview_clicked, texture_cache);
                                    continue;
                                }
                            }
                        }
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("📎").size(12.0));
                            ui.label(
                                egui::RichText::new(f)
                                    .size(12.0)
                                    .color(egui::Color32::from_rgb(200, 230, 225)),
                            );
                        });
                    }
                });
            });
    });
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            ui.add_space(12.0);
            ui.label(
                egui::RichText::new(time.format("%H:%M").to_string())
                    .size(10.0)
                    .color(egui::Color32::from_rgb(180, 180, 180)),
            );
        });
    });
}

#[allow(clippy::too_many_arguments)]
fn draw_received_bubble(
    ui: &mut egui::Ui,
    name: &str,
    content: &str,
    time: &NaiveTime,
    files: &[ReceivedFileEntry],
    _msg_idx: usize,
    peer_ip: &str,
    fd_tx: &Sender<FileDialogResult>,
    ctx: &egui::Context,
    preview_clicked: &mut Option<String>,
    texture_cache: &mut HashMap<String, egui::TextureHandle>,
) {
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        // Avatar
        let avatar_color = avatar_color_for_name(name);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(32.0, 32.0), egui::Sense::hover());
        let center = rect.center();
        ui.painter().circle_filled(center, 14.0, avatar_color);
        let first_char = name.chars().next().unwrap_or('?').to_string();
        ui.painter().text(
            center,
            egui::Align2::CENTER_CENTER,
            &first_char,
            egui::FontId::proportional(12.0),
            egui::Color32::WHITE,
        );

        ui.add_space(4.0);

        // Bubble
        let max_w = ui.available_width() * 0.7;
        let pad = 20.0;
        let content_w = if !content.is_empty() {
            measure_text_width(ui, content, egui::FontId::proportional(13.0))
        } else {
            0.0
        };
        let files_w: f32 = files
            .iter()
            .map(|f| measure_text_width(ui, &format!("📎 {}", f.name), egui::FontId::proportional(12.0)))
            .fold(0.0_f32, f32::max);
        let name_w = measure_text_width(ui, name, egui::FontId::proportional(11.0));
        let has_images = files.iter().any(|f| {
            image_utils::is_image_file(&f.name)
                && f.downloaded_path.as_ref().map_or(false, |p| Path::new(p).exists())
        });
        let img_w = if has_images { THUMBNAIL_MAX_W + pad } else { 0.0 };
        let bubble_w = (content_w.max(files_w).max(name_w).max(img_w) + pad).min(max_w);

        egui::Frame::none()
            .fill(RECV_BUBBLE)
            .rounding(egui::Rounding::same(10.0))
            .inner_margin(egui::Margin::same(10.0))
            .show(ui, |ui| {
                ui.set_max_width(bubble_w);
                ui.vertical(|ui| {
                    // Sender name
                    ui.label(
                        egui::RichText::new(name)
                            .size(11.0)
                            .color(ACCENT)
                            .strong(),
                    );
                    if !content.is_empty() {
                        ui.label(
                            egui::RichText::new(content)
                                .size(13.0)
                                .color(egui::Color32::from_rgb(50, 50, 50)),
                        );
                    }
                    // File attachments
                    for (fi, f) in files.iter().enumerate() {
                        ui.add_space(6.0);
                        if image_utils::is_image_file(&f.name) {
                            if let Some(ref path) = f.downloaded_path {
                                if Path::new(path).exists() {
                                    let key = format!("recv_img_{}_{}", f.packet_id, fi);
                                    render_image_thumbnail(ui, ctx, &key, path, preview_clicked, texture_cache);
                                    continue;
                                }
                            }
                        }
                        egui::Frame::none()
                            .fill(FILE_BUBBLE_RECV)
                            .rounding(egui::Rounding::same(6.0))
                            .inner_margin(egui::Margin::same(6.0))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new("📎").size(13.0));
                                    ui.vertical(|ui| {
                                        ui.label(
                                            egui::RichText::new(&f.name)
                                                .size(12.0)
                                                .color(egui::Color32::from_rgb(40, 40, 40)),
                                        );
                                        ui.label(
                                            egui::RichText::new(format_size(f.size))
                                                .size(10.0)
                                                .color(egui::Color32::from_rgb(140, 140, 140)),
                                        );
                                    });
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if f.downloaded {
                                                ui.label(
                                                    egui::RichText::new("已下载")
                                                        .size(10.0)
                                                        .color(ACCENT),
                                                );
                                            } else {
                                                let btn = ui.add_sized(
                                                    [48.0, 20.0],
                                                    egui::Button::new(
                                                        egui::RichText::new("下载")
                                                            .size(10.0)
                                                            .color(egui::Color32::WHITE),
                                                    )
                                                    .fill(ACCENT)
                                                    .rounding(egui::Rounding::same(4.0)),
                                                );
                                                if btn.clicked() {
                                                    trigger_download(
                                                        f,
                                                        peer_ip,
                                                        fd_tx,
                                                        ctx,
                                                    );
                                                }
                                            }
                                        },
                                    );
                                });
                            });
                    }
                });
            });
    });
    // Timestamp
    ui.horizontal(|ui| {
        ui.add_space(48.0);
        ui.label(
            egui::RichText::new(time.format("%H:%M").to_string())
                .size(10.0)
                .color(egui::Color32::from_rgb(180, 180, 180)),
        );
    });
}

fn trigger_download(
    file: &ReceivedFileEntry,
    peer_ip: &str,
    fd_tx: &Sender<FileDialogResult>,
    ctx: &egui::Context,
) {
    let ip = peer_ip.to_string();
    let recv_file = ReceivedSimpleFileInfo {
        packet_id: file.packet_id,
        file_id: file.file_id,
        name: file.name.clone(),
        attr: file.attr,
        size: file.size,
        mtime: 0,
    };
    let tx = fd_tx.clone();
    let ctx_c = ctx.clone();
    std::thread::spawn(move || {
        if let Some(save_path) = rfd::FileDialog::new()
            .set_title("保存文件")
            .pick_folder()
        {
            let _ = tx.send(FileDialogResult::DownloadFile {
                ip: ip.clone(),
                file: recv_file,
                save_path,
            });
            ctx_c.request_repaint();
        }
    });
}

// ── Utilities ─────────────────────────────────────────────────────────

fn format_size(size: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    if size >= GB {
        format!("{:.1} GB", size as f64 / GB as f64)
    } else if size >= MB {
        format!("{:.1} MB", size as f64 / MB as f64)
    } else if size >= KB {
        format!("{:.1} KB", size as f64 / KB as f64)
    } else {
        format!("{} B", size)
    }
}

fn path_to_file_info(path: &PathBuf, default_attr: u8) -> Option<FileInfo> {
    let metadata = fs::metadata(path).ok()?;
    let attr = if metadata.is_file() {
        crate::constants::protocol::IPMSG_FILE_REGULAR as u8
    } else if metadata.is_dir() {
        crate::constants::protocol::IPMSG_FILE_DIR as u8
    } else {
        default_attr
    };
    let name = path.file_name()?.to_str()?.to_owned();
    Some(FileInfo {
        file_id: Local::now().timestamp() as u32,
        file_name: path.clone(),
        name,
        attr: attr as u8,
        size: metadata.len(),
        mtime: Local::now().time(),
        atime: Local::now().time(),
        crtime: Local::now().time(),
    })
}

fn rect_from_two_points(a: egui::Pos2, b: egui::Pos2) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(a.x.min(b.x), a.y.min(b.y)),
        egui::pos2(a.x.max(b.x), a.y.max(b.y)),
    )
}

fn crop_and_save(temp_path: &Path, x: u32, y: u32, w: u32, h: u32) -> Option<PathBuf> {
    let mut img = image::open(temp_path).ok()?;
    let cropped = img.crop(x, y, w, h);
    let cache_dir = crate::ui::image_utils::get_screenshot_cache_dir();
    let filename = format!("screenshot_{}.png", Local::now().format("%Y%m%d_%H%M%S"));
    let path = cache_dir.join(&filename);
    cropped.save(&path).ok()?;
    Some(path)
}

// ── Main update loop ──────────────────────────────────────────────────

impl eframe::App for IpMsgApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let had_events = self.process_ui_events();
        self.process_file_dialog_results();

        // 处理 Minimizing 状态：如果处于 Minimizing 状态，检查是否超时或按下了 Escape
        if let Some(ScreenshotPhase::Minimizing { started_at, .. }) = &self.screenshot_phase {
            // 如果超过 2 秒还没收到截图数据，可能出错了，恢复窗口
            if started_at.elapsed() > Duration::from_secs(2) {
                eprintln!("Screenshot capture timeout");
                self.exit_screenshot_mode(ctx);
            }
            
            // 在 Minimizing 阶段，我们不绘制正常 UI，也不绘制选择界面
            // 只是等待后台线程发送 ScreenshotReady
            ctx.request_repaint();
            return;
        }

        // Handle pending screenshot capture (从 Minimizing 过渡到 Selection)
        if let Some((ip, temp_path, screen_size)) = self.pending_screenshot.take() {
            if Path::new(&temp_path).exists() {
                if let Some(color_image) = image_utils::load_image_as_color_image(Path::new(&temp_path)) {
                    let texture = ctx.load_texture("screenshot_full", color_image, egui::TextureOptions::LINEAR);
                    self.screenshot_phase = Some(ScreenshotPhase::Selection(ScreenshotState {
                        temp_path,
                        texture,
                        screen_size,
                        drag_start: None,
                        drag_end: None,
                        peer_ip: ip,
                    }));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(false));
                } else {
                    self.exit_screenshot_mode(ctx);
                } 
            } else {
                self.exit_screenshot_mode(ctx);
            }
        }

        // Screenshot selection mode — takes over the entire window
        if self.screenshot_phase.is_some() {
            self.draw_screenshot_selection(ctx);
            ctx.request_repaint();
            return;
        }

        if had_events {
            ctx.request_repaint();
        }

        // Image preview overlay — takes over the entire window
        if self.image_preview_path.is_some() {
            self.draw_image_preview(ctx);
            ctx.request_repaint();
            return;
        }

        // Normal UI
        egui::SidePanel::left("peer_list")
            .min_width(200.0)
            .default_width(220.0)
            .frame(
                egui::Frame::none()
                    .fill(egui::Color32::from_rgb(250, 250, 250))
                    .inner_margin(egui::Margin::same(8.0)),
            )
            .show(ctx, |ui| {
                self.draw_peer_list(ui);
            });

        // Central panel: chat area
        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(egui::Color32::WHITE)
                    .inner_margin(egui::Margin {
                        left: 4.0,
                        right: 4.0,
                        top: 4.0,
                        bottom: 1.0,
                    }),
            )
            .show(ctx, |ui| {
                self.draw_chat_area(ui, ctx);
            });

        // Keep polling for events even when window is inactive/unfocused,
        // so new messages appear without needing to click the window first.
        ctx.request_repaint_after(Duration::from_millis(200));
    }
}

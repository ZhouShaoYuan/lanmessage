use std::path::PathBuf;

use chrono::prelude::*;
use serde::{Deserialize, Serialize};

use crate::ui::chat_panel::{ChatMessage, ReceivedFileEntry};

const MAX_MESSAGES_PER_PEER: usize = 500;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum MessageDirection {
    Sent,
    Received,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SerializableFileEntry {
    pub name: String,
    pub size: u64,
    pub packet_id: Option<u32>,
    pub file_id: Option<u32>,
    pub attr: Option<u8>,
    pub downloaded: Option<bool>,
    #[serde(default)]
    pub downloaded_path: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SerializableChatMessage {
    pub direction: MessageDirection,
    pub content: String,
    pub time: String,
    pub sender_name: Option<String>,
    pub files: Vec<SerializableFileEntry>,
    #[serde(default)]
    pub file_paths: Vec<String>,
}

impl SerializableChatMessage {
    pub fn from_chat_message(msg: &ChatMessage) -> Self {
        match msg {
            ChatMessage::Sent { content, time, files, file_paths } => SerializableChatMessage {
                direction: MessageDirection::Sent,
                content: content.clone(),
                time: time.format("%H:%M:%S").to_string(),
                sender_name: None,
                files: files
                    .iter()
                    .map(|name| SerializableFileEntry {
                        name: name.clone(),
                        size: 0,
                        packet_id: None,
                        file_id: None,
                        attr: None,
                        downloaded: None,
                        downloaded_path: None,
                    })
                    .collect(),
                file_paths: file_paths.clone(),
            },
            ChatMessage::Received {
                name,
                content,
                time,
                files,
            } => SerializableChatMessage {
                direction: MessageDirection::Received,
                content: content.clone(),
                time: time.format("%H:%M:%S").to_string(),
                sender_name: Some(name.clone()),
                files: files
                    .iter()
                    .map(|f| SerializableFileEntry {
                        name: f.name.clone(),
                        size: f.size,
                        packet_id: Some(f.packet_id),
                        file_id: Some(f.file_id),
                        attr: Some(f.attr),
                        downloaded: Some(f.downloaded),
                        downloaded_path: f.downloaded_path.clone(),
                    })
                    .collect(),
                file_paths: Vec::new(),
            },
        }
    }

    pub fn to_chat_message(&self) -> ChatMessage {
        let time = NaiveTime::parse_from_str(&self.time, "%H:%M:%S")
            .unwrap_or_else(|_| Local::now().time());
        match self.direction {
            MessageDirection::Sent => ChatMessage::Sent {
                content: self.content.clone(),
                time,
                files: self.files.iter().map(|f| f.name.clone()).collect(),
                file_paths: self.file_paths.clone(),
            },
            MessageDirection::Received => ChatMessage::Received {
                name: self.sender_name.clone().unwrap_or_default(),
                content: self.content.clone(),
                time,
                files: self
                    .files
                    .iter()
                    .map(|f| ReceivedFileEntry {
                        packet_id: f.packet_id.unwrap_or(0),
                        file_id: f.file_id.unwrap_or(0),
                        name: f.name.clone(),
                        size: f.size,
                        attr: f.attr.unwrap_or(0),
                        downloaded: f.downloaded.unwrap_or(false),
                        downloaded_path: f.downloaded_path.clone(),
                    })
                    .collect(),
            },
        }
    }
}

fn ensure_history_dir() -> Result<PathBuf, std::io::Error> {
    let base = dirs::data_dir().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "Cannot determine data directory")
    })?;
    let history_dir = base.join("lanmessage").join("history");
    std::fs::create_dir_all(&history_dir)?;
    Ok(history_dir)
}

fn sanitize_ip_for_filename(ip: &str) -> String {
    ip.replace(':', "_")
}

pub fn load_history(peer_ip: &str) -> Vec<SerializableChatMessage> {
    let dir = match ensure_history_dir() {
        Ok(d) => d,
        Err(e) => {
            log::error!("Failed to create history dir: {}", e);
            return Vec::new();
        }
    };
    let path = dir.join(format!("{}.json", sanitize_ip_for_filename(peer_ip)));

    if !path.exists() {
        return Vec::new();
    }
    match std::fs::read_to_string(&path) {
        Ok(json) => match serde_json::from_str::<Vec<SerializableChatMessage>>(&json) {
            Ok(messages) => messages,
            Err(e) => {
                log::error!("Failed to parse history for {}: {}", peer_ip, e);
                Vec::new()
            }
        },
        Err(e) => {
            log::error!("Failed to read history for {}: {}", peer_ip, e);
            Vec::new()
        }
    }
}

pub fn save_history(peer_ip: &str, messages: &[SerializableChatMessage]) {
    let dir = match ensure_history_dir() {
        Ok(d) => d,
        Err(e) => {
            log::error!("Failed to create history dir: {}", e);
            return;
        }
    };
    let filename = sanitize_ip_for_filename(peer_ip);
    let path = dir.join(format!("{}.json", filename));

    let to_save = if messages.len() > MAX_MESSAGES_PER_PEER {
        &messages[messages.len() - MAX_MESSAGES_PER_PEER..]
    } else {
        messages
    };

    let tmp_path = dir.join(format!("{}.json.tmp", filename));
    match serde_json::to_string_pretty(to_save) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&tmp_path, &json) {
                log::error!("Failed to write history tmp: {}", e);
                return;
            }
            if let Err(e) = std::fs::rename(&tmp_path, &path) {
                log::error!("Failed to rename history file: {}", e);
            }
        }
        Err(e) => {
            log::error!("Failed to serialize history: {}", e);
        }
    }
}

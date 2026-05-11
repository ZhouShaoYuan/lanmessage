use chrono::prelude::*;

use crate::models::model::{FileInfo, ReceivedSimpleFileInfo};

#[derive(Clone, Debug)]
pub struct ReceivedFileEntry {
    pub packet_id: u32,
    pub file_id: u32,
    pub name: String,
    pub size: u64,
    pub attr: u8,
    pub downloaded: bool,
    pub downloaded_path: Option<String>,
}

impl ReceivedFileEntry {
    pub fn from_received(f: &ReceivedSimpleFileInfo) -> Self {
        ReceivedFileEntry {
            packet_id: f.packet_id,
            file_id: f.file_id,
            name: f.name.clone(),
            size: f.size,
            attr: f.attr,
            downloaded: false,
            downloaded_path: None,
        }
    }
}

#[derive(Clone, Debug)]
pub enum ChatMessage {
    Sent {
        content: String,
        time: NaiveTime,
        files: Vec<String>,
        file_paths: Vec<String>,
    },
    Received {
        name: String,
        content: String,
        time: NaiveTime,
        files: Vec<ReceivedFileEntry>,
    },
}

pub struct ChatPanel {
    pub peer_name: String,
    pub peer_ip: String,
    pub history: Vec<ChatMessage>,
    pub compose_text: String,
    pub pre_send_files: Vec<FileInfo>,
}

impl ChatPanel {
    pub fn new(name: impl Into<String>, ip: impl Into<String>) -> Self {
        ChatPanel {
            peer_name: name.into(),
            peer_ip: ip.into(),
            history: Vec::new(),
            compose_text: String::new(),
            pre_send_files: Vec::new(),
        }
    }
}

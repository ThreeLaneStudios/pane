use serde::{Deserialize, Serialize};
use super::SyncChange;

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum SyncMessage {
    RequestChanges {
        since_version: i64,
    },
    SendChanges {
        changes: Vec<SyncChange>,
    },
    Ack {
        success: bool,
        error: Option<String>,
    },
    Hello {
        device_id: String,
        version: String,
    },
}

impl SyncMessage {
    pub fn request_changes(since_version: i64) -> Self {
        Self::RequestChanges { since_version }
    }

    pub fn send_changes(changes: Vec<SyncChange>) -> Self {
        Self::SendChanges { changes }
    }

    pub fn ack(success: bool, error: Option<String>) -> Self {
        Self::Ack { success, error }
    }

    pub fn hello(device_id: String, version: String) -> Self {
        Self::Hello { device_id, version }
    }

    pub fn unwrap_changes_count(&self) -> usize {
        match self {
            Self::SendChanges { changes } => changes.len(),
            _ => 0,
        }
    }
}
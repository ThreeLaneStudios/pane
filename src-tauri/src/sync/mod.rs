// Retained as dormant peer-to-peer sync modules for protocol compatibility.
#[allow(dead_code)]
pub mod protocol;
#[allow(dead_code)]
pub mod client;
#[allow(dead_code)]
pub mod server;
// pub mod discovery; // Disabled — triggers macOS local network popup (#154)
pub mod auth;
pub mod device;
pub mod commands;
pub mod crypto;
pub mod keystore;
pub mod keystore_migration;
pub mod engine;
pub mod relay;
pub mod manager;
pub mod auto_sync;
pub mod file_sync;

use serde::{Deserialize, Serialize};

// Wire shape retained for the dormant peer-to-peer sync protocol.
#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SyncChange {
    pub table_name: String,
    pub pk: Vec<u8>,  // Raw blob bytes, serialized as JSON array
    pub cid: String,
    pub val: Option<String>,
    pub col_version: i64,
    pub db_version: i64,
    pub site_id: Vec<u8>,
    pub cl: i64,
    pub seq: i64,
}

use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{accept_async, tungstenite::Message, WebSocketStream};
use futures_util::{SinkExt, StreamExt, stream::SplitSink};
use super::protocol::SyncMessage;
use super::SyncChange;
use std::sync::{Arc, Mutex};

pub struct SyncServer {
    device_id: String,
    db: Arc<Mutex<crate::database::DbConn>>,
}

impl SyncServer {
    pub fn new(device_id: String, db: Arc<Mutex<crate::database::DbConn>>) -> Self {
        Self { device_id, db }
    }

    pub async fn start(&self, port: u16) -> Result<(), String> {
        let addr = format!("0.0.0.0:{}", port);
        let listener = TcpListener::bind(&addr).await
            .map_err(|e| format!("Failed to bind to {}: {}", addr, e))?;

        println!("Sync server listening on {}", addr);

        while let Ok((stream, addr)) = listener.accept().await {
            let device_id = self.device_id.clone();
            let db = self.db.clone();
            
            tokio::spawn(async move {
                if let Err(e) = handle_connection(stream, addr, device_id, db).await {
                    eprintln!("Error handling connection from {}: {}", addr, e);
                }
            });
        }

        Ok(())
    }
}

async fn handle_connection(
    stream: TcpStream, 
    addr: SocketAddr, 
    device_id: String,
    db: Arc<Mutex<crate::database::DbConn>>
) -> Result<(), String> {
    let ws_stream = accept_async(stream).await
        .map_err(|e| format!("WebSocket handshake failed: {}", e))?;

    let (mut write, mut read): (SplitSink<WebSocketStream<TcpStream>, Message>, _) = ws_stream.split();
    let mut hello_received = false;

    while let Some(msg) = read.next().await {
        let msg = msg.map_err(|e| format!("WebSocket error: {}", e))?;

        if let Message::Text(text) = msg {
            let request: SyncMessage = serde_json::from_str(&text)
                .map_err(|e| format!("Failed to parse request: {}", e))?;

            match request {
                SyncMessage::Hello { device_id: peer_id, version } => {
                    println!("Peer {} connected (version: {})", peer_id, version);
                    hello_received = true;
                    
                    let response = SyncMessage::hello(device_id.clone(), "3.2.0".to_string());
                    let response_json = serde_json::to_string(&response)
                        .map_err(|e| format!("Failed to serialize response: {}", e))?;
                    
                    write.send(Message::Text(response_json)).await
                        .map_err(|e| format!("Failed to send response: {}", e))?;
                },
                SyncMessage::RequestChanges { since_version } => {
                    if !hello_received {
                        eprintln!("Received RequestChanges before Hello from {}", addr);
                        break;
                    }

                    // Get changes from database, filtering to real tables only
                    let all_changes = get_sync_changes(&db, since_version)
                        .map_err(|e| format!("Failed to get changes: {}", e))?;
                    let valid_tables = vec!["notes", "tasks", "projects", "users", "tags", "note_tags", "note_links"];
                    let changes: Vec<SyncChange> = all_changes.into_iter()
                        .filter(|c| valid_tables.contains(&c.table_name.as_str()))
                        .filter(|c| {
                            let pk_str = String::from_utf8_lossy(&c.pk);
                            !pk_str.contains("crr-test") && !pk_str.contains("test-sync")
                        })
                        .collect();
                    let change_count = changes.len();
                    
                    let response = SyncMessage::send_changes(changes);
                    let response_json = serde_json::to_string(&response)
                        .map_err(|e| format!("Failed to serialize changes: {}", e))?;
                    
                    write.send(Message::Text(response_json)).await
                        .map_err(|e| format!("Failed to send changes: {}", e))?;
                    
                    println!("Sent {} changes to peer {}", change_count, addr);
                },
                SyncMessage::SendChanges { changes } => {
                    if !hello_received {
                        eprintln!("Received SendChanges before Hello from {}", addr);
                        break;
                    }

                    println!("Received {} changes from peer {}", changes.len(), addr);
                    
                    // Apply changes to database
                    let result = apply_sync_changes(&db, changes);
                    
                    let response = match result {
                        Ok(_) => {
                            println!("Successfully applied changes from peer {}", addr);
                            SyncMessage::ack(true, None)
                        },
                        Err(e) => {
                            eprintln!("Failed to apply changes from peer {}: {}", addr, e);
                            SyncMessage::ack(false, Some(e))
                        },
                    };
                    
                    let response_json = serde_json::to_string(&response)
                        .map_err(|e| format!("Failed to serialize ACK: {}", e))?;
                    
                    write.send(Message::Text(response_json)).await
                        .map_err(|e| format!("Failed to send ACK: {}", e))?;
                },
                SyncMessage::Ack { success, error } => {
                    if !success {
                        eprintln!("Peer {} reported error: {:?}", addr, error);
                    } else {
                        println!("Peer {} acknowledged our changes", addr);
                    }
                },
            }
        }
    }

    println!("Peer {} disconnected", addr);
    Ok(())
}

fn get_sync_changes(db: &Arc<Mutex<crate::database::DbConn>>, since_version: i64) -> Result<Vec<SyncChange>, String> {
    let conn = db.lock().map_err(|e| format!("Failed to lock database: {}", e))?;
    
    let mut stmt = conn.prepare(
        "SELECT \"table\", pk, cid, val, col_version, db_version, site_id, cl, seq 
         FROM crsql_changes WHERE db_version > ?"
    ).map_err(|e| format!("Failed to prepare query: {}", e))?;

    let changes = stmt.query_map([since_version], |row| {
        // pk is always a blob in cr-sqlite — read as raw bytes
        let pk: Vec<u8> = row.get(1)?;
        
        // val can be various types
        let val_value: rusqlite::types::Value = row.get(3)?;
        let val = match val_value {
            rusqlite::types::Value::Text(s) => Some(s),
            rusqlite::types::Value::Integer(i) => Some(i.to_string()),
            rusqlite::types::Value::Real(f) => Some(f.to_string()),
            rusqlite::types::Value::Blob(b) => {
                Some(String::from_utf8(b.clone()).unwrap_or_else(|_| {
                    b.iter().map(|byte| format!("{:02x}", byte)).collect::<String>()
                }))
            },
            rusqlite::types::Value::Null => None,
        };

        Ok(SyncChange {
            table_name: row.get(0)?,
            pk,
            cid: row.get(2)?,
            val,
            col_version: row.get(4)?,
            db_version: row.get(5)?,
            site_id: row.get(6)?,
            cl: row.get(7)?,
            seq: row.get(8)?,
        })
    }).map_err(|e| format!("Failed to execute query: {}", e))?;

    let mut result = Vec::new();
    for change in changes {
        result.push(change.map_err(|e| format!("Failed to read change: {}", e))?);
    }

    Ok(result)
}

fn apply_sync_changes(db: &Arc<Mutex<crate::database::DbConn>>, changes: Vec<SyncChange>) -> Result<(), String> {
    let conn = db.lock().map_err(|e| format!("Failed to lock database: {}", e))?;
    
    for change in changes {
        conn.execute(
            "INSERT INTO crsql_changes (\"table\", pk, cid, val, col_version, db_version, site_id, cl, seq) 
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            [
                &change.table_name as &dyn rusqlite::ToSql,
                &change.pk,
                &change.cid, 
                &change.val,
                &change.col_version,
                &change.db_version,
                &change.site_id,
                &change.cl,
                &change.seq,
            ],
        ).map_err(|e| format!("Failed to insert change: {}", e))?;
    }

    Ok(())
}
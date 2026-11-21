use tokio_tungstenite::{connect_async, tungstenite::Message, WebSocketStream, MaybeTlsStream};
use tokio::net::TcpStream;
use futures_util::{SinkExt, StreamExt, stream::{SplitSink, SplitStream}};
use super::protocol::SyncMessage;
use super::SyncChange;

pub struct SyncClient {
    device_id: String,
}

impl SyncClient {
    pub fn new(device_id: String) -> Self {
        Self { device_id }
    }

    pub async fn connect_and_sync(&self, peer_address: &str, since_version: i64, local_changes: Vec<SyncChange>) -> Result<Vec<SyncChange>, String> {
        let url = format!("ws://{}/sync", peer_address);
        
        let (ws_stream, _) = connect_async(&url)
            .await
            .map_err(|e| format!("Failed to connect to peer: {}", e))?;

        let (mut write, mut read): (SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>, SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>) = ws_stream.split();

        // Send hello message
        let hello = SyncMessage::hello(self.device_id.clone(), "3.2.0".to_string());
        let hello_json = serde_json::to_string(&hello)
            .map_err(|e| format!("Failed to serialize hello: {}", e))?;
        
        write.send(Message::Text(hello_json)).await
            .map_err(|e| format!("Failed to send hello: {}", e))?;

        // Wait for Hello response from server
        if let Some(msg) = read.next().await {
            let msg = msg.map_err(|e| format!("WebSocket error reading hello: {}", e))?;
            if let Message::Text(text) = msg {
                let response: SyncMessage = serde_json::from_str(&text)
                    .map_err(|e| format!("Failed to parse hello response: {}", e))?;
                match response {
                    SyncMessage::Hello { device_id: peer_id, .. } => {
                        log::info!("Connected to peer: {}", peer_id);
                    },
                    _ => return Err("Expected Hello from peer".to_string()),
                }
            }
        }

        // Request changes
        let request = SyncMessage::request_changes(since_version);
        let request_json = serde_json::to_string(&request)
            .map_err(|e| format!("Failed to serialize request: {}", e))?;
        
        write.send(Message::Text(request_json)).await
            .map_err(|e| format!("Failed to send request: {}", e))?;

        // Wait for response with remote changes
        let remote_changes = if let Some(msg) = read.next().await {
            let msg = msg.map_err(|e| format!("WebSocket error: {}", e))?;
            
            if let Message::Text(text) = msg {
                let response: SyncMessage = serde_json::from_str(&text)
                    .map_err(|e| format!("Failed to parse response: {}", e))?;
                
                match response {
                    SyncMessage::SendChanges { changes } => {
                        // Send ACK for received changes
                        let ack = SyncMessage::ack(true, None);
                        let ack_json = serde_json::to_string(&ack)
                            .map_err(|e| format!("Failed to serialize ACK: {}", e))?;
                        
                        write.send(Message::Text(ack_json)).await
                            .map_err(|e| format!("Failed to send ACK: {}", e))?;
                        
                        changes
                    },
                    _ => return Err("Unexpected response from peer".to_string()),
                }
            } else {
                return Err("Expected text message".to_string());
            }
        } else {
            return Err("No response from peer".to_string());
        };

        // Now send our local changes to the peer
        if !local_changes.is_empty() {
            let send_changes = SyncMessage::send_changes(local_changes);
            let send_changes_json = serde_json::to_string(&send_changes)
                .map_err(|e| format!("Failed to serialize local changes: {}", e))?;
            
            write.send(Message::Text(send_changes_json)).await
                .map_err(|e| format!("Failed to send local changes: {}", e))?;

            // Wait for ACK from peer
            if let Some(msg) = read.next().await {
                let msg = msg.map_err(|e| format!("WebSocket error: {}", e))?;
                
                if let Message::Text(text) = msg {
                    let response: SyncMessage = serde_json::from_str(&text)
                        .map_err(|e| format!("Failed to parse ACK: {}", e))?;
                    
                    match response {
                        SyncMessage::Ack { success, error } => {
                            if !success {
                                return Err(format!("Peer rejected our changes: {:?}", error));
                            }
                        },
                        _ => return Err("Expected ACK from peer".to_string()),
                    }
                }
            }
        }

        Ok(remote_changes)
    }
}
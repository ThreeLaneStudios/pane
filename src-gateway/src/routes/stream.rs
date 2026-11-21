use crate::auth::TenantContext;
use crate::error::GatewayError;
use crate::ring_buffer::ReplayResult;
use axum::{
    extract::{Path, State},
    response::sse::{Event, KeepAlive, Sse},
    Extension,
};
use axum::http::HeaderMap;
use futures_util::stream::Stream;
use serde_json::json;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

/// GET /v1/sessions/:session_id/messages/stream - SSE endpoint
pub async fn sse_stream(
    State(state): State<Arc<crate::server::SharedState>>,
    Extension(tenant): Extension<TenantContext>,
    Path(session_id): Path<String>,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, GatewayError> {
    eprintln!(
        "[STREAM] SSE connection: tenant={}, session={}",
        tenant.tenant_id, session_id
    );

    // Check for Last-Event-ID header for replay
    let last_event_id = headers
        .get("Last-Event-ID")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());

    if let Some(last_seq) = last_event_id {
        eprintln!("[STREAM] Client requested replay from sequence: {}", last_seq);
    }

    // Subscribe to session messages via OC WebSocket
    let params = serde_json::json!({
        "key": session_id.clone(),
    });

    let mut oc_receiver = state
        .oc_client
        .subscribe("sessions.messages.subscribe", params)
        .await?;

    // Create channel for SSE events
    let (tx, rx) = mpsc::channel(100);

    let ring_buffer = state.ring_buffer.clone();
    let session_id_for_task = session_id.clone();

    // Spawn task to handle events
    tokio::spawn(async move {
        // If replay requested, send buffered events first
        if let Some(last_seq) = last_event_id {
            eprintln!("[STREAM] Replaying events from ring buffer");
            
            match ring_buffer.replay_from(&session_id_for_task, last_seq).await {
                ReplayResult::Events(events) => {
                    eprintln!("[STREAM] Replaying {} buffered events", events.len());
                    for buffered_event in events {
                        let event = Event::default()
                            .id(buffered_event.sequence.to_string())
                            .event(buffered_event.event_type)
                            .json_data(buffered_event.data)
                            .unwrap_or_else(|_| Event::default().data("{}"));

                        if tx.send(Ok(event)).await.is_err() {
                            eprintln!("[STREAM] Client disconnected during replay");
                            return;
                        }
                    }
                }
                ReplayResult::Gap { oldest_available } => {
                    eprintln!(
                        "[STREAM] Replay gap detected: requested {}, oldest {}",
                        last_seq, oldest_available
                    );
                    
                    // Send gap event
                    let gap_event = Event::default()
                        .event("replay.gap")
                        .json_data(json!({
                            "requested_sequence": last_seq,
                            "oldest_available": oldest_available,
                            "message": "Requested events are no longer in buffer"
                        }))
                        .unwrap_or_else(|_| Event::default().data("{}"));

                    if tx.send(Ok(gap_event)).await.is_err() {
                        eprintln!("[STREAM] Client disconnected after gap event");
                        return;
                    }
                }
            }
        }

        // Stream new events from OC WebSocket
        while let Some(notification) = oc_receiver.recv().await {
            eprintln!("[STREAM] Received notification from OC");

            // Parse notification to get event type and data
            let (event_type, data) = parse_notification(&notification);

            // Store in ring buffer
            let buffered_event = ring_buffer
                .push(&session_id_for_task, event_type.clone(), data.clone())
                .await;

            // Send to SSE client
            let event = Event::default()
                .id(buffered_event.sequence.to_string())
                .event(event_type)
                .json_data(data)
                .unwrap_or_else(|_| Event::default().data("{}"));

            if tx.send(Ok(event)).await.is_err() {
                eprintln!("[STREAM] Client disconnected");
                break;
            }
        }

        eprintln!("[STREAM] Event stream ended");
    });

    // Convert receiver to stream
    let stream = ReceiverStream::new(rx);

    // Return SSE response with keep-alive
    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    ))
}

/// Parse OC notification to extract event type and data
fn parse_notification(notification: &serde_json::Value) -> (String, serde_json::Value) {
    // OC notifications should have a "type" field and "data" field
    // Format: { "type": "message.delta", "data": {...} }
    
    let event_type = notification
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("message.delta")
        .to_string();

    let data = notification
        .get("data")
        .cloned()
        .unwrap_or_else(|| notification.clone());

    eprintln!("[STREAM] Parsed event: type={}", event_type);

    (event_type, data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_notification_with_type() {
        let notification = json!({
            "type": "message.complete",
            "data": {
                "message_id": "123",
                "text": "Hello"
            }
        });

        let (event_type, data) = parse_notification(&notification);
        assert_eq!(event_type, "message.complete");
        assert_eq!(data["message_id"], "123");
    }

    #[test]
    fn test_parse_notification_without_type() {
        let notification = json!({
            "message_id": "123",
            "text": "Hello"
        });

        let (event_type, data) = parse_notification(&notification);
        assert_eq!(event_type, "message.delta"); // Default
        assert_eq!(data["message_id"], "123");
    }

    #[test]
    fn test_parse_notification_with_nested_data() {
        let notification = json!({
            "type": "message.error",
            "data": {
                "error": "timeout",
                "details": {
                    "code": 500
                }
            }
        });

        let (event_type, data) = parse_notification(&notification);
        assert_eq!(event_type, "message.error");
        assert_eq!(data["error"], "timeout");
        assert_eq!(data["details"]["code"], 500);
    }
}

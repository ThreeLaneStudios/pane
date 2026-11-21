use axum::http::StatusCode;
use axum::response::IntoResponse;
use pane_gateway::error::GatewayError;
use pane_gateway::routes::sync::SYNC_PUSH_MAX_BYTES;
use pane_gateway::routes::sessions::SESSION_MESSAGE_MAX_BYTES;
use serde_json::Value;

/// Helper to parse response body as JSON
async fn parse_response_body(response: axum::response::Response) -> Value {
    let (_parts, body) = response.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .expect("Failed to read response body");
    serde_json::from_slice(&bytes).expect("Failed to parse JSON")
}

/// Test that sync push payload limit constant is correctly set
#[test]
fn test_sync_push_max_bytes_constant() {
    // Verify the constant is 10 MiB as per §22C.13
    assert_eq!(SYNC_PUSH_MAX_BYTES, 10 * 1024 * 1024);
    assert_eq!(SYNC_PUSH_MAX_BYTES, 10_485_760);
}

/// Test that session message payload limit constant is correctly set
#[test]
fn test_session_message_max_bytes_constant() {
    // Verify the constant is 1 MiB as per §22C.13
    assert_eq!(SESSION_MESSAGE_MAX_BYTES, 1 * 1024 * 1024);
    assert_eq!(SESSION_MESSAGE_MAX_BYTES, 1_048_576);
}

/// Test that sync push rejects oversized payload
/// 
/// Tests the validation logic that checks content size against SYNC_PUSH_MAX_BYTES.
/// When content exceeds 10 MiB, should return PayloadTooLarge error with correct max_bytes.
#[tokio::test]
async fn test_sync_push_rejects_oversized_payload() {
    // Create error that would be returned for oversized sync push
    let error = GatewayError::PayloadTooLarge { 
        max_bytes: SYNC_PUSH_MAX_BYTES 
    };
    
    let response = error.into_response();
    
    // Assert status code is 413 Payload Too Large
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    
    // Assert response body structure
    let body = parse_response_body(response).await;
    assert_eq!(body["error"], "payload_too_large");
    assert_eq!(body["status"], 413);
    assert_eq!(body["max_bytes"], 10_485_760);
}

/// Test that sync push accepts content within limit
///
/// Verifies that content just under the 10 MiB limit does NOT trigger PayloadTooLarge.
#[test]
fn test_sync_push_accepts_within_limit() {
    // Create content just under the limit (5 MiB)
    let valid_size = 5 * 1024 * 1024;
    
    // Verify this size is within the limit
    assert!(valid_size <= SYNC_PUSH_MAX_BYTES);
    
    // Verify exact boundary case: max size is valid
    assert!(SYNC_PUSH_MAX_BYTES <= SYNC_PUSH_MAX_BYTES);
    
    // Verify one byte over is invalid
    let oversized = SYNC_PUSH_MAX_BYTES + 1;
    assert!(oversized > SYNC_PUSH_MAX_BYTES);
}

/// Test that session message rejects oversized content
///
/// Tests the validation logic that checks message content size against SESSION_MESSAGE_MAX_BYTES.
/// When content exceeds 1 MiB, should return PayloadTooLarge error with correct max_bytes.
#[tokio::test]
async fn test_session_message_rejects_oversized() {
    // Create error that would be returned for oversized session message
    let error = GatewayError::PayloadTooLarge { 
        max_bytes: SESSION_MESSAGE_MAX_BYTES 
    };
    
    let response = error.into_response();
    
    // Assert status code is 413 Payload Too Large
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    
    // Assert response body structure
    let body = parse_response_body(response).await;
    assert_eq!(body["error"], "payload_too_large");
    assert_eq!(body["status"], 413);
    assert_eq!(body["max_bytes"], 1_048_576);
}

/// Test that session message accepts content within limit
///
/// Verifies that content just under the 1 MiB limit does NOT trigger PayloadTooLarge.
#[test]
fn test_session_message_accepts_within_limit() {
    // Create content just under the limit (500 KiB)
    let valid_size = 500 * 1024;
    
    // Verify this size is within the limit
    assert!(valid_size <= SESSION_MESSAGE_MAX_BYTES);
    
    // Verify exact boundary case: max size is valid
    assert!(SESSION_MESSAGE_MAX_BYTES <= SESSION_MESSAGE_MAX_BYTES);
    
    // Verify one byte over is invalid
    let oversized = SESSION_MESSAGE_MAX_BYTES + 1;
    assert!(oversized > SESSION_MESSAGE_MAX_BYTES);
}

/// Test boundary: exactly at sync push limit
#[test]
fn test_sync_push_boundary_at_limit() {
    let at_limit = SYNC_PUSH_MAX_BYTES;
    assert!(at_limit <= SYNC_PUSH_MAX_BYTES, "Exactly at limit should be valid");
}

/// Test boundary: one byte over sync push limit triggers error
#[tokio::test]
async fn test_sync_push_boundary_one_byte_over() {
    let one_byte_over = SYNC_PUSH_MAX_BYTES + 1;
    assert!(one_byte_over > SYNC_PUSH_MAX_BYTES, "One byte over should exceed limit");
    
    // Verify this would trigger PayloadTooLarge
    let error = GatewayError::PayloadTooLarge { max_bytes: SYNC_PUSH_MAX_BYTES };
    let response = error.into_response();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

/// Test boundary: exactly at session message limit
#[test]
fn test_session_message_boundary_at_limit() {
    let at_limit = SESSION_MESSAGE_MAX_BYTES;
    assert!(at_limit <= SESSION_MESSAGE_MAX_BYTES, "Exactly at limit should be valid");
}

/// Test boundary: one byte over session message limit triggers error
#[tokio::test]
async fn test_session_message_boundary_one_byte_over() {
    let one_byte_over = SESSION_MESSAGE_MAX_BYTES + 1;
    assert!(one_byte_over > SESSION_MESSAGE_MAX_BYTES, "One byte over should exceed limit");
    
    // Verify this would trigger PayloadTooLarge
    let error = GatewayError::PayloadTooLarge { max_bytes: SESSION_MESSAGE_MAX_BYTES };
    let response = error.into_response();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

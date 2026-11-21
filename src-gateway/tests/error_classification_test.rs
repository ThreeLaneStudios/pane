use axum::http::StatusCode;
use axum::response::IntoResponse;
use pane_gateway::error::GatewayError;
use serde_json::Value;

/// Helper to parse response body as JSON
async fn parse_response_body(response: axum::response::Response) -> Value {
    let (_parts, body) = response.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .expect("Failed to read response body");
    serde_json::from_slice(&bytes).expect("Failed to parse JSON")
}

#[tokio::test]
async fn test_oc_unauthorized_maps_to_502() {
    let error = GatewayError::OcUnauthorized;
    let response = error.into_response();
    
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    
    let body = parse_response_body(response).await;
    assert_eq!(body["error"], "oc_unauthorized");
    assert_eq!(body["status"], 502);
}

#[tokio::test]
async fn test_oc_forbidden_maps_to_502() {
    let error = GatewayError::OcForbidden;
    let response = error.into_response();
    
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    
    let body = parse_response_body(response).await;
    assert_eq!(body["error"], "oc_forbidden");
    assert_eq!(body["status"], 502);
}

#[tokio::test]
async fn test_oc_rate_limited_maps_to_429_with_retry_after() {
    let error = GatewayError::OcRateLimited { retry_after: Some(30) };
    let response = error.into_response();
    
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    
    let body = parse_response_body(response).await;
    assert_eq!(body["error"], "oc_rate_limited");
    assert_eq!(body["status"], 429);
    assert_eq!(body["retry_after"], 30);
}

#[tokio::test]
async fn test_oc_rate_limited_without_retry_after() {
    let error = GatewayError::OcRateLimited { retry_after: None };
    let response = error.into_response();
    
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    
    let body = parse_response_body(response).await;
    assert_eq!(body["error"], "oc_rate_limited");
    assert_eq!(body["status"], 429);
    assert!(body.get("retry_after").is_none());
}

#[tokio::test]
async fn test_oc_server_error_maps_to_502() {
    let error = GatewayError::OcServerError;
    let response = error.into_response();
    
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    
    let body = parse_response_body(response).await;
    assert_eq!(body["error"], "oc_server_error");
    assert_eq!(body["status"], 502);
}

#[tokio::test]
async fn test_oc_unavailable_maps_to_503() {
    let error = GatewayError::OcUnavailable { retry_after: Some(60) };
    let response = error.into_response();
    
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    
    let body = parse_response_body(response).await;
    assert_eq!(body["error"], "oc_unavailable");
    assert_eq!(body["status"], 503);
    assert_eq!(body["retry_after"], 60);
}

#[tokio::test]
async fn test_oc_timeout_maps_to_504() {
    let error = GatewayError::OcTimeout;
    let response = error.into_response();
    
    assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
    
    let body = parse_response_body(response).await;
    assert_eq!(body["error"], "oc_timeout");
    assert_eq!(body["status"], 504);
}

#[tokio::test]
async fn test_oc_connection_lost_maps_to_502() {
    let error = GatewayError::OcConnectionLost;
    let response = error.into_response();
    
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    
    let body = parse_response_body(response).await;
    assert_eq!(body["error"], "oc_connection_lost");
    assert_eq!(body["status"], 502);
}

#[tokio::test]
async fn test_payload_too_large_maps_to_413() {
    let error = GatewayError::PayloadTooLarge { max_bytes: 10_485_760 };
    let response = error.into_response();
    
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    
    let body = parse_response_body(response).await;
    assert_eq!(body["error"], "payload_too_large");
    assert_eq!(body["status"], 413);
    assert_eq!(body["max_bytes"], 10_485_760);
}

#[tokio::test]
async fn test_unauthorized_maps_to_401() {
    let error = GatewayError::Unauthorized("Invalid token".to_string());
    let response = error.into_response();
    
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    
    let body = parse_response_body(response).await;
    assert_eq!(body["error"], "unauthorized");
    assert_eq!(body["status"], 401);
}

#[tokio::test]
async fn test_not_found_maps_to_404() {
    let error = GatewayError::NotFound;
    let response = error.into_response();
    
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    
    let body = parse_response_body(response).await;
    assert_eq!(body["error"], "not_found");
    assert_eq!(body["status"], 404);
}

#[tokio::test]
async fn test_invalid_request_maps_to_400() {
    let error = GatewayError::InvalidRequest("bad field".to_string());
    let response = error.into_response();
    
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    
    let body = parse_response_body(response).await;
    assert_eq!(body["error"], "invalid_request");
    assert_eq!(body["status"], 400);
}

#[tokio::test]
async fn test_error_response_json_structure() {
    let error = GatewayError::InvalidRequest("bad field".to_string());
    let response = error.into_response();
    
    let body = parse_response_body(response).await;
    
    // Assert all required keys are present
    assert!(body.get("error").is_some(), "Missing 'error' field");
    assert!(body.get("status").is_some(), "Missing 'status' field");
    assert!(body.get("message").is_some(), "Missing 'message' field");
}

#[tokio::test]
async fn test_error_response_with_retry_after() {
    let error = GatewayError::OcRateLimited { retry_after: Some(120) };
    let response = error.into_response();
    
    let body = parse_response_body(response).await;
    assert_eq!(body["retry_after"], 120);
}

#[tokio::test]
async fn test_error_response_with_max_bytes() {
    let error = GatewayError::PayloadTooLarge { max_bytes: 1_048_576 };
    let response = error.into_response();
    
    let body = parse_response_body(response).await;
    assert_eq!(body["max_bytes"], 1_048_576);
}

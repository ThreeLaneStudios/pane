use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

/// Structured error response
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
    pub status: u16,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<usize>,
}

/// Gateway error types
#[derive(Debug)]
pub enum GatewayError {
    // Infrastructure errors
    Io(std::io::Error),
    Tls(String),
    Config(String),
    NotImplemented,
    
    // Client errors (4xx)
    InvalidRequest(String),
    Unauthorized(String),
    NotFound,
    PayloadTooLarge { max_bytes: usize },
    
    // OC-originated errors (mapped to appropriate status codes)
    OcUnauthorized,
    OcForbidden,
    OcRateLimited { retry_after: Option<u64> },
    OcServerError,
    OcUnavailable { retry_after: Option<u64> },
    OcTimeout,
    OcConnectionLost,
    OcHandshakeTimeout,
    OcClient(String),
    /// #3106 H1 part 3: the active pairing code has received
    /// `MAX_PAIRING_ATTEMPTS` wrong guesses. Bounded by the code's own
    /// remaining validity — `retry_after` is derived from `expires_at`, not a
    /// separate lockout timer. Same shape as `OcRateLimited` (429 +
    /// `retry_after`) so clients already handling that variant need no new
    /// branch.
    PairingLockedOut { retry_after: u64 },
    /// A well-formed `{"ok": false, "error": {"code", "message"}}` RPC
    /// response from OC itself (#3050 fix PR, Bug 1(b) secondary/gateway
    /// commit). Distinct from `OcClient(String)`, which covers every OTHER
    /// client-side failure on this same enum (serialization, write-channel,
    /// method-not-allowed, auth-channel-closed, etc.) — none of those carry
    /// an OC-assigned `code`, so they correctly stay blanket-502 via
    /// `OcClient`. This variant exists so `oc_client.rs`'s RPC-error handler
    /// can preserve the `code` OC actually sent instead of discarding it,
    /// and `IntoResponse` below can map specific, named codes to an honest
    /// status instead of lumping every OC rejection into one generic 502.
    OcRpcError { code: String, message: String },
}

impl std::fmt::Display for GatewayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "IO error: {}", e),
            Self::Tls(e) => write!(f, "TLS error: {}", e),
            Self::Config(e) => write!(f, "Configuration error: {}", e),
            Self::NotImplemented => write!(f, "Not implemented"),
            Self::InvalidRequest(msg) => write!(f, "Invalid request: {}", msg),
            Self::Unauthorized(msg) => write!(f, "Unauthorized: {}", msg),
            Self::NotFound => write!(f, "Not found"),
            Self::PayloadTooLarge { max_bytes } => write!(f, "Payload too large (max {} bytes)", max_bytes),
            Self::OcUnauthorized => write!(f, "OpenClaw unauthorized"),
            Self::OcForbidden => write!(f, "OpenClaw forbidden"),
            Self::OcRateLimited { .. } => write!(f, "OpenClaw rate limited"),
            Self::OcServerError => write!(f, "OpenClaw server error"),
            Self::OcUnavailable { .. } => write!(f, "OpenClaw unavailable"),
            Self::OcTimeout => write!(f, "OpenClaw timeout"),
            Self::OcConnectionLost => write!(f, "OpenClaw connection lost"),
            Self::OcHandshakeTimeout => write!(f, "OpenClaw handshake timeout"),
            Self::OcClient(msg) => write!(f, "OC client error: {}", msg),
            Self::OcRpcError { code, message } => write!(f, "OC RPC error [{}]: {}", code, message),
            Self::PairingLockedOut { .. } => write!(f, "Pairing code locked out after too many wrong attempts"),
        }
    }
}

impl std::error::Error for GatewayError {}

impl From<std::io::Error> for GatewayError {
    fn from(err: std::io::Error) -> Self {
        GatewayError::Io(err)
    }
}

impl From<rcgen::Error> for GatewayError {
    fn from(err: rcgen::Error) -> Self {
        GatewayError::Tls(err.to_string())
    }
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        let (status, error_code, message, retry_after, max_bytes) = match self {
            Self::NotImplemented => (
                StatusCode::NOT_IMPLEMENTED,
                "not_implemented",
                "This endpoint is not yet implemented".to_string(),
                None,
                None,
            ),
            Self::Config(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "config_error",
                msg,
                None,
                None,
            ),
            Self::Tls(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "tls_error",
                msg,
                None,
                None,
            ),
            Self::Io(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "io_error",
                e.to_string(),
                None,
                None,
            ),
            Self::InvalidRequest(msg) => (
                StatusCode::BAD_REQUEST,
                "invalid_request",
                msg,
                None,
                None,
            ),
            Self::Unauthorized(msg) => (
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                msg,
                None,
                None,
            ),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "not_found",
                "Resource not found".to_string(),
                None,
                None,
            ),
            Self::PayloadTooLarge { max_bytes } => (
                StatusCode::PAYLOAD_TOO_LARGE,
                "payload_too_large",
                format!("Request body exceeds maximum size of {} bytes", max_bytes),
                None,
                Some(max_bytes),
            ),
            Self::OcUnauthorized => (
                StatusCode::BAD_GATEWAY,
                "oc_unauthorized",
                "Gateway cannot authenticate to OpenClaw".to_string(),
                None,
                None,
            ),
            Self::OcForbidden => (
                StatusCode::BAD_GATEWAY,
                "oc_forbidden",
                "OpenClaw rejected gateway request".to_string(),
                None,
                None,
            ),
            Self::OcRateLimited { retry_after } => (
                StatusCode::TOO_MANY_REQUESTS,
                "oc_rate_limited",
                "OpenClaw rate limited".to_string(),
                retry_after,
                None,
            ),
            Self::OcServerError => (
                StatusCode::BAD_GATEWAY,
                "oc_server_error",
                "OpenClaw internal server error".to_string(),
                None,
                None,
            ),
            Self::OcUnavailable { retry_after } => (
                StatusCode::SERVICE_UNAVAILABLE,
                "oc_unavailable",
                "OpenClaw service unavailable".to_string(),
                retry_after,
                None,
            ),
            Self::OcTimeout => (
                StatusCode::GATEWAY_TIMEOUT,
                "oc_timeout",
                "Gateway timeout waiting for OpenClaw".to_string(),
                None,
                None,
            ),
            Self::OcConnectionLost => (
                StatusCode::BAD_GATEWAY,
                "oc_connection_lost",
                "OpenClaw connection lost".to_string(),
                None,
                None,
            ),
            Self::OcHandshakeTimeout => (
                StatusCode::BAD_GATEWAY,
                "oc_handshake_timeout",
                "Gateway did not receive hello-ok from OpenClaw within 10 seconds".to_string(),
                None,
                None,
            ),
            Self::OcClient(msg) => (
                StatusCode::BAD_GATEWAY,
                "oc_client_error",
                msg,
                None,
                None,
            ),
            Self::PairingLockedOut { retry_after } => (
                StatusCode::TOO_MANY_REQUESTS,
                "pairing_locked_out",
                "Too many wrong pairing attempts for the active code".to_string(),
                Some(retry_after),
                None,
            ),
            // #3050 fix PR, Bug 1(b) secondary/gateway commit: preserve the
            // distinction OC's own `error.code` draws instead of collapsing
            // every RPC rejection into a blanket 502. `INVALID_REQUEST` is
            // OC's own code for "the request was syntactically fine but
            // referred to something that doesn't exist / failed validation"
            // (e.g. "session not found: <key>", the exact case live-test
            // #1b hit) — mapped to 400, not 404: Stia's call, named here —
            // 400 keeps a direct one-to-one mapping to OC's OWN semantic
            // category (a bad/invalid request OC rejected), rather than
            // Pane re-interpreting "session not found" into a different
            // category (404) that OC itself never asserted. Any OTHER OC
            // error code not named here falls back to the same blanket 502
            // `OcClient(_)` always used — this narrows the 502 class, it
            // never removes it as a default for codes not yet recognized.
            Self::OcRpcError { code, message } => match code.as_str() {
                "INVALID_REQUEST" => (
                    StatusCode::BAD_REQUEST,
                    "oc_invalid_request",
                    message,
                    None,
                    None,
                ),
                _ => (
                    StatusCode::BAD_GATEWAY,
                    "oc_client_error",
                    message,
                    None,
                    None,
                ),
            },
        };

        let body = Json(ErrorResponse {
            error: error_code.to_string(),
            status: status.as_u16(),
            message,
            retry_after,
            max_bytes,
        });

        (status, body).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_payload_too_large_error_format() {
        let error = GatewayError::PayloadTooLarge { max_bytes: 10_485_760 };
        let response = error.into_response();
        
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[test]
    fn test_oc_rate_limited_includes_retry_after() {
        let error = GatewayError::OcRateLimited { retry_after: Some(30) };
        let response = error.into_response();
        
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[test]
    fn test_oc_server_error_maps_to_502() {
        let error = GatewayError::OcServerError;
        let response = error.into_response();
        
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn test_oc_unavailable_maps_to_503() {
        let error = GatewayError::OcUnavailable { retry_after: Some(60) };
        let response = error.into_response();
        
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn test_oc_handshake_timeout_maps_to_502() {
        let error = GatewayError::OcHandshakeTimeout;
        let response = error.into_response();
        // §22C.5.4: OcHandshakeTimeout MUST map to HTTP 502 (not 504)
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn test_oc_handshake_timeout_display() {
        let error = GatewayError::OcHandshakeTimeout;
        assert_eq!(error.to_string(), "OpenClaw handshake timeout");
    }

    /// #3050 fix PR, Bug 1(b) secondary/gateway commit — item 10 of Reed's
    /// own build brief: mock an OC `{"ok":false,"error":{"code":"INVALID_REQUEST",...}}`
    /// response and assert the gateway's HTTP response is the new 4xx, not
    /// `502`. **Mutation (actually run — see report):** reverted the
    /// `OcRpcError` match arm to always return `StatusCode::BAD_GATEWAY`
    /// regardless of `code` (i.e. collapsed back to the pre-fix behavior).
    /// Failure: `assert_eq!(response.status(), StatusCode::BAD_REQUEST)`
    /// reddens — status is actually `502`.
    #[test]
    fn test_oc_rpc_error_invalid_request_maps_to_400_not_502() {
        let error = GatewayError::OcRpcError {
            code: "INVALID_REQUEST".to_string(),
            message: "session not found: agent:main:openai-user:pane-test".to_string(),
        };
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "an OC INVALID_REQUEST rejection is a clean 4xx, not an upstream-failure 502");
    }

    /// Item 11 of Reed's own build brief: this narrowing must only apply to
    /// the new `OcRpcError` variant's own named codes — it must never touch
    /// the pre-existing, dedicated `OcServerError`/`OcUnavailable`/`OcTimeout`/
    /// `OcConnectionLost` variants, which are real, distinct upstream-failure
    /// shapes with their own already-correct status codes. Re-asserts the
    /// SAME four pre-existing tests above (`test_oc_server_error_maps_to_502`,
    /// `test_oc_unavailable_maps_to_503`, `test_oc_handshake_timeout_maps_to_502`)
    /// still hold, plus a dedicated `OcTimeout` pin not otherwise covered by
    /// name in this file.
    #[test]
    fn test_oc_timeout_unaffected_by_oc_rpc_error_narrowing() {
        let error = GatewayError::OcTimeout;
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT, "OcTimeout is a real upstream-failure shape, untouched by the OcRpcError code-narrowing");
    }

    /// An OC RPC error code NOT specifically named (anything other than
    /// `INVALID_REQUEST`) must still fall back to the pre-existing blanket
    /// `502` — this narrowing only ADDS a specific 4xx mapping for a named
    /// code, it never removes the 502 default for codes not yet recognized.
    #[test]
    fn test_oc_rpc_error_unknown_code_falls_back_to_502() {
        let error = GatewayError::OcRpcError {
            code: "SOME_FUTURE_OC_CODE".to_string(),
            message: "unrecognized".to_string(),
        };
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY, "an unnamed OC error code must still fall back to the pre-existing blanket 502, not panic or default to something else");
    }

    /// #3106 H1 part 3: lockout is a 429, same shape as `OcRateLimited`.
    #[test]
    fn test_pairing_locked_out_maps_to_429_with_retry_after() {
        let error = GatewayError::PairingLockedOut { retry_after: 120 };
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[test]
    fn test_oc_handshake_timeout_distinct_from_connection_lost() {
        // Verify OcHandshakeTimeout and OcConnectionLost are distinct variants
        // (spec §22C.5.4: detection MUST NOT conflate the two 502 producers)
        let handshake_timeout = GatewayError::OcHandshakeTimeout;
        let connection_lost = GatewayError::OcConnectionLost;
        assert_ne!(handshake_timeout.to_string(), connection_lost.to_string());
    }
}

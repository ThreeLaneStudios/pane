use reqwest;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const SUPABASE_URL: &str = "https://jngkiljhmnrygdnfvrgp.supabase.co";
const SUPABASE_ANON_KEY: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJpc3MiOiJzdXBhYmFzZSIsInJlZiI6ImpuZ2tpbGpobW5yeWdkbmZ2cmdwIiwicm9sZSI6ImFub24iLCJpYXQiOjE3NzIyMzUwNzEsImV4cCI6MjA4NzgxMTA3MX0.xe6N-2KaY0Tx2eYI-UZJrlSa3v2EYZJT-PMVFRDPnzI";

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AuthSession {
    pub access_token: String,
    pub refresh_token: String,
    pub user_id: String,
    pub email: String,
    pub expires_at: i64,
    pub device_id: Option<String>,
}

// Retained as a compatibility response shape for legacy auth callers.
#[allow(dead_code)]
#[derive(Serialize, Deserialize)]
pub struct AuthError {
    pub message: String,
}

/// ADR-0070 Decision 4 (#3195 PR C): sign-in errors GoTrue can return,
/// keyed on HTTP status only — not on GoTrue's error `code`/`error_code`
/// field, which is version-dependent (`invalid_grant` on some GoTrue
/// releases, `invalid_credentials` on others, for the exact same wrong-
/// password case). Spec: docs/spec/onboarding.md §6.1.1.
// `retry_after_seconds` and `NetworkError`'s message are structural —
// Display (below) intentionally doesn't echo them into the user-facing
// string (the spec's fixed copy doesn't need the number or the raw
// network error text), but they're asserted directly by the mockito
// tests below and kept for any future caller that wants to branch on
// them rather than the Display string. Same convention as `AuthError`
// above.
#[allow(dead_code)]
#[derive(Debug)]
pub enum SignInError {
    /// 400 — wrong password or unknown email. GoTrue does not distinguish
    /// the two on this endpoint (ADR-0029's privacy posture: no account-
    /// derived metadata on an anonymous endpoint), so Pane doesn't either.
    InvalidCredentials,
    /// 429 — GoTrue sends no retry-after header/body field on this
    /// endpoint, so Pane uses a fixed interval rather than parsing one.
    RateLimited { retry_after_seconds: u32 },
    NetworkError(String),
    ServerError(String),
}

impl std::fmt::Display for SignInError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SignInError::InvalidCredentials => write!(f, "Email or password is incorrect."),
            SignInError::RateLimited { .. } => {
                write!(f, "Too many sign-in attempts. Try again shortly.")
            }
            SignInError::NetworkError(_) => write!(f, "Couldn't reach Pane. Check your connection."),
            SignInError::ServerError(msg) => write!(f, "{}", msg),
        }
    }
}

/// Parse a GoTrue token response (`access_token`, `refresh_token`,
/// `user.id`, `user.email`, `expires_at`) into an `AuthSession`. Shared by
/// `sign_in_with_url` and `refresh_session` — both consume the exact same
/// response shape from the same GoTrue endpoint family
/// (`/auth/v1/token?grant_type=...`), so the parsing lives in one place
/// rather than two copies that could drift (ADR-0070 Decision 4: "parsed
/// with the same shape refresh_session already uses").
fn parse_auth_session_response(body: &Value) -> Result<AuthSession, String> {
    let access_token = body
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "No access_token in response".to_string())?
        .to_string();

    let refresh_token = body
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "No refresh_token in response".to_string())?
        .to_string();

    let user = body
        .get("user")
        .ok_or_else(|| "No user in response".to_string())?;

    let user_id = user
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "No user id in response".to_string())?
        .to_string();

    let email = user
        .get("email")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "No email in response".to_string())?
        .to_string();

    let expires_at = body
        .get("expires_at")
        .and_then(|v| v.as_i64())
        .unwrap_or_else(|| chrono::Utc::now().timestamp() + 3600);

    Ok(AuthSession {
        access_token,
        refresh_token,
        user_id,
        email,
        expires_at,
        device_id: None,
    })
}

/// Sign in with email/password directly against Supabase GoTrue
/// (ADR-0070 Decision 4, #3195 PR C). `install_uuid` and `machine_name`
/// are no longer sent — the ecom wrapper that consumed them is retired.
pub async fn sign_in(email: &str, password: &str) -> Result<AuthSession, SignInError> {
    sign_in_with_url(SUPABASE_URL, email, password).await
}

/// Internal sign_in with configurable URL (for testing — mockito only,
/// never a real provider endpoint).
pub(crate) async fn sign_in_with_url(
    base_url: &str,
    email: &str,
    password: &str,
) -> Result<AuthSession, SignInError> {
    eprintln!("[SIGN_IN] Attempting GoTrue sign-in for email: {}", email);

    let client = reqwest::Client::new();
    let response = client
        .post(format!("{}/auth/v1/token?grant_type=password", base_url))
        .header("apikey", SUPABASE_ANON_KEY)
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "email": email,
            "password": password
        }))
        .send()
        .await
        .map_err(|e| SignInError::NetworkError(format!("Failed to reach server: {}", e)))?;

    let status = response.status();

    let body: Value = response
        .json()
        .await
        .map_err(|e| SignInError::ServerError(format!("Failed to parse response: {}", e)))?;

    if !status.is_success() {
        eprintln!("[SIGN_IN] GoTrue error response: status={}, body={:?}", status, body);

        // Keyed on status, not on GoTrue's error code — see SignInError's
        // doc comment. 400 covers both wrong-password and unknown-email;
        // GoTrue does not distinguish them on this endpoint.
        return match status.as_u16() {
            400 => Err(SignInError::InvalidCredentials),
            429 => Err(SignInError::RateLimited { retry_after_seconds: 30 }),
            _ => {
                let msg = body
                    .get("msg")
                    .or_else(|| body.get("error_description"))
                    .or_else(|| body.get("message"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown error")
                    .to_string();
                Err(SignInError::ServerError(msg))
            }
        };
    }

    let session = parse_auth_session_response(&body).map_err(SignInError::ServerError)?;
    eprintln!("[SIGN_IN] Sign-in successful for user: {}", session.user_id);
    Ok(session)
}

// Refresh an expired token (still calls Supabase for now)
pub async fn refresh_session(refresh_token: &str) -> Result<AuthSession, String> {
    let client = reqwest::Client::new();
    let res = client
        .post(format!("{}/auth/v1/token?grant_type=refresh_token", SUPABASE_URL))
        .header("apikey", SUPABASE_ANON_KEY)
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "refresh_token": refresh_token
        }))
        .send()
        .await
        .map_err(|e| format!("Network error: {}", e))?;

    let status = res.status();
    let body: Value = res
        .json()
        .await
        .map_err(|e| format!("Failed to parse response: {}", e))?;

    if !status.is_success() {
        let error_msg = body
            .get("message")
            .or_else(|| body.get("msg"))
            .or_else(|| body.get("error_description"))
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown error");
        return Err(error_msg.to_string());
    }

    parse_auth_session_response(&body)
}

// Sign out
pub async fn sign_out(access_token: &str) -> Result<(), String> {
    let client = reqwest::Client::new();
    let _res = client
        .post(format!("{}/auth/v1/logout", SUPABASE_URL))
        .header("apikey", SUPABASE_ANON_KEY)
        .header("Authorization", format!("Bearer {}", access_token))
        .send()
        .await
        .map_err(|e| format!("Network error: {}", e))?;
    
    Ok(())
}

// Request password reset email
pub async fn request_password_reset(email: &str) -> Result<(), String> {
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{}/auth/v1/recover", SUPABASE_URL))
        .header("apikey", SUPABASE_ANON_KEY)
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "email": email,
            "gotrue_meta_supabase": {
                "redirect_to": "pane://auth/recovery"
            }
        }))
        .send()
        .await
        .map_err(|e| format!("Failed to send reset email: {}", e))?;
    
    if !response.status().is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Reset request failed: {}", body));
    }
    
    Ok(())
}

// Change password for authenticated user
// Returns rotated tokens: (access_token, refresh_token, expires_at)
pub async fn change_password(access_token: &str, new_password: &str) -> Result<(String, String, i64), String> {
    eprintln!("[CHANGE_PASSWORD] Sending PUT /auth/v1/user to Supabase");
    
    let client = reqwest::Client::new();
    let response = client
        .put(format!("{}/auth/v1/user", SUPABASE_URL))
        .header("apikey", SUPABASE_ANON_KEY)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "password": new_password
        }))
        .send()
        .await
        .map_err(|e| format!("Failed to update password: {}", e))?;
    
    if !response.status().is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Password update failed: {}", body));
    }
    
    // Parse response body for rotated tokens
    let body: Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse response body: {}", e))?;
    
    eprintln!("[CHANGE_PASSWORD] Response body keys: {:?}", body.as_object().map(|o| o.keys().collect::<Vec<_>>()));
    
    let new_access_token = body
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "No access_token in response".to_string())?
        .to_string();
    
    let new_refresh_token = body
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "No refresh_token in response".to_string())?
        .to_string();
    
    // Check if Supabase returns a fresh expires_at
    let expires_at = body
        .get("expires_at")
        .and_then(|v| v.as_i64())
        .unwrap_or_else(|| {
            eprintln!("[CHANGE_PASSWORD] No expires_at in response; will preserve existing value");
            0 // Sentinel value; caller will preserve existing expires_at
        });
    
    eprintln!("[CHANGE_PASSWORD] Successfully parsed rotated tokens (expires_at from response: {})", expires_at);
    
    Ok((new_access_token, new_refresh_token, expires_at))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::ServerGuard;

    // Helper to create a mock server and return its URL. mockito only —
    // no test here ever dispatches to a real provider endpoint.
    async fn setup_mock_server() -> ServerGuard {
        mockito::Server::new_async().await
    }

    // ADR-0070 Decision 4 (#3195 PR C): sign_in now targets GoTrue's
    // `/auth/v1/token?grant_type=password` and parses the same shape
    // `parse_auth_session_response` gives `refresh_session` — success,
    // the two status-keyed error cases from the spec (§6.1.1), and a
    // network-unreachable edge case (happy path / error path / edge case,
    // per the build standard). The four old tests this module carried
    // (machine-id matching/mismatching/autoclaim, wrapper rate-limit) are
    // deleted with the wrapper-bound `sign_in_with_url` signature they
    // exercised — `install_uuid`/`machine_name` are no longer parameters.

    #[tokio::test]
    async fn test_sign_in_success_parses_gotrue_session() {
        let mut server = setup_mock_server().await;

        let mock = server
            .mock("POST", "/auth/v1/token?grant_type=password")
            .match_header("apikey", mockito::Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::json!({
                    "access_token": "gotrue-access-token",
                    "refresh_token": "gotrue-refresh-token",
                    "expires_at": 9999999999i64,
                    "user": {
                        "id": "user-123",
                        "email": "test@example.com"
                    }
                })
                .to_string(),
            )
            .create_async()
            .await;

        let result = sign_in_with_url(&server.url(), "test@example.com", "password123").await;

        mock.assert_async().await;
        let session = result.expect("sign_in_with_url should succeed against a 200 GoTrue response");
        assert_eq!(session.access_token, "gotrue-access-token");
        assert_eq!(session.refresh_token, "gotrue-refresh-token");
        assert_eq!(session.user_id, "user-123");
        assert_eq!(session.email, "test@example.com");
        assert_eq!(session.expires_at, 9999999999);
        assert_eq!(session.device_id, None);
    }

    #[tokio::test]
    async fn test_sign_in_400_invalid_grant_keyed_on_status_not_code() {
        let mut server = setup_mock_server().await;

        // Older GoTrue releases use "invalid_grant" for wrong password /
        // unknown email on this endpoint.
        let mock = server
            .mock("POST", "/auth/v1/token?grant_type=password")
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(r#"{"error": "invalid_grant", "error_description": "Invalid login credentials"}"#)
            .create_async()
            .await;

        let result = sign_in_with_url(&server.url(), "test@example.com", "wrongpass").await;

        mock.assert_async().await;
        assert!(matches!(result, Err(SignInError::InvalidCredentials)));
    }

    #[tokio::test]
    async fn test_sign_in_400_invalid_credentials_keyed_on_status_not_code() {
        let mut server = setup_mock_server().await;

        // Newer GoTrue releases use "invalid_credentials" for the exact
        // same case. Pane must key on the 400 status in both cases — not
        // on which string GoTrue happens to send this version.
        let mock = server
            .mock("POST", "/auth/v1/token?grant_type=password")
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(r#"{"error_code": "invalid_credentials", "msg": "Invalid login credentials"}"#)
            .create_async()
            .await;

        let result = sign_in_with_url(&server.url(), "unknown@example.com", "password123").await;

        mock.assert_async().await;
        assert!(matches!(result, Err(SignInError::InvalidCredentials)));
    }

    #[tokio::test]
    async fn test_sign_in_429_rate_limited_fixed_interval_not_from_response() {
        let mut server = setup_mock_server().await;

        // GoTrue sends no retry-after header or field on this endpoint —
        // the mock body's "retry_after_seconds": 9999 must be IGNORED;
        // Pane always uses its own fixed 30s interval.
        let mock = server
            .mock("POST", "/auth/v1/token?grant_type=password")
            .with_status(429)
            .with_header("content-type", "application/json")
            .with_body(r#"{"error": "too_many_requests", "retry_after_seconds": 9999}"#)
            .create_async()
            .await;

        let result = sign_in_with_url(&server.url(), "test@example.com", "password123").await;

        mock.assert_async().await;
        assert!(matches!(
            result,
            Err(SignInError::RateLimited { retry_after_seconds: 30 })
        ));
    }

    #[tokio::test]
    async fn test_sign_in_network_unreachable() {
        // Edge case: no mock server at all — a closed local port, never a
        // real provider endpoint. Proves the NetworkError path without
        // spending any real credit or making any real network call.
        let result = sign_in_with_url("http://127.0.0.1:1", "test@example.com", "password123").await;
        assert!(matches!(result, Err(SignInError::NetworkError(_))));
    }
}

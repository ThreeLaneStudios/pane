use crate::error::GatewayError;
use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use chrono::{DateTime, Duration, Utc};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Pairing request body
#[derive(Debug, Deserialize)]
pub struct PairRequest {
    pub code: String,
}

/// Pairing response
#[derive(Debug, Serialize)]
pub struct PairResponse {
    pub token: String,
    pub expires_at: String,
    pub tenant_id: String,
    pub gateway_version: String,
}

/// Token metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenInfo {
    pub token: String,
    pub tenant_id: String,
    pub expires_at: DateTime<Utc>,
}

/// Maximum wrong guesses accepted against one active pairing code before it
/// locks out (#3106 H1 part 3). Well above the realistic typo count (1,
/// occasionally 2 — see `test_typo_tolerance_wrong_code_preserves_active_slot`)
/// and low enough that even full use of the budget is a vanishing fraction of
/// the 10^8 code space within the code's own 5-minute window.
pub const MAX_PAIRING_ATTEMPTS: u32 = 10;

/// Pairing code metadata
#[derive(Debug, Clone)]
pub struct PairingCode {
    pub code: String,
    pub expires_at: DateTime<Utc>,
    /// Wrong guesses against THIS code instance. Lives and dies with the
    /// code — a fresh `generate_pairing_code()` call always starts a new
    /// instance with `attempts: 0`, which is also the DoS-escape property:
    /// the owner's own fresh-code path is never blocked by an attacker who
    /// locked out a previous code.
    pub attempts: u32,
}

/// Authentication state shared across handlers
#[derive(Clone)]
pub struct AuthState {
    tokens: Arc<RwLock<HashMap<String, TokenInfo>>>,
    pairing_code: Arc<RwLock<Option<PairingCode>>>,
    tokens_path: PathBuf,
}

impl AuthState {
    pub fn new(tokens_path: PathBuf) -> Self {
        eprintln!("[AUTH] Initializing with tokens file: {}", tokens_path.display());
        
        // Load existing tokens from disk
        let tokens = Self::load_tokens(&tokens_path);
        eprintln!("[AUTH] Loaded {} token(s) from disk", tokens.len());
        
        Self {
            tokens: Arc::new(RwLock::new(tokens)),
            pairing_code: Arc::new(RwLock::new(None)),
            tokens_path,
        }
    }
    
    /// Load tokens from disk, filtering out expired ones
    fn load_tokens(tokens_path: &PathBuf) -> HashMap<String, TokenInfo> {
        match fs::read_to_string(tokens_path) {
            Ok(contents) => {
                match serde_json::from_str::<Vec<TokenInfo>>(&contents) {
                    Ok(token_list) => {
                        let now = Utc::now();
                        let mut valid_tokens = HashMap::new();
                        let mut expired_count = 0;
                        
                        for token_info in token_list {
                            if token_info.expires_at > now {
                                valid_tokens.insert(token_info.token.clone(), token_info);
                            } else {
                                expired_count += 1;
                            }
                        }
                        
                        if expired_count > 0 {
                            eprintln!("[AUTH] Pruned {} expired token(s) during load", expired_count);
                        }
                        
                        valid_tokens
                    }
                    Err(e) => {
                        eprintln!("[AUTH] Warning: failed to parse tokens file: {} — starting with empty token store", e);
                        HashMap::new()
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("[AUTH] Tokens file not found — starting with empty token store");
                HashMap::new()
            }
            Err(e) => {
                eprintln!("[AUTH] Warning: failed to read tokens file: {} — starting with empty token store", e);
                HashMap::new()
            }
        }
    }
    
    /// Save tokens to disk atomically
    async fn save_tokens(&self, tokens: &tokio::sync::RwLockReadGuard<'_, HashMap<String, TokenInfo>>) -> Result<(), GatewayError> {
        // Convert HashMap to Vec for serialization
        let token_list: Vec<TokenInfo> = tokens.values().cloned().collect();
        let json = serde_json::to_string_pretty(&token_list)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("Failed to serialize tokens: {}", e)))?;
        
        // Write to temporary file first
        let temp_path = self.tokens_path.with_extension("json.tmp");
        let mut file = fs::File::create(&temp_path)?;
        
        file.write_all(json.as_bytes())?;
        
        file.sync_all()?;
        
        // Set permissions to 0600 (owner read/write only)
        let permissions = fs::Permissions::from_mode(0o600);
        fs::set_permissions(&temp_path, permissions)?;
        
        // Atomic rename
        fs::rename(&temp_path, &self.tokens_path)?;
        
        eprintln!("[AUTH] Saved {} token(s) to disk", token_list.len());
        Ok(())
    }

    /// Generate a new 8-digit pairing code.
    /// #3106 H1 part 3: widened from 6 to 8 digits alongside the attempt
    /// throttle — entropy "as well as," not "instead of," throttling.
    pub async fn generate_pairing_code(&self) -> String {
        let mut rng = rand::thread_rng();
        let code = format!("{:08}", rng.gen_range(0..100_000_000u32));
        let expires_at = Utc::now() + Duration::minutes(5);

        let mut pairing_code = self.pairing_code.write().await;
        *pairing_code = Some(PairingCode { code: code.clone(), expires_at, attempts: 0 });

        // #3106 M2: never log the code value itself — zero characters, zero shape.
        eprintln!("[AUTH] Generated pairing code (expires at {})", expires_at);
        code
    }

    /// Verify pairing code and generate token.
    /// #3106 H1 part 3: wrong guesses against the active code are throttled —
    /// after `MAX_PAIRING_ATTEMPTS`, the code locks out for its own remaining
    /// validity (no separate lockout timer). The lockout is per-active-code,
    /// not per-IP: there is exactly one pairing ceremony possible at a time.
    pub async fn exchange_code_for_token(&self, code: &str) -> Result<TokenInfo, GatewayError> {
        // #3106 M2: never log the submitted code value.
        eprintln!("[AUTH] Attempting to exchange pairing code");

        let mut pairing_code = self.pairing_code.write().await;

        let pc = match pairing_code.as_mut() {
            Some(pc) => pc,
            None => {
                eprintln!("[AUTH] No pairing code available");
                return Err(GatewayError::Unauthorized("No active pairing code".to_string()));
            }
        };

        if Utc::now() > pc.expires_at {
            eprintln!("[AUTH] Pairing code expired");
            *pairing_code = None;
            return Err(GatewayError::Unauthorized("Pairing code expired".to_string()));
        }

        if pc.attempts >= MAX_PAIRING_ATTEMPTS {
            let retry_after = (pc.expires_at - Utc::now()).num_seconds().max(0) as u64;
            eprintln!("[AUTH] Pairing code locked out after {} wrong attempt(s)", pc.attempts);
            return Err(GatewayError::PairingLockedOut { retry_after });
        }

        if pc.code != code {
            pc.attempts += 1;
            eprintln!("[AUTH] Invalid pairing code provided");
            return Err(GatewayError::Unauthorized("Invalid pairing code".to_string()));
        }

        // Correct code, under threshold, not expired — issue a token.
        // Generate opaque token (256 bits = 32 bytes)
        // Do this before any awaits to avoid Send issues
        let token = {
            let mut rng = rand::thread_rng();
            let token_bytes: [u8; 32] = rng.gen();
            hex::encode(token_bytes)
        };

        let expires_at = Utc::now() + Duration::days(90);
        let tenant_id = "default".to_string();

        let token_info = TokenInfo {
            token: token.clone(),
            tenant_id: tenant_id.clone(),
            expires_at,
        };

        // Store token
        let mut tokens = self.tokens.write().await;
        tokens.insert(token.clone(), token_info.clone());
        drop(tokens);

        // Save tokens to disk
        let tokens_read = self.tokens.read().await;
        if let Err(e) = self.save_tokens(&tokens_read).await {
            eprintln!("[AUTH] Warning: failed to save tokens to disk: {}", e);
        }
        drop(tokens_read);

        // Invalidate pairing code
        *pairing_code = None;

        eprintln!("[AUTH] Token generated successfully for tenant: {}", tenant_id);
        Ok(token_info)
    }

    /// Verify bearer token
    pub async fn verify_token(&self, token: &str) -> Result<TokenInfo, GatewayError> {
        let tokens = self.tokens.read().await;
        
        match tokens.get(token) {
            Some(token_info) => {
                if Utc::now() > token_info.expires_at {
                    eprintln!("[AUTH] Token expired");
                    return Err(GatewayError::Unauthorized("Token expired".to_string()));
                }
                Ok(token_info.clone())
            }
            None => {
                eprintln!("[AUTH] Invalid token");
                Err(GatewayError::Unauthorized("Invalid token".to_string()))
            }
        }
    }
}

/// Tenant context extracted from auth
#[derive(Clone)]
pub struct TenantContext {
    pub tenant_id: String,
}

/// Auth middleware for /v1/* endpoints
pub async fn auth_middleware(
    State(auth_state): State<Arc<AuthState>>,
    mut req: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, Response> {
    // Extract Authorization header
    let auth_header = req
        .headers()
        .get("authorization")
        .and_then(|h| h.to_str().ok());

    let token = match auth_header {
        Some(header) if header.starts_with("Bearer ") => &header[7..],
        _ => {
            eprintln!("[AUTH] Missing or invalid Authorization header");
            return Err(unauthorized_response());
        }
    };

    // Verify token
    match auth_state.verify_token(token).await {
        Ok(token_info) => {
            // Add tenant context to request extensions
            req.extensions_mut().insert(TenantContext {
                tenant_id: token_info.tenant_id,
            });
            Ok(next.run(req).await)
        }
        Err(_) => Err(unauthorized_response()),
    }
}

fn unauthorized_response() -> Response {
    let body = Json(crate::error::ErrorResponse {
        error: "unauthorized".to_string(),
        status: 401,
        message: "Missing or invalid authorization token".to_string(),
        retry_after: None,
        max_bytes: None,
    });
    (StatusCode::UNAUTHORIZED, body).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_generate_pairing_code() {
        let tokens_path = std::env::temp_dir().join("test-tokens-gen.json");
        let _ = std::fs::remove_file(&tokens_path);
        let auth_state = AuthState::new(tokens_path);
        let code = auth_state.generate_pairing_code().await;

        // #3106 H1 part 3: widened from 6 to 8 digits.
        assert_eq!(code.len(), 8);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
    }

    #[tokio::test]
    async fn test_exchange_valid_code() {
        let tokens_path = std::env::temp_dir().join("test-tokens-exchange.json");
        let _ = std::fs::remove_file(&tokens_path);
        let auth_state = AuthState::new(tokens_path);
        let code = auth_state.generate_pairing_code().await;
        
        let result = auth_state.exchange_code_for_token(&code).await;
        assert!(result.is_ok());
        
        let token_info = result.unwrap();
        assert_eq!(token_info.tenant_id, "default");
        assert!(token_info.expires_at > Utc::now());
    }

    #[tokio::test]
    async fn test_exchange_invalid_code() {
        let tokens_path = std::env::temp_dir().join("test-tokens-invalid.json");
        let _ = std::fs::remove_file(&tokens_path);
        let auth_state = AuthState::new(tokens_path);
        let result = auth_state.exchange_code_for_token("999999").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_exchange_expired_code() {
        let tokens_path = std::env::temp_dir().join("test-tokens-expired.json");
        let _ = std::fs::remove_file(&tokens_path);
        let auth_state = AuthState::new(tokens_path);
        
        // Manually insert an expired code
        let expired_code = PairingCode {
            code: "123456".to_string(),
            expires_at: Utc::now() - Duration::minutes(1),
            attempts: 0,
        };
        *auth_state.pairing_code.write().await = Some(expired_code);
        
        let result = auth_state.exchange_code_for_token("123456").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_verify_valid_token() {
        let tokens_path = std::env::temp_dir().join("test-tokens-verify.json");
        let _ = std::fs::remove_file(&tokens_path);
        let auth_state = AuthState::new(tokens_path);
        let code = auth_state.generate_pairing_code().await;
        let token_info = auth_state.exchange_code_for_token(&code).await.unwrap();
        
        let result = auth_state.verify_token(&token_info.token).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_verify_invalid_token() {
        let tokens_path = std::env::temp_dir().join("test-tokens-verify-inv.json");
        let _ = std::fs::remove_file(&tokens_path);
        let auth_state = AuthState::new(tokens_path);
        let result = auth_state.verify_token("invalid_token").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_code_invalidated_after_exchange() {
        let tokens_path = std::env::temp_dir().join("test-tokens-invalidated.json");
        let _ = std::fs::remove_file(&tokens_path);
        let auth_state = AuthState::new(tokens_path);
        let code = auth_state.generate_pairing_code().await;
        
        // First exchange should succeed
        let result1 = auth_state.exchange_code_for_token(&code).await;
        assert!(result1.is_ok());
        
        // Second exchange with same code should fail
        let result2 = auth_state.exchange_code_for_token(&code).await;
        assert!(result2.is_err());
    }
    
    #[tokio::test]
    async fn test_token_persists_across_instances() {
        let dir = std::env::temp_dir().join("pane-test-persist");
        let _ = std::fs::create_dir_all(&dir);
        let tokens_path = dir.join("tokens.json");
        let _ = std::fs::remove_file(&tokens_path); // clean slate
        
        // Create first instance, pair, get token
        let auth1 = AuthState::new(tokens_path.clone());
        let code = auth1.generate_pairing_code().await;
        let token_info = auth1.exchange_code_for_token(&code).await.unwrap();
        
        // Create second instance (simulates restart), verify token still works
        let auth2 = AuthState::new(tokens_path.clone());
        let result = auth2.verify_token(&token_info.token).await;
        assert!(result.is_ok());
        
        // Cleanup
        let _ = std::fs::remove_file(&tokens_path);
        let _ = std::fs::remove_dir(&dir);
    }
    
    #[tokio::test]
    async fn test_typo_tolerance_wrong_code_preserves_active_slot() {
        // spec-covered-by: §22C.22.4 (typo-tolerance contract already spec'd; this PR adds test coverage only)
        // Pins the Some(_) arm in exchange_code_for_token: a wrong code returns 401 WITHOUT
        // clearing the active pairing slot. The correct code must still work afterward.
        let tokens_path = std::env::temp_dir().join("test-tokens-typo.json");
        let _ = std::fs::remove_file(&tokens_path);
        let auth_state = AuthState::new(tokens_path.clone());

        // Generate a real code so the Some(_) arm is reachable
        let code = auth_state.generate_pairing_code().await;

        // Submit a wrong code — must fail (401) without clearing the active slot
        let wrong = auth_state.exchange_code_for_token("000000").await;
        assert!(wrong.is_err(), "wrong code should return an error");

        // Correct code must still succeed — proves active slot survived the typo
        let result = auth_state.exchange_code_for_token(&code).await;
        assert!(result.is_ok(), "active code was incorrectly cleared on wrong submission");

        // Cleanup
        let _ = std::fs::remove_file(&tokens_path);
    }

    /// Red case Matt named: "an 11th wrong pairing attempt accepted today and
    /// refused after." Submit MAX_PAIRING_ATTEMPTS wrong guesses against one
    /// active code, then submit the CORRECT code — it must now be refused
    /// with the lockout variant, not accepted.
    #[tokio::test]
    async fn test_exchange_code_for_token_locks_out_after_threshold_wrong_attempts() {
        let tokens_path = std::env::temp_dir().join("test-tokens-lockout.json");
        let _ = std::fs::remove_file(&tokens_path);
        let auth_state = AuthState::new(tokens_path.clone());
        let code = auth_state.generate_pairing_code().await;
        let wrong = if code == "00000000" { "11111111" } else { "00000000" };

        for attempt in 0..MAX_PAIRING_ATTEMPTS {
            let result = auth_state.exchange_code_for_token(wrong).await;
            assert!(
                result.is_err(),
                "wrong guess #{} (under threshold) must fail",
                attempt + 1
            );
        }

        let result = auth_state.exchange_code_for_token(&code).await;
        match result {
            Err(GatewayError::PairingLockedOut { .. }) => {}
            other => panic!(
                "expected PairingLockedOut on the correct code after {} prior wrong attempts, got {:?}",
                MAX_PAIRING_ATTEMPTS, other
            ),
        }

        let _ = std::fs::remove_file(&tokens_path);
    }

    /// Fix 3 (#3200 FAIL remediation): Reed's mutation testing proved that
    /// `test_exchange_code_for_token_locks_out_after_threshold_wrong_attempts`
    /// above reads `MAX_PAIRING_ATTEMPTS` symbolically (`for attempt in
    /// 0..MAX_PAIRING_ATTEMPTS`) — raising the constant does NOT redden it,
    /// because the loop bound moves with the constant. That test proves the
    /// *mechanism* (something eventually locks out) but not the *magnitude*
    /// Matt ruled ("~10"). This companion test pins a LITERAL attempt count
    /// (10), independent of `MAX_PAIRING_ATTEMPTS`: if the constant is ever
    /// raised above this literal, exactly 10 wrong guesses will no longer be
    /// enough to lock out, the correct-code attempt below will succeed
    /// instead of being refused, and this test reddens.
    #[tokio::test]
    async fn test_exchange_code_for_token_locks_out_after_exactly_ten_wrong_attempts() {
        let tokens_path = std::env::temp_dir().join("test-tokens-lockout-fixed-count.json");
        let _ = std::fs::remove_file(&tokens_path);
        let auth_state = AuthState::new(tokens_path.clone());
        let code = auth_state.generate_pairing_code().await;
        let wrong = if code == "00000000" { "11111111" } else { "00000000" };

        // Literal 10, NOT MAX_PAIRING_ATTEMPTS — the whole point of this test
        // is to be blind to the constant so a future change to it is caught.
        for attempt in 0..10u32 {
            let result = auth_state.exchange_code_for_token(wrong).await;
            assert!(
                result.is_err(),
                "wrong guess #{} (under the fixed threshold of 10) must fail",
                attempt + 1
            );
        }

        let result = auth_state.exchange_code_for_token(&code).await;
        assert!(
            matches!(result, Err(GatewayError::PairingLockedOut { .. })),
            "expected PairingLockedOut on the correct code after exactly 10 wrong attempts, got {:?}",
            result
        );

        let _ = std::fs::remove_file(&tokens_path);
    }

    /// Non-regression companion to the lockout test: the typo-tolerance
    /// contract (§22C.22.4) must still hold for attempt counts well under
    /// the threshold — a single wrong guess must not lock anything out.
    #[tokio::test]
    async fn test_typo_tolerance_survives_under_threshold() {
        let tokens_path = std::env::temp_dir().join("test-tokens-typo-threshold.json");
        let _ = std::fs::remove_file(&tokens_path);
        let auth_state = AuthState::new(tokens_path.clone());
        let code = auth_state.generate_pairing_code().await;
        let wrong = if code == "00000000" { "11111111" } else { "00000000" };

        // A couple of wrong guesses, well under MAX_PAIRING_ATTEMPTS.
        assert!(auth_state.exchange_code_for_token(wrong).await.is_err());
        assert!(auth_state.exchange_code_for_token(wrong).await.is_err());

        let result = auth_state.exchange_code_for_token(&code).await;
        assert!(result.is_ok(), "correct code must still succeed under the threshold");

        let _ = std::fs::remove_file(&tokens_path);
    }

    /// DoS-bound proof: the attacker cannot prevent the owner from pairing.
    /// The owner's own fresh-code path (local CLI / IPC, reached only via the
    /// gateway's own Unix socket, never network-reachable) always resets the
    /// attempt counter — a locked-out code never poisons the next one.
    #[tokio::test]
    async fn test_fresh_pairing_code_is_not_locked_out_by_prior_codes_attempts() {
        let tokens_path = std::env::temp_dir().join("test-tokens-lockout-reset.json");
        let _ = std::fs::remove_file(&tokens_path);
        let auth_state = AuthState::new(tokens_path.clone());
        let old_code = auth_state.generate_pairing_code().await;
        let wrong = if old_code == "00000000" { "11111111" } else { "00000000" };

        for _ in 0..MAX_PAIRING_ATTEMPTS {
            let _ = auth_state.exchange_code_for_token(wrong).await;
        }
        assert!(matches!(
            auth_state.exchange_code_for_token(&old_code).await,
            Err(GatewayError::PairingLockedOut { .. })
        ));

        // The owner generates a fresh code — must be immediately usable with
        // a clean attempt budget, unaffected by the previous code's lockout.
        let fresh_code = auth_state.generate_pairing_code().await;
        let result = auth_state.exchange_code_for_token(&fresh_code).await;
        assert!(
            result.is_ok(),
            "a freshly generated code must not inherit the previous code's lockout"
        );

        let _ = std::fs::remove_file(&tokens_path);
    }

    #[tokio::test]
    async fn test_expired_tokens_pruned_on_load() {
        let dir = std::env::temp_dir().join("pane-test-prune");
        let _ = std::fs::create_dir_all(&dir);
        let tokens_path = dir.join("tokens.json");
        
        // Write an expired token directly to file
        let expired = serde_json::json!([
            {
                "token": "expired_token_123",
                "tenant_id": "default",
                "expires_at": "2020-01-01T00:00:00Z"
            }
        ]);
        std::fs::write(&tokens_path, expired.to_string()).unwrap();
        
        // Load — should prune expired
        let auth = AuthState::new(tokens_path.clone());
        let result = auth.verify_token("expired_token_123").await;
        assert!(result.is_err());
        
        // Cleanup
        let _ = std::fs::remove_file(&tokens_path);
        let _ = std::fs::remove_dir(&dir);
    }
}

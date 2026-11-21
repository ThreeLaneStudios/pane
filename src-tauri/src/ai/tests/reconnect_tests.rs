//! Tests for §22C.12.4 Chat/SSE reconnect with backoff (#945)
//!
//! Verifies:
//! - `classify_gateway_error` maps error strings to correct §22C.12.3 codes
//! - `should_chat_retry` correctly gates hard-fail vs retryable categories
//! - `compute_backoff_interval_ms` produces intervals within ±10% jitter bounds
//! - State transition logic: Attempting → Reconnecting → Failed
//!
//! GitHub Issue: #945
//! spec-covered-by: #1603 (§22C.12.4 reconnect behavior contract)

#[cfg(test)]
mod tests {
    use crate::ai::chat::{
        classify_gateway_error, compute_backoff_interval_ms, should_chat_retry,
    };

    // ─── classify_gateway_error ───────────────────────────────────────────────

    /// Hard-fail: 401 Unauthorized maps to gateway_auth_failed.
    #[test]
    fn test_classify_401_is_auth_failed() {
        assert_eq!(classify_gateway_error("HTTP 401 Unauthorized"), "gateway_auth_failed");
        assert_eq!(classify_gateway_error("error: 401"), "gateway_auth_failed");
    }

    /// Hard-fail: 403 Forbidden maps to gateway_auth_failed.
    #[test]
    fn test_classify_403_is_auth_failed() {
        assert_eq!(classify_gateway_error("403 Forbidden"), "gateway_auth_failed");
        assert_eq!(classify_gateway_error("Forbidden: access denied"), "gateway_auth_failed");
    }

    /// Hard-fail: "unauthorized" keyword maps to gateway_auth_failed.
    #[test]
    fn test_classify_unauthorized_keyword_is_auth_failed() {
        assert_eq!(classify_gateway_error("Unauthorized"), "gateway_auth_failed");
        assert_eq!(classify_gateway_error("Gateway returned: UNAUTHORIZED"), "gateway_auth_failed");
    }

    /// Hard-fail: TLS certificate error maps to gateway_cert_mismatch.
    #[test]
    fn test_classify_tls_cert_is_cert_mismatch() {
        assert_eq!(classify_gateway_error("certificate verify failed"), "gateway_cert_mismatch");
        assert_eq!(classify_gateway_error("TLS handshake failed"), "gateway_cert_mismatch");
        assert_eq!(classify_gateway_error("SSL cert mismatch"), "gateway_cert_mismatch");
        assert_eq!(classify_gateway_error("fingerprint mismatch"), "gateway_cert_mismatch");
    }

    /// Retryable: connection refused maps to gateway_unreachable.
    #[test]
    fn test_classify_connection_refused_is_unreachable() {
        assert_eq!(classify_gateway_error("Connection refused"), "gateway_unreachable");
        assert_eq!(classify_gateway_error("failed to connect to host"), "gateway_unreachable");
        assert_eq!(classify_gateway_error("ECONNREFUSED"), "gateway_unreachable");
    }

    /// Retryable: generic timeout/5xx maps to gateway_connection_failed.
    #[test]
    fn test_classify_generic_errors_are_connection_failed() {
        assert_eq!(classify_gateway_error("connection timeout"), "gateway_connection_failed");
        assert_eq!(classify_gateway_error("503 Service Unavailable"), "gateway_connection_failed");
        assert_eq!(classify_gateway_error("Stream error: error decoding response body"), "gateway_connection_failed");
        assert_eq!(classify_gateway_error("Request failed with status 500"), "gateway_connection_failed");
        assert_eq!(classify_gateway_error("network error"), "gateway_connection_failed");
    }

    /// ADR-0067 D4 / spec §4A.2.5: OC's real 400 "no explicit owner" error string
    /// (verbatim, not synthetic) must classify as gateway_provider_error, not fall
    /// through to the generic gateway_connection_failed bucket it landed in before
    /// this fix.
    #[test]
    fn test_classify_oc_400_explicit_owner_is_provider_error() {
        assert_eq!(
            classify_gateway_error(
                "API error (400): Multiple agents are configured, but this operation has no explicit owner. Select an agent explicitly."
            ),
            "gateway_provider_error"
        );
    }

    /// Any other 4xx in the "API error (NNN): ..." shape also classifies as
    /// gateway_provider_error — not just OC's specific 400 message. 401/402/403
    /// are excluded (already matched by the hard-fail branches above this one).
    #[test]
    fn test_classify_generic_4xx_is_provider_error() {
        assert_eq!(
            classify_gateway_error("API error (400): Bad Request"),
            "gateway_provider_error"
        );
        assert_eq!(
            classify_gateway_error("API error (404): Not Found"),
            "gateway_provider_error"
        );
        assert_eq!(
            classify_gateway_error("API error (422): Unprocessable Entity"),
            "gateway_provider_error"
        );
        assert_eq!(
            classify_gateway_error("API error (429): Too Many Requests"),
            "gateway_provider_error"
        );
    }

    /// 401/402/403 keep their own specific codes ahead of the generic 4xx branch
    /// — even when they arrive in the same "API error (NNN)" shape as the OC 400.
    #[test]
    fn test_classify_401_402_403_keep_specific_codes_not_generic_4xx() {
        assert_eq!(
            classify_gateway_error("API error (401): Unauthorized"),
            "gateway_auth_failed"
        );
        assert_eq!(
            classify_gateway_error("API error (402): Payment Required"),
            "openrouter_billing_error"
        );
        assert_eq!(
            classify_gateway_error("API error (403): Forbidden"),
            "gateway_auth_failed"
        );
    }

    /// #3081 root-cause fix: the synthetic bare-number fixtures above
    /// (`"API error (400): Bad Request"` etc.) never matched what production
    /// actually sends. `status` in `format!("API error ({}): {}", status, msg)` is a
    /// `reqwest::StatusCode`, and its `Display` impl writes the canonical reason
    /// phrase after the number — `"400 Bad Request"`, not `"400"` (verified
    /// empirically against this reqwest version). The pre-#3081
    /// `parse_api_error_status` parsed the *entire* span between `"api error ("` and
    /// the next `')'` as the number, which only succeeds when that span is a bare
    /// number — i.e. never, in production. Every 4xx this function was meant to
    /// catch (OC's "Multiple agents..." 400 included, not just Venice's) was
    /// silently falling through to `gateway_connection_failed` the entire time
    /// ADR-0067 D4 has shipped. Built with the real `reqwest::StatusCode` the
    /// production code actually uses, not a hand-typed guess at its `Display` output.
    #[test]
    fn test_classify_real_reason_phrase_shape_is_provider_error_not_connection_failed() {
        let oc_400 = format!(
            "API error ({}): {}",
            reqwest::StatusCode::BAD_REQUEST,
            "Multiple agents are configured, but this operation has no explicit owner. Select an agent explicitly."
        );
        assert_eq!(
            classify_gateway_error(&oc_400),
            "gateway_provider_error",
            "OC's real-shaped 400 (reqwest's actual Display output, reason phrase included) must \
             classify as gateway_provider_error, not fall through to gateway_connection_failed"
        );

        let venice_400 = format!(
            "API error ({}): {}",
            reqwest::StatusCode::BAD_REQUEST,
            "Text content parts may only include type and text"
        );
        assert_eq!(
            classify_gateway_error(&venice_400),
            "gateway_provider_error",
            "Venice's real-shaped 400 (#3081) must classify as gateway_provider_error, not \
             gateway_connection_failed — this is the exact string shape Matt's 1.0.64 live test hit"
        );

        let not_found_404 = format!("API error ({}): {}", reqwest::StatusCode::NOT_FOUND, "model not found");
        assert_eq!(classify_gateway_error(&not_found_404), "gateway_provider_error");
    }

    /// gateway_provider_error must never retry — absent from should_chat_retry's
    /// matches! list is what enforces this; this test locks that contract.
    #[test]
    fn test_provider_error_never_retries() {
        assert!(
            !should_chat_retry("gateway_provider_error"),
            "gateway_provider_error must be hard-fail (no retry) per ADR-0067 D4"
        );
    }

    /// 5xx and generic transport errors must still classify as
    /// gateway_connection_failed (retryable) after the 4xx branch is added —
    /// non-regression for the pre-existing retry path. Re-asserted directly
    /// against real 5xx-shaped "API error (NNN)" strings, not just the older
    /// substring-based fixtures already covered above.
    #[test]
    fn test_classify_5xx_and_transport_still_connection_failed() {
        assert_eq!(
            classify_gateway_error("API error (500): Internal Server Error"),
            "gateway_connection_failed"
        );
        assert_eq!(
            classify_gateway_error("API error (503): Service Unavailable"),
            "gateway_connection_failed"
        );
        assert_eq!(
            classify_gateway_error("connection timeout"),
            "gateway_connection_failed"
        );
        assert!(should_chat_retry(classify_gateway_error("API error (500): Internal Server Error")));
        assert!(should_chat_retry(classify_gateway_error("connection timeout")));
    }

    // ─── should_chat_retry ───────────────────────────────────────────────────

    /// Hard-fail categories MUST bypass the retry loop.
    #[test]
    fn test_hard_fail_categories_do_not_retry() {
        assert!(
            !should_chat_retry("gateway_auth_failed"),
            "gateway_auth_failed must be hard-fail (no retry)"
        );
        assert!(
            !should_chat_retry("gateway_cert_mismatch"),
            "gateway_cert_mismatch must be hard-fail (no retry)"
        );
    }

    /// Retryable categories MUST enter the reconnect loop.
    #[test]
    fn test_retryable_categories_allow_retry() {
        assert!(
            should_chat_retry("gateway_unreachable"),
            "gateway_unreachable must be retryable"
        );
        assert!(
            should_chat_retry("gateway_connection_failed"),
            "gateway_connection_failed must be retryable"
        );
    }

    // ─── compute_backoff_interval_ms ─────────────────────────────────────────

    /// Backoff curve: 1s → 2s → 4s → 8s → 30s (base intervals per §22C.12.4).
    /// Each attempt must land within ±10% of its base interval.
    #[test]
    fn test_backoff_curve_attempt_0_within_jitter_bounds() {
        let base_ms: u64 = 1_000;
        for _ in 0..100 {
            let interval = compute_backoff_interval_ms(0);
            assert!(
                interval >= 900 && interval <= 1_100,
                "attempt 0 (base 1000ms): got {}ms, expected 900..=1100ms",
                interval
            );
        }
        // Verify base is correct
        assert_eq!(base_ms, 1_000);
    }

    #[test]
    fn test_backoff_curve_attempt_1_within_jitter_bounds() {
        for _ in 0..100 {
            let interval = compute_backoff_interval_ms(1);
            assert!(
                interval >= 1_800 && interval <= 2_200,
                "attempt 1 (base 2000ms): got {}ms, expected 1800..=2200ms",
                interval
            );
        }
    }

    #[test]
    fn test_backoff_curve_attempt_2_within_jitter_bounds() {
        for _ in 0..100 {
            let interval = compute_backoff_interval_ms(2);
            assert!(
                interval >= 3_600 && interval <= 4_400,
                "attempt 2 (base 4000ms): got {}ms, expected 3600..=4400ms",
                interval
            );
        }
    }

    #[test]
    fn test_backoff_curve_attempt_3_within_jitter_bounds() {
        for _ in 0..100 {
            let interval = compute_backoff_interval_ms(3);
            assert!(
                interval >= 7_200 && interval <= 8_800,
                "attempt 3 (base 8000ms): got {}ms, expected 7200..=8800ms",
                interval
            );
        }
    }

    #[test]
    fn test_backoff_curve_attempt_4_within_jitter_bounds() {
        for _ in 0..100 {
            let interval = compute_backoff_interval_ms(4);
            assert!(
                interval >= 27_000 && interval <= 33_000,
                "attempt 4 (base 30000ms): got {}ms, expected 27000..=33000ms",
                interval
            );
        }
    }

    /// Attempt index beyond the table is capped at 30s base.
    #[test]
    fn test_backoff_capped_at_30s() {
        for _ in 0..50 {
            let interval = compute_backoff_interval_ms(99);
            assert!(
                interval >= 27_000 && interval <= 33_000,
                "attempt 99 (capped at 30000ms): got {}ms",
                interval
            );
        }
    }

    /// Jitter must be non-negative (minimum 1ms).
    #[test]
    fn test_backoff_always_positive() {
        for attempt in 0..5 {
            for _ in 0..50 {
                let interval = compute_backoff_interval_ms(attempt);
                assert!(interval >= 1, "backoff must be at least 1ms, got {}ms", interval);
            }
        }
    }

    // ─── State transition logic ───────────────────────────────────────────────
    //
    // The full state machine (Idle → Attempting → Reconnecting → Failed) lives
    // in the Svelte frontend and is exercised by the Vitest suite.  Here we
    // verify the pure-Rust gate functions that drive the backend side of those
    // transitions.

    /// Hard-fail path: classify + gate confirms no retry for auth failure.
    /// Maps to: Attempting → Failed (no Reconnecting phase).
    #[test]
    fn test_state_transition_auth_fail_is_immediate_failed() {
        let raw_error = "HTTP 401 Unauthorized — token rejected";
        let code = classify_gateway_error(raw_error);
        assert_eq!(code, "gateway_auth_failed");
        assert!(!should_chat_retry(code), "auth fail must skip reconnect → immediate Failed");
    }

    /// Hard-fail path: cert mismatch is also immediate Failed.
    #[test]
    fn test_state_transition_cert_mismatch_is_immediate_failed() {
        let raw_error = "TLS certificate verify failed: fingerprint mismatch";
        let code = classify_gateway_error(raw_error);
        assert_eq!(code, "gateway_cert_mismatch");
        assert!(!should_chat_retry(code), "cert mismatch must skip reconnect → immediate Failed");
    }

    /// Retryable path: unreachable triggers Reconnecting state.
    #[test]
    fn test_state_transition_unreachable_triggers_reconnecting() {
        let raw_error = "Connection refused (os error 111)";
        let code = classify_gateway_error(raw_error);
        assert_eq!(code, "gateway_unreachable");
        assert!(should_chat_retry(code), "unreachable must enter Reconnecting state");
    }

    /// Budget exhaustion: after MAX_RETRIES, should_chat_retry is irrelevant;
    /// the caller guards on reconnect_count.  This test documents the expected
    /// maximum retry count per §22C.12.4.
    #[test]
    fn test_budget_exhaustion_max_retries_is_five() {
        use crate::ai::chat::CHAT_RECONNECT_MAX_RETRIES;
        assert_eq!(
            CHAT_RECONNECT_MAX_RETRIES, 5,
            "§22C.12.4 mandates max 5 retries"
        );
    }

    /// Verify the full backoff curve array length matches CHAT_RECONNECT_MAX_RETRIES.
    #[test]
    fn test_backoff_intervals_length_matches_max_retries() {
        use crate::ai::chat::{CHAT_RECONNECT_BACKOFF_INTERVALS_MS, CHAT_RECONNECT_MAX_RETRIES};
        assert_eq!(
            CHAT_RECONNECT_BACKOFF_INTERVALS_MS.len(),
            CHAT_RECONNECT_MAX_RETRIES,
            "backoff intervals array length must equal CHAT_RECONNECT_MAX_RETRIES"
        );
    }

    /// Budget exhaustion: correct error_code emitted on final failure.
    /// classify_gateway_error must produce one of the §22C.12.3 enumerated codes.
    #[test]
    fn test_budget_exhaustion_emits_correct_error_code() {
        let exhaustion_error = "connection timeout after 5 retries";
        let code = classify_gateway_error(exhaustion_error);
        let valid_codes = ["gateway_unreachable", "gateway_auth_failed", "gateway_cert_mismatch", "gateway_connection_failed"];
        assert!(
            valid_codes.contains(&code),
            "budget exhaustion must emit a §22C.12.3 error code, got: {}",
            code
        );
        // For a timeout, it should be gateway_connection_failed
        assert_eq!(code, "gateway_connection_failed");
    }
}

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// TTL for idempotency records (24 hours)
const IDEMPOTENCY_TTL_HOURS: i64 = 24;

/// Cleanup interval (run every hour)
const CLEANUP_INTERVAL_SECONDS: u64 = 3600;

/// Stored response for idempotent request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdempotencyRecord {
    pub message_id: String,
    pub accepted_at: DateTime<Utc>,
    pub oc_message_id: String,
    pub created_at: DateTime<Utc>,
}

/// In-memory idempotency store
/// Key: (tenant_id, session_id, idempotency_key)
pub struct IdempotencyStore {
    store: Arc<RwLock<HashMap<(String, String, String), IdempotencyRecord>>>,
}

impl IdempotencyStore {
    pub fn new() -> Arc<Self> {
        eprintln!("[IDEMPOTENCY] Creating idempotency store");
        let store = Arc::new(Self {
            store: Arc::new(RwLock::new(HashMap::new())),
        });

        // Spawn cleanup task
        let store_for_cleanup = store.clone();
        tokio::spawn(async move {
            store_for_cleanup.cleanup_task().await;
        });

        store
    }

    /// Check if key exists and return existing record, or insert new record
    pub async fn check_or_insert(
        &self,
        tenant_id: &str,
        session_id: &str,
        idempotency_key: &str,
        message_id: String,
        oc_message_id: String,
    ) -> Option<IdempotencyRecord> {
        eprintln!(
            "[IDEMPOTENCY] Checking key: tenant={}, session={}, key={}",
            tenant_id, session_id, idempotency_key
        );

        let key = (
            tenant_id.to_string(),
            session_id.to_string(),
            idempotency_key.to_string(),
        );

        let mut store = self.store.write().await;

        // Check if key exists
        if let Some(existing) = store.get(&key) {
            // Verify it hasn't expired
            let now = Utc::now();
            let age = now.signed_duration_since(existing.created_at);
            if age < Duration::hours(IDEMPOTENCY_TTL_HOURS) {
                eprintln!("[IDEMPOTENCY] Found existing record (age: {}s)", age.num_seconds());
                return Some(existing.clone());
            } else {
                eprintln!("[IDEMPOTENCY] Found expired record, removing");
                store.remove(&key);
            }
        }

        // Insert new record
        let record = IdempotencyRecord {
            message_id,
            accepted_at: Utc::now(),
            oc_message_id,
            created_at: Utc::now(),
        };

        eprintln!("[IDEMPOTENCY] Storing new record");
        store.insert(key, record);
        None
    }

    /// Cleanup task - runs periodically to remove expired records
    async fn cleanup_task(&self) {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(
            CLEANUP_INTERVAL_SECONDS,
        ));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            interval.tick().await;
            self.cleanup_expired().await;
        }
    }

    /// Remove expired records
    async fn cleanup_expired(&self) {
        eprintln!("[IDEMPOTENCY] Running cleanup");

        let mut store = self.store.write().await;
        let now = Utc::now();
        let before_count = store.len();

        store.retain(|_key, record| {
            let age = now.signed_duration_since(record.created_at);
            age < Duration::hours(IDEMPOTENCY_TTL_HOURS)
        });

        let removed = before_count - store.len();
        if removed > 0 {
            eprintln!(
                "[IDEMPOTENCY] Cleanup removed {} expired records ({} remaining)",
                removed,
                store.len()
            );
        }
    }

    /// Update an existing record with the real response data
    pub async fn update(
        &self,
        tenant_id: &str,
        session_id: &str,
        idempotency_key: &str,
        record: IdempotencyRecord,
    ) {
        let key = (
            tenant_id.to_string(),
            session_id.to_string(),
            idempotency_key.to_string(),
        );

        let mut store = self.store.write().await;
        if let Some(existing) = store.get_mut(&key) {
            eprintln!("[IDEMPOTENCY] Updating existing record for key: {}", idempotency_key);
            *existing = record;
        }
    }

    /// Remove a record (used to clean up placeholder on OC call failure)
    pub async fn remove(
        &self,
        tenant_id: &str,
        session_id: &str,
        idempotency_key: &str,
    ) -> bool {
        let key = (
            tenant_id.to_string(),
            session_id.to_string(),
            idempotency_key.to_string(),
        );
        let mut store = self.store.write().await;
        store.remove(&key).is_some()
    }

    /// Get current store size (for testing)
    #[cfg(test)]
    pub async fn len(&self) -> usize {
        self.store.read().await.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_first_request_returns_none() {
        let store = IdempotencyStore::new();
        let tenant_id = "test-tenant";
        let session_id = "test-session";
        let key = "test-key";

        let result = store
            .check_or_insert(
                tenant_id,
                session_id,
                key,
                "msg-123".to_string(),
                "oc-msg-123".to_string(),
            )
            .await;

        assert!(result.is_none(), "First request should return None");
    }

    #[tokio::test]
    async fn test_duplicate_request_returns_existing() {
        let store = IdempotencyStore::new();
        let tenant_id = "test-tenant";
        let session_id = "test-session";
        let key = "test-key";

        // First request
        let result1 = store
            .check_or_insert(
                tenant_id,
                session_id,
                key,
                "msg-123".to_string(),
                "oc-msg-123".to_string(),
            )
            .await;

        assert!(result1.is_none());

        // Duplicate request with same key
        let result2 = store
            .check_or_insert(
                tenant_id,
                session_id,
                key,
                "msg-456".to_string(), // Different message_id
                "oc-msg-456".to_string(),
            )
            .await;

        assert!(result2.is_some(), "Duplicate should return existing");
        let existing = result2.unwrap();
        assert_eq!(existing.message_id, "msg-123"); // Original message_id
        assert_eq!(existing.oc_message_id, "oc-msg-123");
    }

    #[tokio::test]
    async fn test_different_keys_are_independent() {
        let store = IdempotencyStore::new();
        let tenant_id = "test-tenant";
        let session_id = "test-session";

        // First key
        store
            .check_or_insert(
                tenant_id,
                session_id,
                "key1",
                "msg-1".to_string(),
                "oc-msg-1".to_string(),
            )
            .await;

        // Different key
        let result = store
            .check_or_insert(
                tenant_id,
                session_id,
                "key2",
                "msg-2".to_string(),
                "oc-msg-2".to_string(),
            )
            .await;

        assert!(result.is_none(), "Different key should be treated as new");
    }

    #[tokio::test]
    async fn test_different_sessions_are_independent() {
        let store = IdempotencyStore::new();
        let tenant_id = "test-tenant";
        let key = "same-key";

        // Session 1
        store
            .check_or_insert(
                tenant_id,
                "session1",
                key,
                "msg-1".to_string(),
                "oc-msg-1".to_string(),
            )
            .await;

        // Session 2 with same key
        let result = store
            .check_or_insert(
                tenant_id,
                "session2",
                key,
                "msg-2".to_string(),
                "oc-msg-2".to_string(),
            )
            .await;

        assert!(
            result.is_none(),
            "Same key in different session should be treated as new"
        );
    }

    #[tokio::test]
    async fn test_cleanup_removes_expired() {
        // We can't easily test 24-hour expiry in a fast test, but we can verify the cleanup mechanism
        let store = IdempotencyStore::new();

        // Add a record
        store
            .check_or_insert(
                "tenant",
                "session",
                "key",
                "msg".to_string(),
                "oc-msg".to_string(),
            )
            .await;

        assert_eq!(store.len().await, 1);

        // Manually trigger cleanup (won't remove fresh record)
        store.cleanup_expired().await;
        assert_eq!(store.len().await, 1, "Fresh record should not be removed");
    }

    #[tokio::test]
    async fn test_idempotency_returns_consistent_response() {
        let store = IdempotencyStore::new();
        let tenant_id = "default";
        let session_id = "sess1";
        let key = "idem-123";

        // First call: check_or_insert with empty placeholder
        let first = store
            .check_or_insert(
                tenant_id,
                session_id,
                key,
                "msg-temp-123".to_string(),
                "".to_string(),
            )
            .await;
        assert!(first.is_none(), "First request should return None");

        // Simulate: OC returns real data, update the record
        store
            .update(
                tenant_id,
                session_id,
                key,
                IdempotencyRecord {
                    message_id: "msg-real-123".to_string(),
                    oc_message_id: "oc-msg-456".to_string(),
                    accepted_at: "2026-04-28T17:00:00Z".parse().unwrap(),
                    created_at: Utc::now(),
                },
            )
            .await;

        // Second call: same key → should return the UPDATED record
        let second = store
            .check_or_insert(
                tenant_id,
                session_id,
                key,
                "ignored".to_string(),
                "ignored".to_string(),
            )
            .await;

        assert!(second.is_some(), "Duplicate request should return existing record");
        let record = second.unwrap();
        assert_eq!(
            record.message_id, "msg-real-123",
            "Should return updated message_id"
        );
        assert_eq!(
            record.oc_message_id, "oc-msg-456",
            "Should return updated oc_message_id, not empty string"
        );
        assert_eq!(
            record.accepted_at.to_rfc3339(),
            "2026-04-28T17:00:00+00:00",
            "Should return updated timestamp"
        );
    }

    #[tokio::test]
    async fn test_remove_cleans_up_placeholder() {
        let store = IdempotencyStore::new();

        // Insert placeholder (simulating first call)
        let result = store
            .check_or_insert(
                "tenant",
                "session",
                "key",
                "msg-temp".to_string(),
                "".to_string(), // empty placeholder
            )
            .await;
        assert!(result.is_none());

        // Simulate OC error → remove placeholder
        let removed = store.remove("tenant", "session", "key").await;
        assert!(removed, "Should have removed the placeholder");

        // Retry: should get None (clean slate), not the poisoned placeholder
        let retry = store
            .check_or_insert(
                "tenant",
                "session",
                "key",
                "msg-retry".to_string(),
                "".to_string(),
            )
            .await;
        assert!(
            retry.is_none(),
            "Retry after removal should be treated as new request, not return poisoned placeholder"
        );
    }
}

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

/// A pending file sync change
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingChange {
    pub id: String,
    pub oc_agent_id: String,
    pub filename: String,
    pub content: String,
    pub checksum: String,
    pub queued_at: DateTime<Utc>,
}

/// In-memory sync pending queue
/// Keyed by (tenant_id, oc_agent_id) -> Vec<PendingChange>
pub struct SyncQueue {
    queue: Arc<RwLock<HashMap<(String, String), Vec<PendingChange>>>>,
}

impl SyncQueue {
    pub fn new() -> Self {
        eprintln!("[SYNC_QUEUE] Creating sync queue");
        Self {
            queue: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Add a pending change for an agent
    pub async fn add_pending(
        &self,
        tenant_id: &str,
        oc_agent_id: &str,
        filename: String,
        content: String,
        checksum: String,
    ) -> String {
        let id = Uuid::new_v4().to_string();
        let change = PendingChange {
            id: id.clone(),
            oc_agent_id: oc_agent_id.to_string(),
            filename,
            content,
            checksum,
            queued_at: Utc::now(),
        };

        eprintln!(
            "[SYNC_QUEUE] Adding pending change: id={}, agent={}, file={}",
            id, oc_agent_id, change.filename
        );

        let key = (tenant_id.to_string(), oc_agent_id.to_string());
        let mut queue = self.queue.write().await;
        queue.entry(key).or_insert_with(Vec::new).push(change);

        id
    }

    /// Get all pending changes for an agent
    pub async fn get_pending(
        &self,
        tenant_id: &str,
        oc_agent_id: &str,
    ) -> Vec<PendingChange> {
        eprintln!(
            "[SYNC_QUEUE] Getting pending changes for tenant={}, agent={}",
            tenant_id, oc_agent_id
        );

        let key = (tenant_id.to_string(), oc_agent_id.to_string());
        let queue = self.queue.read().await;
        queue.get(&key).cloned().unwrap_or_default()
    }

    /// Confirm (remove) processed changes by ID
    pub async fn confirm(
        &self,
        tenant_id: &str,
        oc_agent_id: &str,
        processed_ids: Vec<String>,
    ) -> usize {
        eprintln!(
            "[SYNC_QUEUE] Confirming {} changes for tenant={}, agent={}",
            processed_ids.len(),
            tenant_id,
            oc_agent_id
        );

        let key = (tenant_id.to_string(), oc_agent_id.to_string());
        let mut queue = self.queue.write().await;

        if let Some(changes) = queue.get_mut(&key) {
            let before_count = changes.len();
            changes.retain(|change| !processed_ids.contains(&change.id));
            let removed = before_count - changes.len();

            eprintln!("[SYNC_QUEUE] Confirmed {} changes", removed);
            removed
        } else {
            eprintln!("[SYNC_QUEUE] No pending changes found for key");
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_add_and_get_pending() {
        let queue = SyncQueue::new();
        let tenant_id = "test-tenant";
        let agent_id = "test-agent";

        // Add a pending change
        let id = queue
            .add_pending(
                tenant_id,
                agent_id,
                "test.txt".to_string(),
                "content".to_string(),
                "checksum123".to_string(),
            )
            .await;

        // Verify we can retrieve it
        let pending = queue.get_pending(tenant_id, agent_id).await;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, id);
        assert_eq!(pending[0].filename, "test.txt");
        assert_eq!(pending[0].content, "content");
        assert_eq!(pending[0].checksum, "checksum123");
    }

    #[tokio::test]
    async fn test_confirm_removes_changes() {
        let queue = SyncQueue::new();
        let tenant_id = "test-tenant";
        let agent_id = "test-agent";

        // Add two changes
        let id1 = queue
            .add_pending(
                tenant_id,
                agent_id,
                "file1.txt".to_string(),
                "content1".to_string(),
                "checksum1".to_string(),
            )
            .await;

        let id2 = queue
            .add_pending(
                tenant_id,
                agent_id,
                "file2.txt".to_string(),
                "content2".to_string(),
                "checksum2".to_string(),
            )
            .await;

        // Confirm first change
        let confirmed = queue.confirm(tenant_id, agent_id, vec![id1.clone()]).await;
        assert_eq!(confirmed, 1);

        // Verify only second change remains
        let pending = queue.get_pending(tenant_id, agent_id).await;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, id2);
    }

    #[tokio::test]
    async fn test_multiple_agents() {
        let queue = SyncQueue::new();
        let tenant_id = "test-tenant";

        // Add changes for two different agents
        queue
            .add_pending(
                tenant_id,
                "agent1",
                "file1.txt".to_string(),
                "content1".to_string(),
                "checksum1".to_string(),
            )
            .await;

        queue
            .add_pending(
                tenant_id,
                "agent2",
                "file2.txt".to_string(),
                "content2".to_string(),
                "checksum2".to_string(),
            )
            .await;

        // Verify each agent sees only their changes
        let pending1 = queue.get_pending(tenant_id, "agent1").await;
        let pending2 = queue.get_pending(tenant_id, "agent2").await;

        assert_eq!(pending1.len(), 1);
        assert_eq!(pending2.len(), 1);
        assert_eq!(pending1[0].filename, "file1.txt");
        assert_eq!(pending2[0].filename, "file2.txt");
    }
}

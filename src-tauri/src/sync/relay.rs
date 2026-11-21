use reqwest;
use serde::{Deserialize, Serialize};

const SUPABASE_URL: &str = "https://jngkiljhmnrygdnfvrgp.supabase.co";
const SUPABASE_ANON_KEY: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJpc3MiOiJzdXBhYmFzZSIsInJlZiI6ImpuZ2tpbGpobW5yeWdkbmZ2cmdwIiwicm9sZSI6ImFub24iLCJpYXQiOjE3NzIyMzUwNzEsImV4cCI6MjA4NzgxMTA3MX0.xe6N-2KaY0Tx2eYI-UZJrlSa3v2EYZJT-PMVFRDPnzI";

/// Push encrypted changeset to the changeset_queue (for offline devices)
pub async fn push_to_queue(
    access_token: &str,
    user_id: &str,
    device_id: &str,
    encrypted_payload: &[u8],
    db_version: i64,
) -> Result<(), String> {
    use base64::{Engine as _, engine::general_purpose};
    let client = reqwest::Client::new();
    
    let payload_b64 = general_purpose::STANDARD.encode(encrypted_payload);
    
    let res = client
        .post(format!("{}/rest/v1/changeset_queue", SUPABASE_URL))
        .header("apikey", SUPABASE_ANON_KEY)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Content-Type", "application/json")
        .header("Prefer", "return=representation")
        .json(&serde_json::json!({
            "user_id": user_id,
            "from_device_id": device_id,
            "encrypted_payload": payload_b64,
            "db_version": db_version,
        }))
        .send()
        .await
        .map_err(|e| format!("Network error: {}", e))?;
    
    let status = res.status();
    let body = res.text().await.unwrap_or_default();
    
    println!("[sync] Push response: {} - {}", status, &body[..body.len().min(200)]);
    
    if !status.is_success() {
        return Err(format!("Push failed ({}): {}", status, body));
    }
    
    Ok(())
}

/// Pull pending changesets from the queue
/// 
/// FIXME (#131): This fetches ALL changesets on every pull (no "since" filter).
/// This caused 187 GB egress in ~1 week (5 GB limit). Should add:
/// `?db_version=gt.{last_pulled_version}` to only fetch NEW changesets.
/// Auto-sync disabled in v11.28.69 as emergency mitigation.
pub async fn pull_from_queue(
    access_token: &str,
    device_id: &str,
) -> Result<Vec<QueuedChangeset>, String> {
    let client = reqwest::Client::new();
    
    // Filter out own device's changesets at API level to reduce download size
    let res = client
        .get(format!(
            "{}/rest/v1/changeset_queue?from_device_id=neq.{}&order=db_version.asc",
            SUPABASE_URL, device_id
        ))
        .header("apikey", SUPABASE_ANON_KEY)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Content-Type", "application/json")
        .send()
        .await
        .map_err(|e| format!("Network error: {}", e))?;
    
    if !res.status().is_success() {
        let body = res.text().await.unwrap_or_default();
        return Err(format!("Failed to pull changesets: {}", body));
    }
    
    let items: Vec<QueuedChangeset> = res
        .json()
        .await
        .map_err(|e| format!("Failed to parse queue: {}", e))?;
    
    Ok(items)
}

/// Clean up own-device changesets from queue (one-time maintenance)
pub async fn cleanup_own_changesets(
    access_token: &str,
    device_id: &str,
) -> Result<u64, String> {
    let client = reqwest::Client::new();
    
    // Delete all changesets from our own device
    let res = client
        .delete(format!(
            "{}/rest/v1/changeset_queue?from_device_id=eq.{}",
            SUPABASE_URL, device_id
        ))
        .header("apikey", SUPABASE_ANON_KEY)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Prefer", "return=representation")
        .send()
        .await
        .map_err(|e| format!("Network error: {}", e))?;
    
    let status = res.status();
    if status.is_success() {
        // Try to count deleted items
        let body = res.text().await.unwrap_or_default();
        let count = serde_json::from_str::<Vec<serde_json::Value>>(&body)
            .map(|v| v.len() as u64)
            .unwrap_or(0);
        eprintln!("[sync] Cleaned up {} own-device changesets", count);
        Ok(count)
    } else {
        let body = res.text().await.unwrap_or_default();
        Err(format!("Cleanup failed: {}", body))
    }
}

/// Delete processed changeset from queue
pub async fn delete_from_queue(
    access_token: &str,
    changeset_id: &str,
) -> Result<(), String> {
    let client = reqwest::Client::new();
    
    client
        .delete(format!("{}/rest/v1/changeset_queue?id=eq.{}", SUPABASE_URL, changeset_id))
        .header("apikey", SUPABASE_ANON_KEY)
        .header("Authorization", format!("Bearer {}", access_token))
        .send()
        .await
        .map_err(|e| format!("Network error: {}", e))?;
    
    Ok(())
}

/// Update device last_seen timestamp
// Retained for the dormant legacy heartbeat path.
#[allow(dead_code)]
pub async fn update_device_heartbeat(
    access_token: &str,
    device_id: &str,
) -> Result<(), String> {
    let client = reqwest::Client::new();
    
    client
        .patch(format!("{}/rest/v1/devices?id=eq.{}", SUPABASE_URL, device_id))
        .header("apikey", SUPABASE_ANON_KEY)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "last_seen_at": chrono::Utc::now().to_rfc3339(),
        }))
        .send()
        .await
        .map_err(|e| format!("Network error: {}", e))?;
    
    Ok(())
}

#[derive(Serialize, Deserialize, Debug)]
pub struct QueuedChangeset {
    pub id: String,
    pub user_id: String,
    pub from_device_id: String,
    pub encrypted_payload: String,  // Base64-encoded bytea from Postgres
    pub db_version: i64,
    pub created_at: String,
}

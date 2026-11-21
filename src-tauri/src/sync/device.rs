use reqwest;
use serde_json::Value;

const SUPABASE_URL: &str = "https://jngkiljhmnrygdnfvrgp.supabase.co";
const SUPABASE_ANON_KEY: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJpc3MiOiJzdXBhYmFzZSIsInJlZiI6ImpuZ2tpbGpobW5yeWdkbmZ2cmdwIiwicm9sZSI6ImFub24iLCJpYXQiOjE3NzIyMzUwNzEsImV4cCI6MjA4NzgxMTA3MX0.xe6N-2KaY0Tx2eYI-UZJrlSa3v2EYZJT-PMVFRDPnzI";

// Register this device with Supabase after auth
pub async fn register_device(
    access_token: &str,
    user_id: &str,
    device_name: &str,
    platform: &str,
    site_id: &[u8],  // from crsql_site_id()
) -> Result<String, String> {
    use base64::{Engine as _, engine::general_purpose};
    let client = reqwest::Client::new();
    let res = client
        .post(format!("{}/rest/v1/devices", SUPABASE_URL))
        .header("apikey", SUPABASE_ANON_KEY)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Content-Type", "application/json")
        .header("Prefer", "return=representation")
        .json(&serde_json::json!({
            "user_id": user_id,
            "device_name": device_name,
            "platform": platform,
            "site_id": general_purpose::STANDARD.encode(site_id),
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
            .and_then(|v| v.as_str())
            .unwrap_or("Failed to register device");
        return Err(error_msg.to_string());
    }

    // Response should be an array with one element
    let device = if let Some(arr) = body.as_array() {
        arr.first().ok_or_else(|| "Empty response".to_string())?
    } else {
        &body
    };

    let device_id = device
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "No device id in response".to_string())?
        .to_string();

    Ok(device_id)
}

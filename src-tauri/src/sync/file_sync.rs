use reqwest;
use std::path::PathBuf;
use super::crypto;
use super::keystore;

const SUPABASE_URL: &str = "https://jngkiljhmnrygdnfvrgp.supabase.co";
const SUPABASE_ANON_KEY: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJpc3MiOiJzdXBhYmFzZSIsInJlZiI6ImpuZ2tpbGpobW5yeWdkbmZ2cmdwIiwicm9sZSI6ImFub24iLCJpYXQiOjE3NzIyMzUwNzEsImV4cCI6MjA4NzgxMTA3MX0.xe6N-2KaY0Tx2eYI-UZJrlSa3v2EYZJT-PMVFRDPnzI";

const STORAGE_BUCKET: &str = "pane-files";

/// Upload an encrypted file to Supabase Storage
pub async fn upload_encrypted_file(
    access_token: &str,
    user_id: &str,
    note_id: &str,
    content: &[u8],
) -> Result<(), String> {
    // Encrypt with file key
    let master_key = keystore::load_master_key()?
        .ok_or_else(|| "No encryption key".to_string())?;
    let file_key = crypto::get_file_key(&master_key);
    let encrypted = crypto::encrypt_file(&file_key, content)?;
    
    let storage_path = format!("{}/{}.enc", user_id, note_id);
    
    let client = reqwest::Client::new();
    let res = client
        .post(format!("{}/storage/v1/object/{}/{}", SUPABASE_URL, STORAGE_BUCKET, storage_path))
        .header("apikey", SUPABASE_ANON_KEY)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Content-Type", "application/octet-stream")
        .header("x-upsert", "true")  // Overwrite if exists
        .body(encrypted)
        .send()
        .await
        .map_err(|e| format!("Upload error: {}", e))?;
    
    let status = res.status();
    if !status.is_success() {
        let body = res.text().await.unwrap_or_default();
        return Err(format!("Upload failed ({}): {}", status, body));
    }
    
    println!("[file-sync] Uploaded {}.enc ({} bytes)", note_id, content.len());
    Ok(())
}

/// Download and decrypt a file from Supabase Storage
pub async fn download_encrypted_file(
    access_token: &str,
    user_id: &str,
    note_id: &str,
) -> Result<Vec<u8>, String> {
    let storage_path = format!("{}/{}.enc", user_id, note_id);
    
    let client = reqwest::Client::new();
    let res = client
        .get(format!("{}/storage/v1/object/{}/{}", SUPABASE_URL, STORAGE_BUCKET, storage_path))
        .header("apikey", SUPABASE_ANON_KEY)
        .header("Authorization", format!("Bearer {}", access_token))
        .send()
        .await
        .map_err(|e| format!("Download error: {}", e))?;
    
    let status = res.status();
    if !status.is_success() {
        let body = res.text().await.unwrap_or_default();
        return Err(format!("Download failed ({}): {}", status, body));
    }
    
    let encrypted = res.bytes().await
        .map_err(|e| format!("Read error: {}", e))?;
    
    // Decrypt with file key
    let master_key = keystore::load_master_key()?
        .ok_or_else(|| "No encryption key".to_string())?;
    let file_key = crypto::get_file_key(&master_key);
    let decrypted = crypto::decrypt_file(&file_key, &encrypted)?;
    
    println!("[file-sync] Downloaded {}.enc ({} bytes decrypted)", note_id, decrypted.len());
    Ok(decrypted)
}

/// Sync a single note's file content
pub async fn sync_note_file(
    access_token: &str,
    user_id: &str,
    note_id: &str,
    file_path: &str,
) -> Result<(), String> {
    let path = PathBuf::from(file_path);
    if !path.exists() {
        return Ok(()); // No file to sync
    }
    
    let content = std::fs::read(&path)
        .map_err(|e| format!("Failed to read file: {}", e))?;
    
    if content.is_empty() {
        return Ok(()); // Don't sync empty files
    }
    
    upload_encrypted_file(access_token, user_id, note_id, &content).await
}

/// Download a note's content and write to disk
pub async fn pull_note_file(
    access_token: &str,
    user_id: &str,
    note_id: &str,
    file_path: &str,
) -> Result<(), String> {
    let content = download_encrypted_file(access_token, user_id, note_id).await?;
    
    let path = PathBuf::from(file_path);
    
    // Create parent directories if needed
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create dirs: {}", e))?;
    }
    
    std::fs::write(&path, &content)
        .map_err(|e| format!("Failed to write file: {}", e))?;
    
    println!("[file-sync] Wrote {} bytes to {}", content.len(), file_path);
    Ok(())
}

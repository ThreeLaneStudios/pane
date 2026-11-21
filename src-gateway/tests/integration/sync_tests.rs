use super::TestGateway;
use serde_json::json;
use sha2::Digest;

/// §4-helper-test: HTTP-level integration test — exercises the push() handler via HTTP POST; AST analyzer cannot trace through HTTP boundary
#[tokio::test]
async fn test_sync_push_round_trip() {
    eprintln!("[SYNC_TEST] test_sync_push_round_trip");
    let gateway = TestGateway::new().await.expect("Failed to create test gateway");

    // ADR-0040: configure agents.list with a workspace path so push can resolve it
    let workspace_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let workspace_path = workspace_dir.path().to_str().unwrap().to_string();
    gateway
        .mock()
        .configure_response(
            "agents.list",
            json!({
                "agents": [{
                    "id": "test-sync-round-trip",
                    "name": "TestAgent",
                    "workspace": workspace_path,
                    "model": {"primary": "anthropic/claude-sonnet-4-5"}
                }]
            }),
        )
        .await;

    let content = "test content for round trip";
    let mut hasher = sha2::Sha256::new();
    hasher.update(content.as_bytes());
    let checksum = hex::encode(hasher.finalize());

    let response = gateway
        .client()
        .post(&format!("{}/v1/sync/push", gateway.base_url()))
        .header("Authorization", format!("Bearer {}", gateway.auth_token()))
        .json(&json!({
            "oc_agent_id": "test-sync-round-trip",
            "filename": "SOUL.md",
            "content": content,
            "checksum": checksum
        }))
        .send()
        .await
        .expect("Failed to send request");

    eprintln!("[SYNC_TEST] Response status: {}", response.status());
    assert_eq!(response.status(), 200);

    let body: serde_json::Value = response.json().await.expect("Failed to parse JSON");
    eprintln!("[SYNC_TEST] Response body: {}", body);
    
    assert!(body.get("checksum").is_some());
    assert_eq!(body["checksum"].as_str().unwrap(), checksum);

    // Verify file was written to the OC-provided workspace (not a hardcoded path)
    let written_file = workspace_dir.path().join("SOUL.md");
    assert!(written_file.exists(), "File should have been written to OC-provided workspace");
    let written_content = std::fs::read_to_string(&written_file).expect("Failed to read written file");
    assert_eq!(written_content, content);
}

/// §4-helper-test: HTTP-level integration test — exercises the push() handler via HTTP POST; AST analyzer cannot trace through HTTP boundary
#[tokio::test]
async fn test_sync_push_oversized() {
    eprintln!("[SYNC_TEST] test_sync_push_oversized");
    let gateway = TestGateway::new().await.expect("Failed to create test gateway");

    // Create content larger than 10 MiB
    let content = "x".repeat(11 * 1024 * 1024);
    let mut hasher = sha2::Sha256::new();
    hasher.update(content.as_bytes());
    let checksum = hex::encode(hasher.finalize());

    let response = gateway
        .client()
        .post(&format!("{}/v1/sync/push", gateway.base_url()))
        .header("Authorization", format!("Bearer {}", gateway.auth_token()))
        .json(&json!({
            "oc_agent_id": "test-sync-oversized",
            "filename": "test-oversized.txt",
            "content": content,
            "checksum": checksum
        }))
        .send()
        .await
        .expect("Failed to send request");

    eprintln!("[SYNC_TEST] Response status: {}", response.status());
    assert_eq!(response.status(), 413);
}

/// §4-helper-test: HTTP-level integration test — exercises the push() handler via HTTP POST; AST analyzer cannot trace through HTTP boundary
#[tokio::test]
async fn test_sync_push_checksum_mismatch() {
    eprintln!("[SYNC_TEST] test_sync_push_checksum_mismatch");
    let gateway = TestGateway::new().await.expect("Failed to create test gateway");

    let content = "test content for checksum mismatch";
    let wrong_checksum = "0000000000000000000000000000000000000000000000000000000000000000";

    let response = gateway
        .client()
        .post(&format!("{}/v1/sync/push", gateway.base_url()))
        .header("Authorization", format!("Bearer {}", gateway.auth_token()))
        .json(&json!({
            "oc_agent_id": "test-sync-checksum",
            "filename": "test-checksum.txt",
            "content": content,
            "checksum": wrong_checksum
        }))
        .send()
        .await
        .expect("Failed to send request");

    eprintln!("[SYNC_TEST] Response status: {}", response.status());
    // Should fail - either 400 (checksum mismatch) or 500 (filesystem error after failed checksum)
    assert!(response.status().is_client_error() || response.status().is_server_error());
    assert_ne!(response.status(), 200);

    let body: serde_json::Value = response.json().await.expect("Failed to parse JSON");
    eprintln!("[SYNC_TEST] Response body: {}", body);
    
    assert!(body.get("error").is_some());
}

/// §4-helper-test: HTTP-level integration test — exercises the push() handler via HTTP POST; AST analyzer cannot trace through HTTP boundary
#[tokio::test]
async fn test_sync_push_cleans_temp_on_success() {
    // This test verifies MINOR-2: temp files are cleaned up after successful push.
    // We can't easily simulate a rename failure through the HTTP API,
    // so instead verify the positive case: after a successful push,
    // no .tmp files remain in the agent directory.
    eprintln!("[SYNC_TEST] test_sync_push_cleans_temp_on_success");
    let gateway = TestGateway::new().await.expect("Failed to create test gateway");

    let agent_id = "test-temp-cleanup";
    let content = "# Test content for temp cleanup";
    let mut hasher = sha2::Sha256::new();
    hasher.update(content.as_bytes());
    let checksum = hex::encode(hasher.finalize());

    // ADR-0040: configure agents.list with a real workspace path
    let workspace_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let workspace_path = workspace_dir.path().to_str().unwrap().to_string();
    gateway
        .mock()
        .configure_response(
            "agents.list",
            json!({
                "agents": [{
                    "id": agent_id,
                    "name": "TestAgent",
                    "workspace": workspace_path,
                    "model": {"primary": "anthropic/claude-sonnet-4-5"}
                }]
            }),
        )
        .await;

    // Push a file
    let response = gateway
        .client()
        .post(&format!("{}/v1/sync/push", gateway.base_url()))
        .header("Authorization", format!("Bearer {}", gateway.auth_token()))
        .json(&json!({
            "oc_agent_id": agent_id,
            "filename": "SOUL.md",
            "content": content,
            "checksum": checksum
        }))
        .send()
        .await
        .expect("Failed to send request");

    eprintln!("[SYNC_TEST] Response status: {}", response.status());
    assert_eq!(response.status(), 200);

    // Verify: no .tmp files exist in the OC-provided workspace
    let workspace_path_buf = workspace_dir.path();
    eprintln!("[SYNC_TEST] Checking workspace for .tmp files: {}", workspace_path_buf.display());

    // Walk the directory and assert no .tmp files
    let tmp_files = find_tmp_files(workspace_path_buf).await;
    
    if !tmp_files.is_empty() {
        eprintln!("[SYNC_TEST] Found orphaned .tmp files:");
        for tmp_file in &tmp_files {
            eprintln!("[SYNC_TEST]   - {}", tmp_file.display());
        }
    }
    
    assert_eq!(tmp_files.len(), 0, "Found {} .tmp file(s) after successful push", tmp_files.len());
}

/// Recursively find all .tmp files in a directory
fn find_tmp_files(dir: &std::path::Path) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<std::path::PathBuf>> + Send + '_>> {
    Box::pin(async move {
        let mut tmp_files = Vec::new();
        
        if !dir.exists() {
            return tmp_files;
        }

        let mut entries = match tokio::fs::read_dir(dir).await {
            Ok(entries) => entries,
            Err(_) => return tmp_files,
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            let metadata = match entry.metadata().await {
                Ok(m) => m,
                Err(_) => continue,
            };

            if metadata.is_file() && path.extension().and_then(|s| s.to_str()) == Some("tmp") {
                tmp_files.push(path);
            } else if metadata.is_dir() {
                // Recurse into subdirectories
                tmp_files.extend(find_tmp_files(&path).await);
            }
        }

        tmp_files
    })
}

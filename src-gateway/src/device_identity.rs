use crate::error::GatewayError;
use ed25519_dalek::{Signature, Signer, SigningKey};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use base64::{Engine as _, engine::general_purpose};

/// Device identity stored at ~/.pane-gateway/device.json
#[derive(Debug, Serialize, Deserialize)]
pub struct DeviceIdentity {
    pub version: u32,
    #[serde(rename = "deviceId")]
    pub device_id: String,
    #[serde(rename = "publicKeyPem")]
    pub public_key_pem: String,
    #[serde(rename = "privateKeyPem")]
    pub private_key_pem: String,
    #[serde(rename = "createdAtMs")]
    pub created_at_ms: u64,
}

impl DeviceIdentity {
    /// Load device identity from ~/.pane-gateway/device.json or generate new one
    pub fn load_or_create() -> Result<Self, GatewayError> {
        let path = Self::identity_path()?;
        
        if path.exists() {
            eprintln!("[DEVICE_IDENTITY] Loading existing identity from {:?}", path);
            Self::load_from_file(&path)
        } else {
            eprintln!("[DEVICE_IDENTITY] No existing identity found, generating new keypair");
            Self::generate_and_save(&path)
        }
    }
    
    /// Get the path to device.json
    fn identity_path() -> Result<PathBuf, GatewayError> {
        let home = std::env::var("HOME")
            .map_err(|_| GatewayError::OcClient("HOME env var not set".to_string()))?;
        
        let gateway_dir = PathBuf::from(home).join(".pane-gateway");
        
        // Ensure directory exists
        if !gateway_dir.exists() {
            fs::create_dir_all(&gateway_dir)
                .map_err(|e| GatewayError::OcClient(format!("Failed to create gateway dir: {}", e)))?;
        }
        
        Ok(gateway_dir.join("device.json"))
    }
    
    /// Load identity from file
    fn load_from_file(path: &PathBuf) -> Result<Self, GatewayError> {
        let content = fs::read_to_string(path)
            .map_err(|e| GatewayError::OcClient(format!("Failed to read device.json: {}", e)))?;
        
        let identity: DeviceIdentity = serde_json::from_str(&content)
            .map_err(|e| GatewayError::OcClient(format!("Failed to parse device.json: {}", e)))?;
        
        eprintln!("[DEVICE_IDENTITY] Loaded identity with device_id: {}", identity.device_id);
        Ok(identity)
    }
    
    /// Generate new identity and save to file
    fn generate_and_save(path: &PathBuf) -> Result<Self, GatewayError> {
        eprintln!("[DEVICE_IDENTITY] Generating Ed25519 keypair");
        
        // Generate random 32-byte seed for Ed25519 key
        let mut seed = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut seed);
        
        let signing_key = SigningKey::from_bytes(&seed);
        
        // Extract raw public key bytes (32 bytes for Ed25519)
        let verifying_key = signing_key.verifying_key();
        let public_key_bytes = verifying_key.to_bytes();
        
        // Compute device ID: SHA-256 of raw public key bytes
        let mut hasher = Sha256::new();
        hasher.update(&public_key_bytes);
        let device_id = format!("{:x}", hasher.finalize());
        
        eprintln!("[DEVICE_IDENTITY] Device ID: {}", device_id);
        
        // Convert to PEM format
        // For Ed25519, we need to construct SPKI (Subject Public Key Info) format
        // SPKI prefix for Ed25519: 302a300506032b6570032100 (12 bytes) + 32-byte public key
        let mut spki_der = vec![
            0x30, 0x2a, // SEQUENCE, length 42
            0x30, 0x05, // SEQUENCE, length 5
            0x06, 0x03, 0x2b, 0x65, 0x70, // OID 1.3.101.112 (Ed25519)
            0x03, 0x21, 0x00, // BIT STRING, length 33, 0 unused bits
        ];
        spki_der.extend_from_slice(&public_key_bytes);
        
        let public_key_pem = format!(
            "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----",
            general_purpose::STANDARD.encode(&spki_der)
        );
        
        // For private key, we need PKCS#8 format
        // PKCS#8 prefix for Ed25519: 302e020100300506032b657004220420 (16 bytes) + 32-byte private key
        let secret_bytes = signing_key.to_bytes();
        let mut pkcs8_der = vec![
            0x30, 0x2e, // SEQUENCE, length 46
            0x02, 0x01, 0x00, // INTEGER 0 (version)
            0x30, 0x05, // SEQUENCE, length 5
            0x06, 0x03, 0x2b, 0x65, 0x70, // OID 1.3.101.112 (Ed25519)
            0x04, 0x22, // OCTET STRING, length 34
            0x04, 0x20, // OCTET STRING, length 32
        ];
        pkcs8_der.extend_from_slice(&secret_bytes);
        
        let private_key_pem = format!(
            "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----",
            general_purpose::STANDARD.encode(&pkcs8_der)
        );
        
        // Get current timestamp
        let created_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| GatewayError::OcClient(format!("Failed to get timestamp: {}", e)))?
            .as_millis() as u64;
        
        let identity = DeviceIdentity {
            version: 1,
            device_id,
            public_key_pem,
            private_key_pem,
            created_at_ms,
        };
        
        // Save to file with 0o600 permissions
        let json = serde_json::to_string_pretty(&identity)
            .map_err(|e| GatewayError::OcClient(format!("Failed to serialize identity: {}", e)))?;
        
        fs::write(path, json)
            .map_err(|e| GatewayError::OcClient(format!("Failed to write device.json: {}", e)))?;
        
        // Set file permissions to 0o600 (owner read/write only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let permissions = fs::Permissions::from_mode(0o600);
            fs::set_permissions(path, permissions)
                .map_err(|e| GatewayError::OcClient(format!("Failed to set permissions: {}", e)))?;
        }
        
        eprintln!("[DEVICE_IDENTITY] Saved new identity to {:?}", path);
        Ok(identity)
    }
    
    /// Sign the v3 device auth payload
    pub fn sign_v3_payload(
        &self,
        nonce: &str,
        token: &str,
        scopes: &[&str],
    ) -> Result<DeviceAuthData, GatewayError> {
        // #3106 M2 (FAIL remediation, #3200): the nonce is a one-time
        // authentication value fed into the signature below — never log it
        // in full. Zero characters, zero shape, same treatment as every
        // other redacted site in this crate.
        eprintln!("[DEVICE_IDENTITY] Signing v3 payload");
        
        // Get current timestamp
        let signed_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| GatewayError::OcClient(format!("Failed to get timestamp: {}", e)))?
            .as_millis() as u64;
        
        // Build payload: "v3|{deviceId}|gateway-client|backend|operator|{scopes}|{signedAtMs}|{token}|{nonce}|macos|"
        let scopes_joined = scopes.join(",");
        let payload = format!(
            "v3|{}|gateway-client|backend|operator|{}|{}|{}|{}|macos|",
            self.device_id, scopes_joined, signed_at_ms, token, nonce
        );
        
        // #3106 M2 (FAIL remediation, #3200): `payload` embeds the live OC
        // bearer token in cleartext (`v3|{deviceId}|...|{token}|{nonce}|...`)
        // — never log it in full. Zero characters, zero shape, same
        // treatment as every other redacted site in this crate.
        eprintln!("[DEVICE_IDENTITY] Payload constructed for signing");
        
        // Parse private key from PEM
        let signing_key = self.parse_signing_key()?;
        
        // Sign the payload
        let signature: Signature = signing_key.sign(payload.as_bytes());
        
        // Encode signature as base64url (no padding)
        let signature_b64url = general_purpose::URL_SAFE_NO_PAD.encode(signature.to_bytes());
        
        // Extract raw public key bytes and encode as base64url
        let public_key_b64url = self.public_key_base64url()?;
        
        eprintln!("[DEVICE_IDENTITY] Signature generated");
        
        Ok(DeviceAuthData {
            device_id: self.device_id.clone(),
            public_key: public_key_b64url,
            signature: signature_b64url,
            signed_at: signed_at_ms,
            nonce: nonce.to_string(),
        })
    }
    
    /// Parse signing key from stored PEM
    fn parse_signing_key(&self) -> Result<SigningKey, GatewayError> {
        // Decode PEM to DER
        let pem_content = self.private_key_pem
            .replace("-----BEGIN PRIVATE KEY-----", "")
            .replace("-----END PRIVATE KEY-----", "")
            .replace("\n", "");
        
        let der = general_purpose::STANDARD.decode(&pem_content)
            .map_err(|e| GatewayError::OcClient(format!("Failed to decode private key PEM: {}", e)))?;
        
        // Extract 32-byte secret from PKCS#8
        // PKCS#8 prefix is 16 bytes, then 32-byte secret
        if der.len() < 48 {
            return Err(GatewayError::OcClient("Invalid PKCS#8 private key".to_string()));
        }
        
        let secret_bytes: [u8; 32] = der[16..48].try_into()
            .map_err(|_| GatewayError::OcClient("Invalid secret key length".to_string()))?;
        
        let signing_key = SigningKey::from_bytes(&secret_bytes);
        
        Ok(signing_key)
    }
    
    /// Get raw public key bytes encoded as base64url
    fn public_key_base64url(&self) -> Result<String, GatewayError> {
        // Decode PEM to DER
        let pem_content = self.public_key_pem
            .replace("-----BEGIN PUBLIC KEY-----", "")
            .replace("-----END PUBLIC KEY-----", "")
            .replace("\n", "");
        
        let der = general_purpose::STANDARD.decode(&pem_content)
            .map_err(|e| GatewayError::OcClient(format!("Failed to decode public key PEM: {}", e)))?;
        
        // Extract 32-byte public key from SPKI (skip 12-byte prefix)
        if der.len() < 44 {
            return Err(GatewayError::OcClient("Invalid SPKI public key".to_string()));
        }
        
        let public_key_bytes = &der[12..44];
        
        // Encode as base64url (no padding)
        Ok(general_purpose::URL_SAFE_NO_PAD.encode(public_key_bytes))
    }
}

/// Device auth data to include in connect request
#[derive(Debug, Clone, Serialize)]
pub struct DeviceAuthData {
    pub device_id: String,
    pub public_key: String,
    pub signature: String,
    pub signed_at: u64,
    pub nonce: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_device_identity_generation() {
        // Generate a test identity
        let temp_dir = std::env::temp_dir();
        let test_path = temp_dir.join(format!("test_device_{}.json", rand::random::<u32>()));
        
        let identity = DeviceIdentity::generate_and_save(&test_path).unwrap();
        
        // Verify fields
        assert_eq!(identity.version, 1);
        assert!(!identity.device_id.is_empty());
        assert!(identity.public_key_pem.starts_with("-----BEGIN PUBLIC KEY-----"));
        assert!(identity.private_key_pem.starts_with("-----BEGIN PRIVATE KEY-----"));
        assert!(identity.created_at_ms > 0);
        
        // Verify file was created
        assert!(test_path.exists());
        
        // Clean up
        let _ = fs::remove_file(test_path);
    }
    
    #[test]
    fn test_load_existing_identity() {
        // Generate and save
        let temp_dir = std::env::temp_dir();
        let test_path = temp_dir.join(format!("test_device_{}.json", rand::random::<u32>()));
        
        let original = DeviceIdentity::generate_and_save(&test_path).unwrap();
        
        // Load it back
        let loaded = DeviceIdentity::load_from_file(&test_path).unwrap();
        
        // Verify they match
        assert_eq!(loaded.device_id, original.device_id);
        assert_eq!(loaded.public_key_pem, original.public_key_pem);
        assert_eq!(loaded.private_key_pem, original.private_key_pem);
        
        // Clean up
        let _ = fs::remove_file(test_path);
    }
    
    #[test]
    fn test_sign_v3_payload() {
        let temp_dir = std::env::temp_dir();
        let test_path = temp_dir.join(format!("test_device_{}.json", rand::random::<u32>()));
        
        let identity = DeviceIdentity::generate_and_save(&test_path).unwrap();
        
        // Sign a test payload
        let scopes = vec!["operator.read", "operator.write"];
        let auth_data = identity.sign_v3_payload("test_nonce", "test_token", &scopes).unwrap();
        
        // Verify fields
        assert_eq!(auth_data.device_id, identity.device_id);
        assert!(!auth_data.public_key.is_empty());
        assert!(!auth_data.signature.is_empty());
        assert!(auth_data.signed_at > 0);
        assert_eq!(auth_data.nonce, "test_nonce");
        
        // Verify base64url encoding (no padding characters)
        assert!(!auth_data.public_key.contains('='));
        assert!(!auth_data.signature.contains('='));
        
        // Clean up
        let _ = fs::remove_file(test_path);
    }
    
    #[test]
    fn test_device_id_is_sha256_of_public_key() {
        let temp_dir = std::env::temp_dir();
        let test_path = temp_dir.join(format!("test_device_{}.json", rand::random::<u32>()));
        
        let identity = DeviceIdentity::generate_and_save(&test_path).unwrap();
        
        // Extract raw public key
        let pem_content = identity.public_key_pem
            .replace("-----BEGIN PUBLIC KEY-----", "")
            .replace("-----END PUBLIC KEY-----", "")
            .replace("\n", "");
        
        let der = general_purpose::STANDARD.decode(&pem_content).unwrap();
        let public_key_bytes = &der[12..44]; // Skip SPKI prefix
        
        // Compute SHA-256
        let mut hasher = Sha256::new();
        hasher.update(public_key_bytes);
        let expected_device_id = format!("{:x}", hasher.finalize());
        
        assert_eq!(identity.device_id, expected_device_id);
        
        // Clean up
        let _ = fs::remove_file(test_path);
    }
}

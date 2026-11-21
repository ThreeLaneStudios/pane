use argon2::Argon2;
use chacha20poly1305::{
    aead::{Aead, KeyInit, OsRng},
    XChaCha20Poly1305, XNonce,
};
use rand::RngCore;

const ARGON2_SALT_PREFIX: &[u8] = b"pane-sync-v1-";

/// Derive a 256-bit master key from password + email (as salt)
pub fn derive_master_key(password: &str, email: &str) -> Result<[u8; 32], String> {
    let mut salt = Vec::new();
    salt.extend_from_slice(ARGON2_SALT_PREFIX);
    salt.extend_from_slice(email.as_bytes());
    
    // Pad or hash the salt to exactly 16 bytes (Argon2 requirement)
    let salt_hash = blake3::hash(&salt);
    let salt_bytes = &salt_hash.as_bytes()[..16];
    
    let argon2 = Argon2::default(); // Argon2id with default params
    let mut key = [0u8; 32];
    
    argon2.hash_password_into(
        password.as_bytes(),
        salt_bytes,
        &mut key,
    ).map_err(|e| format!("Key derivation failed: {}", e))?;
    
    Ok(key)
}

/// Derive a sub-key from the master key using BLAKE3 keyed hash
pub fn derive_sub_key(master_key: &[u8; 32], context: &str) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_keyed(master_key);
    hasher.update(context.as_bytes());
    let hash = hasher.finalize();
    *hash.as_bytes()
}

/// Get the sync encryption key from master key
pub fn get_sync_key(master_key: &[u8; 32]) -> [u8; 32] {
    derive_sub_key(master_key, "pane-sync-encryption-v1")
}

/// Get the file encryption key from master key
pub fn get_file_key(master_key: &[u8; 32]) -> [u8; 32] {
    derive_sub_key(master_key, "pane-file-encryption-v1")
}

/// Encrypt data with XChaCha20-Poly1305
/// Returns: nonce (24 bytes) || ciphertext (variable)
pub fn encrypt(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>, String> {
    let cipher = XChaCha20Poly1305::new(key.into());
    
    // Generate random 24-byte nonce
    let mut nonce_bytes = [0u8; 24];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = XNonce::from_slice(&nonce_bytes);
    
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| format!("Encryption failed: {}", e))?;
    
    // Prepend nonce to ciphertext
    let mut result = Vec::with_capacity(24 + ciphertext.len());
    result.extend_from_slice(&nonce_bytes);
    result.extend_from_slice(&ciphertext);
    
    Ok(result)
}

/// Decrypt data with XChaCha20-Poly1305
/// Input: nonce (24 bytes) || ciphertext
pub fn decrypt(key: &[u8; 32], data: &[u8]) -> Result<Vec<u8>, String> {
    if data.len() < 24 {
        return Err("Data too short to contain nonce".to_string());
    }
    
    let nonce = XNonce::from_slice(&data[..24]);
    let ciphertext = &data[24..];
    
    let cipher = XChaCha20Poly1305::new(key.into());
    
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| format!("Decryption failed: {}", e))
}

/// Encrypt a changeset for sync
pub fn encrypt_changeset(sync_key: &[u8; 32], changeset: &[u8]) -> Result<Vec<u8>, String> {
    encrypt(sync_key, changeset)
}

/// Decrypt a changeset from sync
pub fn decrypt_changeset(sync_key: &[u8; 32], encrypted: &[u8]) -> Result<Vec<u8>, String> {
    decrypt(sync_key, encrypted)
}

/// Encrypt a file for storage
pub fn encrypt_file(file_key: &[u8; 32], content: &[u8]) -> Result<Vec<u8>, String> {
    encrypt(file_key, content)
}

/// Decrypt a file from storage
pub fn decrypt_file(file_key: &[u8; 32], encrypted: &[u8]) -> Result<Vec<u8>, String> {
    decrypt(file_key, encrypted)
}

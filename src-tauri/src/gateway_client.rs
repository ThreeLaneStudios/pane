use crate::database::DbState;
use rustls::ClientConfig;
use rustls::DigitallySignedStruct;
use rustls::Error as TlsError;
use rustls::SignatureScheme;
use rustls::client::danger;
use rustls::crypto::WebPkiSupportedAlgorithms;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use url::Url;

// ── DER walk helpers ─────────────────────────────────────────────────────────

/// Decode the 1-, 2-, or 3-byte DER length field at the start of `data`.
/// Returns `Some((length, bytes_consumed))` or `None` on truncation.
fn der_read_length(data: &[u8]) -> Option<(usize, usize)> {
    if data.is_empty() {
        return None;
    }
    let first = data[0] as usize;
    if first < 0x80 {
        // Short form: length is the byte itself
        Some((first, 1))
    } else {
        let num_bytes = first & 0x7f;
        if num_bytes == 0 || num_bytes > 4 || data.len() < 1 + num_bytes {
            return None;
        }
        let mut len = 0usize;
        for i in 0..num_bytes {
            len = (len << 8) | (data[1 + i] as usize);
        }
        Some((len, 1 + num_bytes))
    }
}

/// Return the total byte length of the TLV at `data[0..]` (tag + length + value).
/// Does not consume; returns `None` on truncation.
fn tlv_total_len(data: &[u8]) -> Option<usize> {
    if data.is_empty() {
        return None;
    }
    let (value_len, len_bytes) = der_read_length(&data[1..])?;
    let total = 1 + len_bytes + value_len;
    if data.len() < total {
        None
    } else {
        Some(total)
    }
}

/// Skip one TLV element at `data[0..]`; return pointer to next element.
fn skip_tlv(data: &[u8]) -> Option<&[u8]> {
    let n = tlv_total_len(data)?;
    data.get(n..)
}

/// Enter a SEQUENCE (tag 0x30); return a slice of the value bytes.
fn enter_sequence(data: &[u8]) -> Option<&[u8]> {
    if data.first() != Some(&0x30) {
        return None;
    }
    let (value_len, len_bytes) = der_read_length(&data[1..])?;
    let value_start = 1 + len_bytes;
    data.get(value_start..value_start + value_len)
}

/// Extract the raw DER bytes of the SubjectPublicKeyInfo from a DER-encoded X.509 certificate.
/// Returns the full SPKI TLV (tag + length + value) bytes.
/// Used for HPKP-style SHA-256 SPKI pinning (RFC 7469 §2.4).
fn extract_spki_der(cert_der: &[u8]) -> Option<Vec<u8>> {
    // Step 1: Enter outer Certificate SEQUENCE
    let cert_body = enter_sequence(cert_der)?;

    // Step 2: Enter TBSCertificate SEQUENCE
    let tbs_body = enter_sequence(cert_body)?;
    let mut pos = tbs_body;

    // Step 3: Skip version [0] EXPLICIT (tag 0xa0, optional)
    if pos.first() == Some(&0xa0) {
        pos = skip_tlv(pos)?;
    }

    // Step 4: Skip serialNumber (INTEGER, tag 0x02)
    pos = skip_tlv(pos)?;

    // Step 5: Skip signature AlgorithmIdentifier (SEQUENCE)
    pos = skip_tlv(pos)?;

    // Step 6: Skip issuer (SEQUENCE)
    pos = skip_tlv(pos)?;

    // Step 7: Skip validity (SEQUENCE)
    pos = skip_tlv(pos)?;

    // Step 8: Skip subject (SEQUENCE)
    pos = skip_tlv(pos)?;

    // Step 9: Capture SubjectPublicKeyInfo SEQUENCE — full TLV
    let spki_len = tlv_total_len(pos)?;
    Some(pos[..spki_len].to_vec())
}

/// Decode a PEM-encoded certificate (first certificate block found) to DER bytes.
fn pem_to_der(pem: &str) -> Result<Vec<u8>, String> {
    let b64: String = pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect::<Vec<_>>()
        .join("");

    base64_decode_standard(&b64)
        .map_err(|e| format!("PEM base64 decode failed: {}", e))
}

/// Standard base64 decode (A–Z, a–z, 0–9, +, /).
fn base64_decode_standard(input: &str) -> Result<Vec<u8>, String> {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = Vec::new();
    let mut buffer: u32 = 0;
    let mut bits = 0u32;
    for ch in input.bytes() {
        if ch == b'=' || ch == b'\n' || ch == b'\r' || ch == b' ' {
            continue;
        }
        let val = CHARS
            .iter()
            .position(|&c| c == ch)
            .ok_or_else(|| format!("Invalid base64 char: {:?}", ch as char))? as u32;
        buffer = (buffer << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            result.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Ok(result)
}

// ── PinnedCertVerifier ────────────────────────────────────────────────────────

/// A custom rustls ServerCertVerifier that pins the SHA-256 hash of the server's
/// SubjectPublicKeyInfo (HPKP-style, RFC 7469 §2.4).
///
/// Hostname/SAN verification is intentionally skipped — the SPKI pin is the
/// only trust gate. This allows the Pane app to connect to the gateway by
/// Tailscale IPv4 even if the self-signed cert's SAN list omits that IP.
#[derive(Debug)]
struct PinnedCertVerifier {
    /// SHA-256 of the SPKI DER of the pinned (stored) certificate.
    /// Computed once at client-build time from the stored cert PEM.
    pinned_spki_hash: [u8; 32],
    supported_algs: WebPkiSupportedAlgorithms,
}

impl danger::ServerCertVerifier for PinnedCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>, // intentionally ignored — SPKI pin supersedes hostname check
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<danger::ServerCertVerified, TlsError> {
        // Extract SPKI from the cert presented by the server
        let spki = extract_spki_der(end_entity.as_ref()).ok_or_else(|| {
            TlsError::InvalidCertificate(rustls::CertificateError::BadEncoding)
        })?;

        // Hash the SPKI bytes
        let mut hasher = Sha256::new();
        hasher.update(&spki);
        let hash: [u8; 32] = hasher.finalize().into();

        if hash == self.pinned_spki_hash {
            Ok(danger::ServerCertVerified::assertion())
        } else {
            eprintln!(
                "[GATEWAY_CLIENT] SPKI pin mismatch — presented: {}, expected: {}",
                hex::encode(hash),
                hex::encode(self.pinned_spki_hash)
            );
            Err(TlsError::InvalidCertificate(
                rustls::CertificateError::ApplicationVerificationFailure,
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<danger::HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.supported_algs,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<danger::HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.supported_algs,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.supported_algs.supported_schemes()
    }
}

// ── TLS config builder ────────────────────────────────────────────────────────

/// Build a rustls ClientConfig that pins the server's SPKI (RFC 7469 §2.4).
/// No hostname/SAN check is performed — the key hash is the only trust gate.
fn build_pin_only_tls_config(pinned_pem: &str) -> Result<ClientConfig, String> {
    // 1. Decode the stored cert PEM → DER
    let der = pem_to_der(pinned_pem)?;

    // 2. Extract SPKI DER from the cert
    let spki = extract_spki_der(&der)
        .ok_or_else(|| "Failed to extract SPKI from stored gateway cert".to_string())?;

    // 3. SHA-256 the SPKI bytes → pinned hash
    let mut hasher = Sha256::new();
    hasher.update(&spki);
    let pinned_spki_hash: [u8; 32] = hasher.finalize().into();

    eprintln!(
        "[GATEWAY_CLIENT] SPKI pin (SHA-256): {}",
        hex::encode(pinned_spki_hash)
    );

    // 4. Build verifier
    let crypto = rustls::crypto::ring::default_provider();
    let supported_algs = crypto.signature_verification_algorithms;
    let verifier = Arc::new(PinnedCertVerifier {
        pinned_spki_hash,
        supported_algs,
    });

    // 5. Build ClientConfig
    ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("TLS protocol versions error: {}", e))?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth()
        .pipe_ok()
}

/// Tiny helper to convert a value into `Ok(value)` — avoids a spurious `let x = ...; Ok(x)`.
trait PipeOk: Sized {
    fn pipe_ok<E>(self) -> Result<Self, E> {
        Ok(self)
    }
}
impl<T> PipeOk for T {}

// ── Public API ────────────────────────────────────────────────────────────────

/// Validate and canonicalize an OpenClaw gateway base URL at the native trust boundary.
/// Private, loopback, LAN, and Tailscale hosts are intentionally allowed.
pub(crate) fn canonicalize_gateway_base_url(raw: &str) -> Result<Url, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("Gateway URL is required".to_string());
    }

    let mut url = Url::parse(raw).map_err(|_| {
        "Invalid gateway URL. Enter an HTTPS host or IP address with an optional port."
            .to_string()
    })?;
    if url.scheme() != "https" {
        return Err("Gateway URL must use HTTPS".to_string());
    }
    if url.cannot_be_a_base() || url.host_str().is_none() {
        return Err("Gateway URL must include a valid host or IP address".to_string());
    }

    let authority = raw
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or_default()
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    if !url.username().is_empty() || url.password().is_some() || authority.contains('@') {
        return Err("Gateway URL must not include credentials".to_string());
    }
    if url.query().is_some() {
        return Err("Gateway URL must not include a query string".to_string());
    }
    if url.fragment().is_some() {
        return Err("Gateway URL must not include a fragment".to_string());
    }
    match url.path() {
        "" | "/" | "/v1" | "/v1/" => {}
        _ => return Err("Gateway URL path must be empty or /v1".to_string()),
    }

    url.set_path("/v1");
    Ok(url)
}

// #2775 — cfg(test) stamp mechanism (Tier 1 regression test). `reqwest::Client`
// doesn't expose its TLS config for inspection, so tests can't assert "was this
// the pinned client or the pre-pairing client" by examining the returned value.
// Instead, each of the two client-building functions below stamps which path it
// took into this thread-local. Tests run under `#[tokio::test]`'s default
// current_thread flavor, so the stamp and the assertion always run on the same
// OS thread. This is a marker, not a log-text match — no `eprintln!` coupling.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClientPathStamp {
    /// `build_gateway_client_with_cert` took the `Some(pem) if !pem.is_empty()` arm.
    Pinned,
    /// `build_pre_pairing_client` was called, or `build_gateway_client_with_cert`
    /// took its `_ =>` (no cert) arm.
    PrePairing,
}

#[cfg(test)]
thread_local! {
    static LAST_CLIENT_PATH: std::cell::RefCell<Option<ClientPathStamp>> = std::cell::RefCell::new(None);
}

#[cfg(test)]
pub(crate) fn test_take_last_client_path() -> Option<ClientPathStamp> {
    LAST_CLIENT_PATH.with(|c| c.borrow_mut().take())
}

#[cfg(test)]
fn stamp_client_path(path: ClientPathStamp) {
    LAST_CLIENT_PATH.with(|c| *c.borrow_mut() = Some(path));
}

/// Build a reqwest client that trusts system roots AND the pinned gateway certificate.
/// If no cert is stored (pre-pairing), returns a client with danger_accept_invalid_certs.
/// If cert is stored (post-pairing), uses SPKI pin via PinnedCertVerifier — no hostname check.
pub fn build_gateway_client(db: &DbState) -> Result<reqwest::Client, String> {
    eprintln!("[GATEWAY_CLIENT] Building gateway client...");

    let cert_pem = {
        let conn = db
            .0
            .lock()
            .map_err(|e| format!("Failed to lock database: {}", e))?;
        crate::database::get_ai_setting(&conn, "gateway_tls_cert_pem")
    };

    build_gateway_client_with_cert(cert_pem)
}

/// Build an async reqwest client using a pre-read cert PEM.
/// Use this variant when the caller already holds the DbState lock and has extracted
/// the cert (or is running outside a Tauri command that has one) — mirrors
/// `build_gateway_client_blocking_with_cert`'s deadlock-avoidance rationale (#1350)
/// for the async client.
///
/// Pass `cert_pem = None` (or an empty string) to get the pre-pairing fallback client.
pub fn build_gateway_client_with_cert(cert_pem: Option<String>) -> Result<reqwest::Client, String> {
    match cert_pem {
        Some(pem) if !pem.is_empty() => {
            // Post-pairing: SPKI-pin to the stored certificate (skips hostname/SAN check)
            eprintln!(
                "[GATEWAY_CLIENT] Using SPKI-pinned TLS config ({} bytes)",
                pem.len()
            );
            #[cfg(test)]
            stamp_client_path(ClientPathStamp::Pinned);
            let tls_config = build_pin_only_tls_config(&pem)?;
            reqwest::Client::builder()
                .use_preconfigured_tls(tls_config)
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(std::time::Duration::from_secs(30))
                .timeout(std::time::Duration::from_secs(300))
                .build()
                .map_err(|e| format!("Failed to build pinned gateway client: {}", e))
        }
        _ => {
            // Pre-pairing: accept any cert (fingerprint fetch stage)
            eprintln!("[GATEWAY_CLIENT] No cert stored, using danger_accept_invalid_certs");
            #[cfg(test)]
            stamp_client_path(ClientPathStamp::PrePairing);
            reqwest::Client::builder()
                .danger_accept_invalid_certs(true)
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(std::time::Duration::from_secs(30))
                .timeout(std::time::Duration::from_secs(300))
                .build()
                .map_err(|e| format!("Failed to build gateway client: {}", e))
        }
    }
}

/// Build a pre-pairing client that accepts any cert (for initial fingerprint fetch only)
pub fn build_pre_pairing_client() -> Result<reqwest::Client, String> {
    eprintln!("[GATEWAY_CLIENT] Building pre-pairing client (danger_accept)");
    #[cfg(test)]
    stamp_client_path(ClientPathStamp::PrePairing);
    reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("Failed to build pre-pairing client: {}", e))
}

/// Build a blocking reqwest client that trusts system roots AND the pinned gateway certificate.
/// Used for sync operations that run in blocking contexts.
/// Retained for compatibility with callers that do not already hold the database lock.
#[allow(dead_code)]
pub fn build_gateway_client_blocking(db: &DbState) -> Result<reqwest::blocking::Client, String> {
    eprintln!("[GATEWAY_CLIENT] Building blocking gateway client...");

    let cert_pem = {
        let conn = db
            .0
            .lock()
            .map_err(|e| format!("Failed to lock database: {}", e))?;
        crate::database::get_ai_setting(&conn, "gateway_tls_cert_pem")
    };

    build_gateway_client_blocking_with_cert(cert_pem)
}

/// Build a blocking reqwest client using a pre-read cert PEM.
/// Use this variant when the caller already holds the DbState lock and has extracted
/// the cert — avoids the deadlock that occurs when `build_gateway_client_blocking`
/// tries to re-acquire a lock already held by the same thread. Fixes #1350.
///
/// Pass `cert_pem = None` (or an empty string) to get the pre-pairing fallback client.
pub fn build_gateway_client_blocking_with_cert(
    cert_pem: Option<String>,
) -> Result<reqwest::blocking::Client, String> {
    eprintln!("[GATEWAY_CLIENT] Building blocking gateway client (with_cert variant)...");

    match cert_pem {
        Some(pem) if !pem.is_empty() => {
            // Post-pairing: SPKI-pin to the stored certificate (skips hostname/SAN check)
            eprintln!(
                "[GATEWAY_CLIENT] Using SPKI-pinned TLS config ({} bytes) (blocking)",
                pem.len()
            );
            let tls_config = build_pin_only_tls_config(&pem)?;
            reqwest::blocking::Client::builder()
                .use_preconfigured_tls(tls_config)
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(std::time::Duration::from_secs(5)) // #1312: 30s→5s; gateway is on LAN/Tailscale
                .timeout(std::time::Duration::from_secs(30)) // #1312: 300s→30s; individual sync ops are small
                .build()
                .map_err(|e| format!("Failed to build pinned gateway client (blocking): {}", e))
        }
        _ => {
            // Pre-pairing: accept any cert (shouldn't happen in sync flows but failsafe)
            eprintln!("[GATEWAY_CLIENT] No cert stored, using danger_accept (blocking)");
            reqwest::blocking::Client::builder()
                .danger_accept_invalid_certs(true)
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(std::time::Duration::from_secs(5)) // #1312: 30s→5s; gateway is on LAN/Tailscale
                .timeout(std::time::Duration::from_secs(30)) // #1312: 300s→30s; individual sync ops are small
                .build()
                .map_err(|e| format!("Failed to build gateway client (blocking): {}", e))
        }
    }
}

// #2775: crate-visible so `ai::openrouter`'s Tier 1/3 tests (and any other
// module needing a paired-install fixture cert) can build a real self-signed
// SPKI-pinnable cert without duplicating the rcgen boilerplate. Same cert shape
// as this module's own `make_test_cert_pem` (EC P-256, CN=test-gateway,
// SAN=localhost) — kept as a thin public alias rather than moving the
// generation logic, so this module's own tests are unaffected.
#[cfg(test)]
pub(crate) fn test_make_gateway_cert_pem() -> String {
    use rcgen::{CertificateParams, DistinguishedName, KeyPair};
    let mut params = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(rcgen::DnType::CommonName, "test-gateway");
    params.distinguished_name = dn;
    params.subject_alt_names =
        vec![rcgen::SanType::DnsName(rcgen::Ia5String::try_from("localhost").unwrap())];
    let kp = KeyPair::generate().unwrap();
    params.self_signed(&kp).unwrap().pem()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database;
    use rustls::client::danger::ServerCertVerifier as _;

    // A real rcgen-generated self-signed cert for testing SPKI extraction and pinning.
    // Generated with: rcgen (EC P-256, CN=test-gateway, SAN=localhost).
    fn make_test_cert_pem() -> String {
        super::test_make_gateway_cert_pem()
    }

    #[test]
    fn test_pem_to_der_roundtrip() {
        let pem = make_test_cert_pem();
        let der = pem_to_der(&pem).expect("pem_to_der should succeed");
        assert!(!der.is_empty(), "DER should not be empty");
        // DER starts with SEQUENCE tag 0x30
        assert_eq!(der[0], 0x30, "DER cert must start with SEQUENCE tag");
    }

    #[test]
    fn test_extract_spki_der_succeeds() {
        let pem = make_test_cert_pem();
        let der = pem_to_der(&pem).unwrap();
        let spki = extract_spki_der(&der);
        assert!(spki.is_some(), "extract_spki_der should return Some for a valid cert");
        let spki = spki.unwrap();
        // SPKI is a SEQUENCE
        assert_eq!(spki[0], 0x30, "SPKI must start with SEQUENCE tag");
        // Typical P-256 SPKI is 59 bytes; RSA varies. Just check non-empty and sane size.
        assert!(spki.len() > 10, "SPKI too short");
        eprintln!("[TEST] SPKI len={}, hex={}", spki.len(), hex::encode(&spki[..8]));
    }

    #[test]
    fn test_spki_hash_stable_across_calls() {
        // Same cert → same SPKI hash every time
        let pem = make_test_cert_pem();
        let der = pem_to_der(&pem).unwrap();
        let spki = extract_spki_der(&der).unwrap();

        let hash1: [u8; 32] = {
            let mut h = Sha256::new();
            h.update(&spki);
            h.finalize().into()
        };
        let hash2: [u8; 32] = {
            let mut h = Sha256::new();
            h.update(&spki);
            h.finalize().into()
        };
        assert_eq!(hash1, hash2, "SPKI hash must be deterministic");
    }

    #[test]
    fn test_build_pin_only_tls_config_accepts_valid_cert() {
        let pem = make_test_cert_pem();
        let result = build_pin_only_tls_config(&pem);
        assert!(
            result.is_ok(),
            "build_pin_only_tls_config should succeed: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_build_pin_only_tls_config_rejects_empty_pem() {
        let result = build_pin_only_tls_config("");
        assert!(result.is_err(), "Empty PEM should fail");
    }

    #[test]
    fn test_extract_spki_der_rejects_garbage() {
        let garbage = &[0x00u8, 0x01, 0x02, 0x03, 0x04];
        assert!(
            extract_spki_der(garbage).is_none(),
            "Garbage bytes should return None"
        );
    }

    #[test]
    fn canonicalizes_supported_gateway_urls() {
        let cases = [
            ("https://gateway.example", "https://gateway.example/v1"),
            ("https://gateway.example/v1/", "https://gateway.example/v1"),
            ("https://100.64.0.1:18789", "https://100.64.0.1:18789/v1"),
            ("https://127.0.0.1:8443/v1", "https://127.0.0.1:8443/v1"),
            ("https://[fd7a:115c:a1e0::1]:18789", "https://[fd7a:115c:a1e0::1]:18789/v1"),
        ];
        for (input, expected) in cases {
            assert_eq!(canonicalize_gateway_base_url(input).unwrap().as_str(), expected);
        }
    }

    #[test]
    fn rejects_unsafe_or_unexpected_gateway_urls() {
        let cases = [
            "",
            "gateway.example",
            "http://gateway.example:18789",
            "ftp://gateway.example",
            "https://user@gateway.example",
            "https://user:pass@gateway.example/v1",
            "https://gateway.example?next=https://evil.example",
            "https://gateway.example#reviewed",
            "https://gateway.example/v1/health",
            "https://gateway.example/pair",
            "https://gateway.example/v2",
        ];
        for input in cases {
            assert!(canonicalize_gateway_base_url(input).is_err(), "unsafe gateway URL was accepted: {input}");
        }
    }

    #[test]
    fn test_build_pre_pairing_client() {
        let client = build_pre_pairing_client();
        assert!(client.is_ok());
    }

    #[tokio::test]
    async fn pre_pairing_client_does_not_follow_redirects() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request);
            stream.write_all(
                b"HTTP/1.1 302 Found\r\nLocation: https://example.invalid/escaped\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            ).unwrap();
        });

        let response = build_pre_pairing_client()
            .unwrap()
            .get(format!("http://{address}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::FOUND);
        server.join().unwrap();
    }

    #[test]
    #[serial_test::serial(pane_test_root)]
    fn test_build_gateway_client_no_cert() {
        // #2775 fix-forward: init_database(None) resolves to the real ~/Pane
        // production install unless PANE_TEST_ROOT is set first (see the
        // #2775 PR's incident writeup — this pre-existing test was silently
        // corrupting production ai_settings on every `cargo test` run before
        // this fix). Isolate via a tempdir, same as the other #2775 tests.
        let temp_dir = tempfile::TempDir::new().expect("tempdir");
        std::env::set_var("PANE_TEST_ROOT", temp_dir.path());
        let conn = database::init_database(None).unwrap();
        let db = DbState::new(conn);

        // With no cert, should return danger_accept client
        let client = build_gateway_client(&db);
        assert!(client.is_ok());
    }

    #[test]
    #[serial_test::serial(pane_test_root)]
    fn test_build_gateway_client_with_valid_cert() {
        // #2775 fix-forward: see test_build_gateway_client_no_cert above — same
        // PANE_TEST_ROOT isolation requirement.
        let temp_dir = tempfile::TempDir::new().expect("tempdir");
        std::env::set_var("PANE_TEST_ROOT", temp_dir.path());
        let conn = database::init_database(None).unwrap();
        let pem = make_test_cert_pem();
        database::set_ai_setting(&conn, "gateway_tls_cert_pem", &pem).unwrap();
        let db = DbState::new(conn);

        // Should build a pinned client successfully
        let client = build_gateway_client(&db);
        assert!(
            client.is_ok(),
            "Should build pinned client with valid cert: {:?}",
            client.err()
        );
    }

    #[test]
    fn test_build_gateway_client_blocking_with_cert_valid() {
        let pem = make_test_cert_pem();
        let result = build_gateway_client_blocking_with_cert(Some(pem));
        assert!(
            result.is_ok(),
            "Should build blocking pinned client: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_build_gateway_client_blocking_with_cert_none() {
        let result = build_gateway_client_blocking_with_cert(None);
        assert!(result.is_ok(), "No-cert blocking client should use danger_accept");
    }

    #[test]
    fn test_pinned_cert_verifier_accepts_matching_cert() {
        let pem = make_test_cert_pem();
        let der = pem_to_der(&pem).unwrap();
        let spki = extract_spki_der(&der).unwrap();
        let mut hasher = Sha256::new();
        hasher.update(&spki);
        let pinned_spki_hash: [u8; 32] = hasher.finalize().into();

        let crypto = rustls::crypto::ring::default_provider();
        let supported_algs = crypto.signature_verification_algorithms;
        let verifier = PinnedCertVerifier {
            pinned_spki_hash,
            supported_algs,
        };

        let cert_der = CertificateDer::from(der);
        // Provide a dummy ServerName — it must be ignored by our verifier
        let server_name = ServerName::try_from("totally-wrong-hostname.invalid")
            .expect("valid static hostname");
        let result = verifier.verify_server_cert(
            &cert_der,
            &[],
            &server_name,
            &[],
            UnixTime::now(),
        );
        assert!(
            result.is_ok(),
            "Verifier should accept cert matching pinned SPKI: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_pinned_cert_verifier_rejects_wrong_cert() {
        // Pin cert A, present cert B → should fail
        let pem_a = make_test_cert_pem();
        let der_a = pem_to_der(&pem_a).unwrap();
        let spki_a = extract_spki_der(&der_a).unwrap();
        let mut hasher = Sha256::new();
        hasher.update(&spki_a);
        let pinned_spki_hash: [u8; 32] = hasher.finalize().into();

        let pem_b = make_test_cert_pem(); // different key pair
        let der_b = pem_to_der(&pem_b).unwrap();

        let crypto = rustls::crypto::ring::default_provider();
        let supported_algs = crypto.signature_verification_algorithms;
        let verifier = PinnedCertVerifier {
            pinned_spki_hash,
            supported_algs,
        };

        let cert_der_b = CertificateDer::from(der_b);
        let server_name = ServerName::try_from("localhost").unwrap();
        let result = verifier.verify_server_cert(&cert_der_b, &[], &server_name, &[], UnixTime::now());
        assert!(
            result.is_err(),
            "Verifier should reject cert with different SPKI"
        );
    }
}

// ── #2775 Tier 3: wrong-cert integration test ──────────────────────────────────

/// Real local TLS listener (rcgen + tokio-rustls, not mockito — mockito is plain
/// HTTP only) presenting a deliberately *mismatched* SPKI certificate, proving
/// that the pinned client built by `build_gateway_client_with_cert` actually
/// rejects the handshake instead of silently succeeding via `danger_accept`.
/// A passing (non-error) response here would mean the pinning is a no-op —
/// exactly the class of bug #2775 exists to close.
#[cfg(test)]
mod wrong_cert_integration_tests {
    use super::*;
    use rcgen::{CertificateParams, DistinguishedName, KeyPair};
    use rustls::pki_types::PrivateKeyDer;
    use std::net::SocketAddr;
    use tokio::net::TcpListener;
    use tokio_rustls::TlsAcceptor;

    /// Generates a fresh self-signed (cert, key) pair distinct from whatever
    /// `test_make_gateway_cert_pem()` returns — each call makes a brand new
    /// key pair, so calling it twice already yields two different SPKIs, but
    /// we call it once for "pinned" and once for "server presents" to make the
    /// mismatch explicit and self-documenting at the call site.
    fn generate_cert_and_key() -> (String, CertificateDer<'static>, PrivateKeyDer<'static>) {
        let mut params = CertificateParams::default();
        let mut dn = DistinguishedName::new();
        dn.push(rcgen::DnType::CommonName, "test-gateway");
        params.distinguished_name = dn;
        params.subject_alt_names =
            vec![rcgen::SanType::DnsName(rcgen::Ia5String::try_from("localhost").unwrap())];
        let kp = KeyPair::generate().unwrap();
        let cert = params.self_signed(&kp).unwrap();
        let pem = cert.pem();
        let cert_der = cert.der().clone();
        let key_der = PrivateKeyDer::Pkcs8(kp.serialize_der().into());
        (pem, cert_der, key_der)
    }

    /// Sends a GET request, retrying a handful of times on connection-level
    /// errors (e.g. "connection reset by peer") before giving up. This exists
    /// because `spawn_tls_server` returns as soon as the listening socket is
    /// bound, but the actual `tokio::spawn`'d accept loop task still needs to
    /// be scheduled and polled before it can complete the TLS handshake. On
    /// slower/loaded CI runners (observed on GitHub Actions macos-latest), the
    /// client's connection can land before that task gets its first poll,
    /// which can surface as a reset connection rather than a queued one. This
    /// retry does NOT change what's being verified (the SPKI pinning logic is
    /// still exercised on whichever attempt actually completes the handshake);
    /// it only makes the test robust against scheduling jitter in the test
    /// harness itself.
    async fn send_with_connection_retry(
        client: &reqwest::Client,
        url: &str,
        max_attempts: u32,
    ) -> Result<reqwest::Response, reqwest::Error> {
        let mut last_err = None;
        for attempt in 0..max_attempts {
            match client.get(url).send().await {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    let is_connection_error = e.is_connect()
                        || e.is_request()
                        || format!("{:?}", e).to_lowercase().contains("connection reset");
                    last_err = Some(e);
                    if attempt + 1 < max_attempts && is_connection_error {
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                        continue;
                    }
                    break;
                }
            }
        }
        Err(last_err.unwrap())
    }

    /// Spawns a bare TLS listener on 127.0.0.1 presenting `server_cert`/`server_key`,
    /// accepts exactly one connection, and (if the handshake succeeds) writes a
    /// minimal valid HTTP response before closing. Returns the bound address.
    async fn spawn_tls_server(
        server_cert: CertificateDer<'static>,
        server_key: PrivateKeyDer<'static>,
    ) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_config = rustls::ServerConfig::builder_with_provider(std::sync::Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .expect("protocol versions")
        .with_no_client_auth()
        .with_single_cert(vec![server_cert], server_key)
        .expect("valid self-signed cert/key pair for server config");
        let acceptor = TlsAcceptor::from(std::sync::Arc::new(server_config));

        tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                // The pinned client is expected to abort during the TLS handshake
                // (SPKI mismatch) before ever writing an HTTP request, so this
                // accept_err path is the expected outcome for the mismatched-cert
                // test. If a handshake DOES complete (e.g. a future regression
                // reintroduces danger_accept), reply with a real HTTP response so
                // the calling test can also observe a spurious 200 rather than
                // hanging.
                if let Ok(mut tls_stream) = acceptor.accept(stream).await {
                    use tokio::io::AsyncWriteExt;
                    let _ = tls_stream
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
                        .await;
                }
            }
        });

        addr
    }

    #[tokio::test]
    #[serial_test::serial(pane_test_root)]
    async fn test_pinned_client_rejects_mismatched_spki_cert() {
        // Pinned cert (what's stored in ai_settings, i.e. what pairing captured).
        let (pinned_pem, _pinned_der, _pinned_key) = generate_cert_and_key();
        // A *different* cert the (misconfigured/compromised/MITM) server actually
        // presents at connection time — different key pair, different SPKI.
        let (_server_pem, server_der, server_key) = generate_cert_and_key();

        let addr = spawn_tls_server(server_der, server_key).await;

        // Build the pinned client exactly as select_openrouter_client would for a
        // paired install (db: Some(..) with a stored cert).
        let temp_dir = tempfile::TempDir::new().expect("tempdir");
        std::env::set_var("PANE_TEST_ROOT", temp_dir.path());
        let conn = crate::database::init_database(None).unwrap();
        crate::database::set_ai_setting(&conn, "gateway_tls_cert_pem", &pinned_pem).unwrap();
        let db = DbState::new(conn);
        let client = build_gateway_client(&db).expect("pinned client should build");

        // hostname is irrelevant to SPKI pinning (intentionally ignored by
        // PinnedCertVerifier), but reqwest still needs a URL to dial the socket.
        let url = format!("https://localhost:{}/", addr.port());
        let result = client.get(&url).send().await;

        assert!(
            result.is_err(),
            "Pinned client MUST reject a connection presenting a non-matching SPKI \
             cert with a TLS/handshake error — a successful response here means \
             danger_accept (or an equivalent always-accept path) is silently \
             swallowing the cert mismatch, which is exactly the #2775 bug class."
        );
        let err_string = format!("{:?}", result.unwrap_err());
        assert!(
            err_string.to_lowercase().contains("certificate")
                || err_string.to_lowercase().contains("tls")
                || err_string.to_lowercase().contains("handshake"),
            "Expected a TLS/certificate-shaped error, got: {}",
            err_string
        );
    }

    #[tokio::test]
    #[serial_test::serial(pane_test_root)]
    async fn test_pinned_client_accepts_matching_spki_cert() {
        // Sanity counterpart: same server, but the client pins the cert the
        // server actually presents — the handshake must succeed. Without this,
        // the test above could pass "by accident" (e.g. if the test TLS server
        // itself is broken) rather than because of a genuine SPKI mismatch.
        let (server_pem, server_der, server_key) = generate_cert_and_key();

        let addr = spawn_tls_server(server_der, server_key).await;

        let temp_dir = tempfile::TempDir::new().expect("tempdir");
        std::env::set_var("PANE_TEST_ROOT", temp_dir.path());
        let conn = crate::database::init_database(None).unwrap();
        crate::database::set_ai_setting(&conn, "gateway_tls_cert_pem", &server_pem).unwrap();
        let db = DbState::new(conn);
        let client = build_gateway_client(&db).expect("pinned client should build");

        let url = format!("https://localhost:{}/", addr.port());
        // Retry on connection-level errors: `spawn_tls_server` returns as soon
        // as the listener is bound, but its accept loop runs in a separately
        // scheduled task, so on slow/loaded CI runners the very first connect
        // attempt can race the server task's first poll and get a reset
        // connection rather than a queued one. This does not weaken the SPKI
        // pinning assertion below — it only guards against test-harness
        // scheduling jitter, not against the behavior under test.
        let result = send_with_connection_retry(&client, &url, 3).await;

        assert!(
            result.is_ok(),
            "Pinned client must accept a connection whose presented cert matches \
             the pinned SPKI: {:?}",
            result.err()
        );
    }
}

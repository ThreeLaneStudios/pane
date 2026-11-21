// og_fetch.rs — Open Graph metadata fetcher (#1780)
// SSRF-guarded Rust-side fetch per ADR-0049 Decision 2.
// Never fetched client-side — enforcement point is this module exclusively.
//
// Guards enforced (all required by ADR-0049 §Decision 2):
//   1. Scheme allowlist: http / https only
//   2. SSRF private-IP blocklist (DNS-resolved, not hostname-only)
//   3. Redirect re-validation: each Location hop re-resolved + re-checked
//   4. Redirect cap: ≤3 hops
//   5. Wall-clock timeout: ≤5s
//   6. Body cap: ≤256 KB (streaming byte counter)
//   7. Content-Type gate: text/html only
//   8. No credential forwarding: fresh client, no ambient auth
//
// #2751: guards 1-3 (scheme allowlist, SSRF blocklist, and the pinned-client
// resolve+connect machinery that defeats DNS rebinding) now live in
// `ai::net_guard`, shared with `ai::web`. This module wires them in per hop
// and keeps its own `MAX_REDIRECTS = 3` cap, `USER_AGENT`, and timeouts
// (distinct from `ai::web`'s own `MAX_WEB_REDIRECTS = 5`; do not share).

use crate::ai::net_guard;
use crate::database::DbState;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tauri::State;

/// Maximum number of redirect hops before aborting.
const MAX_REDIRECTS: usize = 3;

/// Hard body cap: only read up to 256 KB of the response body.
const BODY_CAP_BYTES: usize = 256 * 1024;

/// Wall-clock + connect timeout for each individual HTTP request.
const REQUEST_TIMEOUT_SECS: u64 = 5;
const CONNECT_TIMEOUT_SECS: u64 = 5;

/// Cache TTL: 7 days for successful fetches (unix millis).
pub const CACHE_TTL_OK_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// Cache TTL: 1 hour for failed / blocked / no-og fetches.
pub const CACHE_TTL_FAIL_MS: i64 = 60 * 60 * 1000;

/// Generic browser UA for v1 — maximises og tag retrieval from CDN/server-side responses.
/// A Pane-identified UA could allow server-side opt-out in a future iteration.
const USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

// ─── Public types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OgData {
    pub url: String,
    pub og_title: Option<String>,
    pub og_description: Option<String>,
    pub og_image: Option<String>,
    pub status: String, // "ok" | "timeout" | "blocked" | "no_og" | "error"
    pub fetched_at: i64, // unix millis
}

// ─── SSRF guard ──────────────────────────────────────────────────────────────
//
// The blocklist, hostname pre-check, and DNS resolve-and-pin machinery now
// live in `ai::net_guard` (#2751), shared with `ai::web`. `validate_host`
// below wraps `net_guard::hostname_precheck` + `net_guard::resolve_and_pin` to
// produce this module's own diagnostic error text (distinct from `ai::web`'s
// §4.6.7 exact user-visible strings — this module's callers are internal, not
// model-facing).

/// Hostname pre-check + resolve-and-pin, wrapping `net_guard`. Returns the
/// checked addresses on success — the caller passes these into
/// `net_guard::build_pinned_client` for the actual connection (defeats DNS
/// rebinding: the check and the connection use the same resolved addresses).
///
/// In test builds, localhost (127.0.0.1) is allowed so mockito servers can be
/// reached — this carve-out lives inside `net_guard::resolve_and_pin` itself.
/// The SSRF guard is tested independently via `net_guard`'s own unit tests,
/// which always run.
async fn validate_host(host: &str, port: u16) -> Result<Vec<std::net::SocketAddr>, String> {
    net_guard::hostname_precheck(host)
        .map_err(|reason| format!("SSRF block: hostname '{}' is blocked ({})", host, reason))?;

    net_guard::resolve_and_pin(host, port).await.map_err(|reason| {
        if reason.starts_with("dns_failure") {
            format!("DNS resolution failed for {}: {}", host, reason)
        } else {
            format!(
                "SSRF block: {} resolved to a blocked IP (private/loopback/link-local/reserved)",
                host
            )
        }
    })
}

// ─── URL validation ──────────────────────────────────────────────────────────
//
// The scheme allowlist itself now lives in `net_guard::parse_and_check_scheme`
// (shared with `ai::web`). This wrapper preserves this module's own
// diagnostic error text and host/port extraction.

/// Parse and validate a URL: scheme must be http or https; host and port extracted for SSRF check.
/// Returns (url::Url, host, port) on success.
fn parse_and_validate_url(raw_url: &str) -> Result<(url::Url, String, u16), String> {
    let parsed = match url::Url::parse(raw_url) {
        Ok(u) => u,
        Err(e) => return Err(format!("URL parse error: {}", e)),
    };

    if net_guard::parse_and_check_scheme(raw_url).is_err() {
        return Err(format!("Blocked scheme '{}' — only http/https allowed", parsed.scheme()));
    }

    let host = parsed
        .host_str()
        .ok_or_else(|| "URL has no host".to_string())?
        .to_string();

    let port = parsed.port_or_known_default().ok_or_else(|| {
        format!("Cannot determine port for scheme '{}'", parsed.scheme())
    })?;

    Ok((parsed, host, port))
}

// ─── og: tag parser ──────────────────────────────────────────────────────────

/// Extract og:title, og:description, og:image from raw HTML bytes.
/// Reads only up to the </head> tag (or end of buffer) for efficiency.
/// Returns None for each field that is missing or empty.
fn parse_og_tags(html: &[u8]) -> (Option<String>, Option<String>, Option<String>) {
    // Work with the bytes as a lossy UTF-8 string (og tags are ASCII-safe).
    let text = String::from_utf8_lossy(html);

    // Limit to <head> region for performance.
    let head_end = text
        .to_lowercase()
        .find("</head>")
        .unwrap_or(text.len());
    let head = &text[..head_end];

    let og_title = extract_og_property(head, "og:title");
    let og_description = extract_og_property(head, "og:description");
    let og_image = extract_og_property(head, "og:image");

    (og_title, og_description, og_image)
}

/// Extract the `content` attribute of the first `<meta property="{prop}" ...>` tag.
fn extract_og_property(html: &str, prop: &str) -> Option<String> {
    let html_lower = html.to_lowercase();
    let prop_lower = prop.to_lowercase();

    // Find all occurrences of "og:title" (or whatever prop) in the head.
    let mut search_from = 0;
    while let Some(prop_pos) = html_lower[search_from..].find(&prop_lower) {
        let abs_prop_pos = search_from + prop_pos;

        // Walk backwards to find the opening `<meta` for this tag.
        let tag_start = html_lower[..abs_prop_pos].rfind("<meta")?;
        // Walk forward to find the closing `>`.
        let tag_end = html_lower[abs_prop_pos..].find('>').map(|p| abs_prop_pos + p + 1)?;
        let tag = &html[tag_start..tag_end];

        // Verify this is a property= attribute (not e.g. name= in a twitter:card tag that
        // happens to contain "og:title" elsewhere).
        let tag_lower = tag.to_lowercase();
        if !tag_lower.contains("property=") {
            search_from = abs_prop_pos + prop_lower.len();
            continue;
        }

        // Extract content= attribute value.
        if let Some(content) = extract_attr_value(tag, "content") {
            let trimmed = content.trim().to_string();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }

        search_from = abs_prop_pos + prop_lower.len();
    }

    None
}

/// Extract the value of `attr="..."` or `attr='...'` from a tag string.
fn extract_attr_value(tag: &str, attr: &str) -> Option<String> {
    let lower = tag.to_lowercase();
    let attr_lower = attr.to_lowercase();
    let needle = format!("{}=", attr_lower);
    let pos = lower.find(&needle)?;
    let rest = &tag[pos + needle.len()..];
    let rest = rest.trim_start();

    if rest.starts_with('"') {
        let end = rest[1..].find('"')?;
        Some(html_decode(&rest[1..end + 1]))
    } else if rest.starts_with('\'') {
        let end = rest[1..].find('\'')?;
        Some(html_decode(&rest[1..end + 1]))
    } else {
        // Unquoted attribute value — take until whitespace or >
        let end = rest.find(|c: char| c.is_whitespace() || c == '>').unwrap_or(rest.len());
        Some(html_decode(&rest[..end]))
    }
}

/// Minimal HTML entity decode for og: content values.
fn html_decode(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
}

// ─── Core fetch logic ─────────────────────────────────────────────────────────
//
// #2751: og_fetch now builds a fresh *pinned* client (via
// `net_guard::build_pinned_client`) for every hop, not just the first
// request. Previously a single non-pinned client was built once outside the
// redirect loop and reused for every hop — SSRF re-validation happened
// (`validate_host` was already called per hop), but the connection itself was
// not pinned to the re-checked addresses, leaving a DNS-rebinding gap between
// the per-hop check and the per-hop connect. Every hop below now pins its own
// client to the addresses `validate_host` just checked, closing that gap (no
// cookie store — fresh client, no ambient credentials per ADR-0049;
// cookie_store feature not enabled in Cargo.toml — defaults to disabled).

/// Fetch a URL with all SSRF guards applied. Returns OgData on any outcome.
/// This is the single enforcement point for ADR-0049 Decision 2.
pub async fn fetch_og(url: &str) -> OgData {
    eprintln!("[OG-FETCH] Starting fetch: url={}", url);

    let now_ms = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64
    };

    let result = fetch_og_inner(url).await;

    match result {
        Ok(data) => {
            eprintln!("[OG-FETCH] url={} result={}", url, data.status);
            data
        }
        Err(e) => {
            eprintln!("[OG-FETCH] url={} error: {}", url, e);
            OgData {
                url: url.to_string(),
                og_title: None,
                og_description: None,
                og_image: None,
                status: "error".to_string(),
                fetched_at: now_ms(),
            }
        }
    }
}

async fn fetch_og_inner(initial_url: &str) -> Result<OgData, String> {
    let now_ms = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64
    };

    let mut current_url = initial_url.to_string();
    let mut hops = 0usize;

    loop {
        // 1. Parse and scheme-check the current URL.
        let (parsed, host, port) = parse_and_validate_url(&current_url).map_err(|e| {
            format!("URL validation failed: {}", e)
        })?;

        // 2. DNS-resolve + SSRF blocklist check (pre-connect — before any HTTP
        // request), returning the checked addresses this hop pins to.
        let addrs = validate_host(&host, port).await.map_err(|e| {
            format!("SSRF guard: {}", e)
        })?;

        // 2b. Build a fresh client pinned to the addresses just checked above —
        // one per hop, so the DNS-rebinding gap between check and connect never
        // reopens on a redirect (#2751).
        let client = net_guard::build_pinned_client(
            &host,
            &addrs,
            USER_AGENT,
            Duration::from_secs(CONNECT_TIMEOUT_SECS),
            Duration::from_secs(REQUEST_TIMEOUT_SECS),
        )?;

        // 3. Issue the HTTP request (no redirects — manual redirect follow).
        let response = client
            .get(parsed.as_str())
            .send()
            .await
            .map_err(|e| format!("HTTP request failed: {}", e))?;

        let status_code = response.status();

        // 4. Handle 3xx redirect manually.
        if status_code.is_redirection() {
            hops += 1;
            if hops > MAX_REDIRECTS {
                return Err(format!(
                    "Redirect cap exceeded ({} hops) for {}",
                    MAX_REDIRECTS, initial_url
                ));
            }

            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .ok_or_else(|| "Redirect response has no Location header".to_string())?
                .to_str()
                .map_err(|_| "Location header is not valid UTF-8".to_string())?
                .to_string();

            // Resolve relative Location URLs against the current base.
            let next_url = if location.starts_with("http://") || location.starts_with("https://") {
                location
            } else {
                // Relative redirect — resolve against current URL.
                parsed
                    .join(&location)
                    .map_err(|e| format!("Cannot resolve relative redirect '{}': {}", location, e))?
                    .to_string()
            };

            eprintln!(
                "[OG-FETCH] Redirect {}/{}: {} -> {}",
                hops, MAX_REDIRECTS, current_url, next_url
            );
            current_url = next_url;
            continue;
        }

        // 5. Non-redirect, non-2xx response.
        if !status_code.is_success() {
            return Ok(OgData {
                url: initial_url.to_string(),
                og_title: None,
                og_description: None,
                og_image: None,
                status: "error".to_string(),
                fetched_at: now_ms(),
            });
        }

        // 6. Content-Type check: only parse text/html.
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_lowercase();

        if !content_type.contains("text/html") {
            eprintln!("[OG-FETCH] Non-HTML content-type '{}' — skipping parse", content_type);
            return Ok(OgData {
                url: initial_url.to_string(),
                og_title: None,
                og_description: None,
                og_image: None,
                status: "no_og".to_string(),
                fetched_at: now_ms(),
            });
        }

        // 7. Stream body up to BODY_CAP_BYTES.
        let mut body: Vec<u8> = Vec::with_capacity(BODY_CAP_BYTES.min(64 * 1024));
        let stream = response;
        use futures_util::StreamExt;
        let mut byte_count = 0usize;

        let mut stream_body = stream.bytes_stream();
        while let Some(chunk) = stream_body.next().await {
            let chunk = chunk.map_err(|e| format!("Error reading response body: {}", e))?;
            let remaining = BODY_CAP_BYTES.saturating_sub(byte_count);
            if remaining == 0 {
                eprintln!("[OG-FETCH] Body cap reached ({} bytes) — stopping read", BODY_CAP_BYTES);
                break;
            }
            let to_take = chunk.len().min(remaining);
            body.extend_from_slice(&chunk[..to_take]);
            byte_count += to_take;

            // Early exit optimisation: if we've already passed </head>, stop reading.
            if body.windows(7).any(|w| w.eq_ignore_ascii_case(b"</head>")) {
                break;
            }
        }

        // 8. Parse og: tags.
        let (og_title, og_description, og_image) = parse_og_tags(&body);

        let has_og = og_title.is_some() || og_description.is_some() || og_image.is_some();
        let status = if has_og { "ok" } else { "no_og" }.to_string();

        return Ok(OgData {
            url: initial_url.to_string(),
            og_title,
            og_description,
            og_image,
            status,
            fetched_at: now_ms(),
        });
    }
}

// ─── Cache layer ──────────────────────────────────────────────────────────────

/// Look up og_cache for a URL. Returns cached OgData if fresh (within TTL).
pub fn get_cached_og(conn: &rusqlite::Connection, url: &str) -> Option<OgData> {
    let result: rusqlite::Result<(Option<String>, Option<String>, Option<String>, String, i64)> =
        conn.query_row(
            "SELECT og_title, og_description, og_image, status, fetched_at FROM og_cache WHERE url = ?1",
            params![url],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        );

    match result {
        Ok((og_title, og_description, og_image, status, fetched_at)) => {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64;

            let ttl = if status == "ok" {
                CACHE_TTL_OK_MS
            } else {
                CACHE_TTL_FAIL_MS
            };

            if now_ms - fetched_at < ttl {
                Some(OgData {
                    url: url.to_string(),
                    og_title,
                    og_description,
                    og_image,
                    status,
                    fetched_at,
                })
            } else {
                None // stale
            }
        }
        Err(_) => None,
    }
}

/// Upsert OgData into og_cache.
pub fn upsert_og_cache(conn: &rusqlite::Connection, data: &OgData) -> Result<(), String> {
    conn.execute(
        "INSERT OR REPLACE INTO og_cache (url, og_title, og_description, og_image, status, fetched_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            data.url,
            data.og_title,
            data.og_description,
            data.og_image,
            data.status,
            data.fetched_at,
        ],
    )
    .map_err(|e| format!("og_cache upsert failed: {}", e))?;
    Ok(())
}

// ─── Tauri command ────────────────────────────────────────────────────────────

/// Tauri command: fetch og: metadata for a link URL.
/// Returns cached OgData if fresh; fetches and caches otherwise.
/// Called fire-and-forget from Editor.svelte after link insert.
///
/// Setting gate: `link_previews_enabled` (ai_settings). When 'false', returns null.
#[tauri::command]
pub async fn fetch_link_preview(url: String, db: State<'_, DbState>) -> Result<Option<OgData>, String> {
    eprintln!("[fetch_link_preview] called with: url={}", url);

    // Check setting gate: link_previews_enabled
    {
        let conn = db.0.lock().map_err(|e| format!("DB lock failed: {}", e))?;
        let enabled = crate::database::get_ai_setting(&conn, "link_previews_enabled")
            .map(|v| v != "false")
            .unwrap_or(true); // null → true (default enabled)

        if !enabled {
            eprintln!("[fetch_link_preview] link_previews_enabled=false — skipping fetch");
            return Ok(None);
        }

        // Check cache
        if let Some(cached) = get_cached_og(&conn, &url) {
            eprintln!("[fetch_link_preview] Cache hit for url={} status={}", url, cached.status);
            return Ok(Some(cached));
        }
    }

    // Cache miss — fetch (outside the lock to avoid holding it during network I/O).
    let data = fetch_og(&url).await;

    // Write to cache
    {
        let conn = db.0.lock().map_err(|e| format!("DB lock failed: {}", e))?;
        if let Err(e) = upsert_og_cache(&conn, &data) {
            eprintln!("[fetch_link_preview] Cache write failed: {}", e);
        }
    }

    Ok(Some(data))
}

/// Tauri command: look up cached og data without triggering a fetch.
/// Used by AttachmentNodeView on card render to show persisted og data immediately.
#[tauri::command]
pub fn get_cached_link_preview(url: String, db: State<'_, DbState>) -> Result<Option<OgData>, String> {
    eprintln!("[get_cached_link_preview] called with: url={}", url);
    let conn = db.0.lock().map_err(|e| format!("DB lock failed: {}", e))?;
    Ok(get_cached_og(&conn, &url))
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    // #2751: the blocklist itself (`is_blocked_ip`) and `parse_and_validate_url`'s
    // underlying scheme check now live in `net_guard`, shared with `ai::web`.
    // Importing here keeps every existing test below unchanged (same names,
    // same assertions) while the implementation they cover was extracted.
    use net_guard::is_blocked_ip;
    use std::net::IpAddr;

    // ── SSRF / IP blocklist ───────────────────────────────────────────────────

    #[test]
    fn test_ssrf_loopback_127() {
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        assert!(is_blocked_ip(&ip), "127.0.0.1 must be blocked (loopback)");
    }

    // #2751: 0.0.0.0/8 and 100.64.0.0/10 were missing from og_fetch's own
    // (pre-extraction) blocklist. Both are now covered via the shared
    // `net_guard` blocklist this module consumes.
    #[test]
    fn test_ssrf_0_0_0_0_8() {
        let ip: IpAddr = "0.5.5.5".parse().unwrap();
        assert!(is_blocked_ip(&ip), "0.0.0.0/8 must be blocked (previously missing from og_fetch)");
    }

    #[test]
    fn test_ssrf_100_64_0_0_10_cgnat_tailscale() {
        let ip: IpAddr = "100.64.0.1".parse().unwrap();
        assert!(is_blocked_ip(&ip), "100.64.0.0/10 must be blocked (previously missing from og_fetch)");
        let ip2: IpAddr = "100.127.255.255".parse().unwrap();
        assert!(is_blocked_ip(&ip2), "100.64.0.0/10 upper bound must be blocked");
        let ip3: IpAddr = "100.128.0.0".parse().unwrap();
        assert!(!is_blocked_ip(&ip3), "100.128.0.0 is outside 100.64.0.0/10 — must NOT be blocked");
    }

    #[test]
    fn test_ssrf_loopback_127_other() {
        let ip: IpAddr = "127.100.200.1".parse().unwrap();
        assert!(is_blocked_ip(&ip), "127.100.200.1 must be blocked (127.0.0.0/8)");
    }

    #[test]
    fn test_ssrf_rfc1918_10x() {
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        assert!(is_blocked_ip(&ip), "10.0.0.1 must be blocked (10/8)");
    }

    #[test]
    fn test_ssrf_rfc1918_10x_high() {
        let ip: IpAddr = "10.255.255.255".parse().unwrap();
        assert!(is_blocked_ip(&ip), "10.255.255.255 must be blocked (10/8)");
    }

    #[test]
    fn test_ssrf_rfc1918_172_16() {
        let ip: IpAddr = "172.16.0.1".parse().unwrap();
        assert!(is_blocked_ip(&ip), "172.16.0.1 must be blocked (172.16/12)");
    }

    #[test]
    fn test_ssrf_rfc1918_172_31() {
        let ip: IpAddr = "172.31.255.255".parse().unwrap();
        assert!(is_blocked_ip(&ip), "172.31.255.255 must be blocked (172.16/12)");
    }

    #[test]
    fn test_ssrf_rfc1918_172_not_blocked() {
        // 172.32.0.0 is NOT in 172.16/12 — should be allowed.
        let ip: IpAddr = "172.32.0.1".parse().unwrap();
        assert!(!is_blocked_ip(&ip), "172.32.0.1 should NOT be blocked");
    }

    #[test]
    fn test_ssrf_rfc1918_192_168() {
        let ip: IpAddr = "192.168.1.1".parse().unwrap();
        assert!(is_blocked_ip(&ip), "192.168.1.1 must be blocked (192.168/16)");
    }

    #[test]
    fn test_ssrf_link_local_169_254() {
        let ip: IpAddr = "169.254.1.1".parse().unwrap();
        assert!(is_blocked_ip(&ip), "169.254.1.1 must be blocked (link-local)");
    }

    #[test]
    fn test_ssrf_cloud_metadata_169_254_169_254() {
        let ip: IpAddr = "169.254.169.254".parse().unwrap();
        assert!(is_blocked_ip(&ip), "169.254.169.254 must be blocked (cloud metadata)");
    }

    #[test]
    fn test_ssrf_ipv6_loopback() {
        let ip: IpAddr = "::1".parse().unwrap();
        assert!(is_blocked_ip(&ip), "::1 must be blocked (IPv6 loopback)");
    }

    #[test]
    fn test_ssrf_ipv6_link_local_fe80() {
        let ip: IpAddr = "fe80::1".parse().unwrap();
        assert!(is_blocked_ip(&ip), "fe80::1 must be blocked (link-local fe80::/10)");
    }

    #[test]
    fn test_ssrf_ipv6_link_local_febf() {
        // fe80::/10 spans fe80:: to febf:: 
        let ip: IpAddr = "febf::1".parse().unwrap();
        assert!(is_blocked_ip(&ip), "febf::1 must be blocked (link-local fe80::/10)");
    }

    #[test]
    fn test_ssrf_ipv6_unique_local_fc00() {
        let ip: IpAddr = "fc00::1".parse().unwrap();
        assert!(is_blocked_ip(&ip), "fc00::1 must be blocked (unique-local fc00::/7)");
    }

    #[test]
    fn test_ssrf_ipv6_unique_local_fd00() {
        let ip: IpAddr = "fd00::1".parse().unwrap();
        assert!(is_blocked_ip(&ip), "fd00::1 must be blocked (unique-local fc00::/7)");
    }

    #[test]
    fn test_ssrf_ipv4_mapped_ipv6_loopback() {
        // ::ffff:127.0.0.1 — IPv4-mapped loopback must be blocked
        let ip: IpAddr = "::ffff:127.0.0.1".parse().unwrap();
        assert!(is_blocked_ip(&ip), "::ffff:127.0.0.1 must be blocked (IPv4-mapped loopback)");
    }

    #[test]
    fn test_ssrf_ipv4_mapped_ipv6_private() {
        // ::ffff:192.168.1.1 — IPv4-mapped private must be blocked
        let ip: IpAddr = "::ffff:192.168.1.1".parse().unwrap();
        assert!(is_blocked_ip(&ip), "::ffff:192.168.1.1 must be blocked (IPv4-mapped private)");
    }

    #[test]
    fn test_ssrf_ipv6_multicast() {
        let ip: IpAddr = "ff02::1".parse().unwrap();
        assert!(is_blocked_ip(&ip), "ff02::1 must be blocked (IPv6 multicast)");
    }

    #[test]
    fn test_ssrf_multicast_v4() {
        let ip: IpAddr = "224.0.0.1".parse().unwrap();
        assert!(is_blocked_ip(&ip), "224.0.0.1 must be blocked (multicast)");
    }

    #[test]
    fn test_ssrf_broadcast() {
        let ip: IpAddr = "255.255.255.255".parse().unwrap();
        assert!(is_blocked_ip(&ip), "255.255.255.255 must be blocked (broadcast)");
    }

    #[test]
    fn test_ssrf_public_ip_allowed() {
        let ip: IpAddr = "8.8.8.8".parse().unwrap();
        assert!(!is_blocked_ip(&ip), "8.8.8.8 should NOT be blocked (public IP)");
    }

    #[test]
    fn test_ssrf_public_ipv6_allowed() {
        let ip: IpAddr = "2001:4860:4860::8888".parse().unwrap();
        assert!(!is_blocked_ip(&ip), "Google DNS IPv6 should NOT be blocked");
    }

    // ── Scheme allowlist ──────────────────────────────────────────────────────

    #[test]
    fn test_scheme_http_allowed() {
        let result = parse_and_validate_url("http://example.com/page");
        assert!(result.is_ok(), "http:// should be allowed");
    }

    #[test]
    fn test_scheme_https_allowed() {
        let result = parse_and_validate_url("https://example.com/page");
        assert!(result.is_ok(), "https:// should be allowed");
    }

    #[test]
    fn test_scheme_file_blocked() {
        let result = parse_and_validate_url("file:///etc/passwd");
        assert!(result.is_err(), "file:// must be rejected");
        assert!(result.unwrap_err().contains("Blocked scheme"), "error should mention blocked scheme");
    }

    #[test]
    fn test_scheme_ftp_blocked() {
        let result = parse_and_validate_url("ftp://ftp.example.com/file");
        assert!(result.is_err(), "ftp:// must be rejected");
    }

    #[test]
    fn test_scheme_data_blocked() {
        let result = parse_and_validate_url("data:text/html,<h1>test</h1>");
        assert!(result.is_err(), "data: must be rejected");
    }

    #[test]
    fn test_scheme_javascript_blocked() {
        let result = parse_and_validate_url("javascript:alert(1)");
        assert!(result.is_err(), "javascript: must be rejected");
    }

    // ── og: tag parsing ───────────────────────────────────────────────────────

    #[test]
    fn test_parse_og_tags_all_present() {
        let html = br#"<!DOCTYPE html>
<html>
<head>
  <meta property="og:title" content="Test Article Title" />
  <meta property="og:description" content="A short description of the article." />
  <meta property="og:image" content="https://example.com/img/cover.jpg" />
</head>
<body></body>
</html>"#;

        let (title, desc, image) = parse_og_tags(html);
        assert_eq!(title, Some("Test Article Title".to_string()));
        assert_eq!(desc, Some("A short description of the article.".to_string()));
        assert_eq!(image, Some("https://example.com/img/cover.jpg".to_string()));
    }

    #[test]
    fn test_parse_og_tags_missing_returns_none() {
        let html = br#"<!DOCTYPE html>
<html>
<head>
  <title>No OG tags here</title>
</head>
<body><p>Content</p></body>
</html>"#;

        let (title, desc, image) = parse_og_tags(html);
        assert!(title.is_none(), "og:title should be None when missing");
        assert!(desc.is_none(), "og:description should be None when missing");
        assert!(image.is_none(), "og:image should be None when missing");
    }

    #[test]
    fn test_parse_og_tags_partial() {
        let html = br#"<head>
  <meta property="og:title" content="Just a Title" />
</head>"#;

        let (title, desc, image) = parse_og_tags(html);
        assert_eq!(title, Some("Just a Title".to_string()));
        assert!(desc.is_none(), "og:description should be None");
        assert!(image.is_none(), "og:image should be None");
    }

    #[test]
    fn test_parse_og_tags_html_entities_decoded() {
        let html = br#"<head>
  <meta property="og:title" content="AT&amp;T &quot;Wireless&quot; &lt;5G&gt;" />
</head>"#;

        let (title, _, _) = parse_og_tags(html);
        assert_eq!(title, Some(r#"AT&T "Wireless" <5G>"#.to_string()));
    }

    #[test]
    fn test_parse_og_tags_malformed_html_no_panic() {
        // Intentionally malformed HTML — must not panic.
        let html = b"<head><meta property=\"og:title\" content=";
        let result = std::panic::catch_unwind(|| parse_og_tags(html));
        assert!(result.is_ok(), "parse_og_tags must not panic on malformed HTML");
    }

    #[test]
    fn test_parse_og_tags_empty_content_is_none() {
        // Empty content= attribute should return None (as if missing).
        let html = br#"<head>
  <meta property="og:title" content="" />
  <meta property="og:description" content="   " />
</head>"#;

        // Empty string → None; whitespace-only also collapses to None via trim().is_empty()
        let (title, desc, _) = parse_og_tags(html);
        assert!(title.is_none(), "Empty og:title content should return None");
        assert!(desc.is_none(), "Whitespace-only og:description should return None");
    }

    #[test]
    fn test_parse_og_tags_single_quoted_attributes() {
        let html = br#"<head>
  <meta property='og:title' content='Single Quote Title' />
</head>"#;
        let (title, _, _) = parse_og_tags(html);
        assert_eq!(title, Some("Single Quote Title".to_string()));
    }

    // ── Redirect cap ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_redirect_cap_exceeded() {
        let mut server = mockito::Server::new_async().await;
        let base = server.url();

        // Create a redirect chain: /r0 → /r1 → /r2 → /r3 → /r4 (5 hops — over cap of 3).
        let _m4 = server.mock("GET", "/r4")
            .with_status(200)
            .with_header("content-type", "text/html")
            .with_body("<head></head>")
            .create_async().await;
        let _m3 = server.mock("GET", "/r3")
            .with_status(302)
            .with_header("location", &format!("{}/r4", base))
            .create_async().await;
        let _m2 = server.mock("GET", "/r2")
            .with_status(302)
            .with_header("location", &format!("{}/r3", base))
            .create_async().await;
        let _m1 = server.mock("GET", "/r1")
            .with_status(302)
            .with_header("location", &format!("{}/r2", base))
            .create_async().await;
        let _m0 = server.mock("GET", "/r0")
            .with_status(302)
            .with_header("location", &format!("{}/r1", base))
            .create_async().await;

        let start_url = format!("{}/r0", base);
        let result = fetch_og_inner(&start_url).await;
        assert!(result.is_err(), "Should error when redirect cap exceeded");
        assert!(result.unwrap_err().contains("Redirect cap"), "Error should mention redirect cap");
    }

    #[tokio::test]
    async fn test_redirect_within_cap_succeeds() {
        let mut server = mockito::Server::new_async().await;
        let base = server.url();

        // 2 hops — within cap of 3.
        let _m2 = server.mock("GET", "/final")
            .with_status(200)
            .with_header("content-type", "text/html")
            .with_body(r#"<head><meta property="og:title" content="Found" /></head>"#)
            .create_async().await;
        let _m1 = server.mock("GET", "/hop2")
            .with_status(302)
            .with_header("location", &format!("{}/final", base))
            .create_async().await;
        let _m0 = server.mock("GET", "/hop1")
            .with_status(302)
            .with_header("location", &format!("{}/hop2", base))
            .create_async().await;

        let start_url = format!("{}/hop1", base);
        let result = fetch_og_inner(&start_url).await;
        assert!(result.is_ok(), "2 hops should succeed: {:?}", result);
        let data = result.unwrap();
        assert_eq!(data.og_title, Some("Found".to_string()));
    }

    // ── SSRF redirect re-validation ─────────────────────────────────────────

    // Test: redirect chain re-validates SSRF on each hop (ADR-0049 §Risks).
    // Mock server returns 302 Location: http://192.168.1.1/admin after a legit
    // public-IP first hop (mockito on localhost). The fetch must reject on
    // redirect re-validation, NOT blindly follow to the private IP.
    #[tokio::test]
    async fn test_redirect_revalidates_ssrf_on_location_hop() {
        let mut server = mockito::Server::new_async().await;

        // Serve a 302 redirect whose Location points to an RFC-1918 private address.
        // The mockito server is on localhost (allowed in test mode).
        // The redirect target 192.168.1.1 is private and must be blocked.
        let _m = server.mock("GET", "/public-page")
            .with_status(302)
            .with_header("location", "http://192.168.1.1/admin")
            .create_async().await;

        let start_url = format!("{}/public-page", server.url());
        let result = fetch_og_inner(&start_url).await;

        // The fetch must fail — not succeed with the private-IP body.
        assert!(result.is_err(), "Redirect to private IP must be rejected, not followed");

        let err = result.unwrap_err();
        // Error must be specific to SSRF / blocked IP, not a generic network failure.
        assert!(
            err.contains("SSRF") || err.contains("blocked IP") || err.contains("private"),
            "Error must indicate SSRF / private-range block, got: {:?}",
            err
        );
    }

    // #2751: same shape as `test_redirect_revalidates_ssrf_on_location_hop`
    // above, but with a 10.0.0.0/8 target (task-spec explicit case) instead of
    // 192.168.0.0/16, and asserting the sentinel redirect body never leaks
    // into a successful result.
    #[tokio::test]
    async fn test_redirect_to_10_x_private_address_rejected() {
        let mut server = mockito::Server::new_async().await;

        let _m = server.mock("GET", "/redirect-to-private")
            .with_status(302)
            .with_header("location", "http://10.1.2.3/internal-admin")
            .with_body("SENTINEL_PRIVATE_BODY_SHOULD_NEVER_APPEAR")
            .create_async().await;

        let start_url = format!("{}/redirect-to-private", server.url());
        let result = fetch_og_inner(&start_url).await;

        assert!(result.is_err(), "redirect to a 10.x.x.x address must be rejected, not followed");
        let err = result.unwrap_err();
        assert!(
            err.contains("SSRF") || err.contains("blocked IP") || err.contains("private"),
            "expected SSRF / private-range block text, got: {:?}",
            err
        );
    }

    // #2751: og_fetch's own blocklist previously omitted 0.0.0.0/8 and
    // 100.64.0.0/10. These integration tests exercise the real `fetch_og`
    // entry point end-to-end against those two ranges via a redirect, mirroring
    // `test_redirect_revalidates_ssrf_on_location_hop`'s shape.
    #[tokio::test]
    async fn test_og_fetch_rejects_0_0_0_0_8_address() {
        let mut server = mockito::Server::new_async().await;

        let _m = server.mock("GET", "/redirect-to-0000")
            .with_status(302)
            .with_header("location", "http://0.5.5.5/x")
            .create_async().await;

        let start_url = format!("{}/redirect-to-0000", server.url());
        let result = fetch_og_inner(&start_url).await;

        assert!(result.is_err(), "redirect to a 0.0.0.0/8 address must be rejected");
        let err = result.unwrap_err();
        assert!(
            err.contains("SSRF") || err.contains("blocked IP"),
            "expected SSRF / blocked IP text, got: {:?}",
            err
        );
    }

    #[tokio::test]
    async fn test_og_fetch_rejects_100_64_0_0_10_address() {
        let mut server = mockito::Server::new_async().await;

        let _m = server.mock("GET", "/redirect-to-cgnat")
            .with_status(302)
            .with_header("location", "http://100.64.0.1/x")
            .create_async().await;

        let start_url = format!("{}/redirect-to-cgnat", server.url());
        let result = fetch_og_inner(&start_url).await;

        assert!(result.is_err(), "redirect to a 100.64.0.0/10 (CGNAT/Tailscale) address must be rejected");
        let err = result.unwrap_err();
        assert!(
            err.contains("SSRF") || err.contains("blocked IP"),
            "expected SSRF / blocked IP text, got: {:?}",
            err
        );
    }

    // ── Content-Type gate ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_non_html_content_type_returns_no_og() {
        let mut server = mockito::Server::new_async().await;

        let _m = server.mock("GET", "/image.png")
            .with_status(200)
            .with_header("content-type", "image/png")
            .with_body(b"\x89PNG\r\n")
            .create_async().await;

        let url = format!("{}/image.png", server.url());
        let result = fetch_og_inner(&url).await;
        assert!(result.is_ok());
        let data = result.unwrap();
        assert_eq!(data.status, "no_og", "Non-HTML should return no_og status");
        assert!(data.og_title.is_none());
    }

    // ── Body cap ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_body_cap_does_not_oom() {
        let mut server = mockito::Server::new_async().await;

        // 300 KB body — larger than 256 KB cap.
        let large_body = vec![b'a'; 300 * 1024];
        let _m = server.mock("GET", "/large")
            .with_status(200)
            .with_header("content-type", "text/html")
            .with_body(large_body)
            .create_async().await;

        let url = format!("{}/large", server.url());
        // Should complete without OOM or panic; returns no_og (no og tags in 'a'*300K).
        let result = fetch_og_inner(&url).await;
        assert!(result.is_ok(), "300KB body should not OOM: {:?}", result);
    }

    // ── Happy path with og: tags ──────────────────────────────────────────────

    #[tokio::test]
    async fn test_fetch_og_success() {
        let mut server = mockito::Server::new_async().await;

        let body = r#"<!DOCTYPE html>
<html>
<head>
  <meta property="og:title" content="Pane Blog Post" />
  <meta property="og:description" content="A great article about Pane." />
  <meta property="og:image" content="https://pane.app/og.jpg" />
</head>
<body></body>
</html>"#;

        let _m = server.mock("GET", "/article")
            .with_status(200)
            .with_header("content-type", "text/html; charset=utf-8")
            .with_body(body)
            .create_async().await;

        let url = format!("{}/article", server.url());
        let result = fetch_og_inner(&url).await;
        assert!(result.is_ok());
        let data = result.unwrap();
        assert_eq!(data.status, "ok");
        assert_eq!(data.og_title, Some("Pane Blog Post".to_string()));
        assert_eq!(data.og_description, Some("A great article about Pane.".to_string()));
        assert_eq!(data.og_image, Some("https://pane.app/og.jpg".to_string()));
    }

    #[tokio::test]
    async fn test_fetch_og_no_tags_returns_no_og_status() {
        let mut server = mockito::Server::new_async().await;

        let _m = server.mock("GET", "/plain")
            .with_status(200)
            .with_header("content-type", "text/html")
            .with_body("<head><title>No OG</title></head>")
            .create_async().await;

        let url = format!("{}/plain", server.url());
        let result = fetch_og_inner(&url).await;
        assert!(result.is_ok());
        let data = result.unwrap();
        assert_eq!(data.status, "no_og");
        assert!(data.og_title.is_none());
    }

    // ── Cache layer ───────────────────────────────────────────────────────────

    #[test]
    fn test_cache_fresh_hit() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("
            CREATE TABLE og_cache (
                url TEXT PRIMARY KEY,
                og_title TEXT,
                og_description TEXT,
                og_image TEXT,
                status TEXT NOT NULL DEFAULT 'ok',
                fetched_at INTEGER NOT NULL
            );
        ").unwrap();

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        let data = OgData {
            url: "https://example.com".to_string(),
            og_title: Some("Cached Title".to_string()),
            og_description: None,
            og_image: None,
            status: "ok".to_string(),
            fetched_at: now_ms,
        };

        upsert_og_cache(&conn, &data).unwrap();
        let hit = get_cached_og(&conn, "https://example.com");
        assert!(hit.is_some(), "Fresh cache entry should return a hit");
        assert_eq!(hit.unwrap().og_title, Some("Cached Title".to_string()));
    }

    #[test]
    fn test_cache_stale_miss() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("
            CREATE TABLE og_cache (
                url TEXT PRIMARY KEY,
                og_title TEXT,
                og_description TEXT,
                og_image TEXT,
                status TEXT NOT NULL DEFAULT 'ok',
                fetched_at INTEGER NOT NULL
            );
        ").unwrap();

        // fetched_at is 8 days ago — beyond CACHE_TTL_OK_MS (7 days).
        let stale_ts = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64)
            - (8 * 24 * 60 * 60 * 1000);

        let data = OgData {
            url: "https://stale.example.com".to_string(),
            og_title: Some("Old Title".to_string()),
            og_description: None,
            og_image: None,
            status: "ok".to_string(),
            fetched_at: stale_ts,
        };

        upsert_og_cache(&conn, &data).unwrap();
        let hit = get_cached_og(&conn, "https://stale.example.com");
        assert!(hit.is_none(), "Stale cache entry should return None (cache miss)");
    }

    #[test]
    fn test_cache_failure_ttl_1_hour() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("
            CREATE TABLE og_cache (
                url TEXT PRIMARY KEY,
                og_title TEXT,
                og_description TEXT,
                og_image TEXT,
                status TEXT NOT NULL DEFAULT 'ok',
                fetched_at INTEGER NOT NULL
            );
        ").unwrap();

        // A blocked/error entry fetched 30 min ago — within 1h TTL, should be a hit.
        let thirty_min_ago = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64)
            - (30 * 60 * 1000);

        let data = OgData {
            url: "https://blocked.example.com".to_string(),
            og_title: None,
            og_description: None,
            og_image: None,
            status: "blocked".to_string(),
            fetched_at: thirty_min_ago,
        };

        upsert_og_cache(&conn, &data).unwrap();
        let hit = get_cached_og(&conn, "https://blocked.example.com");
        assert!(hit.is_some(), "Blocked entry within 1h should still be a cache hit");
    }

    #[test]
    fn test_cache_failure_ttl_expired() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("
            CREATE TABLE og_cache (
                url TEXT PRIMARY KEY,
                og_title TEXT,
                og_description TEXT,
                og_image TEXT,
                status TEXT NOT NULL DEFAULT 'ok',
                fetched_at INTEGER NOT NULL
            );
        ").unwrap();

        // A 'timeout' entry fetched 2 hours ago — beyond 1h TTL.
        let two_hours_ago = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64)
            - (2 * 60 * 60 * 1000);

        let data = OgData {
            url: "https://timeout.example.com".to_string(),
            og_title: None,
            og_description: None,
            og_image: None,
            status: "timeout".to_string(),
            fetched_at: two_hours_ago,
        };

        upsert_og_cache(&conn, &data).unwrap();
        let hit = get_cached_og(&conn, "https://timeout.example.com");
        assert!(hit.is_none(), "Timeout entry beyond 1h TTL should be a cache miss");
    }
}

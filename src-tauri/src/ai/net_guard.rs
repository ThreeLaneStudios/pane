// net_guard.rs — Shared SSRF guard (§4.6.5, extracted from web.rs — #2751)
//
// Ownership: this module owns the SSRF blocklist, hostname pre-check, DNS
// resolution + address pinning, and the pinned `reqwest::Client` builder used
// by every outbound fetch Pane makes on the model's or the user's behalf.
// Both `web.rs` (`web_fetch`/`web_search`, §4.6.5, ADR-0064) and `og_fetch.rs`
// (link-preview og: tag fetch, ADR-0049 Decision 2) consume this module as
// their single SSRF enforcement point — neither module keeps its own copy of
// the blocklist or the resolve-then-pin machinery.
//
// Callers keep control of two things this module deliberately does NOT own:
//   - Per-hop redirect caps (`web.rs`'s `MAX_REDIRECTS = 5` vs `og_fetch.rs`'s
//     `MAX_REDIRECTS = 3`) — different tools, different risk tolerance.
//   - User-Agent string and timeout durations passed into `build_pinned_client`
//     — `web.rs` identifies itself as Pane; `og_fetch.rs` uses a generic
//     browser UA to maximize og: tag retrieval (see og_fetch.rs's own doc
//     comment). Redirect *re-validation on every hop* is owned here in spirit
//     (both callers call `hostname_precheck` + `resolve_and_pin` per hop) but
//     the loop itself lives in each caller because the two modules' hop
//     bookkeeping and error text differ (§4.6.7 exact text vs og_fetch's own
//     diagnostic strings).
//
// Every SSRF/security claim below names a mechanism (per ADR-0064's opening
// paragraph) — see docs/INVARIANTS.md "Web Browsing Tools (§4.6)" for the
// pinned invariants this module must uphold.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

// ─── Step 1 — scheme allowlist ───────────────────────────────────────────────

/// Step 1 — parse with the `url` crate; scheme must be http or https.
pub fn parse_and_check_scheme(raw_url: &str) -> Result<url::Url, String> {
    let parsed = url::Url::parse(raw_url).map_err(|e| format!("URL parse error: {}", e))?;
    match parsed.scheme() {
        "http" | "https" => Ok(parsed),
        _ => Err("scheme".to_string()),
    }
}

// ─── Step 2 — hostname pre-check ─────────────────────────────────────────────

/// Step 2 — hostname pre-check: `localhost`, `.localhost`, `.local`, `.internal`, or empty.
/// Runs before any DNS resolution — cheap rejection for the common local-name case.
pub fn hostname_precheck(host: &str) -> Result<(), String> {
    if host.is_empty() {
        return Err("empty_host".to_string());
    }
    let lower = host.to_ascii_lowercase();
    if lower == "localhost"
        || lower.ends_with(".localhost")
        || lower.ends_with(".local")
        || lower.ends_with(".internal")
    {
        return Err("blocked_hostname".to_string());
    }
    Ok(())
}

// ─── Step 4 — blocklist ──────────────────────────────────────────────────────

/// Step 4 — blocked IP classes per §4.6.5. IPv4-mapped/NAT64 IPv6 addresses are
/// unwrapped and re-checked as IPv4.
pub fn is_blocked_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_blocked_v4(v4),
        IpAddr::V6(v6) => is_blocked_v6(v6),
    }
}

fn is_blocked_v4(ip: &Ipv4Addr) -> bool {
    let o = ip.octets();
    // 0.0.0.0/8
    if o[0] == 0 {
        return true;
    }
    // 10.0.0.0/8
    if o[0] == 10 {
        return true;
    }
    // 100.64.0.0/10 — CGNAT / Tailscale (ADR-0064 D7: deliberately blocked)
    if o[0] == 100 && (o[1] & 0xC0) == 64 {
        return true;
    }
    // 127.0.0.0/8
    if o[0] == 127 {
        return true;
    }
    // 169.254.0.0/16 (includes 169.254.169.254 cloud metadata)
    if o[0] == 169 && o[1] == 254 {
        return true;
    }
    // 172.16.0.0/12
    if o[0] == 172 && (o[1] & 0xF0) == 16 {
        return true;
    }
    // 192.168.0.0/16
    if o[0] == 192 && o[1] == 168 {
        return true;
    }
    // 224.0.0.0/4 multicast
    if (o[0] & 0xF0) == 224 {
        return true;
    }
    // 240.0.0.0/4 reserved
    if (o[0] & 0xF0) == 240 {
        return true;
    }
    // 255.255.255.255 broadcast
    if o == [255, 255, 255, 255] {
        return true;
    }
    false
}

fn is_blocked_v6(ip: &Ipv6Addr) -> bool {
    let seg = ip.segments();

    // ::
    if ip.is_unspecified() {
        return true;
    }
    // ::1
    if ip == &Ipv6Addr::LOCALHOST {
        return true;
    }

    // IPv4-mapped IPv6 (::ffff:0:0/96) and NAT64 (64:ff9b::/96) — unwrap and re-check as v4.
    let v4_mapped = seg[0] == 0
        && seg[1] == 0
        && seg[2] == 0
        && seg[3] == 0
        && seg[4] == 0
        && seg[5] == 0xFFFF;
    let nat64 = seg[0] == 0x0064 && seg[1] == 0xFF9B && seg[2] == 0 && seg[3] == 0 && seg[4] == 0 && seg[5] == 0;
    if v4_mapped || nat64 {
        let v4 = Ipv4Addr::new(
            (seg[6] >> 8) as u8,
            (seg[6] & 0xFF) as u8,
            (seg[7] >> 8) as u8,
            (seg[7] & 0xFF) as u8,
        );
        return is_blocked_v4(&v4);
    }

    // fc00::/7 unique-local
    if (seg[0] & 0xFE00) == 0xFC00 {
        return true;
    }
    // fe80::/10 link-local
    if (seg[0] & 0xFFC0) == 0xFE80 {
        return true;
    }
    // ff00::/8 multicast
    if (seg[0] & 0xFF00) == 0xFF00 {
        return true;
    }

    false
}

// ─── Steps 3+4 — resolve + pin ───────────────────────────────────────────────

/// Steps 3+4 — resolve the host (tokio `lookup_host`) and check every resolved
/// address against the blocklist. Returns the checked addresses on success —
/// these are the exact addresses `build_pinned_client` connects to (step 5),
/// which is what defeats DNS rebinding: the check and the connection use the
/// same resolved addresses.
///
/// Error strings are classified by prefix so callers can distinguish DNS
/// failure from a blocked address for their own §4.6.7-style user-facing text:
/// `"dns_failure"` / `"dns_failure: <e>"` vs `"blocked_ip"`.
pub async fn resolve_and_pin(host: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
    // Test-only: allow the mockito loopback address so integration tests can
    // exercise the fetch path. The SSRF blocklist itself is covered independently
    // by the `is_blocked_ip` unit tests below, which always run — this bypass
    // never weakens that coverage.
    // IPv6 literal hosts must be bracketed for the "host:port" lookup_host format
    // (e.g. "::1:80" is ambiguous; "[::1]:80" is not).
    let lookup_target = if host.parse::<Ipv6Addr>().is_ok() {
        format!("[{}]:{}", host, port)
    } else {
        format!("{}:{}", host, port)
    };

    #[cfg(test)]
    if host == "127.0.0.1" || host == "localhost" {
        let addrs: Vec<SocketAddr> = tokio::net::lookup_host(&lookup_target)
            .await
            .map_err(|e| format!("dns_failure: {}", e))?
            .collect();
        if addrs.is_empty() {
            return Err("dns_failure".to_string());
        }
        return Ok(addrs);
    }

    let addrs: Vec<SocketAddr> = tokio::net::lookup_host(&lookup_target)
        .await
        .map_err(|e| format!("dns_failure: {}", e))?
        .collect();

    if addrs.is_empty() {
        return Err("dns_failure".to_string());
    }

    for socket_addr in &addrs {
        if is_blocked_ip(&socket_addr.ip()) {
            return Err("blocked_ip".to_string());
        }
    }

    Ok(addrs)
}

// ─── Step 5 — pinned client builder ──────────────────────────────────────────

/// Step 5 — build a fresh client pinned to the checked addresses via
/// `resolve_to_addrs`. This is what defeats DNS rebinding: the check
/// (`resolve_and_pin`) and the connection use the exact same resolved
/// addresses. Redirects are disabled here; callers follow them manually so
/// each hop can be re-validated (step 6) with a fresh call to
/// `hostname_precheck` + `resolve_and_pin` + `build_pinned_client`.
///
/// `user_agent`/`connect_timeout`/`request_timeout` are caller-supplied since
/// `web.rs` and `og_fetch.rs` use different values (see module doc comment).
pub fn build_pinned_client(
    host: &str,
    addrs: &[SocketAddr],
    user_agent: &str,
    connect_timeout: Duration,
    request_timeout: Duration,
) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(connect_timeout)
        .timeout(request_timeout)
        .user_agent(user_agent)
        .resolve_to_addrs(host, addrs)
        .build()
        .map_err(|e| format!("client_build_failed: {}", e))
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Blocklist: one test per blocked range (§4.6.5 step 4) ────────────────

    #[test]
    fn test_blocks_0_0_0_0_8() {
        let ip: IpAddr = "0.5.5.5".parse().unwrap();
        assert!(is_blocked_ip(&ip), "0.0.0.0/8 must be blocked");
    }

    #[test]
    fn test_blocks_10_0_0_0_8() {
        let ip: IpAddr = "10.1.2.3".parse().unwrap();
        assert!(is_blocked_ip(&ip), "10.0.0.0/8 must be blocked");
    }

    #[test]
    fn test_blocks_100_64_0_0_10_cgnat_tailscale() {
        let ip: IpAddr = "100.64.0.1".parse().unwrap();
        assert!(is_blocked_ip(&ip), "100.64.0.0/10 (CGNAT/Tailscale) must be blocked — ADR-0064 D7");
        let ip2: IpAddr = "100.127.255.255".parse().unwrap();
        assert!(is_blocked_ip(&ip2), "100.64.0.0/10 upper bound must be blocked");
        let ip3: IpAddr = "100.128.0.0".parse().unwrap();
        assert!(!is_blocked_ip(&ip3), "100.128.0.0 is outside 100.64.0.0/10 — must NOT be blocked");
    }

    #[test]
    fn test_blocks_127_0_0_0_8() {
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        assert!(is_blocked_ip(&ip), "127.0.0.0/8 loopback must be blocked");
    }

    #[test]
    fn test_blocks_169_254_0_0_16_and_metadata() {
        let ip: IpAddr = "169.254.169.254".parse().unwrap();
        assert!(is_blocked_ip(&ip), "169.254.169.254 cloud metadata must be blocked");
        let ip2: IpAddr = "169.254.1.1".parse().unwrap();
        assert!(is_blocked_ip(&ip2), "169.254.0.0/16 link-local must be blocked");
    }

    #[test]
    fn test_blocks_172_16_0_0_12() {
        let ip: IpAddr = "172.16.5.5".parse().unwrap();
        assert!(is_blocked_ip(&ip), "172.16.0.0/12 must be blocked");
        let ip2: IpAddr = "172.32.0.1".parse().unwrap();
        assert!(!is_blocked_ip(&ip2), "172.32.0.1 is outside 172.16.0.0/12 — must NOT be blocked");
    }

    #[test]
    fn test_blocks_192_168_0_0_16() {
        let ip: IpAddr = "192.168.1.1".parse().unwrap();
        assert!(is_blocked_ip(&ip), "192.168.0.0/16 must be blocked");
    }

    #[test]
    fn test_blocks_224_0_0_0_4_multicast() {
        let ip: IpAddr = "224.0.0.1".parse().unwrap();
        assert!(is_blocked_ip(&ip), "224.0.0.0/4 multicast must be blocked");
    }

    #[test]
    fn test_blocks_240_0_0_0_4_reserved() {
        let ip: IpAddr = "240.0.0.1".parse().unwrap();
        assert!(is_blocked_ip(&ip), "240.0.0.0/4 reserved must be blocked");
    }

    #[test]
    fn test_blocks_255_255_255_255_broadcast() {
        let ip: IpAddr = "255.255.255.255".parse().unwrap();
        assert!(is_blocked_ip(&ip), "255.255.255.255 broadcast must be blocked");
    }

    #[test]
    fn test_blocks_ipv6_unspecified_and_loopback() {
        let ip: IpAddr = "::".parse().unwrap();
        assert!(is_blocked_ip(&ip), ":: must be blocked");
        let ip2: IpAddr = "::1".parse().unwrap();
        assert!(is_blocked_ip(&ip2), "::1 must be blocked");
    }

    #[test]
    fn test_blocks_ipv6_fc00_7_unique_local() {
        let ip: IpAddr = "fc00::1".parse().unwrap();
        assert!(is_blocked_ip(&ip), "fc00::/7 must be blocked");
        let ip2: IpAddr = "fd12::1".parse().unwrap();
        assert!(is_blocked_ip(&ip2), "fd00::/8 (within fc00::/7) must be blocked");
    }

    #[test]
    fn test_blocks_ipv6_fe80_10_link_local() {
        let ip: IpAddr = "fe80::1".parse().unwrap();
        assert!(is_blocked_ip(&ip), "fe80::/10 must be blocked");
    }

    #[test]
    fn test_blocks_ipv6_ff00_8_multicast() {
        let ip: IpAddr = "ff02::1".parse().unwrap();
        assert!(is_blocked_ip(&ip), "ff00::/8 must be blocked");
    }

    #[test]
    fn test_blocks_ipv4_mapped_ipv6_unwrap_and_recheck() {
        let ip: IpAddr = "::ffff:127.0.0.1".parse().unwrap();
        assert!(is_blocked_ip(&ip), "IPv4-mapped loopback must be unwrapped and blocked");
        let ip2: IpAddr = "::ffff:192.168.1.1".parse().unwrap();
        assert!(is_blocked_ip(&ip2), "IPv4-mapped private must be unwrapped and blocked");
        let ip3: IpAddr = "::ffff:8.8.8.8".parse().unwrap();
        assert!(!is_blocked_ip(&ip3), "IPv4-mapped public IP must NOT be blocked");
    }

    #[test]
    fn test_blocks_nat64_mapped() {
        let ip: IpAddr = "64:ff9b::a00:1".parse().unwrap(); // 64:ff9b::10.0.0.1
        assert!(is_blocked_ip(&ip), "NAT64-mapped private (10.0.0.1) must be unwrapped and blocked");
    }

    #[test]
    fn test_allows_public_ips() {
        let ip: IpAddr = "8.8.8.8".parse().unwrap();
        assert!(!is_blocked_ip(&ip), "public IPv4 must not be blocked");
        let ip2: IpAddr = "2001:4860:4860::8888".parse().unwrap();
        assert!(!is_blocked_ip(&ip2), "public IPv6 must not be blocked");
    }

    // ── Hostname pre-check ────────────────────────────────────────────────────

    #[test]
    fn test_hostname_precheck_blocks_localhost() {
        assert!(hostname_precheck("localhost").is_err());
        assert!(hostname_precheck("foo.localhost").is_err());
    }

    #[test]
    fn test_hostname_precheck_blocks_dot_local() {
        assert!(hostname_precheck("myprinter.local").is_err());
    }

    #[test]
    fn test_hostname_precheck_blocks_dot_internal() {
        assert!(hostname_precheck("service.internal").is_err());
    }

    #[test]
    fn test_hostname_precheck_blocks_empty() {
        assert!(hostname_precheck("").is_err());
    }

    #[test]
    fn test_hostname_precheck_allows_public() {
        assert!(hostname_precheck("example.com").is_ok());
    }

    // ── Scheme allowlist ──────────────────────────────────────────────────────

    #[test]
    fn test_scheme_rejects_file_ftp_data() {
        assert!(parse_and_check_scheme("file:///etc/passwd").is_err());
        assert!(parse_and_check_scheme("ftp://ftp.example.com/x").is_err());
        assert!(parse_and_check_scheme("data:text/html,<h1>x</h1>").is_err());
    }

    #[test]
    fn test_scheme_allows_http_https() {
        assert!(parse_and_check_scheme("http://example.com").is_ok());
        assert!(parse_and_check_scheme("https://example.com").is_ok());
    }

    // ── resolve_and_pin + build_pinned_client ─────────────────────────────────

    #[tokio::test]
    async fn test_resolve_and_pin_connects_to_resolved_address() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/page")
            .with_status(200)
            .with_body("ok")
            .create_async()
            .await;

        let url = format!("{}/page", server.url());
        let host = url::Url::parse(&url).unwrap().host_str().unwrap().to_string();
        let port = url::Url::parse(&url).unwrap().port_or_known_default().unwrap();

        let addrs = resolve_and_pin(&host, port).await.expect("resolve should succeed for test loopback");
        assert!(!addrs.is_empty());

        let client = build_pinned_client(&host, &addrs, "net_guard-test/1.0", Duration::from_secs(5), Duration::from_secs(15))
            .expect("client build should succeed");
        let resp = client.get(&url).send().await.expect("pinned client should connect to resolved address");
        assert!(resp.status().is_success());
    }

    #[tokio::test]
    async fn test_resolve_and_pin_dns_failure_reported() {
        // A hostname with no valid DNS resolution — port 1 is arbitrary since
        // resolution fails before any connection is attempted.
        let result = resolve_and_pin("this-host-does-not-exist.invalid", 80).await;
        assert!(result.is_err(), "unresolvable hostname must error");
        let err = result.unwrap_err();
        assert!(err.starts_with("dns_failure"), "expected dns_failure classification, got: {}", err);
    }
}

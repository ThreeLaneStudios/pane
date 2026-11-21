//! #3106 M2 — structural guard for the ten sites Matt named (seven
//! originally, plus two more found by Reed's #3200 FAIL audit in
//! `device_identity.rs`, plus one more found during #3218's item 10 audit in
//! `oc_client.rs`'s `send_request` — a tenth site, logging the same
//! bearer-token-bearing `connect` request the already-fixed write-task site
//! one function away explicitly calls out, just never itself redacted) as
//! logging a pairing-code, bearer-token, or authentication-nonce value in
//! full.
//!
//! Source-text scan, not a stderr-capture test: Rust's test runner shares one
//! stderr stream across parallel test threads, so asserting on captured
//! output would force either `--test-threads=1` (slow, against this crate's
//! existing convention) or tolerate flakiness from interleaved output. This
//! repo already uses the source-text-scan shape for exactly this kind of
//! "no site may do X" structural guard (see, e.g., the migration-arm sweep
//! tests elsewhere in this crate) — this test follows the same idiom.
//!
//! Each entry below is the EXACT stale format-string text that used to
//! interpolate the sensitive value. If any of these nine patterns is ever
//! reintroduced (a revert, a careless copy-paste of the old line, a new call
//! site added without following the redaction convention), this test goes
//! red immediately rather than silently.

use std::fs;
use std::path::Path;

#[test]
fn test_no_pairing_code_bearer_token_or_nonce_logged_at_named_sites() {
    let base = Path::new(env!("CARGO_MANIFEST_DIR"));

    // (file, the stale substring that would mean this site still leaks a value)
    let known_bad: &[(&str, &str)] = &[
        // auth.rs — pairing-code generation + exchange (Reed's original 2 of 4)
        ("src/auth.rs", "eprintln!(\"[AUTH] Generated pairing code: {}"),
        ("src/auth.rs", "eprintln!(\"[AUTH] Attempting to exchange pairing code: {}"),
        // server.rs — the Unix-socket IPC handler's own generate_pairing_code arm
        ("src/server.rs", "eprintln!(\"[IPC] Generated pairing code: {}"),
        // routes/pair.rs — the public /pair HTTP endpoint
        ("src/routes/pair.rs", "eprintln!(\"[PAIR] Pair endpoint called with code: {}"),
        // main.rs — the startup banner (Matt widened the redaction to cover
        // this intentional-display site too; retrieval moved to the
        // `pane-gateway pairing-code` CLI subcommand, stdout on explicit
        // request, never a log stream)
        ("src/main.rs", "eprintln!(\"{}\", pairing_code)"),
        // oc_client.rs — the write task logs every outgoing WS message in
        // full, including the `connect` request, which embeds the OC bearer
        // token (`"auth": {"token": bearer_token}`) and the signed device
        // auth payload
        ("src/oc_client.rs", "eprintln!(\"[OC_CLIENT] Write task sending: {}\", message)"),
        // oc_client.rs — the one-time authentication nonce fed into the
        // device-identity signature
        ("src/oc_client.rs", "eprintln!(\"[OC_CLIENT] Challenge nonce: {}\", nonce)"),
        // device_identity.rs — found by Reed's #3200 FAIL audit: the nonce
        // fed into the v3 signature, logged again one call deeper than the
        // oc_client.rs site above
        ("src/device_identity.rs", "eprintln!(\"[DEVICE_IDENTITY] Signing v3 payload with nonce: {}\", nonce)"),
        // device_identity.rs — found by Reed's #3200 FAIL audit: the most
        // severe site in the whole set — logs the fully-assembled v3 payload
        // string, which embeds the live OC bearer token in cleartext
        // (`v3|{deviceId}|...|{token}|{nonce}|...`), unconditionally, on
        // every connect/reconnect handshake
        ("src/device_identity.rs", "eprintln!(\"[DEVICE_IDENTITY] Payload to sign: {}\", payload)"),
        // oc_client.rs — found during #3218 item 10's audit: send_request()'s
        // own log line, logging the full serialized RPC message (including
        // the `connect` request's cleartext bearer token) BEFORE handing it
        // to the write-task channel — the write task's own log of the same
        // message was already redacted (see the comment at that call site),
        // this one upstream of it never was. Fixed in the same PR that added
        // this entry.
        ("src/oc_client.rs", "eprintln!(\"[OC_CLIENT] Sending: {}\", message)"),
    ];

    let mut leaks = Vec::new();
    for (rel_path, stale_pattern) in known_bad {
        let full_path = base.join(rel_path);
        let contents = fs::read_to_string(&full_path)
            .unwrap_or_else(|e| panic!("failed to read {}: {}", full_path.display(), e));
        if contents.contains(stale_pattern) {
            leaks.push(format!("{}: still contains {:?}", rel_path, stale_pattern));
        }
    }

    assert!(
        leaks.is_empty(),
        "code/token/nonce-bearing log line(s) found (zero characters, zero shape required):\n{}",
        leaks.join("\n")
    );
}

/// #3218 item 10 — complementary sweep, inverse polarity of `known_bad`
/// above. `known_bad` is a frozen regression guard against ten
/// already-fixed, historically-bad patterns; it says nothing about a NEW
/// site a developer adds next month. This scans every `.rs` file under
/// `src-gateway/src/` for `eprintln!`/`println!`/`tracing::*!` call sites
/// whose format arguments reference one of a small named set of sensitive
/// identifiers, and fails on any site outside the explicit, reviewed
/// allowlist below. "Known currently exempted, should shrink to zero" —
/// as opposed to `known_bad`'s "known historically bad, frozen."
#[test]
fn test_no_sensitive_identifier_logged_outside_allowlist() {
    let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");

    // Reviewed, currently-allowed exceptions, as (file-relative-to-src,
    // snippet) pairs. Empty today — keep it that way; any addition here
    // needs its own review, same bar as `known_bad` above.
    let allowlist: &[(&str, &str)] = &[];

    const SENSITIVE_IDENTS: &[&str] =
        &["pairing_code", "bearer_token", "token", "nonce", "payload"];

    let macro_re = regex::Regex::new(
        r#"(?:eprintln|println|tracing::(?:info|warn|error|debug|trace))!\s*\(\s*"(?:[^"\\]|\\.)*"\s*,\s*([^;]*?)\)\s*;"#,
    )
    .expect("sweep regex must compile");

    let mut rs_files = Vec::new();
    collect_rs_files(&src_dir, &mut rs_files);

    let mut violations = Vec::new();
    for path in &rs_files {
        let contents = fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("failed to read {}: {}", path.display(), e));
        let rel = path
            .strip_prefix(&src_dir)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let rel = format!("src/{}", rel);

        for caps in macro_re.captures_iter(&contents) {
            let whole = caps.get(0).unwrap();
            let args = caps.get(1).unwrap().as_str();
            let line_no = contents[..whole.start()].matches('\n').count() + 1;

            for arg in split_top_level_commas(args) {
                let arg = arg.trim();
                let bare = arg.trim_end_matches("()");
                let last_segment = bare.rsplit('.').next().unwrap_or(bare).trim();
                if SENSITIVE_IDENTS.contains(&last_segment) {
                    let snippet = whole.as_str().trim();
                    let exempt = allowlist
                        .iter()
                        .any(|(f, s)| *f == rel && snippet.contains(s));
                    if !exempt {
                        violations.push(format!(
                            "{}:{}: logs sensitive identifier `{}` — {}",
                            rel, line_no, last_segment, snippet
                        ));
                    }
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "new log site(s) referencing a sensitive identifier found, not in the reviewed \
         allowlist (fix the log line, or add a reviewed allowlist entry):\n{}",
        violations.join("\n")
    );
}

fn collect_rs_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

fn split_top_level_commas(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, ch) in s.char_indices() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if start < s.len() {
        parts.push(&s[start..]);
    }
    parts
}

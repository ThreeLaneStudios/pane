/// Gateway parameter compatibility layer.
///
/// # Purpose
///
/// Some downstream models reject request parameters that were previously accepted (e.g.,
/// Anthropic deprecated `temperature` and `top_p` for `claude-opus-4-7`). The gateway is the
/// correct layer to handle these provider quirks: Pane clients should never need to know which
/// models reject which params.
///
/// This module owns the compat table (loaded from `~/.pane-gateway/param-compat.json`) and the
/// `strip_params` function used by `routes/chat.rs` before forwarding to OC.
///
/// # Config file
///
/// The table lives at `{data_dir}/param-compat.json`. On first gateway run, if no file is found,
/// the bundled default (`src-gateway/config/param-compat-default.json`) is written to disk so
/// operators have a file they can edit.
///
/// Schema:
/// ```json
/// {
///   "version": 1,
///   "rules": [
///     {
///       "match": { "kind": "exact" | "prefix" | "regex", "value": "<string>" },
///       "strip": ["temperature", "top_p"],
///       "reason": "<human-readable rationale>"
///     }
///   ]
/// }
/// ```
///
/// # Adding a new model
///
/// 1. Open `~/.pane-gateway/param-compat.json`
/// 2. Add a rule with the appropriate `match` and `strip` fields
/// 3. Restart the gateway — no rebuild required
///
/// See `src-gateway/README.md` for the full guide.
use crate::error::GatewayError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;

/// Bundled default config; written to disk on first run if no config file exists.
const DEFAULT_CONFIG: &str = include_str!("../config/param-compat-default.json");

// ── Data types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MatchKind {
    /// Model ID must equal this value exactly.
    Exact,
    /// Model ID must start with this value.
    Prefix,
    /// Model ID must match this regular expression. The gateway auto-anchors the
    /// user's pattern with `^(?:...)$` so it always full-string matches. Operators
    /// who want substring matching can write `.*pattern.*` in their rule value.
    Regex,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompatMatch {
    pub kind: MatchKind,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompatRule {
    #[serde(rename = "match")]
    pub match_spec: CompatMatch,
    /// Parameter names to remove from the request body when this rule fires.
    pub strip: Vec<String>,
    /// Human-readable rationale (logged when the rule fires).
    pub reason: String,
}

/// The full compat table loaded from disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompatTable {
    pub version: u32,
    pub rules: Vec<CompatRule>,
}

impl Default for CompatTable {
    /// Returns an empty compat table — no rules, nothing stripped.
    /// Used in tests that don't care about param stripping.
    fn default() -> Self {
        Self {
            version: 1,
            rules: vec![],
        }
    }
}

impl CompatTable {
    // ── Loading ───────────────────────────────────────────────────────────────

    /// Load from `data_dir/param-compat.json`.
    ///
    /// - File exists: parse it; return a clear error if malformed.
    /// - File missing: write the bundled default to disk, then use it.
    pub fn load_or_default(data_dir: &Path) -> Result<Arc<Self>, GatewayError> {
        let config_path = data_dir.join("param-compat.json");

        let contents = if config_path.exists() {
            eprintln!(
                "[PARAM_COMPAT] Loading config from: {}",
                config_path.display()
            );
            std::fs::read_to_string(&config_path).map_err(|e| {
                GatewayError::Config(format!(
                    "Failed to read param-compat.json at {}: {}",
                    config_path.display(),
                    e
                ))
            })?
        } else {
            eprintln!(
                "[PARAM_COMPAT] No config at {}; writing bundled default",
                config_path.display()
            );
            std::fs::create_dir_all(data_dir).map_err(|e| {
                GatewayError::Config(format!(
                    "Failed to create data directory {}: {}",
                    data_dir.display(),
                    e
                ))
            })?;
            std::fs::write(&config_path, DEFAULT_CONFIG).map_err(|e| {
                GatewayError::Config(format!(
                    "Failed to write default param-compat.json to {}: {}",
                    config_path.display(),
                    e
                ))
            })?;
            eprintln!(
                "[PARAM_COMPAT] Default config written to: {}",
                config_path.display()
            );
            DEFAULT_CONFIG.to_string()
        };

        let table: CompatTable = serde_json::from_str(&contents).map_err(|e| {
            GatewayError::Config(format!(
                "Malformed param-compat.json at {}: {}",
                config_path.display(),
                e
            ))
        })?;

        eprintln!(
            "[PARAM_COMPAT] Loaded {} rule(s) (version {})",
            table.rules.len(),
            table.version
        );
        Ok(Arc::new(table))
    }

    // ── Matching ──────────────────────────────────────────────────────────────

    fn rule_matches(rule: &CompatRule, model: &str) -> bool {
        match rule.match_spec.kind {
            MatchKind::Exact => model == rule.match_spec.value,
            MatchKind::Prefix => model.starts_with(&*rule.match_spec.value),
            MatchKind::Regex => {
                // Compile on demand — this is not a hot-inner-loop path; the overhead is
                // negligible compared to the network round-trip the request is about to make.
                //
                // Anchored full-string match: we wrap the user's pattern in `^(?:...)$` so
                // operators get the behavior promised by the README and docstring. Without
                // anchoring, `is_match` would substring-match (e.g., `claude-opus-4-[0-9]+`
                // would match `anthropic/claude-opus-4-7` silently). Anchoring removes the
                // footgun; operators who want substring matching can write `.*pattern.*`.
                let anchored = format!("^(?:{})$", rule.match_spec.value);
                match regex::Regex::new(&anchored) {
                    Ok(re) => re.is_match(model),
                    Err(e) => {
                        eprintln!(
                            "[PARAM_COMPAT] Warning: invalid regex '{}' in rule: {}",
                            rule.match_spec.value, e
                        );
                        false
                    }
                }
            }
        }
    }

    // ── Stripping ─────────────────────────────────────────────────────────────

    /// Strip request parameters from `body` that are unsupported by `model`.
    ///
    /// Returns a list of `(param_name, reason)` for each param that was present and removed.
    /// The caller (chat route) logs these at INFO level.
    ///
    /// All matching rules are applied in order; parameters are stripped by the first matching
    /// rule that lists them. Duplicate strips across multiple rules are harmless.
    pub fn strip_params(&self, model: &str, body: &mut Value) -> Vec<(String, String)> {
        let mut stripped: Vec<(String, String)> = Vec::new();

        for rule in &self.rules {
            if Self::rule_matches(rule, model) {
                if let Some(obj) = body.as_object_mut() {
                    for param in &rule.strip {
                        if obj.remove(param.as_str()).is_some() {
                            stripped.push((param.clone(), rule.reason.clone()));
                        }
                    }
                }
            }
        }

        stripped
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    /// Build a minimal CompatTable for testing — independent of disk.
    fn test_table() -> CompatTable {
        serde_json::from_str(
            r#"{
              "version": 1,
              "rules": [
                {
                  "match": { "kind": "exact", "value": "claude-opus-4-7" },
                  "strip": ["temperature", "top_p"],
                  "reason": "test: opus-4-7 rejects sampling controls"
                },
                {
                  "match": { "kind": "prefix", "value": "anthropic/claude-opus-4-8" },
                  "strip": ["temperature", "top_p"],
                  "reason": "test: opus-4-8+ prefix forward-compat"
                },
                {
                  "match": { "kind": "regex", "value": "^test-regex-model-[0-9]+$" },
                  "strip": ["temperature"],
                  "reason": "test: regex match"
                }
              ]
            }"#,
        )
        .expect("static test table must parse")
    }

    // ── Exact match ──────────────────────────────────────────────────────────

    #[test]
    fn test_exact_match_strips_temperature() {
        let table = test_table();
        let mut body = json!({
            "model": "claude-opus-4-7",
            "messages": [],
            "temperature": 0.7
        });

        let stripped = table.strip_params("claude-opus-4-7", &mut body);

        assert_eq!(stripped.len(), 1, "should strip exactly one param");
        assert_eq!(stripped[0].0, "temperature");
        assert!(body.get("temperature").is_none(), "temperature must be absent from body");
        assert!(body.get("messages").is_some(), "messages must survive");
    }

    #[test]
    fn test_exact_match_strips_both_params() {
        let table = test_table();
        let mut body = json!({
            "model": "claude-opus-4-7",
            "messages": [],
            "temperature": 0.7,
            "top_p": 0.9
        });

        let stripped = table.strip_params("claude-opus-4-7", &mut body);

        let param_names: Vec<&str> = stripped.iter().map(|(p, _)| p.as_str()).collect();
        assert!(param_names.contains(&"temperature"), "temperature should be stripped");
        assert!(param_names.contains(&"top_p"), "top_p should be stripped");
        assert!(body.get("temperature").is_none());
        assert!(body.get("top_p").is_none());
    }

    #[test]
    fn test_exact_match_no_params_present_no_strip() {
        let table = test_table();
        let mut body = json!({
            "model": "claude-opus-4-7",
            "messages": []
        });

        let stripped = table.strip_params("claude-opus-4-7", &mut body);

        // Body doesn't have temperature or top_p — nothing to strip
        assert!(stripped.is_empty(), "nothing to strip when params absent");
    }

    // ── Prefix match ─────────────────────────────────────────────────────────

    #[test]
    fn test_prefix_match_strips_params() {
        let table = test_table();
        let mut body = json!({
            "messages": [],
            "temperature": 0.5,
            "top_p": 0.8
        });

        let stripped = table.strip_params("anthropic/claude-opus-4-8-preview", &mut body);

        let param_names: Vec<&str> = stripped.iter().map(|(p, _)| p.as_str()).collect();
        assert!(param_names.contains(&"temperature"));
        assert!(param_names.contains(&"top_p"));
    }

    #[test]
    fn test_prefix_match_exact_prefix_boundary() {
        let table = test_table();
        let mut body = json!({ "messages": [], "temperature": 0.7 });

        // Exact prefix value itself should also match
        let stripped = table.strip_params("anthropic/claude-opus-4-8", &mut body);
        assert!(!stripped.is_empty(), "exact prefix value should match");
    }

    // ── No match (pass-through) ───────────────────────────────────────────────

    #[test]
    fn test_no_match_passthrough_with_temperature() {
        let table = test_table();
        let mut body = json!({
            "model": "claude-sonnet-4-6",
            "messages": [],
            "temperature": 0.7
        });

        let stripped = table.strip_params("claude-sonnet-4-6", &mut body);

        assert!(stripped.is_empty(), "sonnet-4-6 should not strip any params");
        assert_eq!(
            body["temperature"].as_f64(),
            Some(0.7),
            "temperature must survive for non-stripped model"
        );
    }

    #[test]
    fn test_empty_model_string_no_strip() {
        let table = test_table();
        let mut body = json!({ "messages": [], "temperature": 0.7 });

        let stripped = table.strip_params("", &mut body);

        assert!(stripped.is_empty());
        assert!(body.get("temperature").is_some());
    }

    // ── Regex match ───────────────────────────────────────────────────────────

    #[test]
    fn test_regex_match_strips_temperature() {
        let table = test_table();
        let mut body = json!({ "messages": [], "temperature": 0.4 });

        let stripped = table.strip_params("test-regex-model-42", &mut body);

        assert_eq!(stripped.len(), 1);
        assert_eq!(stripped[0].0, "temperature");
        assert!(body.get("temperature").is_none());
    }

    #[test]
    fn test_regex_no_match_for_partial_string() {
        let table = test_table();
        let mut body = json!({ "messages": [], "temperature": 0.4 });

        // Pattern is anchored ^...$, "prefix-test-regex-model-1" should not match
        let stripped = table.strip_params("prefix-test-regex-model-1", &mut body);
        assert!(stripped.is_empty());
    }

    /// Regression guard: code MUST anchor user regex patterns with ^...$ so that
    /// an unanchored pattern still gets full-string matching. Without this anchoring,
    /// operators who add a regex rule like "claude-opus-4-[0-9]+" would silently get
    /// substring matching against e.g. "anthropic/claude-opus-4-7" — a footgun.
    /// See PR #1330 / Reed audit §3 regex anchoring concern.
    #[test]
    fn test_regex_unanchored_pattern_is_full_string_matched() {
        let json = r#"{
          "version": 1,
          "rules": [
            {
              "match": { "kind": "regex", "value": "claude-opus-4-[0-9]+" },
              "strip": ["temperature"],
              "reason": "test: unanchored regex must not substring-match"
            }
          ]
        }"#;
        let table: CompatTable = serde_json::from_str(json).expect("should parse");

        // Exact full string match — SHOULD strip
        let mut body = json!({ "messages": [], "temperature": 0.7 });
        let stripped = table.strip_params("claude-opus-4-7", &mut body);
        assert_eq!(stripped.len(), 1, "exact full-string match must strip");
        assert!(body.get("temperature").is_none());

        // Provider-prefixed (would substring-match without anchoring) — must NOT strip
        let mut body = json!({ "messages": [], "temperature": 0.7 });
        let stripped = table.strip_params("anthropic/claude-opus-4-7", &mut body);
        assert!(
            stripped.is_empty(),
            "unanchored pattern must NOT substring-match the provider-prefixed form; got: {:?}",
            stripped
        );
        assert_eq!(body.get("temperature"), Some(&json!(0.7)));

        // Trailing suffix (would substring-match without anchoring) — must NOT strip
        let mut body = json!({ "messages": [], "temperature": 0.7 });
        let stripped = table.strip_params("claude-opus-4-7-experimental", &mut body);
        assert!(
            stripped.is_empty(),
            "unanchored pattern must NOT substring-match a trailing-suffix form; got: {:?}",
            stripped
        );
    }

    // ── Table loading ─────────────────────────────────────────────────────────

    #[test]
    fn test_load_from_json_string() {
        let json = r#"{
          "version": 1,
          "rules": [
            {
              "match": { "kind": "exact", "value": "my-model" },
              "strip": ["temperature"],
              "reason": "test reason"
            }
          ]
        }"#;
        let table: CompatTable = serde_json::from_str(json).expect("should parse");
        assert_eq!(table.version, 1);
        assert_eq!(table.rules.len(), 1);
        assert_eq!(table.rules[0].strip, vec!["temperature"]);
    }

    #[test]
    fn test_malformed_json_returns_error() {
        let dir = tempdir().expect("tempdir");
        let config_path = dir.path().join("param-compat.json");
        std::fs::write(&config_path, b"{this is not valid json}").unwrap();

        let result = CompatTable::load_or_default(dir.path());
        assert!(result.is_err(), "malformed JSON should return an error");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Malformed") || err.contains("malformed") || err.contains("param-compat"),
            "error message should reference the file: {}", err
        );
    }

    #[test]
    fn test_missing_file_writes_default_and_loads() {
        let dir = tempdir().expect("tempdir");
        let config_path = dir.path().join("param-compat.json");

        // File must not exist yet
        assert!(!config_path.exists());

        let result = CompatTable::load_or_default(dir.path());
        assert!(result.is_ok(), "missing file should fall back to default: {:?}", result.err());

        // Default should now be written to disk
        assert!(config_path.exists(), "default config should be written to disk");

        let on_disk = std::fs::read_to_string(&config_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&on_disk).unwrap();
        assert_eq!(parsed["version"].as_u64(), Some(1));
    }

    #[test]
    fn test_bundled_default_config_is_valid_json() {
        // If this panics, the bundled default is malformed — CI will catch it at compile time
        let result: Result<CompatTable, _> = serde_json::from_str(DEFAULT_CONFIG);
        assert!(result.is_ok(), "bundled default must be valid JSON: {:?}", result.err());
        let table = result.unwrap();
        assert!(table.version >= 1);
        assert!(!table.rules.is_empty(), "bundled default must contain at least one rule");
    }

    // ── Reason field ──────────────────────────────────────────────────────────

    #[test]
    fn test_reason_returned_with_stripped_param() {
        let table = test_table();
        let mut body = json!({ "temperature": 0.7 });

        let stripped = table.strip_params("claude-opus-4-7", &mut body);

        assert!(!stripped.is_empty());
        let (param, reason) = &stripped[0];
        assert_eq!(param, "temperature");
        assert!(!reason.is_empty(), "reason must be non-empty");
    }

    // ── Non-object body ───────────────────────────────────────────────────────

    #[test]
    fn test_non_object_body_does_not_panic() {
        let table = test_table();
        let mut body = json!([1, 2, 3]); // array, not object

        let stripped = table.strip_params("claude-opus-4-7", &mut body);
        // Should return nothing and not panic
        assert!(stripped.is_empty());
    }
}

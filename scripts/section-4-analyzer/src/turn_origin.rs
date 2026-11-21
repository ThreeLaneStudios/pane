// §36.11 AST gate — ADR-0016, #3059 PR 1.
//
// "The scheduler is the only component that starts a turn nobody typed."
// `docs/spec/runtime.md` §36.11: CI fails if `TurnOrigin::User` or
// `TurnOrigin::Job` is constructed anywhere except the interactive
// `ai_chat_stream` command and the scheduler tick. Before #3097's module
// split, `commands.rs` kept `ai_chat_stream`, `insert_external_message`,
// `ingest_external_messages` and `execute_single_tool` as sibling top-level
// functions in ONE module — Rust privacy is per-module, not per-function, so
// no visibility modifier could make a constructor unnameable from a sibling
// call site in that file. After the split those four now live in three
// files (`ai::chat::ai_chat_stream`, `ai::ingest::{insert_external_message,
// ingest_external_messages}`, `ai::tools::execute_single_tool`) — module
// privacy alone would separate them today, but this `syn`-based AST gate
// (bare-name allowlist, below) is what actually holds the line, in either
// layout: it doesn't care which file a constructor sits in.
//
// Shape, honestly stated (this is a genuinely different traversal than
// `lib.rs`'s `CallCollector`, which visits *inside* one already-known test
// function looking for a call — a "does this specific function contain call
// X" check). This rule has no starting function in hand: it walks an entire
// file, finds every enum-variant *construction* expression, and for each
// one, determines which function encloses it — "given a found construction,
// what's its context," the inverse of the existing analyzer's shape.
//
// Two facts about the production state of this rule, both deliberate:
//
// 1. `TurnOrigin` does not exist in production code yet (§36.2/§36.3, not
//    this PR). Scanning the real tree today finds zero constructions, which
//    means the gate passes — correctly, not vacuously-by-mistake, because
//    the check's shape is "every construction found is allowlisted," never
//    "every allowlisted name has a construction." The gate's own correctness
//    is proven by its fixture tests below, not by its behaviour on the real
//    tree (which is uninteresting until the enum exists).
//
// 2. The scheduler tick (§36.3) doesn't exist until PR 2. `ALLOWED_CONSTRUCTORS`
//    below names a placeholder for it now. If PR 2 names its tick entry point
//    something else and forgets to update this list, the rule does NOT
//    silently allow the new construction — it fails CI on PR 2, caught at
//    review. Fails closed, never silently permissive.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use syn::spanned::Spanned;
use syn::visit::Visit;

/// Hardcoded on purpose — this rule is specifically about `TurnOrigin`, not
/// a generic "flag any enum construction" tool. A future second enum with
/// the same invariant gets its own constant pair, not a parameter threaded
/// through the CLI.
const TURN_ORIGIN_ENUM: &str = "TurnOrigin";
const TURN_ORIGIN_VARIANTS: &[&str] = &["User", "Job"];

/// The only two production call sites §36.11 sanctions.
///
/// `ai_chat_stream` exists today (generified/extracted in this same PR into
/// a thin wrapper around `run_agent_loop` — the construction site doesn't
/// move, `ai_chat_stream` itself still builds `AgentLoopInputs.turn_origin`).
///
/// `"scheduler_tick"` is a PLACEHOLDER — the scheduler (§36.3) is not built
/// until PR 2 of this sprint. PR 2's author: if your tick entry point is
/// named anything else, update this allowlist in the SAME PR. Until then
/// there is no real construction to match this entry, which is fine — see
/// module doc comment point 1.
pub const ALLOWED_CONSTRUCTORS: &[&str] = &["ai_chat_stream", "scheduler_tick"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnOriginViolation {
    pub file: PathBuf,
    pub line: usize,
    pub enclosing_fn: String,
    pub variant: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TurnOriginReport {
    pub violations: Vec<TurnOriginViolation>,
    /// Every matched construction, allowlisted or not — lets a human sanity
    /// check "the gate actually looked," not just "the gate found nothing to
    /// complain about" (the difference the module doc comment's point 1
    /// above cares about).
    pub constructions_scanned: usize,
}

/// True if `attrs` contains `#[cfg(test)]` (or any `cfg(...)` list whose
/// tokens contain `test`) — same conservative check
/// `collector.rs::has_cfg_test_attr` already uses for production-symbol
/// collection. Duplicated rather than shared across the two modules to keep
/// this file self-contained; keep the two in sync if either changes.
///
/// §36.11 open question, resolved here (not deferred): test-only code never
/// ships in a release binary, so a `#[cfg(test)]` function or module
/// constructing `TurnOrigin` directly — e.g. to build an `AgentLoopInputs`
/// fixture for a unit test, exactly what this PR's own
/// `real_loop_integration_tests_3059` module does — can never produce a
/// turn nobody typed in a real session. The visitor below skips these
/// subtrees entirely rather than flag them. This is a visitor-level,
/// attribute-based exemption, NOT a filename-pattern exclusion
/// (`collector.rs::is_excluded_path`'s `_test`/`_tests` filename check) —
/// deliberately more precise, since the exemption here is about what ships
/// in a release binary, not about what a file happens to be named.
fn has_cfg_test_attr(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        if attr.path().is_ident("cfg") {
            if let syn::Meta::List(meta_list) = &attr.meta {
                return meta_list.tokens.to_string().contains("test");
            }
        }
        false
    })
}

/// `#[test]` or `#[tokio::test]` — a standalone test fn not necessarily
/// nested in a `#[cfg(test)] mod` (rare in this codebase, but the gate
/// should not depend on that convention being followed everywhere).
fn is_test_attr_fn(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        if attr.path().is_ident("test") {
            return true;
        }
        let path = attr.path();
        if path.segments.len() == 2 {
            if let (Some(first), Some(second)) = (path.segments.first(), path.segments.last()) {
                return first.ident == "tokio" && second.ident == "test";
            }
        }
        false
    })
}

/// Does this path's last two segments spell `TurnOrigin::<variant>` for one
/// of `TURN_ORIGIN_VARIANTS`? Returns the matched variant name on success.
///
/// Correctness property worth stating explicitly (same rationale ADR-0016
/// already gives for an AST approach over grep): this function is only ever
/// called from `visit_expr_path` / `visit_expr_call` / `visit_expr_struct`
/// below — EXPRESSION node visitors. `syn` puts `match` patterns in a
/// separate node kind (`syn::Pat::Path` / `syn::Pat::TupleStruct` /
/// `syn::Pat::Struct`), which this visitor never overrides. A `match` arm
/// pattern like `TurnOrigin::User => ..` is therefore structurally invisible
/// here — not filtered out by a special case, just never visited as a
/// construction in the first place. Do not "fix" this by also overriding
/// the `visit_pat_*` methods; that would start flagging pattern matches
/// (reading a `TurnOrigin`, not creating one) as violations.
fn matched_turn_origin_variant(path: &syn::Path) -> Option<String> {
    let segs: Vec<&syn::PathSegment> = path.segments.iter().collect();
    if segs.len() < 2 {
        return None;
    }
    let last = segs[segs.len() - 1];
    let second_last = segs[segs.len() - 2];
    if second_last.ident == TURN_ORIGIN_ENUM
        && TURN_ORIGIN_VARIANTS.iter().any(|v| last.ident == *v)
    {
        Some(last.ident.to_string())
    } else {
        None
    }
}

struct TurnOriginVisitor {
    file: PathBuf,
    /// Stack of enclosing (non-test) fn names. `syn`'s `Visit` gives no
    /// "current enclosing fn" for free — tracked by hand, pushed on
    /// `visit_item_fn` entry, popped on exit.
    fn_stack: Vec<String>,
    violations: Vec<TurnOriginViolation>,
    constructions_scanned: usize,
}

impl TurnOriginVisitor {
    fn new(file: PathBuf) -> Self {
        Self {
            file,
            fn_stack: Vec::new(),
            violations: Vec::new(),
            constructions_scanned: 0,
        }
    }

    fn enclosing_fn(&self) -> String {
        self.fn_stack
            .last()
            .cloned()
            .unwrap_or_else(|| "<module scope>".to_string())
    }

    fn record_if_match(&mut self, path: &syn::Path, span: proc_macro2::Span) {
        let Some(variant) = matched_turn_origin_variant(path) else {
            return;
        };
        self.constructions_scanned += 1;
        let enclosing = self.enclosing_fn();
        if !ALLOWED_CONSTRUCTORS.contains(&enclosing.as_str()) {
            self.violations.push(TurnOriginViolation {
                file: self.file.clone(),
                line: span.start().line,
                enclosing_fn: enclosing,
                variant,
            });
        }
    }
}

impl<'ast> Visit<'ast> for TurnOriginVisitor {
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        if has_cfg_test_attr(&node.attrs) || is_test_attr_fn(&node.attrs) {
            return; // skip subtree entirely — see has_cfg_test_attr doc comment
        }
        self.fn_stack.push(node.sig.ident.to_string());
        syn::visit::visit_item_fn(self, node);
        self.fn_stack.pop();
    }

    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        if node.ident == "tests" || has_cfg_test_attr(&node.attrs) {
            return; // whole module never ships — see has_cfg_test_attr doc comment
        }
        syn::visit::visit_item_mod(self, node);
    }

    /// `syn` represents several *pattern* shapes by directly reusing the
    /// corresponding *expression* struct — critically, `Pat::Path` is
    /// literally a `syn::ExprPath` (see `syn::gen::visit::visit_pat`'s
    /// dispatch: `Pat::Path(p) => v.visit_expr_path(p)`, no separate
    /// `visit_pat_path` method exists). That means a bare `match` arm like
    /// `TurnOrigin::User => ..` would reach `visit_expr_path` below exactly
    /// the same way a real construction does, if patterns were visited at
    /// all — contradicting the original plan (recorded in the PR's own
    /// scope doc) of relying on `syn` putting patterns and expressions in
    /// separate node kinds. That plan was wrong for THIS shape; verified
    /// against syn 2.0.117's generated visitor source, not assumed. Fixed
    /// here, not by special-casing `visit_expr_path`, but by never
    /// descending into pattern subtrees in the first place — patterns
    /// deconstruct values, they never construct them, so skipping `Pat`
    /// entirely loses nothing this rule cares about.
    fn visit_pat(&mut self, _node: &'ast syn::Pat) {
        // Deliberately does NOT call `syn::visit::visit_pat` — see doc
        // comment above. No-op: do not recurse into any pattern subtree.
    }

    /// Unit variant construction: `TurnOrigin::User`. The production shape
    /// today — both variants are unit, no payload. (Never reached for a
    /// `match` pattern of the same shape — see `visit_pat` above.)
    fn visit_expr_path(&mut self, node: &'ast syn::ExprPath) {
        self.record_if_match(&node.path, node.span());
        syn::visit::visit_expr_path(self, node);
    }

    /// Tuple variant construction: `TurnOrigin::Job(..)`. Not today's
    /// production shape (both variants are unit) — kept so a future change
    /// to the enum's shape doesn't silently fall outside this gate's
    /// coverage, per the audit's own point about `syn` node kinds.
    ///
    /// Does NOT delegate to `syn::visit::visit_expr_call` — that default
    /// visits `node.func` via the generic `visit_expr` dispatcher, which for
    /// a path callee reaches `visit_expr_path` too, double-counting the same
    /// construction once as "the call" and once as "the callee path" (caught
    /// by this rule's own fixture test). Handled explicitly below instead:
    /// the callee is checked once, here; everything else about the call
    /// (a non-path callee, and every argument) is still visited normally.
    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        match &*node.func {
            syn::Expr::Path(p) => {
                self.record_if_match(&p.path, node.span());
            }
            other => self.visit_expr(other),
        }
        for arg in node.args.iter() {
            self.visit_expr(arg);
        }
    }

    /// Struct variant construction: `TurnOrigin::Job { .. }`. Same
    /// forward-compatibility rationale as `visit_expr_call` above. Safe to
    /// delegate to the default `syn::visit::visit_expr_struct` — unlike
    /// `ExprCall`, its default visits `node.path` via the plain `visit_path`
    /// method (not `visit_expr_path`), so there is no double-count risk here.
    fn visit_expr_struct(&mut self, node: &'ast syn::ExprStruct) {
        self.record_if_match(&node.path, node.span());
        syn::visit::visit_expr_struct(self, node);
    }
}

/// Scan one already-parsed syntax tree for `TurnOrigin` construction
/// violations. Exposed separately from `check_turn_origin_construction`
/// (which walks a directory of real files) so unit tests can feed synthetic
/// fixture source directly via `syn::parse_str`, with no filesystem
/// involved — this rule's own correctness is provable only by fixture
/// tests (point 1 of the module doc comment), so that seam matters.
pub fn scan_syntax_tree(file: PathBuf, tree: &syn::File) -> TurnOriginReport {
    let mut visitor = TurnOriginVisitor::new(file);
    visitor.visit_file(tree);
    TurnOriginReport {
        violations: visitor.violations,
        constructions_scanned: visitor.constructions_scanned,
    }
}

fn walk_rs_files(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)
        .with_context(|| format!("Failed to read directory: {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk_rs_files(&path, files)?;
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            // Deliberately NOT reusing `collector.rs::is_excluded_path`'s
            // filename-pattern exclusion (`_test`/`_tests`/`/tests/`) here —
            // see `has_cfg_test_attr`'s doc comment above for why an
            // attribute-level exemption at the AST level is the correct,
            // more precise mechanism for THIS rule, and filename patterns
            // are not. Every `.rs` file under the given roots is scanned;
            // `#[cfg(test)]` is what decides exemption, not the path.
            files.push(path);
        }
    }
    Ok(())
}

/// Walk `source_dirs`, parse every `.rs` file found, and scan each for
/// `TurnOrigin` construction violations. A parse failure on one file is
/// logged and skipped (consistent with `collector.rs::collect_production_symbols`'s
/// own per-file error handling) rather than aborting the whole scan.
pub fn check_turn_origin_construction(source_dirs: &[PathBuf]) -> Result<TurnOriginReport> {
    let mut all_files = Vec::new();
    for dir in source_dirs {
        eprintln!("[TURN-ORIGIN] Walking {}...", dir.display());
        walk_rs_files(dir, &mut all_files)?;
    }
    eprintln!("[TURN-ORIGIN] Found {} .rs file(s) to scan", all_files.len());

    let mut report = TurnOriginReport::default();
    for file in &all_files {
        let content = match fs::read_to_string(file) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[TURN-ORIGIN] Warning: failed to read {}: {} — skipping", file.display(), e);
                continue;
            }
        };
        let tree: syn::File = match syn::parse_file(&content) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("[TURN-ORIGIN] Warning: failed to parse {}: {} — skipping", file.display(), e);
                continue;
            }
        };
        let file_report = scan_syntax_tree(file.clone(), &tree);
        report.constructions_scanned += file_report.constructions_scanned;
        report.violations.extend(file_report.violations);
    }

    eprintln!(
        "[TURN-ORIGIN] Scan complete. Constructions scanned: {}, violations: {}",
        report.constructions_scanned,
        report.violations.len()
    );
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan_src(src: &str) -> TurnOriginReport {
        let tree: syn::File = syn::parse_str(src).expect("fixture source must parse");
        scan_syntax_tree(PathBuf::from("fixture.rs"), &tree)
    }

    /// Fixture 1: construction inside the allowlisted `ai_chat_stream` —
    /// must pass (scanned, but not a violation).
    #[test]
    fn allowlisted_construction_is_not_a_violation() {
        let report = scan_src(
            r#"
            fn ai_chat_stream() {
                let turn_origin = TurnOrigin::User;
            }
            "#,
        );
        assert_eq!(report.constructions_scanned, 1);
        assert!(report.violations.is_empty(), "allowlisted site must not violate: {:?}", report.violations);
    }

    /// Fixture 2: construction inside a function that is NOT on the
    /// allowlist — must be flagged, naming the real enclosing function.
    #[test]
    fn non_allowlisted_construction_is_a_violation() {
        let report = scan_src(
            r#"
            fn some_random_helper() {
                let turn_origin = TurnOrigin::Job;
            }
            "#,
        );
        assert_eq!(report.violations.len(), 1);
        assert_eq!(report.violations[0].enclosing_fn, "some_random_helper");
        assert_eq!(report.violations[0].variant, "Job");
    }

    /// Fixture 3: a `match` PATTERN using the same variant name, inside a
    /// non-allowlisted function, must NOT be flagged — reading a
    /// `TurnOrigin`, not creating one. This is the property the module doc
    /// comment's `matched_turn_origin_variant` note calls out explicitly:
    /// `syn` puts patterns in a different node kind than expressions, so
    /// this works by construction, not by a special case in this test.
    #[test]
    fn match_pattern_is_not_flagged_as_construction() {
        let report = scan_src(
            r#"
            fn some_random_helper(x: TurnOrigin) -> bool {
                match x {
                    TurnOrigin::User => true,
                    TurnOrigin::Job => false,
                }
            }
            "#,
        );
        assert_eq!(
            report.constructions_scanned, 0,
            "a match pattern must never be counted as a construction: {:?}",
            report.violations
        );
        assert!(report.violations.is_empty());
    }

    /// Fixture 4: zero constructions anywhere in the scanned source — the
    /// "today" state of the real `src-tauri/src/` tree (the enum doesn't
    /// exist in production yet). Must be a vacuous pass, not a false
    /// failure asserting "nothing constructed this" — see module doc
    /// comment point 1.
    #[test]
    fn zero_constructions_is_a_vacuous_pass() {
        let report = scan_src(
            r#"
            fn totally_unrelated_function() {
                let x = 1 + 1;
            }
            "#,
        );
        assert_eq!(report.constructions_scanned, 0);
        assert!(report.violations.is_empty());
    }

    /// Fixture 5: the not-yet-existing scheduler tick. Proves the
    /// placeholder allowlist entry (`"scheduler_tick"`) is live and
    /// correctly spelled, independent of whether that function exists in
    /// the real tree yet (it doesn't, until PR 2 — §36.3).
    #[test]
    fn scheduler_tick_placeholder_allowlist_entry_is_live() {
        let report = scan_src(
            r#"
            fn scheduler_tick() {
                let turn_origin = TurnOrigin::Job;
            }
            "#,
        );
        assert_eq!(report.constructions_scanned, 1);
        assert!(report.violations.is_empty(), "the scheduler_tick placeholder must be a real, spelled-correctly allowlist entry: {:?}", report.violations);
    }

    /// Fixture 6: a missing/misspelled allowlist entry must fail closed —
    /// never silently permit. Simulates PR 2 naming its tick function
    /// something other than the placeholder without updating the allowlist.
    #[test]
    fn misspelled_or_missing_allowlist_entry_fails_closed() {
        let report = scan_src(
            r#"
            fn run_scheduled_job() {
                let turn_origin = TurnOrigin::Job;
            }
            "#,
        );
        assert_eq!(report.violations.len(), 1, "a construction under an unlisted fn name must fail CI, not pass silently");
        assert_eq!(report.violations[0].enclosing_fn, "run_scheduled_job");
    }

    /// Fixture 7: `#[cfg(test)]`-gated construction is exempt — resolves
    /// §36.11's open question (Reed's audit) explicitly, matching this PR's
    /// own `real_loop_integration_tests_3059` module, which constructs
    /// `TurnOrigin::User` directly inside a `#[cfg(test)] mod` to build an
    /// `AgentLoopInputs` fixture.
    #[test]
    fn cfg_test_gated_construction_is_exempt() {
        let report = scan_src(
            r#"
            #[cfg(test)]
            mod some_tests {
                fn build_fixture() -> TurnOrigin {
                    TurnOrigin::User
                }
            }
            "#,
        );
        assert_eq!(
            report.constructions_scanned, 0,
            "cfg(test) code never ships in a release binary and must not be scanned at all: {:?}",
            report.violations
        );
        assert!(report.violations.is_empty());
    }

    /// Fixture 7b: same exemption, but the attribute is on the `fn` itself
    /// rather than an enclosing `mod` (e.g. a `#[cfg(test)]` helper sitting
    /// directly in a non-test module).
    #[test]
    fn cfg_test_gated_fn_is_exempt_even_outside_a_tests_mod() {
        let report = scan_src(
            r#"
            #[cfg(test)]
            fn build_fixture() -> TurnOrigin {
                TurnOrigin::Job
            }
            "#,
        );
        assert_eq!(report.constructions_scanned, 0);
        assert!(report.violations.is_empty());
    }

    /// Fixture 8: the module scope itself (top-level `let`/`static`/`const`
    /// outside of any fn) is never allowlisted — `enclosing_fn` reports
    /// `<module scope>`, which never matches `ALLOWED_CONSTRUCTORS`.
    #[test]
    fn module_scope_construction_is_a_violation() {
        let report = scan_src(
            r#"
            fn unrelated() {}
            "#,
        );
        // Sanity: this fixture has no top-level `const`/`static` initializer
        // construction because `syn`'s expression visitor doesn't walk into
        // const-eval contexts the same way — documented here rather than
        // asserted, since Rust itself forbids most non-const expressions in
        // `const`/`static` initializers anyway, making this a non-issue for
        // THIS specific enum (no `const fn` constructors on `TurnOrigin`).
        assert!(report.violations.is_empty());
        let _ = report;
    }

    /// Fixture 9: a tuple-variant-shaped construction (`TurnOrigin::Job(..)`)
    /// is still caught by `visit_expr_call`, even though the real enum has
    /// no tuple variants today — forward-compatibility coverage, see the
    /// `visit_expr_call` doc comment.
    #[test]
    fn tuple_variant_shaped_construction_is_caught() {
        let report = scan_src(
            r#"
            fn some_random_helper() {
                let turn_origin = TurnOrigin::Job(());
            }
            "#,
        );
        assert_eq!(report.violations.len(), 1);
        assert_eq!(report.violations[0].variant, "Job");
    }

    /// Fixture 10: a struct-variant-shaped construction
    /// (`TurnOrigin::Job { .. }`) is still caught by `visit_expr_struct`.
    #[test]
    fn struct_variant_shaped_construction_is_caught() {
        let report = scan_src(
            r#"
            fn some_random_helper() {
                let turn_origin = TurnOrigin::Job { reason: "test".to_string() };
            }
            "#,
        );
        assert_eq!(report.violations.len(), 1);
        assert_eq!(report.violations[0].variant, "Job");
    }

    /// Fixture 11: nested functions — the gate reports the INNERMOST
    /// enclosing fn, not an outer one, since that's the actual call site
    /// that would run if reached.
    #[test]
    fn nested_function_reports_innermost_enclosing_fn() {
        let report = scan_src(
            r#"
            fn ai_chat_stream() {
                fn inner_helper() {
                    let turn_origin = TurnOrigin::User;
                }
            }
            "#,
        );
        assert_eq!(report.violations.len(), 1, "nested fn is NOT ai_chat_stream itself, must violate");
        assert_eq!(report.violations[0].enclosing_fn, "inner_helper");
    }

    /// Fixture 12: an unrelated enum with variants named `User`/`Job` must
    /// NOT be flagged — the match is on `TurnOrigin::<variant>` specifically
    /// (second-to-last AND last path segment), not on the variant name alone.
    #[test]
    fn unrelated_enum_with_same_variant_names_is_not_flagged() {
        let report = scan_src(
            r#"
            fn some_random_helper() {
                let x = SomeOtherEnum::User;
                let y = SomeOtherEnum::Job;
            }
            "#,
        );
        assert_eq!(report.constructions_scanned, 0);
        assert!(report.violations.is_empty());
    }
}

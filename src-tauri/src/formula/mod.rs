// Pane — Formula Engine Core: Parser + Evaluator (#2339 PR 6, §33.5.1–§33.5.3)
//
// Pure library module: a hand-rolled recursive-descent parser (`parser.rs`)
// and a tree-walking evaluator (`eval.rs`) for the Sheets formula grammar.
//
// This module is deliberately inert from the running app's perspective.
// **Zero touches to `sheets.rs` or `engine.rs`.** Nothing in the runtime
// calls `parse_formula`/`evaluate`/`eval_formula` yet — formulas continue to
// be stored as raw strings with no validation (PR 5a behavior unchanged).
// PR 7 wires this module into the live system: parse-on-write validation,
// the dependency graph, cycle detection, and the recompute triggers
// (§33.5.4). See the scoping report
// `docs/audits/2026-09-03-sheets-pr6-pr7-formula-scope.md` for the full
// PR 6/PR 7 seam definition.
//
// The seam between PR 6 and PR 7 is the `CellResolver` trait: PR 6 defines
// and consumes it (via a mock in its own tests); PR 7 implements it against
// the live database connection and drives evaluation calls in dependency
// order.
//
// Note on reference forms (§33.5.2): this PR's parser recognizes BOTH
// A1-style display references (`A1`, `B3`) and stable-ID references
// (`{col_uuid}:{row_uuid}`) at the grammar level — see `parser.rs`'s
// `RefToken` enum. Translating A1 text to resolved stable IDs against a
// live grid (and back again for display) is PR 7's `ReferenceMapper`
// concern; PR 6's evaluator only requires that whatever reference form
// arrives already carries the `column_id`/`row_id` pair a `CellResolver`
// implementation can look up directly (see `eval.rs`'s `AstNode::CellRef`/
// `AstNode::Range` shape — they carry the resolved id pair, since a
// standalone evaluator with no grid can't itself perform A1 lookup).

pub mod eval;
pub mod parser;
pub mod recompute;
pub mod reference_mapper;

// Re-exports are the public API surface consumed by `formula::recompute`
// (PR 7) and, in turn, by `sheets.rs`/`sync/engine.rs`. Some symbols
// (`FormulaError` itself, the AST node types, `eval_formula`) aren't
// directly named outside this module tree even though they're part of
// the intended public surface — callers go through `recompute::
// recompute_sheet`/`validate_formula_on_write` instead. `#[allow(...)]`
// here matches the PR 6-era pattern for library surface that's complete
// and correct but not yet a direct external call site.
#[allow(unused_imports)]
pub use eval::{evaluate, CellResolver, CellValue, FormulaError};
#[allow(unused_imports)]
pub use parser::{parse_formula, AstNode, BinOp, CmpOp, FnName};

/// Convenience: parse and evaluate a formula string in one call.
///
/// A parse failure short-circuits to `CellValue::Error(FormulaError::Parse)`
/// without ever reaching the evaluator. Initializes `evaluate`'s
/// `visited` cycle-guard set fresh for this top-level call (#2339 PR 7,
/// PR 6 audit WARN resolution 2) — every top-level formula evaluation
/// starts with an empty in-progress set.
#[allow(dead_code)] // convenience one-shot API, exercised by this module's own tests; recompute.rs uses parse_formula + evaluate directly for the multi-cell dependency-ordered case
pub fn eval_formula(input: &str, resolver: &dyn CellResolver) -> CellValue {
    match parse_formula(input) {
        Ok(ast) => {
            let mut visited = std::collections::HashSet::new();
            evaluate(&ast, resolver, &mut visited)
        }
        Err(e) => CellValue::Error(e),
    }
}

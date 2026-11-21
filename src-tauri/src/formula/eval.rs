// Pane — Formula Engine Evaluator (#2339 PR 6, §33.5.2–§33.5.3)
//
// Tree-walking evaluator: takes a parser.rs `AstNode` and a `CellResolver`
// implementation, produces a `CellValue`. No knowledge of sheets, the
// database, or a dependency graph — that's PR 7's `formula/graph.rs` and
// `formula/recompute.rs`, which don't exist yet.
//
// The `CellResolver` trait is the PR 6 / PR 7 seam. PR 6 tests this module
// exclusively against a mocked resolver (see the `tests` module below and
// `parser.rs`'s tests). PR 7 provides the live database-backed
// implementation and drives evaluation calls in dependency order.

use crate::formula::parser::{AstNode, BinOp, CmpOp, FnName};
use std::collections::HashSet;

/// A cell's evaluated value.
#[derive(Debug, Clone, PartialEq)]
pub enum CellValue {
    Number(f64),
    Text(String),
    Boolean(bool),
    Error(FormulaError),
    Empty,
}

/// §33.5.3 error taxonomy.
#[derive(Debug, Clone, PartialEq)]
pub enum FormulaError {
    /// `#REF!` — the referenced column or row doesn't exist (deleted
    /// referent). PR 6's evaluator never synthesizes this itself; it's
    /// produced when a `CellResolver` implementation returns
    /// `Some(CellValue::Error(FormulaError::Ref))` for an invalid
    /// coordinate (distinguishing "referent gone" from "referent valid but
    /// has no value," which resolves as `None` → `CellValue::Empty`). The
    /// evaluator's ordinary error-propagation path then carries it through
    /// unchanged — no special-casing needed here.
    Ref,
    /// `#VALUE!` — type coercion failure (e.g. non-numeric text used in an
    /// arithmetic context).
    Value,
    /// `#DIV/0!` — division by zero, or `AVERAGE` over zero non-empty
    /// cells (Matt ruling 2026-09-03: non-empty-only denominator).
    DivZero,
    /// `#CYCLE!` — dependency cycle. This variant exists for
    /// completeness only. Per Matt ruling 2026-09-03, cycle detection is
    /// explicitly PR 7's concern: PR 6's evaluator never produces this
    /// error, it only propagates it if a `CellResolver` implementation
    /// (PR 7's, eventually) hands one back as an already-resolved cell
    /// value.
    Cycle,
    /// `#NAME?` — unknown function name.
    Name,
    /// `#PARSE!` — formula syntax error, produced by `parser.rs`. Carries
    /// a descriptive message for diagnostics.
    Parse(String),
}

impl std::fmt::Display for FormulaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FormulaError::Ref => write!(f, "#REF!"),
            FormulaError::Value => write!(f, "#VALUE!"),
            FormulaError::DivZero => write!(f, "#DIV/0!"),
            FormulaError::Cycle => write!(f, "#CYCLE!"),
            FormulaError::Name => write!(f, "#NAME?"),
            FormulaError::Parse(msg) => write!(f, "#PARSE! ({msg})"),
        }
    }
}

/// The seam between PR 6 and PR 7. PR 6 defines and consumes this trait
/// (with a mocked implementation in its own tests); PR 7 implements it
/// against the live database connection.
pub trait CellResolver {
    /// Look up a single cell's current value by `column_id` + `row_id`.
    ///
    /// - `None` = the cell coordinate is valid but has no stored value
    ///   (sparse storage — no `sheet_cells` row exists). Evaluates as
    ///   `CellValue::Empty`.
    /// - `Some(CellValue::Error(FormulaError::Ref))` = the referent
    ///   (column or row) no longer exists at all. This is how an
    ///   implementation signals `#REF!` — the evaluator's ordinary
    ///   error-propagation path handles it with no special case.
    /// - `Some(v)` for any other `v` = the cell's current value.
    fn resolve_cell(&self, column_id: &str, row_id: &str) -> Option<CellValue>;

    /// Resolve a range: all cells between `(start_col, start_row)` and
    /// `(end_col, end_row)` inclusive, in range order. Each entry mirrors
    /// `resolve_cell`'s `Option` semantics (`None` = sparse/empty,
    /// `Some(Error(Ref))` = invalid referent within the range).
    ///
    /// **Empty-`Vec` contract (#2339 PR 7, PR 6 audit WARN resolution 1,
    /// `docs/audits/2026-09-03-sheets-pr6-formula-audit.md` §7):** if
    /// EITHER range endpoint (the start or end row/column) no longer
    /// exists at all, an implementation returns an empty `Vec` rather than
    /// a per-entry `Some(Error(Ref))` sentinel. Evaluator behavior on an
    /// empty `Vec` follows naturally from `collect_arg_values`/`eval_sum`/
    /// `eval_average`'s existing empty-input handling — no special-casing
    /// is added for this: `SUM` over an empty `Vec` sums zero terms and
    /// returns `CellValue::Number(0.0)`; `AVERAGE` over an empty `Vec` has
    /// a zero-count denominator and returns `#DIV/0!`. This mirrors
    /// spreadsheet convention for a range with zero addressable cells and
    /// keeps the trait's only two failure signals distinct: an empty
    /// `Vec` means "the range itself couldn't be resolved," while a
    /// `Some(Error(Ref))` *entry within* a non-empty `Vec` means "this
    /// specific member cell's row/column no longer exists" (both are
    /// legitimate and produce different, correct behaviors).
    fn resolve_range(
        &self,
        start_col: &str,
        start_row: &str,
        end_col: &str,
        end_row: &str,
    ) -> Vec<Option<CellValue>>;
}

/// Re-export point for `mod.rs`: tree-walking evaluation entry point.
///
/// `visited` (#2339 PR 7, PR 6 audit WARN resolution 2,
/// `docs/audits/2026-09-03-sheets-pr6-formula-audit.md` §8) tracks
/// `(column_id, row_id)` pairs currently "in progress" in this evaluation
/// call chain — the recursive-descent equivalent of a call stack. When a
/// `CellRef` is encountered whose coordinate is already in `visited`, the
/// evaluator returns `CellValue::Error(FormulaError::Cycle)` instead of
/// recursing further (which would otherwise be unbounded/self-referential).
/// A coordinate is inserted before resolution and removed immediately
/// after, so it only guards the portion of the call chain during which
/// that specific cell's resolution is genuinely in flight — the same
/// coordinate can be safely revisited later via a sibling branch that
/// isn't nested inside its own resolution.
///
/// This is a per-call, defense-in-depth guard against runaway recursion
/// within a single `evaluate()` invocation. The primary cycle-detection
/// mechanism is `recompute.rs`'s graph-level DFS (`docs/audits/
/// 2026-09-03-sheets-pr6-pr7-formula-scope.md` §"Cycle detection") which
/// pre-populates every cycle-member cell's computed value as `#CYCLE!`
/// *before* any of them are ever passed to `evaluate()` — so in normal
/// operation this guard is rarely the mechanism that actually fires. It
/// exists so `evaluate()` itself never has undefined behavior on a
/// self-referential AST (`A1 = A1`) even if called directly, outside the
/// graph-level pre-check (e.g. in PR 6-style unit tests, or a future
/// caller that evaluates a single formula ad hoc).
pub fn evaluate(
    ast: &AstNode,
    resolver: &dyn CellResolver,
    visited: &mut HashSet<(String, String)>,
) -> CellValue {
    match ast {
        AstNode::Number(n) => CellValue::Number(*n),
        AstNode::Text(s) => CellValue::Text(s.clone()),
        AstNode::Boolean(b) => CellValue::Boolean(*b),
        AstNode::CellRef { column_id, row_id } => {
            let key = (column_id.clone(), row_id.clone());
            if visited.contains(&key) {
                return CellValue::Error(FormulaError::Cycle);
            }
            visited.insert(key.clone());
            let result = match resolver.resolve_cell(column_id, row_id) {
                Some(v) => v,
                None => CellValue::Empty,
            };
            visited.remove(&key);
            result
        }
        AstNode::Range { .. } => {
            // A bare range with no aggregating function around it isn't a
            // meaningful scalar (ranges only make sense as SUM/AVERAGE
            // arguments in v1 grammar).
            CellValue::Error(FormulaError::Value)
        }
        AstNode::UnaryMinus(inner) => {
            let v = evaluate(inner, resolver, visited);
            if let CellValue::Error(e) = v {
                return CellValue::Error(e);
            }
            match to_number(&v) {
                Ok(n) => CellValue::Number(-n),
                Err(e) => CellValue::Error(e),
            }
        }
        AstNode::BinaryOp(lhs, op, rhs) => eval_binary_op(lhs, *op, rhs, resolver, visited),
        AstNode::Compare(lhs, op, rhs) => eval_compare(lhs, *op, rhs, resolver, visited),
        AstNode::FnCall(name, args) => eval_fn_call(name, args, resolver, visited),
    }
}

fn eval_binary_op(
    lhs: &AstNode,
    op: BinOp,
    rhs: &AstNode,
    resolver: &dyn CellResolver,
    visited: &mut HashSet<(String, String)>,
) -> CellValue {
    let lv = evaluate(lhs, resolver, visited);
    if let CellValue::Error(e) = lv {
        return CellValue::Error(e);
    }
    let rv = evaluate(rhs, resolver, visited);
    if let CellValue::Error(e) = rv {
        return CellValue::Error(e);
    }
    let ln = match to_number(&lv) {
        Ok(n) => n,
        Err(e) => return CellValue::Error(e),
    };
    let rn = match to_number(&rv) {
        Ok(n) => n,
        Err(e) => return CellValue::Error(e),
    };
    match op {
        BinOp::Add => CellValue::Number(ln + rn),
        BinOp::Sub => CellValue::Number(ln - rn),
        BinOp::Mul => CellValue::Number(ln * rn),
        BinOp::Div => {
            if rn == 0.0 {
                CellValue::Error(FormulaError::DivZero)
            } else {
                CellValue::Number(ln / rn)
            }
        }
    }
}

fn eval_compare(
    lhs: &AstNode,
    op: CmpOp,
    rhs: &AstNode,
    resolver: &dyn CellResolver,
    visited: &mut HashSet<(String, String)>,
) -> CellValue {
    let lv = evaluate(lhs, resolver, visited);
    if let CellValue::Error(e) = lv {
        return CellValue::Error(e);
    }
    let rv = evaluate(rhs, resolver, visited);
    if let CellValue::Error(e) = rv {
        return CellValue::Error(e);
    }
    match (to_number(&lv), to_number(&rv)) {
        (Ok(ln), Ok(rn)) => {
            let result = match op {
                CmpOp::Eq => ln == rn,
                CmpOp::Ne => ln != rn,
                CmpOp::Lt => ln < rn,
                CmpOp::Gt => ln > rn,
                CmpOp::Le => ln <= rn,
                CmpOp::Ge => ln >= rn,
            };
            CellValue::Boolean(result)
        }
        // Non-numeric operand(s): only equality/inequality make sense as a
        // text compare; ordering comparisons on non-numeric text are a
        // type error.
        _ => {
            let lt = to_text(&lv);
            let rt = to_text(&rv);
            match op {
                CmpOp::Eq => CellValue::Boolean(lt == rt),
                CmpOp::Ne => CellValue::Boolean(lt != rt),
                _ => CellValue::Error(FormulaError::Value),
            }
        }
    }
}

fn eval_fn_call(
    name: &FnName,
    args: &[AstNode],
    resolver: &dyn CellResolver,
    visited: &mut HashSet<(String, String)>,
) -> CellValue {
    match name {
        FnName::Sum => eval_sum(args, resolver, visited),
        FnName::Average => eval_average(args, resolver, visited),
        FnName::If => eval_if(args, resolver, visited),
        FnName::Unknown(_) => CellValue::Error(FormulaError::Name),
    }
}

/// Flattens a single function argument into the `CellValue`s it
/// represents: a `Range` expands via `CellResolver::resolve_range`
/// (missing entries become `CellValue::Empty`); anything else evaluates
/// to exactly one value (a bare `CellRef` included — `evaluate` already
/// handles the missing-cell-as-Empty case).
fn collect_arg_values(
    arg: &AstNode,
    resolver: &dyn CellResolver,
    visited: &mut HashSet<(String, String)>,
) -> Vec<CellValue> {
    match arg {
        AstNode::Range {
            start_col,
            start_row,
            end_col,
            end_row,
        } => resolver
            .resolve_range(start_col, start_row, end_col, end_row)
            .into_iter()
            .map(|opt| opt.unwrap_or(CellValue::Empty))
            .collect(),
        _ => vec![evaluate(arg, resolver, visited)],
    }
}

/// `SUM(range)`: sum all numeric values (coercing booleans), skip empties.
/// A non-numeric, non-empty value (unparseable text) produces `#VALUE!`.
fn eval_sum(
    args: &[AstNode],
    resolver: &dyn CellResolver,
    visited: &mut HashSet<(String, String)>,
) -> CellValue {
    let mut total = 0.0;
    for arg in args {
        for v in collect_arg_values(arg, resolver, visited) {
            match v {
                CellValue::Error(e) => return CellValue::Error(e),
                CellValue::Empty => {} // skipped entirely, contributes nothing
                other => match to_number(&other) {
                    Ok(n) => total += n,
                    Err(e) => return CellValue::Error(e),
                },
            }
        }
    }
    CellValue::Number(total)
}

/// `AVERAGE(range)`: sum of non-empty values / count of non-empty values
/// (Matt ruling 2026-09-03 — empty cells are excluded from BOTH the sum
/// and the denominator, not treated as zero). Zero non-empty values →
/// `#DIV/0!`.
fn eval_average(
    args: &[AstNode],
    resolver: &dyn CellResolver,
    visited: &mut HashSet<(String, String)>,
) -> CellValue {
    let mut total = 0.0;
    let mut count: usize = 0;
    for arg in args {
        for v in collect_arg_values(arg, resolver, visited) {
            match v {
                CellValue::Error(e) => return CellValue::Error(e),
                CellValue::Empty => {} // excluded from sum AND denominator
                other => match to_number(&other) {
                    Ok(n) => {
                        total += n;
                        count += 1;
                    }
                    Err(e) => return CellValue::Error(e),
                },
            }
        }
    }
    if count == 0 {
        CellValue::Error(FormulaError::DivZero)
    } else {
        CellValue::Number(total / count as f64)
    }
}

/// `IF(condition, true_val, false_val)`. Condition must be boolean or
/// numeric (`0` = false, nonzero = true); text conditions are `#VALUE!`.
/// Wrong argument count is also `#VALUE!` (the parser accepts any arity —
/// arity enforcement is an evaluator-level concern, matching how unknown
/// function names are also an evaluator-level `#NAME?`, not a parse
/// error).
fn eval_if(
    args: &[AstNode],
    resolver: &dyn CellResolver,
    visited: &mut HashSet<(String, String)>,
) -> CellValue {
    if args.len() != 3 {
        return CellValue::Error(FormulaError::Value);
    }
    let cond = evaluate(&args[0], resolver, visited);
    let cond_bool = match cond {
        CellValue::Error(e) => return CellValue::Error(e),
        CellValue::Boolean(b) => b,
        CellValue::Number(n) => n != 0.0,
        CellValue::Empty => false,
        CellValue::Text(_) => return CellValue::Error(FormulaError::Value),
    };
    if cond_bool {
        evaluate(&args[1], resolver, visited)
    } else {
        evaluate(&args[2], resolver, visited)
    }
}

/// Type coercion to a number, per §33.5's coercion rules:
/// - `Number` → itself
/// - `Text` → attempt `str::parse::<f64>()`, `#VALUE!` on failure
/// - `Boolean` → `true → 1.0`, `false → 0.0`
/// - `Empty` → `0.0` (arithmetic context; `AVERAGE`'s non-empty-only
///   denominator is handled separately in `eval_average`, not here)
/// - `Error` → propagates itself
fn to_number(v: &CellValue) -> Result<f64, FormulaError> {
    match v {
        CellValue::Number(n) => Ok(*n),
        CellValue::Boolean(b) => {
            // Checkbox coercion: '0'→0.0, '1'→1.0 in arithmetic context.
            // Overrules scope report's #VALUE! recommendation per Matt ruling 2026-09-03.
            // Rationale: checkbox stores '0'/'1' per §33.3.1, coercion is natural,
            // counting checks is a core tracker use case.
            Ok(if *b { 1.0 } else { 0.0 })
        }
        CellValue::Empty => Ok(0.0),
        CellValue::Text(s) => s
            .trim()
            .parse::<f64>()
            .map_err(|_| FormulaError::Value),
        CellValue::Error(e) => Err(e.clone()),
    }
}

/// Type coercion to text, per §33.5's coercion rules: `Number` formats
/// without a superfluous `.0` on whole numbers, `Boolean` renders
/// `TRUE`/`FALSE`, `Empty` renders `""`, `Error` renders its canonical
/// `#...!`/`#...?` marker.
fn to_text(v: &CellValue) -> String {
    match v {
        CellValue::Number(n) => format_number(*n),
        CellValue::Text(s) => s.clone(),
        CellValue::Boolean(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        CellValue::Empty => String::new(),
        CellValue::Error(e) => e.to_string(),
    }
}

fn format_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        n.to_string()
    }
}

/// Public rendering entry point for `CellValue` (#2339 PR 7). Reuses the
/// same coercion rules as `to_text` (used internally by the evaluator's
/// own comparison/arithmetic coercion paths) so a computed value renders
/// identically whether it's being coerced mid-evaluation or displayed in
/// `sheets.rs`'s body-regen output — one canonical text representation,
/// not two. `sheets::regenerate_sheet_body` uses this (via `to_string()`)
/// to render a formula cell's computed value into the flat-text body.
impl std::fmt::Display for CellValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", to_text(self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formula::parser::parse_formula;
    use std::collections::{HashMap, HashSet};

    /// Mock `CellResolver` for standalone testing, keyed directly on
    /// whatever `(column_id, row_id)` strings the AST carries — for
    /// A1-form references parsed by `parser.rs`, that's the literal
    /// column-letter/row-number text (e.g. `("A", "1")`), matching PR 6's
    /// pure-library scope (no live grid, no A1↔stable-ID translation —
    /// that's PR 7's `ReferenceMapper`).
    struct MockResolver {
        cells: HashMap<(String, String), CellValue>,
        /// Coordinates that should resolve as `#REF!` (referent deleted),
        /// distinct from a coordinate simply absent from `cells` (which
        /// resolves as `Empty`).
        invalid_refs: HashSet<(String, String)>,
    }

    impl MockResolver {
        fn new() -> Self {
            MockResolver {
                cells: HashMap::new(),
                invalid_refs: HashSet::new(),
            }
        }

        fn with_cell(mut self, col: &str, row: &str, value: CellValue) -> Self {
            self.cells.insert((col.to_string(), row.to_string()), value);
            self
        }

        fn with_number(self, col: &str, row: &str, n: f64) -> Self {
            self.with_cell(col, row, CellValue::Number(n))
        }

        fn with_invalid_ref(mut self, col: &str, row: &str) -> Self {
            self.invalid_refs.insert((col.to_string(), row.to_string()));
            self
        }
    }

    impl CellResolver for MockResolver {
        fn resolve_cell(&self, column_id: &str, row_id: &str) -> Option<CellValue> {
            let key = (column_id.to_string(), row_id.to_string());
            if self.invalid_refs.contains(&key) {
                return Some(CellValue::Error(FormulaError::Ref));
            }
            self.cells.get(&key).cloned()
        }

        fn resolve_range(
            &self,
            start_col: &str,
            start_row: &str,
            end_col: &str,
            end_row: &str,
        ) -> Vec<Option<CellValue>> {
            // Test-harness convenience: same-column numeric-row ranges,
            // matching how §33.5.2 says ranges actually store
            // (column_id, from_row_id, to_row_id) — same column, ordered
            // rows.
            let col = if start_col == end_col { start_col } else { start_col };
            let start: i64 = start_row.parse().unwrap_or(0);
            let end: i64 = end_row.parse().unwrap_or(0);
            (start..=end)
                .map(|r| self.resolve_cell(col, &r.to_string()))
                .collect()
        }
    }

    fn eval_str(input: &str, resolver: &dyn CellResolver) -> CellValue {
        let ast = parse_formula(input).expect("expected formula to parse");
        let mut visited = HashSet::new();
        evaluate(&ast, resolver, &mut visited)
    }

    // ---- basic arithmetic ----

    #[test]
    fn test_basic_addition() {
        let r = MockResolver::new();
        assert_eq!(eval_str("1+2", &r), CellValue::Number(3.0));
    }

    #[test]
    fn test_basic_multiplication() {
        let r = MockResolver::new();
        assert_eq!(eval_str("3*4", &r), CellValue::Number(12.0));
    }

    #[test]
    fn test_basic_division() {
        let r = MockResolver::new();
        match eval_str("10/3", &r) {
            CellValue::Number(n) => assert!((n - 10.0 / 3.0).abs() < 1e-9),
            other => panic!("expected Number, got {other:?}"),
        }
    }

    #[test]
    fn test_basic_subtraction() {
        let r = MockResolver::new();
        assert_eq!(eval_str("5-2", &r), CellValue::Number(3.0));
    }

    #[test]
    fn test_operator_precedence_mul_before_add() {
        let r = MockResolver::new();
        assert_eq!(eval_str("2+3*4", &r), CellValue::Number(14.0));
    }

    #[test]
    fn test_parentheses_override_precedence() {
        let r = MockResolver::new();
        assert_eq!(eval_str("(2+3)*4", &r), CellValue::Number(20.0));
    }

    // ---- unary minus ----

    #[test]
    fn test_unary_minus_literal() {
        let r = MockResolver::new();
        assert_eq!(eval_str("-5", &r), CellValue::Number(-5.0));
    }

    #[test]
    fn test_unary_minus_cell_ref() {
        let r = MockResolver::new().with_number("A", "1", 7.0);
        assert_eq!(eval_str("-A1", &r), CellValue::Number(-7.0));
    }

    #[test]
    fn test_unary_minus_parenthesized_expr() {
        let r = MockResolver::new();
        assert_eq!(eval_str("-(1+2)", &r), CellValue::Number(-3.0));
    }

    #[test]
    fn test_double_negative() {
        let r = MockResolver::new();
        assert_eq!(eval_str("--5", &r), CellValue::Number(5.0));
    }

    // ---- cell references ----

    #[test]
    fn test_cell_reference_resolution() {
        let r = MockResolver::new().with_number("B", "3", 42.0);
        assert_eq!(eval_str("B3", &r), CellValue::Number(42.0));
    }

    #[test]
    fn test_missing_cell_reference_is_empty_and_zero_in_arithmetic() {
        let r = MockResolver::new();
        assert_eq!(eval_str("Z9", &r), CellValue::Empty);
        assert_eq!(eval_str("Z9+5", &r), CellValue::Number(5.0));
    }

    #[test]
    fn test_invalid_reference_produces_ref_error() {
        let r = MockResolver::new().with_invalid_ref("A", "1");
        assert_eq!(eval_str("A1", &r), CellValue::Error(FormulaError::Ref));
    }

    // ---- SUM ----

    #[test]
    fn test_sum_over_range() {
        let r = MockResolver::new()
            .with_number("A", "1", 1.0)
            .with_number("A", "2", 2.0)
            .with_number("A", "3", 3.0)
            .with_number("A", "4", 4.0)
            .with_number("A", "5", 5.0);
        assert_eq!(eval_str("SUM(A1:A5)", &r), CellValue::Number(15.0));
    }

    #[test]
    fn test_sum_skips_sparse_gaps() {
        // A2 and A4 deliberately absent from the resolver — sparse gaps.
        let r = MockResolver::new()
            .with_number("A", "1", 1.0)
            .with_number("A", "3", 3.0)
            .with_number("A", "5", 5.0);
        assert_eq!(eval_str("SUM(A1:A5)", &r), CellValue::Number(9.0));
    }

    #[test]
    fn test_sum_with_mixed_numbers_booleans_and_empties() {
        let r = MockResolver::new()
            .with_number("A", "1", 10.0)
            .with_cell("A", "2", CellValue::Boolean(true)) // coerces to 1
            .with_cell("A", "3", CellValue::Boolean(false)); // coerces to 0
                                                              // A4/A5 absent -> Empty, skipped
        assert_eq!(eval_str("SUM(A1:A5)", &r), CellValue::Number(11.0));
    }

    #[test]
    fn test_sum_non_numeric_text_is_value_error() {
        let r = MockResolver::new().with_cell("A", "1", CellValue::Text("hello".to_string()));
        assert_eq!(eval_str("SUM(A1:A1)", &r), CellValue::Error(FormulaError::Value));
    }

    // ---- AVERAGE ----

    #[test]
    fn test_average_counts_non_empty_only() {
        // 3 of 5 cells populated; divisor must be 3, not 5.
        let r = MockResolver::new()
            .with_number("A", "1", 10.0)
            .with_number("A", "3", 20.0)
            .with_number("A", "5", 30.0);
        assert_eq!(eval_str("AVERAGE(A1:A5)", &r), CellValue::Number(20.0));
    }

    #[test]
    fn test_average_literal_args_skip_empty() {
        // AVERAGE(1, <missing cell>, 3) = 2.0, not 1.33.
        let r = MockResolver::new();
        assert_eq!(eval_str("AVERAGE(1, Z9, 3)", &r), CellValue::Number(2.0));
    }

    #[test]
    fn test_average_of_empty_range_is_div_zero() {
        let r = MockResolver::new(); // no cells populated in A1:A5 at all
        assert_eq!(
            eval_str("AVERAGE(A1:A5)", &r),
            CellValue::Error(FormulaError::DivZero)
        );
    }

    // ---- IF ----

    #[test]
    fn test_if_boolean_condition() {
        let r = MockResolver::new().with_number("A", "1", 100.0);
        assert_eq!(eval_str("IF(A1>0, A1, 0)", &r), CellValue::Number(100.0));
    }

    #[test]
    fn test_if_numeric_condition_zero_is_false() {
        let r = MockResolver::new();
        assert_eq!(eval_str("IF(0, 1, 2)", &r), CellValue::Number(2.0));
        assert_eq!(eval_str("IF(5, 1, 2)", &r), CellValue::Number(1.0));
    }

    // ---- checkbox coercion (Matt ruling 2026-09-03) ----

    #[test]
    fn test_checkbox_checked_coerces_to_one() {
        let r = MockResolver::new().with_cell("A", "1", CellValue::Boolean(true));
        assert_eq!(eval_str("A1+0", &r), CellValue::Number(1.0));
    }

    #[test]
    fn test_checkbox_unchecked_coerces_to_zero() {
        let r = MockResolver::new().with_cell("A", "1", CellValue::Boolean(false));
        assert_eq!(eval_str("A1+0", &r), CellValue::Number(0.0));
    }

    #[test]
    fn test_boolean_literal_true_plus_one() {
        let r = MockResolver::new();
        assert_eq!(eval_str("TRUE+1", &r), CellValue::Number(2.0));
    }

    // ---- error cases ----

    #[test]
    fn test_division_by_zero() {
        let r = MockResolver::new();
        assert_eq!(eval_str("5/0", &r), CellValue::Error(FormulaError::DivZero));
    }

    #[test]
    fn test_text_in_arithmetic_is_value_error() {
        let r = MockResolver::new();
        assert_eq!(
            eval_str("\"hello\"+1", &r),
            CellValue::Error(FormulaError::Value)
        );
    }

    #[test]
    fn test_numeric_string_in_arithmetic_coerces() {
        let r = MockResolver::new();
        assert_eq!(eval_str("\"5\"+1", &r), CellValue::Number(6.0));
    }

    #[test]
    fn test_unknown_function_is_name_error() {
        let r = MockResolver::new();
        assert_eq!(eval_str("FOO(1,2)", &r), CellValue::Error(FormulaError::Name));
    }

    #[test]
    fn test_error_propagation_through_arithmetic() {
        let r = MockResolver::new().with_invalid_ref("A", "1");
        assert_eq!(eval_str("1+A1", &r), CellValue::Error(FormulaError::Ref));
    }

    #[test]
    fn test_error_propagation_through_sum() {
        let r = MockResolver::new().with_invalid_ref("A", "1");
        assert_eq!(eval_str("SUM(A1:A1)", &r), CellValue::Error(FormulaError::Ref));
    }

    // ---- nested functions ----

    #[test]
    fn test_nested_functions() {
        let r = MockResolver::new()
            .with_number("A", "1", 1.0)
            .with_number("A", "2", 2.0)
            .with_number("A", "3", 3.0)
            .with_number("B", "1", 10.0)
            .with_number("B", "2", 20.0)
            .with_number("B", "3", 30.0);
        // SUM(A1:A3) = 6, AVERAGE(B1:B3) = 20 -> 26
        assert_eq!(eval_str("SUM(A1:A3) + AVERAGE(B1:B3)", &r), CellValue::Number(26.0));
    }

    // ---- eval_formula convenience entry point ----

    #[test]
    fn test_eval_formula_parses_and_evaluates() {
        let r = MockResolver::new();
        assert_eq!(
            crate::formula::eval_formula("=1+2*3", &r),
            CellValue::Number(7.0)
        );
    }

    #[test]
    fn test_eval_formula_parse_failure_produces_parse_error() {
        let r = MockResolver::new();
        match crate::formula::eval_formula("=1+", &r) {
            CellValue::Error(FormulaError::Parse(_)) => {}
            other => panic!("expected Parse error, got {other:?}"),
        }
    }

    // ---- to_number / to_text coercion (direct unit tests) ----

    #[test]
    fn test_to_number_coercions() {
        assert_eq!(to_number(&CellValue::Number(5.0)), Ok(5.0));
        assert_eq!(to_number(&CellValue::Boolean(true)), Ok(1.0));
        assert_eq!(to_number(&CellValue::Boolean(false)), Ok(0.0));
        assert_eq!(to_number(&CellValue::Empty), Ok(0.0));
        assert_eq!(to_number(&CellValue::Text("3.5".to_string())), Ok(3.5));
        assert_eq!(
            to_number(&CellValue::Text("abc".to_string())),
            Err(FormulaError::Value)
        );
        assert_eq!(
            to_number(&CellValue::Error(FormulaError::DivZero)),
            Err(FormulaError::DivZero)
        );
    }

    #[test]
    fn test_cell_value_display_matches_to_text_coercions() {
        assert_eq!(CellValue::Number(5.0).to_string(), "5");
        assert_eq!(CellValue::Number(5.5).to_string(), "5.5");
        assert_eq!(CellValue::Boolean(true).to_string(), "TRUE");
        assert_eq!(CellValue::Empty.to_string(), "");
        assert_eq!(CellValue::Error(FormulaError::Ref).to_string(), "#REF!");
        assert_eq!(CellValue::Text("hi".to_string()).to_string(), "hi");
    }

    #[test]
    fn test_to_text_coercions() {
        assert_eq!(to_text(&CellValue::Number(5.0)), "5");
        assert_eq!(to_text(&CellValue::Number(5.5)), "5.5");
        assert_eq!(to_text(&CellValue::Boolean(true)), "TRUE");
        assert_eq!(to_text(&CellValue::Boolean(false)), "FALSE");
        assert_eq!(to_text(&CellValue::Empty), "");
        assert_eq!(to_text(&CellValue::Error(FormulaError::Ref)), "#REF!");
    }

    #[test]
    fn test_formula_error_display() {
        assert_eq!(FormulaError::Ref.to_string(), "#REF!");
        assert_eq!(FormulaError::Value.to_string(), "#VALUE!");
        assert_eq!(FormulaError::DivZero.to_string(), "#DIV/0!");
        assert_eq!(FormulaError::Cycle.to_string(), "#CYCLE!");
        assert_eq!(FormulaError::Name.to_string(), "#NAME?");
        assert!(FormulaError::Parse("bad".to_string())
            .to_string()
            .starts_with("#PARSE!"));
    }

    // ---- comparisons ----

    #[test]
    fn test_numeric_comparisons() {
        let r = MockResolver::new();
        assert_eq!(eval_str("1<2", &r), CellValue::Boolean(true));
        assert_eq!(eval_str("2<=2", &r), CellValue::Boolean(true));
        assert_eq!(eval_str("3>2", &r), CellValue::Boolean(true));
        assert_eq!(eval_str("2>=3", &r), CellValue::Boolean(false));
        assert_eq!(eval_str("2=2", &r), CellValue::Boolean(true));
        assert_eq!(eval_str("2!=3", &r), CellValue::Boolean(true));
    }

    #[test]
    fn test_text_equality_comparison() {
        let r = MockResolver::new().with_cell("A", "1", CellValue::Text("hi".to_string()));
        assert_eq!(eval_str("A1=\"hi\"", &r), CellValue::Boolean(true));
    }

    // ---- checkbox stored as raw text '0'/'1' (§33.3.1) also coerces ----

    #[test]
    fn test_checkbox_text_form_also_coerces() {
        let r = MockResolver::new().with_cell("A", "1", CellValue::Text("1".to_string()));
        assert_eq!(eval_str("A1+0", &r), CellValue::Number(1.0));
    }

    // ---- evaluate() visited-set cycle guard (#2339 PR 7, PR 6 audit WARN
    // resolution 2) ----

    #[test]
    fn test_cell_ref_already_in_visited_returns_cycle_error() {
        // Simulates the in-flight portion of a self-referential (or
        // cyclic) chain: the coordinate is already marked "in progress"
        // when evaluate() reaches a CellRef pointing at it — must short-
        // circuit to #CYCLE! without ever calling resolve_cell.
        let r = MockResolver::new().with_cell("A", "1", CellValue::Number(1.0));
        let ast = parse_formula("A1").unwrap();
        let mut visited = HashSet::new();
        visited.insert(("A".to_string(), "1".to_string()));
        let result = evaluate(&ast, &r, &mut visited);
        assert_eq!(result, CellValue::Error(FormulaError::Cycle));
    }

    #[test]
    fn test_cycle_error_propagates_through_arithmetic() {
        // #CYCLE! must propagate through binary ops like any other error,
        // per evaluate()'s existing error short-circuit pattern.
        let r = MockResolver::new().with_cell("A", "1", CellValue::Number(1.0));
        let ast = parse_formula("A1+5").unwrap();
        let mut visited = HashSet::new();
        visited.insert(("A".to_string(), "1".to_string()));
        let result = evaluate(&ast, &r, &mut visited);
        assert_eq!(result, CellValue::Error(FormulaError::Cycle));
    }

    #[test]
    fn test_visited_set_is_scoped_not_global() {
        // A1 and B1 are both plain cell refs, neither in `visited` at the
        // top-level call — they must resolve normally (no false-positive
        // cycle just because they're evaluated within the same top-level
        // formula).
        let r = MockResolver::new()
            .with_cell("A", "1", CellValue::Number(3.0))
            .with_cell("B", "1", CellValue::Number(4.0));
        assert_eq!(eval_str("A1+B1", &r), CellValue::Number(7.0));
    }

    #[test]
    fn test_visited_set_cleared_after_resolution_allows_diamond_dependency() {
        // A1 appears twice in the same formula (A1+A1) — a legitimate
        // "diamond" reference, not a cycle. Because `evaluate` removes the
        // coordinate from `visited` immediately after resolving it, the
        // second reference must NOT be treated as a cycle.
        let r = MockResolver::new().with_cell("A", "1", CellValue::Number(5.0));
        assert_eq!(eval_str("A1+A1", &r), CellValue::Number(10.0));
    }
}

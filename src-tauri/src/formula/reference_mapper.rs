// Pane — ReferenceMapper (#2339 PR 14, §33.5.2, Matt ruling 2026-09-05 #8)
//
// Closes the interim gap `recompute.rs`'s own module comment names
// explicitly (see that file's header, and PR 16's audit): the parser
// (`parser.rs`) tokenizes A1-style references (`A1`, `B2:B9`) but never
// translates them against a live grid — it hands `AstNode::CellRef`/
// `Range` nodes carrying the LITERAL A1 text (`column_id: "A", row_id:
// "1"`) straight through, which then silently resolves to `Empty` at
// recompute time forever (never a real UUID match).
//
// This module is the missing translation layer. Two directions:
//
//   - `translate_formula_a1_to_ids` (WRITE path): given a formula string
//     a human just typed (via the formula bar or an in-cell editor) and
//     the sheet's current column/row ordering, resolves every A1-style
//     reference to the real `sheet_columns.id`/`sheet_rows.id` stable-ID
//     pair the letter/number position CURRENTLY denotes, and rewrites the
//     formula text to canonical `{column_id:row_id}` storage form. An
//     out-of-bounds reference (a position with no column/row at that
//     index) is rejected with a descriptive `Err` — same UX surface as a
//     parse rejection (Matt ruling 2026-09-05 #3): "a ref stores as a
//     permanent cell ID; a nonexistent cell has no ID to store."
//
//   - `translate_formula_ids_to_a1` (READ path): the inverse, for
//     display only. Given a formula already stored in canonical
//     `{column_id:row_id}` form, re-renders every reference as the
//     referent's CURRENT A1 position (§33.5.2: "display always re-
//     renders references as the referent's current A1 position") — this
//     is what makes stable-ID storage transparent to the user even as
//     rows/columns get inserted, deleted, or reordered.
//
// **Placement (Matt ruling 2026-09-05 #8):** called from
// `sheets::update_sheet_cell_internal`, BEFORE
// `recompute::validate_formula_on_write`. Since `set_sheet_cells` (the AI
// tool write path) calls the SAME `update_sheet_cell_internal`, an
// AI-tool-written A1 formula translates through the identical door as a
// human-typed one — no separate AI-specific code path. The ONE path that
// stays untranslated is `apply_changesets` (sync replay), which never
// routes through `update_sheet_cell_internal` at all and is expected to
// surface a sync-arrived A1-form formula as `#PARSE!`-or-silent-`Empty`
// per existing behavior (documented in `recompute.rs`) — unchanged by
// this module.
//
// **A1-vs-ID-form heuristic:** once parsed, `AstNode::CellRef`/`Range`
// carry plain strings with no provenance tag saying "I came from `{...}`
// brace syntax" vs. "I came from a bare `A1` identifier." Since a real
// `sheet_columns.id`/`sheet_rows.id` is always a UUID (contains hyphens,
// mixed-case hex), and this module's `is_a1_shape` check requires the
// column part be ALL ASCII letters and the row part be ALL ASCII digits
// with nothing else, a real stored ID can never accidentally match this
// shape — so a reference is treated as "needs translation" iff it looks
// exactly like an A1 address, and left untouched otherwise (already
// ID-form, e.g. an AI tool that wrote canonical `{col:row}` syntax
// directly, or a formula being re-translated a second time — this
// makes both translation directions idempotent on already-translated
// input, a defensive property exercised by this module's own tests).

use crate::formula::parser::{parse_formula, AstNode, BinOp, CmpOp, FnName};

/// True iff `column_id`/`row_id` together look exactly like an A1-style
/// address component pair (e.g. `("A", "1")`, `("AB", "123")`) — the
/// shape `parser.rs`'s `split_a1_ref` produces for bare-identifier input.
/// A real stable `sheet_columns.id`/`sheet_rows.id` UUID can never match
/// this (UUIDs contain hyphens and digits mixed into the "letters" part).
fn is_a1_shape(column_id: &str, row_id: &str) -> bool {
    !column_id.is_empty()
        && column_id.chars().all(|c| c.is_ascii_alphabetic())
        && !row_id.is_empty()
        && row_id.chars().all(|c| c.is_ascii_digit())
}

/// A1 column letters (`"A"`, `"AB"`) -> zero-based index. Inverse of
/// `a1_col_letters`. Returns `None` for anything not composed purely of
/// ASCII letters (shouldn't happen — `is_a1_shape` is always checked
/// first — but never panics regardless).
fn a1_col_index(letters: &str) -> Option<usize> {
    if letters.is_empty() || !letters.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let mut n: i64 = 0;
    for c in letters.chars() {
        let d = (c.to_ascii_uppercase() as u8 - b'A') as i64 + 1;
        n = n * 26 + d;
    }
    if n <= 0 {
        None
    } else {
        Some((n - 1) as usize)
    }
}

/// Zero-based column index -> A1 letters. Matches `SheetGrid.svelte`'s
/// own `colRef` function exactly (same base-26, no-zero-digit algorithm)
/// so header letters and formula-bar/in-cell reference text can never
/// disagree.
fn a1_col_letters(index: usize) -> String {
    let mut n = index as i64;
    let mut out = String::new();
    loop {
        let rem = (n % 26) as u8;
        out.insert(0, (b'A' + rem) as char);
        n = n / 26 - 1;
        if n < 0 {
            break;
        }
    }
    out
}

/// A1 row text (`"1"`, `"512"`) -> zero-based index. `None` for anything
/// non-numeric or `"0"`/negative (A1 rows are 1-indexed; `"0"` is not a
/// valid A1 address per the tokenizer's own digit-only grammar, but this
/// function is defensive regardless).
fn a1_row_index(row_text: &str) -> Option<usize> {
    let n: u64 = row_text.parse().ok()?;
    if n == 0 {
        None
    } else {
        Some((n - 1) as usize)
    }
}

/// Zero-based row index -> A1 row number (1-indexed display).
fn a1_row_number(index: usize) -> String {
    (index + 1).to_string()
}

/// Recursively walks `node`, translating every A1-shaped `CellRef`/
/// `Range` to stable-ID form using `ordered_column_ids`/`ordered_row_ids`
/// (both already sorted by `sort_order ASC` — position `i` IS the
/// column-letter/row-number index at that position). Non-A1-shaped refs
/// (already ID-form) pass through unchanged. Returns `Err` with a
/// human-readable message the instant any reference resolves to a
/// position past either list's bounds — the whole formula is rejected,
/// nothing partial is ever returned (Matt ruling 2026-09-05 #3).
fn translate_node_to_ids(
    node: &AstNode,
    ordered_column_ids: &[String],
    ordered_row_ids: &[String],
) -> Result<AstNode, String> {
    match node {
        AstNode::Number(_) | AstNode::Text(_) | AstNode::Boolean(_) => Ok(node.clone()),
        AstNode::CellRef { column_id, row_id } => {
            if !is_a1_shape(column_id, row_id) {
                return Ok(node.clone());
            }
            let (col_id, row_id_real) =
                resolve_a1_pair(column_id, row_id, ordered_column_ids, ordered_row_ids)?;
            Ok(AstNode::CellRef { column_id: col_id, row_id: row_id_real })
        }
        AstNode::Range { start_col, start_row, end_col, end_row } => {
            if !is_a1_shape(start_col, start_row) || !is_a1_shape(end_col, end_row) {
                return Ok(node.clone());
            }
            let (start_col_id, start_row_id) =
                resolve_a1_pair(start_col, start_row, ordered_column_ids, ordered_row_ids)?;
            let (end_col_id, end_row_id) =
                resolve_a1_pair(end_col, end_row, ordered_column_ids, ordered_row_ids)?;
            // Item 37 (#2874): column ranges (`start_col_id != end_col_id`
            // is false, i.e. same column) and NEW single-row ranges (same
            // row, different columns) are both accepted; a genuinely
            // mixed-shape range (different column AND different row) is
            // rejected inline — same UX surface as the pre-item-37
            // cross-column-only rejection, just phrased for the new
            // 3-way shape space. Comparing RESOLVED ids (not raw A1 text)
            // so this sits after, and is unaffected by, whatever
            // column/row reordering happened during resolution above.
            match (start_col_id == end_col_id, start_row_id == end_row_id) {
                (true, _) => Ok(AstNode::Range {
                    start_col: start_col_id,
                    start_row: start_row_id,
                    end_col: end_col_id,
                    end_row: end_row_id,
                }),
                (false, true) => Ok(AstNode::Range {
                    start_col: start_col_id,
                    start_row: start_row_id,
                    end_col: end_col_id,
                    end_row: end_row_id,
                }),
                (false, false) => Err(format!(
                    "'{start_col}{start_row}:{end_col}{end_row}' is a mixed range — ranges spanning both rows and columns are not supported, only single-column (e.g. A1:A5) or single-row (e.g. A1:C1) ranges"
                )),
            }
        }
        AstNode::UnaryMinus(inner) => Ok(AstNode::UnaryMinus(Box::new(translate_node_to_ids(
            inner,
            ordered_column_ids,
            ordered_row_ids,
        )?))),
        AstNode::BinaryOp(l, op, r) => Ok(AstNode::BinaryOp(
            Box::new(translate_node_to_ids(l, ordered_column_ids, ordered_row_ids)?),
            *op,
            Box::new(translate_node_to_ids(r, ordered_column_ids, ordered_row_ids)?),
        )),
        AstNode::Compare(l, op, r) => Ok(AstNode::Compare(
            Box::new(translate_node_to_ids(l, ordered_column_ids, ordered_row_ids)?),
            *op,
            Box::new(translate_node_to_ids(r, ordered_column_ids, ordered_row_ids)?),
        )),
        AstNode::FnCall(name, args) => {
            let translated = args
                .iter()
                .map(|a| translate_node_to_ids(a, ordered_column_ids, ordered_row_ids))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(AstNode::FnCall(name.clone(), translated))
        }
    }
}

/// Resolves one A1 `(column_letters, row_digits)` pair to its current
/// `(column_id, row_id)` stable-ID pair, or an `Err` naming exactly which
/// half is out of bounds.
fn resolve_a1_pair(
    column_letters: &str,
    row_digits: &str,
    ordered_column_ids: &[String],
    ordered_row_ids: &[String],
) -> Result<(String, String), String> {
    let a1_text = format!("{column_letters}{row_digits}");
    let col_idx = a1_col_index(column_letters)
        .ok_or_else(|| format!("'{a1_text}' is not a valid cell reference"))?;
    let row_idx = a1_row_index(row_digits)
        .ok_or_else(|| format!("'{a1_text}' is not a valid cell reference"))?;
    let col_id = ordered_column_ids.get(col_idx).ok_or_else(|| {
        format!("'{a1_text}' is out of bounds — this sheet has no column {column_letters}")
    })?;
    let row_id = ordered_row_ids.get(row_idx).ok_or_else(|| {
        format!("'{a1_text}' is out of bounds — this sheet has no row {row_digits}")
    })?;
    Ok((col_id.clone(), row_id.clone()))
}

/// Recursively walks `node`, translating every stable-ID-form `CellRef`/
/// `Range` back to its CURRENT A1 address using `ordered_column_ids`/
/// `ordered_row_ids`'s live positions. A referent whose id no longer
/// appears in either list (column/row deleted since the formula was
/// written) is left in its raw ID form — display-only fallback; the
/// recompute engine's own `#REF!` handling (`recompute.rs`) already
/// covers the EVALUATION side of a deleted referent, this function only
/// covers what text to show in the bar/cell.
fn translate_node_to_a1(node: &AstNode, ordered_column_ids: &[String], ordered_row_ids: &[String]) -> AstNode {
    match node {
        AstNode::Number(_) | AstNode::Text(_) | AstNode::Boolean(_) => node.clone(),
        AstNode::CellRef { column_id, row_id } => {
            if is_a1_shape(column_id, row_id) {
                return node.clone();
            }
            match (
                ordered_column_ids.iter().position(|c| c == column_id),
                ordered_row_ids.iter().position(|r| r == row_id),
            ) {
                (Some(ci), Some(ri)) => AstNode::CellRef {
                    column_id: a1_col_letters(ci),
                    row_id: a1_row_number(ri),
                },
                _ => node.clone(),
            }
        }
        AstNode::Range { start_col, start_row, end_col, end_row } => {
            let start = if is_a1_shape(start_col, start_row) {
                (start_col.clone(), start_row.clone())
            } else {
                match (
                    ordered_column_ids.iter().position(|c| c == start_col),
                    ordered_row_ids.iter().position(|r| r == start_row),
                ) {
                    (Some(ci), Some(ri)) => (a1_col_letters(ci), a1_row_number(ri)),
                    _ => (start_col.clone(), start_row.clone()),
                }
            };
            let end = if is_a1_shape(end_col, end_row) {
                (end_col.clone(), end_row.clone())
            } else {
                match (
                    ordered_column_ids.iter().position(|c| c == end_col),
                    ordered_row_ids.iter().position(|r| r == end_row),
                ) {
                    (Some(ci), Some(ri)) => (a1_col_letters(ci), a1_row_number(ri)),
                    _ => (end_col.clone(), end_row.clone()),
                }
            };
            AstNode::Range { start_col: start.0, start_row: start.1, end_col: end.0, end_row: end.1 }
        }
        AstNode::UnaryMinus(inner) => {
            AstNode::UnaryMinus(Box::new(translate_node_to_a1(inner, ordered_column_ids, ordered_row_ids)))
        }
        AstNode::BinaryOp(l, op, r) => AstNode::BinaryOp(
            Box::new(translate_node_to_a1(l, ordered_column_ids, ordered_row_ids)),
            *op,
            Box::new(translate_node_to_a1(r, ordered_column_ids, ordered_row_ids)),
        ),
        AstNode::Compare(l, op, r) => AstNode::Compare(
            Box::new(translate_node_to_a1(l, ordered_column_ids, ordered_row_ids)),
            *op,
            Box::new(translate_node_to_a1(r, ordered_column_ids, ordered_row_ids)),
        ),
        AstNode::FnCall(name, args) => AstNode::FnCall(
            name.clone(),
            args.iter().map(|a| translate_node_to_a1(a, ordered_column_ids, ordered_row_ids)).collect(),
        ),
    }
}

// ---------------------------------------------------------------------
// AST -> formula-text serializer (minimal-parens, precedence-aware)
// ---------------------------------------------------------------------
//
// Needed by both translation directions: after rewriting reference
// nodes, the AST must be turned back into a formula string for storage
// (write path) or display (read path). Uses standard precedence-based
// paren-omission (comparisons=1, add/sub=2, mul/div=3, unary minus=4,
// everything else=5/primary) so a round-tripped `=A1+B1*2` doesn't come
// back out as `=A1+(B1*2)` — cosmetically ugly but not incorrect, so this
// is a display-quality concern, not a correctness one, but worth getting
// right since users will see this text in the bar/cell every time they
// open a formula that references anything.

fn bin_op_symbol(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
    }
}

fn cmp_op_symbol(op: CmpOp) -> &'static str {
    match op {
        CmpOp::Eq => "=",
        CmpOp::Ne => "!=",
        CmpOp::Lt => "<",
        CmpOp::Gt => ">",
        CmpOp::Le => "<=",
        CmpOp::Ge => ">=",
    }
}

fn fn_name_str(name: &FnName) -> String {
    match name {
        FnName::Sum => "SUM".to_string(),
        FnName::Average => "AVERAGE".to_string(),
        FnName::If => "IF".to_string(),
        FnName::Unknown(s) => s.clone(),
    }
}

/// Formats a numeric literal without a trailing `.0` for whole numbers
/// (matches how a user would have typed it — `5`, not `5.0`).
fn format_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

fn node_precedence(node: &AstNode) -> u8 {
    match node {
        AstNode::Compare(..) => 1,
        AstNode::BinaryOp(_, BinOp::Add, _) | AstNode::BinaryOp(_, BinOp::Sub, _) => 2,
        AstNode::BinaryOp(_, BinOp::Mul, _) | AstNode::BinaryOp(_, BinOp::Div, _) => 3,
        AstNode::UnaryMinus(_) => 4,
        _ => 5,
    }
}

/// Serializes `node` to formula text (no leading `=`), wrapping in `(...)`
/// only when `node`'s own precedence is lower than `min_prec` requires.
fn serialize_prec(node: &AstNode, min_prec: u8) -> String {
    let prec = node_precedence(node);
    let body = match node {
        AstNode::Number(n) => format_number(*n),
        AstNode::Text(s) => format!("\"{s}\""),
        AstNode::Boolean(b) => if *b { "TRUE".to_string() } else { "FALSE".to_string() },
        AstNode::CellRef { column_id, row_id } => {
            if is_a1_shape(column_id, row_id) {
                format!("{column_id}{row_id}")
            } else {
                format!("{{{column_id}:{row_id}}}")
            }
        }
        AstNode::Range { start_col, start_row, end_col, end_row } => {
            if is_a1_shape(start_col, start_row) && is_a1_shape(end_col, end_row) {
                format!("{start_col}{start_row}:{end_col}{end_row}")
            } else {
                // Item 37 (#2874) / Matt ruling: ALL ID-form ranges —
                // column, row, or (already-rejected-by-this-point) any
                // other shape — serialize as two complete `{col_id:
                // row_id}` cell refs joined by a colon. No shape
                // branching needed here: each endpoint is independently
                // self-describing, so there's no narrowing/ambiguity to
                // reason about the way the old single-column-only
                // `{col:row1:row2}` form required. This REPLACES (not
                // extends) the old branch — the legacy 3-part form is
                // never emitted by this function again, though the
                // parser still reads it (read-only backward compat).
                format!("{{{start_col}:{start_row}}}:{{{end_col}:{end_row}}}")
            }
        }
        AstNode::UnaryMinus(inner) => format!("-{}", serialize_prec(inner, 4)),
        AstNode::BinaryOp(l, op, r) => {
            let op_prec = match op {
                BinOp::Add | BinOp::Sub => 2,
                BinOp::Mul | BinOp::Div => 3,
            };
            // Non-commutative ops (Sub, Div) force parens around a
            // same-precedence RIGHT operand to preserve left-associative
            // grouping (`a-(b-c)` must stay distinct from `a-b-c`).
            let right_min = match op {
                BinOp::Sub | BinOp::Div => op_prec + 1,
                BinOp::Add | BinOp::Mul => op_prec,
            };
            format!(
                "{}{}{}",
                serialize_prec(l, op_prec),
                bin_op_symbol(*op),
                serialize_prec(r, right_min)
            )
        }
        AstNode::Compare(l, op, r) => {
            format!("{}{}{}", serialize_prec(l, 1), cmp_op_symbol(*op), serialize_prec(r, 2))
        }
        AstNode::FnCall(name, args) => {
            let arg_strs: Vec<String> = args.iter().map(|a| serialize_prec(a, 0)).collect();
            format!("{}({})", fn_name_str(name), arg_strs.join(","))
        }
    };
    if prec < min_prec {
        format!("({body})")
    } else {
        body
    }
}

fn serialize(node: &AstNode) -> String {
    serialize_prec(node, 0)
}

// ---------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------

/// WRITE path (§33.5.2, Matt ruling 2026-09-05 #8). `formula_value` is
/// the full raw cell value as typed, INCLUDING its leading `=`. Returns
/// the rewritten formula (still `=`-prefixed) with every A1-style
/// reference resolved to `{column_id:row_id}` stable-ID form, or an
/// `Err` if the formula doesn't parse, OR any reference is out of bounds
/// for the current `ordered_column_ids`/`ordered_row_ids`, OR any range
/// reference spans more than one column (e.g. `A1:B5`) — the v1 grammar
/// and this module's storage/serialization form only support single-
/// column ranges. Any of these leaves nothing for the caller to store
/// (Matt ruling 2026-09-05 #3: reject the whole formula, same UX surface
/// as a parse rejection).
///
/// `ordered_column_ids`/`ordered_row_ids` MUST already be sorted by
/// `sort_order ASC` — position `i` in each slice is exactly what column
/// letter / row number `i` currently denotes.
pub fn translate_formula_a1_to_ids(
    formula_value: &str,
    ordered_column_ids: &[String],
    ordered_row_ids: &[String],
) -> Result<String, String> {
    let body = formula_value.strip_prefix('=').unwrap_or(formula_value);
    let ast = parse_formula(body).map_err(|e| format!("Invalid formula: {e}"))?;
    let translated = translate_node_to_ids(&ast, ordered_column_ids, ordered_row_ids)?;
    Ok(format!("={}", serialize(&translated)))
}

/// READ path (§33.5.2). `formula_value` is a stored cell value (already
/// in canonical `{column_id:row_id}` form, `=`-prefixed). Returns the
/// SAME formula with every stable-ID reference re-rendered as its
/// current A1 position, for display in the formula bar / in-cell raw
/// text. If `formula_value` doesn't parse (a sync-arrived malformed
/// formula, §33.5.3's `#PARSE!` case) the raw string is returned
/// unchanged — there's no AST to walk, and showing the user exactly what
/// was stored is the correct fallback (never panics, never blanks the
/// cell).
pub fn translate_formula_ids_to_a1(
    formula_value: &str,
    ordered_column_ids: &[String],
    ordered_row_ids: &[String],
) -> String {
    let Some(body) = formula_value.strip_prefix('=') else {
        return formula_value.to_string();
    };
    match parse_formula(body) {
        Ok(ast) => {
            let translated = translate_node_to_a1(&ast, ordered_column_ids, ordered_row_ids);
            format!("={}", serialize(&translated))
        }
        Err(_) => formula_value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cols(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("col-{i}")).collect()
    }

    fn rows(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("row-{i}")).collect()
    }

    // ---- a1_col_index / a1_col_letters round trip ----

    #[test]
    fn test_a1_col_index_single_letter() {
        assert_eq!(a1_col_index("A"), Some(0));
        assert_eq!(a1_col_index("B"), Some(1));
        assert_eq!(a1_col_index("Z"), Some(25));
    }

    #[test]
    fn test_a1_col_index_multi_letter() {
        assert_eq!(a1_col_index("AA"), Some(26));
        assert_eq!(a1_col_index("AB"), Some(27));
    }

    #[test]
    fn test_a1_col_index_lowercase() {
        assert_eq!(a1_col_index("a"), Some(0));
        assert_eq!(a1_col_index("ab"), Some(27));
    }

    #[test]
    fn test_a1_col_index_rejects_non_alpha() {
        assert_eq!(a1_col_index("A1"), None);
        assert_eq!(a1_col_index(""), None);
    }

    #[test]
    fn test_a1_col_letters_round_trips_with_index() {
        for i in [0usize, 1, 25, 26, 27, 51, 52, 700] {
            let letters = a1_col_letters(i);
            assert_eq!(a1_col_index(&letters), Some(i), "round trip failed for index {i}");
        }
    }

    #[test]
    fn test_a1_col_letters_known_values() {
        assert_eq!(a1_col_letters(0), "A");
        assert_eq!(a1_col_letters(25), "Z");
        assert_eq!(a1_col_letters(26), "AA");
    }

    // ---- a1_row_index / a1_row_number round trip ----

    #[test]
    fn test_a1_row_index_basic() {
        assert_eq!(a1_row_index("1"), Some(0));
        assert_eq!(a1_row_index("512"), Some(511));
    }

    #[test]
    fn test_a1_row_index_rejects_zero_and_non_numeric() {
        assert_eq!(a1_row_index("0"), None);
        assert_eq!(a1_row_index("abc"), None);
        assert_eq!(a1_row_index(""), None);
    }

    #[test]
    fn test_a1_row_number_round_trips_with_index() {
        for i in [0usize, 1, 511] {
            assert_eq!(a1_row_index(&a1_row_number(i)), Some(i));
        }
    }

    // ---- is_a1_shape ----

    #[test]
    fn test_is_a1_shape_true_for_real_a1_pairs() {
        assert!(is_a1_shape("A", "1"));
        assert!(is_a1_shape("AB", "123"));
    }

    #[test]
    fn test_is_a1_shape_false_for_uuid_like_ids() {
        assert!(!is_a1_shape("col-uuid-123", "row-uuid-456"));
        assert!(!is_a1_shape("3f9c2b1a-45de", "9e88-12"));
    }

    #[test]
    fn test_is_a1_shape_false_for_empty_parts() {
        assert!(!is_a1_shape("", "1"));
        assert!(!is_a1_shape("A", ""));
    }

    // ---- translate_formula_a1_to_ids: write path ----

    #[test]
    fn test_translate_single_cell_ref_to_ids() {
        let result = translate_formula_a1_to_ids("=A1", &cols(3), &rows(3)).unwrap();
        assert_eq!(result, "={col-0:row-0}");
    }

    #[test]
    fn test_translate_second_column_and_row() {
        // B2 -> column index 1, row index 1
        let result = translate_formula_a1_to_ids("=B2", &cols(3), &rows(3)).unwrap();
        assert_eq!(result, "={col-1:row-1}");
    }

    #[test]
    fn test_translate_arithmetic_expression_translates_both_refs() {
        let result = translate_formula_a1_to_ids("=A1+B1", &cols(3), &rows(3)).unwrap();
        assert_eq!(result, "={col-0:row-0}+{col-1:row-0}");
    }

    #[test]
    fn test_translate_range_reference() {
        // Item 37 (#2874): `serialize_prec` now unconditionally emits the
        // new two-cell-ref form for ALL ID-form ranges, including plain
        // column ranges like this one — the legacy `{col:row1:row2}`
        // shape is never written anymore, only read (see
        // `test_parse_legacy_column_range_form` in parser.rs).
        let result = translate_formula_a1_to_ids("=SUM(A1:A5)", &cols(3), &rows(5)).unwrap();
        assert_eq!(result, "=SUM({col-0:row-0}:{col-0:row-4})");
    }

    #[test]
    fn test_translate_if_function_call_multiple_refs() {
        let result = translate_formula_a1_to_ids("=IF(A1>0, B1, C1)", &cols(3), &rows(3)).unwrap();
        assert_eq!(result, "=IF({col-0:row-0}>0,{col-1:row-0},{col-2:row-0})");
    }

    #[test]
    fn test_translate_unary_minus_on_ref() {
        let result = translate_formula_a1_to_ids("=-A1", &cols(3), &rows(3)).unwrap();
        assert_eq!(result, "=-{col-0:row-0}");
    }

    #[test]
    fn test_translate_multi_letter_column() {
        let result = translate_formula_a1_to_ids("=AA1", &cols(30), &rows(3)).unwrap();
        assert_eq!(result, "={col-26:row-0}");
    }

    #[test]
    fn test_translate_formula_with_no_refs_unchanged_besides_prefix() {
        let result = translate_formula_a1_to_ids("=1+2*3", &cols(3), &rows(3)).unwrap();
        assert_eq!(result, "=1+2*3");
    }

    #[test]
    fn test_translate_lowercase_a1_ref_normalizes() {
        let result = translate_formula_a1_to_ids("=a1", &cols(3), &rows(3)).unwrap();
        assert_eq!(result, "={col-0:row-0}");
    }

    #[test]
    fn test_translate_out_of_bounds_column_is_rejected() {
        let err = translate_formula_a1_to_ids("=Z99", &cols(3), &rows(3)).unwrap_err();
        assert!(err.contains("out of bounds"), "expected out-of-bounds message, got: {err}");
    }

    #[test]
    fn test_translate_out_of_bounds_row_is_rejected() {
        // 3 columns (A-C), but only 3 rows — row 99 doesn't exist.
        let err = translate_formula_a1_to_ids("=A99", &cols(3), &rows(3)).unwrap_err();
        assert!(err.contains("out of bounds"), "expected out-of-bounds message, got: {err}");
    }

    #[test]
    fn test_translate_out_of_bounds_rejects_whole_formula_not_partial() {
        // A1 is in bounds, Z99 is not — the WHOLE formula must be
        // rejected, nothing partially translated/stored.
        let result = translate_formula_a1_to_ids("=A1+Z99", &cols(3), &rows(3));
        assert!(result.is_err());
    }

    // ---- mixed-range rejection (#2339 PR 14 fix; generalized to a
    // 3-way branch by item 37 / #2874) ----

    #[test]
    fn test_translate_cross_column_range_is_rejected() {
        // A1:B5 on a 5x5 sheet spans column A AND column B, AND row 1
        // and row 5 — a genuinely mixed-shape range, which neither the
        // column-range nor (post-item-37) row-range storage form can
        // represent, so it must be rejected at write time (not silently
        // narrowed, dropping data).
        let err = translate_formula_a1_to_ids("=SUM(A1:B5)", &cols(5), &rows(5)).unwrap_err();
        assert!(
            err.to_lowercase().contains("mixed"),
            "expected a mixed-range rejection message, got: {err}"
        );
    }

    #[test]
    fn test_translate_single_column_range_still_accepted_on_5x5_sheet() {
        // Same-shape sheet as the cross-column case above — a genuine
        // single-column range (A1:A5) must still translate and round-trip
        // correctly; this is not collateral damage from the new check.
        let c = cols(5);
        let r = rows(5);
        let stored = translate_formula_a1_to_ids("=SUM(A1:A5)", &c, &r).unwrap();
        assert_eq!(stored, "=SUM({col-0:row-0}:{col-0:row-4})");
        let displayed = translate_formula_ids_to_a1(&stored, &c, &r);
        assert_eq!(displayed, "=SUM(A1:A5)");
    }

    #[test]
    fn test_translate_individual_cell_refs_different_columns_still_accepted() {
        // `=A1+B1` is two SEPARATE single-cell CellRef nodes, not a Range
        // node at all — the cross-column-range check only applies to
        // `AstNode::Range`, so this must be entirely unaffected.
        let result = translate_formula_a1_to_ids("=A1+B1", &cols(5), &rows(5)).unwrap();
        assert_eq!(result, "={col-0:row-0}+{col-1:row-0}");
    }

    #[test]
    fn test_translate_cross_column_range_rejected_even_nested_in_expression() {
        // The rejection must propagate up through the whole AST walk, not
        // just at the top level — same "reject the whole formula" rule as
        // out-of-bounds refs.
        let err = translate_formula_a1_to_ids("=SUM(A1:B5)+1", &cols(5), &rows(5)).unwrap_err();
        assert!(err.to_lowercase().contains("mixed"));
    }

    #[test]
    fn test_translate_cross_column_range_message_names_the_range() {
        let err = translate_formula_a1_to_ids("=SUM(A1:B5)", &cols(5), &rows(5)).unwrap_err();
        assert!(err.contains("A1:B5"), "expected the message to name the offending range, got: {err}");
    }

    #[test]
    fn test_translate_unparseable_formula_returns_parse_error() {
        let err = translate_formula_a1_to_ids("=1+", &cols(3), &rows(3)).unwrap_err();
        assert!(err.contains("Invalid formula"), "expected parse-error message, got: {err}");
    }

    #[test]
    fn test_translate_already_id_form_is_idempotent() {
        // Simulates an AI tool writing canonical ID-form syntax directly
        // (Matt ruling 2026-09-05 #8: AI writes go through the same door,
        // but if they already used ID-form, translation must be a no-op).
        let input = "={col-0:row-0}+{col-1:row-0}";
        let result = translate_formula_a1_to_ids(input, &cols(3), &rows(3)).unwrap();
        assert_eq!(result, input);
    }

    #[test]
    fn test_translate_comparison_expression() {
        let result = translate_formula_a1_to_ids("=A1>B1", &cols(3), &rows(3)).unwrap();
        assert_eq!(result, "={col-0:row-0}>{col-1:row-0}");
    }

    // ---- translate_formula_ids_to_a1: read path ----

    #[test]
    fn test_translate_ids_to_a1_single_ref() {
        let result = translate_formula_ids_to_a1("={col-0:row-0}", &cols(3), &rows(3));
        assert_eq!(result, "=A1");
    }

    #[test]
    fn test_translate_ids_to_a1_range() {
        let result = translate_formula_ids_to_a1("=SUM({col-0:row-0}:{col-0:row-4})", &cols(3), &rows(5));
        assert_eq!(result, "=SUM(A1:A5)");
    }

    #[test]
    fn test_translate_ids_to_a1_legacy_range_form_still_reads() {
        // Legacy `{col:row1:row2}` form must still display correctly —
        // read-only backward compat (item 37 / #2874).
        let result = translate_formula_ids_to_a1("=SUM({col-0:row-0:row-4})", &cols(3), &rows(5));
        assert_eq!(result, "=SUM(A1:A5)");
    }

    #[test]
    fn test_translate_ids_to_a1_arithmetic() {
        let result = translate_formula_ids_to_a1("={col-0:row-0}+{col-1:row-0}", &cols(3), &rows(3));
        assert_eq!(result, "=A1+B1");
    }

    #[test]
    fn test_translate_ids_to_a1_deleted_referent_falls_back_to_raw_id() {
        // "col-9" isn't in the (3-column) ordered list -> referent gone.
        let result = translate_formula_ids_to_a1("={col-9:row-0}", &cols(3), &rows(3));
        assert_eq!(result, "={col-9:row-0}");
    }

    #[test]
    fn test_translate_ids_to_a1_unparseable_formula_returned_as_is() {
        let result = translate_formula_ids_to_a1("=1+", &cols(3), &rows(3));
        assert_eq!(result, "=1+");
    }

    #[test]
    fn test_translate_ids_to_a1_reflects_current_sort_order_after_reorder() {
        // A column that WAS at position 1 (B) is now at position 0 (A) —
        // the display must reflect the CURRENT position, not any
        // position frozen at write time. This is the entire point of
        // stable-ID storage (§33.5.2).
        let reordered_cols = vec!["col-1".to_string(), "col-0".to_string()];
        let result = translate_formula_ids_to_a1("={col-1:row-0}", &reordered_cols, &rows(3));
        assert_eq!(result, "=A1");
    }

    // ---- write -> read round trip ----

    #[test]
    fn test_write_then_read_round_trip_preserves_a1_text() {
        let c = cols(5);
        let r = rows(5);
        let stored = translate_formula_a1_to_ids("=SUM(A1:A3)+B1", &c, &r).unwrap();
        let displayed = translate_formula_ids_to_a1(&stored, &c, &r);
        assert_eq!(displayed, "=SUM(A1:A3)+B1");
    }

    #[test]
    fn test_write_then_read_round_trip_survives_precedence_mix() {
        let c = cols(5);
        let r = rows(5);
        let original = "=A1+B1*C1-D1/E1";
        let stored = translate_formula_a1_to_ids(original, &c, &r).unwrap();
        let displayed = translate_formula_ids_to_a1(&stored, &c, &r);
        assert_eq!(displayed, original);
    }

    // ---- serializer precedence correctness (exercised via round trip) ----

    #[test]
    fn test_serializer_preserves_subtraction_grouping() {
        // a-(b-c) must NOT collapse to a-b-c (different values) — the
        // stored/reparsed AST must retain the original grouping.
        let c = cols(3);
        let r = rows(3);
        let stored = translate_formula_a1_to_ids("=A1-(B1-C1)", &c, &r).unwrap();
        let reparsed = parse_formula(stored.strip_prefix('=').unwrap()).unwrap();
        // A1-(B1-C1) with A1=10,B1=3,C1=1 would be 10-(3-1)=8, vs a flat
        // 10-3-1=6 if grouping were lost — assert the STRUCTURE directly.
        match reparsed {
            AstNode::BinaryOp(_, BinOp::Sub, rhs) => {
                assert!(matches!(*rhs, AstNode::BinaryOp(_, BinOp::Sub, _)));
            }
            other => panic!("expected top-level Sub, got {other:?}"),
        }
    }

    #[test]
    fn test_serializer_preserves_division_grouping() {
        let c = cols(3);
        let r = rows(3);
        let stored = translate_formula_a1_to_ids("=A1/(B1/C1)", &c, &r).unwrap();
        let reparsed = parse_formula(stored.strip_prefix('=').unwrap()).unwrap();
        match reparsed {
            AstNode::BinaryOp(_, BinOp::Div, rhs) => {
                assert!(matches!(*rhs, AstNode::BinaryOp(_, BinOp::Div, _)));
            }
            other => panic!("expected top-level Div, got {other:?}"),
        }
    }

    #[test]
    fn test_serializer_preserves_parenthesized_comparison_under_unary_minus() {
        let c = cols(3);
        let r = rows(3);
        let stored = translate_formula_a1_to_ids("=-(A1=B1)", &c, &r).unwrap();
        let reparsed = parse_formula(stored.strip_prefix('=').unwrap()).unwrap();
        match reparsed {
            AstNode::UnaryMinus(inner) => {
                assert!(matches!(*inner, AstNode::Compare(_, CmpOp::Eq, _)));
            }
            other => panic!("expected top-level UnaryMinus, got {other:?}"),
        }
    }

    // ---- item 37 (#2874): single-row ranges ----

    #[test]
    fn test_translate_single_row_range_accepted() {
        // =SUM(A1:C1) on a 3-col/3-row sheet: same row, different
        // columns — accepted per the new 3-way branch, stored in the new
        // two-cell-ref form.
        let result = translate_formula_a1_to_ids("=SUM(A1:C1)", &cols(3), &rows(3)).unwrap();
        assert_eq!(result, "=SUM({col-0:row-0}:{col-2:row-0})");
    }

    #[test]
    fn test_translate_mixed_range_rejected_inline() {
        // =SUM(A1:C3) spans columns A-C AND rows 1-3 — genuinely mixed,
        // must be rejected with a distinct-from-"cross-column" message.
        let err = translate_formula_a1_to_ids("=SUM(A1:C3)", &cols(3), &rows(3)).unwrap_err();
        assert!(
            err.to_lowercase().contains("mixed"),
            "expected a mixed-range rejection message, got: {err}"
        );
    }

    #[test]
    fn test_translate_single_column_range_still_accepted() {
        // Regression: plain column range must still work post-3-way-branch.
        let result = translate_formula_a1_to_ids("=SUM(A1:A5)", &cols(3), &rows(5)).unwrap();
        assert_eq!(result, "=SUM({col-0:row-0}:{col-0:row-4})");
    }

    #[test]
    fn test_translate_row_range_round_trips_to_a1_display() {
        let c = cols(3);
        let r = rows(3);
        let stored = translate_formula_a1_to_ids("=SUM(A1:C1)", &c, &r).unwrap();
        let displayed = translate_formula_ids_to_a1(&stored, &c, &r);
        assert_eq!(displayed, "=SUM(A1:C1)");
    }

    #[test]
    fn test_serialize_emits_new_form_only() {
        // Prove the OLD column-range serialization path has actually
        // changed too, not just the new row-range path — write a plain
        // COLUMN range via A1 input, assert the stored text is the new
        // two-cell-ref form and explicitly NOT the legacy 3-part shape.
        let stored = translate_formula_a1_to_ids("=SUM(A1:A5)", &cols(3), &rows(5)).unwrap();
        assert_eq!(stored, "=SUM({col-0:row-0}:{col-0:row-4})");
        assert!(
            !stored.contains("{col-0:row-0:row-4}"),
            "serialize_prec must never emit the legacy {{col:row1:row2}} form, got: {stored}"
        );
    }

    #[test]
    fn test_legacy_form_round_trips_to_new_form() {
        // Parse a formula already stored in legacy `{col:row1:row2}`
        // form directly (bypassing the A1 write path, since legacy form
        // is never a WRITE output anymore) and re-serialize via
        // `serialize_prec` (through `translate_formula_ids_to_ids`'s
        // equivalent — here, round-tripping through A1 display and back
        // to ids is the most direct way to exercise `serialize_prec`
        // without a new public entry point).
        let legacy = "=SUM({col-0:row-0:row-4})";
        let c = cols(3);
        let r = rows(5);
        let displayed = translate_formula_ids_to_a1(legacy, &c, &r);
        assert_eq!(displayed, "=SUM(A1:A5)");
        let re_stored = translate_formula_a1_to_ids(&displayed, &c, &r).unwrap();
        assert_eq!(re_stored, "=SUM({col-0:row-0}:{col-0:row-4})");
        assert!(
            !re_stored.contains("{col-0:row-0:row-4}"),
            "re-serialization must upgrade to the new form, not keep legacy shape: {re_stored}"
        );
    }
}

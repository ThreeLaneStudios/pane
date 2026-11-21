// Pane — Sheets Recompute Engine (#2339 PR 7, §33.5.4)
//
// This module owns the PR 7 side of the PR 6 / PR 7 seam: dependency
// graph construction over a sheet's formula cells, cycle detection,
// topological-order evaluation, the in-memory computed-value store, and
// the formula-count guard rail. Lives under `formula::recompute` (not a
// top-level `src-tauri/src/recompute.rs`) because it's a consumer of, and
// tightly coupled to, `formula::eval`/`formula::parser`'s types
// (`CellValue`, `CellResolver`, `AstNode`) — grouping it under the
// `formula` module keeps the whole engine (parse → graph → evaluate) in
// one place, matching the scoping report's file-layout recommendation.
//
// `sheets.rs` and `sync/engine.rs` are this module's only callers.
//
// **Scope note on reference forms (out of scope for this PR's 7
// deliverables):** the scoping report's original PR 6 design sketch
// (`docs/audits/2026-09-03-sheets-pr6-pr7-formula-scope.md` §"canonical
// stable-ID reference form") called for an A1↔ID `ReferenceMapper`
// translation layer (`resolve_refs(Ast<A1Ref>, &dyn RefResolver) ->
// Ast<IdRef>` at write time, plus ID→A1 re-rendering at display time).
// The PR 6 implementation as actually merged (`dd9069d6`) did NOT build
// this: `parser.rs` accepts both A1-style (`A1`) and stable-ID-form
// (`{column_id:row_id}`) syntax at the token level, producing the SAME
// `AstNode::CellRef`/`Range` shape carrying whatever `column_id`/`row_id`
// strings the source syntax yielded verbatim — with no translation step,
// per `formula/mod.rs`'s own module comment ("Translating A1 text to
// resolved stable IDs against a live grid ... is PR 7's `ReferenceMapper`
// concern"). This PR's Deliverables 1–7 do not include building that
// `ReferenceMapper` (it is not named in the task brief's deliverable
// list) — building a full A1-position-drift-aware translation layer
// un-requested would be scope creep beyond an already large PR. This
// module therefore operates directly on whatever `column_id`/`row_id`
// strings a stored formula's AST carries, treating them as-is against
// `sheet_cells`' real `(column_id, row_id)` composite key. In practice
// this means: a formula using the `{column_id:row_id}` stable-ID syntax
// (matching the real `sheet_columns.id`/`sheet_rows.id` UUIDs) resolves
// correctly against live grid data; a formula using A1-style letter/
// number syntax (`A1`, `B2`) will only resolve if `column_id`/`row_id`
// literally equal `"A"`/`"1"` etc. (never true for real UUID-keyed
// columns/rows) and otherwise safely resolves to `Empty` (sparse-cell
// semantics, never a crash or panic — see `LiveCellResolver::
// resolve_cell`'s `None` fallback). This is a known, documented interim
// gap for a follow-up PR to close by implementing the `ReferenceMapper`
// and wiring it into `sheets.rs`'s write path and body-regen display
// path; it does not block this PR's own deliverables, all of which are
// exercised end-to-end by this module's tests using ID-form references.

use crate::formula::eval::{evaluate, CellResolver, CellValue, FormulaError};
use crate::formula::parser::{parse_formula, AstNode};
use rusqlite::{params, Connection};
use std::collections::{HashMap, HashSet};

/// A cell's coordinate: `(column_id, row_id)`. Matches `eval.rs`'s
/// `CellResolver` trait's argument shape and `AstNode::CellRef`'s field
/// pair, so a `CellId` can be built directly from either without
/// translation.
pub type CellId = (String, String);

/// Provisional formula-cell soft limit: 500. Graceful degradation past
/// this — formulas store and display but recompute is skipped. Design
/// phase can tune this value. Matt ruling 2026-09-03.
///
/// Chosen as a round number comfortably above any sheet size the v1 grid
/// UI is expected to render interactively (per the scoping report's
/// perf-risk section), while still being small enough that a full
/// graph-rebuild-per-mutation (see `recompute_sheet`'s doc comment) stays
/// well under a UI-blocking cost on typical hardware.
pub const FORMULA_CELL_SOFT_LIMIT: usize = 500;

/// One sheet's grid state, loaded once per `recompute_sheet` call. Exists
/// so the graph-building and evaluation passes never hold a live
/// `&Connection` mid-walk (every DB round-trip happens up front, in this
/// loader) and so `LiveCellResolver` can implement `CellResolver`
/// (a synchronous, non-fallible-by-signature trait) against plain owned
/// data instead of threading `Result`s through `eval.rs`'s call chain.
struct SheetSnapshot {
    /// `column_id -> col letter`-independent; keyed by A1 letters is NOT
    /// needed here — `sheet_cells` rows are already keyed by the resolved
    /// `(column_id, row_id)` pair, which is exactly what `AstNode::CellRef`
    /// carries once PR 7's formulas are stored using ID-form references
    /// (`{column_id:row_id}` — see `parser.rs`'s `IdRef` token). Row
    /// ordering (`ordered_row_ids`) is still needed for range expansion.
    ordered_row_ids: Vec<String>,
    /// `column_id -> sort_order` position, used only to validate a range's
    /// two endpoint columns are the same column (v1 grammar: ranges are
    /// single-column, `A1:A5`-shaped — see `parser.rs`'s `Range` doc
    /// comment).
    ordered_column_ids: Vec<String>,
    /// Raw stored values, keyed by `(column_id, row_id)`. A formula cell's
    /// raw value starts with `=`; anything else is a literal.
    raw_values: HashMap<CellId, String>,
}

impl SheetSnapshot {
    fn load(conn: &Connection, note_id: &str) -> Result<Self, String> {
        let mut col_stmt = conn
            .prepare("SELECT id FROM sheet_columns WHERE note_id = ?1 ORDER BY sort_order ASC")
            .map_err(|e| format!("SheetSnapshot::load: column query prepare failed: {}", e))?;
        let ordered_column_ids: Vec<String> = col_stmt
            .query_map(params![note_id], |r| r.get::<_, String>(0))
            .map_err(|e| format!("SheetSnapshot::load: column query failed: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("SheetSnapshot::load: column row collect failed: {}", e))?;

        let mut row_stmt = conn
            .prepare("SELECT id FROM sheet_rows WHERE note_id = ?1 ORDER BY sort_order ASC")
            .map_err(|e| format!("SheetSnapshot::load: row query prepare failed: {}", e))?;
        let ordered_row_ids: Vec<String> = row_stmt
            .query_map(params![note_id], |r| r.get::<_, String>(0))
            .map_err(|e| format!("SheetSnapshot::load: row query failed: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("SheetSnapshot::load: row row collect failed: {}", e))?;

        let mut cell_stmt = conn
            .prepare("SELECT row_id, column_id, value FROM sheet_cells WHERE note_id = ?1")
            .map_err(|e| format!("SheetSnapshot::load: cell query prepare failed: {}", e))?;
        let raw_values: HashMap<CellId, String> = cell_stmt
            .query_map(params![note_id], |r| {
                Ok((
                    (r.get::<_, String>(1)?, r.get::<_, String>(0)?),
                    r.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| format!("SheetSnapshot::load: cell query failed: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("SheetSnapshot::load: cell row collect failed: {}", e))?
            .into_iter()
            .collect();

        Ok(SheetSnapshot { ordered_row_ids, ordered_column_ids, raw_values })
    }

    /// DB-free constructor: builds the same `SheetSnapshot` shape from
    /// already-in-memory column/row/cell data (e.g. a parsed historical
    /// version-history envelope) instead of querying a live `Connection`.
    /// Used by `compute_envelope_values` — the sibling of
    /// `recompute_sheet` for displaying a historical snapshot's formula
    /// results without ever touching the live `sheet_columns`/
    /// `sheet_rows`/`sheet_cells` tables. Shares every downstream method
    /// on `SheetSnapshot` (`expand_range`, `formula_cell_ids`) verbatim
    /// with the DB-backed `load` path, so a snapshot's computed values
    /// are guaranteed identical to what `recompute_sheet` would have
    /// produced had this state been live at capture time.
    fn from_parts(
        ordered_column_ids: Vec<String>,
        ordered_row_ids: Vec<String>,
        raw_values: HashMap<CellId, String>,
    ) -> Self {
        SheetSnapshot { ordered_row_ids, ordered_column_ids, raw_values }
    }

    fn formula_cell_ids(&self) -> Vec<CellId> {
        self.raw_values
            .iter()
            .filter(|(_, v)| v.starts_with('='))
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Expand a single-column range `(start_col, start_row, end_col,
    /// end_row)` into concrete `CellId`s in row order. Returns an empty
    /// `Vec` if either endpoint column/row no longer exists in this
    /// sheet's snapshot — mirrors `CellResolver::resolve_range`'s
    /// documented empty-`Vec` contract (`eval.rs` §"Empty-Vec contract").
    fn expand_range(&self, start_col: &str, start_row: &str, end_col: &str, end_row: &str) -> Vec<CellId> {
        if start_col == end_col {
            if !self.ordered_column_ids.iter().any(|c| c == start_col) {
                return vec![];
            }
            let start_pos = self.ordered_row_ids.iter().position(|r| r == start_row);
            let end_pos = self.ordered_row_ids.iter().position(|r| r == end_row);
            let (Some(start_pos), Some(end_pos)) = (start_pos, end_pos) else {
                return vec![];
            };
            let (lo, hi) = if start_pos <= end_pos { (start_pos, end_pos) } else { (end_pos, start_pos) };
            return self.ordered_row_ids[lo..=hi]
                .iter()
                .map(|row_id| (start_col.to_string(), row_id.clone()))
                .collect();
        }
        if start_row == end_row {
            // Item 37 (#2874): single-row range, mirror of the
            // column-range branch above — same row, vary column across
            // `ordered_column_ids[lo..=hi]`. Operates purely on the
            // sort-order-based membership model, which is what makes
            // "insert a column between the endpoints extends the range"
            // come for free (the newly-inserted column's id lands inside
            // `[lo..=hi]` in the live `ordered_column_ids` slice at
            // evaluation time, with no separate bookkeeping needed).
            if !self.ordered_row_ids.iter().any(|r| r == start_row) {
                return vec![];
            }
            let start_pos = self.ordered_column_ids.iter().position(|c| c == start_col);
            let end_pos = self.ordered_column_ids.iter().position(|c| c == end_col);
            let (Some(start_pos), Some(end_pos)) = (start_pos, end_pos) else {
                return vec![];
            };
            let (lo, hi) = if start_pos <= end_pos { (start_pos, end_pos) } else { (end_pos, start_pos) };
            return self.ordered_column_ids[lo..=hi]
                .iter()
                .map(|col_id| (col_id.clone(), start_row.to_string()))
                .collect();
        }
        // Mixed-shape range (different column AND different row):
        // structurally unreachable post-reference_mapper rejection, but
        // handled defensively (returns empty rather than panicking) in
        // case a malformed node ever reaches this function directly
        // (e.g. from a test or a future caller that bypasses the mapper).
        vec![]
    }
}

/// Walks a parsed formula AST and extracts every `CellId` it directly
/// references — `CellRef` nodes as themselves, `Range` nodes expanded to
/// their member cells via the snapshot's row ordering. Used both to build
/// dependency-graph edges and (indirectly, via the same expansion logic)
/// by `LiveCellResolver::resolve_range`.
fn extract_dependencies(ast: &AstNode, snapshot: &SheetSnapshot, out: &mut Vec<CellId>) {
    match ast {
        AstNode::Number(_) | AstNode::Text(_) | AstNode::Boolean(_) => {}
        AstNode::CellRef { column_id, row_id } => {
            out.push((column_id.clone(), row_id.clone()));
        }
        AstNode::Range { start_col, start_row, end_col, end_row } => {
            out.extend(snapshot.expand_range(start_col, start_row, end_col, end_row));
        }
        AstNode::UnaryMinus(inner) => extract_dependencies(inner, snapshot, out),
        AstNode::BinaryOp(lhs, _, rhs) | AstNode::Compare(lhs, _, rhs) => {
            extract_dependencies(lhs, snapshot, out);
            extract_dependencies(rhs, snapshot, out);
        }
        AstNode::FnCall(_, args) => {
            for arg in args {
                extract_dependencies(arg, snapshot, out);
            }
        }
    }
}

/// Dependency graph over a sheet's formula cells. `edges[cell]` is the
/// list of cells `cell`'s formula directly depends on (its formula
/// references them). Only formula cells that parsed successfully appear
/// as keys — a formula cell that fails to parse (`#PARSE!`, e.g. a
/// sync-arrived malformed formula, see `sheets.rs`'s parse-on-write vs.
/// sync-arrival asymmetry doc) has no outgoing edges and is evaluated
/// (trivially, to its parse error) independent of graph order.
struct DependencyGraph {
    edges: HashMap<CellId, Vec<CellId>>,
}

impl DependencyGraph {
    /// Full-graph rebuild per mutation: Matt ruling 2026-09-03. Deviates
    /// from §33.5.4's dirty-subgraph design for v1 simplicity. Revisit if
    /// perf requires targeted invalidation.
    fn build(snapshot: &SheetSnapshot, parsed: &HashMap<CellId, AstNode>) -> Self {
        let mut edges = HashMap::new();
        for (id, ast) in parsed {
            let mut deps = Vec::new();
            extract_dependencies(ast, snapshot, &mut deps);
            edges.insert(id.clone(), deps);
        }
        DependencyGraph { edges }
    }

    /// Three-color DFS cycle detection. Returns the set of ALL cells that
    /// are members of at least one cycle (not just the back-edge
    /// endpoints) — per the deliverable spec: "mark ALL cells in the
    /// cycle with #CYCLE!". A cell that merely *depends on* a cycle
    /// member (without itself being a cycle member) is NOT included here
    /// — it's handled separately by `recompute_sheet`'s
    /// propagate-through-dependents pass, since #CYCLE! propagates like
    /// any other error once the cycle-member cells are pre-seeded.
    ///
    /// Idempotent by construction: this is a pure function of `edges`
    /// (itself rebuilt fresh from `raw_values` every call, per the
    /// full-graph-rebuild ruling above) — unchanged input always produces
    /// the same cycle-member set.
    fn detect_cycles(&self) -> HashSet<CellId> {
        #[derive(Clone, Copy, PartialEq)]
        enum Color {
            White,
            Gray,
            Black,
        }

        let mut color: HashMap<CellId, Color> = self.edges.keys().map(|k| (k.clone(), Color::White)).collect();
        let mut cycle_members: HashSet<CellId> = HashSet::new();
        // Current DFS stack (path from the root of this walk to "here"),
        // used to materialize every member of a cycle once a back-edge is
        // found, not just the two endpoints.
        let mut stack: Vec<CellId> = Vec::new();

        fn visit(
            node: &CellId,
            edges: &HashMap<CellId, Vec<CellId>>,
            color: &mut HashMap<CellId, Color>,
            stack: &mut Vec<CellId>,
            cycle_members: &mut HashSet<CellId>,
        ) {
            color.insert(node.clone(), Color::Gray);
            stack.push(node.clone());

            if let Some(deps) = edges.get(node) {
                for dep in deps {
                    // A dependency outside the formula-cell set (a plain
                    // value cell, or a cell with no sheet_cells row at
                    // all) has no entry in `edges`/`color` — it can't be
                    // part of a cycle among formula cells, skip it.
                    let Some(&dep_color) = color.get(dep) else {
                        continue;
                    };
                    match dep_color {
                        Color::White => visit(dep, edges, color, stack, cycle_members),
                        Color::Gray => {
                            // Back-edge found: `dep` is on the current
                            // stack. Every node from `dep`'s position to
                            // the top of the stack is a cycle member.
                            if let Some(pos) = stack.iter().position(|n| n == dep) {
                                for member in &stack[pos..] {
                                    cycle_members.insert(member.clone());
                                }
                            }
                        }
                        Color::Black => {} // already fully explored, not on current path
                    }
                }
            }

            stack.pop();
            color.insert(node.clone(), Color::Black);
        }

        // Sort keys for deterministic traversal order (HashMap iteration
        // order is otherwise unspecified) — doesn't change WHICH cells are
        // cycle members, only keeps DFS visitation order reproducible for
        // tests/debugging.
        let mut keys: Vec<CellId> = self.edges.keys().cloned().collect();
        keys.sort();

        for key in &keys {
            if color.get(key) == Some(&Color::White) {
                visit(key, &self.edges, &mut color, &mut stack, &mut cycle_members);
            }
        }

        cycle_members
    }

    /// Kahn's algorithm topological sort over the nodes NOT in
    /// `exclude` (the cycle-member set, already resolved to `#CYCLE!`
    /// separately). Nodes depending on an excluded node still appear in
    /// the graph — their dependency edges into `exclude`d nodes are
    /// simply not counted against their in-degree, since an excluded
    /// node's value is already known (#CYCLE!) before evaluation starts,
    /// same as any other precomputed dependency.
    fn topo_order(&self, exclude: &HashSet<CellId>) -> Vec<CellId> {
        let nodes: Vec<CellId> = self.edges.keys().filter(|k| !exclude.contains(*k)).cloned().collect();
        let node_set: HashSet<CellId> = nodes.iter().cloned().collect();

        // in_degree[n] = number of remaining (non-excluded, in-graph)
        // dependencies of n not yet emitted.
        let mut in_degree: HashMap<CellId, usize> = HashMap::new();
        // reverse_edges[dep] = nodes that depend on dep (dep -> dependents)
        let mut reverse_edges: HashMap<CellId, Vec<CellId>> = HashMap::new();

        for n in &nodes {
            let deps = self.edges.get(n).cloned().unwrap_or_default();
            let relevant_deps: Vec<CellId> = deps.into_iter().filter(|d| node_set.contains(d)).collect();
            in_degree.insert(n.clone(), relevant_deps.len());
            for d in relevant_deps {
                reverse_edges.entry(d).or_default().push(n.clone());
            }
        }

        let mut queue: Vec<CellId> = nodes
            .iter()
            .filter(|n| in_degree.get(*n).copied().unwrap_or(0) == 0)
            .cloned()
            .collect();
        queue.sort(); // deterministic order among independent-at-this-point nodes

        let mut order = Vec::with_capacity(nodes.len());
        let mut i = 0;
        while i < queue.len() {
            let n = queue[i].clone();
            i += 1;
            order.push(n.clone());
            if let Some(dependents) = reverse_edges.get(&n) {
                let mut newly_ready = Vec::new();
                for dependent in dependents {
                    if let Some(deg) = in_degree.get_mut(dependent) {
                        *deg -= 1;
                        if *deg == 0 {
                            newly_ready.push(dependent.clone());
                        }
                    }
                }
                newly_ready.sort();
                queue.extend(newly_ready);
            }
        }

        // Every non-excluded node must appear — `detect_cycles` already
        // pulled every cycle member into `exclude`, so the remaining graph
        // is guaranteed acyclic and Kahn's algorithm always drains fully.
        debug_assert_eq!(order.len(), nodes.len());
        order
    }
}

/// Live, database-backed `CellResolver` (#2339 PR 7). Reads non-formula
/// cell values directly from the snapshot's raw values; for formula
/// cells, reads from the in-progress computed-value map (populated in
/// topological order by `recompute_sheet`, so by the time a dependent
/// formula resolves a dependency, that dependency's entry is already
/// present) — never the database's stale raw formula string, and never a
/// live re-parse/re-evaluate of the dependency (that's what topological
/// ordering is FOR: every dependency is computed before its dependents).
struct LiveCellResolver<'a> {
    snapshot: &'a SheetSnapshot,
    computed: &'a HashMap<CellId, CellValue>,
}

impl<'a> CellResolver for LiveCellResolver<'a> {
    fn resolve_cell(&self, column_id: &str, row_id: &str) -> Option<CellValue> {
        let id = (column_id.to_string(), row_id.to_string());
        if let Some(v) = self.computed.get(&id) {
            return Some(v.clone());
        }
        // `#REF!` contract (`eval.rs`'s `CellResolver::resolve_cell` doc
        // comment): the referent itself (column OR row) no longer exists
        // at all — distinct from "referent valid, no value stored" (which
        // resolves as `None` → `CellValue::Empty`, handled below).
        let column_exists = self.snapshot.ordered_column_ids.iter().any(|c| c == column_id);
        let row_exists = self.snapshot.ordered_row_ids.iter().any(|r| r == row_id);
        if !column_exists || !row_exists {
            return Some(CellValue::Error(FormulaError::Ref));
        }
        match self.snapshot.raw_values.get(&id) {
            None => None, // sparse — referent valid, no stored value at all
            Some(raw) if raw.starts_with('=') => {
                // A formula cell with no computed entry yet: shouldn't
                // happen given topological ordering, but defensively
                // treat as empty rather than panicking or recursing.
                None
            }
            Some(raw) => Some(literal_to_cell_value(raw)),
        }
    }

    fn resolve_range(
        &self,
        start_col: &str,
        start_row: &str,
        end_col: &str,
        end_row: &str,
    ) -> Vec<Option<CellValue>> {
        let ids = self.snapshot.expand_range(start_col, start_row, end_col, end_row);
        if ids.is_empty() {
            return vec![];
        }
        ids.iter().map(|id| self.resolve_cell(&id.0, &id.1)).collect()
    }
}

/// Coerce a non-formula raw stored string into a `CellValue`. Mirrors the
/// grid's storage convention: numbers/booleans are stored as their
/// canonical text form (§33.3.1's checkbox-as-'0'/'1' convention is left
/// as `Text` here — `eval.rs`'s own arithmetic coercion already handles a
/// `Text("1")` cell used numerically, matching the existing
/// `test_checkbox_text_form_also_coerces` PR 6 test). A bare numeric
/// literal is parsed as `Number` so comparisons/arithmetic against it
/// don't need a text-to-number round trip; anything else stays `Text`.
fn literal_to_cell_value(raw: &str) -> CellValue {
    if raw.is_empty() {
        return CellValue::Empty;
    }
    if let Ok(n) = raw.parse::<f64>() {
        return CellValue::Number(n);
    }
    CellValue::Text(raw.to_string())
}

/// The recompute engine's entry point (#2339 PR 7, §33.5.4, Deliverable
/// 2). Loads the sheet's current grid state, builds the dependency graph
/// over its formula cells (full rebuild — Matt ruling 2026-09-03, see
/// `DependencyGraph::build`), detects cycles, and evaluates every formula
/// cell in topological order.
///
/// Returns the freshly computed `HashMap<CellId, CellValue>` — this is
/// the in-memory computed-value store (#2339 PR 7, "no schema changes":
/// computed values are NEVER persisted to `sheet_cells`, which has no
/// `computed_value` column — see the module-level ADR-0066 D4 note in
/// `sheets.rs`). The caller (`sheets.rs`, `sync/engine.rs`) is expected to
/// pass this map straight into `regenerate_sheet_body` in the same
/// mutation/sync-handling pass; nothing here retains it across calls.
///
/// Guard rail (Deliverable 3, Matt ruling 2026-09-03): if the sheet has
/// more than `FORMULA_CELL_SOFT_LIMIT` formula cells, recompute is
/// skipped entirely — logs a warning and returns an empty map. Formula
/// cells continue to store and display their raw formula strings (the
/// pre-PR-7 behavior); this is a deliberate, permanent-for-v1 degradation
/// path, not a crash or a UI freeze.
pub fn recompute_sheet(conn: &Connection, note_id: &str) -> Result<HashMap<CellId, CellValue>, String> {
    #[cfg(test)]
    RECOMPUTE_CALL_COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let snapshot = SheetSnapshot::load(conn, note_id)?;
    Ok(compute_from_snapshot(&snapshot, note_id))
}

/// Test-only call counter (#2874 item 51) — `update_sheet_cells_style_
/// batch_internal`'s whole-point assertion is "ONE recompute for an
/// N-cell batch, not N", and there is no other way to observe that from
/// outside this module (the computed-value map a successful call returns
/// doesn't reveal how many times the underlying pass ran). A global
/// atomic counter, incremented at the top of `recompute_sheet` only under
/// `cfg(test)`, with zero runtime cost/behavior change outside tests.
/// Safe across this crate's serial (`--test-threads=1`, #2962) test run:
/// each test resets the counter before exercising the call path it cares
/// about.
#[cfg(test)]
pub(crate) static RECOMPUTE_CALL_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
pub(crate) fn reset_recompute_call_count() {
    RECOMPUTE_CALL_COUNT.store(0, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(test)]
pub(crate) fn get_recompute_call_count() -> usize {
    RECOMPUTE_CALL_COUNT.load(std::sync::atomic::Ordering::SeqCst)
}

/// Recomputes formula cell values for an arbitrary in-memory grid state
/// — e.g. a historical version-history snapshot envelope — without
/// touching the live database. Shares `recompute_sheet`'s exact
/// parse/graph/cycle/evaluate pipeline via `compute_from_snapshot`, so a
/// snapshot's computed values are guaranteed identical to what
/// `recompute_sheet` would have produced had this state been live at
/// capture time. Read-only: never writes anything anywhere (no
/// `Connection` parameter at all).
pub fn compute_envelope_values(
    ordered_column_ids: Vec<String>,
    ordered_row_ids: Vec<String>,
    raw_values: HashMap<CellId, String>,
) -> HashMap<CellId, CellValue> {
    let snapshot = SheetSnapshot::from_parts(ordered_column_ids, ordered_row_ids, raw_values);
    compute_from_snapshot(&snapshot, "<in-memory envelope>")
}

/// Shared core of `recompute_sheet`/`compute_envelope_values`: formula
/// parse -> dependency-graph build -> cycle detect -> topological
/// evaluate, operating purely on an already-built `SheetSnapshot`
/// (regardless of whether it came from a live DB query via `load` or
/// from in-memory data via `from_parts`). `note_id_for_log` is used only
/// for the soft-limit warning message (a real note ID for the live path,
/// a placeholder string for the DB-free path).
fn compute_from_snapshot(snapshot: &SheetSnapshot, note_id_for_log: &str) -> HashMap<CellId, CellValue> {
    let formula_ids = snapshot.formula_cell_ids();

    // Guard rail: [FORMULA_CELL_SOFT_LIMIT]. Graceful degradation past
    // this — formulas store and display but recompute is skipped.
    // Design phase can tune this value. Matt ruling 2026-09-03.
    if formula_ids.len() > FORMULA_CELL_SOFT_LIMIT {
        eprintln!(
            "[recompute] sheet {} has {} formula cells (soft limit {}) — skipping recompute, formulas will display as raw strings",
            note_id_for_log,
            formula_ids.len(),
            FORMULA_CELL_SOFT_LIMIT
        );
        return HashMap::new();
    }

    // Parse every formula cell up front. A parse failure produces
    // #PARSE! immediately (no graph edges for it — see DependencyGraph's
    // doc comment) rather than aborting the whole recompute pass, per
    // §33.5.3's sync/AI-arrival asymmetry: a formula that couldn't be
    // validated at write time (sync/AI arrival) must still degrade
    // gracefully at recompute time, not block every other cell's
    // computation.
    let mut parsed: HashMap<CellId, AstNode> = HashMap::new();
    let mut computed: HashMap<CellId, CellValue> = HashMap::new();
    for id in &formula_ids {
        let raw = snapshot.raw_values.get(id).cloned().unwrap_or_default();
        let formula_text = raw.strip_prefix('=').unwrap_or(&raw);
        match parse_formula(formula_text) {
            Ok(ast) => {
                parsed.insert(id.clone(), ast);
            }
            Err(e) => {
                computed.insert(id.clone(), CellValue::Error(e));
            }
        }
    }

    let graph = DependencyGraph::build(snapshot, &parsed);
    let cycle_members = graph.detect_cycles();

    // Pre-seed every cycle-member cell with #CYCLE! BEFORE any
    // topological evaluation runs, so a dependent that reads a cycle
    // member's value (via LiveCellResolver) sees #CYCLE! like any other
    // precomputed value — no special-casing needed in the evaluator
    // itself beyond ordinary error propagation.
    for id in &cycle_members {
        computed.insert(id.clone(), CellValue::Error(FormulaError::Cycle));
    }

    let order = graph.topo_order(&cycle_members);

    for id in order {
        let Some(ast) = parsed.get(&id) else {
            continue; // shouldn't happen — every topo_order node came from `parsed`'s keys
        };
        let resolver = LiveCellResolver { snapshot, computed: &computed };
        let mut visited: HashSet<CellId> = HashSet::new();
        let value = evaluate(ast, &resolver, &mut visited);
        computed.insert(id, value);
    }

    computed
}

/// §33.5.3: Local formula edits are validated at write time — parse
/// failures rejected inline. Sync/AI-originated formulas bypass
/// validation (already committed by cr-sqlite) and display #PARSE! on
/// recompute failure.
///
/// Called by `sheets.rs`'s `update_sheet_cell_internal` before any DB
/// write when the incoming `value` is a formula (`=`-prefixed). Not
/// called anywhere in the sync-apply path (`sync/engine.rs::
/// apply_changesets`) — sync writes go straight through cr-sqlite, and
/// this function is never in that call chain, which IS the asymmetry.
pub fn validate_formula_on_write(value: &str) -> Result<(), String> {
    if let Some(formula_text) = value.strip_prefix('=') {
        if let Err(e) = parse_formula(formula_text) {
            return Err(format!("Invalid formula: {}", e));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// Minimal `notes`/`sheet_columns`/`sheet_rows`/`sheet_cells` schema —
    /// enough for `recompute_sheet` to run unmodified against, matching
    /// `sheets.rs`'s own test-schema pattern.
    fn create_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE notes (
                 id TEXT PRIMARY KEY,
                 note_type TEXT
             );
             CREATE TABLE sheet_columns (
                 id TEXT PRIMARY KEY,
                 note_id TEXT NOT NULL,
                 sort_order INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE sheet_rows (
                 id TEXT PRIMARY KEY,
                 note_id TEXT NOT NULL,
                 sort_order INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE sheet_cells (
                 row_id TEXT NOT NULL,
                 column_id TEXT NOT NULL,
                 note_id TEXT NOT NULL,
                 value TEXT NOT NULL DEFAULT '',
                 PRIMARY KEY (row_id, column_id)
             );",
        )
        .unwrap();
        conn
    }

    fn seed_sheet(conn: &Connection, note_id: &str, columns: &[&str], rows: &[&str]) {
        conn.execute("INSERT INTO notes (id, note_type) VALUES (?1, 'sheet')", params![note_id])
            .unwrap();
        for (i, col) in columns.iter().enumerate() {
            conn.execute(
                "INSERT INTO sheet_columns (id, note_id, sort_order) VALUES (?1, ?2, ?3)",
                params![col, note_id, i as i64],
            )
            .unwrap();
        }
        for (i, row) in rows.iter().enumerate() {
            conn.execute(
                "INSERT INTO sheet_rows (id, note_id, sort_order) VALUES (?1, ?2, ?3)",
                params![row, note_id, i as i64],
            )
            .unwrap();
        }
    }

    fn set_cell(conn: &Connection, note_id: &str, col: &str, row: &str, value: &str) {
        conn.execute(
            "INSERT INTO sheet_cells (row_id, column_id, note_id, value) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(row_id, column_id) DO UPDATE SET value = excluded.value",
            params![row, col, note_id, value],
        )
        .unwrap();
    }

    // ---- linear dependency chain ----

    #[test]
    fn test_linear_dependency_chain_multi_level() {
        // A1 = 5, B1 = A1*2, C1 = B1+1 -> C1 = 11
        let conn = create_test_db();
        let note_id = "n1";
        seed_sheet(&conn, note_id, &["A", "B", "C"], &["1"]);
        set_cell(&conn, note_id, "A", "1", "5");
        set_cell(&conn, note_id, "B", "1", "=A1*2");
        set_cell(&conn, note_id, "C", "1", "=B1+1");

        let computed = recompute_sheet(&conn, note_id).unwrap();
        assert_eq!(computed.get(&("B".to_string(), "1".to_string())), Some(&CellValue::Number(10.0)));
        assert_eq!(computed.get(&("C".to_string(), "1".to_string())), Some(&CellValue::Number(11.0)));
    }

    #[test]
    fn test_dependency_chain_recomputes_after_value_change() {
        let conn = create_test_db();
        let note_id = "n1";
        seed_sheet(&conn, note_id, &["A", "B", "C"], &["1"]);
        set_cell(&conn, note_id, "A", "1", "5");
        set_cell(&conn, note_id, "B", "1", "=A1*2");
        set_cell(&conn, note_id, "C", "1", "=B1+1");
        recompute_sheet(&conn, note_id).unwrap();

        set_cell(&conn, note_id, "A", "1", "10");
        let computed = recompute_sheet(&conn, note_id).unwrap();
        assert_eq!(computed.get(&("C".to_string(), "1".to_string())), Some(&CellValue::Number(21.0)));
    }

    #[test]
    fn test_diamond_dependency() {
        // B1 = A1, C1 = A1, D1 = B1+C1
        let conn = create_test_db();
        let note_id = "n1";
        seed_sheet(&conn, note_id, &["A", "B", "C", "D"], &["1"]);
        set_cell(&conn, note_id, "A", "1", "3");
        set_cell(&conn, note_id, "B", "1", "=A1");
        set_cell(&conn, note_id, "C", "1", "=A1");
        set_cell(&conn, note_id, "D", "1", "=B1+C1");

        let computed = recompute_sheet(&conn, note_id).unwrap();
        assert_eq!(computed.get(&("D".to_string(), "1".to_string())), Some(&CellValue::Number(6.0)));
    }

    // ---- cycle detection ----

    #[test]
    fn test_direct_cycle_both_cells_show_cycle_error() {
        let conn = create_test_db();
        let note_id = "n1";
        seed_sheet(&conn, note_id, &["A", "B"], &["1"]);
        set_cell(&conn, note_id, "A", "1", "=B1");
        set_cell(&conn, note_id, "B", "1", "=A1");

        let computed = recompute_sheet(&conn, note_id).unwrap();
        assert_eq!(
            computed.get(&("A".to_string(), "1".to_string())),
            Some(&CellValue::Error(FormulaError::Cycle))
        );
        assert_eq!(
            computed.get(&("B".to_string(), "1".to_string())),
            Some(&CellValue::Error(FormulaError::Cycle))
        );
    }

    #[test]
    fn test_indirect_cycle_all_three_cells_show_cycle_error() {
        let conn = create_test_db();
        let note_id = "n1";
        seed_sheet(&conn, note_id, &["A", "B", "C"], &["1"]);
        set_cell(&conn, note_id, "A", "1", "=B1");
        set_cell(&conn, note_id, "B", "1", "=C1");
        set_cell(&conn, note_id, "C", "1", "=A1");

        let computed = recompute_sheet(&conn, note_id).unwrap();
        for col in ["A", "B", "C"] {
            assert_eq!(
                computed.get(&(col.to_string(), "1".to_string())),
                Some(&CellValue::Error(FormulaError::Cycle)),
                "column {col} should be #CYCLE!"
            );
        }
    }

    #[test]
    fn test_self_reference_shows_cycle_error() {
        let conn = create_test_db();
        let note_id = "n1";
        seed_sheet(&conn, note_id, &["A"], &["1"]);
        set_cell(&conn, note_id, "A", "1", "=A1");

        let computed = recompute_sheet(&conn, note_id).unwrap();
        assert_eq!(
            computed.get(&("A".to_string(), "1".to_string())),
            Some(&CellValue::Error(FormulaError::Cycle))
        );
    }

    #[test]
    fn test_mixed_cycle_and_non_cycle_cells_in_same_sheet() {
        // A1 = B1, B1 = A1 (cycle); C1 = 5, D1 = C1+1 (normal, same sheet)
        let conn = create_test_db();
        let note_id = "n1";
        seed_sheet(&conn, note_id, &["A", "B", "C", "D"], &["1"]);
        set_cell(&conn, note_id, "A", "1", "=B1");
        set_cell(&conn, note_id, "B", "1", "=A1");
        set_cell(&conn, note_id, "C", "1", "5");
        set_cell(&conn, note_id, "D", "1", "=C1+1");

        let computed = recompute_sheet(&conn, note_id).unwrap();
        assert_eq!(
            computed.get(&("A".to_string(), "1".to_string())),
            Some(&CellValue::Error(FormulaError::Cycle))
        );
        assert_eq!(
            computed.get(&("B".to_string(), "1".to_string())),
            Some(&CellValue::Error(FormulaError::Cycle))
        );
        assert_eq!(computed.get(&("D".to_string(), "1".to_string())), Some(&CellValue::Number(6.0)));
    }

    #[test]
    fn test_cycle_detection_is_idempotent() {
        let conn = create_test_db();
        let note_id = "n1";
        seed_sheet(&conn, note_id, &["A", "B"], &["1"]);
        set_cell(&conn, note_id, "A", "1", "=B1");
        set_cell(&conn, note_id, "B", "1", "=A1");

        let first = recompute_sheet(&conn, note_id).unwrap();
        let second = recompute_sheet(&conn, note_id).unwrap();
        assert_eq!(first, second);
    }

    // ---- guard rail ----

    #[test]
    fn test_formula_count_at_soft_limit_recomputes_normally() {
        let conn = create_test_db();
        let note_id = "n1";
        let row_ids: Vec<String> = (0..FORMULA_CELL_SOFT_LIMIT).map(|i| format!("r{i}")).collect();
        let row_refs: Vec<&str> = row_ids.iter().map(|s| s.as_str()).collect();
        seed_sheet(&conn, note_id, &["A"], &row_refs);
        for row in &row_refs {
            set_cell(&conn, note_id, "A", row, "=1+1");
        }

        let computed = recompute_sheet(&conn, note_id).unwrap();
        assert_eq!(computed.len(), FORMULA_CELL_SOFT_LIMIT);
        assert_eq!(computed.get(&("A".to_string(), row_refs[0].to_string())), Some(&CellValue::Number(2.0)));
    }

    #[test]
    fn test_formula_count_past_soft_limit_skips_recompute_gracefully() {
        let conn = create_test_db();
        let note_id = "n1";
        let n = FORMULA_CELL_SOFT_LIMIT + 1;
        let row_ids: Vec<String> = (0..n).map(|i| format!("r{i}")).collect();
        let row_refs: Vec<&str> = row_ids.iter().map(|s| s.as_str()).collect();
        seed_sheet(&conn, note_id, &["A"], &row_refs);
        for row in &row_refs {
            set_cell(&conn, note_id, "A", row, "=1+1");
        }

        // Must not panic/crash/hang — returns an empty map (graceful
        // degradation, raw formula strings continue displaying via
        // sheets.rs's body-regen fallback).
        let computed = recompute_sheet(&conn, note_id).unwrap();
        assert!(computed.is_empty());
    }

    // ---- parse-on-write validation ----

    #[test]
    fn test_validate_formula_on_write_accepts_valid_formula() {
        assert!(validate_formula_on_write("=1+1").is_ok());
        assert!(validate_formula_on_write("=SUM(A1:A5)").is_ok());
    }

    #[test]
    fn test_validate_formula_on_write_rejects_invalid_formula() {
        assert!(validate_formula_on_write("=1+").is_err());
        assert!(validate_formula_on_write("=(((").is_err());
    }

    #[test]
    fn test_validate_formula_on_write_ignores_non_formula_values() {
        // Plain values (no leading '=') are never formula-validated.
        assert!(validate_formula_on_write("hello").is_ok());
        assert!(validate_formula_on_write("42").is_ok());
        assert!(validate_formula_on_write("").is_ok());
    }

    // ---- sync/AI arrival: unparseable formula degrades to #PARSE! at
    // recompute time, doesn't block the rest of the sheet ----

    #[test]
    fn test_unparseable_formula_shows_parse_error_without_blocking_other_cells() {
        let conn = create_test_db();
        let note_id = "n1";
        seed_sheet(&conn, note_id, &["A", "B"], &["1"]);
        // Simulates a sync-arrived malformed formula bypassing
        // validate_formula_on_write (which is only called from the local
        // write path in sheets.rs, not from the sync-apply path).
        set_cell(&conn, note_id, "A", "1", "=1+");
        set_cell(&conn, note_id, "B", "1", "=2+2");

        let computed = recompute_sheet(&conn, note_id).unwrap();
        assert!(matches!(
            computed.get(&("A".to_string(), "1".to_string())),
            Some(CellValue::Error(FormulaError::Parse(_)))
        ));
        assert_eq!(computed.get(&("B".to_string(), "1".to_string())), Some(&CellValue::Number(4.0)));
    }

    // ---- resolve_range empty-Vec contract, exercised via SUM/AVERAGE ----

    #[test]
    fn test_range_referencing_nonexistent_rows_sum_is_zero() {
        let conn = create_test_db();
        let note_id = "n1";
        seed_sheet(&conn, note_id, &["A", "B"], &["1"]);
        // B1's formula ranges over rows 5..10 which don't exist in this
        // sheet at all -> expand_range returns [] -> resolve_range
        // returns [] -> SUM treats empty Vec as summing zero terms.
        set_cell(&conn, note_id, "B", "1", "=SUM(A5:A10)");

        let computed = recompute_sheet(&conn, note_id).unwrap();
        assert_eq!(computed.get(&("B".to_string(), "1".to_string())), Some(&CellValue::Number(0.0)));
    }

    #[test]
    fn test_range_referencing_nonexistent_rows_average_is_div_zero() {
        let conn = create_test_db();
        let note_id = "n1";
        seed_sheet(&conn, note_id, &["A", "B"], &["1"]);
        set_cell(&conn, note_id, "B", "1", "=AVERAGE(A5:A10)");

        let computed = recompute_sheet(&conn, note_id).unwrap();
        assert_eq!(
            computed.get(&("B".to_string(), "1".to_string())),
            Some(&CellValue::Error(FormulaError::DivZero))
        );
    }

    // ---- #REF! contract: referent (column/row) genuinely deleted ----

    #[test]
    fn test_cell_ref_to_nonexistent_column_is_ref_error() {
        let conn = create_test_db();
        let note_id = "n1";
        seed_sheet(&conn, note_id, &["A"], &["1"]);
        // "Z" is not a real column in this sheet at all — the referent
        // itself doesn't exist, distinct from a sparse-but-valid cell.
        set_cell(&conn, note_id, "A", "1", "={Z:1}");

        let computed = recompute_sheet(&conn, note_id).unwrap();
        assert_eq!(
            computed.get(&("A".to_string(), "1".to_string())),
            Some(&CellValue::Error(FormulaError::Ref))
        );
    }

    #[test]
    fn test_cell_ref_to_nonexistent_row_is_ref_error() {
        let conn = create_test_db();
        let note_id = "n1";
        seed_sheet(&conn, note_id, &["A", "B"], &["1"]);
        set_cell(&conn, note_id, "B", "1", "={A:999}");

        let computed = recompute_sheet(&conn, note_id).unwrap();
        assert_eq!(
            computed.get(&("B".to_string(), "1".to_string())),
            Some(&CellValue::Error(FormulaError::Ref))
        );
    }

    #[test]
    fn test_cell_ref_to_sparse_but_valid_cell_is_empty_not_ref_error() {
        // A2 exists (valid row+column) but has no stored value — must
        // resolve as Empty (-> 0 in arithmetic), NOT #REF!.
        let conn = create_test_db();
        let note_id = "n1";
        seed_sheet(&conn, note_id, &["A", "B"], &["1", "2"]);
        set_cell(&conn, note_id, "B", "1", "={A:2}+5");

        let computed = recompute_sheet(&conn, note_id).unwrap();
        assert_eq!(computed.get(&("B".to_string(), "1".to_string())), Some(&CellValue::Number(5.0)));
    }

    #[test]
    fn test_sum_over_range_with_real_rows() {
        let conn = create_test_db();
        let note_id = "n1";
        seed_sheet(&conn, note_id, &["A", "B"], &["1", "2", "3"]);
        set_cell(&conn, note_id, "A", "1", "1");
        set_cell(&conn, note_id, "A", "2", "2");
        set_cell(&conn, note_id, "A", "3", "3");
        set_cell(&conn, note_id, "B", "1", "=SUM(A1:A3)");

        let computed = recompute_sheet(&conn, note_id).unwrap();
        assert_eq!(computed.get(&("B".to_string(), "1".to_string())), Some(&CellValue::Number(6.0)));
    }

    // ---- compute_envelope_values: DB-free sibling of recompute_sheet
    // (item 32, #2874) ----

    #[test]
    fn test_compute_envelope_values_matches_recompute_sheet() {
        // Build the same grid state both as a live in-memory SQLite DB
        // (via recompute_sheet) and as from_parts-built in-memory data
        // (via compute_envelope_values) — assert identical output maps.
        // This directly guards against the highest-risk failure mode of
        // the compute_from_snapshot extraction: breaking the live grid's
        // recompute while adding the new DB-free sibling.
        let conn = create_test_db();
        let note_id = "n1";
        seed_sheet(&conn, note_id, &["A", "B", "C"], &["1", "2"]);
        set_cell(&conn, note_id, "A", "1", "5");
        set_cell(&conn, note_id, "A", "2", "10");
        set_cell(&conn, note_id, "B", "1", "=A1*2");
        set_cell(&conn, note_id, "C", "1", "=SUM(A1:A2)");

        let via_recompute_sheet = recompute_sheet(&conn, note_id).unwrap();

        let ordered_column_ids = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let ordered_row_ids = vec!["1".to_string(), "2".to_string()];
        let mut raw_values: HashMap<CellId, String> = HashMap::new();
        raw_values.insert(("A".to_string(), "1".to_string()), "5".to_string());
        raw_values.insert(("A".to_string(), "2".to_string()), "10".to_string());
        raw_values.insert(("B".to_string(), "1".to_string()), "=A1*2".to_string());
        raw_values.insert(("C".to_string(), "1".to_string()), "=SUM(A1:A2)".to_string());

        let via_compute_envelope_values = compute_envelope_values(ordered_column_ids, ordered_row_ids, raw_values);

        assert_eq!(via_recompute_sheet, via_compute_envelope_values);
    }

    #[test]
    fn test_compute_envelope_values_div_by_zero() {
        let ordered_column_ids = vec!["A".to_string(), "B".to_string()];
        let ordered_row_ids = vec!["1".to_string()];
        let mut raw_values: HashMap<CellId, String> = HashMap::new();
        raw_values.insert(("A".to_string(), "1".to_string()), "0".to_string());
        raw_values.insert(("B".to_string(), "1".to_string()), "=14/A1".to_string());

        let computed = compute_envelope_values(ordered_column_ids, ordered_row_ids, raw_values);
        assert_eq!(
            computed.get(&("B".to_string(), "1".to_string())),
            Some(&CellValue::Error(FormulaError::DivZero))
        );
    }

    #[test]
    fn test_compute_envelope_values_cycle() {
        let ordered_column_ids = vec!["A".to_string(), "B".to_string()];
        let ordered_row_ids = vec!["1".to_string()];
        let mut raw_values: HashMap<CellId, String> = HashMap::new();
        raw_values.insert(("A".to_string(), "1".to_string()), "=B1".to_string());
        raw_values.insert(("B".to_string(), "1".to_string()), "=A1".to_string());

        let computed = compute_envelope_values(ordered_column_ids, ordered_row_ids, raw_values);
        assert_eq!(
            computed.get(&("A".to_string(), "1".to_string())),
            Some(&CellValue::Error(FormulaError::Cycle))
        );
        assert_eq!(
            computed.get(&("B".to_string(), "1".to_string())),
            Some(&CellValue::Error(FormulaError::Cycle))
        );
    }

    #[test]
    fn test_compute_envelope_values_normal_formula_computed_number() {
        let ordered_column_ids = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let ordered_row_ids = vec!["1".to_string()];
        let mut raw_values: HashMap<CellId, String> = HashMap::new();
        raw_values.insert(("A".to_string(), "1".to_string()), "3".to_string());
        raw_values.insert(("B".to_string(), "1".to_string()), "4".to_string());
        raw_values.insert(("C".to_string(), "1".to_string()), "=A1+B1".to_string());

        let computed = compute_envelope_values(ordered_column_ids, ordered_row_ids, raw_values);
        assert_eq!(computed.get(&("C".to_string(), "1".to_string())), Some(&CellValue::Number(7.0)));
    }

    #[test]
    fn test_compute_envelope_values_non_formula_cell_excluded() {
        let ordered_column_ids = vec!["A".to_string(), "B".to_string()];
        let ordered_row_ids = vec!["1".to_string()];
        let mut raw_values: HashMap<CellId, String> = HashMap::new();
        raw_values.insert(("A".to_string(), "1".to_string()), "plain literal".to_string());
        raw_values.insert(("B".to_string(), "1".to_string()), "=1+1".to_string());

        let computed = compute_envelope_values(ordered_column_ids, ordered_row_ids, raw_values);
        assert!(!computed.contains_key(&("A".to_string(), "1".to_string())));
        assert_eq!(computed.get(&("B".to_string(), "1".to_string())), Some(&CellValue::Number(2.0)));
    }

    #[test]
    fn test_compute_envelope_values_a1_translation_uses_snapshot_column_order() {
        // A formula stored in ID-form referencing another cell by
        // stable-ID — translate_formula_ids_to_a1 should resolve it to
        // A1 notation based on THIS snapshot's own column/row ordering.
        let ordered_column_ids = vec!["col-id-1".to_string(), "col-id-2".to_string()];
        let ordered_row_ids = vec!["row-id-1".to_string()];
        let formula = "={col-id-1:row-id-1}";
        let a1 = crate::formula::reference_mapper::translate_formula_ids_to_a1(
            formula,
            &ordered_column_ids,
            &ordered_row_ids,
        );
        assert_eq!(a1, "=A1");
    }

    // ---- item 37 (#2874): single-row ranges ----

    #[test]
    fn test_evaluate_sum_over_row_range() {
        // SUM(A1:C1) with A1=10, B1=20, C1=30 -> 60.
        let conn = create_test_db();
        let note_id = "n1";
        seed_sheet(&conn, note_id, &["A", "B", "C", "D"], &["1"]);
        set_cell(&conn, note_id, "A", "1", "10");
        set_cell(&conn, note_id, "B", "1", "20");
        set_cell(&conn, note_id, "C", "1", "30");
        set_cell(&conn, note_id, "D", "1", "=SUM(A1:C1)");

        let computed = recompute_sheet(&conn, note_id).unwrap();
        assert_eq!(computed.get(&("D".to_string(), "1".to_string())), Some(&CellValue::Number(60.0)));
    }

    #[test]
    fn test_row_range_uses_stored_column_order_at_evaluation_time() {
        // Row range stored as ID-form `{col-a:row-0}:{col-c:row-0}` must
        // expand using the CURRENT `ordered_column_ids` order at
        // evaluation time, not whatever order existed when the formula
        // was written. col-x sits OUTSIDE the range in the original
        // order but gets reordered to sit BETWEEN col-a and col-c — if
        // the range picks up the live order, col-x's value must be
        // included in the sum after reordering.
        let mut raw_values: HashMap<CellId, String> = HashMap::new();
        raw_values.insert(("col-a".to_string(), "row-0".to_string()), "1".to_string());
        raw_values.insert(("col-x".to_string(), "row-0".to_string()), "100".to_string());
        raw_values.insert(("col-c".to_string(), "row-0".to_string()), "3".to_string());
        raw_values.insert(
            ("col-d".to_string(), "row-0".to_string()),
            "=SUM({col-a:row-0}:{col-c:row-0})".to_string(),
        );

        // Original order: col-x sits AFTER col-c, so it's outside the
        // a..c range -> sum = 1 + 3 = 4.
        let original_order = vec![
            "col-a".to_string(),
            "col-c".to_string(),
            "col-x".to_string(),
            "col-d".to_string(),
        ];
        let original =
            compute_envelope_values(original_order, vec!["row-0".to_string()], raw_values.clone());
        assert_eq!(
            original.get(&("col-d".to_string(), "row-0".to_string())),
            Some(&CellValue::Number(4.0))
        );

        // Reordered: col-x now sits BETWEEN col-a and col-c -> it's
        // INSIDE the a..c range by live sort order -> sum = 1+100+3=104.
        let reordered_order = vec![
            "col-a".to_string(),
            "col-x".to_string(),
            "col-c".to_string(),
            "col-d".to_string(),
        ];
        let reordered = compute_envelope_values(reordered_order, vec!["row-0".to_string()], raw_values);
        assert_eq!(
            reordered.get(&("col-d".to_string(), "row-0".to_string())),
            Some(&CellValue::Number(104.0))
        );
    }

    #[test]
    fn test_insert_column_inside_row_range_extends_it() {
        // Snapshot with row range A1:C1 (ids col-a..col-c); insert a 4th
        // column between B and C. `expand_range` must now return 4
        // cells, not 3 — comes free from the sort-order-based membership
        // model (no separate range-bounds bookkeeping to update).
        let ordered_column_ids = vec![
            "col-a".to_string(),
            "col-b".to_string(),
            "col-new".to_string(),
            "col-c".to_string(),
        ];
        let ordered_row_ids = vec!["row-0".to_string()];
        let snapshot = SheetSnapshot::from_parts(ordered_column_ids, ordered_row_ids, HashMap::new());
        let expanded = snapshot.expand_range("col-a", "row-0", "col-c", "row-0");
        assert_eq!(expanded.len(), 4);
        assert_eq!(
            expanded,
            vec![
                ("col-a".to_string(), "row-0".to_string()),
                ("col-b".to_string(), "row-0".to_string()),
                ("col-new".to_string(), "row-0".to_string()),
                ("col-c".to_string(), "row-0".to_string()),
            ]
        );
    }

    #[test]
    fn test_expand_range_mixed_shape_defensive_empty() {
        // A malformed mixed-shape range (different column AND different
        // row) reaching expand_range directly must return an empty Vec,
        // not panic — structurally unreachable post-reference_mapper
        // rejection, but defended against here too.
        let snapshot = SheetSnapshot::from_parts(
            vec!["col-a".to_string(), "col-b".to_string()],
            vec!["row-0".to_string(), "row-1".to_string()],
            HashMap::new(),
        );
        let expanded = snapshot.expand_range("col-a", "row-0", "col-b", "row-1");
        assert_eq!(expanded, Vec::<CellId>::new());
    }

    #[test]
    fn test_expand_range_column_range_regression() {
        // Existing A1:A5-shaped expand_range behavior unchanged.
        let snapshot = SheetSnapshot::from_parts(
            vec!["col-a".to_string()],
            vec!["row-0".to_string(), "row-1".to_string(), "row-2".to_string()],
            HashMap::new(),
        );
        let expanded = snapshot.expand_range("col-a", "row-0", "col-a", "row-2");
        assert_eq!(
            expanded,
            vec![
                ("col-a".to_string(), "row-0".to_string()),
                ("col-a".to_string(), "row-1".to_string()),
                ("col-a".to_string(), "row-2".to_string()),
            ]
        );
    }
}

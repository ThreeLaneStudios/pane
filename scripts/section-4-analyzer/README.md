# Section 4 AST Analyzer

AST-based analyzer for enforcing §4 of the Dev Hygiene Playbook: integration tests must call production entry points.

Part of ADR-0016 (Mechanical §4 Enforcement), Step 1.

## What It Does

Parses Rust integration test files and verifies that each `#[test]` or `#[tokio::test]` function:
- Calls at least one production symbol (from the provided symbol set)
- OR has a valid allowlist annotation

## Usage

```bash
section-4-analyzer <test-file> <production-symbols-json>
```

**Arguments:**
- `<test-file>`: Path to a Rust test file (`*_integration_test.rs` or under `tests/integration/`)
- `<production-symbols-json>`: JSON file containing array of production function names

**Exit codes:**
- `0`: No violations found
- `1`: Violations found (output as JSON to stdout)

## Allowlist Annotations

### `// §4-helper-test: <justification>`
Suppresses the production-call check for tests that legitimately test helpers rather than production entry points.

**Requirements:**
- Justification text must be non-empty and non-whitespace
- Place in a doc comment above the test function

**Example:**
```rust
/// §4-helper-test: Tests internal threshold calculation before integration
#[test]
fn test_threshold_helper() {
    assert_eq!(calculate_threshold(100), 50);
}
```

### `// §4-untestable: #NNN`
Suppresses the production-call check for production code that is architecturally untestable.

**Requirements:**
- Must reference a valid GitHub issue number (`#NNN` format)
- Issue-open verification is handled separately (Step 4 of ADR-0016, not this analyzer)

**Example:**
```rust
/// §4-untestable: #774
#[test]
fn test_requires_app_handle() {
    // AppHandle coupling prevents direct testing
    let helper_result = setup_test_state();
    assert!(helper_result.is_ok());
}
```

## Violation Types

1. **NoProductionCall** - Test function contains no calls to production symbols
2. **InvalidHelperAnnotation** - `§4-helper-test` annotation has empty justification
3. **UntestableMissingIssue** - `§4-untestable` annotation has invalid issue reference format

## Production Symbols

The production symbols JSON should contain an array of function names exported from production modules:

```json
[
  "route_memory_facts",
  "create_gateway",
  "send_request",
  "process_request"
]
```

**Included:**
- Functions defined in `src-tauri/src/` or `src-gateway/src/`
- Visible to tests (`pub` or `pub(crate)`)

**Excluded:**
- Functions in test modules (`*_test*.rs`, `*_tests.rs`, `mod tests { }`)
- Functions in test utilities (`src-tauri/src/test_utils/`, `src-gateway/tests/common/`)
- Standard library and third-party crate functions

Symbol set generation (Step 2 of ADR-0016) is handled by a separate script.

## Conservative Analyzer

The analyzer is designed to minimize false positives (erring on the side of passing tests):
- Method calls, closures, and macro invocations count toward production calls
- When in doubt about whether a call is production code, the analyzer passes the test
- False negatives are acceptable; false positives are not (review friction)

## Testing

Run the test suite:

```bash
cargo test
```

The test fixtures in `tests/fixtures/` validate all acceptance criteria from the ADR.

## Integration

This analyzer is used in CI workflows (Step 3 of ADR-0016) to enforce §4 mechanically on every PR touching integration test files.

See:
- `docs/adr/0016-mechanical-section-4-enforcement.md` - Full ADR
- `docs/audits/2026-04-29-section-4-workflow-stubs.md` - Implementation spec

## `check-turn-origin` (§36.11, #3059)

A second, differently-shaped rule, added for the Pane runtime sprint's
"the scheduler is the only component that starts a turn nobody typed"
invariant (`docs/spec/runtime.md` §36.11).

```bash
section-4-analyzer check-turn-origin <source-dir-1> [source-dir-2] ...
```

**What it does:** walks every `.rs` file under the given directories and
flags every `TurnOrigin::User` / `TurnOrigin::Job` *construction* expression
(unit, tuple, or struct variant shape) whose enclosing function is not on
the `ALLOWED_CONSTRUCTORS` allowlist in `src/turn_origin.rs`.

**Why this is a different shape than `analyze` above:** `analyze` visits
*inside* one already-known test function, looking for a call to a known
symbol ("does this function call X"). `check-turn-origin` has no starting
function in hand — it finds every construction in the whole tree first,
then asks "what function encloses this one" (the inverse traversal).

**`#[cfg(test)]` is exempt, deliberately** — code that never ships in a
release binary can never produce a turn nobody typed in a real session. This
is an AST-level (attribute) exemption, not the filename-pattern exclusion
`collector.rs::is_excluded_path` uses for symbol collection — every `.rs`
file under the given roots is scanned; `#[cfg(test)]` decides exemption.

**Fails closed:** the allowlist is checked by function name. A construction
under an unlisted name is always a violation — there is no "only check if
the allowlisted function doesn't already have one" mode. Until PR 2 of the
runtime sprint (§36.3) builds the scheduler tick, the real tree has zero
`TurnOrigin` constructions and the gate passes vacuously — provably correct,
not provably vacuous, because its own fixture tests (`src/turn_origin.rs`'s
`#[cfg(test)] mod tests`) exercise the allow/deny logic directly against
synthetic source, independent of whether the real enum exists yet.

**Exit codes:** `0` no violations, `1` violations found (JSON report on stdout).

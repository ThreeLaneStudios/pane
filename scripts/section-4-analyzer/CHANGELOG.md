# Changelog - Section 4 Analyzer

All notable changes to the §4 integration test analyzer will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [Unreleased]

### Added
- Production symbol collector that walks Pane source tree (`src-tauri/src/`, `src-gateway/src/`)
- `collect-symbols` subcommand that outputs JSON list of all `pub` and `pub(crate)` functions
- Excludes test modules, test utility directories, and test-pattern files
- Tests validate against actual Pane repository structure
- Verifies known production symbols from `memory_router.rs` are collected
- Verifies test-only functions and `mod tests { }` blocks are excluded

### Changed
- `main.rs` now uses subcommand structure (`analyze` and `collect-symbols`)
- Previous single-command interface moved to `analyze` subcommand

## [0.1.0] - 2026-04-30

### Added
- Initial analyzer scaffold with `syn`-based AST parsing
- Support for detecting §4 violations (stub-shaped tests)
- `§4-helper-test:` and `§4-untestable:` annotation support
- JSON violation output format
- Integration tests for all analyzer scenarios

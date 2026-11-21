pub mod provider;
pub mod credential_migration;
pub mod anthropic;
pub mod ollama;
pub mod openrouter;
// #3097: `commands.rs` (46,056 lines) split by responsibility — see
// `docs/architecture-cheat-sheet.md` "AI backend module map" for the
// file -> responsibility -> spec-section table.
pub mod chat;
pub mod ingest;
pub mod scheduler;
pub mod runtime_constants;
pub mod connections;
pub mod canonical_format;
pub mod tools;
pub mod adapter;
pub mod extraction_worker;
pub mod audit_markers;
pub mod context_budget;
pub mod memory_router;
pub mod consolidator;
pub mod runtime_resolution;
pub mod net_guard;
pub mod web;
pub mod image_gen;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod memory_integration_test;

# ADR-0047: Local pipeline progress reported via callback, not AppHandle injection

**Status:** Accepted

**Date:** 2026-06-24

**Context.** The embedding pipeline (`embeddings.rs`) is a pure Rust library — no Tauri
dependency, unit-testable in isolation. #637 needs its progress (model download, indexing)
surfaced to the frontend. The granular signals originate inside the library; the Tauri
`AppHandle` lives in `lib.rs`.

**Decision.** The library reports progress through a caller-supplied callback
(`Fn(EmbeddingProgress) + Send + Sync`) and defines its own progress type. `lib.rs` owns the
Tauri `emit_all` inside the closure it passes in. The library never receives an `AppHandle`.
Coarse stages with no in-library granularity are emitted directly from `lib.rs`.

**Alternatives rejected.**

- *Emit only at `lib.rs` call sites:* loses per-file-download and per-25-item granularity (the
  whole point of #637).
- *Plumb `Option<AppHandle>` into the library:* couples a portable library to the Tauri runtime
  and breaks isolated testing.

**Consequences.** Embedding library stays decoupled and testable; full granularity preserved.
Establishes the pattern for any future local-pipeline→frontend progress (import, backup,
sync) — report via callback, emit at the Tauri-aware boundary.

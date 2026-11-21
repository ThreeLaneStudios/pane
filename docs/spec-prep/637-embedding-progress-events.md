# Embedding Progress Events

*Line citations predate the #3097 module split.*
> **Status:** NOT BUILT — #637
> **Relates to:** memory-pipeline.md §5.9 (Memory Search), ADR-0047 (library-boundary callback)

The embedding pipeline (model download + indexing) runs in a background thread at startup
(memory-pipeline.md §5.9; `lib.rs` SCOPE 2). Today it reports only via `eprintln!` to stderr —
the frontend has no way to show download progress (the model is ~438 MB), indexing progress
(thousands of items), readiness, or failure. This spec defines a Tauri event contract that
surfaces pipeline state to the UI without blocking search.

Embeddings run locally regardless of provider topology, so the §29 dual-provider boundary does
not apply here.

## 1. Library boundary (ADR-0047)

`embeddings.rs` is a pure Rust library with no knowledge of Tauri. It MUST NOT take an
`AppHandle`. Progress is reported through a caller-supplied callback:

- `embeddings.rs` defines a library-local progress type (e.g. `EmbeddingProgress`) carrying
  only the data the library has — never a Tauri type.
- The granular functions accept a callback parameter, `progress: impl Fn(EmbeddingProgress) + Send + Sync`:
  - `download_model(...)` — fires per file (start + complete).
  - `run_embedding_pipeline(...)` — fires every 25 items per sub-pass (§3 throttle).
- `lib.rs` constructs the callback, capturing the `AppHandle`, and is the **sole** owner of
  `app_handle.emit_all("embeddings://status", payload)`.
- Coarse stages with no in-library granularity (`ram_check`, `loading`, `ready`, `error`,
  `disabled`) are emitted directly at the `lib.rs` call sites — no callback needed.

This keeps the embedding library portable and unit-testable without a Tauri runtime, while
preserving full progress granularity.

## 2. Event contract

Single unified event, `embeddings://status`, with a `stage` discriminant. The frontend
subscribes once.

```typescript
type EmbeddingsStatusPayload =
  | { stage: "ram_check"; ram_gb: number; enabled: boolean }
  | { stage: "downloading"; file: string; bytes_received?: number; total_bytes?: number }
  | { stage: "loading" }
  | { stage: "indexing"; notes: number; facts: number; chats: number;
      notes_total: number; facts_total: number; chats_total: number }
  | { stage: "ready"; notes: number; facts: number; chats: number; failed?: number }
  | { stage: "error"; message: string; phase: "download" | "engine" | "pipeline" }
  | { stage: "disabled"; reason: "low_ram" | "user_override"; ram_gb?: number };
```

All events are transient IPC (§5). The event name and payload variants are the contract;
adding a field is backward-compatible, renaming `stage` values is breaking.

## 3. Emit locations & throttle

| Stage | Emit owner | Location | When |
|---|---|---|---|
| `ram_check` | lib.rs (direct) | `check_ram_sufficient()` call site | after RAM check returns |
| `disabled` | lib.rs (direct) | embeddings-disabled branch | RAM check fails or user override false |
| `downloading` | **callback** → lib.rs emit | `download_model()` internals | before each file download; after each completes |
| `loading` | lib.rs (direct) | before `get_engine()` call | before ONNX/BERT engine init |
| `indexing` | **callback** → lib.rs emit | `run_embedding_pipeline()` internals | per sub-pass, batched every 25 items |
| `ready` | lib.rs (direct) | after `run_embedding_pipeline` succeeds | pipeline complete |
| `error` | lib.rs (direct) | each error branch | `phase` = download/engine/pipeline |

Locations anchored to symbols, not line numbers — confirm at build time.

**Throttle.** `indexing` MUST batch — emit at most once per 25 items per sub-pass, not per item.
Per-item emission across thousands of items would flood the IPC bus. The final item of each
sub-pass should flush so counts land on their totals.

## 4. Frontend contract

- **A single listener** subscribes to `embeddings://status` (a status component — sidebar
  footer / search-area badge / settings AI panel; exact placement is Lane's call).
- **Render per stage:**

  | Stage | UI |
  |---|---|
  | `disabled` | grey badge "Semantic search off", tooltip "Needs 16 GB RAM" / "Turned off in Settings" |
  | `downloading` | spinner + "Downloading AI model (file X of 3)…", progress if `total_bytes` present |
  | `loading` | spinner + "Loading model…" |
  | `indexing` | counter/progress "Indexing N / Total…" (notes → facts → chats) |
  | `ready` | green dot, optional brief toast "Semantic search ready" (auto-dismiss ~3 s) |
  | `error` | warning icon + "Semantic search unavailable", details in tooltip |

- **Search is never blocked.** FTS results MUST always appear immediately. While
  `downloading`/`loading`/`indexing`, the UI MAY mark results provisional (FTS-only), but MUST
  NOT gate the search box on embedding readiness (memory-pipeline.md §5.9 graceful degradation).
  The Settings → AI panel shows a persistent status line for all stages including `disabled`.

## 5. Persistence — durable vs transient

- **Durable (`ai_settings`):** `embeddings_status` is the authoritative terminal state, written
  by the pipeline today — `"ready"`, `"download error: …"`, `"error: …"`. `embeddings_enabled`
  holds the RAM/override gate. These survive restart and are the source of truth for the
  `search_memory` semantic gate (`commands.rs` reads `embeddings_status == "ready"`).
- **Transient (IPC only):** all `embeddings://status` events. Progress stages
  (`downloading`/`loading`/`indexing`) have **no** durable counterpart — they exist only for the
  current run and are not stored.
- **Reconciliation on frontend mount:** read the durable `embeddings_status` for the current
  terminal state (ready / error / building), then subscribe to `embeddings://status` for live
  transitions. A client that connects mid-run sees the durable state first and live progress
  thereafter; a client that misses the `ready` event still reflects readiness from the durable
  key. Emitting an event MUST NOT be the only signal of a terminal state — the durable write
  remains authoritative.

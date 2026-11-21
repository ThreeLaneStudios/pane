# ADR-0048: Semantic search is opt-in, multi-engine; no automatic model download

**Status:** Accepted — supersedes the #66 decision (auto local semantic search, RAM-gated auto-enable + auto-download, 2026-03-15).

> **Amended (2026-06-24, pre-build — never shipped):** The local `bge-base-en-v1.5` engine is removed. Semantic search is **OpenRouter or Ollama only**, both via HTTP to the provider's embeddings endpoint; Pane runs no model in-process and downloads nothing. The privacy path is **Ollama** (the user's own instance), not a Pane-bundled local model. Removing the local engine also drops the candle / tokenizers native-ML dependency (binary-size reduction). Amended in place rather than superseded because ADR-0048 was accepted the same day and nothing was built against it.

**Date:** 2026-06-24

**Context.** #66 shipped semantic search that auto-enabled when ≥16 GB RAM was detected and auto-downloaded a ~438 MB local model at first run, with no user action. This forces a large background download and on-device indexing on users who never asked for it, and bakes in a single local engine. FTS5 keyword search already covers the default case well and needs nothing downloaded.

**Decision.** Semantic search is **opt-in, default OFF**. Nothing downloads or indexes until the user enables it in Settings → AI → Semantic Search and picks an engine. Engines: Ollama (`nomic-embed-text` etc.) or OpenRouter (`text-embedding-3-*`), both via HTTP to the provider's embeddings endpoint. *(Amended 2026-06-24: local `bge-base-en-v1.5` removed — no Pane-side model download.)* FTS5 remains the always-on default for everyone. Switching engines invalidates the index and triggers a full re-index (dimensions differ). RAM-based auto-enable is removed.

**Rationale.** Privacy and consent: no forced third-party or large local download. Users who want semantic search choose how — Ollama for fully private embeddings on their own machine, OpenRouter for those who accept a hosted API. *(Amended 2026-06-24: the privacy path is Ollama, not a Pane-bundled local model.)* Anthropic has no embeddings API, so Anthropic Smart-Features users get Ollama unless they add an OR key. *(Amended 2026-06-24: "local" removed — no Pane-side local engine.)*

**Consequences.** Default installs do zero embedding work — lighter, faster, no surprise downloads. Multi-engine support adds an engine-selection + re-index path. `embeddings_enabled` defaults false and is user-set, not RAM-derived. Downstream `search_memory` semantic gate (`embeddings_status == "ready"`) is unchanged. *(Amended 2026-06-24:)* with the local engine removed, Pane downloads no model and ships no in-process inference — dropping the candle / tokenizers native-ML dependency for a smaller binary. Both engines are HTTP calls.

**Supersedes:** #66 (2026-03-15, "Adaptive embeddings: auto-enabled ≥16GB RAM, FTS5 always active"). That ADR/decision-log entry is left intact; this ADR records the reversal.

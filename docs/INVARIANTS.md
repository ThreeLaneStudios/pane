# INVARIANTS.md — Hard Constraints (Never Violate)
# Last updated: 2026-04-27
# Read this BEFORE every code change. These survive compaction.

## Agent Architecture (§24)
- NEVER create "Pane System" as a project in the `projects` table
- NEVER write agent data to the `projects` table — agents live in `ai_agents` only
- NEVER auto-create "General" or "AI" folders for agents
- NEVER call verify_system_notes_integrity() when ai_agents table exists
- Agents are managed via Settings → Agents, NOT the sidebar
- Agent identity files are AI-managed and protected (read-only in UI)

## Memory System (§5)
- NEVER modify `ai_memory` table structure without explicit Matt approval
- NEVER change memory extraction, pipeline, or decay logic as a side effect of other work
- NEVER write to Memory notes from chat (append_to_note → daily log ONLY)
- **Writer ownership:** The router appends or removes bullets in memory files (never rewrites). The consolidator rewrites memory files (never appends). The end-of-day agent deduplicates and headers daily logs (never touches memory files). No other code path writes to memory files or daily logs.
- **`ai_memory` is the source of truth for memory content.** Memory files (`MEMORY.md`, project Memory notes) are rendered views of the active fact list. The consolidator reads from `ai_memory WHERE is_active = 1` plus existing prose for style; it does not treat the existing file as authoritative content.
- **Pane editor opens all system notes read-only.** Any note with a non-null `note_type` (memory, daily, soul, rules, identity, user, tools) cannot be edited through the Pane editor UI. Memory Settings is the write surface for memory facts; the onboarding wizard creates soul/rules/identity/user/tools at first run, but post-onboarding UI editing of these is deferred (ADR-0011); daily logs are written only by the memory pipeline. Agents settings is the view surface for system notes (with per-file token cost). Direct filesystem edits at `~/Pane/.pane/system/{agent}/` exist as an unsupported escape valve. For memory files specifically, filesystem edits are overwritten by the next consolidation; for other system notes, filesystem edits persist but bypass any UI validation. See ADR-0011.
- **Daily logs are append-only.** All memory mutations (save, edit, delete) append audit-trail entries using the `[saved]`, `[memory edited]`, and `[memory deleted]` markers. Daily log entries are never rewritten in place. Audit-trail markers are not re-extracted by the extraction pipeline.
- Source_message_id dedup: one extraction per message, enforced

## Pane Gateway (§22C)

Sync transport, multi-tenancy seam, and chat/session protocol for split-machine Tier 3. Closes bridge audit gap #7 (sync INVARIANTS missing entirely, 2026-04-27). Each entry cross-references its §22C subsection; ADR references lock the architectural decisions.

- **Gateway is the sole sync transport for split-machine Tier 3.** Bridge plugin is legacy post-cutover (§22C.16). No two-transport coexistence in Pane client logic — Pane checks `ai_settings.tier3_topology` at startup and routes statically. (§22C.1, ADR-0012)
- Same-machine Tier 3 never deploys the gateway. Filesystem-direct: symlink for sync, direct `~/.openclaw/openclaw.json` read for model discovery, existing OpenRouter adapter for chat. (§22C.11, ADR-0012)
- Tier 2 never routes through the gateway. BYOK is direct-to-provider. (§22C.1, ADR-0012)
- **Gateway terminates TLS architecturally.** No assumption that the network layer is encrypted. Self-signed cert + fingerprint pinning during pairing at launch; ACME support is a calibration question, not a gap. (§22C.4.3, ADR-0012)
- **Cross-tenant access is impossible by construction, not by check.** Every gateway state row carries `tenant_id`; every query is tenant-scoped. No global queries. Single-tenant at launch (`default`); Tier 1 hosted is a config flip, not a refactor. (§22C.5, ADR-0013)
- Bearer token is opaque and gateway-resolved at launch. No JWT until claim-based authorization warrants it; `token_type` discriminator preserves the migration path. (§22C.5.1, ADR-0013)
- `oc_agent_id` is the routing key for sync. Names are display-only. Carries forward from migration #364; renames must not break sync. (§22C.10)
- **Sync queue invariants port from bridge:** max 5 retries, RFC3339 timestamps, max 100 items per flush cycle. **Never `datetime('now')` in queue queries** — lexicographic incompatibility caught in PR #645. (§22C.8.3)
- Atomic write + read-back checksum is mandatory on the gateway side. Prevents torn writes; carries forward from bridge plugin discipline. (§22C.8.3)
- **WS subscription gaps are invisible to the Pane client.** Per-session ring buffer (1000 events / 5 min) + `replay.gap` reset semantics + paginated refetch via `GET /v1/sessions/:id/messages?since=...`. Streaming reliability is load-bearing for chat UX. (§22C.7.3)
- No `chat.send` or `chat.abort` calls from the gateway. `sessions.send` and `sessions.abort` are canonical. `chat.history` is permitted because no `sessions.history` parallel exists in OC's surface. (§22C.7.1, ADR-0015)
- Gateway invokes only the nine OC WS methods in the §22C.7.1 whitelist. Adding methods is an intentional spec change, not implementation drift. (§22C.7.1, ADR-0015)
- Payload caps are hard rejects, not warnings. 10 MiB push, 1 MiB session message. (§22C.13)
- **Gateway external API changes require a ClawHub skill version review.** Skill is published manually by Matt via clawhub CLI.
- **Reconciliation logic lives in Pane; the gateway is transport only.** Timestamp comparison, LWW conflict resolution, content-identity skip, and atomic-write decisions are all Pane-side. Logic sourced from `pull_pending_changes()` lines 688–743 and shared between the gateway client (split-machine) and the watcher startup pass (same-machine). The gateway moves bytes; it does not decide which version of a file wins. (§22C.8.2, §22C.11.2a)
- **Sync ingest MUST NOT write outside `$HOME/Pane/.pane/system/{uuid}/`.** `file_ops::safe_path_join` is the single enforcement point. Every path constructed from external (gateway-supplied or migration-supplied) filenames MUST go through it. Defense-by-allowlist alone is insufficient — the allowlist is scope filtering (ADR-0041); `safe_path_join` is filesystem sandboxing. The two controls are complementary, not alternatives. (`#1300`, §22C.8.5, §22C.18 INVARIANT #15)
- **Ingest writes MUST use `WriteOrigin::Remote`.** Files written by `pull_pending_changes` and `initial_sync` (and any future ingest/recovery flows) MUST call `file_ops::atomic_write` with `WriteOrigin::Remote` to suppress `trigger_sync`. Calling `trigger_sync` on an ingest write creates a ping-pong loop (ingest → re-push → ingest cycle). Canonical write-back after ingest is handled by the explicit `flush_sync_queue` / `flush_sync_queue_blocking` call at the end of the ingest loop — not by `trigger_sync`. Local/AI writes MUST use `WriteOrigin::Local`. (§22C.8.6, #1298)

## Notifications (§19)

Notification system constraints. Cross-references the full spec at `docs/spec/notifications.md`. ADR references lock the architectural decisions.

- **Notification triggers are event-driven, not cron-based.** Due-date watcher runs on app focus (debounced ≤1/60s), 5 min after app start, and on `update_note` for affected tasks. No timer thread, no background scheduler. (`docs/spec/notifications.md` §19.5, ADR-0021)
- **Every multi-fire trigger uses an idempotency key.** Format: `{type_prefix}:{stable_identifier}:{threshold_or_dimension}`. Dedup is enforced by indexed SELECT-before-INSERT, not a UNIQUE constraint. The `emit_notification` Rust function requires an explicit `idem_key` argument (callers may pass `None` only by conscious choice). (`docs/spec/notifications.md` §19.5, ADR-0021)
- **Only coordinator agents can emit `ai_nudge` notifications.** The `emit_notification` AI tool is gated at the dispatcher on `ai_agents.is_coordinator = 1`. Sub-agents that call the tool receive a tool error, returned to the AI per §4.4. Default `is_coordinator = 0`; the seeded coordinator at install is Brit. (`docs/spec/notifications.md` §19.5.3, ADR-0021)
- **Notification type is a named string constant, not a Rust enum — corrected 2026-10-29 (#3050 PR 6, Ruling 5).** This bullet previously asserted a `NotificationType` enum in `notifications.rs`; none has ever existed (`grep -rn "NotificationType" src-tauri/src/` returns zero hits — confirmed before correcting). Every type, `info` through this PR's `agent_message`/`job_failed`, is a plain `&str` with no `CHECK` constraint on the `type` column; the DB column stays `TEXT` for forward-compat as originally stated. Adding a type means adding a named `&str` constant (mirroring `refresh_event.rs`'s `ENTITY_NOTE`/`ENTITY_SHEET` pattern) and the corresponding match arms in the panel renderer and channel router — no compile-time arm-completeness check exists today; this is documenting the actual mechanism, not proposing a stronger one. (`docs/spec/notifications.md` §19.2)
- **Notifications expire automatically by type.** Type-specific `expires_at` defaults; a single SQL update in the same event tick that runs `check_due_dates()` auto-dismisses expired rows. Discipline-only "remember to dismiss" is not the model. (`docs/spec/notifications.md` §19.4, ADR-0021)
- **Read and dismiss state for notifications is per-device.** The `notifications` table is not a CRR table. Sync of *content* uses the encrypted Supabase relay with idempotency keys; sync of *state* does not exist. (`docs/spec/notifications.md` §19.8, ADR-0022)
- **Dismissed and expired rows are kept (soft-delete via `dismissed_at IS NOT NULL`), not physically deleted.** Preserves audit trail and avoids races between sync round-trips and user dismiss actions. The panel filters on `dismissed_at IS NULL`. (`docs/spec/notifications.md` §19.4)
- **No `sync_conflict` notification type at launch.** All sync conflicts auto-resolve via LWW + content-identity per §22B.3.5 and INVARIANT (Pane Gateway #14). Adding the type requires a clarifying supersession ADR triggered by an actual non-LWW case. (`docs/spec/notifications.md` §19.5.2)

## Smart Routing (§27)

Per-message dispatch decisions and the audit trail that records them. Cross-references its §27 subsections; `#issue` references lock the architectural decisions.

- **Per-message model badges are immutable at stream time.** The badge attached to an assistant message reflects the model that ACTUALLY ran for that specific message — set once at stream time from the value populated into `chat_messages_local.model` (§27.2), and never updated retroactively. Not on provider switch, not on model rename, not on app restart, not on any subsequent setting change. **Load-bearing for smart routing**: different messages in the same conversation legitimately ran on different models (Tier 1 / Tier 2 / Tier 3 per §27.1, or historical per-project resolution — retired by ADR-0061; badges written under it remain valid audit records), and the badge is the audit record of that decision. A future spec or implementation change that "fixes" stale badges by syncing them to the current provider/model would silently destroy the audit trail and break smart-routing introspection. Display invariants for *current-provider* state (Settings primary-model picker per `docs/spec/smart-routing.md` §27.6.1c; AI Health Context Budget row per `docs/spec/ai-integration.md` §27.8 inv #11) apply to current-state surfaces and have opposite refresh semantics — the distinction is deliberate. Full callout at `docs/spec/smart-routing.md` §27.3. (#1448)
- **Pane's stored OC model is a display cache, never authoritative for dispatch** *(cache location
  moves at ADR-0063's build: `ai_settings.model` retires; the `X-OC-Resolved-Model` write-back
  re-targets the answering OC agent's `ai_agents.model` row — same decoupling, per-agent form;
  ADR-0063 D9)*. OC chat dispatch always goes through the gateway, which always resolves the actual model from `agent_registry.get_agents()`. Pane's stored value drives Pane-side display surfaces (Settings primary-model row, AI Health Context Budget Model row) and is kept in sync with OC's truth via the reverse-channel header contract (`X-OC-Resolved-Model` on `/v1/chat/completions` responses per `docs/spec/sync.md` §22C.6.7; consumed on successful stream completion per `docs/spec/smart-routing.md` §27.6.1d). A future refactor that "optimizes" by reading `ai_settings.model` directly to determine which model handles an OC chat — bypassing the gateway's agent_registry resolution — re-introduces the #1453 drift bug class: the stored cache can lag OC's truth (e.g., when `openclaw.json` is edited outside Pane between chats), and treating the cache as authoritative gets the wrong answer without any signal that anything went wrong. The cache and dispatch are decoupled by design; §27.6.1d keeps the cache honest about that decoupling. (#1453)

## Data Integrity
- Filesystem is source of truth for agent identity files; DB is an index
- Newer timestamp always wins for flat-file sync (§ v11.28.65)
- Notes and tasks are the SAME entity (unified `notes` table)
- `is_system` notes are non-editable in the UI

## Default Project (#2142)
- **The default project ("My Notes") is created once during first-run onboarding and never auto-recreated.** The `initial_project_created` flag in `ai_settings` is the guard. Once set (on first creation or retroactively for existing users), `resolveWorkshopProjectId()` skips creation even if zero user projects exist.
- **User deletion of the default project is permanent.** The app functions normally with zero user projects.
- **Project/folder directory deletion uses `trash::delete()` (cross-platform).** A trash failure stops the delete — no `remove_dir_all()` fallback exists (#3028). Aligns with §2A.2/§2A.6a.

## Search Indexing
- **INVARIANT: Every note content write MUST update FTS5 index and trigger embedding computation.**
- No content write path may skip FTS5 indexing. This includes: user creation, AI tool creation, daily log creation, sync import, integrity check reconstitution, memory synthesis, and flat-file write-back.
- The startup FTS5 rebuild is a SAFETY NET, not the primary indexing mechanism.
- Bug #390: Integrity check reconstitution was missing FTS5 updates → imported notes unsearchable
- **Embeddings terminal state is the durable `ai_settings.embeddings_status`; `embeddings://status` events are transient and never the sole signal of readiness.** (§5 persistence contract, ADR-0047, #637)
- **Semantic search is opt-in; FTS5 is the always-on default; nothing downloads or indexes without explicit user enable.** (ADR-0048, supersedes #66)

## Agent Identity
- **The default-agent flag is singular and moves atomically.** At most one `ai_agents` row holds
  the flag at any time (enforced by the partial unique index). Every change is a demote-then-promote
  within a single transaction — never two independent UPDATEs. Phase 1 the flag is `is_main`
  (`idx_ai_agents_main`); at ADR-0063's build it renames to `is_default` (`idx_ai_agents_default`)
  with unchanged shape. Changing the flag never affects an existing session — bindings are
  immutable (below). (ADR-0045 D1/D2 preserved; ADR-0062 D6; ADR-0063 D3; §24.7A)
- **`workspace_path` is always the agent UUID.** Every `ai_agents` row's `workspace_path` equals
  its `id` (UUID) — never the display `name`, never the `slug` — for every agent regardless of
  `source` (`import`, `onboarding`, `ai_created`). Renaming an agent never alters `workspace_path`.
  (§24.2, §24.3, §24.10; enforced v0.18.3)
- **An agent is usable only under its own provider class** *(governs until ADR-0063's build, then
  superseded)*. Agents with `oc_agent_id IS NOT NULL` are selectable only while
  `provider = openclaw`; agents with `oc_agent_id IS NULL` only under an API provider. Outside the
  active class: rendered disabled, never hidden. Predicate is `oc_agent_id` nullity, never `source`.
  At ADR-0063's build the active class ceases to exist; the successor invariant is runtime
  permanence (below). (§24.7C, ADR-0059; superseded by ADR-0063 D1)
- **`is_main` always points at an agent usable under the active provider** *(governs until
  ADR-0063's build, then retired with the mode)*. Every provider switch re-targets it via the
  §24.7A transaction. Under ADR-0063 the successor rule is D3's: the default may be any active
  agent of any runtime, and a deactivated default re-targets in the deactivating transaction.
  (§24.5.5, §24.7C, ADR-0059; ADR-0063 D3)
- **Session agent binding is immutable.** `chat_sessions.agent_id` is written exactly once, at
  INSERT, on every creation path. No update path exists; a different agent means a new session.
  The sole sanctioned post-INSERT write is the migration NULL self-heal (`[SESSION] BINDING
  SELF-HEAL:` — logged, default-resolving, one-shot per row). Any future "move chat to another
  agent" is a deliberate supersession of this entry, never a drive-by. (ADR-0062 D1/D3, §30.1.3)
- **Assistant-message attribution equals the session binding.** `chat_messages_local.agent_id`
  is stamped from `chat_sessions.agent_id` via `thread_id` at save — never from the ambient
  `is_main`/`is_default` row. NULL-legacy semantics per ADR-0059 D5 stand: NULL means "written
  before attribution shipped," is never backfilled, and never means "the current agent."
  (ADR-0062 D4, §24.8; #2248 closed by elimination)
- **Agent resolution rule: session agent where a session is in scope; designated default
  otherwise.** The five formerly-ambient sites (attribution, daily-log creation, memory router,
  consolidator, workspace-path resolution) follow it; no new ambient default lookup may be added
  on a session-scoped path. Agents are permanent to their runtime at ADR-0063's build
  (`runtime_type` immutable at creation; `runtime_ref` namespaced by runtime — OC's `"main"` slug
  is sync-allowlist identity in OC's namespace, never Pane's default flag). (ADR-0062 D7,
  ADR-0063 D1/D5)

## Database Migrations
- **Migration-arm continuity is enforced by the smoke-harness upgrade proof (`tests/fixtures/upgrade-from-v75.db` + `scripts/smoke-launch.sh upgrade`).** If the upgrade fixture triggers `SCHEMA_MISMATCH` in CI, a required migration arm is missing — that is a blocking failure, not an expected condition. Do NOT add `§smoke-allowlist` annotations to suppress `[CRITICAL] Migration failed: SCHEMA_MISMATCH`; those annotations were removed in #2207 precisely because SCHEMA_MISMATCH is now a real failure signal. Note: ADR-0017's per-pair migration test (`test_schema_version_mismatch_triggers_error`) is a known false green as of #2254 and is NOT the enforcement pointer — the smoke harness is the sole enforcement mechanism until #2254 is resolved.
- **INVARIANT: `ai_settings` must NEVER be dropped, recreated, or cleared by any migration.**
- All migrations that touch table structure must backup ai_settings BEFORE and restore AFTER.
- CRR migration lives in the numbered migration system (migration 17) — NEVER in the startup path.
- **Migration completion is tracked by the `schema_version` key in `ai_settings` — NEVER inferred from table structure.** (The `schema_migrations` table of `file-system.md` §2B.4 was specced but never built; the `ai_settings` key is the live and only mechanism. The rule that completion is never inferred from whether a table or column exists is unchanged — see the §2B.5 v0.15.0 incident for why.)

## Build Process
- NEVER run `tauri build` directly, and NEVER without Matt's explicit "go ahead" — DMGs come only from `scripts/build.sh`, which wraps `tauri build` with signing, notarization, and stapling. A raw `tauri build` cannot produce a notarized, distributable artifact.
- NEVER restart the gateway without Matt's explicit "go ahead"
- ALWAYS run `scripts/pre-build.sh` before `scripts/build.sh`
- ALWAYS `git add -A && git commit && git push` BEFORE building DMG
- ALWAYS close issues with: Version | Spec section | Summary

## Settings / AI Tab
- **Settings connection status — primary and internal — requires a real validation/probe result.** The AI tab MUST NOT display a connected/success state for any provider (primary or internal) without a successful validation call — OR/Anthropic key validator, Ollama probe, or OC agent-model fetch. No blind success; no save on validation failure. Internal defaults to not-tested on load. (§28.1, §28.2)

## AI Provider Error Classification (§4.3)
- **Billing errors never enter retry/reconnect loops.** Anthropic (§4.3.1) and OpenRouter (§4.3.2) billing errors are hard-fails — backoff cannot resolve a credit or suspension state. `should_chat_retry` returns false for billing error codes.

## Agentic Loop
- **Tool-call parsing is native-first for every provider; the text-fallback runs only when native parsing returns zero calls (empty-result guard, single-fire) and is never gated on provider identity.** (§4.5.8, ADR-0046, #1586)
- Tool-call argument truncation is REQUEST-BUILD-ONLY and ONE-DIRECTIONAL: prior-iteration arguments exceeding 1024 bytes are replaced with a `{"_truncated": "<N> bytes omitted"}` JSON object placeholder in the outbound request copy only. Canonical history is never mutated. The placeholder must be a JSON object (valid on both sides of the §29 dual-provider boundary — OpenAI/OR string-encoded args and Anthropic native-object `input`). The transform runs ONCE pre-dispatch; it MUST NOT be duplicated per-serializer. (§4.5.7)

## Web Browsing Tools (§4.6)

Cross-references `docs/spec/ai-integration.md` §4.6 and ADR-0064. Every entry names a mechanism; none is a rule the model is asked to follow.

- **Web tools never dispatch to a private, loopback, link-local, CGNAT, multicast, or reserved address.** The check runs on every resolved address after DNS resolution, the connection is pinned to the checked addresses (`ClientBuilder::resolve_to_addrs`), and the full check runs again on every redirect hop. A hostname string check alone is never sufficient. (§4.6.5, ADR-0064 D7)
- **A tool call is executed only if its name is in the set of tools offered for that dispatch.** The gate sits in the agentic loop before `execute_single_tool`, regardless of whether the call was parsed natively or by the §4.5.8 text fallback. Web tools are offered only to sessions whose bound agent `runtime_type` is in the allowlist `{anthropic, openrouter, ollama, venice}`; OpenClaw-bound sessions never receive them. (§4.6.2, ADR-0064 D3, ADR-0065 D6)
- **Once external content enters a turn, every tool that mutates existing data or creates something that acts later is withheld and refused for the rest of that turn; only `create_note` and `create_sheet` stay available.** `MUTATE_EXISTING_TOOLS` (23: `edit_note`, `append_to_note`, `generate_image`, `delete_note`, `move_note`, `move_folder`, `create_project`, `create_folder`, `create_subtask`, `set_status`, `set_priority`, `star_project`, `set_sheet_cells`, `add_sheet_row`, `add_sheet_column`, `delete_sheet_column`, `add_sheet_rows`, `add_sheet_columns`, `schedule_job`, `spawn_job`, `cancel_job`, `pause_job`, `resume_job` — every `ToolAccess::Write` tool except the two create tools, plus `create_project`; pinned by `test_mutate_existing_tools_and_project_scoped_tools_differ_exactly_by_three`) are refused at the dispatch gate (`check_external_content_taint`) after the first outcome with `is_external_content == true` from `web_fetch`, `web_search` or `read_attachment`. There is no offer-time filter alongside this gate — the tool list sent to the provider is computed once per dispatch and never rebuilt per iteration, so the dispatch gate is the only mechanism, not a second belt-and-suspenders layer. Taint does not cross turns because `role: tool` messages are stripped from history, and no job can carry it forward because a tainted turn cannot create one. (§4.6.3, §36.5.2, ADR-0064 D4 + Amendment 2026-11-09)
- **Fetched and searched content is always wrapped in the untrusted-content delimiters by the tool executor itself, and never reaches the user-facing tool log.** No caller can return undelimited external content to the model; the "Tools used" line is the executor's `display` string (`web_fetch(<url>) → <Title>`), never `provider_result`. Pane-authored error strings are never delimited. (§4.6.1, ADR-0064 D4)
- **Memory extraction never receives tool results.** The extraction pair is built from `chat_messages_local.content` only; tool results live in `tool_results` and are structurally unreachable by `ai_extract_memory_with_db`. Pinned by test on `load_extraction_pair`. (§4.6.9, ADR-0064 D6)
- **The Brave Search API key is keychain-only.** Account `ai_brave_key` under ADR-0044; never written to `ai_settings` or `config.json`; no plaintext fallback (the key postdates the migration). (§4.6.8, ADR-0064 D5)
- **Venice never browses through Venice.** No `venice_parameters` field other than `include_venice_system_prompt` is ever sent; `enable_web_search`, `enable_web_scraping`, `enable_x_search` are never set. External content reaches a Venice model only through `web_fetch` / `web_search`, so the taint, SSRF, extraction-exclusion, and "Tools used" mechanisms above cover it. (§4.3.4, ADR-0065 D2)
- **The Venice API key is keychain-only.** Account `ai_venice_key` under ADR-0044; never written to `ai_settings` or `config.json`; no plaintext fallback in any migration regime (the key postdates the migration). (§4.3.4, ADR-0065 D4)
- **Venice is a variant, not a URL.** Any site on the OpenAI-shaped path that branches OpenRouter versus OpenClaw on `base_url.is_some()` is a defect once Venice exists; the branch is on `AiProvider` variant. Known sites: §4A.2.4 temperature policy, §30.7.2 session hint, `openrouter.rs:158` / `:489` (gateway client selection: SPKI pin to OC cert, or `danger_accept_invalid_certs(true)` when no cert is stored), `openrouter.rs:604-605` (model sentinel overwrite to `"openclaw"`). Full census: build-scope report §3. (§4.3.4, ADR-0065 D1)

## UI
- No emojis in chrome/UI — use Lucide SVG icons
- Card design is LOCKED: 130px fixed, 3px left border, CSS grid
- Dark theme: #0a0a0f bg, #00E4E4 teal accent

## Attachments

- **INV-ATT-HTML-SANDBOX.** HTML attachments render **only** in an empty-sandbox iframe (`<iframe sandbox="">`) inside `AttachmentHTMLPreview.svelte`. `allow-scripts` is never set on an attachment iframe; `allow-same-origin` is excluded by default (addable only after security review, never with `allow-scripts`). HTML attachment content is never injected into the note's contenteditable DOM and has no render path other than that modal. The app CSP is not loosened for attachment HTML. (ADR-0049 D1; `AttachmentNodeView` routes `html` type only to `AttachmentHTMLPreview`.) Empty sandbox blocks scripted requests but NOT passive subresource loads (static `<img>`/`<link>`/`<iframe>`/font). To deliver the "no network" guarantee, the rendered document additionally carries a restrictive CSP (`default-src 'none'`; only inert needs permitted) applied by Pane, and/or remote references are stripped. Empty sandbox alone is insufficient. (ADR-0049 Amendment B.)

- **INV-ATT-LINK-EGRESS.** Link-preview metadata is fetched only by `og_fetch.rs::fetch_link_preview`, which: allows `http`/`https` only; rejects requests whose **resolved** IP is loopback/private/link-local/unique-local or the cloud-metadata address `169.254.169.254`; re-validates the resolved IP on every redirect hop (capped); forwards no cookies/credentials; enforces a hard timeout and a byte cap on the fetched HTML head; and is gated by `link_previews_enabled` (default on, disclosed). Arbitrary remote `og:image` URLs are metadata only and MUST NOT be loaded by the WebView; link cards use a local icon unless a future guarded backend proxy/cache is approved. Failure → plain-address/domain fallback. (ADR-0049 D2, security amendment #2181.)

- **INV-ATT-REMOTE-FRAMES.** Remote link embeds are not part of the current launch surface. Production CSP keeps `frame-src 'self'` for the local empty-sandbox `srcdoc` HTML preview and permits no remote frame origins. Reintroducing a YouTube/Vimeo iframe requires an explicit superseding security decision and must not be done by widening CSP as a side effect. (ADR-0050 historical design; #2181 current enforcement.)

- **INV-ATT-REF-PORTABLE.** A note's attachment reference stored in the flat `.md` is a file-relative path to `Assets/<uuid>.<ext>` (never `attachment://…`, never a `tauri://`/`asset://` runtime URL). Display `src` is derived at render via `convertFileSrc`; the resolved URL is never written to disk. (file-system §2.1; owned by #1777.)

## Restore / Database Swap (ADR-0056)

- **No file-level copy or swap of the live DB may occur while a connection to that file is open.**
  Restore must close/invalidate the live `DbState` connection before any `fs::copy` or `fs::rename`
  targeting the live `.db` path, and a fresh connection MUST be opened after the swap.
  (ADR-0056; `backup.rs` restore path is the only current swap site; R1–R2 in `backup.md`.)

- **A restore MUST delete `.db-wal` and `.db-shm` for the swapped DB file unconditionally.**
  WAL presence after a file-level swap causes SQLite to replay pre-restore frames, silently
  corrupting the restored state. This applies to every restore regardless of backup quality.
  (ADR-0056 §Rationale; R1 in `backup.md`; #1922 root cause.)

## Backup / Debounce Flush (R5)
- **Every `create_backup()` call MUST flush pending file-watcher events before the WAL checkpoint.**
  Notes saved to `.md` files reach SQLite only after the watcher's debounce window expires.
  Without the flush, a backup triggered within the debounce window snapshots a DB missing the note.
  (R5 in `backup.md`; #1923 root cause.)

## Backup / UI Thread (R7)
- **Manual backup MUST NOT block the application event loop.**
  The Tauri command wrapper runs create_backup() on a spawn_blocking thread.
  The core create_backup() function remains synchronous — restore, migration,
  and daily callers are unaffected.
  (R7 in `backup.md`; #1921 root cause.)

## Sheets (§33, ADR-0066)
- **Computed values never sync — never persisted.** Formula strings and raw values sync; every device recomputes locally. No CRR column, no snapshot ever holds a computed value. (ADR-0066 D4, §33.5.4.)
- **`body` is derived state.** Written atomically in the same transaction as every grid mutation. Never parsed back into grid data. LWW skew self-heals via local recompute. (§33.4.)
- **One creation chokepoint.** `note_type = 'sheet'` set atomically at INSERT. No path creates a sheet with `note_type` unset. AI `create_sheet` calls the same chokepoint. (§33.2.1.)
- **Sheets are fileless.** `file_path = ''`; no `.md` on disk; never enters file watcher. The lib.rs integrity sweep allowlist includes `'sheet'` — repair-from-file is meaningless. (ADR-0066 D3.)
- **`note_type IS NULL` no longer means "all user content."** Any future content-kind addition must re-walk the canonical predicate map (§33.1.1). The widened predicate `(note_type IS NULL OR note_type = 'sheet')` is the new task-surface filter.
- **Orphan sweep is the integrity contract.** CRR = no FK; orphaned side-table rows are possible via concurrent sync. Sweep runs at launch and after each `apply_changesets` batch touching sheet tables, scoped to affected `note_id`s, before post-sync recompute. (§33.8.)
- **Sheets never route through `maybe_save_manual_version`.** Dedicated snapshot path (§33.9); the notes.rs:1370 gate stays byte-identical.
- **Deletion cascade uses bare hard-DELETE on CRR tables.** Mechanism-named exemption: cr-sqlite's `cl` changefeed provides delete tracking; no app-side ledger to protect. §1.9.4 subtasks precedent. (§1.12.2.)

## Runtime (§36)
- **Pane never runs an agent loop for an OpenClaw-bound session beyond one gateway round-trip per turn.** Auto-continue, spawn and scheduled execution are allowlisted to API runtimes; the allowlist is an allowlist, never an `openclaw` denylist. (§36.0.1)
- **A spawn session is never a surface.** `origin = 'spawn'` rows are never `is_active = 1` and never appear in `list_sessions`, whatever `include_hidden` says. Schedule sessions (`origin = 'schedule'`) are listed only in Home's Scheduled panel, never in a project drawer, and never auto-activated. (§36.3.6)
- **Report-back is an ingest, never a dispatch.** The only write path into an origin session from a finished job is `ingest_external_messages`, attributed to the session's own agent with `job_id` set; the binding check applies unchanged. "Assistant-message attribution equals the session binding" has no exception. (§36.8.1)
- **The scheduler is the only component that starts a turn nobody typed.** Enforced by a `syn`-based AST gate per ADR-0016, not by visibility: CI fails if `TurnOrigin::User` or `TurnOrigin::Job` is constructed anywhere except the interactive `ai_chat_stream` command and the scheduler tick. Today, `ai_chat_stream`, `insert_external_message`, `ingest_external_messages` and `execute_single_tool` are all top-level functions in one module (`ai::commands`); Rust privacy is per-module, not per-function, so no visibility modifier can make a constructor unnameable from a sibling call site in that file — the AST gate, not the compiler, is what holds this line. PR 1 (#3059) ships the AST gate alongside the loop extraction. It becomes compiler-enforced after the planned module split (tracked in a separate issue), once `ai::scheduler` exists as its own module and the constructors take `pub(in …)` scoping. (§36.4.5, §36.5)
- **Chains terminate.** `chain_depth ≥ RUNTIME_MAX_CHAIN_DEPTH` withholds `spawn_job`/`schedule_job`; the root ceiling is shared across the chain. (§36.4.5)
- **Proactive turns never receive `delete_note`, `move_note`, `move_folder`.** Withheld from the offered set and refused at the dispatch gate. (§36.5.1)
- **Scheduler claims before it runs.** `pending → running` is a single transaction keyed on the row; two ticks cannot both run one job. (§36.3.3)
- **Missed recurring runs collapse to one.** (§36.3.5)
- **Attachment text is external content.** Delimited and tainted exactly like `web_fetch`. (§36.9.3)
- **OC's `failed` status on its own downstream run is not evidence — scoped to a session Pane never dispatched into.** A Pane-dispatched OC job settles its own terminal status through `write_job_terminal_status`, same as every other runtime; there is no Pane-side `'sent'` status to protect. The remaining audience is §36.6 background pickup for OC's own spawns/cron, read back for display only. (§36.3.4, §36.6, §36.11 — reworded 2026-10-31, #3129)

## What This File Is
This file lists architectural invariants — things that must NEVER be done regardless of what a task description says. If a task conflicts with an invariant, STOP and ask before proceeding. These rules exist because past compaction cycles caused agents to undo previous architectural decisions.

# Spec Session — Attachments Storage + Rendering (Session Notes)

**Date:** 2026-06-29
**Author:** Matt Carano, Claude (spec session)
**Source-read:** live `main` — `migrations.rs` (SCHEMA_VERSION `66`, `CRR_TABLES[20]`, `create_attachment_tables`), `attachments.rs` (v10.47.0, 5 commands), `src/lib/extensions/Attachment.ts`, `file-system.md §2.1/§2A/§2B`, `core-data-model.md`. Plus Reed `#1775` fix scope and Reed `#401` rendering scope (2026-06-29).
**Gates:** `#1775` (storage fix), `#1777` (refs-on-nodes), `#401` (rendering).

---

## Deliverables in this folder

| File | Applies to | Form |
|---|---|---|
| `file-system-2.1-amendment.md` | `docs/spec/file-system.md` §2.1 | BEFORE/AFTER |
| `core-data-model-attachments-schema.md` | `docs/spec/core-data-model.md` | new section + migration contract |
| `adr-0049-attachment-html-sandbox-and-link-egress.md` | `docs/adr/0049-*.md` | new ADR |
| `INVARIANTS-attachment-entries.md` | `INVARIANTS.md` | append block |

Each file carries an "Implementation note for Brit" header with content-anchored `str_replace` targets (no line numbers).

---

## The five in-session decisions

### #1 — Reference format (the gate) — DECIDED

**Canonical stored ref = file-relative markdown path to the asset.** The TipTap node additionally carries `data-attachment-id` (already exists) as the durable in-app handle; display `src` is resolved at render time via Tauri's asset protocol (`convertFileSrc`), replacing the current base64 `resolveAttachmentUrls()` path. Render-mode + type ride as **node attributes** (see #2).

- Stored form (default image): `![<original_filename>](<rel>/Assets/<uuid>.<ext>)`, where `<rel>` is computed from the note's depth (`Assets/…` at project root, `../Assets/…` one folder deep, `../../Assets/…` nested).
- On-disk filename is `<uuid>.<ext>` (per `save_attachment`); the human name lives in the markdown alt text. Self-describing + portable.
- Why not `attachment://uuid` (current code): it is dead outside Pane and breaks §2.1's "browsable if Pane disappears" promise for the **live** `.md` file (not just exports). Why not a raw `tauri://`/`asset://` string in markdown: runtime/version/platform-specific, worst for portability.
- **Stability property (strengthens the choice):** relative refs survive **project rename** and **folder rename** (note + `Assets/` move together; relative offset preserved). They require rewrite only on (a) note moved to a different *depth*, or (b) note moved to a different *project* (which also requires copying the asset into the destination project's `Assets/`).

Amends `file-system.md §2.1` (currently states relative paths but is mislabeled `BUILT`; live code uses `attachment://uuid`). See `file-system-2.1-amendment.md`.

### #2 — Render-mode persistence — DECIDED: node attribute (not column, not settings)

Render mode is **per-embed** (the same file can appear twice with different modes; it describes how *this note* shows *this occurrence*), so it belongs on the ProseMirror node, not the per-file `attachments` row and not global settings. This matches Reed `#401` PR1 (`renderMode` as a persisted node attribute).

**Serialization rule (keeps #1 portable):**
- **Default mode** → plain portable markdown (`![alt](rel/Assets/uuid.ext)` for image; `[alt](rel/Assets/uuid.ext)` link for others).
- **Non-default mode** → raw-HTML block carrying `data-attachment-id`, `data-render-mode`, and the relative `src`/`href`. Still renders in external viewers; round-trips losslessly in-app.

**Defaults per type:** image = `full-inline`; **all other types = collapsed card**. (Link/PDF/Audio/Video/HTML default to card.)

`render_mode` is intentionally **NOT** a DB column and **NOT** a settings key (preempt anyone adding it).

### #3 — HTML sandbox stance — DECIDED → ADR-0049

HTML attachments render **only** in a sandboxed `<iframe sandbox="">` (empty sandbox: no scripts, no forms, no same-origin, no popups, no navigation). Never `allow-scripts`. `allow-same-origin` only if a concrete relative-CSS need is proven and audited — default excluded. Never inline-executed into the note's contenteditable DOM. Enforced at the single render path (`AttachmentHTMLPreview.svelte`); `AttachmentNodeView` routes `html` type only there.

### #4 — og:-unfurl network egress — DECIDED → ADR-0049

Server-side `fetch_link_preview` (Reed `#401` PR7) with: hard timeout → failure = "couldn't load" card (plain-address fallback); **SSRF guard** (block loopback/private/link-local/ULA + cloud-metadata `169.254.169.254`; http/https only; **re-validate the resolved IP and every redirect hop**; cap redirects; strip cookies/credentials); **byte cap** on the fetched HTML head. og:image is shown **by URL through the webview under CSP**, not server-downloaded in v1. Gated by a new `link_previews_enabled` setting (default on, disclosed). Requires confirming outbound-HTTP capability in `tauri.conf.json` (Reed PR7 flag). **Tracked as #1781.**

### #5 — Schema spec — DECIDED (with a blocker Reed missed)

Confirm `#1775`'s shape: `project_id` carries a **UUID** (frontend fixes at `Editor.svelte` / `ChatSidebar.svelte`; column stays `TEXT NOT NULL`), add denormalized `project_name TEXT NOT NULL DEFAULT ''`, add `attachments` to `CRR_TABLES` (→21), bump `SCHEMA_VERSION` `66`→`67`, add the v66→v67 migration (name→UUID `UPDATE` + one-time file move). See `core-data-model-attachments-schema.md` for the full migration contract.

---

## Push-backs

**P1 — Reed #1775 Q6 is overturned by Decision #1.** Reed asserts `attachment://uuid` "is UNCHANGED by this fix." Decision #1 changes it to a relative path. This adds a **ref-rewrite** step (rewrite stored `attachment://uuid` → `rel/Assets/uuid.ext`) and switches resolution from base64 (`resolveAttachmentUrls`) to `convertFileSrc`. Assign that work to **#1777** (refs-on-nodes is the natural owner) or fold into #1775 PR2 — Matt/Reed to sequence. #1775's storage-path/UUID fix itself is unaffected and can proceed.

**P2 — CRR-readiness blocker on the `attachments` table (Reed #1775 PR3 missed this).** The moment `attachments` is added to `CRR_TABLES`, the rule enforced by `test_crr_tables_not_null_columns_have_defaults` applies: **every NOT NULL column must have a DEFAULT.** Today `original_filename`, `mime_type`, `size_bytes`, and `project_id` are `NOT NULL` with **no DEFAULT** → the test fails and the build breaks. The PR that adds `attachments` to CRR **must** also add DEFAULTs (`''`, `''`, `0`, `''`) or make those columns nullable, in the same change. Flag on #1775 PR3.

**P3 — Chat/global attachments fall outside the per-project Assets model.** Attachments come from two sources: note embeds (project-scoped, governed by #1) and **chat drops** (referenced by `chat_messages_local.attachment_ids`, with `project_id` = a chat-thread UUID or `__global__`/`workspace` — never a project). #1775's name→UUID migration explicitly leaves these unmigrated ("sentinel or delete"). Decision #1 governs **note-embedded** attachments only; it does not define a home for thread/global attachments under `Projects/{name}/Assets/`. **Open product question:** where do `__global__`/thread attachments live and back up? (Candidate: a `Workspace/Assets/` or retained `.pane/attachments/__global__/`.) Out of scope for this session; must be tracked before #1775's UUID fix ships, or chat-attachment reads break. **Filed as #1782.**

**P4 — `render_mode` cannot live in CommonMark image syntax.** Persisting render-mode as a node attribute (Decision #2) means non-default modes must serialize to a raw-HTML block; `![](…)` has nowhere to hold `data-render-mode`. Confirmed acceptable (default mode stays plain markdown; only non-default uglifies to HTML, still portable). Noted so #1777/#401 implement the serializer accordingly.

---

## Backend-key / attribute table

| Name | Existing or new | Where read/written | Notes |
|---|---|---|---|
| `attachments.project_id` | existing | `attachments.rs` (all 5 cmds); `projects.rs::delete_project` | Will hold UUID post-#1775 (today holds name/thread-id) |
| `attachments.project_name` | **new** (#1775) | `attachments.rs` path construction (proposed) | `TEXT NOT NULL DEFAULT ''`; update on project rename alongside `notes.file_path` |
| `attachments.original_filename / mime_type / size_bytes` | existing | `attachments.rs` | **NOT NULL, no DEFAULT** — must gain DEFAULTs when CRR-enabled (P2) |
| `render_mode` | **new** | TipTap node attr (`Attachment.ts`, #401 PR1) | **not** a DB column, **not** a setting |
| `attachment_type` / `duration` / `page_count` | **new** | TipTap node attrs (#401 PR1) | dimensions (`width`/`height`) **dropped** per locked design |
| `link_previews_enabled` | **new** | `ai_settings` (key/value); read in `attachments.rs::fetch_link_preview` via `get_ai_setting` | default `'true'`; ADR-0049 |
| `schema_version` | existing | `ai_settings`; `migrations.rs::get/set_schema_version` | bumped to `67` |
| `SCHEMA_VERSION` const | existing `66`→`67` | `migrations.rs` | per ADR-0017 add v66→v67 arm + append to terminal arms |
| `CRR_TABLES` | existing `[20]`→`[21]` | `migrations.rs` | `+ "attachments"` (#1775 PR3) — gated by P2 |
| `SIDECAR_SUFFIXES` | existing | `backup.rs` | `+ "_assets.tar.gz"` (#1775 PR4) |

Confirmed: `project_name` does **not** already exist under another name. No `get_ai_setting`/`set_ai_setting` calls exist in `attachments.rs` today (the `link_previews_enabled` read is new wiring).

## Constraint-collision table (`attachments`, from live CREATE TABLE)

| Constraint | Live definition | name→UUID migration | path move | Verdict |
|---|---|---|---|---|
| `PRIMARY KEY (id)` | `id TEXT PRIMARY KEY`; server-generated UUIDv4 per upload | **untouched** — migration updates `project_id` only | **untouched** — move changes disk path, not `id` | No collision |
| UNIQUE indexes | **none exist** | n/a | n/a | n/a |

Both row-touching operations (name→UUID `UPDATE`, file move) leave the only key (`id`) untouched, so neither can violate a uniqueness constraint. The real risk on this table is **not** a constraint collision but the **CRR NOT-NULL-needs-DEFAULT** rule (P2). Adding to CRR auto-creates `attachments__crsql_clock` / `attachments__crsql_pks` keyed on `id` (stable, unique) — safe.

---

## Sequencing / what unblocks

- **#1775** storage-path + `project_id`→UUID fix: proceed (Decision #5). Add DEFAULTs in the CRR PR (P2). Leave thread/global rows per P3.
- **#1777** owns the ref-rewrite (`attachment://uuid` → relative path) + `convertFileSrc` resolution + render-mode node attribute + serializer (P1, P4).
- **#401** rendering build proceeds on the node-attribute model (#2); HTML/link PRs cite ADR-0049 (#3/#4).
- ADR-0049 + INVARIANTS land before #401 PR6 (HTML) and PR7 (Link).
- Spec PRs merge before code PRs; cite `spec-covered-by:` this session's outputs.

## Out of scope / to file (Matt-only)

- **#1781** — link-preview egress (ADR-0049 #4).
- **#1782** — Thread/global attachment storage home (P3).
- Dimensions extraction (`width`/`height`) — dropped per locked design; revisit post-launch.
- Binary attachment sync (#1775 PR5, future).

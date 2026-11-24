# Multi-Device Sync, Pane Bridge & Pane Gateway

**Last Updated:** 2026-05-07
**Source:** Extracted from pane-architecture-spec.md
**Sections:** §9, §22B (LEGACY), §22C

---

## 9. Multi-device Sync
> **Status:** BUILT - Supabase relay, zero-knowledge, CRDTs

### 9.1 Architecture: Local-First with Cloud Relay
> **Status:** BUILT - Supabase relay, zero-knowledge, CRDTs

```
┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│  Mac Desktop │     │  iPhone App  │     │  Mac #2      │
│  SQLite +    │     │  SQLite +    │     │  SQLite +    │
│  cr-sqlite   │     │  cr-sqlite   │     │  cr-sqlite   │
└──────┬───────┘     └──────┬───────┘     └──────┬───────┘
       │    E2E Encrypted changesets              │
       └────────────┬───────┴─────────────────────┘
                    │
          ┌─────────▼─────────┐
          │  Supabase Relay   │
          │  • Auth (JWT)     │
          │  • WebSocket hub  │
          │  • Blob storage   │
          │  • ZERO plaintext │
          └───────────────────┘
```

### 9.2 cr-sqlite - The Sync Engine
> **Status:** BUILT - Supabase relay, zero-knowledge, CRDTs

**cr-sqlite** (v0.16.0+, MIT license, `vlcn-io/cr-sqlite`) is a **run-time loadable SQLite extension** that adds CRDT support to existing tables. It is NOT a Rust crate - it's a compiled native library (`.dylib` on macOS) loaded via `load_extension()`.

**How it works:**
1. `SELECT crsql_as_crr('table_name')` - upgrades a table to a "conflict-free replicated relation"
2. `crsql_changes` virtual table - extract/apply changesets between databases
3. Each column of each row gets an independent version counter (Lamport timestamps)
4. Conflict resolution is **Last-Writer-Wins per column** (not per row) - two devices editing different fields of the same note both win

**Production readiness:** Used by Reflect.app, Expo, Turso. v0.16.0 brought 5x reduction in CRDT metadata. Mostly single-maintainer (tantaman) - mitigate by vendoring the binary.

**Performance impact:**
- Reads: No measurable impact
- Writes: ~2.5x slower due to CRDT bookkeeping triggers
- Database size: ~30-50% increase for CRR tables (shadow tables for metadata)
- For Pane's usage pattern (infrequent writes, mostly reads), this is acceptable

**Tauri integration:**
- Bundle pre-built binaries in `src-tauri/resources/` per platform
- Enable `load_extension` feature in rusqlite: `rusqlite = { version = "0.32", features = ["bundled", "load_extension"] }`
- Load on startup with `conn.load_extension()`, disable extension loading after
- **CRITICAL:** Call `crsql_finalize()` before closing connection or CRDT metadata corrupts

**Fallback if cr-sqlite stalls:** libSQL (Turso's SQLite fork) can load cr-sqlite and may absorb its features natively.

### 9.3 What Syncs
> **Status:** BUILT - Supabase relay, zero-knowledge, CRDTs

**SYNC via cr-sqlite (19 tables):**

| Table | Why |
|---|---|
| `projects` | Core content structure |
| `notes` | Core content - the most important table |
| `tags`, `note_tags`, `note_links` | Cross-device tag/link consistency |
| `ai_context_markers`, `ai_pinned_messages`, `ai_conversation_summaries` | AI chat context across devices |
| `ai_memory`, `ai_actions` | AI memory + audit log |
| `user_profiles` | User profile data |
| `user_dashboard_config`, `dashboard_modules`, `module_items` | Dashboard configuration |
| `daily_entries`, `habit_entries`, `reflection_entries`, `gratitude_entries` | Daily data |
| `sync_state` | Device registry |

> **Chat messages are per-device by design, not CRR-synced.** `chat_messages_local` is deliberately excluded from `CRR_TABLES` (`migrations.rs`, "Per-device table"). Chat history — including the per-message `agent_id` attribution added in agents.md §24.8 — stays on the device that produced it. This follows ADR-0022's per-device precedent and v1's single-machine posture (onboarding.md §6: multi-Pane installs are not supported in v1). A prior revision of this list included the chat-messages table as a CRR member; that entry was incorrect and is removed here (#2030).

**DON'T SYNC (6 tables):**

| Table | Why |
|---|---|
| `users`, `user_roles`, `role_model_permissions` | Server-authoritative (from Supabase Auth) |
| `ai_settings` | Per-device AI config |
| `ai_message_queue` | Per-device offline queue |
| `extensions` | Per-device extension config |
| `notes_fts` | FTS virtual table - rebuilt locally after sync |
| `sync_changelog` | Replaced by cr-sqlite's own change tracking |

### 9.4 Conflict Resolution
> **Status:** BUILT - Supabase relay, zero-knowledge, CRDTs

cr-sqlite uses **Last-Writer-Wins per column** with Lamport timestamps (causal clocks, NOT wall clocks - immune to clock skew):

| Scenario | Resolution | Notes |
|----------|-----------|-------|
| Same note title edited on 2 devices | LWW - last causal edit wins | Title edits are infrequent |
| Different fields of same note edited | **Both edits preserved** | cr-sqlite tracks at column granularity |
| Same note content edited on 2 devices | **Yjs CRDT** (character-level merge) | cr-sqlite can't merge text - see §9.7 |
| Note deleted on A, edited on B | Edit wins - resurrect with edits | cr-sqlite default behavior |
| Tag created with same name on 2 devices | Needs dedup pass after sync | UUID PKs avoid ID collision |
| New notes created on both devices | No conflict (different UUIDs) | Works perfectly |
| Sort order changed on 2 devices | LWW per row | May cause temporary inconsistency |

### 9.5 Supabase Relay
> **Status:** BUILT - Supabase relay, zero-knowledge, CRDTs

Supabase serves three purposes - **none involve storing user content:**

1. **Authentication** - Supabase Auth (email/password → JWT)
2. **WebSocket relay** - Supabase Realtime Broadcast for changeset exchange
3. **Blob storage** - Supabase Storage for encrypted file content (.md files, attachments)

**Why Broadcast, NOT Postgres Changes:**
- Postgres Changes would require storing user data in Supabase Postgres - violates zero-knowledge
- Broadcast is just a message relay - Supabase sees encrypted bytes, nothing else
- Broadcast is low-latency (WebSocket) and doesn't require database writes for online peers
- The `changeset_queue` table (server-side) is only for offline catch-up

**Minimal Supabase schema (server-side Postgres):**

```sql
-- Device registry
CREATE TABLE devices (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    device_name TEXT,
    platform TEXT, -- 'macos' | 'ios' | 'windows'
    site_id BYTEA NOT NULL, -- cr-sqlite site_id
    last_seen_at TIMESTAMPTZ DEFAULT now(),
    created_at TIMESTAMPTZ DEFAULT now(),
    UNIQUE(user_id, site_id)
);

-- Encrypted changeset mailbox (for offline delivery)
CREATE TABLE changeset_queue (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    from_device_id UUID NOT NULL REFERENCES devices(id),
    encrypted_payload BYTEA NOT NULL,
    db_version BIGINT NOT NULL,
    created_at TIMESTAMPTZ DEFAULT now()
);

-- RLS: users can only access their own data
ALTER TABLE devices ENABLE ROW LEVEL SECURITY;
ALTER TABLE changeset_queue ENABLE ROW LEVEL SECURITY;
CREATE POLICY "Users manage own devices" ON devices FOR ALL USING (user_id = auth.uid());
CREATE POLICY "Users read own queue" ON changeset_queue FOR SELECT USING (user_id = auth.uid());
CREATE POLICY "Users insert own changesets" ON changeset_queue FOR INSERT WITH CHECK (user_id = auth.uid());
CREATE POLICY "Users delete own processed" ON changeset_queue FOR DELETE USING (user_id = auth.uid());
```

**Sync flow:**

```
Device A makes local edits
    ↓
cr-sqlite tracks changes in crsql_changes
    ↓
Sync engine queries: SELECT * FROM crsql_changes WHERE db_version > last_sync_version
    ↓
Changesets serialized (MessagePack) → encrypted with user's sync key
    ↓
If Device B is online:
    → Broadcast via Supabase Realtime channel: sync:{user_id}
    → Device B receives, decrypts, applies: INSERT INTO crsql_changes VALUES (...)
If Device B is offline:
    → Store in changeset_queue table (encrypted)
    → When B comes online, pulls from queue, decrypts, applies
    → Deletes processed queue entries
```

### 9.6 E2E Encryption
> **Status:** BUILT - Supabase relay, zero-knowledge, CRDTs

**Cryptographic primitives:**

| Purpose | Algorithm | Rust Crate |
|---|---|---|
| Symmetric encryption | XChaCha20-Poly1305 | `chacha20poly1305` v0.10 |
| Key derivation | Argon2id | `argon2` v0.5 |
| Key exchange | X25519 | `x25519-dalek` v2.0 |
| Hashing | BLAKE3 | `blake3` v1.5 |
| Random bytes | OS CSPRNG | `rand` v0.8 |

**Key hierarchy:**

```
User Password (never leaves device)
    ↓ Argon2id(password, salt)
Master Key (256-bit)
    ↓ HKDF-BLAKE3(master_key, "sync-encryption")
Sync Encryption Key - encrypts changesets
    ↓ HKDF-BLAKE3(master_key, "file-encryption")
File Encryption Key - encrypts note content blobs
    ↓ HKDF-BLAKE3(master_key, "device-auth")
Device Auth Key - signs device authorization requests
```

**Key storage:**
- Master key: Derived from password, NEVER stored. Re-derived on unlock.
- Sync master key (`sync_key.bin`): OS keychain (account `sync_master_key`, service `com.threelane.pane`) per ADR-0044 Addendum A (#1498). Migration from legacy plaintext file is copy-then-clear with read-back confirmation, gated by DB flag (`migration_flags` table). Post-migration, `sync_key.bin` is never read or re-adopted.
- Encrypted master key blob: OS keychain via `tauri-plugin-stronghold`
- Sync/File keys: Derived on-demand, cached in memory only while app runs
- Recovery key: 24-word BIP39 mnemonic shown once during setup. User writes it down.

**Device authorization flow (new device):**
1. New device generates X25519 keypair, displays 8-digit pairing code
2. User enters code on existing device
3. Existing device encrypts master key with new device's public key, sends via Broadcast
4. New device decrypts, stores in local keychain
5. Device registered in Supabase `devices` table
6. Initial full sync (pull all changesets from queue + storage)

**Comparable implementations:** Standard Notes (password → encryption key, zero-knowledge server), 1Password (Secret Key + Master Password), Obsidian Sync (optional E2E).

### 9.7 Rich Text Sync (Yjs)
> **Status:** BUILT - Supabase relay, zero-knowledge, CRDTs

**cr-sqlite cannot merge rich text.** It operates at the SQL column level - if two devices edit `notes.content` (a single TEXT column), one edit is lost (LWW).

**Solution: Yjs for note content, cr-sqlite for everything else.**

```
Note sync = cr-sqlite (metadata: title, status, tags, dates) + Yjs (content: markdown/TipTap)
```

- Rust: `yrs` crate v0.21 (Yjs port)
- Frontend: `@tiptap/extension-collaboration` + `yjs` + `y-prosemirror`
- Yjs document state stored as BLOB on the note: `ALTER TABLE notes ADD COLUMN yjs_state BLOB`
- This column does NOT go through cr-sqlite CRR - Yjs handles its own merge
- Yjs updates synced through the same encrypted Broadcast channel

**Can be deferred** - Phase 7 in Reed's implementation plan. cr-sqlite LWW on content is acceptable for single-user multi-device until collaborative editing is needed.

> **Sheets (§33, ADR-0066):** Three CRR side tables — `sheet_columns`, `sheet_rows`, `sheet_cells` — sync via the standard cr-sqlite changefeed (§9.3). Computed formula values never sync; every device recomputes locally after `apply_changesets` (§33.5.4). Post-sync hook: orphan sweep then recompute, both scoped to affected sheets. Multi-device conflict UX deferred (§33.10).

### 9.8 Sync State Tracking
> **Status:** BUILT - Supabase relay, zero-knowledge, CRDTs

```sql
CREATE TABLE sync_state (
    id              TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
    device_id       TEXT NOT NULL,
    device_name     TEXT,
    platform        TEXT,                   -- 'macos' | 'ios' | 'windows' | 'web'
    last_sync_at    TEXT,
    sync_version    INTEGER NOT NULL DEFAULT 0,
    is_current      INTEGER NOT NULL DEFAULT 0,  -- 1 for this device
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);
```

Note: `sync_changelog` table is replaced by cr-sqlite's built-in `crsql_changes` virtual table.

### 9.9 Risks and Mitigations
> **Status:** BUILT - Supabase relay, zero-knowledge, CRDTs

| Risk | Impact | Mitigation |
|---|---|---|
| cr-sqlite project abandoned | Can't get updates/fixes | Vendor binary; it's standalone. Fork if needed. libSQL as fallback. |
| CRDT metadata corruption | Sync breaks | Keep pre-CRR backup; implement integrity checks on startup |
| Encryption key loss (password + recovery) | Data unrecoverable | Prominent recovery key display; optional iCloud Keychain storage |
| Supabase goes down | Sync pauses | Local-first = app keeps working. Queue catches up later. |
| Large changeset exceeds WebSocket | Sync fails for batch | Chunk large changesets; use Storage for oversized ones |
| CRR migration failure | Database corruption | Back up DB before migration; rollback on failure |
| macOS Gatekeeper quarantines .dylib | Extension won't load | Codesign the binary; strip quarantine; graceful fallback to local-only |
| Schema changes on CRR tables | CRDT metadata corrupts | MUST use `crsql_begin_alter`/`crsql_commit_alter` for ALTER TABLE |
| Bulk imports (hundreds of notes) | Slow due to 2.5x write overhead | Temporarily drop CRR, import, re-enable |

### 9.10 Implementation Phases (estimated 8-10 weeks)
> **Status:** BUILT - Supabase relay, zero-knowledge, CRDTs

| Phase | Goal | Duration |
|---|---|---|
| 1 | Local cr-sqlite integration - load extension, upgrade tables to CRRs | 1-2 weeks |
| 2 | Changeset extraction + serialization (MessagePack) | 1 week |
| 3 | E2E encryption - key derivation, encrypt/decrypt changesets | 1 week |
| 4 | Supabase Auth integration - signup/login, JWT, token refresh | 1 week |
| 5 | Network sync via Supabase Broadcast - real-time + offline queue | 2 weeks |
| 6 | File content sync - hash comparison, encrypted blob transfer | 1 week |
| 7 | Yjs for rich text (can be deferred) - TipTap collaboration | 1 week |
| 8 | Device authorization flow - pairing, key exchange | 3-5 days |

**Full implementation guide with code examples:** `~/Shared/overnight/2026-02-24/pane-sync-implementation-guide.md`

### 9.11 Cargo Dependencies for Sync
> **Status:** BUILT - Supabase relay, zero-knowledge, CRDTs

```toml
# Crypto
chacha20poly1305 = "0.10"
argon2 = "0.5"
x25519-dalek = { version = "2.0", features = ["static_secrets"] }
blake3 = "1.5"
# Serialization
rmp-serde = "1.3"       # MessagePack for compact changeset serialization
base64 = "0.22"
# Networking
tokio-tungstenite = { version = "0.24", features = ["native-tls"] }
futures-util = "0.3"
# Yjs (Phase 7)
yrs = "0.21"
# Ensure rusqlite has load_extension
rusqlite = { version = "0.32", features = ["bundled", "load_extension"] }
```

---


---

## 22B. Pane Bridge - Control Plane Between Pane and OpenClaw
> **Status:** LEGACY as of 2026-04-27 (spec session). Superseded by §22C (Pane Gateway) for split-machine Tier 3. Bridge plugin remains in production as dev infrastructure during gateway build per §22C.16 cutover criteria. Section retained for historical context only.

*Added 2026-03-23 (Matt/Brit). Pane Bridge starts as an onboarding tool for two-machine Tier 3 setups. It grows into the integration layer between Pane and OpenClaw across all tiers.*

### 22B.1 What It Is

A headless mode built into the Pane binary. Runs on the OpenClaw machine. No UI, no database. Lightweight HTTPS server that provides a deterministic API for Pane to communicate with OpenClaw infrastructure.

```
pane --bridge
```

Generates a self-signed TLS cert at startup, displays bridge URL + one-time auth code + cert fingerprint.

### 22B.2 v1 API (Onboarding) - #229

> **Status:** RETIRED as of 2026-05-07 (onboarding redesign #953 / PR #960). Superseded by gateway endpoints for the onboarding flow:
> - Bridge `GET /agents` (one-time onboarding agent fetch) → gateway initial sync `GET /v1/sync/initial` per §22C.6.2 + §22C.22.4
> - Bridge `POST /identity` (one-time identity-file upload) → gateway sync push `POST /v1/sync/push` per §22C.6.2
>
> No new onboarding narrative references this sub-section. Existing references in non-retired sections must be updated to gateway equivalents per §22C.16 cutover criteria. Section body retained for historical context until full bridge-plugin retirement; archive to `docs/archive/` after universal customer upgrade per §22C.16.

The initial build for move-in:

| Endpoint | Method | Purpose |
|----------|--------|---------|

| `/agents` | GET | Returns agent list: id, name, role, model. Parsed from `openclaw.json`. Uses config-level `name` (not `identity.name`). Pane stores results in `ai_agents` table (§24.5). |
| `/agents/{id}/identity` | GET | Returns whitelisted identity files for one agent (SOUL.md, MEMORY.md, etc.). Pane writes files to `.pane/system/{agent}/`. |
| `/configure` | POST | Runs `openclaw config set` commands on the local machine. Used for symlink setup. |

**Security:** HTTPS (self-signed cert, generated at startup) + one-time 8-digit auth code. Pane pins the gateway TLS certificate after initial pairing (stored as PEM in DB). The client uses SPKI pubkey pinning (SHA-256 of SubjectPublicKeyInfo DER) via a custom `PinnedCertVerifier` that skips hostname/SAN verification — the pin is the sole trust anchor. (Updated from `add_root_certificate` in PR #2016/#2021, 2026-07-12.)

**Cross-platform:** Scan paths are OS-aware. macOS: `~/.openclaw/`. Linux: `~/.openclaw/`. Windows: `%APPDATA%\openclaw\` or `~\.openclaw\`.

### 22B.3 v2 API (File Sync) - Post Move-In

> **Status:** RETIRED as of 2026-05-07 (onboarding redesign #953 / PR #960). Superseded by gateway sync endpoints:
> - Bridge `POST /bridge/push` → gateway `POST /v1/sync/push` per §22C.6.2
> - Bridge `GET /bridge/pull` + `DELETE /bridge/pull/confirm` → gateway `GET /v1/sync/pull` per §22C.6.2
> - Bridge `GET /bridge/sync-all` → gateway `GET /v1/sync/initial` per §22C.6.2
>
> The `sync_pending` queue mechanics, atomic write + checksum, and pull/confirm semantics carry forward to the gateway equivalents (per §22C.6.2 and the §22C carry-forward invariants in §22C.18). The pre-redesign event-driven push/pull narrative does not retire conceptually - only the bridge-plugin transport retires. Section body retained for historical context until full bridge-plugin retirement; archive to `docs/archive/` after universal customer upgrade per §22C.16.

> **Status (pre-retirement):** BUILT in v0.17.61 (Apr 3, 2026) - fixed push sync trigger (issue #323)

Bridge v2 replaces Tailscale symlink dependency with event-driven file sync. Pane is the source of truth (§5.3), OpenClaw is read-only for identity files. Bridge v2 keeps them synchronized.

#### 22B.3.1 The Problem (Mar 25-26 Incident)

Bridge v1 copies files from OpenClaw → Pane during onboarding. After import, OpenClaw's workspace symlink points to a **local directory on the OpenClaw machine** - a stale snapshot frozen at onboarding time.

**What happens next:**
1. Pane writes synthesis updates to `.pane/system/{agent}/MEMORY.md` on the **user's machine**
2. OpenClaw reads from its **local symlink** - the old snapshot directory
3. Files diverge immediately after onboarding
4. OpenClaw agents operate on outdated context
5. Synthesis on Pane overwrites files OpenClaw can't see
6. No sync mechanism exists to reconcile the two

**Result:** After onboarding, OpenClaw MUST read agent identity files that reflect Pane's current state. Stale reads are bugs.

#### 22B.3.2 Sync Direction

**PRIMARY: Pane → Gateway → OpenClaw filesystem (push)**

- **Pane** is source of truth (§0.3.3)
- **OpenClaw workspace** is REPLICA when Pane is open
- **Gateway plugin** writes to OpenClaw's workspace directories on behalf of Pane

**SECONDARY: OpenClaw → Pane (pull reconciliation)**

- When **Pane is closed**, OpenClaw agents CAN write locally (autonomous operation)
- Writes queue to `sync_pending` table on the gateway
- When Pane reconnects: `GET /bridge/pull` retrieves pending changes
- Pane reconciles (timestamp-based, newer wins, §0.3.4), pushes canonical state back

Two operating modes: **push-only** (Pane open) and **push-pull** (reconnect after Pane closed).

#### 22B.3.3 What Syncs

All files in `.pane/system/{agent}/` matching the identity whitelist (§6.5.4):

```
SOUL.md
MEMORY.md
IDENTITY.md
USER.md
TOOLS.md
AGENTS.md
HEARTBEAT.md
RULES.md
logs/              (entire subdirectory)
```

**Everything else is ignored.** Scripts, build artifacts, temp files - none of it syncs. Strict whitelist, not blocklist.

#### 22B.3.4 Sync Trigger (Event-Driven)

> **Status:** BUILT (v0.65.76, PR #1351) — bug fix restoring event-driven push.
> Trigger path had been silently dead since commit `7776bd4d` (May 4 cert-pinning refactor): `trigger_sync` wrapped its background-thread `rusqlite::Connection` in a `DbState(Mutex)`, locked it, then called `sync_push_blocking` → `build_gateway_client_blocking`, which tried to re-lock the same non-reentrant `std::sync::Mutex` on the same thread → hung forever. Fix: extract `gateway_tls_cert_pem` before locking; new `build_gateway_client_blocking_with_cert(cert_pem: Option<String>)` takes the cert by value and never touches `DbState`. No lock is held during the HTTP call. Behaviour now matches this section as originally specified.

Sync is **not polling-based**. Every file write to `.pane/system/` triggers a sync push:

1. Pane writes to `.pane/system/{agent}/MEMORY.md`
2. Write passes through the §2C write guard (`safe_write_system_file()`)
3. Write guard logs the write → emits a sync event
4. Sync handler pushes that specific file to the Bridge
5. Bridge writes to OpenClaw's workspace directory

**Implementation:**

```rust
async fn safe_write_system_file(
    path: &Path,
    new_content: &str,
    operation: WriteOperation
) -> Result<()> {
    // ... existing write guard logic ...

    // After successful write:
    if let Some(agent) = extract_agent_from_path(path) {
        if let Some(filename) = path.file_name() {
            bridge_client.push_file(agent, filename, new_content, checksum).await?;
        }
    }

    Ok(())
}
```

**Why event-driven:** Polling introduces delay (stale reads) and wastes resources. Event-driven sync is immediate and efficient.

#### 22B.3.5 Conflict Resolution (sync_pending Queue)
> **Status:** BUILT in v0.17.62 (Apr 3, 2026) - pull endpoint + sync_pending tracking (issue #324)

**Timestamp-based, automatic, no user dialogs.** All conflict resolution happens via the `sync_pending` queue on the gateway.

**Operating Modes:**

**Mode 1: Pane Open**
- Push only: Pane → `POST /bridge/push` → gateway writes to workspace
- No conflicts possible (one-directional)

**Mode 2: Pane Closed**
- OpenClaw agents write locally to workspace files
- Gateway plugin queues these writes to `sync_pending` table with timestamps
- Pending changes accumulate until Pane reconnects

**Mode 3: Reconnection**
1. Pane calls `GET /bridge/pull` → gateway returns `sync_pending` entries
2. For each pending change:
   - Compare timestamps: Pane's `file_modified_at` vs OC's `queued_at`
   - **Newer wins** - if OC timestamp > Pane timestamp, ingest the change
   - If Pane timestamp >= OC timestamp, discard (Pane's version is fresher)
3. Pane reconciles all ingested changes
4. Pane pushes canonical state back via `POST /bridge/push`
5. Gateway clears `sync_pending` for that agent

**Schema (`sync_pending` table on gateway):**

```sql
CREATE TABLE sync_pending (
    id              TEXT PRIMARY KEY DEFAULT (random_uuid()),
    agent           TEXT NOT NULL,
    filename        TEXT NOT NULL,
    content         TEXT NOT NULL,
    checksum        TEXT NOT NULL,  -- SHA-256
    queued_at       TEXT NOT NULL,  -- ISO 8601 timestamp
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(agent, filename)  -- one pending change per file, last write wins
);
```

**Why queue-based:** No `.conflict` files cluttering the workspace. All reconciliation is traceable, automatic, deterministic. The queue acts as a buffer between autonomous OpenClaw operation and Pane's return.

#### 22B.3.5A Pull Endpoint Specification

**Endpoint:** `GET /bridge/pull`

**Purpose:** Retrieve pending changes from OpenClaw that occurred while Pane was closed.

**Request:**
```
GET /bridge/pull?agent={agent_name}
Authorization: Bearer <token>
```

**Response:**
```json
{
  "pending_changes": [
    {
      "id": "uuid",
      "agent": "Brit Freed",
      "filename": "MEMORY.md",
      "content": "...full file content...",
      "checksum": "a3b2c1...",
      "queued_at": "2026-03-31T15:30:00Z"
    },
    {
      "id": "uuid",
      "agent": "Brit Freed",
      "filename": "logs/2026-03-31.md",
      "content": "...full file content...",
      "checksum": "f4e3d2...",
      "queued_at": "2026-03-31T16:45:00Z"
    }
  ]
}
```

**Behavior:**
- Returns all `sync_pending` entries for the requested agent
- Pane processes each entry:
  - Compares `queued_at` with local file `modified_at`
  - Ingests if OC timestamp is newer
  - Discards if Pane timestamp is newer or equal
- After processing, Pane sends a confirmation request to clear the queue:

```
DELETE /bridge/pull/confirm
Authorization: Bearer <token>
Content-Type: application/json

{
  "agent": "Brit Freed",
  "processed_ids": ["uuid1", "uuid2"]
}
```

- Gateway deletes confirmed entries from `sync_pending`
- Pane then pushes canonical state back via `POST /bridge/push`

**Error handling:**
- If pull fails (network), Pane retries with exponential backoff
- Unconfirmed entries remain in queue for next reconnect
- Queue has a size limit (e.g., 100 entries) - if exceeded, oldest entries are dropped with a warning logged

**Why two-phase (pull + confirm):** Ensures Pane successfully processed the changes before clearing the queue. If Pane crashes mid-reconciliation, the queue persists for retry.

#### 22B.3.6 Same-Machine vs Split-Machine

**Same-machine setup (Pane + OpenClaw on one box) — REMOVED per #1108:**
- ~~Pane’s `.pane/system/` is the source~~
- ~~OpenClaw workspace symlinks to it: `~/.openclaw/workspace/{agent}/ → ~/Pane/.pane/system/{agent}/`~~
- ~~No HTTP push/pull needed — symlink keeps both in sync instantly~~
- ~~Gateway plugin is NOT needed for same-machine setups~~

**Split-machine setup (Pane on MacBook, OpenClaw on Mac Studio):**
- No shared filesystem, no symlinks
- HTTP push/pull via gateway plugin:
  - Pane saves → `POST /bridge/push` → gateway writes to local workspace
  - Pane reconnects → `GET /bridge/pull` → retrieves pending changes
- Gateway plugin (`pane-bridge`) manages OpenClaw workspace as REPLICA
- All sync is over HTTP/HTTPS - no Tailscale file sharing required

**Why split is better for production:** No filesystem mount dependencies. No cross-machine symlinks. Pure HTTP API sync is simpler, more reliable, and works across any network (LAN, internet, VPN).

#### 22B.3.7 Offline Handling

If Pane can't reach the bridge (network down, OpenClaw machine off), sync operations are queued locally:

1. Pane maintains a local sync queue in SQLite: `sync_queue` table
2. Each failed sync push is enqueued with: agent, filename, content, checksum, timestamp
3. Pane periodically retries (exponential backoff: 1s, 2s, 4s, 8s, 60s, 5min)

> Status: BUILT — periodic flush thread opens a fresh `database::open_connection(None)` connection per iteration (PR #1363, fix/1180). Shared `db.0` mutex is never held during blocking HTTP. See `lib.rs:967` and audit `docs/audits/2026-06-02-freeze-cluster-1347-1180.md`.
4. When connection restores, queue is flushed in order
5. Checksum comparison prevents duplicate writes

**No data loss - just delay.** User can work offline on Pane; changes sync when OpenClaw machine comes back online.

**Sync Queue Retry Invariants:**

- `next_retry_at` stores RFC3339 timestamps via `chrono::Utc::now().to_rfc3339()` (e.g., `2026-04-22T18:08:00+00:00`)
- All queries filtering by `next_retry_at` MUST use bound RFC3339 parameters (`?1` with `chrono::Utc::now().to_rfc3339()`), NOT SQLite's `datetime('now')` - the formats are incompatible for lexicographic comparison
- Flush functions (`flush_sync_queue`, `flush_sync_queue_blocking`) skip items with future `next_retry_at` - only items where `next_retry_at IS NULL OR next_retry_at <= now` are eligible
- Maximum 5 retries per item before drop (exponential backoff: 2^n seconds, max 32s at retry 5)
- Dropped items emit `[SYNC] Dropping {agent}/{filename} after {n} retries` log and are deleted from queue
- `retry_sync_queue()` handles items whose backoff has expired - same RFC3339 bound parameter pattern

**Why this matters:** SQLite's `datetime('now')` returns `YYYY-MM-DD HH:MM:SS` format. RFC3339 uses `YYYY-MM-DDTHH:MM:SS+00:00`. Lexicographic comparison between these formats fails silently - `T` > ` ` means RFC3339 timestamps always appear "greater than" SQLite timestamps, causing items to never match the WHERE clause. This was caught in review (2026-04-22, PR #645).

**Implementation:**

```sql
CREATE TABLE sync_queue (
    id              TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
    agent           TEXT NOT NULL,
    filename        TEXT NOT NULL,
    content         TEXT NOT NULL,
    checksum        TEXT NOT NULL,  -- SHA-256
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    retry_count     INTEGER DEFAULT 0,
    next_retry_at   TEXT,           -- RFC3339 format, NOT SQLite datetime
    UNIQUE(agent, filename, checksum)  -- prevent duplicates
);
```

#### 22B.3.8 Bridge Persistent Mode

Bridge v1 is one-time (`pane --bridge` for onboarding). Bridge v2 runs **persistently** on the OpenClaw machine:

**Deployment options:**
1. **Integrated into OpenClaw gateway** - Bridge endpoints added to the gateway HTTP server (preferred)
2. **Standalone daemon** - `pane --bridge --daemon` runs in background, restarts on boot
3. **Cron/systemd** - Periodic checks + long-lived HTTPS server

**Preferred:** Integrate into OpenClaw gateway. Reuses existing auth, TLS, and lifecycle management.

**Startup:**
```bash
# On OpenClaw machine:
openclaw gateway start --with-bridge
```

Gateway listens for sync push requests from Pane. No polling, no scheduled tasks - purely reactive.

#### 22B.3.9 Authentication

**Same mechanism as Bridge v1:**
- One-time pairing code (8 digits, expires after 5 minutes)
- Token-based auth for ongoing sync
- Token persists in Pane's config after initial pairing

**Pairing flow:**
1. User starts bridge on OpenClaw machine: `openclaw gateway start --with-bridge`
2. Bridge displays: URL + one-time code + TLS cert fingerprint
3. User enters code in Pane's connection screen
4. Pane exchanges code for long-lived token
5. Token stored in Pane's config, used for all subsequent sync requests

**Token rotation:** Tokens expire after 90 days. Pane auto-renews before expiration. If renewal fails, user re-pairs.

#### 22B.3.10 API Endpoints (Gateway Plugin)

**Note:** The standalone `bridge.rs` process is DEPRECATED. All bridge functionality is now provided by the `pane-bridge` gateway plugin. Agent discovery is handled by the existing `GET /v1/agents` endpoint on the OpenClaw gateway (no separate bridge needed for discovery).

| Endpoint | Method | Purpose |
|----------|--------|---------|
| `/bridge/push` | POST | Pane pushes a file. Body: `{ agent, filename, content, checksum }`. Plugin writes to OC workspace. Returns: `{ success }`. |
| `/bridge/pull` | GET | Pane retrieves pending changes. Query: `?agent={name}`. Returns: `{ pending_changes: [{id, agent, filename, content, checksum, queued_at}] }`. See §22B.3.5A. |
| `/bridge/pull/confirm` | DELETE | Pane confirms processed changes. Body: `{ agent, processed_ids }`. Plugin deletes entries from `sync_pending`. |
| `/bridge/status` | GET | Returns last sync per agent per file. Response: `{ agent, filename, last_sync_at, checksum, queue_depth }`. |
| `/bridge/health` | GET | Plugin health check. Returns: `{ status, uptime, queue_depth, last_sync_at, connection_status }`. |

**Discovery handled by:** `GET /v1/agents` (standard OpenClaw gateway endpoint, already built) - returns all agents with id, name, role, model. Pane stores results in `ai_agents` table (§24.5).

**Example `/bridge/push` request:**

```json
POST /bridge/push
Authorization: Bearer <token>
Content-Type: application/json

{
  "agent": "Brit Freed",
  "filename": "MEMORY.md",
  "content": "# MEMORY.md\n\n## Identity\n...",
  "checksum": "a3b2c1..."
}
```

**Example `/bridge/push` response:**

```json
{
  "success": true,
  "written_at": "2026-03-31T14:32:15Z",
  "checksum": "a3b2c1..."
}
```

**Example `/bridge/status` response:**

```json
{
  "syncs": [
    {
      "agent": "Brit Freed",
      "filename": "MEMORY.md",
      "last_sync_at": "2026-03-31T14:32:15Z",
      "checksum": "a3b2c1...",
      "queue_depth": 0
    },
    {
      "agent": "Stia",
      "filename": "SOUL.md",
      "last_sync_at": "2026-03-31T12:15:00Z",
      "checksum": "f4e3d2...",
      "queue_depth": 2
    }
  ],
  "total_queue_depth": 2
}
```

#### 22B.3.11 Implementation Phases

**Phase 1:** Gateway plugin server
- `/bridge/push`, `/bridge/pull`, `/bridge/pull/confirm`, `/bridge/status`, `/bridge/health` endpoints
- `sync_pending` queue table
- Timestamp-based conflict resolution
- Write to OpenClaw workspace directories

**Phase 2:** Pane sync client
- Event-driven push triggers (§2.3A write guard integration)
- Pull reconciliation on reconnect
- Offline queue + retry logic (Pane-side)

**Phase 3:** Deployment variants
- Same-machine: symlink only, no HTTP
- Split-machine: HTTP push/pull, no filesystem sharing
- Documentation for both setups

**Why:** Pure HTTP API sync is simpler and more reliable than filesystem-based approaches. No cross-machine symlinks. No Tailscale file sharing dependencies. Works over any network.

### 22B.4 v3 API (Task Management) - With §17 Agent Spawning

When Pane starts spawning agent tasks (#224):

| Endpoint | Method | Purpose |
|----------|--------|---------|
| `/tasks` | POST | Pane sends a task to the bridge. Bridge creates `.pane/tasks/{id}/workspace/`, spawns the agent via OpenClaw. |
| `/tasks/{id}` | GET | Returns task status, progress, token usage. |
| `/tasks/{id}/result` | GET | Returns deliverable (note content, file, etc.). |
| `/tasks` | GET | Lists all tasks (active + completed). |

**Why:** Clean API for task lifecycle instead of chat-based spawning. The bridge manages workspace creation, agent spawning, and result delivery.

### 22B.5 v4 API (Health & Monitoring) - With Settings Agents Tab

Live agent monitoring for the Settings → Agents tab (#197):

| Endpoint | Method | Purpose |
|----------|--------|---------|
| `/status` | GET | Gateway health, disk space, OpenClaw version, uptime. |
| `/agents/{id}/status` | GET | Agent status: last active, current task, model, token usage. |
| `/logs` | GET | Recent gateway/agent logs (paginated). |

**Why:** Pane's Agents tab can show live status badges, health indicators, and diagnostics without any filesystem access.

### 22B.6 Multi-Machine Fleet (Future)

If a user runs OpenClaw on multiple machines (home server + VPS + work laptop):
- Each machine runs `pane --bridge`
- Pane discovers and manages all bridges from one interface
- Tasks can be routed to specific machines based on capability (GPU, model availability)
- Settings → Agents tab shows all machines and their agents

This is the beginning of an orchestration layer.

### 22B.7 Tier 1 (Pane Cloud) Deployment

When we host OpenClaw for Pane Cloud customers:
- Pane Bridge runs as a server-side service alongside hosted OpenClaw instances
- Same API, deployed as a persistent service instead of user-launched CLI
- Pane Cloud frontend talks to the bridge exactly like the desktop app
- No code changes - same endpoints, different deployment

### 22B.8A Bridge Authentication

> **Status:** RETIRED as of 2026-05-07 (onboarding redesign #953 / PR #960). Superseded by gateway bearer-token authentication (§22C.5.1) plus TLS fingerprint pinning (§22C.4.3). The redesign's onboarding pairing flow (`docs/spec/onboarding.md` §6.5.3 + §22C.22.4) replaces the bridge's one-time auth-code model with the gateway's:
> - 8-digit pairing code with 5-min TTL, one-time use (§22C.5.1, external-contract-captures §3)
> - 256-bit opaque bearer token with 90-day rotation (§22C.5.1)
> - Per-tenant scope (`tenant_id`, single tenant at launch per ADR-0013)
> - TLS fingerprint accept-on-pair, hard-fail on rotation (§22C.4.3)
>
> No new authentication narrative references this sub-section. The header/query-param validation pattern, short-lived auth-code semantics, and self-hosted trust model do not carry forward - the gateway's bearer-plus-fingerprint model is structurally different and intentionally so (per ADR-0013). Section body retained for historical context until full bridge-plugin retirement; archive to `docs/archive/` after universal customer upgrade per §22C.16.

> **Status (pre-retirement):** NOT BUILT
> **Created:** 2026-03-28

**One auth model, all endpoints.** The bridge uses a single `validate_auth()` function for every endpoint. No per-endpoint auth logic duplication.

**Auth sources (checked in order):**
1. `Authorization: Bearer {code}` header - standard for API calls (sync push, programmatic access)
2. `?code={code}` query parameter - convenience for browser testing and curl

If both are present, the header takes precedence. If neither is present or the code is wrong, return `401 Unauthorized`.

**Why both?** Bearer headers are the industry standard for API auth and required for Pane Cloud (§22B.7). Query params are convenient for browser health checks and manual curl testing during development. Supporting both costs nothing and avoids forcing a choice between developer ergonomics and production correctness.

**Implementation:** A single `validate_auth()` function extracts the code from header or query param, compares against `BridgeState.auth_code`, and returns `Result<(), StatusCode>`. Every endpoint calls it. Zero copy-paste.

**Future (Pane Cloud):** The auth code becomes a long-lived API token stored in the user's account. Same `validate_auth()` function, same two sources, different token lifecycle.

### 22B.8 Design Principles

1. **Deterministic operations only.** No AI interpretation layer. Every endpoint is a direct operation - read a file, copy a file, run a command, return status.
2. **Pane owns identity, bridge owns execution.** Pane stores Soul/Memory/notes. The bridge provides access to the OpenClaw machine's resources.
3. **Short-lived for self-hosted.** User runs `pane --bridge` when needed, shuts it down. Not a permanent daemon (for v1).
4. **Same code, all tiers.** Desktop users run it as CLI. Pane Cloud runs it as a service. API is identical.
5. **Progressive build.** Each version adds endpoints. Never breaks backward compatibility.

---


---

# §22C Pane Gateway

> **Status:** PROPOSED - Spec session 2026-04-27. Supersedes §22B (Pane Bridge) for split-machine Tier 3. Same-machine Tier 3 is unchanged. §22B.4 (task management) is NOT included; deferred to a follow-up spec.

> **Section structure note:** This section folds in everything previously scoped into §22B.3 (file sync), §22B.5 (health), and the bridge plugin's `/bridge/models` route. §22B is marked LEGACY upon merge of this section. The bridge plugin remains in production as dev infrastructure during gateway build (see §22C.16 cutover criteria). All forward-applicable bridge constraints port into §22C.

---

## §22C.1 What the gateway is

The Pane Gateway is a Pane-owned service that sits between the Pane client and a self-hosted OpenClaw instance for split-machine Tier 3 deployments. It exposes a clean HTTP/2 + SSE surface to the Pane client and absorbs OpenClaw's WebSocket-RPC protocol behind it. The gateway is the sole sync transport for split-machine Tier 3 at launch and the forward-compatibility seam for Tier 1 (hosted) post-launch.

Three problems the gateway solves:

1. **Protocol clean-up.** OpenClaw is WS-only at the core (147 methods). The current `pane-bridge` plugin grafts six HTTP routes onto OC for sync, model discovery, and health - none of which are core OC features. The Pane client is shaped around HTTP and shouldn't grow a WS client.
2. **Forward compatibility.** Tier 1 (hosted) cannot run a per-user OC plugin. A Pane-owned service that brokers between Pane clients and OC instances generalizes to multi-tenant hosting via a config flip, not a refactor.
3. **Hardening surface.** The bridge audit (#689, 2026-04-27) found 11 gaps including 0% test coverage, plaintext HTTP, and undocumented architectural decisions. The gateway is the architectural answer - TLS terminated, tested from day one, multi-tenancy seam designed in, every decision recorded in an ADR.

Everything Pane currently does through `pane-bridge` plus the ten OC WS methods needed for chat and sessions is mediated by the gateway. **OpenClaw is split-machine only (#2649)** — there is no same-machine topology and no symlink bypass path; every OC integration goes through the gateway.

## §22C.2 What the gateway is NOT (non-goals at launch)

Locked non-goals to prevent scope creep during implementation:

- **Not a task orchestrator.** §22B.4 v3 (task management) is deferred. No `/tasks` endpoints, no agent spawning, no workspace allocation.
- **Not a real-time push channel from gateway to Pane outside session SSE.** The gateway does not push events to Pane on its own initiative. Pane subscribes via SSE for streaming session messages; that is the only push-style surface. No general-purpose server-sent events for sync, agent status, or activity feeds at launch.
- **Not an LLM proxy in any meaningful sense.** Tier 2 (BYOK) never touches the gateway. Tier 3 chat goes through the gateway because OC is WS-only, but the gateway does not aggregate, route, or rate-limit LLM calls. OC is the LLM router; the gateway is the wire.
- **Not an authoritative source of agent identity files.** Pane is the source of truth (§0.3.3, §22B.3.2 carries forward). The gateway is the transport.
- **Not applicable to same-machine setups — there are none.** OpenClaw is split-machine only (#2649). The gateway is the sole OC integration path; no symlink fallback exists.
- **Not where multi-tenancy enforcement implementation lives at launch.** The seam is present (every operation tenant-scoped, all gateway state has `tenant_id`); the implementation is single-tenant config at launch. Tier 1 is a config flip, not a refactor.
- **Not where hosting concerns live.** Billing, admin UI, user management, HA deployment, observability stack - all post-launch.
- **Not the canonical home for agent creation/update/deletion.** `agents.create/update/delete` are not exposed. Read-only `agents.list` only.
- **Not where agent auto-registration in `openclaw.json` lives.** Per Reed's same-machine inventory (2026-04-27), no code writes to `openclaw.json` on agent creation today - neither Pane, nor the bridge plugin, nor OC core. This is a pre-existing gap, not a gateway concern. The gateway does not introduce auto-registration. If addressed later, it belongs to a separate issue scoped to agent lifecycle, not to this spec.

Anything not in §22C.6 (Pane-facing API) or §22C.7 (gateway↔OC WS surface) is out of scope until a follow-up spec adds it.

## §22C.3 Architecture

```
┌──────────────────────────┐         ┌──────────────────────────┐
│   Pane (Tauri client)    │         │    Pane Gateway          │
│                          │ HTTP/2  │    [same host as OC]     │
│   sync_client.rs         ├────────►│                          │
│   (refactored: HTTP      │ + SSE   │   gateway HTTP/2 server  │
│    layer points at       │◄────────┤   gateway WS client      │
│    gateway, not bridge)  │         │   tenant_id-scoped       │
│                          │         │   queue, sessions, agents│
└──────────────────────────┘         └────────────┬─────────────┘
                                                  │ WS (loopback)
                                                  ▼
                                     ┌──────────────────────────┐
                                     │   OpenClaw gateway       │
                                     │   (WS-RPC core, 147      │
                                     │    methods; 11 used      │
                                     │    by gateway at launch) │
                                     └──────────────────────────┘
```

Trust boundaries:

- Pane client trusts gateway via TLS fingerprint pinning + bearer token.
- Gateway trusts OC via OC's pairing token. Trust is scoped per-tenant.
- OC has no notion of Pane clients. From OC's perspective, the gateway is the user.

**Deployment topology.** In split-machine setups, the gateway is co-located with OpenClaw on the OC-side machine. This is a load-bearing assumption, not a recommendation:

- The gateway↔OC WebSocket connection runs over loopback. Not network-encrypted, not network-failure-prone - it's an in-host socket.
- Sync writes from the gateway to agent identity files in OC's workspace are filesystem-local on the OC machine, not wire-transmitted. This preserves the bridge plugin's same-host write profile and avoids round-tripping every sync write through OC's WS-RPC surface (which has no native sync-write method anyway - see ADR-0015).
- The only network hop in a split-machine deployment is Pane↔gateway. Everything between gateway and OC is in-host.

The gateway is therefore not a candidate for hosting separately from OC in any deployment shape this spec covers. If a future deployment shape ever separates them, sync writes would need to move back through some OC-side mechanism (WS-RPC method addition, or a slim file-write helper), which is out of scope. INVARIANT addition not needed - this is a deployment-shape constraint, not a runtime invariant.

**OpenClaw is split-machine only (#2649).** There is no same-machine topology, no filesystem-symlink diagram to collapse to, and no topology choice at startup — the Pane client routes exclusively through the gateway path described above. There is no "gateway-optional" runtime fallback.

## §22C.4 Transport

### §22C.4.1 Pane ↔ gateway

**Wire:** HTTP/2 over TLS for all request/response operations. Server-Sent Events (`text/event-stream`) over HTTP/2 for streaming session messages.

**No WebSocket endpoint exposed to Pane.** SSE is the streaming primitive. Pane does not grow a WS client.

**Connection lifecycle:**

- Pane creates one HTTP/2 connection to the gateway and reuses it via multiplexing for all request/response calls. Idle timeout 5 minutes; gateway issues `GOAWAY` cleanly on shutdown.
- SSE streams are one per active session, multiplexed within the HTTP/2 connection.
- On TCP-level disconnect: Pane retries connection establishment with exponential backoff (1s → 2s → 4s → 8s → 30s capped). No app-layer heartbeat - HTTP/2 PING frames + TCP keepalive cover liveness.
- On TLS failure (cert mismatch, expired, untrusted): hard fail with a specific error to the user. Gateway URL must be reconfigured. No silent re-trust.

**SSE reconnect (Pane-side):**
SSE follows standard `Last-Event-ID` semantics. Pane records the last sequence number processed per session. On reconnect, Pane sends `Last-Event-ID: <seq>` and the gateway replays from its session-message buffer (see §22C.7.3).

### §22C.4.1.1 Streaming-proxy idle deadline

**Scope.** This sub-section governs the lifetime of an individual SSE stream proxying a single chat completion from gateway to Pane. It is distinct from §22C.4.1's persistent HTTP/2 connection lifecycle: the HTTP/2 connection's liveness continues to be handled by HTTP/2 PING + TCP keepalive ("no app-layer heartbeat" remains accurate at the connection layer). The contract below applies to per-completion stream timeouts within that connection, not to connection-level liveness.

**Contract.** The gateway extends an SSE stream's idle deadline whenever the gateway↔OC WebSocket (§22C.4.2) shows activity within the idle window. The timeout fires only when both channels — the Pane-facing SSE byte stream AND the gateway↔OC WS — have been silent for `streaming_idle_timeout_secs`. Per-channel silence in isolation does not trigger termination.

The §22C.4.2 thirty-second WS ping cycle is the correlation signal: a healthy gateway↔OC connection emits ping/pong traffic every 30s regardless of session activity, which keeps the deadline extended through any OC-side silence shorter than `streaming_idle_timeout_secs`. Agent events delivered over the same WS connection during tool execution also count as activity. The contract is "WS shows activity," not "WS shows ping specifically."

The timeout still catches genuinely dead connections. When OC crashes or the gateway↔OC WS is severed, WS ping/pong stops alongside chat events, both channels go silent, and the deadline fires as designed. A crashed OC is not silent on one channel and chatty on another; the dual-channel rule does not weaken the kill-on-dead-connection guarantee.

**Why the contract exists.** OC's HTTP SSE stream can be silent for tens of seconds during tool execution — tool calls execute synchronously between message boundaries on OC's side, so no SSE bytes flow until the tool completes. A single-channel idle timer kills healthy connections mid-tool-run on any tool whose execution exceeds `streaming_idle_timeout_secs`, regardless of whether OC is alive (it is) and regardless of whether OC is producing useful work (it is). The dual-channel correlation rule resolves the bug surfaced in #1407 (Reed diagnostic `2026-06-20-1407-stream-drop-diagnostic.md`) and addressed in #1557.

**End state.** With this contract enforced, `streaming_idle_timeout_secs` returns to its 60-second default. The 300-second launchd plist backstop introduced as a workaround for #1407 is removed; the dual-channel rule is sufficient on its own.

**Implementation latitude.** The contract is WHAT the gateway guarantees, not WHICH internal mechanism it uses. The gateway is free to implement the rule via select-loop awakening on either channel, via separate timers per channel, or via any other approach that satisfies the dual-channel silence requirement.

**Status:** BUILT-pending (#1557)

### §22C.4.2 Gateway ↔ OC

**Wire:** WebSocket over TLS to the OC gateway endpoint, holding the OC bearer token in the WS handshake (`Authorization: Bearer ...`). One WS connection per tenant.

**Connection lifecycle:**

- Persistent. Gateway opens connection on first request that requires it (lazy) and keeps it open.
- Heartbeat: WS ping every 30s, pong required within 10s. Missed pong → connection torn down → reconnect.
- Reconnect: exponential backoff (1s → 2s → 4s → 8s → 30s capped). Permanent failure (e.g., auth rejected) marks the OC connection unhealthy, surfaces in `/v1/health`, and stops automatic reconnect - user must re-pair OC.
- On reconnect, gateway re-issues all active subscriptions to OC with the latest known sequence positions. Subscription gap handling is in §22C.7.3.

### §22C.4.3 TLS

**Architectural, not Tailscale-dependent.** The gateway terminates TLS itself. No assumption that the network layer is encrypted.

Cert source at launch (two-tier, per Spike #2 finding 2026-04-28):

- **Primary: Tailscale cert** (`tailscale cert <hostname>`). Zero-config for Tailscale deployments - Tailscale handles LE DNS-01 behind the scenes, produces trusted CA certs, natural fit for the split-machine-over-Tailscale deployment model (§22C.1). 90-day expiry; manual renewal acceptable at launch. Gateway checks for Tailscale hostname availability at startup and uses this path when available.
- **Fallback: Self-signed cert + fingerprint pinning.** Generated at gateway startup via `rcgen`. Stored in gateway data directory. 5-year lifetime (rare rotation). Fingerprint (SHA-256) displayed at startup and surfaced via `/v1/health` for the user to confirm during pairing. Covers non-Tailscale setups (direct LAN, VPN, etc.). Proven model (Syncthing uses device-ID-as-fingerprint at scale).
- **mkcert for local dev** is supported but unspec'd - devs use what they want for local rings.
- **Full ACME automation** deferred to post-launch. `tls.cert_path` / `tls.key_path` config options reserved for user-supplied certs. `instant-acme` crate available when needed.

**Startup TLS resolution order:** (1) Check config for explicit cert path → use if present. (2) Check for Tailscale hostname → run `tailscale cert` if available. (3) Generate/load self-signed cert from data directory.

Pane client pins the cert fingerprint during pairing (applies to both Tailscale and self-signed - Tailscale certs have stable fingerprints within their 90-day window). After pairing, fingerprint mismatch is hard-fail. Fingerprint rotation is a re-pair operation.

The gateway-to-OC connection uses standard TLS verification against OC's cert. OC pairing carries forward as-is from current bridge behavior.

## §22C.5 Auth and multi-tenancy seam

**Threat model at launch:** single-user, two machines, on Tailscale (or LAN). Tailscale handles network-level access control. The gateway provides tenant-scoped operation primitives; it does not enforce tenant identity against attackers at launch.

At TCP accept, the gateway checks the connecting remote address against an allowlist — defined in the gateway config file rather than hardcoded, so it is visible and operator-overridable, and defaulting to loopback plus the Tailscale ranges `100.64.0.0/10` and `fd7a:115c:a1e0::/48` — refusing any connection whose address falls outside it and logging only the rejected remote address, which makes this allowlist check, not Tailscale or TLS, the gateway's actual network-level access enforcement at launch.

### §22C.5.1 Token model

**Opaque bearer tokens.** Issued during pairing, stored on Pane and on the gateway. No JWT at launch - token is a 256-bit random string, gateway looks it up server-side.

**Pairing flow:**

1. Gateway started on the OC machine. On first run, generates a 8-digit pairing code (5-min expiry) and the TLS cert fingerprint. Displays both.
2. User enters URL + code + fingerprint into Pane.
3. Pane exchanges code for an opaque token via `POST /pair`. Token is long-lived (90 days, auto-rotate before expiry).
4. Pane stores token + fingerprint + gateway cert PEM (from `/v1/health` `tls_cert_pem` field). All subsequent HTTP clients use `add_root_certificate(stored_pem)` with `tls_built_in_root_certs(false)` - only the gateway's specific certificate is trusted. Shared builder: `gateway_client::build_gateway_client(db)`. Pre-pairing requests (fingerprint fetch) use `danger_accept_invalid_certs` via `build_pre_pairing_client()`.

**Pairing-code generation.** Pane Gateway generates pairing codes via `auth_state.generate_pairing_code()`. Codes are 8 digits, valid for 5 minutes from generation, single-use.

Two trigger paths:

1. **On gateway startup.** `pane-gateway` (no subcommand) starts the gateway server and emits the first pairing code plus the TLS fingerprint to stderr. This is the bootstrap path — the OC operator starts the gateway, reads the pairing code off the terminal, and gives it to the user pairing from Pane. The fingerprint is also emitted at the same time, but Pane fetches the fingerprint over `/v1/health` at probe time rather than relying on terminal copy-paste; the stderr fingerprint exists for visual comparison.

2. **On demand after startup.** `pane-gateway pairing-code` is a CLI subcommand that invokes `auth_state.generate_pairing_code()` against the running gateway's state and prints the resulting code to stdout. This is the recovery path for the 5-minute expiry: if the user takes longer than 5 minutes to enter the code in Pane, the OC operator runs this subcommand to mint a fresh one.

The CLI subcommand requires the gateway to be running and to share state with the invoking command. Implementation detail (out of scope for this spec section): the subcommand either connects to the running gateway via a local IPC mechanism or reads/writes the auth state file directly. Pane Gateway's implementation PR resolves which.

**Pairing-code generation is not exposed as an HTTP endpoint.** No `/v1/pairing-code` or admin endpoint. The pairing code is the bootstrap secret that establishes the bearer token — exposing it via an HTTP surface would create a chicken-and-egg auth requirement. The CLI subcommand runs in the OC operator's local terminal context, which is the appropriate trust boundary.

**TLS fingerprint acquisition and recovery.** The fingerprint is stable across the gateway's lifetime because the cert persists at `~/.pane-gateway/gateway.crt` (paired with `gateway.key`; default location, configurable via `GATEWAY_DATA_DIR` env var). Recovery paths, ordered by ease:

1. Read it from the gateway's startup stderr banner (primary path; emitted on every `pane-gateway` start).
2. Fetch it from `GET /v1/health` (works once the gateway is up; this is what Pane does at probe time).
3. Compute it from `~/.pane-gateway/gateway.crt` directly (or `$GATEWAY_DATA_DIR/gateway.crt` if overridden): `openssl x509 -in ~/.pane-gateway/gateway.crt -outform DER | shasum -a 256`. Useful for advanced users or scripted recovery.
4. Restart the gateway — cert persists across restarts, so the banner re-emits the same fingerprint.

The fingerprint never needs a regeneration command because it doesn't expire and doesn't change. Earlier drafts referenced `openclaw gateway fingerprint` as a regeneration mechanism; that was a spec error and is removed.

**Why opaque, not JWT.** Simpler at single-tenant; no token-format migration cost when Tier 1 introduces signed tokens. Gateway has a `tokens` table; resolution is a single lookup. JWT can be introduced post-launch with a `token_type` discriminator; existing opaque tokens stay valid.

**Token rotation.** Gateway issues a new token 7 days before expiry; Pane swaps on next call. Failed rotation surfaces a re-pair prompt at the 90-day mark.

### §22C.5.2 Tenant isolation seam

**Every gateway operation is scoped by `tenant_id`.** At launch, exactly one tenant exists per gateway deployment (`default`). The seam is structural, not implementation:

- Token resolution returns `tenant_id` (always `default` at launch).
- Every gateway-side state table - `sessions`, `sync_queue`, `sync_pending`, `tenant_agents`, `tokens`, `tenant_oc_credentials` - has a `tenant_id` column. Default value at launch is `default`.
- Every endpoint handler reads `tenant_id` from the auth context and passes it to every query. No global queries; no "give me all sessions" - only "give me all sessions for tenant X."
- Gateway holds OC connection state per tenant. At launch: one tenant, one OC connection. Tier 1: N tenants, N OC connections (or pooled).

**Per-tenant quotas and rate limits.** Schema present, no-op at launch. `tenant_quotas` table with `tenant_id`, `quota_kind` (e.g., `requests_per_minute`, `max_sessions`, `max_agents`), `value`. Endpoint middleware checks quotas before dispatching. At launch, quota table is empty for `default`, middleware short-circuits with no enforcement.

**Tier 1 flip:** introduce real tenant resolution from token, multi-tenant pairing UI, OC connection pooling, and quota population. None of those touch the endpoint handlers - they all already pass `tenant_id` through.

### §22C.5.3 Gateway → OC auth

Gateway authenticates to OC as the user, using the OC pairing token. At launch, gateway holds one OC token in its config (`oc.bearer_token`), used to establish the gateway↔OC WS connection. Token is opaque to the gateway.

In Tier 1, the gateway holds a per-tenant OC token mapping (`tenant_oc_credentials` table, `tenant_id` → opaque OC token). User-supplied tokens. Gateway never proxies user-side OC tokens to other tenants.

### §22C.5.4 Gateway → OC token-rotation recovery

**Problem.** OC owns the gateway token (§22C.5.3, ADR-0013). The Pane gateway reads it once, from the `OC_BEARER_TOKEN` environment variable, at process startup (`src-gateway/src/main.rs`); there is no reload path. Re-pairing refreshes only the Pane↔gateway token (`pair.rs`) — it never reads or writes `OC_BEARER_TOKEN`. When OC rotates its gateway token (observed after the 2026-07-07 `openclaw update` / SecretRefs migration; also reachable via `openclaw doctor --fix`, config reset, and setups that auto-rotate), the Pane gateway is stranded with a stale token until an operator updates the service file and restarts the gateway. This reaches any two-machine OC user across a token rotation, not only the launch operator.

**Failure signature (source-locked).** OC validates the gateway token at the connect-*frame* layer, after the WebSocket upgrade (§22C.6.5). A stale token therefore opens the socket cleanly, then dies silently: OC sends no rejection frame and no `hello-ok`. The gateway's hello-ok wait — `timeout(Duration::from_secs(10), hello_rx)` in `oc_client.rs` — elapses at ~10 s and today collapses into an opaque 502 on the OC-proxied endpoints (`/v1/agents`, `/v1/sync/initial`). No auth-coded rejection exists to key on; the sole distinguishing signal is the fixed ~10 s handshake timeout. The ~10 s bound uniquely identifies this arm: `connect_async` failures fail fast → `OcConnectionLost`; the auth-retry loop bounds at 5 s; the request timeout is 30 s → 504. This structural bound is the contract's source of truth in place of a captured failure-arm log, which does not survive gateway restart.

**Detection contract.** The gateway MUST distinguish the hello-ok-timeout arm from other 502 causes at the source:

- A dedicated `GatewayError::OcHandshakeTimeout` variant is thrown at the hello-ok-timeout arm (the `Err(_)` branch of `timeout(Duration::from_secs(10), hello_rx)`), replacing the generic `OcClient("Authentication timeout")` at that call site only.
- It MUST remain distinct from `OcConnectionLost` (WS never opened — OC down / unreachable / TLS) and from the 5 s auth-retry-loop `OcClient("Authentication timeout")` string, which shares wording but is a different arm. Detection MUST NOT string-match `"Authentication timeout"`; it keys on the dedicated variant.
- `OcHandshakeTimeout` maps to HTTP **502** on the wire (status unchanged), carrying the structured code `error: "oc_handshake_timeout"` in the `ErrorResponse` body (`src-gateway/src/error.rs`). Pane discriminates on this `error` code, not on the HTTP status — consistent with the existing OC error codes that all surface at 502 (`oc_unauthorized`, `oc_connection_lost`, …).

**Surface contract (Pane).** On `error === "oc_handshake_timeout"`, the Pane-side sync error mapping — where `fetch_gateway_agents` today renders the generic "Could not reach gateway" — MUST render a recovery message that:

- names a stale gateway→OC token as the **likely** cause, without asserting it as certain, and acknowledges the alternative (OC restarting or busy immediately after the WS upgrade);
- is agent-actionable from the message alone: it names the source of truth (`~/.openclaw/openclaw.json` → `gateway.auth.token`, on the OC machine), the target (`OC_BEARER_TOKEN` in the Pane gateway service file — macOS launchd plist / Linux systemd unit), and the restart step (restart the **Pane gateway** service so it re-reads the env var).

**Recovery message copy.**

> **Can't finish connecting to OpenClaw — the gateway's OpenClaw token may be out of date.**
>
> The Pane gateway reached OpenClaw but couldn't complete the authenticated handshake within 10 seconds. The most likely cause is that OpenClaw rotated its gateway token and the Pane gateway is still using the old one. (Less commonly, OpenClaw may be restarting or busy right after the connection opens — if this clears on its own, the token is fine.)
>
> To update a stale token:
>
> 1. On the OpenClaw machine, read the current token at `~/.openclaw/openclaw.json` → `gateway.auth.token`.
> 2. Find the Pane gateway's service file — the one that sets `OC_BEARER_TOKEN` — and change that value to match. Let the search locate it, whatever it's named:
>    - **macOS (launchd):** `grep -rl OC_BEARER_TOKEN ~/Library/LaunchAgents/ /Library/LaunchAgents/ /Library/LaunchDaemons/ 2>/dev/null` — edit the `OC_BEARER_TOKEN` entry under `EnvironmentVariables` in the plist it returns.
>    - **Linux (systemd):** `grep -rl OC_BEARER_TOKEN /etc/systemd/system/ ~/.config/systemd/user/ 2>/dev/null` — edit the `Environment=OC_BEARER_TOKEN=…` line in the unit it returns.
> 3. Restart the **Pane gateway** so it re-reads the token — using the file the previous step found, not a fixed name:
>    - **macOS (launchd):** take the `Label` value from that plist and run `launchctl kickstart -k gui/$(id -u)/LABEL` (for a user agent) or `launchctl kickstart -k system/LABEL` (for a system daemon).
>    - **Linux (systemd):** `systemctl restart UNIT` using the unit filename from step 2 (add `--user` if it lives under `~/.config/systemd/user/`).

**Scope boundary.** Detection-and-surface only. Pane MUST NOT capture, refresh, or re-derive the OC token, and MUST NOT rewire re-pairing to touch it — OC owns the token and the gateway reads it startup-only by design (ADR-0013). The two-copy token model (Pane↔gateway vs gateway→OC) is unchanged. See ADR-0055.

## §22C.6 Pane-facing HTTP/2 API

All endpoints prefixed with `/v1/`. Versioning is path-based; `v2` is reserved as a future-compatible namespace alongside `v1`.

All endpoints require `Authorization: Bearer <token>` unless noted. Authentication errors are 401. Rate-limit responses are 429 (no enforcement at launch but semantics locked).

### §22C.6.1 Pairing & health

| Method | Path | Purpose |
|---|---|---|
| `POST` | `/pair` | One-time. Body: `{ code }`. Returns: `{ token, expires_at, tenant_id, gateway_version }`. No bearer required. |
| `GET` | `/v1/health` | Returns `{ status, uptime, tls_fingerprint, oc_connection_status, gateway_version, tenant_id }`. `status` and `uptime` available without auth; full body requires bearer. |

### §22C.6.2 Sync (porting from §22B.3.10)

**Status:** BUILT (PR #718 endpoints, PR #752 atomic writes + endpoint naming fix). Client `SyncPushResponse` aligned to `{ written_at, checksum }` contract in v0.65.36 (PR #1178, #1176).

Four operations, one-to-one with current bridge ops. Renamed for clarity (no `/bridge/` prefix).

| Method | Path | Purpose |
|---|---|---|
| `POST` | `/v1/sync/push` | Push a single file. Body: `{ oc_agent_id, filename, content, checksum }`. Returns: `{ written_at, checksum }`. 10 MiB content cap (§22C.13). |
| `GET` | `/v1/sync/pull?oc_agent_id=...` | Retrieve pending changes for an agent. Returns `{ pending_changes: [{ id, oc_agent_id, filename, content, checksum, queued_at }] }`. |
| `DELETE` | `/v1/sync/pull/confirm` | Body: `{ oc_agent_id, processed_ids }`. Clears confirmed entries from `sync_pending`. |
| `GET` | `/v1/sync/initial` | Initial bulk fetch. Returns `{ agents: [{ oc_id, name, is_default, files: [{ filename, content, checksum }] }] }`. `is_default` is forwarded from OC's per-agent `isDefault` (§22C.6.4; ADR-0023 capture in §22C.10). Replaces `/bridge/sync-all`. Per-agent workspace paths are resolved from OC's `agents.list` response `workspace` field (§22C.6.4); see ADR-0040. |

**Path resolution for `/v1/sync/push` and `/v1/sync/initial`.** Both endpoints resolve per-agent workspace paths from OC's `agents.list` response — specifically, the `workspace` field forwarded via the gateway's agent registry (§22C.10). The gateway does not read `openclaw.json` from the OC machine's filesystem; OC's RPC surface is the contract per ADR-0015. Resolution is per-call against the existing registry cache, refreshing at the registry's TTL or on agent-set change. If `workspace` is absent, null, or empty for an agent the endpoint targets, the gateway returns 500 with structured logging including `oc_agent_id` and OC version — no hardcoded fallback path. Sub-agents resolve via the same mechanism; symlinks at the workspace path are followed transparently. See ADR-0040 for full mechanism and error-handling rules.

#### Default agent resolution (ADR-0067 D5)

Every call site that needs "the default agent" — session bind/self-heal (`docs/spec/sessions.md` §30.1.3), the parameter-compat strip-table lookup (`docs/spec/ai-integration.md` §4A.2.2), the agent picker's preselection, and any other resolver — uses this order, stated once here and referenced everywhere else instead of re-implemented:

1. **OC's reported default** — the agent `agents.list` marks `isDefault: true` (§22C.6.4, §22C.10).
2. **The user's Settings choice** — `ai_agents.is_default`, set through `DefaultAgentPickerModal`.
3. **The agent whose `runtime_ref == "main"`.**

Against OC 2026.8.2, every agent reports `isDefault: false` (§22C.10's ADR-0023 capture), so step 1 does not fire today; step 2 is the live production path and step 3 is the first-run path before any Settings choice exists.

Behavioral semantics carry forward verbatim from §22B.3 and are restated in §22C.8 for self-containment.

### §22C.6.3 Model discovery

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/v1/models` | Returns `{ models: [{ provider, id, label }], source, refreshed_at }`. Replaces the AI-as-librarian hack in `extraction_worker.rs`. Internally calls `models.list` over WS. Cached 30s; force refresh via `?refresh=1`. |

### §22C.6.4 Agents

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/v1/agents` | List agents for the tenant. Internally calls `agents.list`. Returns `{ agents: [{ oc_agent_id, name, model, role }] }`. No caching - always live. **OC response mapping:** OC's `agents.list` returns agents with field `id` (not `agent_id`), which gateway maps to `oc_agent_id` in the Pane-facing response. OC's `name` field is nullable; gateway falls back to `id` when null. OC's `model` is an object with a `primary` field; gateway extracts the primary model string. OC's `workspace` field (added to capture per ADR-0040; source: `session-utils-hDe0hdis.js:413-455`, `listAgentsForGateway(cfg)` line 449 `workspace: resolveAgentWorkspaceDir(cfg, id)`) is forwarded into the gateway's internal agent registry (§22C.10) and used for sync-layer path resolution (§22C.6.2); it is **not** forwarded into the Pane-facing `/v1/agents` response — Pane has no use for OC-machine filesystem paths and the gateway treats `workspace` as internal state. (OC response shape current as of OC v2026.4.15; `workspace` field verified present on OC v2026.4.11+.) |

### §22C.6.5 Sessions

`session_id` is gateway-assigned. Gateway maps to OC's session ID internally. From Pane's perspective, the gateway is the source of truth for session existence - OC may have additional sessions Pane doesn't know about; the gateway only surfaces sessions it created on Pane's behalf.

| Method | Path | Purpose |
|---|---|---|
| `POST` | `/v1/sessions` | Body: `{ oc_agent_id, title?, context_hint? }`. Internally `sessions.create`. Returns `{ session_id, created_at, oc_session_id }`. **OC response mapping:** OC returns `sessionId` (camelCase) and `key`; gateway reads `sessionId` with fallback to `key`. Gateway sends `agentId` param (not `oc_agent_id`). |
| `GET` | `/v1/sessions` | List active sessions for the tenant. Internally `sessions.list`, filtered. Returns `{ sessions: [{ session_id, title, oc_agent_id, created_at, last_message_at }] }`. **OC response mapping:** OC returns camelCase fields (`sessionId`, `displayName`, `agentId`, `createdAt`, `updatedAt`); gateway manually maps to snake_case (`session_id`, `title`, `oc_agent_id`, `created_at`, `last_message_at`). |
| `GET` | `/v1/sessions/:session_id/messages` | Paginated message history. Query: `?before=<message_id>&limit=N` (paginate backwards, for loading history) or `?since=<message_id>&limit=N` (paginate forwards, for gap recovery). Default limit 50, max 200. Returns `{ messages: [...], has_more, next_cursor }`. Backed by `chat.history`. **OC response mapping:** OC returns camelCase pagination fields (`hasMore`, `nextCursor`); gateway maps to snake_case (`has_more`, `next_cursor`). <!-- OC response capture (ADR-0023) Source: dist/server/chunks/chat-CMSNlsvD.js (lines ~1795-1848) OC version: v2026.4.11 Captured: 2026-05-01 Note: OC is pre-1.0; field names may change without notice Audit: §826 cycle --> |
| `DELETE` | `/v1/sessions/:session_id` | Internally `sessions.delete`. Returns `{ deleted_at }`. **OC param mapping:** Gateway sends `key` param (not `session_id`). |
| `POST` | `/v1/sessions/:session_id/messages` | Send a message. Body: `{ content, role: "user", idempotency_key }`. Internally `sessions.send`. Returns `{ message_id, accepted_at, oc_message_id }`. **Non-streaming.** Caller subscribes to the SSE endpoint to receive the assistant response. **OC param/response mapping:** Gateway sends `key` param (not `session_id`). OC returns `messageSeq` (number); gateway converts to string `oc_message_id`. |
| `GET` | `/v1/sessions/:session_id/messages/stream` | SSE. Streams session events. `Last-Event-ID` supported for replay. |
| `POST` | `/v1/sessions/:session_id/abort` | Cancel in-flight generation. Internally `sessions.abort`. Returns `{ aborted_at }`. **OC param mapping:** Gateway sends `key` param (not `session_id`). |

**Removed endpoint:** `GET /v1/sessions/:session_id` (session metadata) was removed in v0.65.4. Zero callers verified - frontend uses Tauri invoke, not HTTP; Tauri backend has no HTTP calls to this endpoint. YAGNI principle: future consumers will spec the endpoint when they need it. See PR body for rationale.

#### OC WebSocket Connection Protocol

<!-- §827 amendment: OC WebSocket connection protocol capture (2026-05-03) -->

> **OC capture:** OC v2026.4.11 (2026-05-03), `/opt/homebrew/lib/node_modules/openclaw/dist/server.impl-GQ72oJBa.js` + `protocol-C6T5DFc8.js`. Versioning: unstable; re-verify on OC version bumps.

**All methods in §22C.6.5 depend on this connection being established first.**

**Connection flow:**
1. Client connects via WebSocket to `ws://HOST:PORT/` with `Authorization: Bearer TOKEN` header
2. Server sends `connect.challenge` event: `{ type: "event", event: "connect.challenge", payload: { nonce: "<uuid>", ts: <unix_ms> } }`
3. Client responds with `connect` request frame (see schema below)
4. Server validates and responds with `hello-ok` (protocol 3, features list, server version)

**Connect request params (strict - additionalProperties: false):**
```json
{
  "minProtocol": 3,
  "maxProtocol": 3,
  "client": {
    "id": "gateway-client",
    "displayName": "Pane Gateway",
    "version": "0.1.0",
    "platform": "macos",
    "mode": "backend"
  },
  "auth": { "token": "<oc-bearer-token>" },
  "role": "operator",
  "caps": [],
  "locale": "en-US",
  "userAgent": "pane-gateway/0.1.0"
}
```

**hello-ok response shape:**
```json
{
  "type": "hello-ok",
  "protocol": 3,
  "server": { "version": "<oc-version>", "connId": "<uuid>" },
  "features": { "methods": [...], "events": [...] },
  "snapshot": { ... },
  "policy": { "maxPayload": "<bytes>", "maxBufferedBytes": "<bytes>", "tickIntervalMs": "<ms>" }
}
```

**Request frame format (all subsequent method calls):**
```json
{ "type": "req", "id": "<unique-string>", "method": "<method-name>", "params": { ... } }
```

**Response frame format:**
```json
{ "type": "res", "id": "<matching-id>", "ok": true, "payload": { ... } }
```
Or error: `{ "type": "res", "id": "<matching-id>", "ok": false, "error": { "code": "<code>", "message": "<desc>" } }`

**Protocol requirements:**
- Protocol version must be 3 (`maxProtocol >= 3 && minProtocol <= 3`)
- `client.id` must be `"gateway-client"` (allowed list enforced server-side)
- `client.mode` must be `"backend"` (allowed list enforced server-side)
- Token auth: `auth.token` set to OC bearer token; device identity not required (`roleCanSkipDeviceIdentity` returns true for token auth with role "operator")
- Handshake timeout: configurable, defaults to a few seconds; client must send connect frame before timeout
- After hello-ok: 30s ping / 10s pong keepalive (§22C.4.2)

**Scope assignment:** Bearer token auth assigns the token's native scopes to the connection. The gateway's OC admin token has `operator.read`, `operator.write`, `operator.admin` - all methods in §22C.6.5 are accessible.

#### `models.list`

> **OC capture:** OC v2026.4.11 (2026-05-03), `/opt/homebrew/lib/node_modules/openclaw/dist/server.impl-GQ72oJBa.js:10092-10109`. Schema source: `protocol-C6T5DFc8.js:330`. Versioning: unstable; re-verify on OC version bumps.

**Required scope:** `operator.read` (`method-scopes-D3xbsVVt.js:67`)

**Request shape (OC):**
```json
{}
```
Empty object, `additionalProperties: false`. No parameters.

**Response shape (OC, ok: true):**
```json
{
  "models": [
    {
      "id": "string (provider/model format, e.g. 'anthropic/claude-sonnet-4-5')",
      "name": "string (human-readable display name)",
      "provider": "string (provider identifier)",
      "contextWindow": "number (optional, max tokens)",
      "reasoning": "boolean (optional, supports extended thinking)",
      "input": "number (optional, cost per million input tokens)"
    }
  ]
}
```

**Error codes:** `INVALID_REQUEST` (validation failed), `UNAVAILABLE` (catalog load error)

**Events emitted:** None

**Gateway → Pane mapping:** Pane's `GET /v1/models` route invokes OC `models.list`. Mapping:
- OC's `models[].id` → Pane's `id`
- OC's `models[].name` → Pane's `label` (display name in UI)
- OC's `models[].provider` → Pane's `provider`
- Optional fields (`contextWindow`, `reasoning`, `input`) forwarded when present
- Gateway caches response for 30 seconds; force refresh via `?refresh=1` query param

**Implementation notes:**
- Empty params object is strict (`additionalProperties: false`) - gateway must send `{}` not `null`
- OC returns the full allowed catalog (filtered by config's model allowlist); gateway does not filter further
- The HTTP `/v1/models` endpoint wraps the response in OpenAI-compatible format (`object: "list", data: [...]`) for the HTTP surface; the WS method returns the raw shape above

#### OC method captures (sessions + chat)

> **Status:** NOT BUILT (captures land here per ADR-0023; gateway-side adapter fixes follow in a separate code PR)

This subsection lands the OC contract captures for the eight WS methods the gateway invokes for sessions and chat workflows: `sessions.list`, `sessions.create`, `sessions.send`, `sessions.delete`, `sessions.abort`, `chat.history`, `chat.send`, and `sessions.messages.subscribe`. Each capture follows the format established in §22C.6.4 (`agents.list`, PR #844): inline JSON request/response shapes from OC's actual code, source-path cited, OC version stamped, versioning policy noted.

The captures are the canonical reference for the gateway's adapter layer. Gateway code that deserializes OC responses or constructs OC requests matches these shapes; any drift is either a gateway bug or a stale capture (re-verify against current OC version per ADR-0023).

**Source for all captures in this subsection:** Reed's contract sweep audit, `~/Shared/ExampleApp/docs/audits/2026-05-03-oc-remaining-contracts.md`. Captured from OC v2026.4.11 on 2026-05-03 against `/opt/homebrew/lib/node_modules/openclaw/dist/`. Versioning: unstable, no SemVer contract - OC schema changes ship with version bumps. Captures must be re-verified on every OC upgrade; use ADR-0018's CI smoke check (`startup-smoke.yml`) plus periodic Reed audits as the drift-detection layer.

##### §22C.6.5.0 Cross-cutting gateway adapter rules

Four adapter-layer rules apply across all eight captures below. They are spec'd here in the preamble rather than repeated in each per-method capture.

**Rule 1 - Route param `:id` maps to OC's `sessionKey`.** Pane's HTTP surface uses `:id` in routes like `GET /v1/sessions/:id/messages`. OC uses `sessionKey` as the primary identifier (canonical store key, e.g., `agent:main:dashboard`, `unknown`, `global`). Gateway maps the route param `:id` → request param `sessionKey` when invoking OC RPC methods. The gateway never exposes OC's `sessionKey` to Pane; Pane sees `:id` consistently.

**Rule 2 - OC responses are camelCase.** Every OC method response uses camelCase field names (`sessionKey`, `agentId`, `idempotencyKey`, `messageSeq`, `runId`, etc.). Gateway Rust structs that deserialize OC responses need `#[serde(rename_all = "camelCase")]` at the struct level. Per-field `#[serde(rename = "...")]` annotations left over from the original §22C invented field names should be removed in favor of the struct-level rule. The captures below are authoritative for the exact field names.

**Rule 3 - `chat.send` is the streaming primitive.** The original §22C.7.3 design assumed `sessions.messages.subscribe` would deliver streaming chat output. Reed's capture confirms a different pattern: `chat.send` returns `{runId, status: "started"}` immediately, then broadcasts `chat` events via OC's `context.broadcast` to subscribed clients. The gateway must subscribe to `chat` events for the session (via `sessions.messages.subscribe`) before invoking `chat.send`, then forward the streamed `chat` events through the gateway's SSE channel to Pane. The two-method dance is spec'd in detail in §22C.6.5.7. INVARIANT-relevant: the gateway never invents the streaming protocol; it forwards OC's actual events.

**Rule 4 - `idempotencyKey` is required and camelCase on `chat.send`.** OC requires `idempotencyKey` on every `chat.send` invocation; runs are deduplicated against this key (returning `status: "in_flight"` if a key is already active). Gateway code that currently uses `idempotencyK` (truncated) or `idempotency_key` (snake_case) - both observed in audit findings - is incorrect. Fix is mechanical: rename to `idempotencyKey`, mark required on the gateway-side struct, generate a UUID if Pane didn't supply one.

**Implementation responsibility.** All four rules are Stia's follow-up code PR work. This spec lands the captures and names the rules; the code PR applies them. Reed audits the code PR against the §22C.6.5 captures.

##### §22C.6.5.1 `sessions.list`

> **OC capture:** OC v2026.4.11 (2026-05-03), `/opt/homebrew/lib/node_modules/openclaw/dist/server.impl-GQ72oJBa.js:12824-12835`. Schema source: `protocol-C6T5DFc8.js:1578-1590`. Implementation source: `session-utils-hDe0hdis.js:850-920`. Versioning: unstable; re-verify on OC version bumps.

**Required scope:** `operator.read` (`method-scopes-D3xbsVVt.js:77`)

**Request shape (OC):**
```json
{
  "limit": "number (optional, min 1)",
  "activeMinutes": "number (optional, min 1)",
  "includeGlobal": "boolean (optional)",
  "includeUnknown": "boolean (optional)",
  "includeDerivedTitles": "boolean (optional)",
  "includeLastMessage": "boolean (optional)",
  "label": "string (optional, SessionLabelString)",
  "spawnedBy": "string (optional, NonEmptyString)",
  "agentId": "string (optional, NonEmptyString)",
  "search": "string (optional)"
}
```

**Response shape (OC, ok: true):**
```json
{
  "ts": "number (timestamp)",
  "path": "string (store path)",
  "count": "number",
  "defaults": "object (session defaults from config)",
  "sessions": [
    {
      "key": "string (canonical session key, camelCase)",
      "sessionId": "string (UUID or derived ID, optional)",
      "agentId": "string (optional)",
      "spawnedBy": "string (optional)",
      "spawnedWorkspaceDir": "string (optional)",
      "forkedFromParent": "boolean (optional)",
      "spawnDepth": "number (optional)",
      "subagentRole": "string (optional)",
      "subagentControlScope": "string (optional)",
      "kind": "string (session classification)",
      "label": "string (optional)",
      "displayName": "string (optional)",
      "derivedTitle": "string (optional, if includeDerivedTitles=true)",
      "lastMessagePreview": "string (optional, if includeLastMessage=true)",
      "modelProvider": "string",
      "model": "string",
      "channel": "string (optional)",
      "subject": "string (optional)",
      "groupChannel": "string (optional)",
      "space": "string (optional)",
      "chatType": "string (optional)",
      "origin": "string (optional)",
      "updatedAt": "number (timestamp, optional)",
      "startedAt": "number (timestamp, optional)",
      "endedAt": "number (timestamp, optional)",
      "runtimeMs": "number (optional)",
      "status": "string (optional)",
      "systemSent": "boolean (optional)",
      "abortedLastRun": "boolean (optional)",
      "thinkingLevel": "string (optional)",
      "fastMode": "boolean (optional)",
      "verboseLevel": "string (optional)",
      "traceLevel": "string (optional)",
      "reasoningLevel": "string (optional)",
      "elevatedLevel": "string (optional)",
      "sendPolicy": "string (optional)",
      "inputTokens": "number (optional)",
      "outputTokens": "number (optional)",
      "totalTokens": "number (optional)",
      "totalTokensFresh": "boolean (optional)",
      "estimatedCostUsd": "number (optional)",
      "parentSessionKey": "string (optional)",
      "childSessions": "string[] (optional)",
      "responseUsage": "string (optional)",
      "contextTokens": "number (optional)",
      "deliveryContext": "object (optional)",
      "lastChannel": "string (optional)",
      "lastTo": "string (optional)",
      "lastAccountId": "string (optional)",
      "lastThreadId": "string (optional)",
      "compactionCheckpointCount": "number (optional)",
      "latestCompactionCheckpoint": "object (optional)"
    }
  ]
}
```

**Error codes:** `INVALID_REQUEST` (validation failed on params)

**Events emitted:** None

**Gateway → Pane mapping:** Pane's `GET /v1/sessions` route invokes OC `sessions.list` with no required params (or filters per Pane query string) and returns a Pane-shaped subset to the client. Pane's response surface (specified in §22C.6.2) is a thinned projection of OC's session record - Pane does not surface every OC field. Mapping rules:
- Pane's `id` (in route and response) → OC's `key`.
- Pane's `agent_id` → OC's `agentId`.
- Pane's `title` → OC's `displayName` (fall back to `label`, then to `derivedTitle` if `includeDerivedTitles=true`).
- Pane's `updated_at` → OC's `updatedAt`.
- Fields OC returns that Pane does not surface (e.g., `runtimeMs`, `responseUsage`, internal compaction checkpoints) are dropped at the gateway layer; not exposed in Pane's response.

**Implementation notes:**
- Sessions sorted by `updatedAt` descending. The gateway preserves OC's sort order.
- `key` is the canonical store key (e.g., `agent:main:dashboard`); `sessionId` is the transcript file UUID. Pane uses `key` as `id` per Rule 1.
- Optional flags (`includeGlobal`, `includeUnknown`, etc.) default false at OC; the gateway exposes them via Pane query params for filtering.
- Empty `sessions` array is a valid response (no sessions exist) - gateway returns `[]`, not an error.

##### §22C.6.5.2 `sessions.create`

> **OC capture:** OC v2026.4.11 (2026-05-03), `/opt/homebrew/lib/node_modules/openclaw/dist/server.impl-GQ72oJBa.js:12985-13130`. Schema source: `protocol-C6T5DFc8.js:1604-1612`. Versioning: unstable; re-verify on OC version bumps.

**Required scope:** `operator.write` (`method-scopes-D3xbsVVt.js:122`)

**Request shape (OC):**
```json
{
  "key": "string (optional, NonEmptyString - requested session key)",
  "agentId": "string (optional, NonEmptyString)",
  "label": "string (optional, SessionLabelString)",
  "model": "string (optional, NonEmptyString)",
  "parentSessionKey": "string (optional, NonEmptyString)",
  "task": "string (optional)",
  "message": "string (optional)"
}
```

**Response shape (OC, ok: true):**
```json
{
  "ok": true,
  "key": "string (canonical session key, may differ from requested)",
  "sessionId": "string (UUID or derived ID)",
  "entry": {
    "key": "string",
    "agentId": "string (optional)",
    "kind": "string",
    "label": "string (optional)",
    "displayName": "string (optional)",
    "modelProvider": "string",
    "model": "string",
    "startedAt": "number (timestamp)",
    "updatedAt": "number (timestamp)",
    "...": "(full session store metadata; see §22C.6.5.1 for the union of fields)"
  },
  "runStarted": "boolean",
  "runId": "string (optional, present if initial message triggered a run)",
  "status": "string (optional, run status: 'started' | 'ok' | 'in_flight')",
  "messageSeq": "number (optional, transcript message sequence if run started)",
  "runError": "object (optional, present if initial message failed)"
}
```

**Error codes:**
- `INVALID_REQUEST` - validation failed, agentId/key mismatch, unknown parent session, or send policy blocks.
- `UNAVAILABLE` - failed to create transcript file.

**Events emitted:**
- `sessions.changed` event with `reason: "create"` (via `context.broadcast`).
- If `runStarted` is true: additional `sessions.changed` event with `reason: "send"`.

**Gateway → Pane mapping:** Pane's `POST /v1/sessions` route invokes OC `sessions.create`. Mapping:
- Pane request body's `agent_id` → OC's `agentId`.
- Pane request body's `model` → OC's `model` (passes through; the gateway's model whitelist in §22C.6.3 applies upstream).
- Pane request body's `initial_message` → OC's `message`.
- OC's `key` → Pane's `id` in the response.
- OC's `runStarted` and `runId` are forwarded to Pane only if Pane's request asked to send an initial message; otherwise dropped from response.
- OC's `entry` (full store metadata) is thinned to Pane's session-detail shape (per §22C.6.2's response schema for `POST /v1/sessions`); not all fields surface.

**Implementation notes:**
- The gateway must use OC's returned `key` as the canonical identifier in subsequent calls - the requested key may have been mutated by OC (e.g., collision-resolution).
- If the request includes an initial message (`task` or `message`), OC delegates to `chat.send` internally and returns the run state. Gateway forwards run start to Pane; the streaming response arrives via the SSE channel per §22C.6.5.7.
- Transcript file creation failure surfaces as `UNAVAILABLE`; gateway should map this to a 503 in Pane's response.
- `parentSessionKey` is used for forked/spawned sessions; Pane's launch surface does not currently expose it.

**Divergence note (2026-10-28, live-test #1b, Part of #3050):** this schema/mapping text above was
already correct and was never the bug. `src-gateway/src/routes/sessions.rs::create()` diverged from
its OWN cited spec section by forwarding Pane's `title` and `context_hint` verbatim into the OC RPC
params object as additional, unrecognized properties — OC's validator rejected the call with
`INVALID_REQUEST: unexpected property 'context_hint'`. Fixed by a new `build_oc_create_params()`
helper: `title` → OC's `label` (omitted, not `null`, when absent); `context_hint` dropped entirely
(no OC equivalent exists, per the mapping list above). A new gateway unit test asserts every key the
helper emits is a member of this section's own documented field list, so a future invented field
fails that test without the test needing to know the field's name in advance. No contract change —
this note documents that the gateway's handler now actually matches the schema already captured here.

##### §22C.6.5.3 `sessions.send`

> **OC capture:** OC v2026.4.11 (2026-05-03), `/opt/homebrew/lib/node_modules/openclaw/dist/server.impl-GQ72oJBa.js:13250-13260` (delegates to `handleSessionSend` at `:12742-12820`). Schema source: `protocol-C6T5DFc8.js:1613-1622`. Versioning: unstable; re-verify on OC version bumps.

**Required scope:** `operator.write` (`method-scopes-D3xbsVVt.js:123`)

**Request shape (OC):**
```json
{
  "key": "string (NonEmptyString, session key)",
  "message": "string",
  "thinking": "string (optional)",
  "attachments": "array (optional, unknown element type)",
  "timeoutMs": "number (optional, min 0)",
  "idempotencyKey": "string (optional, NonEmptyString - auto-generated UUID if absent)"
}
```

**Response shape (OC, ok: true):**
```json
{
  "runId": "string",
  "status": "string ('started' | 'ok')",
  "messageSeq": "number (optional, message sequence in transcript)",
  "interruptedActiveRun": "boolean (optional, true if a prior run was interrupted)",
  "messageId": "string (optional, transcript message ID)"
}
```

**Error codes:** `INVALID_REQUEST` (validation failed, session not found, or send policy blocks)

**Events emitted:**
- `sessions.changed` event with `reason: "send"` (or `"steer"` if a prior run was interrupted).
- `chat` events streamed via `chat.send` delegation (see §22C.6.5.7 for the streaming pattern).

**Gateway → Pane mapping:** Pane's `POST /v1/sessions/:id/messages` route invokes OC `sessions.send` (which internally delegates to `chat.send`). Mapping:
- Route param `:id` → OC's `key` (per cross-cutting Rule 1).
- Pane request body's `content` → OC's `message`.
- Pane request body's `idempotency_key` → OC's `idempotencyKey` (per cross-cutting Rule 4 - required, camelCase). If Pane omits, gateway generates a UUID.
- OC's `runId`, `status`, `messageSeq`, `messageId` flow back to Pane in the immediate response. Pane uses these to correlate subsequent SSE events.
- The response is non-streaming; the streamed `chat` events arrive via the SSE channel (`GET /v1/sessions/:id/events`), set up before the `sessions.send` call. See §22C.6.5.7 and §22C.6.5.8 for the subscription pattern.

**Implementation notes:**
- `sessions.send` is a thin wrapper around `chat.send`. The gateway can invoke either; `sessions.send` is preferred per the §22C.7.1 whitelist (sessions namespace is canonical).
- `interruptedActiveRun: true` indicates a prior run was aborted before this one was sent (steer behavior). Surface to Pane via the `status` field if downstream UX needs it; otherwise drop.
- `attachments` shape is undocumented in OC's schema (`unknown element type`). Gateway treats as opaque pass-through if Pane includes them; Pane's surface does not expose attachments at launch.
- `timeoutMs` not currently surfaced by Pane; defaults to OC's internal timeout.
- Auto-generated UUID for `idempotencyKey`: gateway generates one client-side rather than relying on OC's auto-generation, so retries on the gateway → OC link can deduplicate cleanly.

##### §22C.6.5.4 `sessions.delete`

> **OC capture:** OC v2026.4.11 (2026-05-03), `/opt/homebrew/lib/node_modules/openclaw/dist/server.impl-GQ72oJBa.js:13402-13470` (handler inferred from response construction). Schema source: `protocol-C6T5DFc8.js:1676-1680`. Versioning: unstable; re-verify on OC version bumps.

**Required scope:** `operator.admin` (`method-scopes-D3xbsVVt.js:150`)

**Request shape (OC):**
```json
{
  "key": "string (NonEmptyString, session key)",
  "deleteTranscript": "boolean (optional, default true)",
  "emitLifecycleHooks": "boolean (optional, default true)"
}
```

**Response shape (OC, ok: true):**
```json
{
  "ok": true,
  "deleted": "boolean",
  "archived": "string[] (paths to archived transcripts)"
}
```

**Error codes:**
- `INVALID_REQUEST` - validation failed, session not found, cannot delete the main session, or webchat mutation rejected.
- Errors from `cleanupSessionBeforeMutation` (abort/cleanup failures bubble up).

**Events emitted:**
- `sessions.changed` event with `reason: "delete"`.
- Plugin hook: `session:end` (if `emitLifecycleHooks=true`).
- Lifecycle event: `session-delete` (if `emitLifecycleHooks=true`).

**Gateway → Pane mapping:** Pane's `DELETE /v1/sessions/:id` route invokes OC `sessions.delete`. Mapping:
- Route param `:id` → OC's `key` (per Rule 1).
- Gateway sends `deleteTranscript: true` and `emitLifecycleHooks: true` (OC defaults; gateway does not currently expose these to Pane). If a future Pane requirement needs to preserve transcripts, add a query param and forward to OC.
- OC's response (`deleted`, `archived`) is mapped to Pane's response shape - `deleted` becomes the success flag; `archived` paths are surfaced if the gateway ever exposes a session-archive view, otherwise dropped.

**Implementation notes:**
- **Scope is `operator.admin`, not `operator.write`.** This is the only delete in the whitelist with admin scope. Gateway must invoke with admin-scoped credentials; if the gateway's bearer token is provisioned with only `operator.write`, deletes fail with auth error. Stia confirms the gateway's bearer token has `operator.admin` scope per the pairing flow (§22C.5).
- The main session cannot be deleted (OC enforces). Pane surface should hide the delete affordance for the main session, or the gateway can return 403 with a clear message ("Cannot delete the main session").
- Archived transcripts are written to `<storePath>/.archive/`. Pane has no concept of the archive directory; this is OC-side bookkeeping.
- Webchat session deletion is rejected unless authorized; not currently relevant to Pane's surface.
- `cleanupSessionBeforeMutation` is OC's pre-delete hook (aborts active runs, etc.); failures here bubble up as the same error chain. Gateway maps to Pane's 5xx response.

##### §22C.6.5.5 `sessions.abort`

> **OC capture:** OC v2026.4.11 (2026-05-03), `/opt/homebrew/lib/node_modules/openclaw/dist/server.impl-GQ72oJBa.js:13274-13310`. Schema source: `protocol-C6T5DFc8.js:1623-1626`. Versioning: unstable; re-verify on OC version bumps.

**Required scope:** `operator.write` (`method-scopes-D3xbsVVt.js:125`)

**Request shape (OC):**
```json
{
  "key": "string (NonEmptyString, session key)",
  "runId": "string (optional, NonEmptyString)"
}
```

**Response shape (OC, ok: true):**
```json
{
  "ok": true,
  "abortedRunId": "string | null",
  "status": "string ('aborted' | 'no-active-run')"
}
```

**Error codes:**
- `INVALID_REQUEST` - validation failed, session not found, unauthorized.
- Errors from `chat.abort` (bubbled up - `sessions.abort` delegates internally).

**Events emitted:**
- `sessions.changed` event with `reason: "abort"` (only if a run was actually aborted).

**Gateway → Pane mapping:** Pane's `POST /v1/sessions/:id/abort` route invokes OC `sessions.abort`. Mapping:
- Route param `:id` → OC's `key` (per Rule 1).
- Pane's optional `run_id` query param → OC's `runId`.
- OC's `abortedRunId` → Pane's response (or `null` for no-op).
- OC's `status` ('aborted' | 'no-active-run') → Pane's response, used to inform the UI whether the abort had effect.

**Implementation notes:**
- If `runId` is omitted, OC aborts ALL active runs for the session and returns the first aborted run's ID. Pane's surface currently passes through whatever the user supplies; if Pane omits, the broadcast-abort is the result.
- `status: "no-active-run"` is a successful response, not an error - the abort was a no-op because nothing was running. Gateway returns 200 with this status; Pane's UI distinguishes via the status field.
- `sessions.abort` delegates to `chat.abort` internally. Per the §22C.7.1 whitelist, the gateway invokes `sessions.abort` (sessions namespace canonical); `chat.abort` is not directly invoked.
- Authorization rule: a non-admin caller cannot abort runs initiated by another user. Single-tenant Tier 3 deployments don't surface this constraint, but multi-tenant Tier 1 (post-launch) does. Spec'd here for forward-compatibility per §22C.5's tenancy seam.

##### §22C.6.5.6 `chat.history`

> **OC capture:** OC v2026.4.11 (2026-05-03), `/opt/homebrew/lib/node_modules/openclaw/dist/chat-CMSNlsvD.js:1795-1850`. Schema source: `protocol-C6T5DFc8.js:1323-1333`. Versioning: unstable; re-verify on OC version bumps.

**Required scope:** `operator.read` (`method-scopes-D3xbsVVt.js:98`)

**Request shape (OC):**
```json
{
  "sessionKey": "string (NonEmptyString)",
  "limit": "number (optional, min 1, max 1000)",
  "maxChars": "number (optional, min 1, max 500000)"
}
```

**Response shape (OC, ok: true):**
```json
{
  "sessionKey": "string",
  "sessionId": "string (optional)",
  "messages": [
    {
      "role": "string ('user' | 'assistant' | 'system')",
      "content": "array (message content blocks)",
      "timestamp": "number (optional)",
      "usage": "object (optional, token counts)",
      "stopReason": "string (optional)"
    }
  ],
  "thinkingLevel": "string (optional)",
  "fastMode": "boolean (optional)",
  "verboseLevel": "string (optional)"
}
```

**Error codes:** `INVALID_REQUEST` (validation failed)

**Events emitted:** None

**Gateway → Pane mapping:** Pane's `GET /v1/sessions/:id/messages` route invokes OC `chat.history`. Mapping:
- Route param `:id` → OC's `sessionKey` (per Rule 1).
- Pane's `limit` query param → OC's `limit` (default 200, max 1000).
- Pane's `max_chars` query param → OC's `maxChars` (defaults to OC config, max 500000).
- OC's `messages` array → Pane's response messages array, with each message's `content` blocks adapted to Pane's content-block schema (existing per §22C.6.2; not redefined here).
- OC's `thinkingLevel`, `fastMode`, `verboseLevel` are session-level metadata - gateway forwards as `session_settings` object on Pane's response if Pane needs them; otherwise dropped.

**Implementation notes:**
- Default `limit` is 200; explicit OC default if the caller omits.
- `maxChars` caps total payload size, not per-message length. Oversized messages are replaced with placeholders by OC (truncation marker in content); gateway forwards as-is.
- Envelope metadata is stripped by OC before return - `messages` is the decoded message stream, not the raw transcript file.
- Canvas blocks are augmented into the response if a session has them; treated as a content-block type, no special handling at the gateway.
- Pane's launch surface uses this method for: initial session view (load last N messages), scroll-back pagination (call again with smaller `limit` and a synthetic before-cursor on the gateway side, or full re-fetch - gateway implements the chosen pagination strategy).
- **No native pagination cursor in OC.** `chat.history` returns “last N messages.” If Pane requires cursor pagination (older-than-X), the gateway must implement it: fetch limit, return tail, track on Pane side. This is an adapter-layer responsibility, not a method-shape one.

> **Compliance note (2026-08-23, #2410 prerequisite PR):** the gateway’s `GET /v1/sessions/:id/messages` route previously did not implement the mapping this section documents — it sent `"key"` where OC’s schema requires `"sessionKey"`, and serialized absent `before`/`since` as JSON `null` (schema-rejected → 502). The prerequisite PR brings the implementation into conformance with this capture: `"sessionKey"` carries the OC session key (resolved by key, never a cached sessionId — see sessions.md §30.11.3), and unmapped pagination params are omitted rather than passed as null. No change to the documented OC request shape — the implementation now matches it.

##### §22C.6.5.7 `chat.send` (streaming primitive)

> **OC capture:** OC v2026.4.11 (2026-05-03), `/opt/homebrew/lib/node_modules/openclaw/dist/chat-CMSNlsvD.js:1918-2410`. Schema source: `protocol-C6T5DFc8.js:1334-1347`. Versioning: unstable; re-verify on OC version bumps.

**Required scope:** `operator.write` (`method-scopes-D3xbsVVt.js:120`)

**Request shape (OC):**
```json
{
  "sessionKey": "string (ChatSendSessionKeyString)",
  "message": "string",
  "thinking": "string (optional)",
  "deliver": "boolean (optional)",
  "originatingChannel": "string (optional, requires admin scope)",
  "originatingTo": "string (optional, requires admin scope)",
  "originatingAccountId": "string (optional, requires admin scope)",
  "originatingThreadId": "string (optional, requires admin scope)",
  "attachments": "array (optional, unknown element type)",
  "timeoutMs": "number (optional, min 0)",
  "systemInputProvenance": "object (optional, requires admin scope)",
  "systemProvenanceReceipt": "string (optional, requires admin scope)",
  "idempotencyKey": "string (NonEmptyString, REQUIRED)"
}
```

**Initial response shape (OC, ok: true) - returned synchronously:**
```json
{
  "runId": "string (equals idempotencyKey)",
  "status": "string ('started' | 'ok' | 'in_flight')",
  "ok": "boolean (optional, true for completed runs)",
  "aborted": "boolean (optional, if a /stop command was issued)",
  "runIds": "string[] (optional, aborted run IDs if /stop)",
  "messageId": "string (optional, transcript message ID for completed runs)"
}
```

**Streaming response (OC, after initial) - broadcast as `chat` events:**
The streamed agent response arrives as `chat` events broadcast via OC's `context.broadcast`. Each event payload includes a `state` field tracking the agent's progress through the run (`pending` → `final`, intermediate states for tool use, etc.). Subscribed clients receive these events on the WebSocket connection. The `chat` event payload schema is detailed in the §826 audit; the gateway's job is to forward each event through the SSE channel to Pane without inventing structure.

**Companion events emitted alongside `chat` events:**
- `session.message` events (transcript updates for the user's message and the agent's final reply).
- `session.tool` events (only if the client has the `TOOL_EVENTS` capability).

**Error codes:**
- `INVALID_REQUEST` - validation failed, unauthorized (admin-only fields used without admin scope), message/attachment required, send policy blocks, attachment parsing failed.
- `UNAVAILABLE` - media offload error, dispatch error.

**Gateway → Pane mapping (the streaming dance):** Pane never invokes `chat.send` directly. Pane invokes `POST /v1/sessions/:id/messages` (which is `sessions.send`, §22C.6.5.3, which delegates to `chat.send` inside OC). The streaming output reaches Pane via the SSE channel that Pane subscribed to before sending. Sequence:

1. Pane opens (or already has open) an SSE stream via `GET /v1/sessions/:id/events`.
2. Gateway, on opening that SSE stream, invokes OC's `sessions.messages.subscribe` (§22C.6.5.8) to subscribe to `chat`, `session.message`, and `session.tool` events for the session.
3. Pane sends the user message via `POST /v1/sessions/:id/messages`. Gateway invokes OC's `sessions.send` → OC delegates to `chat.send`.
4. OC returns the initial response synchronously (`{runId, status: "started"}`). Gateway forwards as the HTTP response to Pane.
5. OC streams `chat` events (and accompanying `session.message`/`session.tool` events) via `context.broadcast`.
6. Gateway receives events on its WS connection to OC, forwards them to Pane via the SSE channel as discrete events.
7. Pane reconciles: the synchronous response gives `runId` for correlating subsequent SSE events.

**The two-method dance (cross-cutting Rule 3 from §22C.6.5.0).** The SSE pattern is `chat.send` for emission + `sessions.messages.subscribe` for delivery, not a single bidirectional method. Gateway must orchestrate both: subscribe before send (or before send returns), then forward events as they arrive. Failure to subscribe before send means events arrive in OC but have no recipient; gateway loses the stream.

**Implementation notes:**
- **`idempotencyKey` is REQUIRED.** Per cross-cutting Rule 4. Gateway generates a UUID if Pane omits it, then preserves it across retries on the gateway → OC link so OC's deduplication works (returning `status: "in_flight"` if the same key is already active).
- **`status: "in_flight"` is a valid response, not an error** - it means OC already has this run in progress (idempotent retry). Gateway forwards `status: "in_flight"` to Pane; Pane treats it as success and listens for SSE events as normal.
- **Cached responses include `meta.cached: true`** in the response. Gateway forwards as-is; Pane can surface "(cached)" in UI if desired.
- **Stop commands** (`/stop`, `stop`, etc. in the message body) trigger an abort instead of a run, returning `aborted: true` and `runIds` listing the aborted run IDs. Gateway forwards.
- **Admin-only fields** (`originatingChannel`, `originatingTo`, `originatingAccountId`, `originatingThreadId`, `systemInputProvenance`, `systemProvenanceReceipt`) require admin scope on the bearer token. Pane's surface does not expose these; gateway never sets them on outbound `chat.send` calls.
- **Attachments** are parsed for images (if the model supports them) or offloaded to files. Pane's launch surface does not expose attachments; gateway treats as opaque pass-through if Pane includes them.
- **Send policy blocks** can prevent the send entirely. Surfaces as `INVALID_REQUEST`. Gateway maps to Pane's 4xx response with the OC error message.

##### §22C.6.5.8 `sessions.messages.subscribe`

> **OC capture:** OC v2026.4.11 (2026-05-03), `/opt/homebrew/lib/node_modules/openclaw/dist/server.impl-GQ72oJBa.js:12849-12870`. Schema source: `protocol-C6T5DFc8.js:1619` (SessionsMessagesSubscribeParamsSchema). Versioning: unstable; re-verify on OC version bumps.

**Required scope:** `operator.read` (inferred - not explicitly listed in `method-scopes-D3xbsVVt.js`)

**Request shape (OC):**
```json
{
  "key": "string (NonEmptyString, session key)"
}
```

**Response shape (OC, ok: true):**
```json
{
  "subscribed": "boolean",
  "key": "string (canonical session key, may differ from requested)"
}
```

**Error codes:** `INVALID_REQUEST` (validation failed, session not found)

**Events delivered after subscription:**
- `session.message` events - user and assistant messages (transcript updates).
- `session.tool` events - tool use events; only delivered if the client has the `TOOL_EVENTS` capability flag set.
- (Note: `chat` events are also delivered to subscribed clients during a `chat.send` run; this is how the streaming dance from §22C.6.5.7 works.)

**Gateway → Pane mapping:** Pane never invokes this method directly. The gateway invokes it as part of the streaming setup (§22C.6.5.7 step 2) when Pane opens an SSE stream via `GET /v1/sessions/:id/events`. The subscription is a side effect of the SSE connection lifecycle, not a Pane-callable operation.

Mapping:
- The route param `:id` from the SSE-open path → OC's `key` (per Rule 1).
- OC's `subscribed: true` is the gateway's signal that events will flow; on `subscribed: false`, gateway closes the SSE channel with an error.
- OC's returned `key` is the canonical session key; gateway uses it for all subsequent operations on this session.
- The companion method `sessions.messages.unsubscribe` is invoked by the gateway when the SSE channel closes (Pane disconnect, timeout, etc.). Spec'd here as the lifecycle pair though it isn't a separate Pane-facing method.

**Implementation notes:**
- **Subscription requires a WebSocket connection.** OC returns `subscribed: false` if no `connId` exists for the caller - i.e., no WebSocket session. The gateway holds the WS connection to OC; this is always satisfied for gateway-initiated subscribes.
- **Subscription scope is per-session.** Each subscribed session is independent; the gateway can hold many active subscriptions concurrently (one per Pane SSE stream).
- **Companion `sessions.messages.unsubscribe`** takes the same `{key}` shape and tears down the subscription. Gateway invokes on SSE channel close. If unsubscribe fails or times out, the events continue to broadcast but have no recipient - harmless, but worth logging at the gateway layer.
- **No event replay.** OC doesn't replay missed events on subscribe; the subscriber receives only events broadcast after the subscribe-ack arrives. This means the gateway's subscribe must precede the `chat.send` that triggers events. Race condition: if `chat.send` is issued before subscribe completes, early events are lost. The streaming-dance order in §22C.6.5.7 prevents this.
- **`TOOL_EVENTS` capability** is a per-client flag set during the OC connect handshake. The gateway should advertise this capability if Pane needs tool-use events surfaced; otherwise OC suppresses them. Stia confirms gateway advertises `TOOL_EVENTS` on connect.

### §22C.6.6 SSE event format

Stream: `text/event-stream`. Each event has the standard SSE shape:

```
id: <gateway-assigned monotonic sequence number, scoped per session>
event: <event_type>
data: <json>
```

Event types at launch:

- `message.delta` - assistant token chunk. Data: `{ message_id, content_delta }`.
- `message.complete` - final assistant message. Data: `{ message_id, content, finish_reason, usage }`.
- `tool.call` - model requested a tool. Data: `{ message_id, tool_call_id, tool_name, arguments }`.
- `tool.result` - tool response sent back. Data: `{ message_id, tool_call_id, result }`.
- `error` - generation error. Data: `{ message_id?, code, message }`.
- `replay.gap` - gateway buffer overflow during reconnect; client must re-fetch via `GET /v1/sessions/:session_id`. Data: `{ last_known_seq, current_seq }`.

Sequence numbers are per-session, monotonic, gateway-assigned. They are not OC sequence numbers; the gateway maintains its own.

### §22C.6.7 /v1/chat/completions — `X-OC-Resolved-Model` response header

The gateway proxies chat completions from Pane to OC via `POST /v1/chat/completions` (handler: `src-gateway/src/routes/chat.rs::chat_completions`). On every successful response (streaming and non-streaming), the gateway emits the `X-OC-Resolved-Model` HTTP response header carrying the OC-resolved model identifier for the request.

**Source of value:** The header value is `agent_registry.get_agents(&tenant.tenant_id, false)` resolved per request — preferring the agent with `oc_agent_id == "main"` (the OC default agent), falling back to the first available agent if no `"main"` agent exists. This is the same value the gateway uses for §4A.2.2 PARAM-COMPAT param-stripping; it is computed once per request and reused. Adding the header requires no additional `agent_registry` lookup.

**Header presence rules:**

- **Present on 2xx responses** (streaming and non-streaming) whenever `agent_registry` resolved a model for the request. The header value is the resolved model identifier (e.g., `anthropic/claude-opus-4-6`, `claude-opus-4-7`).
- **Absent on 2xx responses** when `agent_registry.get_agents()` errored, or when the resolved agent has no configured model (`model.is_none()`). The gateway logs the absence via the same `[PARAM_COMPAT]` log lines that today guard the strip step. Header absence on a successful response signals "OC resolved a chat dispatch but the gateway could not determine which model handled it" — Pane treats this as a no-op per §27.6.1d.
- **Absent on non-2xx responses.** Error responses (4xx, 5xx, including PayloadTooLarge, OcUnauthorized, OcServerError, OcTimeout) do not include the header. Pane's contract per §27.6.1d treats missing header as no-op.

**Casing.** Per HTTP semantics, header names are case-insensitive. The gateway emits `X-OC-Resolved-Model` in canonical title-case; Pane consumers MUST match case-insensitively (`response.headers().get("x-oc-resolved-model")` works regardless of emission casing).

**Versioning.** This header is a stable gateway contract as of the gateway version that ships with #1453's code fix. Pre-#1453 gateway versions do not emit it; Pane consumers per §27.6.1d treat absence as no-op (older gateway is indistinguishable from "no model resolved" from Pane's perspective — both correctly result in keeping the last-known cached value).

**Naming convention.** The `X-` prefix is retained for clarity (matches the existing `x-openclaw-model` request header on the Pane → gateway direction, and `x-request-id` on the middleware-emitted side). `X-` prefixes are deprecated in modern HTTP per RFC 6648 but remain common practice for gateway-specific extension headers; this contract follows the convention already established in the gateway's other custom-header surfaces.

**Cross-references:**

- Pane-side consumer contract: `docs/spec/smart-routing.md` §27.6.1d.
- Gateway code site: `src-gateway/src/routes/chat.rs::chat_completions` (the `if is_streaming` branch's `HeaderMap` construction; mirror in the non-streaming branch).
- Resolution mechanism: `agent_registry.get_agents()` per §22C.7 model discovery (in-memory cache after first call, sub-ms hot path).
- Issue: #1453.

**Status:** BUILT in #1457.

## §22C.7 Gateway ↔ OC WS surface

### §22C.7.1 Method whitelist

The gateway invokes exactly these nine OC WS methods at launch. Any other method call is a bug.

| OC method | Used by gateway endpoint | Notes |
|---|---|---|
| `models.list` | `GET /v1/models` | Cached 30s. |
| `agents.list` | `GET /v1/agents` | No caching - always live. |
| `sessions.create` | `POST /v1/sessions` | |
| `sessions.list` | `GET /v1/sessions` | Filtered to tenant-owned sessions. |
| `sessions.delete` | `DELETE /v1/sessions/:id` | |
| `sessions.send` | `POST /v1/sessions/:id/messages` | |
| `sessions.messages.subscribe` | SSE stream backing | One subscription per session; replay buffer in gateway. |
| `sessions.abort` | `POST /v1/sessions/:id/abort` | |
| `chat.history` | `GET /v1/sessions/:id/messages` | Paginated message retrieval. The only `chat.*` method invoked - `sessions.*` lacks a parallel history method. |

**Removed in v0.65.4:** `gateway.identity.get` (was mapped to `GET /v1/gateway-info`) - zero callers, OC returns device identity (deviceId/publicKey) not gateway metadata, conceptual mismatch. Future consumers spec the endpoint when they need it.

Anything else from the Tier 3-tagged set in Reed's inventory (`sessions.compact`, `sessions.steer`, `sessions.usage.*`, `sessions.preview`, `sessions.reset`, `sessions.patch`, `sessions.changed`, `sessions.subscribe` (broad), `agents.create/update/delete`, `agent.identity.get`, `agent.heartbeat`, `config.get`, `config.patch`, `logs.tail`, `status.request`, `tools.catalog`, `tools.effective`, `usage.cost`, `usage.status`) is **deferred** to a follow-up spec. The launch list is intentionally minimal.

### §22C.7.2 Connection lifecycle (recap)

Per §22C.4.2: persistent WS, 30s ping / 10s pong, exponential backoff reconnect, lazy-on-first-request open. Auth via OC bearer token in WS handshake.

### §22C.7.3 Subscription replay & gap-invisibility

**Goal:** subscription gaps caused by gateway↔OC reconnects are invisible to the Pane client. This is the load-bearing reliability invariant for streaming.

**Mechanism:**

1. When Pane subscribes to a session via SSE, the gateway issues `sessions.messages.subscribe` to OC and assigns a per-session monotonic sequence number to every event it forwards downstream.
2. The gateway maintains a per-session ring buffer of recent events: **last 1000 events or last 5 minutes, whichever fills first.** Bounded memory; bounded recovery window.
3. **Gateway↔OC disconnect.** In-flight events between OC and gateway during the disconnect window may be lost. On reconnect, gateway re-issues `sessions.messages.subscribe`. If gateway detects a sequence gap on OC's side (events arrive non-contiguously, or OC signals reset), gateway emits `replay.gap` to all SSE clients on that session, and the client refetches via `GET /v1/sessions/:id/messages?since=<last_known_message_id>` (paginating as needed).
4. **Pane↔gateway SSE disconnect.** Pane reconnects with `Last-Event-ID: <seq>`. Gateway replays from the buffer. If `<seq>` is older than the buffer's earliest event, gateway emits `replay.gap` and the client refetches via the same paginated messages endpoint.
5. **Buffer overflow during normal operation** (>1000 events without consumption): gateway drops oldest, emits `replay.gap` on next read.

**The two failure modes - gateway-OC gap (#3) and Pane-gateway gap (#4) - both terminate at the same recovery path: `replay.gap` event + paginated refetch via `GET /v1/sessions/:id/messages?since=...`.** From Pane's perspective, it never sees a partial stream; it either gets every event in order, or it gets a `replay.gap` and refetches deterministically. Subscription gaps are invisible (INVARIANT §22C.18).

Note: in-flight messages mid-stream (deltas before `message.complete`) are not recoverable as deltas - they re-stream once the subscription re-establishes, or the completed message becomes visible via the history endpoint once OC finalizes it. Pane UI treats a `replay.gap` as "discard in-flight delta state for affected messages, refetch finalized state."

**Spike #1 finding (2026-04-28):** OC does **not** buffer or replay subscription events. Subscriptions are cleaned up immediately on WS close. Events use `dropIfSlow: true` - dropped if socket buffer exceeds 50MB. No configurable replay window, no event history. **Gaps are frequent, not rare.** Every gateway↔OC disconnect = missed events. The ring buffer is the load-bearing safety net, not a nice-to-have. OC provides zero recovery assistance.

**Sizing consideration:** The 1000-event / 5-minute ring buffer sizing should be validated under high-event-rate sessions (e.g., streaming tool-call-heavy responses). Flagged for Track 4 test design (§22C.15) - do not redesign now, but stress-test the buffer under realistic load and adjust if needed.

Full spike report: `docs/spikes/2026-04-28-spike1-oc-subscribe-buffering.md`.

## §22C.8 Sync semantics (port forward from §22B.3)

Restatements of bridge semantics that carry forward. The kickoff requires they survive verbatim.

### §22C.8.1 Push, pull, confirm, sync-all

Four operations, defined in §22C.6.2. Behavioral semantics:

- **Push** is event-driven on the Pane side. Every successful write to `.pane/system/{agent_uuid}/<whitelisted-file>` triggers a push. No polling. Push is non-blocking from the user's perspective; failures queue locally (§22C.8.3).
- **Pull** runs at Pane startup and after any successful gateway reconnect. Retrieves all `sync_pending` entries for active agents on the tenant. Pane reconciles each entry against local file state via timestamp comparison (§22C.8.2), then confirms processed entries.
- **Confirm** clears entries from the gateway's `sync_pending` queue. Two-phase (pull + confirm) ensures Pane crash mid-reconcile doesn't lose data.
- **Sync-all** is the bulk fetch used during onboarding and after token re-pair. Returns the full current state of all agent identity files for the tenant.

**Canonical keys.** All four operations target the same gateway, identified by the same `ai_settings` keys the chat layer uses (§2.3C): `endpoint` (gateway URL) and `api_key` (gateway bearer token), with `gateway_tls_fingerprint` for cert pinning. When `provider = openclaw`, the chat and sync layers share auth state — they are not separate concerns at the key-storage level. The sync layer does not read `pane_gateway_url` or `pane_gateway_token` (those keys were removed per #1167).

#### §22C.8.1a Stale `pane_gateway_token` / `pane_gateway_url` row cleanup (#1499)
> **Status:** BUILT — v65 schema migration, `migrations.rs:migrate_pane_gateway_rows()`, PR fix/1499-stale-gateway-token.

#1167 (PR #1168/#1169) stopped all readers and writers from touching `pane_gateway_url` / `pane_gateway_token`, but did not clear any `ai_settings` rows that pre-existing installs had already written under those keys. Such a row is orphaned — present in the table, read by nothing. A one-time migration clears it. (At the time of authoring there are no live installs carrying these rows; #1499 closes the gap before any install can hit it, rather than repairing a live fleet.)

**Migration rule.**

- **`pane_gateway_token` → `api_key`, copy-only-if-absent.** If a `pane_gateway_token` row exists, copy its value into `api_key` ONLY when `api_key` is absent or empty. NEVER overwrite a present, non-empty `api_key`. `api_key` is authoritative post-#1167: an install that re-paired after the #1167 collapse holds the current token in `api_key`, and a lingering `pane_gateway_token` is necessarily the older value. On divergence, `api_key` wins. After the copy decision (whether or not it copied), delete the `pane_gateway_token` row.
- **`pane_gateway_url` → delete the row.** This key was only ever written as an empty string (per the #1167 root-cause: writers populated `pane_gateway_token` but never the URL). There is no value to preserve, so it is deleted, not migrated. The canonical gateway URL lives in `endpoint`.

The migration is idempotent: once the stale rows are gone, the pass is a no-op (no rows to act on).

**Sequencing invariant (#1499 before #1008).** This cleanup MUST run before #1008's credential-to-keychain migration. #1008 migrates `api_key` (the gateway token, when `provider = openclaw`) from plaintext `ai_settings` into the OS keychain. If #1008 ran first, it would migrate `api_key` to the keychain while a stale `pane_gateway_token` row still sat in `ai_settings` unmigrated — and the "copy-if-`api_key`-absent" rule above would then misfire (it would see the keychain-migrated `api_key` as absent from `ai_settings` and wrongly copy the stale token over it). Running #1499 first means the stale rows are resolved into `api_key` (or deleted) while `api_key` is still the plaintext authoritative value, before #1008 moves it to the keychain. #1008 inherits this ordering: its migration assumes `ai_settings` carries no stale gateway-token rows.

> **Considered and dropped: clearing the `keychain_migration_v1` flag.** An earlier analysis (Reed's source-read) proposed that #1499 clear the `keychain_migration_v1` flag, on the theory that #1008's credential migration would already have set it and #1499's late row-copy would need #1008 to re-run. That flag-clear is NOT needed and is deliberately omitted. It only mattered if #1008 had already shipped and set the flag before #1499 ran — but #1008 is open, and the sequencing invariant above puts #1499 first. With #1499 before #1008, the flag is never set at the moment the stale token is still unmigrated, so there is nothing to clear. Do not re-add the flag-clear: it solves a problem the sequencing eliminates, and re-adding it would couple #1499 to #1008's flag semantics for no benefit.

> **Gateway token storage (ADR-0044).** When `provider = openclaw`, the gateway bearer token is stored in the OS keychain (account `ai_gateway_token`, service `com.threelane.pane`), NOT in plaintext `ai_settings.api_key` and NEVER in `config.json`. `get_gateway_token()` reads the keychain. This splits the credential out of the shared `api_key` field that the #1167 collapse created — the gateway token and the primary provider key now occupy distinct keychain accounts, so no provider-check is needed to interpret a fetched credential's meaning. Post-migration, a keychain miss for `ai_gateway_token` is "gateway not configured" (no plaintext fallback); the cascade-flow re-pairing path (§22C.11A.5) re-populates it. The wipe semantics in §22C.11A.4 still apply — a cascade that changes the gateway target clears the keychain token alongside the other gateway-state keys.

### §22C.8.2 Conflict resolution

**Timestamp-based LWW with Pane tie-break.** Honors the Pane-PRIMARY / OC-REPLICA model (ADR-0012, `docs/spec/README.md` §0.3.3):

1. On pull, for each pending change, compare local file `modified_at` vs remote `queued_at`.
2. Newer timestamp wins.
3. Ties resolve to Pane (deterministic; avoids ping-pong; Pane is PRIMARY per ADR-0012 and §0.3.3, so the local file is the canonical version on tie). See ADR-0042 for the decision lineage.
4. Content-identity check: if local content == remote content after the timestamp says "ingest," skip the write. Avoids gratuitous filesystem churn and FTS5 invalidation.

Logic ports cleanly from `sync_client.rs:661-681`. **Tie-break direction was inverted in #1299 (ADR-0042):** the original spec inherited "gateway wins on tie" from the bridge plugin without a decision artifact (commit `57dab52da`); the corrected rule resolves ties to Pane, consistent with the Primary-Replica model. All other LWW logic ports unchanged.

### §22C.8.3 Queue invariants (carry forward verbatim from §22B.3.7)

**Status:** BUILT (PR #752 - atomic write + read-back checksum verification on gateway side).

These become INVARIANTS in §22C.18.

- `next_retry_at` stores RFC3339 timestamps via `chrono::Utc::now().to_rfc3339()`.
- All queries filtering by `next_retry_at` use bound RFC3339 parameters. **Never `datetime('now')`.** Lexicographic incompatibility was caught in PR #645; this stays locked.
- Maximum 5 retries per item. Exponential backoff: 2^n seconds, capped at 32s.
- Dropped items log `[SYNC] Dropping {agent}/{filename} after {n} retries` and are deleted from queue.
- Queue depth: max 100 items per flush cycle. Carries from current `LIMIT 100` in `flush_sync_queue`.
- Atomic write semantics on disk: temp-file + rename. Write verification (read-back checksum) is mandatory on the gateway side.
- Retry, flush, and pull paths read the same `ai_settings` keys as push (§22C.8.1): `endpoint` and `api_key`. No separate retry-keys path; no per-operation drawer.

### §22C.8.4 Pane-side queue + gateway-side queue

**Both.** Pane has a `sync_queue` for outbound pushes that failed (network down, gateway unreachable). Gateway has a `sync_pending` for OC-originated changes waiting for Pane to reconnect. The two queues are independent - a push failure does not interact with pending OC writes.

This matches current bridge architecture and carries forward.

### §22C.8.6 Ingest writes and the ping-pong suppression invariant
> **Status:** BUILT (v0.65.71, #1298)
> **Added:** 2026-05-31

**The ping-pong problem (fixed in #1298):** Prior to v0.65.71, `atomic_write` unconditionally called `trigger_sync` whenever `db_conn` was `Some` and the path was a system file. Both ingest call sites (`pull_pending_changes` and `initial_sync`) passed `Some(conn)` — so every pulled file immediately re-queued for push back to the gateway. 224 files pulled = 224 re-push attempts.

**The fix:** `atomic_write` now requires an explicit `WriteOrigin` parameter (see `docs/spec/file-system.md` §2.3A). Ingest call sites pass `WriteOrigin::Remote`, which suppresses `trigger_sync`. Local/AI write sites pass `WriteOrigin::Local`, which preserves the push-on-write behavior.

**`flush_sync_queue` is the sole canonical write-back mechanism after ingest.** After the ingest loop in `pull_pending_changes` and `initial_sync`, there is an explicit call to `flush_sync_queue` / `flush_sync_queue_blocking`. This flush pushes any items already in the queue from *local* writes (unrelated to the current ingest). The ingested-from-gateway files are NOT re-queued — no ping-pong.

**Rule:** Any future ingest or recovery flow that writes files to disk MUST use `WriteOrigin::Remote`. Any future local/AI write path MUST use `WriteOrigin::Local`. The compiler enforces this — `WriteOrigin` has no `Default` impl.

### §22C.8.5 Sync allowlist — scope, contents, and three-surface enforcement
> **Status:** Pane ingest filter BUILT (v0.66.82, PR #1372 — `is_allowed_sync_file` added to `pull_pending_changes_blocking`; stale MISSING marker corrected in PR #1300). Gateway read filter still NOT BUILT (tracked in #1278).
> **Added:** 2026-05-28 — formalizes the allowlist that has existed on the push surface since v0.18; extends enforcement requirement to gateway read and Pane ingest (ADR-0041)

**Governing principle (ADR-0041):** The set of files that sync between Pane and OC is a filtered subset of the set of files that Pane persists locally. Sync scope ⊊ persistence scope. A file may legitimately exist in Pane's local storage and never cross the wire; that exclusion is correct, not a desync.

#### Allowlist contents

The sync allowlist (`IDENTITY_WHITELIST`, `sync_client.rs:75-83`) defines the canonical set of files that cross the Pane↔OC wire. The allowlist is scoped by agent role:

**Main agent (strategy agent):**
- `SOUL.md` — required
- `MEMORY.md` — required
- `IDENTITY.md` — optional
- `AGENTS.md` — optional
- `USER.md` — optional
- `TOOLS.md` — optional
- `HEARTBEAT.md` — optional (main agent only)
- `BOOTSTRAP.md` — optional (main agent only)
- `RULES.md` — Pane-only (BYOK path; not an OC concept). Sync if present; never required. Main agent only.
- `memory/*.md` — flat only (depth 1); main agent only. See depth rule below.
- `logs/*.md` — flat only (depth 1); main agent only. Pane-originated.

**Sub-agents:**
- `SOUL.md` — required
- `MEMORY.md` — required
- `IDENTITY.md` — optional
- `AGENTS.md` — optional
- `USER.md` — optional
- `TOOLS.md` — optional

This is OC's `MINIMAL_BOOTSTRAP_ALLOWLIST` (SOUL, IDENTITY, AGENTS, USER, TOOLS) plus MEMORY.md. Sub-agents do **not** sync `memory/`, `logs/`, `HEARTBEAT.md`, `BOOTSTRAP.md`, or `RULES.md`.

All other files in the OC workspace (`archive/`, `scratchpad-*.md`, temp files, backup archives, `memory/` subdirectories at depth ≥ 2, etc.) do **not** cross the wire. Their presence in the OC workspace is not a sync concern.

#### Depth-1 rule for `memory/` and `logs/`

`memory/*.md` and `logs/*.md` are flat-only (depth 1) **by contract, not by convenience.** OC's dreaming/consolidation reader reads `memory/*.md` via `entry.isFile()` with no recursion. Syncing `memory/subdir/note.md` (depth 2) would put a file on the wire that OC never reads — dead weight plus a false user expectation that nested memory organization is visible to OC. The depth-1 rule is alignment with OC's reader contract and **must not be relaxed to recursive** without a corresponding change to OC's reader (cross-reference: ADR-0041 §Decision 6).

#### Three-surface enforcement

The allowlist is enforced on all three sync surfaces. Defense-in-depth: a file blocked at any one surface is blocked; all three enforce so that a version-skewed component during rolling upgrade cannot leak non-allowlisted files.

| Surface | Location | Status |
|---------|----------|--------|
| **Push** | `trigger_sync` via `extract_agent_and_filename` (`sync_client.rs:117-155`) | ✅ EXISTS — `IDENTITY_WHITELIST` (`sync_client.rs:75-83`) |
| **Gateway read** | `read_workspace_files` (`src-gateway/src/routes/sync.rs:282-336`) | ❌ MISSING — currently walks all non-dotfiles recursively |
| **Pane ingest** | `pull_pending_changes_blocking` | ✅ BUILT (v0.66.82, PR #1372) — `is_allowed_sync_file` guard at ingest; path-traversal enforcement via `file_ops::safe_path_join` added in PR #1300 |

Implementation note: Stia should extend `IDENTITY_WHITELIST`'s enforcement to the two missing surfaces rather than creating independent filter literals. If the gateway crate cannot import the same constant as `sync_client`, define the allowlist in a shared location both crates import (filed as a follow-up per ADR-0041).

#### Why ingest filtering is safe (no desync risk)

Filtering at `pull_pending_changes` ingest is clean because ingest is flat-only: system notes are `body=NULL` on creation with the DB row populated on first edit. At ingest time, no DB row is written for system files — only the flat file would be written. Filtering a received file at ingest therefore writes nothing (no flat file, no DB row). Since Pane is PRIMARY and OC is REPLICA (§0.3.3), a file Pane does not ingest is correctly absent from Pane's authoritative state. See ADR-0041 §Decision 3–4 for the full reasoning.

#### Known limitation: already-synced junk not cleaned

The steady-state filter stops new non-allowlisted files from crossing the wire but does not remove files that prior unfiltered syncs already wrote to disk. A cleanup migration is not built (pre-launch; affected population is test installs that get nuked routinely). Filed post-launch if a long-lived dogfood install accumulates enough junk to matter.

#### Dual-write clarification: `memory/` files written by both Pane and OC

Both Pane and OC write to `memory/*.md` (and Pane writes `logs/*.md`). These are not Pane-only or OC-only files. Sync for these files follows the standard LWW conflict-resolution path (§22C.8.2) — no special-case handling. The allowlist inclusion of `memory/*.md` and `logs/*.md` reflects that they are legitimately bidirectional and belong on the wire; their presence on the allowlist does not imply Pane ownership or OC ownership exclusively.

## §22C.9 Chat / session semantics

### §22C.9.1 Session creation

Pane creates a session via `POST /v1/sessions` with an `oc_agent_id`. Gateway calls `sessions.create` on OC, stores the mapping `(tenant_id, gateway_session_id) → oc_session_id`. Returns the gateway session ID to Pane.

Pane never sees the OC session ID. The gateway abstracts it. This isolates Pane from OC session-ID renaming, OC-side cleanup, and future Tier 1 routing where the same Pane session could be backed by different OC sessions over time.

### §22C.9.2 Sending a message

Two-step UX, single-step from the API perspective:

1. Pane calls `POST /v1/sessions/:id/messages` with the user's message + `idempotency_key`. Gateway calls `sessions.send` on OC. Returns `message_id, accepted_at`. Non-streaming.
2. Pane is already subscribed to the SSE stream (`GET /v1/sessions/:id/messages/stream`). Assistant response arrives as `message.delta` events, terminated by `message.complete`.

If Pane is not subscribed when a message is sent, the gateway buffers events in the per-session ring buffer. On subscription open, gateway replays buffered events. This handles "Pane sends a message, then briefly loses connection, then reconnects" without losing the response.

### §22C.9.3 Aborting

`POST /v1/sessions/:id/abort` is best-effort. Gateway calls `sessions.abort` on OC. The SSE stream emits an `error` event with `code: "aborted"` once OC confirms.

### §22C.9.4 Tool calls

Tool execution happens in OC. The gateway forwards `tool.call` and `tool.result` events through the SSE stream. Pane displays tool activity to the user but does not execute tools - tool execution is OC-side, regardless of gateway involvement.

This matches current Tier 3 behavior; the gateway is a pure relay for tool events.

## §22C.10 Agent registry

The gateway has a `tenant_agents` table mapping `(tenant_id, oc_agent_id) → { name, role, workspace_path, registered_at }`. Populated lazily on first `/v1/agents` call (gateway calls `agents.list` on OC and caches). Refreshed on demand or when sync detects an unknown `oc_agent_id`.

**Source of truth remains OC's `openclaw.json`.** The gateway does not author agent identity; it caches the routing map.

**Auto-registration is out of scope for §22C.** Per Reed's same-machine inventory (2026-04-27), no code in Pane, the bridge plugin, or OC core writes to `openclaw.json` on agent creation today. Agents created in Pane appear in OC's view only if the user manually edits `openclaw.json` or uses an OC CLI command. This is a pre-existing gap, not a gateway concern, and the gateway does not introduce auto-registration to fix it. If addressed later, candidate owners include the gateway (filesystem watcher + writes to `openclaw.json` from the OC machine), Pane core (writes via OC's CLI on agent creation), or a separate helper. That decision belongs to a separate issue scoped to agent lifecycle, not to this spec. (See §22C.2, §22C.17.)

**`oc_agent_id` is the stable routing key per the existing bridge migration (#364).** Carries forward verbatim. Pane never uses agent names for routing; the gateway never uses agent names for routing.

**Agent registration (creation/update/deletion) at launch:** out of scope per §22C.2. The gateway exposes `agents.list` (read-only) only. `agents.create/update/delete` calls to OC are not made by the gateway.

**ADR-0023 capture — `agents.list`, `isDefault` field (2026-09-21).**

> **OC capture:** OC v2026.8.2 (2026-09-21), live gateway probe + `openclaw agents list`. Versioning: unstable; re-verify on OC version bumps.

Five agents observed, each carrying a per-agent `isDefault` boolean; all five report `isDefault: false`. No top-level `defaultId` field is present in the response — OC does not advertise a single global default in this shape. Gateway and Pane code resolve default-agent status per §22C.6.2 "Default agent resolution" (ADR-0067 D5) rather than reading a `defaultId` OC does not send; a fixture asserting a `defaultId` field is stale against this capture.

## §22C.11 Same-machine vs split-machine routing

> **Status: REMOVED (PR #NNNN, #2649).** OpenClaw is split-machine only. Same-machine
> was never an allowed topology and is a security risk — running OC on the same
> machine as Pane violates the isolation boundary that makes split-machine
> deployments secure. No live install carries `same_machine` state. Section body
> removed; retained as a stub for git-blame context. See PR #NNNN.

### §22C.11.4 Bridge plugin retirement

The bridge plugin **fully retires** post-cutover.

- Split-machine moves to the gateway end-to-end.
- The bridge plugin can sit unused in the OC extensions directory without harming anything; older Pane versions (pre-gateway) continue to function until they upgrade. Once everyone has upgraded, the npm package can be archived.
- No migration tooling is needed for users - the upgrade itself is the migration. Pane stops calling bridge endpoints; bridge plugin stops being relevant.

## §22C.11A Settings UI - Topology-Aware Display

**Status:** BUILT (v0.65.5, PR #885; exercise fixes PR #889 - /v1 URL dedup, model persistence, fingerprint load; cascade-UX v2 PR #891 - auth fingerprint fetch, universal save button; cascade-UX v3 PR #894 - banner refire prevention, model loading on all provider switches, searchable model dropdown; field name alignment PR #1149 - Settings.svelte `result.fingerprint` → `result.tls_fingerprint`)

The Settings → AI tab adapts its OpenClaw configuration fields based on the detected topology.

### §22C.11A.1 Field visibility rules

| Field | Visibility / Behavior |
|-------|-----------------------|
| AI Provider selector | Shown |
| Gateway URL | Shown (editable) |
| Gateway Token | Shown (masked, obtained via inline pairing flow - not directly editable, use "Re-pair" action; see §22C.11A.5) |
| TLS Fingerprint | Shown (truncated monospace with Copy button, obtained during pairing from `/v1/health` → `tls_fingerprint`) |
| Connection status | Shown (green/yellow/red dot + text) |

**Token source instructions (#884).** Inline help text under relevant fields:
- Pairing code: "Find this in the terminal where Pane Gateway is running. If expired, run `pane-gateway pairing-code` on the OC machine for a fresh code."
- TLS fingerprint: "Verify this matches the fingerprint shown when your gateway started. Check the terminal on the OC machine."

**Model and Agent rows removed from Settings AI tab (#882, #883).** Both are OC-side state shown on the Agents page. The Settings AI tab focuses on connection config (URL, token, fingerprint, status), not what's running over the connection. The underlying model value still populates from OC for chat routing - it's just not displayed in Settings.

Topology changes happen as a side effect of editing provider, gateway URL, or related fields and saving - see §22C.11A.3. There is no dedicated "Switch topology" link.

### §22C.11A.2 Error states per topology

| Topology | Error | Message |
|----------|-------|---------|
| `split_machine` | Can't reach gateway | "Unable to connect to gateway at {url}. Check the URL and that the gateway is running." |
| `split_machine` | Auth failure | "Gateway rejected the token. Re-pair or check the token." |
| `split_machine` | No agents returned | "Gateway returned no agents. Check the gateway's OpenClaw connection." |

### §22C.11A.3 Cascade-change flow

> **Status:** BUILT (v0.65.5, PR #873)

When the user edits AI configuration fields and saves, Settings detects whether the new config requires more than a value update - provider switch, OC URL host-class change, or split-machine gateway swap all carry downstream state that must be cleared, re-inferred, or re-paired. The cascade runs in the backend on save; the user's interaction stays in the AI tab.

**Detection (on Settings save).** Settings diffs the proposed config against the saved config. The cascade fires if any of the following triggers match:

| Trigger | Condition | Downstream effect |
|---|---|---|
| Provider change | `provider` field differs from saved value | Wipe provider-side keys (§22C.11A.4); re-infer topology if new provider is OpenClaw |
| OC URL host-class flip | Provider was/is OpenClaw, and host portion of `endpoint` crossed local/remote boundary (`localhost`/`127.0.0.1`/`::1` ↔ any other host) | Re-infer topology; wipe pairing state if topology was split-machine; run inline pairing if new topology is split-machine (§22C.11A.5) |
| Split-machine gateway swap | Provider unchanged (still OpenClaw), topology unchanged (still split-machine), but `endpoint` host portion changed to a different remote host | Wipe pairing state for old gateway; run inline pairing for new gateway (§22C.11A.5) |
| Token rotation | Provider unchanged, URL unchanged, only `api_key` changed | No cascade - token-only swap. Test connection re-runs to confirm. |

If no trigger matches (e.g., user changed only the model selector), the save is an ordinary value update with no cascade.

**Confirmation rule.** The cascade runs silently when no paired state is being discarded. When the diff would discard paired state - pairing token, TLS fingerprint, or gateway-side cached state - Settings shows an inline warning banner above the Save/Cancel buttons:

| Trigger | Banner text |
|---|---|
| OC → non-OC provider | "Switching to {new provider} will clear your gateway pairing for {hostname}. You'll need to re-pair if you switch back." |
| OC URL host-class flip (remote → local) | "Switching to a local OpenClaw will clear your gateway pairing for {old hostname}. Same-machine setups don't pair." |
| OC URL host-class flip (local → remote) OR split-machine gateway swap | "Switching the gateway to {new hostname} will clear your pairing for {old hostname}. You'll re-pair on save." |

The banner is inline UI within the AI tab, not a modal dialog. User clicks Save to proceed or Cancel to revert the field edits. No modal pop-ups; no wizard navigation.

**No banner cases.** Three cascade-eligible saves run without confirmation:

1. Non-OC ↔ non-OC provider switch (OpenRouter ↔ Anthropic ↔ Ollama). No pairing state exists; the wipe is just provider-side keys (API key, endpoint, model). Save runs straight through.
2. First-time OC config from a non-OC starting state (e.g., OpenRouter → OpenClaw). The user has no prior OC pairing to discard. The inline pairing flow (§22C.11A.5) is the affordance for the new state - pairing-flow itself is the user's active confirmation.
3. Local → different local URL change (e.g., port number on `127.0.0.1`). Topology unchanged, no pairing involved.

**Cascade execution order (on confirmed save).**

1. **Wipe stale keys.** Clear the keys named in §22C.11A.4 according to the trigger. Wipe scope is AI configuration only - user data is never touched (§22C.11A.4 lists the explicit boundary).
2. **Set topology.** If the new `endpoint` is set (gateway URL when `provider = openclaw`), write `tier3_topology = split_machine` to `ai_settings` — OpenClaw is split-machine only (#2649); there is no host-based classification. If new provider is non-OC, `tier3_topology` is also wiped (Tier 2 has no topology).
3. **Update field visibility.** Settings re-renders the AI tab per §22C.11A.1 against the new topology. Hidden fields disappear; newly-visible fields appear with their saved or default values.
4. **Run inline pairing if needed.** If new topology is `split_machine` and pairing state was just wiped (or never existed for this gateway), the inline pairing flow runs (§22C.11A.5). The user stays in Settings. If pairing fails, the connection is marked failed and the user can retry from the same panel.
5. **Test connection.** Final step - verify the new config works end-to-end: model fetch for non-OC, agents fetch + gateway-info call for OC. Result populates the gateway-status row in §22C.11A.1.

**Cascade is atomic from the user's perspective.** The user clicks Save once; the entire flow (wipe → infer → re-render → pair if needed → test) runs as one operation. If any step fails, Settings surfaces the specific error per §22C.11A.6 and the AI tab stays in a recoverable state - partial saves are visible (the new provider is set, the wipe ran), but the user can edit further fields and retry. There is no rollback to the prior config; the user owns the decision to revert.

**INVARIANT preserved.** "Pane checks `ai_settings.tier3_topology` at startup and routes statically" (§22C.18, ADR-0012). The cascade is a discrete save-time event; the next app launch reads the new topology via the same path as any other setting. No runtime mode flip.

### §22C.11A.4 Wipe scope

> **Status:** BUILT (v0.65.5, PR #873)

When the cascade flow fires per §22C.11A.3, it clears specific `ai_settings` keys based on which trigger matched. The boundary is **AI configuration only** - user data is never touched.

**Keys wiped per trigger.**

When `provider = openclaw`, the gateway URL is stored in `endpoint` and the gateway bearer token is stored in `api_key` — the canonical generic-provider keys defined in `docs/spec/file-system.md` §2.3C. The sync layer reads the same keys (§22C.8.1). `gateway_tls_fingerprint` is gateway-pairing-specific and persists as its own key alongside the cert PEM in `gateway_tls_cert_pem`.

| Key | Provider change | OC URL host-class flip | Split-machine gateway swap | Token rotation only |
|---|---|---|---|---|
| `provider` | Updated | - | - | - |
| `api_key` | Wiped if leaving OC / Set on pair if entering OC / Refilled by user for non-OC providers | Wiped + re-paired | Wiped + re-paired | Updated |
| `endpoint` | Wiped if leaving OC / Updated if entering OC / Refilled by user for non-OC providers | Updated | Updated | - |
| `model` | Wiped (re-selected after new connection) | - | - | - |
| `simple_tier_model` | Wiped if new provider differs from prior internal-provider expectations | - | - | - |
| `router_model` | Same as `simple_tier_model` | - | - | - |
| `memory_search_llm_rerank_model` | Same as `simple_tier_model` | - | - | - |
| `gateway_tls_fingerprint` | Wiped | Wiped | Wiped | - |
| `gateway_tls_cert_pem` | Wiped | Wiped | Wiped | - |
| `tier3_topology` | Wiped (if leaving OC) / Re-inferred (if entering OC) | Re-inferred | - (still split_machine) | - |
| `internal_provider` | Reviewed - wipe only if no longer compatible | - | - | - |
| `internal_api_key` | Same as `internal_provider` | - | - | - |
| `internal_endpoint` | Same as `internal_provider` | - | - | - |
| `last_sync_push` | Wiped (refers to old gateway) | Wiped | Wiped | - |

**Drawer collapse (2026-05-17, #1167).** Earlier drafts of this table listed `pane_gateway_url` and `pane_gateway_token` as separate keys from `endpoint` and `api_key`. Those keys were scaffolding from the cascade-change UX session that did not survive architectural review: per ADR-0012 / ADR-0015, the gateway is the chat-layer endpoint when `provider = openclaw`, and the chat and sync layers share the same auth target. The `pane_gateway_*` keys are deleted; `endpoint` / `api_key` are the canonical pair. `gateway_tls_cert_pem` is added explicitly because it persists alongside the fingerprint and follows the same wipe semantics — present in the live cascade-flow code (§22C.11A.5) but missing from the prior wipe table.

Stia confirms the exact key list against `database.rs::seed_defaults()` and any settings written by `ai_configure_and_save()` or topology-change handlers before landing. Additions OK; deletions need spec session re-review. Any gateway-state cache keys discovered during implementation (cached agent registry, cached models, etc.) follow the same wipe rule as `last_sync_push` - they refer to the old gateway and are wiped on any cascade that changes the gateway target.

**What stays untouched.** The cascade never wipes user data:

- `notes` table (all user-authored notes)
- `projects` table (project structure)
- `ai_agents` table (agent identity records)
- `ai_memory` table (extracted facts)
- Sync history (`sync_queue`, `sync_pending` if present)
- Daily logs (`logs/*.md` on disk, `notes` rows for daily entries)
- Agent identity files (`.pane/system/{agent_uuid}/SOUL.md`, `MEMORY.md`, etc.)
- Onboarding completion flag (`onboarding_completed`)
- Theme, vault location, categories, and other General-tab settings

The user's content survives every cascade. The cascade only resets the connection layer.

**`internal_provider` review rule.** The internal provider (smart routing / rerank, §29) is logically independent of the primary provider but practically often co-configured. The cascade wipes `internal_provider` / `internal_api_key` / `internal_endpoint` only when the new primary provider's config invalidates the existing internal setup - for example, switching from OpenClaw (which often pairs with OpenRouter as internal) to Ollama (which has no internal-provider pattern) wipes; switching from Anthropic to OpenRouter does not. Stia implements the review rule conservatively - when in doubt, prompt the user via §22C.11A.6's banner text rather than wiping silently.

### §22C.11A.5 Inline pairing flow

> **Status:** BUILT (v0.65.5, PR #873)

> **Amended (2026-08-22, #2642/#2630 — architecture reversal).** The pairing ceremony described below no longer lives in `ProviderConnectionCard.svelte`'s `completePairing()`. It now lives in `NewProviderModal.svelte`, which owns the ceremony end-to-end (URL entry → fingerprint accept → pairing code → import → connection test) for both first-time OC connections and re-pair. `ProviderConnectionCard.svelte` is manage-only (disconnect, test connection, sync, status display) — its "Pair gateway"/"Re-pair" buttons call an `onPairRequested('pair' | 're-pair')` prop instead of running any ceremony steps in-card. `ProviderConnectionsPanel.svelte` wires that callback to a `startCeremony` deep-link prop on `NewProviderModal`, opening the modal directly at the ceremony's URL step (skipping the provider-picker screen) with the URL prefilled from the existing endpoint on re-pair. `pairSyncError` remains a prop the panel threads externally into the card (via the modal's `onConnectionSaved(lastImportError)` callback) so the collapsed-card "⚠️ Import failed" badge (§22C, D13) still works even though the ceremony that can produce that error moved out of the card. Every invariant in this subsection (first-pair/re-pair routing, `oc_agent_row_exists`, D18/D20/D21 rules) is unchanged in behavior — only which component's markup executes it moved.

When the cascade results in a new split-machine OC connection that needs pairing - first-time OC config, OC URL host-class flip to remote, or split-machine gateway swap - the pairing flow runs **inside the AI tab**, not via the onboarding wizard. The user does not leave Settings.

**Pairing UX placement.** The Gateway URL and Gateway Token rows in §22C.11A.1 expand below their input fields to show the pairing flow inline. The expansion contains, in order:

1. **TLS fingerprint display.** The first connection to the new gateway returns its self-signed cert fingerprint (§22C.4.3). Settings displays the fingerprint in a read-only block with a clear "Accept" button. Until accepted, no token is stored.

2. **Pairing token entry.** The user pastes the pairing code (8-digit, 5-min expiry per §22C.5.1) generated by the gateway on its host machine. Settings calls the pairing exchange (`POST /pair`) and on success stores the resulting opaque bearer token to `api_key` (the canonical key per §22C.11A.4 and §2.3C) — both the persistent DB setting and the in-memory runtime provider state, so that subsequent requests (model fetch, chat streaming) use the fresh token immediately without requiring an app restart.

> **SECURITY INVARIANT (2026-08-25, #2717).** The user-entered gateway origin is the sole destination throughout fingerprint fetch, pairing-code exchange, persistence, and initial/re-pair sync. Pane canonicalizes it at the native boundary, requires HTTPS, rejects credentials/query/fragment/unexpected paths, and disables redirects. Private, loopback, LAN, and Tailscale hosts remain intentionally supported. Data returned by the unauthenticated health endpoint—including any advertised hostname—MUST NOT replace or redirect the reviewed origin. Health and pairing response bodies are bounded. Before either onboarding or New Provider accepts a pairing code, the UI MUST disclose that pairing enables automatic two-way synchronization: the gateway receives supported agent workspace context (including identity, instructions, memory, tools, and logs) and may update local allowlisted agent files.

3. **Initial agent import (`sync_initial_import`).** Once the token is stored, Settings calls `sync_initial_import(gateway_url, token)` (the same command §24.5.1 documents for onboarding) to pull the gateway's agent roster. This step is not cosmetic - it is **the only code path that INSERTs an `ai_agents` row with `runtime_type = 'openclaw'`.** `ai_configure_and_save`'s `openclaw` branch (which ran in step 2, alongside the `api_key` write) only UPDATEs an existing `runtime_type = 'openclaw'` row keyed on `runtime_ref = 'main'` - it deliberately does not INSERT one (INSERT capability is out of scope per the #2635 fix; see the note below). Skipping this step leaves zero `ai_agents` rows for a brand-new pairing, which silently breaks sync: `sync_client::has_active_openclaw_agent()` (and therefore `get_gateway_url()`) gate on that row's existence, so a customer who paired successfully (token/endpoint/TLS cert all correctly stored) would still see sync never engage. A `sync_initial_import` failure at this step does not undo the pairing above (the token/endpoint/cert are already durably stored) - it surfaces as a distinct, non-blocking warning, with "Sync now" (§14) as the retry affordance once the gateway is reachable again.

4. **Connection test.** Once the token is stored, Settings runs the standard test-connection flow (§22C.6.4 `agents.list` + §22C.6.1 `gateway.identity.get` ping). Result populates the gateway-status row.

The four steps run sequentially in the same panel. The user does not navigate between screens.

> **INVARIANT (added 2026-08-21, #2635 / D11 fix).** Every OpenClaw pairing ceremony - onboarding's (§6.5.3, §24.5.1) and Settings'/New-Provider's (this section) alike - MUST call `sync_initial_import` after `ai_configure_and_save` succeeds. `sync_initial_import` is the canonical (and, as of this fix, only) way to create the `ai_agents` row that `sync_client::has_active_openclaw_agent()` / `get_gateway_url()` depend on. Prior to this fix, the Settings/New-Provider pairing ceremony (`ProviderConnectionCard.svelte`'s `completePairing()`) called `ai_configure_and_save` but never `sync_initial_import`, so a pairing performed here (as opposed to during onboarding) never created that row and `sync_client` never detected the gateway - see `docs/audits/2026-08-21-d11-sync-endpoint-mismatch.md`. Teaching `ai_configure_and_save` to INSERT the row itself (an alternative fix eliminating this ordering dependency) was considered and explicitly deferred - file a separate issue before implementing it, since two independent code paths (`ai_configure_and_save` and `sync_initial_import`) racing to INSERT the same row needs its own conflict-handling review, not a drive-by change alongside this fix.
>
> **INVARIANT (added 2026-08-21, #2640 / D13 fix).** `sync_initial_import` MUST NOT let the imported OC main agent collide with, or steal, an existing default agent's `is_default = 1` status. `ai_agents.is_default` is protected by a partial UNIQUE index (`idx_ai_agents_default ON ai_agents(is_default) WHERE is_default = 1`) that the INSERT's `ON CONFLICT(runtime_type, runtime_ref)` clause does not name, so an unconditional `is_default = (oc_id == "main")` derivation collides and rolls back the entire import batch whenever a non-OC default agent already exists (e.g. an OpenRouter-onboarded default created during onboarding, before OC is ever paired) - see `docs/audits/2026-08-21-d13-is-default-collision-scoping.md`. The fix: a pre-flight `SELECT EXISTS(SELECT 1 FROM ai_agents WHERE is_default = 1)` check inside the same transaction as the per-agent INSERT loop, gating `is_default` on `runtime_type == 'openclaw' && oc_id == "main" && !default_agent_exists`. **When a default agent already exists, the imported OC main agent imports as non-default with `role = 'local'`** (not `'sub'` - `'sub'` would get it filtered out of the agent picker per `DefaultAgentPickerModal.svelte`'s `role !== 'sub'` eligibility filter, and OC's own main agent must stay pickable). The existing default agent's row is never touched - **no import path may ever steal or collide on default status.** This is a one-directional check, not a demote-then-promote: `sync_re_import`'s demote-then-promote pattern (which actively steals default status from a pre-existing default) is a semantically distinct, pre-existing behavior for the explicit-provider-switch re-import path and is out of scope for this fix (tracked separately as #2644). `sync_initial_import` also ports `sync_re_import`'s `claimed_slugs` collision-avoidance pattern (pre-populated `HashSet<String>` from `SELECT slug FROM ai_agents`, suffix-increment on collision) to close the same root-cause class on the `slug` UNIQUE constraint, which had no collision avoidance at all prior to this fix.

> **INVARIANT (added 2026-08-21, #2647 / D17 fix, #2648 / D18 fix, #2644).** Four rules govern OC pairing's first-pair/re-pair branch and the `model` column, closing the D17/D18/#2644 gaps:
>
> 1. **`oc_agent_row_exists` is the only first-pair/re-pair signal.** `completePairing()` calls this dedicated DB-only command (`SELECT EXISTS ... WHERE runtime_type='openclaw'`) to decide which branch to take. It does NOT use `info.connected` (which requires a valid keychain token) — using `info.connected` would misroute a token-revoked-but-rows-present re-pair as a first pair, since a revoked token makes `info.connected` false even though the `ai_agents` rows still exist.
>
> 2. **Row exists → `reimport_oc_agents`, never `sync_initial_import`.** When `oc_agent_row_exists` returns true, `completePairing()` calls `check_oc_agent_collisions` (confirmation modal) then `reimport_oc_agents` (delegates to `sync_re_import`, overwrites in place, no new UUID directories). `sync_initial_import` is reserved for first-pair only — calling it on re-pair would mint duplicate UUID agent directories (#2648).
>
> 3. **`sync_initial_import` writes model via the CASE.** The INSERT carries a `model` column resolved via `fetch_oc_agent_model` (`GET /v1/agents`) before the DB transaction. `ON CONFLICT(runtime_type, runtime_ref) DO UPDATE` uses `model = CASE WHEN ai_agents.model IS NULL THEN excluded.model ELSE ai_agents.model END` — self-heals NULL rows from pre-fix pairings, never overwrites a user's explicitly chosen model. If the model-fetch fails, import proceeds with `model = NULL` and surfaces which agents landed without a model in the import result (not log-only) (#2647).
>
> 4. **`sync_re_import` lands OC main `role='local'` when a foreign default exists.** Both of `sync_re_import`'s demotion blocks gate default-stealing on a `default_agent_exists` pre-flight check (porting D13's pattern, §22C, from `sync_initial_import`). When a non-OC default already exists, the imported OC main agent lands with `is_default = 0` and `role = 'local'` — no import or re-import path may steal an existing foreign default's `is_default = 1` status (#2644).

> **INVARIANT (added 2026-08-21, #2647 / #2648 - OC disconnect cleanup, PR2).** Four rules govern OC disconnect (`ai_disconnect_runtime_impl`'s `"openclaw"` arm) and its interaction with re-pair:
>
> 1. **Disconnect = unpair, not delete.** `ai_disconnect_runtime_impl` sets `is_active = 0` on every `ai_agents` row with `runtime_type = 'openclaw'` — it never `DELETE`s a row. Directories and files under `.pane/system/{UUID}/` (IDENTITY.md, MEMORY.md, logs) stay on disk untouched; only DB state changes. This is deliberate: it is what makes reactivation-not-recreation (rule 3) possible on the next pair.
>
> 2. **Demote-on-deactivate.** The same UPDATE that sets `is_active = 0` on every deactivated OC row must also `SET is_default = 0` on all of them, unconditionally, regardless of their prior `is_default` value. `idx_ai_agents_default` (`CREATE UNIQUE INDEX ... ON ai_agents(is_default) WHERE is_default = 1`) is a partial unique index that enforces at most one `is_default = 1` row **across the entire table, regardless of `is_active`** — a lingering `is_default = 1` on a deactivated row is a live hazard for the next writer that tries to promote a different agent (confirmed: `docs/audits/2026-08-21-pr2-disconnect-delta-scope.md` §B reproduces the resulting `UNIQUE constraint failed` and traces a concrete silent-failure victim, `verify_agents_integrity_with_path`'s `.ok()`-swallowed self-heal promotion). Demote must always precede any new promote. After the unconditional demote, if another active agent with `role != 'sub'` exists, disconnect promotes one (`ORDER BY created_at DESC LIMIT 1`, same shape `resolve_designated_default_agent_id`'s fallback query and `set_default_agent_impl`'s demote-then-promote already use) to `is_default = 1`, in the same transaction. A sub-agent (`role = 'sub'`) must never be promoted to default. If no eligible agent exists, `is_default` is left unset on every row — this is not an error state; every downstream reader (`resolve_designated_default_agent_id`, `derive_active_provider_label`, the two consolidator call sites) already falls through to a correct "no default" / `"none"` result when zero rows satisfy `is_default = 1 AND is_active = 1`.
>
> **CORRECTION (added 2026-09-01, #2793).** The demote in this rule only ever clears `is_default` on rows matching the disconnecting runtime's own `runtime_type` (e.g. `WHERE runtime_type = 'openclaw'`) — it was never table-wide. A foreign provider's default row (a different `runtime_type` that already holds `is_default = 1`) is therefore untouched by demote-on-deactivate and may still occupy `idx_ai_agents_default`'s single global slot. The promote step above must gate on a **global** existence check (`SELECT EXISTS(SELECT 1 FROM ai_agents WHERE is_active = 1 AND is_default = 1)`) taken immediately before the promote query, in the same transaction — not merely "no default among this runtime's own rows." Without this, an unconditional promote either silently no-ops (the foreign default happens to be the newest-`created_at` row) or attempts a second `is_default = 1` write and hits the UNIQUE constraint, rolling back the entire disconnect. This guard shape (`has_default` global check, gating promote) is now shared by every disconnect arm that promotes a fallback default (`openclaw`, `venice`) and mirrors the identical pattern already used by `ai_configure_and_save`'s Venice-reconnect promote step (#2796).
>
> 3. **`is_active` reactivation rule.** Both `sync_re_import`'s overwrite-in-place `UPDATE ai_agents SET name=..., is_default=..., role=... WHERE id=?` (`sync_client.rs`) and `sync_initial_import`'s `ON CONFLICT(runtime_type, runtime_ref) DO UPDATE SET ...` (`ai::ingest`) must include `is_active = 1` in their SET/DO UPDATE clause. Without this, a re-pair after disconnect correctly reactivates the same UUID and `workspace_path` (rule 1's promise) but the row stays permanently invisible to every `is_active = 1`-gated reader (`has_active_openclaw_agent`, `ai_list_runtime_connections_impl`, every agent picker) — a silent, unrecoverable-without-a-second-fix break of the reactivation contract. `sync_re_import` is the load-bearing path (it's what an actual re-pair-after-disconnect calls, per rule 4 below); `sync_initial_import`'s inclusion is defense-in-depth for edge cases, since routing (rule 4) should normally keep this path from ever reaching a pre-existing inactive row at all.
>
> 4. **`oc_agent_row_exists` is `is_active`-blind by design.** `SELECT EXISTS(SELECT 1 FROM ai_agents WHERE runtime_type = 'openclaw')` has no `is_active` clause and must not gain one. This is intentional, not an oversight: it means a disconnected-but-not-deleted OC row still makes this check return `true`, which routes the next pairing ceremony to `reimport_oc_agents`/`sync_re_import` (reactivation, rule 3) rather than `sync_initial_import` (fresh creation, which would mint new UUIDs and duplicate directories). The reactivation-not-recreation contract this whole invariant block describes depends entirely on this function staying blind to `is_active`.

> **INVARIANT (added 2026-08-22, #2649 / D21 fix).** Disconnect (`ai_disconnect_runtime_impl`'s `"openclaw"` arm) preserves `chat_sessions.agent_id` — it does not NULL or reassign session bindings. This reverses PR2's rule (see the disconnect invariant block above, which previously NULLed the binding via `null_out_chat_sessions_for_deactivating_runtime`, now deleted): a session bound to a deactivated agent (`is_active = 0`) is treated as read-only per `docs/spec/sessions.md`'s unavailable-bound-agent rule (ADR-0062 D5), not silently rebound to whichever agent is currently `is_default = 1`. Dispatch enforcement lives in `resolve_runtime_config_for_session` (`runtime_resolution.rs`) via a new guard, `agents::check_bound_agent_active`, which runs BEFORE `resolve_and_selfheal_session_agent` and errors explicitly naming the agent when the session's existing non-NULL `agent_id` resolves to an inactive row. NULL `agent_id` rows (legacy migration backfill) are unaffected — the guard is a no-op for them, and self-heal proceeds exactly as before. The lock clears automatically the next time the bound agent's `is_active` flips back to `1` (e.g., re-pairing OC via rule 3 above) — no explicit unlock step is needed since the guard just stops firing. See `docs/spec/sessions.md` for the read-only-session UI contract this feeds (`ReadOnlySessionNotice.svelte`, `SessionDropdownHeader.svelte`), which was already correctly built and is now reachable for the first time.
>
> **INVARIANT (added 2026-08-22, #2649 / D20 fix).** Re-pair always prompts for confirmation via `ConfirmDialog.svelte`, regardless of whether `check_oc_agent_collisions` detects any collisions — the prior `collisions.length >= 1` gate in `runOcImportPhase` (`ocPairingCeremony.ts`) is removed, since both re-pair callers previously passed native `confirm()` as the `confirmCollision` callback, which resolves synchronously to `false` without rendering in Tauri's WebView, so re-pair silently declined for every collision-bearing case with no dialog ever shown and `reimport_oc_agents` never running. A declined re-pair resolves as `{ ok: true, declined: true }` — the `declined` field is a new discriminant on `ImportPhaseOutcome`/`PairingCeremonyOutcome`, structurally distinct from both a genuine 0-agent success (`declined` absent/falsy) and an error (`ok: false`). Callers (`Onboarding.svelte`'s `runOcSync`, `ProviderConnectionCard.svelte`'s `completePairing`) render a dismissable, non-error notice ("Re-pair cancelled, nothing changed") on `declined === true`, styled apart from the existing `ocSyncError`/`pairSyncError` error banners — declining is a user choice, not a failure.

**Step failures and recovery.** Each step has a recoverable failure mode:

| Step | Failure | Recovery |
|---|---|---|
| Fingerprint fetch | Gateway unreachable (network, wrong URL, gateway not running) | Settings shows the §22C.11A.6 "Can't reach gateway" error inline. User edits the URL and saves again - fingerprint fetch retries. No state is committed yet at this step. |
| Fingerprint accept | User rejects fingerprint (e.g., realizes it's wrong) | Pairing flow stops. No state stored. User can edit URL or cancel. |
| Pairing token exchange | Code expired or invalid | Settings shows §22C.11A.6 "Pairing failed" inline. User generates a new code on the gateway host, pastes, retries. The TLS fingerprint accept does not need to repeat - it's already stored for this URL. |
| Connection test | Token stored but gateway rejects subsequent calls (auth failure, no agents, gateway misconfigured) | Settings shows §22C.11A.6 error inline. The user can re-pair (regenerate code on gateway, paste again) without going through TLS accept again, since the URL and fingerprint are still valid. |

**Why inline, not wizard.** The onboarding wizard (§6.5.3) is shaped for fresh-install: provider choice → URL+token entry → agent detect → completion. A user changing settings has already made the provider choice (it's the field they edited) and has context for the URL+token decision (they're swapping a known gateway for another). The wizard's linear sequence discards that context. Inline expansion keeps the user in their decision loop.

**Pairing flow is the implicit confirmation.** Per §22C.11A.3, "non-OC → OC" cascades run without a confirmation banner because the inline pairing flow is itself the user's active confirmation - they have to perform the fingerprint accept and code paste, which can't happen by accident.

**State during pairing.** Between Save click and pairing completion, Settings displays the gateway-status row as "Pairing..." with a cancel control. If the user cancels mid-pairing, partial state (URL stored, no token, no fingerprint accept) remains; the user can resume by entering a code, edit the URL, or change provider away from OC. There's no "rolled back to old gateway" state - the cascade has already committed the URL/topology change.

**Fingerprint storage and pinning.** Once accepted, the fingerprint AND the gateway's TLS certificate PEM are stored (`gateway_tls_fingerprint`, `gateway_tls_cert_pem`). The cert PEM is served by the gateway at `/v1/health` (unauthenticated, `tls_cert_pem` field). All gateway-bound HTTP clients (14 instances across `openrouter.rs`, `ai/{chat,ingest,connections}.rs`, `system_files.rs`, `sync_client.rs`, `notes.rs`) use the shared `gateway_client::build_gateway_client(db)` builder which loads the PEM from DB and builds a pinned `reqwest::Client` with `add_root_certificate(cert)` + `tls_built_in_root_certs(false)`. A certificate mismatch (gateway cert rotation/restart) causes TLS handshake failure → error surfaced → re-pairing required. Only `gateway_fetch_fingerprint` (pre-pairing) uses `danger_accept_invalid_certs`; it accepts only `url` (no token parameter — `/v1/health` returns fingerprint + cert in the unauthenticated partial response).

> **Status:** BUILT (v0.65.11, PR #901)

### §22C.11A.6 Error states per cascade step

> **Status:** BUILT (v0.65.5, PR #873)

The cascade's five-step execution order (§22C.11A.3) plus the inline pairing flow (§22C.11A.5) define the points where errors surface. This subsection enumerates each error category, the inline UI for it, and the recovery path. All errors render inline within the AI tab; none open modal dialogs.

**Wipe step (§22C.11A.4 keys cleared).** The wipe is a local DB operation and rarely fails. Failure modes:

| Error | Inline UI | Recovery |
|---|---|---|
| DB write fails (rare - disk full, DB locked) | Inline error banner: "Couldn't save settings. Try again or restart Pane." | User retries. If persistent, this is a system-level issue, not a cascade issue. |

**Re-infer topology.** Always succeeds - it's a string parse + DB write. Listed here for completeness; no user-facing error.

**Update field visibility.** UI re-render. No user-facing error path.

**Inline pairing (§22C.11A.5 invoked).** Multiple recoverable failure modes, each rendered as an inline status under the relevant Gateway field:

| Error | Inline UI | Recovery |
|---|---|---|
| Can't reach gateway (network unreachable, DNS fail, port closed) | Status under Gateway URL row: "Unable to connect to gateway at {url}. Check the URL and that the gateway is running." | User edits URL or fixes network, save again. |
| TLS handshake fails (gateway returned non-TLS or invalid cert) | Status under Gateway URL row: "Gateway didn't respond with a valid TLS connection. Check that the gateway is configured for TLS." | User checks gateway-side TLS config. The cascade leaves URL set; user can retry without re-walking. |
| Fingerprint mismatch on previously-paired URL | Status under Gateway URL row: "TLS fingerprint changed for {url}. Accept new fingerprint to re-pair." | "Accept" button shown inline; on accept, runs the §22C.11A.5 pairing flow with the new fingerprint. |
| Pairing code invalid or expired | Status under Gateway Token row: "Pairing code is invalid or expired. Generate a new code on the gateway host and paste it here." | User generates new code on gateway host machine, pastes, retries. URL and fingerprint accept persist. |
| Pairing exchange returns unexpected response | Status under Gateway Token row: "Gateway returned an unexpected response during pairing. Check that the gateway version matches Pane's expected version." | Likely version mismatch. User verifies versions; if matched, this is a gateway bug to file. |

**Connection test (after pairing or after non-OC provider change).** Final cascade step. Failure modes:

| Error | Inline UI | Recovery |
|---|---|---|
| Auth failure (token rejected) | Gateway-status row: red dot + "Gateway rejected the token. Re-pair." Re-pair link inline. | User clicks Re-pair, which regenerates the §22C.11A.5 pairing flow. |
| Agents endpoint returned no agents | Gateway-status row: yellow dot + "Connected, but the gateway returned no agents. Check the gateway's OpenClaw connection." | Connection is technically working; user investigates gateway-side OC config. Cascade is committed; no rollback. |
| Model fetch fails for non-OC provider | Provider-row error: "Couldn't fetch models from {provider}. Check API key and network." | User verifies API key, retries from the test-connection button. |
| `internal_provider` test fails (smart routing config left orphaned by cascade) | Yellow status under Smart Features section: "Internal provider {name} couldn't be reached. Smart routing is disabled until reconfigured." | User reconfigures internal provider; Smart Features section guides them through. |

**Mid-cascade abort (user cancels Save).** The cascade is initiated by Save click; once started, it runs to one of two terminal states (success, or a recoverable failure surfaced as inline UI). The user cannot abort mid-execution - there's no Cancel button during the wipe → infer → re-render → pair → test sequence. Cancel is only available before Save (in the inline confirmation banner per §22C.11A.3) or during pairing (via the §22C.11A.5 cancel control). This intentional constraint prevents the cascade from leaving partial-rollback state.

**Persistent failure - what to tell the user.** If the user gets stuck in a loop (e.g., gateway URL works for fingerprint but pairing repeatedly fails), Settings shows a "Need help?" link in the gateway-status row that opens the docs link for Tier 3 setup troubleshooting. This is the failsafe - there is no in-app step where the user is told to restart the app or contact support without first being given recoverable options.

**No silent failures.** Every error in this subsection produces visible inline UI. If a cascade step fails without surfacing an error to the user, that's a Stia bug to file, not an acceptable state.

## §22C.12 Error handling and rate limiting

**Status:** BUILT (PR #740 error enum + IntoResponse, PR #753 OC error classification in oc_client.rs).

Resolves bridge audit gap #9.

### §22C.12.1 Pane ↔ gateway

| Status | Pane behavior |
|---|---|
| 2xx | Success. |
| 401 | Bearer token invalid/expired. Surface re-pair prompt. **No retry.** |
| 403 | Forbidden (e.g., tenant quota exceeded). Surface error to user. **No retry.** |
| 404 | Resource not found (session deleted, agent unknown). No retry. |
| 409 | Conflict (idempotency_key collision with mismatched body). Caller bug; surface as error. |
| 413 | Payload too large (see §22C.13). No retry; user must split content. |
| 429 | Rate limited. Honor `Retry-After` header. Exponential backoff up to header value. |
| 5xx | Server error. Retry with exponential backoff (1s, 2s, 4s, 8s, 30s capped). Max 5 retries, then queue or surface. |

Network-level errors (connection refused, TLS failure, timeout) are treated as 5xx-equivalent for retry purposes, except TLS fingerprint mismatch which is hard-fail.

### §22C.12.2 Gateway ↔ OC

OC error responses are mapped to gateway-side errors:

| OC error | Gateway behavior | Pane-facing status |
|---|---|---|
| Auth failure | Mark OC connection unhealthy in `/v1/health`. Stop reconnect. User must re-pair OC. | 503 |
| Method not found | Bug in gateway. Log, return 500 to Pane. | 500 |
| Rate limit from OC | Backoff per OC's `Retry-After` if present, else 1s default. Buffer outbound calls during backoff. | 429 (with synthesized `Retry-After`) |
| Internal OC error | Retry once after 1s, then surface. | 502 |
| Connection lost mid-call | Retry once after reconnect, then surface. | 502 |
| `error.code: "INVALID_REQUEST"` from a well-formed `{"ok":false,"error":{...}}` RPC response (e.g. "session not found: \<key\>") | **Corrected 2026-10-28, #3050 fix PR.** Previously collapsed into the same blanket-502 bucket as "Internal OC error" above (`oc_client.rs`'s RPC-error handler kept only `error.message`, discarding `error.code`; `error.rs`'s `IntoResponse` mapped every `GatewayError::OcClient(_)` to 502 regardless). Live-test #1b hit exactly this case (a never-before-dispatched scheduled job's `sessions.send`, surfaced as a misleading 502 that read as "gateway broke" instead of "session doesn't exist"). Now: `oc_client.rs` preserves `error.code` on a new `GatewayError::OcRpcError { code, message }` variant instead of folding it into `OcClient(String)`; `error.rs`'s `IntoResponse` matches `code == "INVALID_REQUEST"` specifically. Any OTHER OC error code not yet named this way still falls back to the pre-existing blanket 502 (`OcRpcError`'s `_` arm) — this narrows the 502 class, it does not remove it as a default. | **400** (not 404 — a direct one-to-one mapping to OC's own semantic category, "the request was syntactically fine but failed validation/referred to something that doesn't exist," rather than Pane re-interpreting that into a different category OC itself never asserted) |

### §22C.12.3 Client-facing error categories (OC topology)

> **Status:** NOT BUILT (Phase 3 launch-blocking, #946). Closes the single-regex collapse at `streamManager.ts:81-83` / `ChatSidebar.svelte:1130-1131` identified in `docs/audits/2026-06-22-phase3-scope-audit.md`.

**Scope.** This sub-section governs the user-facing error surface for chat-time and other Pane-initiated gateway calls when `provider = openclaw`. The four categories below cover the OC-topology chat path only. BYOK (Anthropic, OpenRouter) and Ollama are single-machine paths that do not transit the gateway and therefore do not surface any of these categories — their error UX is owned by their respective adapter sub-sections.

The contract is the contract between Pane's backend (`src-tauri/src/ai/chat.rs`) and Pane's frontend (`ChatSidebar.svelte`, `streamManager.ts`): the backend returns a structured error code; the frontend branches on the code and renders the locked copy. Backend code must surface a category-mapped code rather than a raw status or generic string; frontend code must render the category's locked copy rather than the backend's `message` field.

**Categories and structured error codes.**

| Category | Error code | Triggers |
|---|---|---|
| 1. Gateway not running | `gateway_unreachable` | `reqwest::Error::is_connect()` (connection refused, no listener on port); analogous platform errors that confirm the gateway process is not accepting connections. Distinguishable from category 4 by signal that the host is reachable but the gateway port is closed. |
| 2. Auth failure | `gateway_auth_failed` | Gateway HTTP response 401 or 403 (per §22C.12.1). No retry; user action required. |
| 3. Cert/fingerprint mismatch | `gateway_cert_mismatch` | TLS handshake failure where the gateway's certificate fingerprint does not match the pinned value (per §22C.4.3). Hard fail per §22C.12.1; no retry. |
| 4. Generic / unreachable / timeout | `gateway_connection_failed` | All other reqwest error kinds (request timeout, DNS resolution failure, host unreachable, network unreachable, TLS errors other than fingerprint mismatch). Gateway HTTP 5xx responses (per §22C.12.1, retry with backoff exhausted). Fallback for any unmapped failure. |

The four codes form a complete partition: every gateway-call failure on the OC topology maps to exactly one category. The frontend's branch logic is a four-way switch on `error_code`; there is no fifth "default" copy.

**Category → user-facing copy (locked).**

| Error code | User-facing copy |
|---|---|
| `gateway_unreachable` | Pane can't reach your gateway. Make sure the gateway is running on your other machine, then try again. |
| `gateway_auth_failed` | Your gateway login is no longer valid. Click Re-pair to reconnect. |
| `gateway_cert_mismatch` | Your gateway's fingerprint has changed. This is normal if you reinstalled or moved it — but if you didn't, stop and check before reconnecting. |
| `gateway_connection_failed` | Pane couldn't connect to your gateway. Check that both machines are online, then try again. |

Copy is verbatim. The frontend renders these strings without substitution or templating. The backend's `message` field, if returned, is not displayed to the user under any of the four categories — it is logged for diagnostics only. For `gateway_auth_failed`, the copy references a literal "Re-pair" button that the frontend renders inline; the Re-pair action navigates the user to Settings, where the §22C.11A.5 inline pairing flow lives.

**Out-of-scope failure modes (do not map to these four).** The following responses from the gateway are not collapsed into the four-category surface; each has its own UX path:

| Response | UX path |
|---|---|
| 404 (resource not found) | Per §22C.12.1 — no retry. Surfaced as session-level error (e.g., "This session was deleted on the other machine"), not a chat-input error. |
| 409 (idempotency collision) | Caller bug per §22C.12.1. Surfaces as a developer-mode diagnostic, not user-visible copy. |
| 413 (payload too large) | Per §22C.13. Pane refuses to send messages exceeding the limit client-side; the user-facing surface is "Message too long" on the input field, not a gateway-error category. |
| 429 (rate limited) | Per §22C.12.1, exponential backoff up to `Retry-After`. While backing off, no error is surfaced; if retries exhaust, falls through to `gateway_connection_failed`. |

**Streaming-vs-non-streaming surface.** The four categories apply to both pre-200 errors (before the SSE stream opens) and mid-stream errors (after 200, during chunk delivery). Mid-stream errors carry a structured-event variant on the SSE channel that maps to the same four codes; the frontend's renderer treats stream-aborted errors identically to request-failed errors for the purpose of copy selection. The streaming-proxy idle deadline (§22C.4.1.1) firing mid-stream surfaces as `gateway_connection_failed` — the dual-channel silence rule means a true timeout indicates either OC or the gateway↔OC link has failed, both of which read to the user as "couldn't connect."

**Reconcile with §22C.12.1.** §22C.12.1's status-code → Pane-behavior table is the transport-layer contract: it says when Pane retries, when it backs off, when it surfaces. §22C.12.3 is the UX-layer contract layered on top: once §22C.12.1's retry-or-surface decision lands at "surface," §22C.12.3 says what copy renders. The two are complementary. §22C.12.1 governs whether the user sees anything; §22C.12.3 governs what they see.

**Adjacency to #945 (reconnect).** Reconnect behavior — auto-retry windows, exponential backoff state machine, when the UI shows "Reconnecting…" vs the categories above — lives in #945's spec work, sibling to this sub-section. The four categories here apply once reconnect attempts have exhausted or are not applicable (e.g., `gateway_auth_failed` is hard-fail per §22C.12.1, so no reconnect attempt happens; `gateway_connection_failed` is reached after #945's retry budget is exhausted). #945 may extend this table with a "Reconnecting" pre-error state that occupies the UI before any of these four render; that extension is in scope for #945's spec, not this amendment.

**Backend contract.** `src-tauri/src/ai/chat.rs` returns a typed error variant per category. The wire shape between Rust and the frontend uses the `error_code` string above; the frontend's branch logic matches on this string. The error type is OC-topology-scoped — BYOK and Ollama adapters return their own error variants from their own code paths and do not produce these codes.

### §22C.12.4 Reconnect behavior (chat/SSE path)

> **Status:** BUILT (v0.67.28, PR fix/945-auto-reconnect). Implemented per spec: 5-retry exponential backoff (1s→2s→4s→8s→30s ±10% jitter), hard-fail on gateway_auth_failed/gateway_cert_mismatch, passive Reconnecting indicator, Send disabled during backoff, error_code emission for #946 frontend rendering, chat-retries telemetry counter. spec-covered-by: #1603 (§22C.12.4).

**Scope.** This sub-section governs the application-layer reconnect loop that runs when a chat-completion request to the gateway fails on the OC topology. It is distinct from §22C.4.1's TCP-level reconnect (HTTP/2 connection re-establishment) and from §22C.12.1's per-request retry budget within an established connection. §22C.12.4 governs the user-facing reconnect cycle that wraps a single chat completion: retry the failed request with backoff, surface "Reconnecting…" in the UI between attempts, fall through to §22C.12.3's error categories when the budget exhausts.

**Surface scope.** This contract applies ONLY to the chat/SSE path (`src-tauri/src/ai/chat.rs` chat completion entry points; `ChatSidebar.svelte` / `streamManager.ts` on the frontend). Explicitly out of scope:

- **Sync client paths** (`pull_pending_changes`, `flush_sync_queue`, `trigger_sync`, initial sync) — these have their own retry per §22B / §22C.8.3 and #1312's resilience work. §22C.12.4 does not govern sync retry behavior.
- **Settings "Save & Test"** — single-shot by design. The test-connection button surfaces success or failure on one attempt; no silent retry. A user clicking "Save & Test" expects an immediate verdict, not a hidden reconnect loop. If the test fails, the §22C.11A.6 error-state UI surfaces; no §22C.12.4 reconnect cycle.
- **Pairing exchange** (§22C.11A.5) — also single-shot. Pairing-code expiry (5 min) makes silent retry inappropriate; user re-enters or regenerates.
- **Background telemetry / health-check pings** — no UI surface, no user-facing reconnect.

**Backoff curve.** Per §22C.4.1 and §22C.12.1: 1s → 2s → 4s → 8s → 30s capped, max 5 retries. With one addition specific to this surface: **±10% uniform jitter applied to each interval.** Concretely, the wait before retry N is `base_interval × (1 + uniform(-0.10, +0.10))`. The jitter prevents thundering-herd when the gateway recovers and multiple clients (or multiple Pane installs on the same network) reconnect simultaneously. Jitter is the only new value §22C.12.4 introduces; the curve and budget are carried forward unchanged.

**Hard-fail exclusions.** Per §22C.12.3, two categories skip the reconnect loop entirely:

- `gateway_auth_failed` (401/403) — no retry, ever. Surface immediately per §22C.12.3 copy.
- `gateway_cert_mismatch` (TLS fingerprint mismatch) — no retry, ever. Surface immediately per §22C.12.3 copy.

For all other §22C.12.3 categories (`gateway_unreachable`, `gateway_connection_failed`) and for transient 5xx responses per §22C.12.1, the reconnect loop runs.

**State machine.**

| State | Trigger | UI |
|---|---|---|
| `Idle` | No active chat request, or last request completed successfully. | Normal chat UI; no reconnect indicator. |
| `Attempting` | Chat request in flight (first attempt). | Normal request UI (existing spinner / typing indicator per chat surface). No reconnect indicator yet — the first attempt failing is what triggers the indicator. |
| `Reconnecting` | First attempt failed with a retry-eligible error; backoff timer running before next attempt. | Passive status indicator visible in the chat surface (e.g., header or near the input — implementation choice for #945's frontend work, not spec'd here). Indicator does NOT block the input field. User can type, but Send is disabled until `Idle` or `Failed`. |
| `Failed` | Retry budget (5 attempts) exhausted, or hard-fail category hit on any attempt. | §22C.12.3 error copy surfaces. Reconnect indicator clears. Send re-enables; user can retry manually. |

**Transitions.**

- `Idle` → `Attempting`: user sends a chat message.
- `Attempting` → `Idle`: request succeeded.
- `Attempting` → `Reconnecting`: request failed with retry-eligible error; backoff timer starts.
- `Attempting` → `Failed`: request failed with hard-fail category (`gateway_auth_failed`, `gateway_cert_mismatch`).
- `Reconnecting` → `Attempting`: backoff timer elapsed; retry attempt in flight.
- `Reconnecting` → `Idle`: skipped — `Reconnecting` always transitions through `Attempting` (the retry attempt) before returning to `Idle` on success.
- `Attempting` (retry) → `Idle`: retry attempt succeeded.
- `Attempting` (retry) → `Reconnecting`: retry failed, budget not exhausted, backoff for next attempt.
- `Attempting` (retry) → `Failed`: retry failed AND (budget exhausted OR hard-fail category).
- `Failed` → `Idle`: user dismisses error or sends a new message (which starts a fresh `Attempting` cycle with the budget reset).

**Reconnect indicator is passive.** The indicator informs the user that retry is in progress; it does NOT modal-block the UI. User can continue reading prior messages, scrolling, switching channels. The chat input field accepts typing in the `Reconnecting` state but Send is disabled — a queued message would create ordering ambiguity with the in-flight retry. Send re-enables on `Failed` (user can retry manually) or on `Idle` (retry succeeded; next message is a fresh request).

**In-flight SSE on mid-stream disconnect.** When a chat completion is mid-stream (SSE bytes flowing, response not yet complete) and the connection drops:

1. Pane aborts the current SSE stream immediately.
2. State transitions to `Reconnecting`.
3. On retry attempt, Pane re-sends the **original chat message** (not a continuation request).
4. The partial response received before the disconnect is discarded from the visible chat — it does not persist in the conversation history.

**No `Last-Event-ID` resume for chat completions.** SSE's `Last-Event-ID` mechanism (used by §22C.7.3 for subscription-event replay against the gateway's per-session ring buffer) does NOT apply to chat completions. The ring buffer's scope is subscription events, not chat-completion bytes — repurposing it would require expanding §22C.7.3's contract and adding chat-byte buffering at the gateway. Out of scope for #945.

**No partial-result splicing.** Pane does NOT attempt to merge the partial response (received before disconnect) with the retry response. The retry produces a complete fresh response; the partial is discarded. Splicing would require both gateway-side buffering and client-side merge logic — same architectural expansion as `Last-Event-ID` resume. Out of scope for #945.

**Why re-send original message.** It is the only option that uses existing primitives. The reasoning is derived from §22C.7.3's scope (subscription-event replay only), not selected from architecturally-viable alternatives. A future expansion of §22C.7.3 to cover chat-completion buffering — if motivated by other requirements (long-running tool calls, multi-modal streaming) — would unlock `Last-Event-ID` resume for chat as a follow-up; that expansion is not in scope here.

**Interlock with §22C.12.3.** The reconnect loop and the error categories are sequential, not overlapping:

1. Request fails with a retry-eligible error → `Reconnecting` (indicator shown).
2. Backoff elapses → retry attempt → succeed (`Idle`) OR fail and re-enter `Reconnecting` (budget permitting).
3. Budget exhausts (5 attempts failed) → final error emitted per §22C.12.3 category rules:
 - Network-level retry-eligible failures (was `gateway_unreachable` or `gateway_connection_failed` at original attempt) → emit the same code on the final failure. §22C.12.3 renders the locked copy.
 - Hard-fail categories (`gateway_auth_failed`, `gateway_cert_mismatch`) bypass the loop entirely and emit on the first attempt.

The "Reconnecting…" indicator clears the moment the final error renders. Users see at most two UI states for a single failed chat: `Reconnecting` during retry, then §22C.12.3 error copy when the budget exhausts. No overlap; no race where both render simultaneously.

**Telemetry / observability.** Each retry attempt is logged per §22C.14's structured-log fields (`ts`, `level`, `request_id`, `endpoint`, `oc_method`, `latency_ms`, `status`, `error_code`). A `retry_attempt` field is added when the request is a retry (1-indexed); attempt 1 is the original request and has no `retry_attempt` field (or `retry_attempt = 0` — implementation choice). The metrics counter `ws-reconnects` (per §22C.14) does NOT cover §22C.12.4's application-layer retries; a new counter `chat-retries` is added to distinguish.

## §22C.13 Payload limits

**Status:** BUILT (PR #740 handler-level validation, PR #752 HTTP-layer `DefaultBodyLimit`).

Resolves bridge audit gap #10.

| Endpoint | Limit | Behavior on exceed |
|---|---|---|
| `POST /v1/chat/completions` content | 8 MiB | 413, no retry. |
| `POST /v1/sync/push` content | 10 MiB | 413, no retry. Logged. |
| `GET /v1/sync/pull` total response | 50 MiB | 500 with truncation marker; client must re-request with smaller agent scope. (Edge case; typical response < 1 MiB.) |
| `GET /v1/sync/initial` total response | 100 MiB | 500. Mitigation: per-agent fetch endpoint deferred but reserved as escape valve if hit. |
| `POST /v1/sessions/:id/messages` content | 1 MiB | 413, no retry. |
| Single SSE event payload | 256 KiB | Gateway splits into multiple `message.delta` events. |

10 MiB matches the documented MEMORY.md ceiling. Audit data: typical files <50 KiB, worst case ~5 MiB year-of-daily-logs. 10 MiB caps with 2× headroom. **Hard reject at the limit (no warn-then-allow)** - easier to enforce, easier to detect drift.

## §22C.14 Observability

Minimal at launch, hooks designed in.

**Logs:** structured JSON, line-per-event, written to gateway data directory. Fields: `ts, level, tenant_id, request_id, endpoint, oc_method, latency_ms, status, error_code?`. No log shipping at launch - user can `tail` the file.

**Metrics:** in-memory counters exposed at `GET /v1/metrics` (auth required). Counters at launch: requests-by-endpoint, errors-by-status, oc-method-calls, sync-queue-depth, ws-reconnects, sse-replays, replay-gaps. Format: Prometheus exposition. No scrape endpoint plumbing at launch - user can curl.

**Tracing:** request IDs only. `x-request-id` header propagated end-to-end (Pane → gateway → OC). No distributed tracing at launch.

This is enough to debug production issues. Tier 1 adds shipping, dashboards, alerting.

## §22C.15 Test strategy

Resolves bridge audit gap #6 (zero test coverage). All seven categories below are blocking for cutover (§22C.16) and run in CI on every gateway PR.

1. **Network failure tests.** Mock gateway endpoint goes offline mid-push; assert queue retains entry, retry succeeds on reconnect. Equivalent test for pull. Equivalent test for SSE stream interruption.
2. **Conflict resolution tests.** Two clients write same file with different timestamps; assert LWW resolves correctly. Tie-case asserts Pane wins (§22C.8.2; ADR-0042). Content-identity skip case asserts no write.
3. **Reconnect-with-gap tests.** Gateway↔OC connection torn down with N events buffered on gateway side; on reconnect, assert client receives all N replayed via `Last-Event-ID`. Buffer overflow case asserts `replay.gap` event emitted and refetch path works.
4. **Multi-tenancy isolation tests.** Two tenants, same gateway. Assert tenant A cannot read tenant B's sessions, sync queue, or agents. Even at single-tenant launch, this tests the seam - important for the Tier 1 flip and load-bearing for the "cross-tenant access impossible by construction" INVARIANT.
5. **Auth boundary tests.** Bearer token revoked mid-session; assert 401 on next request, no leak of in-flight data. Cert fingerprint mismatch on Pane reconnect; assert hard fail.
6. **Payload limit tests.** Push 11 MiB content; assert 413, queue not advanced. Send 1.1 MiB session message; assert 413.
7. **Error classification tests.** Mock OC returns 401, 403, 429, 500, 503; assert gateway maps each correctly to Pane and respects `Retry-After`.

Pane-side integration tests (Tier 2 in `dev-hygiene-playbook.md` §4) cover end-to-end behavior on the Pane client.

**Same-machine no-bridge tests are Pane-core, not part of this suite.** They live alongside Pane's existing watcher/integrity tests and verify the post-cutover same-machine path: bridge plugin disabled, `watcher.rs` catches OC writes, `verify_agents_integrity()` reconciles after Pane restart, model discovery via direct `openclaw.json` read, chat via existing OpenRouter adapter. Cutover criterion §22C.16 requires these green throughout dogfood.

**Status:** BUILT (v0.63.0). Test file: `src-tauri/src/tests/test_same_machine_no_bridge.rs`. Six tests covering topology detection, sync noops, model discovery (calls `fetch_openclaw_models_topology_aware` with `pub(crate)` visibility for testability), and watcher reconciliation (calls `reconcile_file` directly).

## §22C.16 Bridge cutover criteria

The `pane-bridge` plugin remains in production during gateway build. It is not deprecated until the gateway has met all of the following:

1. **Gateway↔OC layer passes its own §11 audit.** Same five-check pattern (spec coverage, code/spec drift, test coverage, INVARIANTS coverage, ADR coverage). Reed runs it.
2. **All seven test categories from §22C.15 are green and have been green continuously throughout the dogfood window.**
3. **Same-machine no-bridge integration tests pass green continuously throughout the dogfood window.** Pane-core test suite covers: bridge plugin disabled, sync via filesystem watcher works, model discovery via direct `openclaw.json` read works, chat via existing OpenRouter adapter works, OC-writes-while-Pane-closed reconciles correctly via integrity check. Required because Matt's personal dogfood is split-machine; same-machine validation needs its own automated path.
4. **Pane client (split-machine) has been on gateway exclusively for ≥7 days of dogfood with no regressions.** "Exclusively" means the bridge plugin is unloaded on Matt's OC instance during the dogfood window. One regression resets the clock.
5. **Matt sign-off** that the gateway is rock-solid for launch, captured as a comment on the cutover issue.

**Regression definition.** A regression is behavior the bridge plugin handled correctly that the gateway does not. Pre-existing bugs surfacing during dogfood are filed against their original system and do not reset the clock. If unclear which side a bug belongs to, default to "not a gateway regression" unless reproducible against the gateway specifically (and not reproducible against the bridge in equivalent setup).

After cutover:

- §22B is marked LEGACY in spec. The section is retained for historical context but no longer normative.
- The `pane-bridge` plugin **fully retires.** Per §22C.11.4 there is no same-machine version to preserve — OpenClaw is split-machine only (#2649). The plugin can sit unused in the OC extensions directory without harming anything; older Pane versions continue to function until they upgrade. Once everyone has upgraded, the npm package can be archived.
- `sync_client.rs` HTTP layer is refactored to point at the gateway only. Bridge endpoint constants removed.
- The `/bridge/models` call from Pane is replaced by `GET /v1/models` through the gateway.

## §22C.17 What the gateway is NOT - non-goals (consolidated)

Restated for the implementation team. Anything the spec did not explicitly specify is out of scope. Specifically not in scope:

- Task management endpoints (§22B.4 v3 deferred).
- Real-time push from gateway to Pane outside session SSE.
- LLM proxying or routing decisions.
- Tier 1 hosting concerns (multi-tenant implementation, billing, admin UI, HA).
- Same-machine sync — not applicable; OpenClaw is split-machine only (#2649), no filesystem-direct path exists.
- Agent creation/update/deletion endpoints.
- Auto-registration of new agents in `openclaw.json` (pre-existing gap; not a gateway concern).
- The 23+ Tier 3-tagged OC methods not in §22C.7.1.
- Any non-`v1` API surface.
- `chat.send` and `chat.abort` (replaced by `sessions.send` / `sessions.abort`).

## §22C.18 INVARIANTS introduced

To be added to `docs/INVARIANTS.md` in a follow-up PR. One line each, with one-line justification.

1. **Gateway is the sole sync transport for split-machine Tier 3.** Bridge plugin is legacy post-cutover. *Prevents drift back to dual-transport.*
2. **Same-machine Tier 3 never deploys the gateway.** Filesystem-direct: symlink for sync, direct `openclaw.json` read for model discovery, existing OpenRouter adapter for chat. *Gateway is split-machine only.*
3. **Tier 2 never routes through the gateway.** BYOK is direct-to-provider. *Gateway is Tier 1/3 only by design.*
4. **Gateway terminates TLS architecturally.** *No assumption that the network is encrypted.*
5. **Cross-tenant access is impossible by construction, not by check.** Every gateway state row carries `tenant_id`; every query is tenant-scoped. *Prevents seam erosion as features land.*
6. **Bearer token is opaque and gateway-resolved.** No JWT at launch. *Simpler at single-tenant; no migration cost when JWT introduced.*
7. **`oc_agent_id` is the routing key for sync.** Names are display-only. *Carries forward from #364; renames must not break sync.*
8. **Sync queue: max 5 retries, RFC3339 timestamps, max 100 items per flush.** *Carries forward from §22B.3.7.*
9. **Atomic write + read-back checksum is mandatory on the gateway side.** *Write verification was the bridge's defense against torn writes; carries forward.*
10. **WS subscription gaps are invisible to the Pane client.** Replay buffer + `replay.gap` reset semantics. *Streaming reliability is load-bearing for chat UX.*
11. **No `chat.send` or `chat.abort` calls from the gateway.** `sessions.send` and `sessions.abort` are canonical for those operations. `chat.history` is permitted because no `sessions.history` parallel exists in OC's surface. *Protocol consistency where parallels exist; chat.* are otherwise legacy aliases.*
12. **Gateway invokes only the methods in §22C.7.1.** *Scope discipline; prevents creep into deferred surface.*
13. **Payload caps are hard rejects, not warnings.** 10 MiB push, 1 MiB session message. *Easier to detect and enforce than soft limits.*
14. **Reconciliation logic lives in Pane; the gateway is transport only.** Timestamp comparison, LWW conflict resolution, content-identity skip, and atomic-write decisions are all Pane-side. Logic lives in `conflict_resolution.rs` and is invoked by `pull_pending_changes()` when pulling from the gateway. *The gateway moves bytes; it does not decide which version of a file wins. Single source of truth for sync semantics.*
15. **Pane MUST NOT write externally controlled paths outside `$HOME/Pane/.pane/system/{uuid}/`.** `file_ops::safe_path_join` is the single enforcement point. Every path segment originating outside the Rust trust boundary—including gateway/migration filenames and WebView-supplied workspace or filename values—MUST go through it before a system or log write. `write_system_file` and `write_log_file` enforce this boundary as of #1377. Defense-by-allowlist (§22C.8.5, ADR-0041) is scope filtering, not filesystem sandboxing—the two controls are complementary. (`#1300`, `#1377`, `docs/INVARIANTS.md` Pane Gateway section)

## §22C.19 ADRs needed

Drafted in follow-up session, filed before implementation. One-paragraph framing each.

- **ADR-0012: Pane Gateway as primary sync transport for split-machine Tier 3.** Establishes the gateway as the canonical transport, supersedes the bridge plugin. Captures the soundness argument over Reed's defer-to-Tier-1 recommendation (the data - 70% transport-agnostic, bridge gaps that the gateway absorbs forward, Tier 1 forward-compatibility - was sound; the recommendation was velocity-mismatched). Captures the cutover criteria. References §22C.

- **ADR-0013: Auth and multi-tenancy seam for the Pane Gateway.** Locks the opaque-bearer-token + tenant-scoped-state design. Captures the rationale for opaque-over-JWT at launch, the Tier-1-flip argument (config change, not refactor), and the per-tenant-OC-credentials approach. References §22C.5.

- **ADR-0014: Bridge vs Supabase split.** Formalizes the already-shipped two-system decision: Supabase CRDT for user data (notes, projects, AI chat history), bridge/gateway HTTP for agent identity files. The rationale (LWW text files vs. CRDT-able rows; zero-knowledge violation if SOUL/MEMORY were in Supabase; same-machine symlink optimization) lives in audits but has no ADR. **Sequencing note:** ADR-0014 doesn't depend on the gateway spec and can be filed independently - recommend doing so this week regardless of gateway timeline.

- **ADR-0015: Gateway↔OC transport - WS where required, HTTP where supported.** Captures the protocol decision: gateway speaks WS to OC because OC is WS-only at the core (147 methods, 0 core HTTP), but Pane sees clean HTTP/2 + SSE. The "absorbs OC's protocol weirdness so Pane stays clean" framing. References §22C.4.

## §22C.20 Spikes

Named, time-boxed, do not block spec lock. Each spike answers a specific question and produces a written finding before its work is implemented.

1. **OC `sessions.messages.subscribe` buffering semantics.** Question: does OC buffer events while the gateway is disconnected, or drop them? Method: instrument a test gateway, induce reconnects, measure gap rate. Time-box: 0.5 day. Output: written finding pasted into §22C.7.3 as a footnote. Outcome modulates how often `replay.gap` fires in practice but does not change the gateway's design.

2. **TLS cert source for production deployments.** Question: is self-signed-with-fingerprint-pinning acceptable for the long term, or should Let's Encrypt be supported at launch? Method: 1-day investigation of LE rate limits, ACME client options for non-public hostnames (DNS-01), and the UX cost of cert rotation. Time-box: 1 day. Output: either confirmation that self-signed is launch-final or a small spec addendum for ACME support.

3. **Implementation language calibration.** Not a spec spike - flagged here because the kickoff explicitly excluded language choice from spec. Stia + the implementation issues will choose. The 70% transport-agnostic finding (queue/conflict logic in `sync_client.rs` ports cleanly) implies Rust as the natural choice; a Node/TS gateway re-implementing those primitives is also viable. If the choice ends up architecturally consequential, file an ADR alongside the implementation issues.

4. **`verify_agents_integrity()` content reconciliation coverage.** Spec'd in full in §22C.11.3a. Blocking check before the same-machine refactor PR opens. Question: does the existing integrity check reconcile file-content changes from OC-writes-while-Pane-closed for all whitelisted files? Method: read the function end-to-end + manual test (stop Pane, edit SOUL.md via symlink, restart, observe). Time-box: 0.5 day. Output: written finding (outcome A/B/C); under B or C, re-size the same-machine refactor track per §22C.21 before implementation. Highest priority of the four - same-machine cutover hinges on this.

## §22C.21 Sizing

Honest sizing against Matt's velocity at the agreed scope, re-estimated after Reed's same-machine inventory (2026-04-27) brought same-machine sync work in-scope and after Reed's §22C.11.3a check (2026-04-27) returned outcome B - `verify_agents_integrity()` covers existence but not file-content reconciliation. Active assumption: outcome B.

| Track | Estimate (outcome B active) |
|---|---|
| Gateway core service (HTTP/2 server, WS client to OC, queue, conflict, agent registry, auth, TLS) | 3-5 days |
| Pane split-machine refactor (`sync_client.rs` HTTP layer points at gateway; topology routing; conflict-resolution module extraction) | 1-2 days |
| Pane same-machine refactor (`watcher.rs` startup-reconciliation upgrade per §22C.11.3; replace `/bridge/models` with direct `openclaw.json` read; bridge endpoint cleanup) | 2.5-4.5 days |
| Gateway test suite (seven categories from §22C.15, blocking) | 2-3 days |
| Same-machine no-bridge integration tests (Pane core CI; covers watcher startup reconciliation) | 1 day |
| **Build subtotal** | **9.5-15.5 days** |
| Dogfood window (cutover criterion §22C.16) | 7 days minimum |

**Excludes:** ADR drafting, INVARIANTS update PR, issue scaffolding, Reed pre-audit on issues. Those are sequenced as separate sessions before implementation per the kickoff.

**Delta from previous estimate (8.5-13.5 days, outcome A assumption).** Outcome B adds 1-2 days to the same-machine track (watcher startup-reconciliation pass + conflict-resolution module extraction). Range shifts to 9.5-15.5 days. Note that the conflict-resolution module extraction work also slightly broadens the split-machine refactor - the same module gets reused there, which is reflected in keeping that track at 1-2 days rather than narrowing it.

**Conditional expansion under §22C.11.3a outcomes.** Active outcome is B (resolution: watcher upgrade + shared module per §22C.11.3 and §22C.11.2a). Outcome A would have been 1.5-2.5 days for the same-machine track (1 day less, total 8.5-13.5 days) but the check disproved A. Outcome C (no file-content reconciliation anywhere) was the worst case at 3.5-6.5 days for the same-machine track; B is bounded above by it. The largest source of variance remains the test suite plus the new shared-module extraction. Cutting categories cuts the rock-solid promise. Don't.

---

## §22C.22 Onboarding Integration

> **Status:** DESIGN LOCKED (#953 / PR #960). NOT BUILT - sub-tasks #879 (SameMachine auto-discover), #880 (SplitMachine pairing screen), #881 (write topology on completion).
> **Updated:** 2026-05-07 - added per redesign 2026-05-06.
> **Updated (#2649, PR 3A):** Step 2's pairing exchange + Step 4's initial-sync/re-import routing described below are now implemented as a shared module (`src/lib/services/ocPairingCeremony.ts`) called by both onboarding (`docs/spec/onboarding.md` §6.6.3) and the Settings inline pairing flow (§22C.11A.5 / `ProviderConnectionCard.svelte`), rather than being duplicated across the two callers as they previously were. Onboarding now also does the first-pair/re-pair `oc_agent_row_exists` routing this sub-section documents (previously Settings-only) and writes `gateway_paired_at` on its own pairing path. See §22C.22.5 for the corresponding `provider_state` retirement.

This sub-section bridges the onboarding redesign screens (`docs/spec/onboarding.md` §6) to the gateway operations elsewhere in §22C. It is the canonical reference for *which gateway operations onboarding invokes, in what order, and with what UX implications*. Each sub-step references the existing §22C section that defines its mechanics; this sub-section does not redefine mechanics, only sequences them.

External-system contracts that onboarding depends on (gateway pairing wire shape, error envelope, pairing-code semantics) are captured in `docs/spec-prep/2026-05-07-external-contract-captures.md` per ADR-0023, and are referenced inline below.

### §22C.22.1 Scope

The cases this sub-section covers:

- **First-time pairing** during onboarding (SplitMachine only; see §6.5.3).
- **Initial sync** - the `GET /v1/sync/initial` call that pulls every agent's whitelisted identity files on first connect.
- **Topology-choice writeback** - the persistence of the customer's screen 2.5 choice into `ai_settings.tier3_topology` (§22C.11), including the interaction between the customer's stated choice and §22C.11's URL-based topology inference.

The cases this sub-section does **not** cover (referenced for completeness):

- Settings post-onboarding cascade-flow re-pairing - covered by §22C.11A.5 (the Settings inline pairing flow). The redesign uses §22C.11A.5 as the template for §6.5.3's "Verify and connect" screen; the two screens implement the same mechanics in different contexts (Settings vs. onboarding wizard).
- Subscription/lapse/take-over checks at sign-in - these are ecom-side, not gateway-side, and live in `docs/spec/onboarding.md` §6.1 (with ADRs 0024/0025/0026).
- Same-machine direct-read of `openclaw.json` for model discovery - covered by §22C.11.

### §22C.22.4 SplitMachine - URL entry + pairing + initial sync

When the customer chose "On another machine" at screen 2.5:

**Step 1: Gateway URL entry.**

Customer enters the gateway URL (e.g., `http://100.64.0.1:18789`). Format normalization per sub-task #18 (auto-prepend `https://` and `:18789`). On Continue, Pane probes the URL.

URL probe must distinguish "unreachable" from "reachable but not a gateway" (sub-task #17, protocol check after TCP connect succeeds). Probe failures route to error states per §22C.11A.6.

The probe also fetches the gateway's TLS cert fingerprint via `/v1/health` (the `tls_fingerprint` and `tls_cert_pem` fields per §22C.4.3 / §22C.5.1). Pane holds the fingerprint pending customer verification.

**Step 2: Verify-and-connect (combined screen).**

The redesign collapses fingerprint verification + pairing-code exchange into one screen (per design brief screen F; sub-task #880). Same flow as §22C.11A.5 (Settings inline pairing) but in a wizard context instead of a Settings panel.

The screen displays:

- The TLS fingerprint Pane fetched at probe time.
- An instruction to compare the displayed TLS fingerprint against what Pane Gateway emitted to its stderr on startup on the OC machine. The fingerprint Pane displays was fetched from `/v1/health` at probe time; the user reads the gateway's startup banner and visually confirms a match.
- A 8-digit pairing-code field. Static helper copy states codes expire after 5 minutes and instructs the user to run `pane-gateway pairing-code` on the OC machine if expired — no live countdown timer (no gateway API exposes the code's remaining TTL).
- An instruction to read the 8-digit pairing code from Pane Gateway's stderr on the OC machine (emitted alongside the fingerprint at startup). If the 5-minute window expires before pairing completes, the user runs `pane-gateway pairing-code` on the OC machine to generate a fresh code.
- A Connect button and a "Fingerprint doesn't match" hard-block path.

On Connect:

1. Pane verifies the fingerprint stored at probe time matches what the customer accepted. (This is local - Pane does not re-fetch from the gateway. The customer's accept-action is the trust event.)
2. Pane calls `POST /pair` with the entered code. Wire shape per external-contract-captures §1; error mapping per §2.
3. On 200, gateway returns:

   ```json
   {
     "token": "<256-bit hex>",
     "expires_at": "<RFC3339, +90 days>",
     "tenant_id": "default",
     "gateway_version": "0.1.0"
   }
   ```

   Pane writes the pairing result to `ai_settings` via two paths:

   - **Primary auth (canonical write path):** `ai_configure_and_save` writes `endpoint` (the gateway URL the customer entered at Step 1, normalized) and `api_key` (the `token` from the response body, encrypted per §22C.5.1). These are the keys the chat layer (§22C.9) and the sync layer (§22C.8.1) both read; see `docs/spec/file-system.md` §2.3C for the canonical key definition.
   - **Gateway pairing state (separate writes):** `gateway_tls_fingerprint` (the fingerprint Pane fetched at probe time from `/v1/health`) and `gateway_tls_cert_pem` (the cert PEM from the same response) are written directly to `ai_settings`. These are gateway-pairing-specific and persist as their own keys — they are not part of `ai_configure_and_save`'s scope.
   - **Topology state:** `tier3_topology = "split_machine"` (matches the customer's screen-2.5 choice and the URL-inference backstop), `provider = "openclaw"`, `paired_at = now()`.

   **`gateway_version` — required field (#1508).** The `PairResponse` body includes `gateway_version`, a semver string identifying the running Pane Gateway build (sourced from the gateway's `CARGO_PKG_VERSION`). This field is a **required** part of the pairing wire contract: the gateway MUST emit it on every 200 `PairResponse`, and Pane MAY reject a `PairResponse` that lacks it as a malformed/incompatible response. The field exists because Pane (user machine) and Pane Gateway (OC machine) ship and update as independent artifacts, so the two machines can run mismatched versions; `gateway_version` is the runtime signal Pane uses to detect that skew at pairing time. What Pane does with the value — any minimum-version floor, comparison logic, warn-versus-block behavior, or where it is stored/logged — is Pane-side implementation (#1508) and is deliberately NOT part of this wire contract; the contract guarantees only that the field is present, is a semver string, and is required.

4. On 401 (any of the three reachable `Unauthorized(_)` sub-cases per external-contract-captures §2), Pane surfaces the customer-visible copy per §6.5.3's error mapping table. The `gateway_exchange_pairing_code` command extracts the wire `message` field from the 401 JSON body and returns it as the error string, enabling frontend error discrimination. The customer can retry within the 5-min code window without invalidating the active code (capture §3 confirms this).

**Step 3: TLS-pin enforcement on subsequent calls.**

Once `gateway_tls_fingerprint` and `gateway_tls_cert_pem` are stored (per Step 2's separate-writes path), Pane uses `add_root_certificate(stored_pem)` with `tls_built_in_root_certs(false)` for all subsequent gateway calls - only the gateway's specific cert is trusted. This matches the §22C.5.1 token-storage flow; the redesign does not change this mechanism.

**Step 4: Initial sync (`GET /v1/sync/initial`).**

Pulls every agent's whitelisted identity files (per §6.5.4). Endpoint behavior is defined in §22C.6.2 (which currently documents the bridge endpoint `/bridge/sync-all`; that section needs the gateway equivalent - see §22C.22.7 below for the cross-reference cleanup needed).

On sync success, Pane writes `provider_state = configured_verified` to `ai_settings`. This is the field §22C.22.5 lists at completion and that §6.4.3 reads to gate chat-in-Home and the welcome message.

> **Retired (#2649, PR 3A):** the `provider_state` write described in this paragraph and listed in §22C.22.5's table has been deleted from the onboarding write path. A full-codebase search (`.svelte`/`.ts`/`.rs`/`.js`) found zero readers of `provider_state` anywhere — whatever currently gates chat-in-Home and the welcome message, it is not this key. The table row and this paragraph are retained for historical/git-blame context; do not resurrect the write.

The redesign decision (per Q-decision in dev dispatch and sub-task #21) is **retry-all-or-fail-pairing on any sync failure**. Partial-success state is not maintained in v1. If the initial sync errors on any agent, the customer sees:

> Sync failed. [Retry]

...with retry-all behavior, no skip path. Sub-task #25 (graceful partial-sync handling, including per-agent retry) is filed POST-LAUNCH; v1 ships the simpler retry-all-or-fail.

A side effect of retry-all-or-fail is that an interrupted onboarding (network drops mid-sync) leaves `onboarding_resume_state = oc_pairing` - not `sync_in_progress` - so resuming requires re-pairing. Per dev dispatch §State machine: edge cases. Sub-task #42 (smart resume on mid-sync quit) is also filed POST-LAUNCH.

**Step 5: Read-only Your Team display.**

Per §6.5.5.

**Step 6: Onboarding completion.**

Per §6.4. SplitMachine does **not** need a "Point OpenClaw to Pane" workspace-reconfig screen - gateway sync handles bidirectional file movement, OC needs no workspace reconfig in SplitMachine (decision OC-C; verified by Brit sub-task #22).

**Spec-error correction (2026-05-15).** An earlier draft of this subsection referenced `openclaw gateway fingerprint` and `openclaw gateway pairing-code` as user-facing commands on the OC machine. Those commands do not exist in OC's CLI and should not — pairing is between Pane and Pane Gateway, both of which are Pane-owned components. The corrected references are above: the fingerprint is fetched by Pane from `/v1/health` and verified by the user against the gateway's stderr banner; the pairing code is read from the gateway's stderr (or regenerated via `pane-gateway pairing-code` if expired). See §22C.5 for the full pairing-code lifecycle and §6.5.3 for the user-facing onboarding step.

### §22C.22.5 Topology-choice persistence and provider-state

The redesign writes more state at completion than the pre-redesign flow. All rows below are literal `ai_settings` keys.

| Key | Value at SplitMachine completion |
|---|---|
| `provider` | `openclaw` |
| `tier3_topology` | `split_machine` |
| `provider_state` | ~~`configured_verified`~~ **retired (#2649, PR 3A)** — zero readers found codebase-wide; this write has been deleted, row kept for historical context |
| `endpoint` | customer-entered URL |
| `api_key` (encrypted) | from `POST /pair` response `token` field |
| `gateway_tls_fingerprint` | from `/v1/health` `tls_fingerprint` field at probe time |
| `gateway_tls_cert_pem` | from `/v1/health` `tls_cert_pem` field at probe time |
| `paired_at` | timestamp at `POST /pair` 200 |
| `main_agent_id` | per OC's config |
| `last_validation_at` | timestamp |
| `onboarding_resume_state` | `complete` (after Home reached) |

`provider_state = configured_verified` on completion enables chat in Home + fires the welcome message (§6.4.3).

Per Amendment 1 (#1167), `endpoint` and `api_key` are the canonical generic-provider keys (§2.3C) shared by chat and sync layers when `provider = openclaw`. `gateway_tls_fingerprint` and `gateway_tls_cert_pem` are gateway-pairing-specific keys written separately, not through `ai_configure_and_save`.

### §22C.22.6 Failure-mode propagation

Failure modes within onboarding's OC paths use the same error categories as §22C.11A.6 (Settings inline pairing failures). Onboarding's contribution is the *screen* on which each error surfaces - the wire shapes, retry mechanics, and recovery paths come from §22C.

| Onboarding screen | Failure modes (with §22C.11A.6 reference) |
|---|---|

| §6.5.3 step 1 - URL probe | "Unreachable" vs. "reachable but not a gateway" - sub-task #17 distinction. §22C.11A.6 "Can't reach gateway" + new "Not a gateway" error class. |
| §6.5.3 step 2 - fingerprint mismatch | §22C.4.3 hard-block. No Continue path; customer aborts. |
| §6.5.3 step 2 - pairing failure | §22C.11A.6 "Pairing failed" with three sub-cases distinguished by error message (capture §2): expired, invalid code, no active code. UX per §6.5.3 error-mapping table. |
| §6.5.3 step 4 - initial sync failure | New v1 behavior: retry-all-or-fail-pairing per sub-task #21. §22C.11A.6 covers the underlying transport errors; the v1 decision to not partially commit lives in the onboarding spec. |

### §22C.22.7 Cross-references that need the bridge→gateway endpoint update

The redesign assumes gateway endpoints throughout. Several existing §22C sections still carry bridge-endpoint references that pre-date the cutover criteria in §22C.16. Those references continue to be valid for the bridge plugin during the dogfood window but must update to gateway endpoints in any *new* spec narrative. The full retirement of `/bridge/*` endpoints follows §22C.16; this onboarding-integration section assumes the post-cutover state.

Endpoint mapping for §22C.22 narrative:

- `/bridge/push` → `/v1/sync/push` (per §22C.6.2)
- `/bridge/pull` / `/bridge/pull/confirm` → `/v1/sync/pull` (per §22C.6.2)
- `/bridge/sync-all` → `/v1/sync/initial` (per §22C.6.2)
- `/bridge/health` → `/v1/health` (per §22C.4.x)
- `/bridge/models` → direct read of `~/.openclaw/openclaw.json` on SameMachine, `GET /v1/models` through gateway on SplitMachine (per §22C.16 cutover)

### §22C.22.8 Re-pairing after onboarding (Settings)

A customer who completes OC onboarding and later wants to switch to a different gateway (different machine, re-installed OC, etc.) does **not** re-enter §6.5 onboarding screens. The Settings cascade-UX flow (§22C.11A.3) handles provider switching; the inline pairing flow (§22C.11A.5) handles the pairing-code dance within Settings, retaining the customer's session context (open notes, current chat, other tab state) rather than discarding it by re-entering the wizard.

This sub-section is for **first-time** pairing during onboarding only. The redesign explicitly preserves §22C.11A.5 verbatim per Reed's audit - the Settings flow is the template, the onboarding flow is the application of that template in a wizard context.

---

# Appendix A: §22B status change

The PR that lands §22C also updates §22B's status banner:

> **Status:** LEGACY as of 2026-04-27 (spec session). Superseded by §22C (Pane Gateway) for split-machine Tier 3. Bridge plugin remains in production as dev infrastructure during gateway build per §22C.16 cutover criteria. Section retained for historical context only.

No content removed from §22B. Future readers see the lineage.

---

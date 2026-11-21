# Architecture Decision Records

ADRs capture architectural decisions with context, rationale, and consequences. One file per decision, numbered sequentially.

- **Template:** [TEMPLATE.md](TEMPLATE.md)
- **Process:** See the [Dev Hygiene Playbook](../dev-hygiene-playbook.md#2-architecture-decision-records-adrs)

## Index

| ADR | Title | Status | Date |
|-----|-------|--------|------|
| [0001](0001-tauri-over-electron.md) | Tauri over Electron | Accepted | 2026-02-08 |
| [0002](0002-crdts-for-sync.md) | CRDTs for Multi-Device Sync | Accepted | 2026-02-27 |
| [0003](0003-notes-and-tasks-same-model.md) | Notes and Tasks Are the Same Data Model | Accepted | 2026-02-10 |
| [0004](0004-zero-knowledge-encryption.md) | Zero-Knowledge Encryption for Sync | Accepted | 2026-02-27 |
| [0005](0005-db-primary-flat-file-backup.md) | DB as Primary, Flat Files as Backup | Accepted | 2026-04-18 |
| [0006](0006-canonical-schema-no-migrations.md) | Canonical Schema, No Migration Chain (Pre-Launch) | Superseded by ADR-0019 | 2026-04-18 |
| [0007](0007-local-first-architecture.md) | Local-First Architecture | Accepted | 2026-02-08 |
| [0008](0008-mechanical-dev-hygiene.md) | Mechanical Dev Hygiene Over Discipline-Based Rules | Accepted | 2026-04-19 |
| [0009](0009-openclaw-gateway-provider-path.md) | OpenClaw Gateway Provider Semantics | Proposed | 2026-04-21 |
| [0010](0010-three-writer-memory-pipeline.md) | Three-Writer Memory Pipeline | Accepted | 2026-04-23 |
| [0011](0011-system-notes-readonly.md) | System Notes are Read-Only in the Pane Editor | Accepted | 2026-04-24 |
| [0012](0012-pane-gateway-as-primary-sync-transport.md) | Pane Gateway as Primary Sync Transport for Split-Machine Tier 3 | Accepted | 2026-04-27 |
| [0013](0013-auth-and-multi-tenancy-seam.md) | Auth and Multi-Tenancy Seam for the Pane Gateway | Accepted | 2026-04-27 |
| [0014](0014-bridge-vs-supabase-split.md) | Bridge vs Supabase — Two Sync Systems by Design | Accepted | 2026-04-27 |
| [0015](0015-gateway-oc-transport.md) | Gateway↔OC Transport — WS Where Required, HTTP Where Supported | Accepted | 2026-04-27 |
| [0016](0016-mechanical-section-4-enforcement.md) | Mechanical §4 Enforcement | Accepted | 2026-04-29 |
| [0017](0017-pr690-migration-fix-and-pre-launch-reconciliation.md) | PR #690 Migration Fix and Pre-Launch Mode Reconciliation | Proposed | 2026-04-30 |
| [0018](0018-ci-smoke-check-startup-log-severity.md) | CI Smoke Check for Startup Log Severity | Accepted | 2026-04-30 |
| [0019](0019-pre-launch-schema-management-clarification.md) | Pre-Launch Schema Management Clarification | Accepted | 2026-04-30 |
| [0020](0020-project-memory-canonical-path.md) | Project Memory Lives at Projects/{name}/Memory.md | Proposed | 2026-04-30 |
| [0021](0021-notification-trigger-architecture.md) | Event-Driven Notification Triggers and Coordinator-Only AI Nudges | Proposed | 2026-05-02 |
| [0022](0022-notification-state-per-device.md) | Notification State Is Per-Device, Not CRR-Synced | Proposed | 2026-05-02 |
| [0023](0023-external-contract-capture-discipline.md) | External-System Contracts Must Be Captured at Spec Time | Accepted | 2026-05-02 |
| [0024](0024-local-only-decryption.md) | Local-Only Decryption for Pane User Data | Accepted | 2026-05-07 |
| [0025](0025-ecom-check-cadence.md) | Ecom Check Cadence — Launch + On-Focus | Accepted (supplemented by ADR-0036) | 2026-05-07 |
| [0026](0026-install-uuid-active-machine-id.md) | Install UUID for `active_machine_id` Matching | Accepted | 2026-05-07 |
| [0027](0027-pane-ecom-api-contract.md) | Pane ↔ Ecom API Contract for Sign-in and Single-Active-Machine Enforcement | Accepted (superseded in part by ADR-0029) | 2026-05-07 |
| [0028](0028-ecom-platform-selection.md) | Ecom Platform Selection — Supabase Auth + Stripe + Thin Wrapper | Accepted | 2026-05-07 |
| [0029](0029-entitlement-response-shape-and-email-check-privacy.md) | Entitlement Response Shape and Email-Check Privacy Posture | Accepted | 2026-05-08 |
| [0030](0030-paneapp-ai-stripe-return-url-contract.md) | paneapp.ai Stripe Return-URL Contract | Accepted | 2026-05-09 |
| [0031](0031-paneapp-ai-signup-flow.md) | paneapp.ai signup flow — account creation before Stripe Checkout | Accepted | 2026-05-10 |
| [0032](0032-paneapp-ai-stack-and-hosting.md) | paneapp.ai Stack and Hosting — SvelteKit + Cloudflare Pages | Accepted | 2026-05-10 |
| [0033](0033-pane-dmg-distribution.md) | Pane DMG Distribution — paneapp.ai-Hosted, Three Surfaces | Accepted | 2026-05-10 |
| [0034](0034-paneapp-ai-email-verification-posture.md) | paneapp.ai Email Verification Posture — Disabled, Stripe-as-Identity | Accepted | 2026-05-10 |
| [0035](0035-active-machine-mismatch-call-site-rule.md) | Active-Machine Mismatch — Call-Site Rule for UI Disposition | Accepted | 2026-05-10 |
| [0036](0036-subscription-status-routing-matrix-across-surfaces.md) | Subscription Status Routing Matrix Across Pane Surfaces | Accepted | 2026-05-10 |
| [0037](0037-settings-ui-architecture-model-vs-behavior.md) | Settings UI Architecture — Model Selection vs Feature Behavior | Accepted | 2026-05-18 |
| [0038](0038-extraction-model-routing.md) | Extraction Model Routing — Dedicated Setting and Fallback Chain | Accepted | 2026-05-18 |
| [0039](0039-agent-identity-sot.md) | Agent Identity — IDENTITY.md as Single Source of Truth | Accepted | 2026-05-27 |
| [0040](0040-gateway-workspace-resolution.md) | Gateway Workspace Resolution via `agents.list` | Built v0.65.68 | 2026-05-27 |
| [0041](0041-sync-scope-subset-of-persistence-scope.md) | Sync Scope Is a Filtered Subset of Persistence Scope | Accepted | 2026-05-28 |
| [0042](0042-lww-tie-break-pane-wins.md) | LWW Tie-Break Resolves to Pane (Source of Truth) | Accepted | 2026-06-02 |
| [0043](0043-dynamic-context-window-and-percentage-compaction.md) | Dynamic Context Window Sourcing + Percentage-Based Compaction | Accepted | 2026-06-08 |
| [0044](0044-keychain-credential-storage.md) | Keychain Credential Storage Contract | Accepted | 2026-06-17 |
| [0045](0045-is-main-selection-contract.md) | Main-Agent (`is_main`) Selection Contract | Proposed | 2026-06-19 |
| [0046](0046-native-first-tool-call-parsing.md) | Native-first tool-call parsing with a provider-agnostic text fallback | Proposed | 2026-06-22 |
| [0047](0047-library-boundary-callback-pattern.md) | Local pipeline progress reported via callback, not AppHandle injection | Accepted | 2026-06-24 |
| [0048](0048-semantic-search-opt-in.md) | Semantic search is opt-in, multi-engine; no automatic model download | Accepted (amended 2026-06-24) | 2026-06-24 |
| [0049](0049-attachment-html-sandbox-and-link-egress.md) | Attachment HTML Sandbox & Link-Preview Network Egress | Accepted | 2026-06-29 |
| [0050](0050-link-rich-embed-security.md) | Link Rich-Embed Security Stance (third-party inline players) | Accepted | 2026-06-30 |
| [0051](0051-migrations-separate-connection.md) | Migrations Run on a Separate Short-Lived Connection | Proposed | 2026-07-05 |
| [0052](0052-chat-session-identity.md) | Chat Session Identity — `chat_sessions` Table, `thread_id` Becomes Load-Bearing | Accepted | 2026-07-08 |
| [0053](0053-compaction-session-continuity.md) | Compaction Keys on Session Id | Accepted | 2026-07-08 |
| [0055](0055-oc-handshake-timeout-recovery.md) | OC Gateway Handshake-Timeout Recovery Surfacing | Proposed | 2026-07-13 |
| [0056](0056-restore-quiesces-live-db.md) | Restore Quiesces the Live Database Before Swapping It | Proposed | 2026-07-14 |
| [0057](0057-subtasks-are-component-line-items.md) | Subtasks Are Component Line Items, Not Tasks | Proposed | 2026-07-18 |
| [0058](0058-templates-dedicated-table.md) | Templates Are Named Snapshots in a Dedicated Table | Accepted | 2026-07-20 |
| [0059](0059-provider-scoped-agent-usability.md) | Provider-Scoped Agent Usability and Local Agent Creation | Proposed | 2026-07-24 |
| [0060](0060-system-file-canon-rules-retirement.md) | System-File Canon per Provider Class; RULES.md Retired as a Generated Concept | Proposed | 2026-07-28 |
| [0061](0061-smart-routing-global-only.md) | Smart Routing Is Global-Only — Per-Project Routing Columns Retired | Accepted | 2026-08-16 |
| [0062](0062-per-session-agent-binding.md) | Per-Session Agent Binding | Proposed | 2026-08-16 |
| [0063](0063-runtime-per-agent.md) | Runtime-Per-Agent — Global Provider Mode Retired | Proposed | 2026-08-16 |
| [0064](0064-web-browsing-tools.md) | Web Browsing Tools (`web_fetch`, `web_search`) | Proposed | 2026-08-27 |
| [0065](0065-venice-runtime.md) | Venice as a Pinnable Runtime | Proposed | 2026-08-29 |
| [0066](0066-sheets-typed-notes-sparse-grid.md) | Sheets Are Typed Notes with Sparse Grid Side Tables | Proposed | 2026-09-03 |
| [0067](0067-explicit-agent-selection-on-the-oc-wire.md) | Explicit Agent Selection on the OC Wire | Accepted | 2026-09-21 |
| [0068](0068-pane-runtime.md) | Pane Runtime — Scheduler, Jobs, Spawn and Wake | Proposed (amended 2026-10-26) | 2026-10-25 |
| [0069](0069-image-generation-via-provider-connections.md) | Image generation goes through the user's provider connection, into Assets, placed by the brief | Proposed | 2026-11-03 |
| [0070](0070-local-first-launch-account-behind-flag.md) | Pane opens to Home with no sign-in; account and sync stay behind a default-off flag | Proposed | 2026-11-09 |

**Note on numbering:** `0049`, `0050`, `0057`, and `0054` each also have a `-session-notes.md` file sharing that number (e.g. `0049-session-notes.md` alongside `0049-attachment-html-sandbox-and-link-egress.md`) — these are raw spec-session scratch notes, not ADRs in their own right (each is explicitly "non-normative" or superseded by the real, numbered ADR it informed), so they are intentionally not rows in this table. `0054` has no standalone ADR at all, only its session-notes file — not a gap to fill, just a number that never got a formal decision document.

This table was completed (0024–0057, 0059–0068 added) in #3161 PR 1, 2026-11-03 — it had silently stopped at 0058 against 72 real files on disk. A follow-up issue (not built in this sprint — unrelated to image generation) proposes a CI check scanning `docs/adr/*.md` filenames against this table, the same self-correcting shape `tool_patterns_registry_sync_tests` already uses for tool-name coverage, so it can't rot again.

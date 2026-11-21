# ADR-0040: Gateway Workspace Resolution via `agents.list`

*Line citations predate the #3097 module split.*

**Status:** BUILT v0.65.68
**Date:** 2026-05-27
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** None (closes a gap left at gateway implementation time; supersedes the `get_agent_workspace()` placeholder at `src-gateway/src/routes/sync.rs:264-280`)
**Supplements:** ADR-0012 (Pane Gateway as primary sync transport), ADR-0014 (Bridge vs Supabase split - gateway owns sync), ADR-0015 (Gateway↔OC transport - RPC-only boundary), ADR-0023 (External-system contracts must be captured at spec time - this ADR's §22C.6.4 capture extension is an instance)
**Related:** `docs/spec/sync.md` §22C.6.2 (`/v1/sync/initial`), §22C.6.4 (`agents.list` capture), §22C.8.2 (LWW conflict resolution)

## Context

Agent system files (SOUL.md, MEMORY.md, IDENTITY.md, and the rest of the §6.6.4 whitelist) don't pull from gateway to Pane on fresh SplitMachine OC onboard. Agent folders are created on the Pane machine, but contents are empty stubs or missing entirely. Bug surfaced during fresh OC onboard testing on v0.65.64.

Source-read at `src-gateway/src/routes/sync.rs:264-280` confirmed root cause. `get_agent_workspace()` is a hardcoded placeholder:

```
fn get_agent_workspace(tenant_id: &str, oc_agent_id: &str) -> PathBuf {
    // TODO: This needs to be updated with actual OC workspace location logic.
    home_dir().join(".openclaw").join("workspaces").join(tenant_id).join(oc_agent_id)
}
```

The function is called from both `/v1/sync/push` (line 102) and `/v1/sync/initial` (line 234). The path it returns does not match where OC actually stores agent files - the path is determined per-agent by OC's `openclaw.json` config and surfaced in OC's `agents.list` response. The placeholder has been in place since the endpoints were created (not a regression).

This is the same shape as ADR-0023's external-contract-capture failure mode at a different scope. The §22C.6.4 spec capture for `agents.list` documents that the gateway forwards `id → oc_agent_id`, `name`, `model.primary → model`, and `role` to Pane. OC's actual response includes more fields than the capture enumerates - `workspace` among them (confirmed by source-read of `session-utils-hDe0hdis.js:413-455`, `listAgentsForGateway(cfg)`, line 449: `workspace: resolveAgentWorkspaceDir(cfg, id)`). The gateway code parses OC's response into a struct shaped after the Pane-facing capture and discards everything else, including `workspace`. The capture was incomplete; the implementation followed the capture; the resulting bug is invisible until a downstream consumer (sync's path resolution) needs the field that was stripped.

Three foundations are verified and load-bearing for this ADR:

1. **`/v1/sync/initial` is already spec'd at `sync.md:1009`** (§22C.6.2, 4th row of the endpoint table). The endpoint contract is documented; the path-resolution mechanism is not. The amendment scope is "spec the resolution mechanism," not "add the endpoint."
2. **`workspace` field exists in OC's `agents.list` response** at `session-utils-hDe0hdis.js:449`. The gateway has access to it; it just discards it.
3. **Path A - RPC-only boundary - is locked.** Per ADR-0015, the gateway absorbs OC's protocol asymmetry over RPC. Reaching into OC's filesystem (`openclaw.json`) to derive workspace paths would couple the gateway to OC's on-disk layout - a layer violation that creates the same external-contract surface ADR-0023 was filed to bound. The gateway uses OC's RPC surface; if a field isn't there, the fix is to extend the RPC contract, not work around it.

What if we didn't decide:

- **Bug persists.** Fresh OC onboards continue to produce empty stubs on the Pane side, breaking OC's value prop for every new user.
- **`get_agent_workspace()` gets patched ad-hoc.** Without an ADR pinning the resolution mechanism, the next reader is free to introduce a filesystem read or a hardcoded mapping table; the layer violation creeps in by way of fix.
- **§22C.6.4 capture stays incomplete.** The next external-system field the gateway needs from `agents.list` (or another OC method whose capture is similarly truncated) hits the same "the field exists in OC but isn't in the capture" gap. ADR-0023's discipline is a checkpoint; this is a recurring instance of the same gap, and not naming it as such lets the next instance surface as another launch-blocking bug.

## Decision

**The gateway resolves agent workspace paths from OC's `agents.list` response.** Specifically:

1. **`agents.list` is the single source of truth for per-agent workspace paths.** The gateway's `agents.list` invocation parses the `workspace` field from OC's response (per the extended §22C.6.4 capture) and uses it for every workspace-path resolution. No `openclaw.json` filesystem read on the gateway machine. No hardcoded path templates. No platform-specific path construction beyond what OC itself already did when it computed the `workspace` value.

2. **Path resolution happens per-call, not via startup cache.** Each `/v1/sync/push` and `/v1/sync/initial` invocation that needs a workspace path triggers (or relies on a recent cached) `agents.list` call. The gateway already maintains an agent registry (§22C.10); the `workspace` field is added to that registry's per-agent state. Cache lifetime is the existing registry's lifetime (refresh on agent-set change, on reconnect, or per the existing TTL - implementation detail, not spec'd here). Startup-cache-only is rejected as a design because OC operators can edit `openclaw.json` (workspace path) at runtime; the gateway needs to pick up changes without restart.

3. **`openclaw.json` is OC-owned and never read by the gateway.** The file lives on the OC machine at `~/.openclaw/openclaw.json`; it is OC's configuration artifact. The gateway treats `openclaw.json` as opaque internal state - same trust boundary as any other OC implementation detail. If OC moves to a different config format or location, the gateway is unaffected because the gateway depends on OC's `agents.list` response, not on OC's on-disk representation. This is the load-bearing application of ADR-0015's RPC-only boundary.

4. **Error handling for missing/malformed workspace data.**

   | Failure mode | Gateway behavior |
   |---|---|
   | `workspace` field absent from `agents.list` response for an agent | Log structured error including the `oc_agent_id` and OC version. Return 500 to Pane for any push/initial-sync call that targets that agent. Do not fall back to a hardcoded path; do not silently skip the agent. |
   | `workspace` field present but value is empty string or null | Same as above. The gateway does not invent a default. |
   | `workspace` value present but path does not exist on the OC machine's filesystem when the gateway attempts to write | OC owns workspace directory creation. Gateway logs the I/O error from the underlying write, returns 500 to Pane. Pane surfaces "Couldn't write to OpenClaw workspace" inline per §22C.11A.6's error-state pattern. |
   | `workspace` value present but path exists with permissions the gateway process can't read/write | Same as above. The gateway does not chmod or otherwise attempt remediation. |
   | `agents.list` call itself fails (network, auth, OC unavailable) | Existing §22C.12.2 OC error handling applies - gateway maps to appropriate Pane-facing status per the gateway↔OC error table. Path resolution is blocked; push/initial-sync return the upstream error. |

   Client-side behavior on each: Pane treats a 500 from sync endpoints as transient-error → exponential backoff per §22C.8.3, max 5 retries, then drop and log. Pane does not invent a stub; missing workspace = missing data, not "create empty file."

5. **Sub-agent workspace resolution.** OC's `agents.list` returns workspace paths for every agent in the registry, not just the main agent. Sub-agents (researcher, coder, designer, etc., per `Pane_Onboarding_Dev_Dispatch__2026-05-06.md` and the agent registry §22C.10) get their workspace paths resolved by the same mechanism. The gateway iterates the response and resolves per-agent; no special-case for main vs sub. Symlinks at the workspace path are followed transparently by the OS during read/write - the gateway does not resolve symlinks explicitly; `tokio::fs::write` and friends follow the symlink. Timestamps used for §22C.8.2 LWW comparison come from the file the symlink points to, which is the correct behavior (the symlink is a transport mechanism, not a logical content boundary).

6. **`tenant_id` handling at v1.** Per ADR-0012's multi-tenancy seam: at launch, `tenant_id = "default"`. The gateway's workspace resolution does not embed `tenant_id` in the path returned by `agents.list` - that's OC's responsibility (OC's `resolveAgentWorkspaceDir(cfg, id)` is per-tenant when OC is configured for multi-tenancy; single-tenant OC returns a single-tenant path). The gateway passes `tenant_id` to OC's `agents.list` via the existing auth context (§22C.5.1) and uses whatever workspace path OC returns for that tenant. **No change to the multi-tenancy seam.** Tier 1 flip (per ADR-0012) introduces real per-tenant resolution; this ADR's mechanism carries forward because the gateway is already passing `tenant_id` and is already trusting OC's per-tenant workspace path.

7. **Migration / backward compatibility - stub-detection and forced re-sync.** Existing OC users who paired pre-fix have empty-stub content in `.pane/system/{agent}/` on their Pane install (per the bug symptoms). The integrity check wrote stubs with recent timestamps; real files on the OC side have older timestamps. Naive LWW (§22C.8.2) would keep the stubs.

   The migration trigger:

   - **First launch after the fix lands**, if `ai_settings.workspace_resolution_migration_complete` is unset and `provider = openclaw` and `tier3_topology = split_machine`: Pane invokes `/v1/sync/initial` against the gateway. The gateway resolves workspaces per Decision 1 and returns real content for every agent. Pane writes the response to disk **with an explicit override of §22C.8.2's LWW rule**: real content always wins over stubs regardless of timestamp.
   - **Stub detection** uses content-identity. The stub predicate is: `content.trim().is_empty() || content.contains("auto-created by integrity check")`. This reuses the existing IDENTITY.md stub-detection pattern at `commands.rs:7683-7684`. The marker string appears in all three integrity-check stub templates:
     - SOUL.md (`commands.rs:7665`): `*(This file was auto-created by integrity check. Please update with agent personality and directives.)*`
     - MEMORY.md (`commands.rs:7673`): `*(This file was auto-created by integrity check. Agent memories will be written here.)*`
     - IDENTITY.md (`commands.rs:7687`): detected via `trimmed.contains("auto-created by integrity check")` at line 7684; the ADR-0039 replacement content (`# {name}\nYour name is {name}.\n`) does NOT contain the marker.

     Edge cases: empty files are stubs (migration overwrites); whitespace-only files `trim()` to empty → stubs; false-positive risk is near zero — the marker is machine-generated boilerplate no user would write. Match is case-sensitive, ASCII, substring (not prefix or full-content).

     If a local file's content is non-stub (does not match the predicate), the migration preserves the local file unchanged. This is a conservative approximation of §22C.8.2's LWW rule — without reliable mtime tracking on system files during migration, preserving local is the safer default (zero data-loss risk for user edits). True timestamp-based LWW for migrated files is a post-launch enhancement.
   - On migration completion, `ai_settings.workspace_resolution_migration_complete = true`. Subsequent launches skip the migration trigger and operate on normal LWW.
   - **Migration is one-shot per install.** If a user has multiple Pane installs (multi-machine), each install runs its own migration on first launch after fix.

   The migration trigger lives in Pane (not the gateway), because the gateway has no concept of "is the Pane install in a pre-fix state." Pane's first-launch logic checks the flag and runs the trigger.

## Rationale

### Why `agents.list` and not `openclaw.json`

ADR-0015 established that the gateway talks to OC over RPC, not filesystem. The gateway's role is to absorb OC's protocol asymmetry behind a clean Pane-facing surface; reaching into OC's on-disk state for workspace paths is a layer violation that would create a parallel coupling path between gateway and OC, with the corresponding maintenance burden every time OC changes its config format. The RPC contract is already there (`workspace` exists in `agents.list`); the gateway just needs to stop discarding the field. Path A is the architecturally consistent answer; Path B (filesystem read) is the expedient one. The expedient one has the higher long-term cost.

### Why per-call (with cache) instead of startup-cache-only

Startup-cache-only fails on a real operator workflow: OC operator edits `agents.defaults.workspace` in `openclaw.json` to relocate the workspace, restarts OC, and expects the gateway to pick up the change. If the gateway caches at gateway startup only, the operator has to restart the gateway too. That's the same kind of operator burden the §22C.5 pairing flow tried to eliminate with the on-demand `pane-gateway pairing-code` subcommand. Per-call lookup against the existing agent registry's refresh cycle gives correctness without an extra restart step.

### Why surface missing `workspace` as 500 rather than fall back to a default

A default path (the current `~/.openclaw/workspaces/{tenant_id}/{oc_agent_id}/` placeholder) is exactly what produced this bug. Silent fallback to "some path the gateway invents" creates two failure modes: (a) the path doesn't match where OC writes, and the bug recurs; (b) the path *does* happen to match for some configurations, hiding the structural issue until a different configuration surfaces it. Surface-as-500 makes the failure visible to Pane, which logs it with structured context, and gives Reed something to audit against. The current placeholder is the worst of both worlds - silently wrong for the configurations it doesn't match, silently correct for the ones it does.

### Why stub-detection migration is content-based, not timestamp-based

A pure timestamp-based migration ("if local file is older than gateway file, pull") is the inverse of LWW (§22C.8.2) and would be wrong: a user who edited SOUL.md on the Pane side post-stub-write should keep their edit, not have it overwritten. Content-based stub detection identifies the specific pathological case (file content matches integrity-check's stub pattern) and migrates only those. Real edits are preserved by definition because their content doesn't match the stub pattern. The migration override of LWW is narrow and conditional.

### Why this ships as ADR-0040, not as a §22C.6.2 spec touch alone

The decision has three architectural layers - capture extension (ADR-0023's discipline applied), resolution mechanism (Path A vs Path B), and migration override (LWW exception for stubs). A spec touch would record the mechanics but not the reasoning; the reasoning needs to survive into post-launch because (a) the next external-system field the gateway needs from OC will hit the same capture-extension question, and (b) the next migration-shaped fix will hit the same LWW-override question. ADR-0040 is the durable record of the answers.

### Process pattern (cross-reference)

ADR-0038's amendment named the pattern: "a fix that didn't carry across an architectural transition" (issues #1167 / #1176 / #1177). This bug is another instance - the gateway implementation transition (bridge → gateway) left `get_agent_workspace()` as a placeholder, and no checkpoint caught that the placeholder was hardcoded against an OC layout the gateway never confirmed. The ADR-0023 capture-extension discipline catches the "external-system shape" instance of this pattern; the broader pattern - implementation-transition gaps - remains a recurring audit target. Filed as a post-launch playbook addition (see "Follow-ups filed" below).

## Consequences

### Positive

- **Bug fixed.** Fresh OC onboards produce real content on the Pane side. Existing pre-fix installs auto-migrate on first launch after the fix lands.
- **`/v1/sync/initial` and `/v1/sync/push` use the same resolution mechanism.** No divergence between push and pull paths - both use `agents.list`'s `workspace` field via the agent registry. The two endpoints stay symmetric.
- **ADR-0015's RPC-only boundary holds.** No filesystem coupling introduced. The gateway remains the layer that knows OC's protocol; OC's on-disk layout remains OC's concern.
- **§22C.6.4 capture becomes complete for the field this ADR needs.** Future readers see `workspace` in the capture; ADR-0023's discipline is reinforced by this concrete instance.
- **Migration is bounded and one-shot.** Stub-detection narrows the override to the specific case; users with real edits keep their edits.
- **Sub-agent support comes for free.** No special-case code for main vs sub; the same mechanism resolves all agents.

### Negative

- **§22C.6.4 capture extension is one more field for Reed to verify against OC versions.** ADR-0023's versioning discipline applies - captures rot. The `workspace` field's stability across OC versions is treated as "current as of OC v2026.4.X" with re-verification cadence per ADR-0023. Mitigated by the existing audit cadence (§826-shape sweeps) covering `agents.list` anyway.
- **Migration adds a one-shot code path in Pane.** Stub detection is small but real code; the `workspace_resolution_migration_complete` flag is a new `ai_settings` key. Mitigated by being deletable post-launch once all existing installs have migrated - Stia files a cleanup issue for a future release to remove the migration trigger and the flag.
- **Per-call lookup costs one extra registry hit per push.** The registry already exists for chat purposes; the marginal cost is a hashmap lookup, not a network call (the registry caches `agents.list` per its existing TTL). Cost is negligible.
- **A `workspace` field that exists in OC but is empty/null now fails loudly.** Pre-fix, the hardcoded path always returned something (possibly wrong but never empty). Post-fix, a misconfigured OC produces a visible error. This is the right behavior - silent wrongness is worse than loud failure - but it's a behavioral shift for any operator who had OC misconfigured and didn't notice.

### Risks

- **OC changes the `workspace` field name or moves it to a different location in `agents.list`.** Mitigated by ADR-0023's capture extension (which versions the field's source) and by Reed's `agents.list` audit cadence catching shape drift before it ships. Failure mode if the audit misses: gateway falls into the missing-`workspace`-field branch of Decision 4, returns 500s, bug is loud, fix is to re-verify against current OC and update the capture.
- **A stub pattern that doesn't match what `verify_agents_integrity()` actually writes.** The migration override depends on stub detection being accurate. Mitigated by defining the stub pattern against current `verify_agents_integrity()` source rather than a guess; Stia verifies the pattern matches against a fresh-install test fixture before merging the implementation PR.
- **An existing user with real content that happens to match the stub pattern.** Edge case - content-identity false positive. Mitigated because the stub pattern is short and specific (placeholder text rather than valid SOUL.md content); the probability of a real user's edit matching the stub byte-for-byte is functionally zero. Accepted risk.
- **`tenant_id` semantics change at Tier 1 flip.** This ADR's mechanism assumes OC's `agents.list` response already accounts for `tenant_id` - true at v1 (single tenant), true at Tier 1 if OC's multi-tenancy passes the tenant context through. Mitigated by the structural guarantee: the gateway passes `tenant_id` to `agents.list`'s auth context already; the workspace field comes back per-tenant by OC's contract. If Tier 1 breaks this, the break is in OC, not in this ADR's mechanism. Out of scope.
- **Gateway agent registry refresh lag.** A workspace path that changed via OC config update may not be picked up until the next registry refresh. Mitigated by the existing registry's TTL being short enough for operator workflows (per §22C.10). If TTL turns out to be too long for operator expectations, that's a §22C.10 amendment, not this ADR.

## Alternatives Considered

**Path B - Gateway reads `openclaw.json` directly.** The expedient option: gateway reads `~/.openclaw/openclaw.json` on demand or at startup, parses the workspace paths per agent, uses them for resolution. **Rejected** because it violates ADR-0015's RPC-only boundary. The gateway-to-OC contract is OC's RPC surface, not OC's on-disk layout. Filesystem coupling creates a parallel maintenance surface (OC moves the config, gateway breaks; OC changes the schema, gateway breaks). The expedient cost is a one-time fix; the architectural cost is every future change to OC's config format. Path A is the same effort with no architectural debt.

**Hardcoded resolver table maintained in gateway config.** Gateway ships with a config file mapping `oc_agent_id` to workspace path; operator edits the gateway config to match their OC setup. **Rejected** because (a) it's the same kind of placeholder that produced this bug, just slightly more flexible; (b) it requires operators to maintain two config files (`openclaw.json` and gateway config) in sync, which is exactly the kind of split-state operator burden the gateway is supposed to absorb; (c) any drift between the two configs produces the same bug shape we're fixing.

**Add a new RPC method `gateway.workspace.get(agent_id)` to OC.** Cleaner separation than reusing `agents.list` - a dedicated RPC for workspace resolution. **Rejected** because OC already exposes the field via `agents.list`; adding a new RPC method for the same data is duplication. The right fix is to use what's already there, not to extend OC's surface area for our convenience.

**Startup-cache-only with manual refresh endpoint.** Gateway caches workspaces at startup; operator triggers a `/v1/admin/refresh` endpoint when they change `openclaw.json`. **Rejected** because it adds an admin endpoint and an operator burden for what should be transparent. Per-call resolution against the existing registry's TTL gives correctness without the operator step.

**Timestamp-only migration (no stub detection).** Migration runs once on first-launch post-fix, pulls everything, overwrites local content unconditionally. **Rejected** because it would overwrite real user edits made between stub-write and migration. Stub-detection narrows the override to the bug's actual surface area.

**No migration - let users manually re-onboard.** Existing installs detect the stub state, surface "Re-pair gateway" prompt, user re-runs onboarding which re-runs initial sync (with the fix in place). **Rejected** because (a) it requires every existing user to take action for a bug that wasn't their fault; (b) the re-pair flow doesn't actually trigger initial-sync overwrite in current §22C.22.4 - it would need a separate spec amendment to allow stub overwrite on re-pair. Automatic stub-detection migration is less work overall and produces a better user experience.

## References

- Issue #1278 - gateway workspace path placeholder produces empty stubs on fresh OC onboard
- Bug source-read: `src-gateway/src/routes/sync.rs:264-280` - `get_agent_workspace()` placeholder
- Spec source-read: `sync.md:1009` - `/v1/sync/initial` row in §22C.6.2's endpoint table
- OC source-read: `session-utils-hDe0hdis.js:413-455` - `listAgentsForGateway(cfg)`; line 449 = `workspace: resolveAgentWorkspaceDir(cfg, id)`
- ADR-0012 - Pane Gateway as primary sync transport (gateway owns sync layer)
- ADR-0014 - Bridge vs Supabase split (gateway is the sync layer)
- ADR-0015 - Gateway↔OC transport (RPC-only boundary; load-bearing for Path A)
- ADR-0023 - External-system contracts must be captured at spec time (§22C.6.4 capture extension is an instance)
- ADR-0038 amendment - process pattern reference ("a fix that didn't carry across an architectural transition")
- `docs/spec/sync.md` §22C.6.2 - `/v1/sync/initial` endpoint contract (amended by this ADR to specify path-resolution mechanism)
- `docs/spec/sync.md` §22C.6.4 - `agents.list` capture (amended by this ADR to include `workspace` field)
- `docs/spec/sync.md` §22C.8.2 - LWW conflict resolution (migration trigger overrides for stub case only)
- `docs/spec/sync.md` §22C.10 - Agent registry (the cache layer this ADR's per-call resolution uses)
- `docs/spec/sync.md` §22C.11A.6 - Error-state pattern (migration and resolution errors surface inline per this pattern)
- `docs/spec/sync.md` §22C.12.2 - Gateway↔OC error handling (existing handling applies when `agents.list` itself fails)

## Supplement — #2674: `workspace_resolution_migration_complete_v2` flag semantics (2026-08-23)

Production defect: `migration_workspace_policy()` ran a redundant, incorrectly-scoped
`std::fs::canonicalize()` on the raw `workspace_path` (a bare UUID, not a full path)
in addition to the correct `PaneFs::authorize_existing()` check documented in
Decision 1/7 above. The redundant check always failed for bare-UUID inputs
(the production shape), rejecting all 107/107 migration files on every
startup with "Workspace is not canonically beneath PaneRoot::System" — while
`apply_migration_files()` unconditionally set the completion flag to `true`
regardless of `err_count`, permanently masking the total failure.

Fix (Issue #2674):

1. **Path resolution simplified.** `migration_workspace_policy()` now relies
   solely on `PaneFs::authorize_existing(root, &relative, ExistingKind::Directory)`
   — the mechanism Decision 1/7 always intended — and no longer duplicates
   canonicalization against the process cwd.
2. **Completion flag is gated on `err_count == 0`.** `apply_migration_files()`
   only stamps the completion flag when every file in the batch succeeded.
   Any error (including a total-failure run) leaves the flag unset, so
   `should_run_workspace_migration()` retries automatically on the next
   startup instead of permanently marking a failed run as done.
3. **Flag key versioned to `workspace_resolution_migration_complete_v2`.**
   Installs that already have the old (unversioned)
   `workspace_resolution_migration_complete = "true"` key stamped by the
   pre-fix unconditional-set bug will have the new `_v2` key unset, so the
   migration retries once and self-heals without manual DB intervention.
   This follows the same versioned-flag precedent used by
   `sync_master_key_v1` (ADR-0044 Addendum A, #1498). A blanket re-run for
   all `split_machine` + `openclaw` installs is accepted — `is_stub_file`
   detection makes the re-run a no-op for installs whose original migration
   already succeeded.

This supplement does not change Decision 1/7's design intent (bare-UUID
workspace resolution via `agents.list`'s workspace field, strict-vs-conditional
completion semantics) — it corrects an implementation bug in the path-check
and closes the retry gap the unconditional flag left open.

## Follow-ups filed (not gated by this ADR)

- **Migration cleanup post-launch.** Once all existing pre-fix installs have run the migration (track via telemetry on `workspace_resolution_migration_complete` flag uptake), file a cleanup PR removing the migration trigger and the flag. Conservative window: 60-90 days post-launch, gated on telemetry showing >99% of active installs flag-set.
- **Implementation-transition-gap audit.** ADR-0038's amendment named the broader pattern ("a fix that didn't carry across an architectural transition"). This ADR is another instance. Audit candidates: any code path that crossed the bridge → gateway transition with a TODO, placeholder, or hardcoded value. Reed-led sweep filed as a separate post-launch issue; this ADR's role is to name the pattern and ship the immediate fix, not to do the broader sweep.
- **`agents.list` capture completeness sweep.** §22C.6.4's current capture lists 4 forwarded fields; OC's actual response (per source-read) includes more. Reed-led capture-extension sweep filed against ADR-0023's discipline - verify every field OC returns is either forwarded with documented mapping, explicitly discarded with documented reason, or flagged for extension. Not gated by this ADR's launch.
- **`workspace_resolution_migration_complete` schema definition.** The `ai_settings` key needs to be added to the canonical key list (per §2.3C); Stia confirms during implementation whether a separate spec touch is required or if the existing schema accepts the addition without amendment.

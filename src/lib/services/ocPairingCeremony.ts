/**
 * ocPairingCeremony.ts — shared OC pairing ceremony (PR 3A, #2649).
 *
 * Extracts the OC pairing sequence that previously lived, duplicated with
 * subtle drift, in two places:
 *   - Onboarding.svelte's `exchangePairingCode()` + `runOcSync()`
 *   - ProviderConnectionCard.svelte's `completePairing()` (Settings)
 *
 * Canonical order per docs/spec/sync.md §22C.22.4 / §22C.22.8:
 *
 *   1. gateway_exchange_pairing_code(url, code) -> token
 *   2. set_ai_setting('gateway_tls_fingerprint', fingerprint)
 *   3. set_ai_setting('gateway_tls_cert_pem', certPem)
 *   4. set_ai_setting('tier3_topology', 'split_machine')
 *   5. ai_configure_and_save({ provider: 'openclaw', apiKey: token, model: null, endpoint: url })
 *   6. set_ai_setting('gateway_paired_at', new Date().toISOString())
 *   7. First-pair vs re-pair routing (#2647/#2648 D18 fix, ported verbatim from
 *      ProviderConnectionCard.svelte's completePairing — see that component's
 *      module doc for the full rationale):
 *        - oc_agent_row_exists() === false -> sync_initial_import (first pair)
 *        - oc_agent_row_exists() === true  -> check_oc_agent_collisions, then
 *          confirmCollision is ALWAYS prompted on a re-pair (D20, #2649 —
 *          unconditionally, even with zero collisions), then reimport_oc_agents
 *          only runs if the user confirms. A decline resolves as
 *          `{ ok: true, declined: true }` — structurally distinct from both a
 *          successful reimport and an error; see `ImportResult.declined`.
 *
 * `ensureMainAgent` is deliberately OUTSIDE this module — both callers run
 * their own agent-creation/routing after the ceremony resolves (Onboarding's
 * ensureMainAgent creates a name/screen flow; Settings doesn't call it at all,
 * `onChange()` reloads the roster instead). See #2649's root-cause writeup for
 * why `ensureMainAgent` running BEFORE the ceremony's DB rows exist is exactly
 * the bug this split avoids — callers must only run `ensureMainAgent()` AFTER
 * `runOcPairingCeremony`/`runOcImportPhase` resolves `ok: true`, so it always
 * sees the freshly-imported row.
 *
 * Two exported phases, not just one combined function, because pairing codes
 * are one-time-use (docs/spec/onboarding.md §6.6.3 pairing-code semantics:
 * "Code is one-time-use; on success it's invalidated"). Onboarding's existing
 * "Retry" button on the sync-failure screen re-attempts ONLY the import step
 * using the token already obtained in phase 1 — it must never re-submit the
 * (now-consumed) pairing code. Collapsing everything into a single function
 * that always starts from `gateway_exchange_pairing_code` would break that
 * retry path (a real behavior change), so:
 *
 *   - `runOcPairingExchange(...)`  — phase 1 only (steps 1-6). Returns the
 *     token + versionWarning, or a failure.
 *   - `runOcImportPhase(...)`      — phase 2 only (step 7, first-pair/re-pair
 *     routing). Takes the token directly — this is what Onboarding's Retry
 *     button calls, and what a caller re-invokes on a sync-only failure.
 *   - `runOcPairingCeremony(...)`  — convenience wrapper = exchange, then
 *     (on success) import. This is what Settings' `completePairing()` uses,
 *     since Settings has no retry-only-sync affordance (a sync failure there
 *     surfaces as a non-blocking `pairSyncError` badge, not a retry button).
 */
import { invoke } from '@tauri-apps/api/core';
import { mapPairingError } from '$lib/utils/onboarding-errors';

export type AgentCollision = {
  oc_agent_id: string;
  local_name: string;
  gateway_name: string;
};

export type PairingExchangeResult = {
  token: string;
  versionWarning: string | null;
};

export type ImportResult = {
  importedAgentNames: string[];
  agentsMissingModel: string[];
  isFirstPair: boolean;
  /** D20 (#2649): `true` when the user explicitly declined the re-pair
   *  confirmation dialog. Distinct from a successful reimport with zero
   *  agents — callers must branch on this before treating `ok: true` as
   *  "agents were (re)imported". Always `false` on a first pair (no
   *  confirmation is prompted). Omitted/`undefined` is equivalent to `false`. */
  declined?: boolean;
};

export type PairingResult = PairingExchangeResult & ImportResult;

export type StructuredError = {
  errorCode: string | null;
  errorMessage: string;
};

export type PairingExchangeOutcome =
  | ({ ok: true } & PairingExchangeResult)
  | ({ ok: false } & StructuredError);

export type ImportPhaseOutcome =
  | ({ ok: true } & ImportResult)
  | ({ ok: false } & StructuredError);

export type PairingCeremonyOutcome =
  | ({ ok: true } & PairingResult)
  | ({ ok: false; phase: 'pairing' | 'sync' } & StructuredError);

/**
 * Normalize the reviewed gateway origin before any pairing invoke or persistence.
 * Rust performs the authoritative validation again at the native trust boundary.
 * Private, loopback, LAN, and Tailscale hosts are intentionally allowed.
 */
export function normalizeOcGatewayUrl(raw: string): string {
  let candidate = raw.trim();
  if (!candidate) throw new Error('Enter a gateway URL.');

  if (/^http:\/\//i.test(candidate)) {
    throw new Error('Gateway URL must use HTTPS.');
  }
  if (/^[a-z][a-z\d+.-]*:\/\//i.test(candidate) && !/^https:\/\//i.test(candidate)) {
    throw new Error('Gateway URL must use HTTPS.');
  }
  if (!/^https:\/\//i.test(candidate)) candidate = `https://${candidate}`;

  let parsed: URL;
  try {
    parsed = new URL(candidate);
  } catch {
    throw new Error('Enter a valid HTTPS gateway host or IP address with an optional port.');
  }

  const authority = candidate.split('://', 2)[1]?.split(/[/?#]/, 1)[0] ?? '';
  if (parsed.username || parsed.password || authority.includes('@')) {
    throw new Error('Gateway URL must not include credentials.');
  }
  if (parsed.search || candidate.includes('?')) {
    throw new Error('Gateway URL must not include a query string.');
  }
  if (parsed.hash || candidate.includes('#')) {
    throw new Error('Gateway URL must not include a fragment.');
  }
  if (!['/', '/v1', '/v1/'].includes(parsed.pathname)) {
    throw new Error('Gateway URL path must be empty or /v1.');
  }

  if (!parsed.port) parsed.port = '18789';
  parsed.pathname = '/v1';
  return parsed.toString();
}

/**
 * Parse a gateway error string from Rust's GatewayFetchError Display output.
 * When an error code is present, the string is JSON:
 * `{"error_code":"...","message":"..."}`. Otherwise it is a plain
 * human-readable message. Returns `{ code, message }` always.
 *
 * Moved here verbatim from Onboarding.svelte (previously the only caller that
 * parsed this shape) so this module's sync-phase error handling and any other
 * structured-error consumer share one parser instead of each re-implementing
 * the same JSON-detection logic.
 */
export function parseGatewayError(raw: string): { code: string | null; message: string } {
  if (raw.startsWith('{')) {
    try {
      const parsed = JSON.parse(raw) as { error_code?: string; message?: string };
      return { code: parsed.error_code ?? null, message: parsed.message ?? raw };
    } catch {
      // fall through to plain
    }
  }
  return { code: null, message: raw };
}

/**
 * Phase 1 — pairing-code exchange + settings persistence (steps 1-6 above).
 * Never throws; failures resolve to `{ ok: false, errorCode: null, errorMessage }`
 * with `mapPairingError`'s customer-facing copy (matches both callers' prior
 * pairing-code-error handling — `gateway_exchange_pairing_code` never returns
 * the JSON error-code envelope, only plain messages, so `errorCode` is always
 * `null` on this phase).
 */
export async function runOcPairingExchange(args: {
  url: string;
  code: string;
  fingerprint: string;
  certPem: string;
}): Promise<PairingExchangeOutcome> {
  const { url, code, fingerprint, certPem } = args;
  try {
    const canonicalUrl = normalizeOcGatewayUrl(url);
    const pairResult: { token: string; gateway_version: string; version_warning: string | null } =
      await invoke('gateway_exchange_pairing_code', { url: canonicalUrl, code });
    const token = pairResult.token;
    const versionWarning = pairResult.version_warning ?? null;

    await invoke('set_ai_setting', { key: 'gateway_tls_fingerprint', value: fingerprint });
    await invoke('set_ai_setting', { key: 'gateway_tls_cert_pem', value: certPem });
    // Fold-in: matches ProviderConnectionCard.svelte's completePairing note —
    // written unconditionally for both callers now (parity fix, #2649 scope).
    await invoke('set_ai_setting', { key: 'tier3_topology', value: 'split_machine' });
    await invoke('ai_configure_and_save', {
      provider: 'openclaw',
      apiKey: token,
      model: null,
      endpoint: canonicalUrl,
    });
    await invoke('set_ai_setting', { key: 'gateway_paired_at', value: new Date().toISOString() });

    return { ok: true, token, versionWarning };
  } catch (e: any) {
    const raw = e?.message || String(e);
    return { ok: false, errorCode: null, errorMessage: mapPairingError(raw) };
  }
}

/**
 * Phase 2 — first-pair/re-pair import routing (step 7 above). Takes an
 * already-obtained token directly, so this is safe to call on its own for a
 * sync-only retry (no pairing code involved, nothing re-consumed).
 *
 * #2647/#2648 D18 fix: branches on DB row-existence (`oc_agent_row_exists`),
 * NOT `info.connected` — see ProviderConnectionCard.svelte's completePairing
 * doc comment for why `connected` (keychain-token-dependent) is the wrong
 * signal here. `sync_initial_import` must never run on a re-pair (it mints
 * fresh UUID directories every time); `reimport_oc_agents` overwrites in
 * place.
 */
export async function runOcImportPhase(args: {
  url: string;
  token: string;
  /** Renders the caller's own confirmation UI via a real dialog component
   *  (ConfirmDialog.svelte, Promise-wrapped by the caller — native `confirm()`
   *  does not render in Tauri's WebView and must never be passed here, D20
   *  #2649). Called UNCONDITIONALLY on every re-pair (`isFirstPair === false`),
   *  regardless of whether `check_oc_agent_collisions` found any collisions —
   *  `collisions` may be an empty array; the caller's dialog copy must handle
   *  that case gracefully. Never called on a first pair.
   *  Returning `false` means the user declined the overwrite: the phase
   *  resolves as `{ ok: true, declined: true, importedAgentNames: [], ... }` —
   *  declining is a legitimate choice, not an error, but it IS structurally
   *  distinct from a successful reimport (see `ImportResult.declined`). */
  confirmCollision: (collisions: AgentCollision[]) => Promise<boolean>;
}): Promise<ImportPhaseOutcome> {
  const { url, token, confirmCollision } = args;
  try {
    const alreadyPaired: boolean = await invoke('oc_agent_row_exists');
    const isFirstPair = !alreadyPaired;

    let importedAgentNames: string[] = [];
    let agentsMissingModel: string[] = [];
    let declined = false;

    if (isFirstPair) {
      const outcome: { agent_names: string[]; agents_missing_model: string[] } =
        await invoke('sync_initial_import', { gatewayUrl: url, token });
      importedAgentNames = outcome.agent_names ?? [];
      agentsMissingModel = outcome.agents_missing_model ?? [];
    } else {
      const check: { collisions: AgentCollision[]; new_agents: { oc_agent_id: string; name: string }[] } =
        await invoke('check_oc_agent_collisions', { url, token });

      // D20 (#2649): always prompt on a re-pair, even with zero collisions —
      // the prior `check.collisions.length >= 1` gate is removed. Zero
      // collisions is still a meaningful confirmation point ("re-import from
      // the gateway?"), and gating it made the native-`confirm()` bug (never
      // rendering, silently resolving `false`) invisible in the common case.
      const proceed = await confirmCollision(check.collisions);

      if (proceed) {
        const names: string[] = await invoke('reimport_oc_agents', { url, token });
        importedAgentNames = names ?? [];
        agentsMissingModel = [];
      } else {
        // Declined: not an error — pairing already succeeded, the user just
        // chose not to overwrite. Structurally distinct from success via the
        // `declined` flag (D20 ruling #3) so callers can't collapse this into
        // "0 agents imported".
        declined = true;
      }
    }

    return { ok: true, importedAgentNames, agentsMissingModel, isFirstPair, declined };
  } catch (e: any) {
    const raw = e?.message || String(e);
    const { code, message } = parseGatewayError(raw);
    return { ok: false, errorCode: code, errorMessage: message };
  }
}

/**
 * Full ceremony — phase 1 then (on success) phase 2. Used by callers with no
 * retry-only-sync affordance (Settings' `completePairing`); Onboarding uses
 * the two phases directly (see module doc) so its existing Retry button can
 * re-run phase 2 alone.
 *
 * `onPaired`, fired once between phase 1 and phase 2, lets the caller flip its
 * own "we are now durably paired" UI state (e.g. a screen transition) at the
 * exact point everything up to `gateway_paired_at` is guaranteed persisted —
 * a phase-2 failure afterward must not undo that.
 */
export async function runOcPairingCeremony(args: {
  url: string;
  code: string;
  fingerprint: string;
  certPem: string;
  confirmCollision: (collisions: AgentCollision[]) => Promise<boolean>;
  onPaired?: (token: string, versionWarning: string | null) => void;
}): Promise<PairingCeremonyOutcome> {
  const { url, code, fingerprint, certPem, confirmCollision, onPaired } = args;

  const exchangeOutcome = await runOcPairingExchange({ url, code, fingerprint, certPem });
  if (!exchangeOutcome.ok) {
    return { ok: false, phase: 'pairing', errorCode: exchangeOutcome.errorCode, errorMessage: exchangeOutcome.errorMessage };
  }

  onPaired?.(exchangeOutcome.token, exchangeOutcome.versionWarning);

  const canonicalUrl = normalizeOcGatewayUrl(url);
  const importOutcome = await runOcImportPhase({ url: canonicalUrl, token: exchangeOutcome.token, confirmCollision });
  if (!importOutcome.ok) {
    return { ok: false, phase: 'sync', errorCode: importOutcome.errorCode, errorMessage: importOutcome.errorMessage };
  }

  return {
    ok: true,
    token: exchangeOutcome.token,
    versionWarning: exchangeOutcome.versionWarning,
    importedAgentNames: importOutcome.importedAgentNames,
    agentsMissingModel: importOutcome.agentsMissingModel,
    isFirstPair: importOutcome.isFirstPair,
    declined: importOutcome.declined,
  };
}

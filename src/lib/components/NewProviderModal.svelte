<!--
  NewProviderModal.svelte — "New Provider" flow, ADR-0063 D8, #2522 Stage 8a/8b.
  #2642/#2630 (2026-08-22): this modal now owns the OpenClaw pairing ceremony
  end-to-end (architecture reversal from the original Stage 8a/8b "Amendment 1"
  design, see git history for that version). ProviderConnectionCard.svelte's
  OpenClaw surface is manage-only now (read-only paired summary, Disconnect,
  Test connection, Sync now) — see that component's module comment.
  Design: docs/design/agent-sessions-sprint/pc-c.jsx `PcNewProvider1` /
  `PcNewProvider2` / `PcNewProviderSteps` (the `PcNewProviderPaired` design ref
  no longer applies — that summary+Re-pair/Disconnect branch was deleted in
  this PR; Step 1 filters a connected OpenClaw out of the picker entirely, so
  there is no more "already paired" Step 2 state to render here).

  Two-step flow, dots not a wizard bar (per the `PcNewProviderSteps` design
  caption: "Two steps, dots not a wizard bar, and the second step is
  entirely determined by the first"):
    Step 1 — pick which provider (Stage 8a). OpenClaw is filtered out once
      connected (`isProviderConfigured`) — nothing left to "add".
    Step 2 — provider-specific config (Stage 8b):
      - Anthropic/OpenRouter/Ollama → API Key (key providers only) + Base URL
        fields, Test (separate from Add connection per the design caption:
        "a failed key does not block saving a connection the user will fix
        later") + Add connection, OR (if already connected) a read-only
        summary + Update/Disconnect. Calls the SAME Tauri commands
        ProviderConnectionCard.svelte's testConnection/saveConnection call
        (`validate_anthropic_key`, `validate_openrouter_key`,
        `ollama_probe_two_step`, `ai_configure_and_save`) directly via
        `invoke` — those functions are card-internal (not exported, not
        wrapped in api.ts) but every other call site in this codebase
        (Settings.svelte, Onboarding.svelte, ProviderConnectionCard.svelte)
        already invokes them the same direct way, so this follows the
        existing convention rather than inventing a new one or refactoring
        the card to expose them.
      - OpenClaw (always unpaired by the time Step 2 renders it, per Step 1's
        filter above) → the full pairing ceremony, owned entirely by this
        modal: URL entry → fingerprint verify → pairing code → import, using
        the shared `$lib/services/ocPairingCeremony.ts` module (zero changes
        to that module — it was already built UI-agnostic). Reachable two
        ways: normal Step-1 click-through (`handleContinue`), or the
        `startCeremony` deep-link prop (set by ProviderConnectionCard's
        Pair/Re-pair button via the Panel, for the paired-summary card's
        Re-pair action specifically, since Step 1 can't route there once OC
        is connected).

  Reuses the scrim + modal shell pattern from LegacyProviderPrompt.svelte
  (scrim click / stopPropagation on the modal body) rather than inventing a
  new dialog shell.
-->
<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { disconnectRuntime, type RuntimeConnectionInfo } from '$lib/api';
  import { normalizeOcGatewayUrl, runOcPairingExchange, runOcImportPhase, type AgentCollision } from '$lib/services/ocPairingCeremony';
  import ImportedAgentsModal from './ImportedAgentsModal.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';

  type ProviderOption = 'anthropic' | 'openrouter' | 'ollama' | 'openclaw' | 'venice';
  type ApiProviderOption = 'anthropic' | 'openrouter' | 'ollama' | 'venice';

  type ProviderRow = {
    id: ProviderOption;
    logo: string;
    name: string;
    desc: string;
  };

  // Order + copy VERBATIM from pc-c.jsx PcNewProvider1 `rows`.
  const PROVIDER_ROWS: ProviderRow[] = [
    { id: 'anthropic', logo: 'AN', name: 'Anthropic', desc: 'API key. Claude models direct from Anthropic.' },
    { id: 'openrouter', logo: 'OR', name: 'OpenRouter', desc: 'API key. One key, many model vendors.' },
    { id: 'ollama', logo: 'OL', name: 'Ollama', desc: 'Local server. No key, models run on this machine.' },
    { id: 'openclaw', logo: 'OC', name: 'OpenClaw', desc: 'Gateway pairing. One pairing per install.' },
    { id: 'venice', logo: 'VE', name: 'Venice', desc: 'API key. Your Venice API models. Get a key at venice.ai/settings/api' },
  ];

  // Per-provider Step 2 config — copy VERBATIM from pc-c.jsx PcNewProvider2's
  // hint text and this stage's task spec (Anthropic/OpenRouter API key hints,
  // all three Base URL defaults).
  const API_PROVIDER_META: Record<ApiProviderOption, {
    hasApiKey: boolean;
    apiKeyHint: string;
    baseUrlDefault?: string;
    hasBaseUrl: boolean;
  }> = {
    anthropic: {
      hasApiKey: true,
      apiKeyHint: 'Get your API key at console.anthropic.com',
      baseUrlDefault: 'https://api.anthropic.com/v1',
      hasBaseUrl: true,
    },
    openrouter: {
      hasApiKey: true,
      apiKeyHint: 'Get your API key at openrouter.ai/keys',
      baseUrlDefault: 'https://openrouter.ai/api/v1',
      hasBaseUrl: true,
    },
    ollama: {
      hasApiKey: false,
      apiKeyHint: '',
      baseUrlDefault: 'http://localhost:11434',
      hasBaseUrl: true,
    },
    venice: {
      hasApiKey: true,
      apiKeyHint: 'Get a key at venice.ai/settings/api',
      // Spec §28.5: Venice has no Base URL field — no default to prefill/discard.
      hasBaseUrl: false,
    },
  };

  let {
    onClose,
    ocConnectionInfo = null,
    connections = [],
    pairingFingerprint = $bindable(''),
    onConnectionSaved,
    startCeremony = null,
  }: {
    /** () => void — called on Cancel, scrim click, Escape, or after a terminal
     *  Step 2 action (Add connection success dismiss, ceremony success/decline
     *  dismiss). Nothing is written by onClose itself; the caller
     *  (ProviderConnectionsPanel) just hides the modal. */
    onClose: () => void;
    /** The current `ai_list_runtime_connections` entry for openclaw, threaded
     *  down from the Panel (which already fetches the full list). Drives Step 1's
     *  `isProviderConfigured` filter and the ceremony's 're-pair' URL prefill
     *  (`ocConnectionInfo?.endpoint`). `null` while the Panel's own fetch hasn't
     *  resolved yet — treated the same as unpaired (defensive default; matches
     *  the card's own `!info.connected` treatment). */
    ocConnectionInfo?: RuntimeConnectionInfo | null;
    /** #2522 design polish batch, item 4: full `ai_list_runtime_connections` list,
     *  threaded down from the Panel (which already fetches it — same source
     *  ocConnectionInfo is derived from there). Lets Step 2 detect an
     *  already-connected anthropic/openrouter/ollama selection too, not just
     *  OpenClaw, and show its existing connection state instead of a fresh
     *  blank config form. Defaults to `[]` so every existing call site that
     *  only passes `ocConnectionInfo` keeps working unchanged. */
    connections?: RuntimeConnectionInfo[];
    /** Shared with ProviderConnectionCard.svelte's paired OpenClaw card (see
     *  ProviderConnectionsPanel.svelte's `bind:pairingFingerprint`) — the
     *  ceremony's `exchangeOcCeremonyCode` writes this directly on phase-1
     *  success, same point the card's `onPaired` callback used to, so the
     *  card's TLS Fingerprint row picks it up without waiting for a reload
     *  round-trip. */
    pairingFingerprint?: string;
    /** #2642/#2630 Commit 4, Matt's ruling (c): called after a successful Add
     *  connection, after Disconnect (card-only now, unaffected), or after this
     *  modal's OC ceremony resolves (success OR a non-blocking phase-2 import
     *  failure) — so the Panel can re-fetch `ai_list_runtime_connections` and the
     *  OC card's own `hasOcAgentRows`/roster state stays in sync. Optional
     *  `lastImportError` argument: set when the ceremony's phase 2
     *  (`runOcImportPhase`) failed non-blockingly after phase 1 already durably
     *  succeeded — the Panel threads this into the card's `pairSyncError` prop
     *  (Commit 5) so the #2640 D13 fix's collapsed-card "⚠️ Import failed" badge
     *  keeps working with the ceremony now living outside the card. `undefined`/
     *  omitted (every other call site) clears any prior badge, matching the
     *  card's own previous per-open-instance `pairSyncError` reset behavior. */
    onConnectionSaved?: (lastImportError?: string) => void;
    /** #2642/#2630 Commit 2: deep-link entry for the OC pairing ceremony this
     *  modal now owns. When set to `'pair'` or `'re-pair'`, the modal skips
     *  Step 1 entirely and mounts directly at the ceremony's URL step
     *  (`ocCeremonyStep = 'url'`) — the caller (ProviderConnectionCard's
     *  Pair/Re-pair button, via the Panel's `onPairRequested`) already knows
     *  which mode applies (driven by its own `hasOcAgentRows` check, #2642
     *  Commit 1), so the modal doesn't need to re-derive it before the
     *  ceremony starts — it only affects copy ("Pair OpenClaw" vs "Re-pair
     *  OpenClaw") and the URL field's prefill (`'re-pair'` prefills from
     *  `ocConnectionInfo?.endpoint`). Actual first-pair/re-pair ROUTING
     *  (`sync_initial_import` vs `reimport_oc_agents`) is independently
     *  determined by `runOcImportPhase`'s own internal `oc_agent_row_exists`
     *  check per Reed's audit §1.5 — this prop never bypasses that. `null`
     *  (default) means "normal Step-1 entry", unchanged behavior. */
    startCeremony?: 'pair' | 're-pair' | null;
  } = $props();

  let step = $state<1 | 2>(1);
  let selectedProvider = $state<ProviderOption | null>(null);

  // ---------------------------------------------------------------------------
  // Step 1 — unconfigured-only filter (#2590 UX batch 2, item 2)
  // ---------------------------------------------------------------------------
  // "Add Provider" / "New Provider" is for connecting a NEW provider — a
  // provider that's already configured/connected has nothing left to "add".
  // Filters PROVIDER_ROWS down to providers that are NOT already connected,
  // checked against the same `connections` list Step 2's already-connected
  // detection above uses (`ai_list_runtime_connections`, the fixed 4-entry
  // set — see ProviderConnectionsPanel.svelte's module comment). OpenClaw is
  // additionally checked against `ocConnectionInfo` (its dedicated prop) so a
  // caller that only threads `ocConnectionInfo` (not the full `connections`
  // list) still filters OpenClaw correctly once paired.
  //
  // F1 fix (v1.0.36 live-test round 2): the non-openclaw branch used to check
  // `c.connected === true` — a presence check that can be true from a stale
  // config (e.g. Ollama's `ollama_endpoint` grandfathered in by a migration,
  // #2777) even though the user never explicitly configured that provider this
  // install. That caused a provider with NO card on the Panel (because the
  // Panel gates on `configured_this_install`) to ALSO be hidden here — leaving
  // it permanently unreachable from either surface. Both filters must agree on
  // the same field.
  // INVARIANT: must match ProviderConnectionsPanel's card-visibility filter
  // (`connections.filter((c) => c.configured_this_install)`) — a provider
  // without a card is always addable via New Provider. If this predicate's
  // field ever changes, ProviderConnectionsPanel's filter must change with it,
  // or the two surfaces will diverge again (see ProviderConnectionsPanel.svelte's
  // matching INVARIANT comment on its card filter).
  function isProviderConfigured(id: ProviderOption): boolean {
    if (id === 'openclaw') {
      return ocPaired || connections.some((c) => c.runtime_type === 'openclaw' && c.connected === true);
    }
    return connections.some((c) => c.runtime_type === id && c.configured_this_install === true);
  }

  const visibleProviderRows = $derived(PROVIDER_ROWS.filter((row) => !isProviderConfigured(row.id)));

  // ---------------------------------------------------------------------------
  // Step 2 — API providers (anthropic/openrouter/ollama)
  // ---------------------------------------------------------------------------
  let apiKeyInput = $state('');
  let baseUrlInput = $state('');
  let testStatus = $state<'idle' | 'testing' | 'success' | 'error'>('idle');
  let testMessage = $state('');
  let saving = $state(false);
  let saveError = $state('');
  let saveSuccess = $state(false);

  const isApiProvider = $derived(
    selectedProvider === 'anthropic' || selectedProvider === 'openrouter' || selectedProvider === 'ollama' || selectedProvider === 'venice'
  );
  const apiMeta = $derived(isApiProvider ? API_PROVIDER_META[selectedProvider as ApiProviderOption] : null);
  const canTest = $derived(
    !apiMeta ? false : apiMeta.hasApiKey ? apiKeyInput.trim() !== '' : true
  );
  const canAdd = $derived(canTest);

  // #2522 design polish batch, item 4: an already-connected anthropic/openrouter/
  // ollama selection shows its existing connection state (masked key or endpoint,
  // agent count) instead of jumping straight into a fresh blank config form —
  // generalizing the pattern the OpenClaw "already paired" branch below already
  // has. `connections` is optional (defaults to []), so callers that only pass
  // `ocConnectionInfo` (pre-existing call sites/tests) never hit this branch.
  const selectedApiConnectionInfo = $derived(
    isApiProvider ? connections.find((c) => c.runtime_type === selectedProvider) ?? null : null
  );
  const apiProviderAlreadyConnected = $derived(
    isApiProvider && selectedApiConnectionInfo?.connected === true
  );
  // Lets the user reach the config form anyway from the already-connected summary,
  // via "Update connection" — same escape hatch OpenClaw's paired card offers
  // through Re-pair, just without leaving this modal.
  let showReconfigureForm = $state(false);
  let apiDisconnectError = $state('');
  let isApiDisconnecting = $state(false);
  let showApiDisconnectConfirm = $state(false);

  const apiDisconnectLabel = $derived(
    PROVIDER_ROWS.find((r) => r.id === selectedProvider)?.name ?? selectedProvider
  );

  function handleApiProviderDisconnect() {
    if (!selectedProvider) return;
    showApiDisconnectConfirm = true;
  }

  async function confirmApiProviderDisconnect() {
    showApiDisconnectConfirm = false;
    if (!selectedProvider) return;
    apiDisconnectError = '';
    isApiDisconnecting = true;
    try {
      await disconnectRuntime(selectedProvider);
      await onConnectionSaved?.();
      onClose();
    } catch (e: any) {
      apiDisconnectError = e?.toString?.() ?? String(e);
    } finally {
      isApiDisconnecting = false;
    }
  }

  function cancelApiProviderDisconnect() {
    showApiDisconnectConfirm = false;
  }

  function resetStep2Result() {
    testStatus = 'idle';
    testMessage = '';
    saveError = '';
    saveSuccess = false;
  }

  // §28.2 / #1630 fix: shared per-runtime validator dispatch, factored out of
  // `handleTest` so `handleAddConnection` can call the identical validation logic
  // before `ai_configure_and_save` instead of duplicating it. Throws on failure.
  // Ollama has no key to validate here — its connectivity probe (`ollama_probe_two_step`)
  // is left in `handleTest` only, matching ProviderConnectionCard's split (Ollama's
  // "Save"/"Add" was never gated on Test in the original design either; only the
  // key-provider gap is #1630's scope).
  async function runApiProviderValidator(): Promise<void> {
    if (selectedProvider === 'anthropic') {
      await invoke('validate_anthropic_key', { apiKey: apiKeyInput });
    } else if (selectedProvider === 'openrouter') {
      await invoke('validate_openrouter_key', { apiKey: apiKeyInput });
    } else if (selectedProvider === 'venice') {
      // ADR-0065 D8: validate via GET /api_keys/rate_limits (not /models)
      await invoke('validate_venice_key', { apiKey: apiKeyInput });
    }
  }

  async function handleTest() {
    if (!apiMeta || !selectedProvider) return;
    resetStep2Result();
    testStatus = 'testing';
    try {
      if (selectedProvider === 'anthropic' || selectedProvider === 'openrouter' || selectedProvider === 'venice') {
        await runApiProviderValidator();
      } else if (selectedProvider === 'ollama') {
        const result = (await invoke('ollama_probe_two_step', {
          endpoint: baseUrlInput.trim() || undefined,
        })) as { models: string[]; error_class: string | null };
        if (result.error_class) {
          const endpointForMsg = baseUrlInput.trim() || API_PROVIDER_META.ollama.baseUrlDefault;
          throw new Error(
            result.error_class === 'unreachable'
              ? `Couldn't reach Ollama at ${endpointForMsg}. Make sure it's running.`
              : result.error_class === 'no_models'
                ? 'Ollama is running but no models are installed.'
                : "Connected to Ollama but couldn't list models."
          );
        }
      }
      testStatus = 'success';
      testMessage = 'Connected';
    } catch (e: any) {
      testStatus = 'error';
      testMessage = e?.toString?.() ?? String(e);
    }
  }

  async function handleAddConnection() {
    if (!selectedProvider || selectedProvider === 'openclaw') return;
    resetStep2Result();
    saving = true;
    try {
      // §28.2 / #1630 fix: validate before ai_configure_and_save, same pattern as
      // ProviderConnectionCard.svelte's saveConnection(). Previously `canAdd` was
      // derived purely from field-presence (`$derived(canTest)`), identical to the
      // Test button's enablement — Add was reachable, and saved, the instant a key
      // was typed, entirely independent of whether Test was ever clicked or what
      // it returned. Key providers (anthropic/openrouter/venice) now validate here
      // unconditionally before saving; ollama has no key to validate (unchanged).
      if (selectedProvider === 'anthropic' || selectedProvider === 'openrouter' || selectedProvider === 'venice') {
        try {
          await runApiProviderValidator();
        } catch (e: any) {
          saveError = e?.toString?.() ?? String(e);
          return;
        }
      }
      await invoke('ai_configure_and_save', {
        provider: selectedProvider,
        apiKey: selectedProvider === 'ollama' ? null : apiKeyInput,
        model: null,
        endpoint: baseUrlInput.trim() || null,
        simpleTierModel: null,
      });
      saveSuccess = true;
      await onConnectionSaved?.();
    } catch (e: any) {
      saveError = e?.toString?.() ?? String(e);
    } finally {
      saving = false;
    }
  }

  // ---------------------------------------------------------------------------
  // Step 2 — OpenClaw already paired (Step-1 filter, `isProviderConfigured` above)
  // ---------------------------------------------------------------------------
  // #2642/#2630 Commit 4: the paired-summary branch (Gateway URL/Fingerprint/
  // Paired date/Agents count + Re-pair/Disconnect/Open connection, all
  // delegating to callback props) is deleted — it's unreachable dead code now.
  // Step 1's `isProviderConfigured` already filters a connected OpenClaw out of
  // the provider picker, and `handleContinue` below no longer has an
  // "already paired, show summary" branch of its own (every OpenClaw selection
  // now goes straight into the ceremony, `openOcCeremony('pair')`) — so the only
  // way to reach Step 2 with `selectedProvider === 'openclaw'` is via the
  // ceremony (either through `handleContinue` or the `startCeremony` deep link
  // below), which is exactly what the surviving `ocCeremonyStep !== 'closed'`
  // branch renders. `ocPaired` itself survives (still used by
  // `isProviderConfigured`'s Step-1 filter and `visibleProviderRows`).
  const ocPaired = $derived(ocConnectionInfo?.connected === true);

  // ---------------------------------------------------------------------------
  // #2642/#2630 Commit 2: OC pairing ceremony, ported from
  // ProviderConnectionCard.svelte's `pairStep` (see that component's module
  // comment for the full history) — this modal now owns the ceremony end-to-end
  // instead of handing off to the card. Two behavioral differences from the
  // card's version, both explicit Matt rulings (see task doc):
  //   1. Retry-only-sync: uses `runOcPairingExchange` + `runOcImportPhase`
  //      separately (Onboarding.svelte's pattern), not the combined
  //      `runOcPairingCeremony` the card uses — a failed import can retry via
  //      `retryImport()` without re-submitting the (one-time-use) pairing code.
  //   2. `pairSyncError` on the card was a persistent, non-blocking badge that
  //      closed the ceremony immediately on import failure. The modal keeps the
  //      ceremony open with an inline error + Retry button instead (Onboarding's
  //      pattern) since there's no separate "closed ceremony, still-visible card"
  //      state here to fall back to — this modal *is* the ceremony's only UI.
  // ---------------------------------------------------------------------------
  type OcCeremonyStep = 'closed' | 'url' | 'verify' | 'code' | 'importing';
  let ocCeremonyStep = $state<OcCeremonyStep>('closed');
  let ocCeremonyUrlInput = $state('');
  let ocCeremonyGatewayUrl = $state('');
  let ocCeremonyFingerprint = $state('');
  let ocCeremonyCertPem = $state('');
  let ocCeremonyCode = $state('');
  let ocCeremonyToken = $state('');
  let ocCeremonyVersionWarning = $state('');
  let ocCeremonyProbing = $state(false);
  let ocCeremonyExchanging = $state(false);
  let ocCeremonyImporting = $state(false);
  let ocCeremonyProbeError = $state<{ message: string; address: string; port: string; elapsedMs: number; validation: boolean } | null>(null);
  let ocCeremonyCodeError = $state('');
  let ocCeremonyRejected = $state(false);
  let ocCeremonyImportError = $state('');
  let ocCeremonyRepairDeclined = $state(false);
  let ocCeremonyImportedAgentNames = $state<string[]>([]);
  let ocCeremonyAgentsMissingModel = $state<string[]>([]);
  let showImportedAgentsPopup = $state(false);
  // D20 (#2649) pattern, ported verbatim from the card: ConfirmDialog is
  // callback-based, `confirmCollision` (passed into `runOcImportPhase`) needs a
  // Promise<boolean> — these three bridge the two. Native `confirm()` never
  // renders in Tauri's WebView (silently resolves `false`), so this Promise
  // bridge is load-bearing, not decorative.
  let showOcRepairConfirm = $state(false);
  let ocRepairCollisions = $state<AgentCollision[]>([]);
  let pendingOcRepairResolve: ((v: boolean) => void) | null = $state(null);


  /** Entry point for both the normal Step-1 "Continue" click (unpaired OpenClaw,
   *  wired up in Commit 4) and the `startCeremony` deep-link prop below —
   *  `'re-pair'` prefills the URL field from `ocConnectionInfo?.endpoint`,
   *  matching the card's Re-pair button behavior; `'pair'` starts blank. */
  function openOcCeremony(mode: 'pair' | 're-pair') {
    ocCeremonyStep = 'url';
    ocCeremonyUrlInput = mode === 're-pair' ? (ocConnectionInfo?.endpoint ?? '') : '';
    ocCeremonyFingerprint = '';
    ocCeremonyCertPem = '';
    ocCeremonyCode = '';
    ocCeremonyToken = '';
    ocCeremonyVersionWarning = '';
    ocCeremonyProbeError = null;
    ocCeremonyCodeError = '';
    ocCeremonyRejected = false;
    ocCeremonyImportError = '';
    ocCeremonyRepairDeclined = false;
  }

  async function probeOcCeremonyUrl() {
    ocCeremonyProbeError = null;
    ocCeremonyProbing = true;
    const startedAt = Date.now();
    let normalizedUrl = '';
    let address = ocCeremonyUrlInput.trim();
    let port = '18789';
    try {
      normalizedUrl = normalizeOcGatewayUrl(ocCeremonyUrlInput);
      ocCeremonyGatewayUrl = normalizedUrl;
      ocCeremonyUrlInput = normalizedUrl;
      const parsed = new URL(normalizedUrl);
      address = parsed.hostname || address;
      port = parsed.port || port;

      const result: any = await invoke('gateway_fetch_fingerprint', { url: normalizedUrl });
      ocCeremonyFingerprint = result.tls_fingerprint || '';
      ocCeremonyCertPem = result.cert_pem || '';
      // Health data is unauthenticated during trust-on-first-use. Never let it
      // change the origin the user reviewed; fingerprint, exchange, persistence,
      // and sync all continue using this same canonical URL.
      ocCeremonyRejected = false;
      ocCeremonyStep = 'verify';
    } catch (e: any) {
      const msg = e?.message || String(e);
      let reason = msg;
      if (msg.includes('Cannot reach')) {
        reason = 'Make sure Pane Gateway and Tailscale are running on the OC machine.';
      } else if (msg.includes('Invalid response') || msg.includes('No tls_fingerprint') || msg.includes('No tls_cert_pem')) {
        reason = "Something responded but it doesn't look like a Pane Gateway.";
      }
      const validation = normalizedUrl === '';
      ocCeremonyProbeError = { message: reason, address, port, elapsedMs: Date.now() - startedAt, validation };
    } finally {
      ocCeremonyProbing = false;
    }
  }

  function rejectOcCeremonyFingerprint() {
    ocCeremonyRejected = true;
  }

  /** URL survives the failure (matches the card's rule) — only the fingerprint/
   *  cert are cleared, step goes back to 'url' so the user can re-check the
   *  address before probing again. */
  function retryOcCeremonyAfterReject() {
    ocCeremonyRejected = false;
    ocCeremonyStep = 'url';
    ocCeremonyFingerprint = '';
    ocCeremonyCertPem = '';
  }

  function acceptOcCeremonyFingerprint() {
    ocCeremonyRejected = false;
    ocCeremonyCodeError = '';
    ocCeremonyCode = '';
    ocCeremonyStep = 'code';
  }

  /** Phase 1 (pairing-code exchange) via the shared `runOcPairingExchange` — on
   *  success, immediately chains into phase 2 (`runImportPhase` below), matching
   *  Onboarding's `exchangePairingCode` → `screen = 'oc-sync'` → auto-triggered
   *  `runOcSync()` flow (Onboarding.svelte's `$effect` on `screen === 'oc-sync'`).
   *  A phase-1 failure (bad/expired code) keeps `ocCeremonyStep = 'code'` with the
   *  fingerprint/URL untouched — "the accepted fingerprint survives a bad code",
   *  same rule as the card. */
  async function exchangeOcCeremonyCode() {
    ocCeremonyCodeError = '';
    ocCeremonyExchanging = true;
    try {
      const outcome = await runOcPairingExchange({
        url: ocCeremonyGatewayUrl,
        code: ocCeremonyCode,
        fingerprint: ocCeremonyFingerprint,
        certPem: ocCeremonyCertPem,
      });
      if (!outcome.ok) {
        ocCeremonyCodeError = outcome.errorMessage;
        return;
      }
      ocCeremonyToken = outcome.token;
      ocCeremonyVersionWarning = outcome.versionWarning ?? '';
      // Feed the shared Settings.svelte state var directly, same point the card's
      // `onPaired` callback did — no need to wait for a reload round-trip before
      // the paired card's TLS Fingerprint row can show it. Pairing is durably
      // persisted at this point (token/endpoint/TLS material already written by
      // `runOcPairingExchange`), so this is safe even if phase 2 below fails.
      pairingFingerprint = ocCeremonyFingerprint;
      await runOcCeremonyImportPhase();
    } finally {
      ocCeremonyExchanging = false;
    }
  }

  /** Phase 2 (first-pair/re-pair import routing) via the shared
   *  `runOcImportPhase` — kept as its own function (not folded into
   *  `exchangeOcCeremonyCode`) specifically so `retryOcCeremonyImport` below can
   *  call it alone on a sync-only retry, without re-submitting the already-
   *  consumed pairing code (Matt's retry-only-sync ruling). */
  async function runOcCeremonyImportPhase() {
    ocCeremonyStep = 'importing';
    ocCeremonyImportError = '';
    ocCeremonyRepairDeclined = false;
    ocCeremonyImporting = true;
    try {
      const outcome = await runOcImportPhase({
        url: ocCeremonyGatewayUrl,
        token: ocCeremonyToken,
        confirmCollision: (collisions: AgentCollision[]) =>
          new Promise<boolean>((resolve) => {
            ocRepairCollisions = collisions;
            pendingOcRepairResolve = resolve;
            showOcRepairConfirm = true;
          }),
      });

      if (!outcome.ok) {
        ocCeremonyImportError = outcome.errorMessage;
        // Matt's ruling (c): thread the failure back to the Panel immediately
        // (not just on eventual dismiss) so the card's collapsed-header
        // "⚠️ Import failed" badge (#2640 D13 fix) can appear right away — pairing
        // itself already durably succeeded (token/endpoint/cert/gateway_paired_at
        // all written in phase 1), the ceremony just stays open here for the
        // Retry button, it does NOT call onClose. A later successful retry
        // clears the badge (see the `showImportedAgentsPopup` dismiss below,
        // which calls `onConnectionSaved()` with no argument).
        onConnectionSaved?.(outcome.errorMessage);
        return;
      }

      // D20 (#2649): a declined re-pair is `ok: true` but structurally distinct
      // from a successful reimport — dismissable notice, not the import-error
      // banner styling. This is a user choice, not a failure.
      if (outcome.declined) {
        ocCeremonyRepairDeclined = true;
        return;
      }

      ocCeremonyAgentsMissingModel = outcome.agentsMissingModel;
      ocCeremonyImportedAgentNames = outcome.importedAgentNames;
      showImportedAgentsPopup = true;
      ocCeremonyStep = 'closed';
    } finally {
      ocCeremonyImporting = false;
    }
  }

  /** Retry-only-sync (Matt's ruling #3): re-runs ONLY `runOcCeremonyImportPhase`
   *  against the already-obtained `ocCeremonyToken` — never re-calls
   *  `exchangeOcCeremonyCode`, so a consumed one-time pairing code is never
   *  resubmitted. Matches Onboarding's `runOcSync` Retry button exactly. */
  function retryOcCeremonyImport() {
    runOcCeremonyImportPhase();
  }

  function dismissImportedAgentsPopup() {
    showImportedAgentsPopup = false;
    ocCeremonyImportedAgentNames = [];
    ocCeremonyAgentsMissingModel = [];
    onConnectionSaved?.();
    onClose();
  }

  function dismissOcCeremonyDeclined() {
    ocCeremonyStep = 'closed';
    ocCeremonyRepairDeclined = false;
    onConnectionSaved?.();
    onClose();
  }

  /** No separate backend call — a new code is generated on the OC machine
   *  (`pane-gateway pairing-code`); this just clears the stale code/error so the
   *  user can retype without a leftover error message. */
  function requestNewOcCeremonyCode() {
    ocCeremonyCode = '';
    ocCeremonyCodeError = '';
  }

  function confirmOcRepair() {
    showOcRepairConfirm = false;
    pendingOcRepairResolve?.(true);
    pendingOcRepairResolve = null;
  }

  function cancelOcRepair() {
    showOcRepairConfirm = false;
    pendingOcRepairResolve?.(false);
    pendingOcRepairResolve = null;
  }

  function cancelOcCeremony() {
    ocCeremonyStep = 'closed';
    // Deep-link entry (`startCeremony` set) has no Step 1 selection to fall back
    // to sensibly — close the whole modal, same as Cancel does everywhere else in
    // this modal. Normal Step-1-driven entry (wired in Commit 4) falls back to
    // Step 1 instead, matching every other Step 2 Cancel/Back in this modal.
    if (startCeremony) {
      onClose();
    } else {
      handleBack();
    }
  }

  // #2642/#2630 Commit 2: `startCeremony` deep-link entry — skips Step 1 entirely
  // and mounts directly at the ceremony's URL step. Runs once per modal open
  // (guarded by ocCeremonyStep === 'closed' so it doesn't re-trigger mid-ceremony
  // if `ocConnectionInfo` happens to update while this modal is still mounted).
  $effect(() => {
    if (startCeremony && ocCeremonyStep === 'closed' && !showImportedAgentsPopup) {
      selectedProvider = 'openclaw';
      step = 2;
      openOcCeremony(startCeremony);
    }
  });

  // ---------------------------------------------------------------------------
  // Navigation
  // ---------------------------------------------------------------------------
  function selectProvider(id: ProviderOption) {
    // Radio-style: only one selection at a time — assigning replaces
    // whatever was previously selected rather than toggling.
    selectedProvider = id;
  }

  function resetAllStep2State() {
    apiKeyInput = '';
    baseUrlInput = '';
    resetStep2Result();
    showReconfigureForm = false;
    apiDisconnectError = '';
  }

  function handleContinue() {
    if (!selectedProvider) return;
    if (selectedProvider === 'openclaw') {
      // #2642/#2630 Commit 4: Amendment 1 ("this modal does NOT implement the
      // pairing ceremony") is reversed — the modal now owns it end-to-end. Step 1
      // already filters out a CONNECTED OpenClaw (isProviderConfigured above), so
      // reaching here means either never-paired or disconnected-but-not-deleted;
      // either way `ocConnectionInfo?.endpoint` is null (disconnect clears it
      // server-side, per ai_disconnect_runtime — see ProviderConnectionCard.svelte's
      // module comment), so 'pair' mode's blank-URL prefill is correct for both.
      // (The deep-link `startCeremony` prop below is the path that actually needs
      // to distinguish 'pair' vs 're-pair' — it's driven by the card's own
      // hasOcAgentRows check, which this modal has no equivalent of for a normal
      // Step-1-driven entry.)
      step = 2;
      openOcCeremony('pair');
      return;
    }
    resetAllStep2State();
    step = 2;
  }

  function handleBack() {
    step = 1;
    resetAllStep2State();
  }

  function selectedProviderName(): string {
    const row = PROVIDER_ROWS.find((r) => r.id === selectedProvider);
    return row?.name ?? '';
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      onClose();
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="npm-scrim" onclick={onClose} role="presentation">
  <div
    class="npm-modal"
    role="dialog"
    aria-modal="true"
    aria-label="New Provider"
    onclick={(e) => e.stopPropagation()}
  >
    <div class="npm-modal__head">
      <h3 class="npm-modal__title">New Provider</h3>
      <div class="npm-modal__head-right">
        <div class="npm-steps" aria-hidden="true">
          <i class:npm-step--on={step === 1} class:npm-step--done={step > 1}></i>
          <i class:npm-step--on={step === 2}></i>
        </div>
        <button type="button" class="npm-close-btn" onclick={onClose} aria-label="Close">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
            stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M18 6L6 18M6 6l12 12"/>
          </svg>
        </button>
      </div>
    </div>

    {#if step === 1}
      <div class="npm-flow__head">
        <div class="npm-eyebrow">New provider · step 1 of 2</div>
        <h2 class="npm-flow__title">Which provider are you connecting?</h2>
        <p class="npm-flow__sub">
          The provider determines the fields on the next step. Agents you create on it stay on it.
        </p>
      </div>

      <div class="npm-choose" role="radiogroup" aria-label="Provider">
        {#each visibleProviderRows as row (row.id)}
          <button
            type="button"
            class="npm-choice"
            class:npm-choice--sel={selectedProvider === row.id}
            role="radio"
            aria-checked={selectedProvider === row.id}
            onclick={() => selectProvider(row.id)}
          >
            <span class="npm-choice__logo" aria-hidden="true">{row.logo}</span>
            <span class="npm-choice__body">
              <span class="npm-choice__name">{row.name}</span>
              <span class="npm-choice__desc">{row.desc}</span>
            </span>
            <svg class="npm-choice__chev" width="15" height="15" viewBox="0 0 24 24" fill="none"
              stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <path d="M9 18l6-6-6-6"/>
            </svg>
          </button>
        {:else}
          <!-- Edge case: every provider is already configured. All 4 rows are
               filtered out rather than none — show an explicit message instead
               of silently rendering an empty radiogroup with a permanently
               disabled Continue button. -->
          <div class="npm-choose__empty">All providers are already connected. Disconnect one first to reconnect it here.</div>
        {/each}
      </div>

      <div class="npm-flow__foot">
        <button class="npm-btn npm-btn--quiet" onclick={onClose}>Cancel</button>
        <button
          class="npm-btn npm-btn--primary"
          disabled={!selectedProvider}
          onclick={handleContinue}
        >
          Continue
        </button>
      </div>
    {:else if isApiProvider && apiProviderAlreadyConnected && !showReconfigureForm}
      <!-- Step 2 — already-connected anthropic/openrouter/ollama (#2522 design polish
           batch, item 4): existing connection state, not a fresh config form.
           Generalizes the OpenClaw "already paired" summary pattern below to the
           other three providers. -->
      <div class="npm-flow__head">
        <div class="npm-eyebrow npm-eyebrow--accent">Already connected</div>
        <h2 class="npm-flow__title">{selectedProviderName()} is already connected</h2>
        <p class="npm-flow__sub">
          Update the connection to change its key or endpoint, or disconnect it first to start fresh.
        </p>
      </div>

      <div class="npm-paired">
        <div class="npm-paired__head">
          <span class="npm-choice__logo" aria-hidden="true">{PROVIDER_ROWS.find((r) => r.id === selectedProvider)?.logo}</span>
          <span class="npm-paired__name">
            {selectedProviderName()}
            <span class="npm-badge npm-badge--paired">Connected</span>
          </span>
          <span class="npm-status">Connected</span>
        </div>
        <div class="npm-paired__body">
          {#if apiMeta?.hasApiKey}
            <div class="npm-kv"><span class="npm-kv__k">API Key</span><span class="npm-kv__v">{selectedApiConnectionInfo?.masked_credential ?? '—'}</span></div>
          {:else}
            <div class="npm-kv"><span class="npm-kv__k">Endpoint</span><span class="npm-kv__v">{selectedApiConnectionInfo?.endpoint ?? '—'}</span></div>
          {/if}
          <div class="npm-kv"><span class="npm-kv__k">Agents</span><span class="npm-kv__v">{selectedApiConnectionInfo?.agents.length ?? 0}</span></div>
        </div>
        {#if apiDisconnectError}
          <div class="npm-test-err npm-test-err--inline">{apiDisconnectError}</div>
        {/if}
        <div class="npm-paired__foot">
          <button class="npm-btn npm-btn--ghost npm-btn--sm" onclick={() => (showReconfigureForm = true)}>Update connection</button>
          <span class="npm-spacer"></span>
          <button class="npm-btn npm-btn--danger npm-btn--sm" onclick={handleApiProviderDisconnect} disabled={isApiDisconnecting}>
            {isApiDisconnecting ? 'Disconnecting…' : 'Disconnect'}
          </button>
        </div>
      </div>

      <div class="npm-flow__foot">
        <button class="npm-btn npm-btn--quiet" onclick={handleBack}>Back</button>
      </div>
    {:else if isApiProvider}
      <!-- Step 2 — API providers (pc-c.jsx PcNewProvider2). -->
      <div class="npm-flow__head">
        <div class="npm-eyebrow">New provider · step 2 of 2</div>
        <h2 class="npm-flow__title">Connect {selectedProviderName()}</h2>
        <p class="npm-flow__sub">
          Pane tests the key before adding the connection. Existing connections keep working while you do this.
        </p>
      </div>

      {#if apiMeta?.hasApiKey}
        <div class="npm-field">
          <span class="npm-label">API Key</span>
          <input
            class="npm-textin npm-textin--mono"
            type="text"
            bind:value={apiKeyInput}
            oninput={resetStep2Result}
            aria-label="API Key"
          />
          <span class="npm-hint">{apiMeta.apiKeyHint}</span>
        </div>
      {/if}

      {#if apiMeta?.hasBaseUrl !== false}
        <div class="npm-field">
          <span class="npm-label">Base URL</span>
          <input
            class="npm-textin npm-textin--mono"
            type="text"
            bind:value={baseUrlInput}
            oninput={resetStep2Result}
            placeholder={apiMeta?.baseUrlDefault ?? ''}
            aria-label="Base URL"
          />
          <span class="npm-hint">Optional. Leave blank for the default endpoint.</span>
        </div>
      {/if}

      {#if testStatus === 'success'}
        <div class="npm-test-ok">✓ {testMessage}</div>
      {:else if testStatus === 'error'}
        <div class="npm-test-err">✗ {testMessage}</div>
      {/if}
      {#if saveError}
        <div class="npm-test-err">✗ {saveError}</div>
      {/if}
      {#if saveSuccess}
        <div class="npm-test-ok">✓ Connection added</div>
      {/if}

      <div class="npm-flow__foot">
        <button class="npm-btn npm-btn--quiet" onclick={handleBack}>Back</button>
        <div class="npm-flow__foot__g">
          <button class="npm-btn npm-btn--ghost" onclick={handleTest} disabled={!canTest || testStatus === 'testing'}>
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
              stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <path d="M23 4v6h-6M1 20v-6h6"/>
              <path d="M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15"/>
            </svg>
            {testStatus === 'testing' ? 'Testing…' : 'Test'}
          </button>
          <button class="npm-btn npm-btn--primary" onclick={handleAddConnection} disabled={!canAdd || saving}>
            {saving ? 'Adding…' : 'Add connection'}
          </button>
        </div>
      </div>
    {:else if selectedProvider === 'openclaw' && ocCeremonyStep !== 'closed'}
      <!-- #2642/#2630 Commit 2: OC pairing ceremony, owned end-to-end by this modal
           now. Takes priority over the ocPaired branch below so a re-pair
           (started while ocPaired is still true, right up until the ceremony's
           phase 1 completes) renders the ceremony steps, not the paired summary.
           Ported from ProviderConnectionCard.svelte's `pairStep` template — see
           the script block above for the full behavioral-difference rationale
           (retry-only-sync, inline errors instead of a persistent card badge). -->
      <div class="npm-flow__head">
        <div class="npm-eyebrow">New provider · step 2 of 2</div>
        <h2 class="npm-flow__title">{startCeremony === 're-pair' ? 'Re-pair OpenClaw' : 'Pair OpenClaw'}</h2>
        <p class="npm-flow__sub">Pairing enables automatic two-way sync. The gateway receives supported Pane agent workspace context—including identity, instructions, memory, tools, and logs—and may update local allowlisted agent files.</p>
      </div>

      {#if ocCeremonyStep !== 'url'}
        <div class="npm-field">
          <span class="npm-label">Gateway URL</span>
          <div class="npm-kv__v" data-testid="oc-ceremony-url-readonly">{ocCeremonyUrlInput}</div>
        </div>
      {/if}

      {#if ocCeremonyStep === 'url'}
        <div class="npm-field">
          <span class="npm-label">Gateway URL</span>
          <input
            type="text"
            class="npm-textin npm-textin--mono"
            bind:value={ocCeremonyUrlInput}
            placeholder="https://100.x.x.x:18789"
            aria-label="Gateway URL"
            onkeydown={(e) => { if (e.key === 'Enter') probeOcCeremonyUrl(); }}
          />
        </div>
        {#if ocCeremonyProbeError}
          <div class="npm-test-err" data-testid="oc-ceremony-probe-error">
            <div><strong>{ocCeremonyProbeError.validation ? "Gateway URL isn't valid" : "Couldn't reach the gateway"}</strong></div>
            {#if !ocCeremonyProbeError.validation}
              <div>{ocCeremonyProbeError.address}:{ocCeremonyProbeError.port} · {(ocCeremonyProbeError.elapsedMs / 1000).toFixed(1)}s</div>
            {/if}
            <div>{ocCeremonyProbeError.message}</div>
            {#if !ocCeremonyProbeError.validation}
              <ul>
                <li>Pane Gateway is running on the OC machine</li>
                <li>Tailscale is connected on both machines</li>
                <li>The URL and port are correct</li>
              </ul>
            {/if}
          </div>
        {/if}
        <div class="npm-flow__foot">
          <button class="npm-btn npm-btn--quiet" onclick={cancelOcCeremony}>Cancel</button>
          <button class="npm-btn npm-btn--primary" onclick={probeOcCeremonyUrl} disabled={ocCeremonyProbing || !ocCeremonyUrlInput.trim()}>
            {ocCeremonyProbing ? 'Connecting…' : 'Start pairing'}
          </button>
        </div>
      {:else if ocCeremonyStep === 'verify'}
        <div class="npm-field">
          <span class="npm-label">TLS Fingerprint</span>
          <div class="npm-kv__v" style="word-break: break-all;">{ocCeremonyFingerprint}</div>
          <span class="npm-hint">Compare this fingerprint against the one Pane Gateway printed to its terminal on startup (on the OC machine).</span>
        </div>
        {#if ocCeremonyRejected}
          <div class="npm-test-err" data-testid="oc-ceremony-fingerprint-rejected">
            <div><strong>Fingerprint rejected</strong></div>
            <div>This usually means either the gateway address points somewhere unexpected, or the certificate changed since the last time you paired. The fingerprint above is left as-is so you can compare it again.</div>
          </div>
          <div class="npm-flow__foot">
            <span class="npm-spacer"></span>
            <button class="npm-btn npm-btn--primary" onclick={retryOcCeremonyAfterReject}>Start pairing again</button>
          </div>
        {:else}
          <div class="npm-flow__foot">
            <button class="npm-btn npm-btn--quiet" onclick={rejectOcCeremonyFingerprint}>Reject</button>
            <button class="npm-btn npm-btn--primary" onclick={acceptOcCeremonyFingerprint}>Accept</button>
          </div>
        {/if}
      {:else if ocCeremonyStep === 'code'}
        <div class="npm-field">
          <span class="npm-label">Pairing Code</span>
          <input
            type="text"
            class="npm-textin npm-textin--mono"
            bind:value={ocCeremonyCode}
            placeholder="000000"
            maxlength="6"
            inputmode="numeric"
            aria-label="Pairing Code"
            onkeydown={(e) => { if (e.key === 'Enter') exchangeOcCeremonyCode(); }}
          />
          <span class="npm-hint">Enter the 6-digit code from Pane Gateway's terminal on the OC machine. Codes expire after 5 minutes.</span>
        </div>
        {#if ocCeremonyCodeError}
          <div class="npm-test-err" data-testid="oc-ceremony-code-error">{ocCeremonyCodeError} The fingerprint you accepted is still valid, so only the code is being asked for again.</div>
          <div class="npm-flow__foot">
            <button class="npm-btn npm-btn--quiet" onclick={requestNewOcCeremonyCode}>Request new code</button>
            <button class="npm-btn npm-btn--primary" onclick={exchangeOcCeremonyCode} disabled={ocCeremonyExchanging || !ocCeremonyCode.trim()}>
              {ocCeremonyExchanging ? 'Connecting…' : 'Complete pairing'}
            </button>
          </div>
        {:else}
          <div class="npm-flow__foot">
            <button class="npm-btn npm-btn--quiet" onclick={cancelOcCeremony}>Cancel</button>
            <button class="npm-btn npm-btn--primary" onclick={exchangeOcCeremonyCode} disabled={ocCeremonyExchanging || !ocCeremonyCode.trim()}>
              {ocCeremonyExchanging ? 'Connecting…' : 'Complete pairing'}
            </button>
          </div>
        {/if}
      {:else if ocCeremonyStep === 'importing'}
        {#if ocCeremonyImporting}
          <div class="npm-flow__sub" data-testid="oc-ceremony-importing">Importing agents from the gateway…</div>
        {:else if ocCeremonyImportError}
          <!-- Matt's ruling #3 / Onboarding's retry-only-sync pattern: pairing already
               durably succeeded (token/fingerprint/cert/gateway_paired_at all written
               in phase 1) by the time this can render — Retry re-runs ONLY phase 2
               (runOcCeremonyImportPhase), never re-exchanges the one-time code. -->
          <div class="npm-test-err" data-testid="oc-ceremony-import-error">
            Paired, but importing agents from the gateway failed: {ocCeremonyImportError}
          </div>
          <div class="npm-flow__sub">Pairing itself succeeded — the gateway URL, token, and TLS fingerprint are already saved. You can retry the import without re-entering a pairing code.</div>
          <div class="npm-flow__foot">
            <span class="npm-spacer"></span>
            <button class="npm-btn npm-btn--primary" onclick={retryOcCeremonyImport} data-testid="oc-ceremony-retry-import">Retry</button>
          </div>
        {:else if ocCeremonyRepairDeclined}
          <div class="npm-flow__sub" data-testid="oc-ceremony-declined">Re-pair confirmation declined — your existing agents were left as-is.</div>
          <div class="npm-flow__foot">
            <span class="npm-spacer"></span>
            <button class="npm-btn npm-btn--primary" onclick={dismissOcCeremonyDeclined}>Done</button>
          </div>
        {/if}
      {/if}
    {/if}
  </div>
</div>

{#if showApiDisconnectConfirm}
  <!-- #2892: replaces the native confirm() that never renders in Tauri's
       WebView (silently resolves false per Apple's documented WKUIDelegate
       default). Same open/submit handler-split pattern as showOcRepairConfirm
       below and #2894's PromptDialog migration. -->
  <ConfirmDialog
    title="Disconnect {apiDisconnectLabel}"
    message={`Disconnect ${apiDisconnectLabel}? This will deactivate all agents on this provider and delete the saved key. You can reconnect later through New Provider.`}
    confirmLabel="Disconnect"
    confirmColor="#EF4444"
    onConfirm={confirmApiProviderDisconnect}
    onCancel={cancelApiProviderDisconnect}
  />
{/if}

{#if showOcRepairConfirm}
  <!-- D20 (#2649) pattern, ported verbatim from ProviderConnectionCard.svelte:
       replaces the native confirm() that never renders in Tauri's WebView
       (silently resolves false). Shown unconditionally on every re-pair,
       including zero detected collisions — copy branches on whether any exist. -->
  <ConfirmDialog
    title="Re-pair OpenClaw"
    message={ocRepairCollisions.length > 0
      ? `Switching to OpenClaw will overwrite ${ocRepairCollisions.length} local agent file${ocRepairCollisions.length === 1 ? '' : 's'} with the gateway's versions. Local edits will be lost.`
      : 'This will re-import your agents from the gateway. Continue?'}
    confirmLabel="Continue"
    onConfirm={confirmOcRepair}
    onCancel={cancelOcRepair}
  />
{/if}

{#if showImportedAgentsPopup}
  <!-- #2635 (D11 fix) pattern, ported from ProviderConnectionCard.svelte — shown
       after a successful sync_initial_import (first pair) / reimport_oc_agents
       (re-pair) call. Stacks its own scrim on top of this modal's scrim (a
       double-dim, functionally fine — see Reed's audit §4, same pattern
       DefaultAgentPickerModal-over-Settings already uses elsewhere). Dismissing
       calls onConnectionSaved then onClose (Commit 2/4: this is the modal's own
       terminal action now, not delegated to a caller). -->
  <ImportedAgentsModal
    agentNames={ocCeremonyImportedAgentNames}
    agentsMissingModel={ocCeremonyAgentsMissingModel}
    onDismiss={dismissImportedAgentsPopup}
  />
{/if}

<style>
  .npm-scrim {
    position: fixed;
    inset: 0;
    z-index: 1000;
    background: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .npm-modal {
    background: var(--surface-1);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow-pop);
    padding: 24px;
    max-width: 500px;
    width: 90%;
    max-height: 80vh;
    display: flex;
    flex-direction: column;
    font-family: var(--font-sans);
    overflow-y: auto;
  }

  .npm-modal__head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 16px;
  }

  .npm-modal__title {
    font-size: 18px;
    font-weight: 700;
    color: var(--fg-1);
    margin: 0;
  }

  /* #2522 design polish batch, item 4: X close button next to the step dots. */
  .npm-modal__head-right {
    display: flex;
    align-items: center;
    gap: 14px;
  }

  .npm-close-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 26px;
    height: 26px;
    padding: 0;
    background: transparent;
    border: none;
    border-radius: var(--radius-md);
    color: var(--fg-3);
    cursor: pointer;
    transition: background 0.15s, color 0.15s;
  }

  .npm-close-btn:hover {
    background: var(--overlay-hover);
    color: var(--fg-1);
  }

  /* Step dots — per PcNewProviderSteps caption: "dots, not a wizard bar". */
  .npm-steps {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .npm-steps i {
    display: inline-block;
    width: 6px;
    height: 6px;
    border-radius: var(--radius-pill);
    background: var(--border-strong);
    font-style: normal;
  }

  .npm-steps i.npm-step--on {
    background: var(--accent);
    width: 16px;
  }

  .npm-steps i.npm-step--done {
    background: var(--fg-3);
  }

  .npm-flow__head {
    margin-bottom: 16px;
  }

  .npm-eyebrow {
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--fg-3);
    margin-bottom: 6px;
  }

  .npm-flow__title {
    font-size: 18px;
    font-weight: 700;
    color: var(--fg-1);
    margin: 0 0 6px;
  }

  .npm-flow__sub {
    font-size: 13px;
    color: var(--fg-2);
    margin: 0;
    line-height: 1.5;
  }

  .npm-choose {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-bottom: 16px;
  }

  .npm-choice {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 12px;
    border-radius: var(--radius-md);
    border: 1px solid var(--border);
    background: var(--surface-2, transparent);
    cursor: pointer;
    text-align: left;
    width: 100%;
    color: var(--fg-1);
    font-family: inherit;
    transition: border-color 0.15s, background 0.15s;
  }

  .npm-choice:hover {
    border-color: var(--border-strong);
    background: var(--overlay-hover);
  }

  .npm-choice--sel {
    border-color: var(--accent);
    background: var(--accent-soft);
  }

  .npm-choice__logo {
    width: 30px;
    height: 30px;
    border-radius: 8px;
    background: var(--surface-2);
    color: var(--fg-2);
    font-size: 12px;
    font-weight: 600;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }

  .npm-choice__body {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .npm-choice__name {
    font-size: 14px;
    font-weight: 500;
    color: var(--fg-1);
  }

  .npm-choice__desc {
    font-size: 12px;
    color: var(--fg-3);
  }

  .npm-choice__chev {
    color: var(--fg-3);
    flex-shrink: 0;
  }

  .npm-choose__empty {
    font-size: 13px;
    color: var(--fg-3);
    padding: 16px 4px;
    text-align: center;
  }

  .npm-flow__foot {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }

  .npm-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    font-family: var(--font-sans);
    font-size: 13.5px;
    font-weight: 600;
    padding: 8px 16px;
    border-radius: var(--radius-md);
    border: 1px solid transparent;
    cursor: pointer;
    white-space: nowrap;
    transition: background 0.15s, color 0.15s, border-color 0.15s;
  }

  .npm-btn--quiet {
    background: transparent;
    color: var(--fg-2);
    border-color: var(--border);
  }

  .npm-btn--quiet:hover {
    background: var(--overlay-hover);
    color: var(--fg-1);
  }

  .npm-btn--primary {
    background: var(--accent);
    color: var(--fg-on-accent);
  }

  .npm-btn--primary:hover {
    background: var(--accent-hover);
  }

  .npm-btn--primary:disabled {
    cursor: not-allowed;
    background: var(--surface-2);
    color: var(--fg-4);
    border-color: var(--border-faint);
  }

  .npm-btn--ghost {
    background: transparent;
    color: var(--fg-2);
    border-color: var(--border);
  }

  .npm-btn--ghost:hover:not(:disabled) {
    background: var(--overlay-hover);
    color: var(--fg-1);
    border-color: var(--border-strong);
  }

  .npm-btn--ghost:disabled {
    cursor: not-allowed;
    color: var(--fg-4);
  }

  .npm-btn--sm {
    padding: 5px 10px;
    font-size: 12px;
  }

  .npm-btn--danger {
    background: transparent;
    color: #ef4444;
    border-color: rgba(239, 68, 68, 0.3);
  }

  .npm-btn--danger:hover:not(:disabled) {
    background: rgba(239, 68, 68, 0.08);
  }

  .npm-btn--danger:disabled {
    cursor: not-allowed;
    opacity: 0.5;
  }

  .npm-flow__foot__g {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  /* Step 2 API providers — API Key + Base URL fields (pc-c.jsx PcNewProvider2). */
  .npm-field {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-bottom: 14px;
  }

  .npm-label {
    font-size: 12px;
    font-weight: 600;
    color: var(--fg-2);
  }

  .npm-textin {
    font-family: var(--font-sans);
    font-size: 13.5px;
    color: var(--fg-1);
    background: var(--surface-2, transparent);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    padding: 8px 10px;
    width: 100%;
    box-sizing: border-box;
  }

  .npm-textin:focus {
    outline: none;
    border-color: var(--accent);
  }

  .npm-textin--mono {
    font-family: var(--font-mono);
    font-size: 13.5px;
  }

  .npm-hint {
    font-size: 11.5px;
    color: var(--fg-3);
  }

  .npm-test-ok {
    font-size: 12.5px;
    color: #22c55e;
    margin-bottom: 12px;
  }

  .npm-test-err {
    font-size: 12.5px;
    color: #ef4444;
    margin-bottom: 12px;
  }

  .npm-test-err--inline {
    margin: 8px 0 0;
  }

  /* Step 2 OpenClaw paired — summary card (pc-c.jsx PcNewProviderPaired). */
  .npm-eyebrow--accent {
    color: var(--accent);
  }

  .npm-paired {
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--surface-2, transparent);
    padding: 12px 14px;
    margin-bottom: 16px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .npm-paired__head {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .npm-paired__name {
    flex: 1;
    font-size: 14px;
    font-weight: 600;
    color: var(--fg-1);
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .npm-badge {
    font-size: 10.5px;
    font-weight: 600;
    padding: 2px 6px;
    border-radius: var(--radius-pill);
  }

  .npm-badge--paired {
    background: var(--accent-soft);
    color: var(--accent);
  }

  .npm-status {
    font-size: 12px;
    color: #22c55e;
    font-weight: 500;
  }

  .npm-paired__body {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .npm-kv {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    font-size: 12.5px;
  }

  .npm-kv__k {
    color: var(--fg-3);
  }

  .npm-kv__v {
    color: var(--fg-1);
    font-family: var(--font-mono);
    text-align: right;
    overflow-wrap: anywhere;
  }

  .npm-paired__foot {
    display: flex;
    align-items: center;
    gap: 8px;
    padding-top: 4px;
    border-top: 1px solid var(--border-faint, var(--border));
  }

  .npm-spacer {
    flex: 1;
  }
</style>

<!--
  ProviderConnectionCard.svelte — ADR-0063 D8, #2522 Stage 5b / Stage 7b

  Renders ONE provider's connection UI: identity row, config rows (key providers,
  Ollama, and OpenClaw), read-only agent roster, failure strip when the runtime is
  disconnected, and an action foot.
  Design: docs/design/agent-sessions-sprint/pc-b.jsx `PcCardAnatomy` /
  README-group-b.md "The provider card template" + "OpenClaw pairing" + "Sync now".

  D8 rulings this component follows:
   - No card is "the active one" — every connected provider is live simultaneously.
     This component never reads or writes a single global "current provider".
   - Roster is READ-ONLY: name, model, Default badge only. No create button, no
     set-default control, no provider switcher — agents never move between cards
     (cross-provider default selection lives in DefaultAgentPickerModal.svelte).
   - Save is the only primary action (anthropic/openrouter/ollama). Disconnect is
     quiet + destructive-coloured, never filled. Test connection sits apart from Save.
   - Failure names its consequence: the strip carries the backend error and the
     roster dims, matching the design's "blast radius" rule.

  OpenClaw pairing ceremony — #2642/#2630 (2026-08-22): this card no longer owns
  the pairing ceremony. It previously did (Stage 7b, #2522: a 3-step in-card
  `pairStep` state machine, re-homed from Onboarding.svelte's OC flow), but that
  entire ceremony — URL entry, fingerprint verify, pairing code, first-pair/
  re-pair routing, collision confirmation, and the post-import `ImportedAgentsModal`
  popup — has moved to `NewProviderModal.svelte`, which now owns it end-to-end.
  This card is manage-only for OpenClaw: read-only paired summary (Gateway URL,
  Gateway Token placeholder, TLS Fingerprint + copy, File sync + Sync now),
  Disconnect, and Test connection. Its "Pair gateway"/"Re-pair" button (label
  driven by `hasOcAgentRows`, see §2.3 below) calls the `onPairRequested` callback
  prop instead of starting a local ceremony — the caller (ProviderConnectionsPanel)
  opens `NewProviderModal` with its `startCeremony` prop set. A card that is
  `!info.connected` with no `info.error` is treated as "Not paired" (grey, a state
  not an error, per README-group-b.md §12); a card that is `!info.connected` WITH
  `info.error` is a genuine failing connection and keeps the existing red failure
  strip below. The already-paired card's read-only TLS Fingerprint row is fed by
  the `pairingFingerprint` prop threaded down from Settings.svelte (see
  Settings.svelte:145) so the ceremony (now in the modal) and this card's display
  share one source instead of double-fetching the same setting.

  "Has this pairing ever existed" (`hasOcAgentRows`, §2.3): driven by
  `invoke('oc_agent_row_exists')`, NOT `info.agents.length`/`info.connected` —
  after a disconnect, `ai_agents` rows are soft-deactivated (`is_active = 0`), not
  deleted, so `info.agents` goes to `[]` and `info.connected` goes to `false`,
  identical in shape to a never-paired install. `oc_agent_row_exists()` has no
  `is_active` filter and still returns `true` for a disconnected-but-not-deleted
  install — using the wrong signal here would mislabel a disconnected card as
  never-paired ("Pair gateway" instead of "Re-pair") and, worse, would tell the
  modal to route the next ceremony through `sync_initial_import` (fresh UUIDs)
  instead of `reimport_oc_agents` (reactivation) — exactly the D18 bug class
  (#2647/#2648, #2649) this codebase has already been bitten by twice.

  `pairSyncError` (the #2640 D13 fix's collapsed-card "⚠️ Import failed" badge):
  now an EXTERNAL prop, not internal ceremony state — threaded down from the Panel,
  which receives it from the modal's `onConnectionSaved?.(lastImportError?: string)`
  at the point a post-pairing import fails (Matt's ruling (c) for this PR).

  Sync Now (§14, Stage 7a/7b): the paired OpenClaw card's "File sync" row wires the
  ghost "Sync now" button to `syncNow()` (src/lib/api.ts, added in Stage 7a — thin
  wrapper over the `sync_now` Tauri command / `sync_now_blocking`). Success updates
  the row's timestamp to "Just now"; failure leaves the previous timestamp in place
  and surfaces the error on the card, per the design's "Sync now" ruling.

  Backend constraints that shape this component (see src-tauri/src/ai/commands.rs
  module comment above `ai_list_runtime_connections` for the authoritative version):
   - anthropic/openrouter currently share ONE keychain slot
     (`KEYCHAIN_ACCOUNT_PROVIDER_KEY`) — connecting one clears the other's stored
     credential. This is a pre-existing backend limitation, not introduced here;
     flagged in the Stage 5b completion summary, not fixed by this component.
   - Raw secrets are never sent to the frontend — `ai_list_runtime_connections`
     returns a masked display string only, and is `null` for openclaw specifically
     (only anthropic/openrouter get a masked string today). The paired card's
     "Gateway Token" row therefore renders a fixed masked placeholder rather than a
     real masked value — there is nothing real to mask without a backend change,
     which is out of this stage's scope (no backend edits here). An API-key field
     is always blank on load for anthropic/openrouter; the user must retype the
     value to change or re-test it. Save stays disabled until a fresh value is
     entered so a blank field can never silently clobber a working credential.
   - Disconnect (`ai_disconnect_runtime`, PR2 #2647/#2648) is a soft-deactivate
     (unpair), not a delete: server-side it clears all seven OC settings keys
     (`endpoint` / `gateway_tls_fingerprint` / `gateway_tls_cert_pem` /
     `gateway_paired_at` / `tier3_topology` / `last_sync_push` / `api_key`) and
     deactivates + demotes every OC agent row (`is_active = 0`, `is_default = 0`).
     `chat_sessions.agent_id` is left untouched (D21, #2649 — disconnect never
     NULLs or reassigns session bindings; a session bound to the now-deactivated
     agent renders read-only per docs/spec/sessions.md's "Unavailable bound agent"
     rule instead). Directories/files are never touched either, so a later re-pair
     reactivates the same UUIDs. This component does not duplicate that logic; it
     shows a `ConfirmDialog` naming the concrete consequence (agent count), then
     resets its own local pairing/sync UI state and calls `onChange()`.
-->
<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import type { RuntimeConnectionInfo, SyncNowResult } from '$lib/api';
  import { disconnectRuntime, syncNow } from '$lib/api';
  import ConfirmDialog from './ConfirmDialog.svelte';

  /** Thin wrapper over the existing `ai_configure_and_save` command (unchanged in this
   *  stage — see src-tauri/src/ai/commands.rs). Per-card equivalent of what
   *  Settings.svelte's single-provider form used to call directly. Anthropic/OpenRouter/
   *  Ollama share this path; OpenClaw's connect path (pairing ceremony) now lives
   *  entirely in NewProviderModal.svelte — see this component's module comment. */
  async function configureAndSave(
    runtimeType: 'anthropic' | 'openrouter' | 'ollama',
    apiKey: string | null,
    endpoint: string | null,
  ): Promise<void> {
    await invoke('ai_configure_and_save', {
      provider: runtimeType,
      apiKey,
      model: null,
      endpoint,
      simpleTierModel: null,
    });
  }

  let {
    info,
    onChange,
    pairingFingerprint = $bindable(''),
    onPairRequested,
    pairSyncError = '',
  }: {
    info: RuntimeConnectionInfo;
    onChange: () => void | Promise<void>;
    /** Shared with Settings.svelte (see Settings.svelte:145) so the paired card's
     *  truncated TLS Fingerprint row and Settings' own load-on-mount logic read/write
     *  the same value instead of each independently re-fetching `gateway_tls_fingerprint`. */
    pairingFingerprint?: string;
    /** #2642/#2630 Commit 3: replaces the in-card pairing ceremony entirely —
     *  the card no longer holds any ceremony state (pairStep et al., deleted this
     *  commit). Clicking "Pair gateway"/"Re-pair" now calls this instead, with
     *  `mode` derived from `hasOcAgentRows` (Commit 1) so the caller
     *  (ProviderConnectionsPanel, wired in Commit 5) knows which ceremony entry
     *  point to deep-link NewProviderModal.svelte's `startCeremony` prop into,
     *  without needing to re-derive it itself. */
    onPairRequested?: (mode: 'pair' | 're-pair') => void;
    /** #2642/#2630 Commit 3/4, Matt's ruling (c): the #2640 D13 fix's collapsed-card
     *  "⚠️ Import failed" badge has no more local ceremony state to read from now
     *  that pairing lives in NewProviderModal.svelte — this is an EXTERNAL prop
     *  instead, threaded down from ProviderConnectionsPanel.svelte (Commit 5),
     *  which itself receives it from the modal's `onConnectionSaved?.(lastImportError?:
     *  string)` (Commit 4) at the point a phase-2 (import) failure happens.
     *  Deliberately NOT internal `$state` here — the card owns zero ceremony
     *  knowledge post-refactor, only this passthrough display value plus its
     *  existing `onChange()` reload. */
    pairSyncError?: string;
  } = $props();

  // Ride-along 4 fix (#2777 venice live-test scoping): both maps were missing a
  // `venice` entry, so the Venice card fell through to the `?? info.runtime_type`
  // / `?? '??'` fallbacks in the template below and rendered "venice" / "??"
  // instead of "Venice" / "VE". Every other runtime (including venice) already has
  // an entry in Settings.svelte's own RUNTIME_TYPE_LABELS / NewProviderModal's
  // PROVIDER_ROWS ('VE' logo) — this map just hadn't been kept in sync with those.
  const RUNTIME_LABELS: Record<string, string> = {
    openclaw: 'OpenClaw',
    openrouter: 'OpenRouter',
    anthropic: 'Anthropic',
    ollama: 'Ollama',
    venice: 'Venice',
  };
  const RUNTIME_LOGOS: Record<string, string> = {
    openclaw: 'OC',
    openrouter: 'OR',
    anthropic: 'AN',
    ollama: 'OL',
    venice: 'VE',
  };

  let expanded = $state(false);

  // Editable fields — always start blank, never prefilled from a raw secret (backend
  // never sends one). See module comment above. N/A for OpenClaw (read-only in this stage).
  let apiKeyInput = $state('');
  let showApiKey = $state(false);
  let ollamaEndpointInput = $state(info.endpoint ?? 'http://localhost:11434');

  let testStatus = $state<'idle' | 'testing' | 'success' | 'error'>('idle');
  let testMessage = $state('');
  let saving = $state(false);
  let saveError = $state('');
  let saveSuccess = $state(false);
  let disconnecting = $state(false);
  // PR2 (#2647/#2648): OC disconnect is a soft-deactivate (unpair) that takes agents
  // offline until re-pair — the native confirm() this replaces named nothing concrete.
  // #2892: anthropic/openrouter/ollama now use the same styled dialog (below),
  // replacing the native confirm() they used to fall back to.
  let showDisconnectConfirm = $state(false);
  let showApiDisconnectConfirm = $state(false); // parallel to showDisconnectConfirm, non-OC runtimes (#2892)
  // #2522 design polish batch, item 5: presentation-only acknowledgment that a
  // Disconnect click on the Ollama card was received. Ollama's backend disconnect
  // path (`ai_disconnect_runtime` "ollama" arm) has no stored credential to clear
  // today and is a documented no-op — giving Disconnect real semantics for Ollama
  // (e.g. an explicit "user disconnected" flag distinct from reachability) is a
  // backend change, out of scope for this visual-only batch.
  // B3: Disconnect semantics TBD — this is presentation-only feedback.
  let ollamaDisconnectAck = $state(false);

  // D8 (ADR-0065): Venice "Use Venice system prompt" toggle.
  // Persisted as venice_system_prompt_enabled in ai_settings ("1" / "0").
  // Default (absent) = off. NOT cleared on disconnect — preference, not connection state.
  let veniceSystemPromptEnabled = $state(false);

  async function loadVeniceSystemPromptSetting() {
    try {
      const stored = (await invoke('get_ai_setting', { key: 'venice_system_prompt_enabled' })) as string | null;
      veniceSystemPromptEnabled = stored === '1';
    } catch {
      // Non-fatal — default remains false.
    }
  }

  async function handleVeniceSystemPromptToggle() {
    try {
      await invoke('set_ai_setting', {
        key: 'venice_system_prompt_enabled',
        value: veniceSystemPromptEnabled ? '1' : '0',
      });
    } catch (e) {
      console.error('[ProviderConnectionCard] Failed to set venice_system_prompt_enabled:', e);
    }
  }

  $effect(() => {
    if (info.runtime_type === 'venice') {
      loadVeniceSystemPromptSetting();
    }
  });

  const isKeyProvider = $derived(info.runtime_type === 'anthropic' || info.runtime_type === 'openrouter' || info.runtime_type === 'venice');
  const isOllama = $derived(info.runtime_type === 'ollama');
  const isOpenClaw = $derived(info.runtime_type === 'openclaw');

  // Moved above `canTest`/`canSave` (was previously declared further down, next to
  // `unconfigured`) so both derived values below can reference it without a
  // TS "used before declaration" error — `$derived` closures resolve lazily at
  // runtime regardless of declaration order, but svelte-check's TS pass still
  // enforces block-scope TDZ rules on the source order.
  //
  // #2641 D14 Commit 4: `ocUnpaired` is now TEMPLATE-ONLY — it still decides which
  // BODY renders (the "Pair gateway"/"Re-pair" ceremony-entry body vs. the paired
  // config-rows body), which is a connectivity-shape question the existing signal
  // already answers correctly (`!info.connected && !info.error` is true both for
  // never-paired AND for a soft-deactivated disconnect, and both cases want the
  // "unpaired" body, just with different copy driven by hasOcAgentRows below). It is
  // NO LONGER used to decide card COLOR — see `cardStatus` below, which uses
  // `hasOcAgentRows` per Matt's ruling instead. Reed's audit §1.5 flagged the pre-fix
  // version of this file as using `ocUnpaired` for BOTH jobs, which is exactly the
  // gap this commit closes.
  const ocUnpaired = $derived(isOpenClaw && !info.connected && !info.error);

  // #2642/#2630 Commit 1: "Pair" vs "Re-pair" must be driven by oc_agent_row_exists()
  // (a DB row-existence check with no is_active filter), NOT by info.agents.length or
  // info.connected. After a disconnect, ai_agents rows are soft-deactivated
  // (is_active = 0), not deleted — ai_list_runtime_connections filters on is_active = 1,
  // so info.agents goes to [] and info.connected goes to false, identical in shape to an
  // install that has NEVER paired. oc_agent_row_exists() has no is_active filter and still
  // returns true for a disconnected-but-not-deleted install. Using ocUnpaired/info.agents
  // as the "has this ever been paired" signal would mislabel a disconnected card as
  // never-paired ("Pair" instead of "Re-pair") and — worse — would route the next ceremony
  // into sync_initial_import (fresh UUIDs) instead of reimport_oc_agents (reactivation),
  // exactly the D18 bug class (#2647/#2648, #2649) this codebase has already been bitten
  // by twice. See Reed's audit §2.3 (2026-08-22-2642-2630-new-provider-oc-scope.md).
  let hasOcAgentRows = $state<boolean | null>(null);

  async function loadHasOcAgentRows() {
    try {
      // `?? null` guards against a test double / mock resolving to `undefined`
      // (e.g. a blanket `invokeMock.mockResolvedValue(undefined)` default that
      // doesn't special-case this command) — keeps hasOcAgentRows's declared
      // `boolean | null` type honest and routes back through the ocUnpaired
      // fallback guess below rather than silently coercing to `false`.
      hasOcAgentRows = ((await invoke('oc_agent_row_exists')) as boolean | undefined) ?? null;
    } catch (e) {
      console.error('[ProviderConnectionCard] Failed to check oc_agent_row_exists:', e);
      // Non-fatal — button falls back to the ocUnpaired-derived guess below until
      // the next successful check (e.g. after onChange() re-fetches).
    }
  }

  // Recompute whenever info changes (paired/unpaired transition, roster reload after
  // a pairing ceremony completes) so the label doesn't go stale until a full remount.
  $effect(() => {
    if (isOpenClaw) {
      loadHasOcAgentRows();
    }
  });

  /** Sane fallback while hasOcAgentRows === null (first render, before the invoke
   *  resolves) — avoids hiding the button or flashing wrong copy for a full round-trip.
   *  Once hasOcAgentRows resolves, it is authoritative regardless of what this guessed. */
  const ocPairButtonLabel = $derived(
    hasOcAgentRows === null ? (ocUnpaired ? 'Pair gateway' : 'Re-pair') : hasOcAgentRows ? 'Re-pair' : 'Pair gateway'
  );

  const canSave = $derived(
    isKeyProvider ? apiKeyInput.trim() !== '' :
    isOllama ? true :
    false // openclaw/venice: read-only card once configured; Save is in NewProviderModal
  );

  // Ride-along 7 fix (#2777 venice live-test scoping): the Test button was gated
  // on `apiKeyInput.trim() !== ''`, so an already-configured key-provider card
  // (the common case now that Blocker 3's fix means a card only renders once
  // configured) never showed Test at all unless the user retyped their key —
  // OpenClaw and Ollama both render Test unconditionally once configured/paired.
  // Key providers now get the same parity: Test always renders when the card is a
  // key provider; `testConnection()` below surfaces a clear "enter a key" error if
  // clicked with nothing typed, rather than silently sending a blank key.
  const canTest = $derived(
    isKeyProvider ? true :
    isOllama ? true :
    // #2590 UX batch 2, item 3: OC gets Test connection parity with the other
    // three provider cards. Only shown once paired (`!ocUnpaired`) — same as
    // `unconfigured` below, there's nothing to test on a never-paired slot,
    // and the unpaired body doesn't render a footer at all (see `pcc-body`'s
    // "Pair gateway" button being the only action in that state). Left
    // available while paired-but-erroring so Test can help diagnose a
    // failing gateway, same as it would for a failing key/endpoint.
    isOpenClaw ? !ocUnpaired :
    false
  );

  function resetTransientState() {
    testStatus = 'idle';
    testMessage = '';
    saveError = '';
    saveSuccess = false;
    ollamaDisconnectAck = false;
  }

  // §28.2 / #1630 fix: shared per-runtime validator dispatch, factored out of
  // `testConnection` so `saveConnection` can call the exact same validation before
  // `configureAndSave` instead of duplicating (and risking drift from) the Test
  // button's logic. Throws on failure (validators throw/reject); callers decide
  // what to do with that (surface as testMessage, or abort save with saveError).
  async function runKeyProviderValidator(): Promise<void> {
    // F2 fix (#2455 venice live-test v2): Test previously always sent whatever
    // was typed in the (usually blank, since a configured card doesn't prefill
    // the input) apiKeyInput field, silently validating an empty string against
    // the stored key's actual working credential — "Test" on an already-connected
    // card would report failure even though the saved key was fine. The three
    // validate_* commands now accept `apiKey: string | null`; a blank/untyped
    // input is passed through as `null` so the backend falls back to reading the
    // stored keychain credential server-side instead of validating an empty
    // string. Only an explicitly-typed non-blank key overrides the stored one —
    // this lets Test also verify a NEW key before Save, same as before.
    const key = apiKeyInput.trim() === '' ? null : apiKeyInput;
    if (info.runtime_type === 'anthropic') {
      await invoke('validate_anthropic_key', { apiKey: key });
    } else if (info.runtime_type === 'openrouter') {
      await invoke('validate_openrouter_key', { apiKey: key });
    } else if (info.runtime_type === 'venice') {
      // ADR-0065 D8: validate via GET /api_keys/rate_limits
      await invoke('validate_venice_key', { apiKey: key });
    }
  }

  async function testConnection() {
    resetTransientState();
    testStatus = 'testing';
    try {
      if (isKeyProvider) {
        await runKeyProviderValidator();
      } else if (info.runtime_type === 'ollama') {
        const result = (await invoke('ollama_probe_two_step', { endpoint: ollamaEndpointInput })) as {
          models: string[];
          error_class: string | null;
        };
        if (result.error_class) {
          throw new Error(
            result.error_class === 'unreachable'
              ? `Couldn't reach Ollama at ${ollamaEndpointInput}. Make sure it's running.`
              : result.error_class === 'no_models'
                ? 'Ollama is running but no models are installed.'
                : "Connected to Ollama but couldn't list models."
          );
        }
      } else if (info.runtime_type === 'openclaw') {
        // #2590 UX batch 2, item 3: parity Test connection for the paired OC card.
        // Reuses `gateway_fetch_fingerprint` (notes.rs) — the same reachability probe
        // NewProviderModal.svelte's pairing ceremony Step 1 (`probeOcCeremonyUrl`)
        // already calls against this gateway's `/v1/health` — rather than adding a
        // new backend command for
        // what is, functionally, the identical check: "is this gateway URL reachable
        // and does it look like a Pane Gateway". No credential is sent (health is
        // unauthenticated, same as pairing Step 1); this only confirms the gateway
        // itself answers, which is exactly what "Test connection" means for a
        // read-only paired card with no key/endpoint fields to (re)submit.
        if (!info.endpoint) {
          throw new Error('No Gateway URL configured.');
        }
        await invoke('gateway_fetch_fingerprint', { url: info.endpoint });
      }
      testStatus = 'success';
      testMessage = 'Connected';
    } catch (e: any) {
      testStatus = 'error';
      testMessage = e?.toString?.() ?? String(e);
    }
  }

  async function saveConnection() {
    resetTransientState();
    saving = true;
    try {
      if (isKeyProvider) {
        // §28.2 / #1630 fix: validate the key BEFORE ai_configure_and_save. Previously
        // Save called configureAndSave() directly with zero reference to testStatus/
        // testMessage — a bad key always showed green because nothing ever checked
        // it before persisting. Abort with a visible error state on validation
        // failure; configureAndSave() (and therefore `{runtime}_configured_at`,
        // which drives the Blocker-3 card-visibility filter) is only reached after
        // this succeeds.
        try {
          await runKeyProviderValidator();
        } catch (e: any) {
          saveError = e?.toString?.() ?? String(e);
          return;
        }
        await configureAndSave(info.runtime_type as 'anthropic' | 'openrouter', apiKeyInput, null);
        apiKeyInput = '';
      } else if (isOllama) {
        await configureAndSave('ollama', null, ollamaEndpointInput);
      }
      saveSuccess = true;
      await onChange();
    } catch (e: any) {
      saveError = e?.toString?.() ?? String(e);
    } finally {
      saving = false;
    }
  }

  function handleDisconnect() {
    if (isOpenClaw) {
      showDisconnectConfirm = true;
      return;
    }
    showApiDisconnectConfirm = true;
  }

  async function confirmApiDisconnect() {
    showApiDisconnectConfirm = false;
    await performDisconnect();
  }

  function cancelApiDisconnect() {
    showApiDisconnectConfirm = false;
  }

  async function performDisconnect() {
    resetTransientState();
    disconnecting = true;
    try {
      await disconnectRuntime(info.runtime_type);
      if (isOpenClaw) {
        // Backend already wiped gateway_tls_fingerprint/cert_pem/last_sync_push
        // (§22C.11A.4) — mirror that locally so this card's UI doesn't show stale
        // pairing/sync state for the instant before onChange() re-fetches.
        pairingFingerprint = '';
        syncTimestamp = null;
      }
      if (isOllama) {
        // B3: Disconnect semantics TBD — this is presentation-only feedback.
        // The backend call above still runs (existing behavior, unchanged), but
        // since its "ollama" arm is a documented no-op (no stored credential to
        // clear), this card would otherwise show no acknowledgment at all that
        // the click did anything. Visible-only: clears itself the next time the
        // card's transient state resets (re-open, new action, etc.).
        ollamaDisconnectAck = true;
      }
      await onChange();
    } catch (e: any) {
      saveError = e?.toString?.() ?? String(e);
    } finally {
      disconnecting = false;
    }
  }

  async function confirmOpenClawDisconnect() {
    showDisconnectConfirm = false;
    await performDisconnect();
  }

  function cancelOpenClawDisconnect() {
    showDisconnectConfirm = false;
  }

  // #2641 D14 three-state color fix: replaces the old binary `unconfigured` derivation.
  // Three states, driven by the backend's `configured_this_install` signal (Commit 1)
  // for anthropic/openrouter/ollama — NOT by `info.connected`/`masked_credential`
  // presence, which is exactly what let a stale keychain leftover from a previous
  // install (#2631) show green without the user ever touching this provider this
  // install. Matt's ruling:
  //   grey ('unconfigured')  = !info.configured_this_install
  //   green ('active')       = info.configured_this_install && !info.error
  //   red ('failed')         = info.configured_this_install && info.error
  // For OpenClaw specifically, Matt's ruling additionally says a user-initiated
  // disconnect (paired, not connected, no error) stays GREY ("Paired, disconnected"),
  // not red — red is reserved for `hasOcAgentRows === true && info.error` (a genuine
  // failure). That case is folded into 'unconfigured' below (still grey) since the
  // badge/pill only carry 3 colors, not 4 — `statusText` below differentiates the
  // COPY ("Not paired" vs "Paired, disconnected") without adding a 4th color.
  const cardStatus = $derived<'unconfigured' | 'active' | 'failed'>(
    isOpenClaw
      ? hasOcAgentRows !== true
        ? 'unconfigured' // never paired (false), or still loading (null) — grey either way
        : info.error
          ? 'failed' // paired + a real error — genuine failure, red
          : info.connected
            ? 'active' // paired + connected + no error — green
            : 'unconfigured' // paired but not connected, no error — user-initiated disconnect, grey per Matt's ruling
      : !info.configured_this_install
        ? 'unconfigured'
        : info.error
          ? 'failed'
          : 'active'
  );

  /** Status text shown in the collapsed-header status slot + badge. OpenClaw has three
   *  distinct grey/red copy variants depending on `hasOcAgentRows`; other runtime
   *  types only ever show "Not configured" for the grey state. The failure strip below
   *  (rendered from `info.error` directly) carries the actual error message — this
   *  pill stays the short "Disconnected" label, matching pre-fix behavior/tests. */
  const statusText = $derived(
    cardStatus === 'unconfigured'
      ? (isOpenClaw ? (hasOcAgentRows === true ? 'Paired, disconnected' : 'Not paired') : 'Not configured')
      : cardStatus === 'failed'
        ? 'Disconnected'
        : 'Connected'
  );

  /** C1 (#2585): `migrate_v83` seeds a `'openclaw-pending'` sentinel into `ai_agents.model`
   *  for OC rows that were left without a real model value (see migrations.rs). That
   *  sentinel is an internal implementation detail — it's never sent on the wire
   *  (openrouter.rs's `stream_openrouter_inner` unconditionally rewrites it to
   *  "openclaw" before the request leaves the machine) and D9's write-back replaces
   *  it with the real resolved model on the agent's first successful dispatch. Users
   *  should never see the literal placeholder string here — mask it with a neutral
   *  message instead. */
  function formatAgentModel(model: string | null): string {
    if (model === 'openclaw-pending') return 'resolves on first message';
    return model || '—';
  }

  function truncateFingerprint(fp: string): string {
    if (fp.length <= 24) return fp;
    return `${fp.slice(0, 12)}…${fp.slice(-10)}`;
  }

  async function copyFingerprint() {
    try {
      await navigator.clipboard.writeText(pairingFingerprint);
    } catch {
      // Clipboard API unavailable — fingerprint stays selectable in the DOM either way.
    }
  }

  // ---------------------------------------------------------------------------
  // Sync Now (#2522 §14, Stage 7a/7b) — wires the paired card's ghost button to
  // `syncNow()` (src/lib/api.ts, Stage 7a's thin wrapper over the `sync_now` command).
  // ---------------------------------------------------------------------------
  let syncTimestamp = $state<string | null>(null);
  let syncNowRunning = $state(false);
  let syncNowError = $state('');

  async function loadSyncTimestamp() {
    try {
      const stored = (await invoke('get_ai_setting', { key: 'last_sync_push' })) as string | null;
      if (stored) syncTimestamp = stored;
    } catch {
      // Non-fatal — row just shows "Never" until the next successful sync.
    }
  }

  async function loadStoredFingerprintIfMissing() {
    if (pairingFingerprint) return;
    try {
      const stored = (await invoke('get_ai_setting', { key: 'gateway_tls_fingerprint' })) as string | null;
      if (stored) pairingFingerprint = stored;
    } catch {
      // Non-fatal — TLS Fingerprint row just stays hidden until Settings.svelte loads it.
    }
  }

  function formatSyncTimestamp(ts: string | null): string {
    if (!ts) return 'Never';
    const then = new Date(ts).getTime();
    if (Number.isNaN(then)) return ts;
    const diffMs = Date.now() - then;
    const minutes = Math.floor(diffMs / 60000);
    if (minutes < 1) return 'Just now';
    if (minutes < 60) return `${minutes}m ago`;
    const hours = Math.floor(minutes / 60);
    if (hours < 24) return `${hours}h ago`;
    const days = Math.floor(hours / 24);
    return `${days}d ago`;
  }

  async function handleSyncNow() {
    syncNowError = '';
    syncNowRunning = true;
    try {
      const result: SyncNowResult = await syncNow();
      // On success the timestamp updates to "Just now"; on failure the row keeps its
      // previous timestamp (syncTimestamp is untouched in the catch branch) while the
      // error surfaces separately, per README-group-b.md "Sync now".
      syncTimestamp = result.timestamp;
    } catch (e: any) {
      syncNowError = e?.toString?.() ?? String(e);
    } finally {
      syncNowRunning = false;
    }
  }

  $effect(() => {
    if (isOpenClaw && info.connected) {
      loadSyncTimestamp();
      loadStoredFingerprintIfMissing();
    }
  });
</script>

<div class="pcc-card" class:pcc-card--attn={cardStatus === 'failed'} data-runtime={info.runtime_type}>
  <button class="pcc-top" onclick={() => (expanded = !expanded)} aria-expanded={expanded}>
    <span class="pcc-logo" data-runtime={info.runtime_type}>{RUNTIME_LOGOS[info.runtime_type] ?? '??'}</span>
    <span class="pcc-id">
      <span class="pcc-name">
        {RUNTIME_LABELS[info.runtime_type] ?? info.runtime_type}
        {#if isOllama}<span class="pcc-badge pcc-badge--local">Local</span>{/if}
        {#if cardStatus === 'unconfigured'}<span class="pcc-badge pcc-badge--unpaired">{statusText}</span>{/if}
      </span>
      <span class="pcc-sub">
        {#if isKeyProvider}
          {info.masked_credential ?? 'Not configured'}
        {:else if isOllama || isOpenClaw}
          {info.endpoint ?? 'Not configured'}
        {/if}
      </span>
    </span>
    <span class="pcc-actions">
      {#if pairSyncError}
        <!-- #2640 D13 FIX: collapsed-card error indicator, driven off pairSyncError.
             Before this, pairSyncError only rendered inside {#if expanded} (see the
             pcc-error block below), so a collapsed card whose OC pairing succeeded
             but whose agent import failed and rolled back would show "Connected"
             with zero visual signal that anything needs attention (Reed's audit
             §7). Does NOT change info.connected semantics — that's D14 (#2641)
             scope — this is a purely additive indicator alongside the existing
             status text. -->
        <span class="pcc-status-warn" title={`Paired, but importing agents failed: ${pairSyncError}`} data-testid="oc-pair-sync-error-badge">
          ⚠️ Import failed
        </span>
      {/if}
      <span class="pcc-status" class:pcc-status--bad={cardStatus === 'failed'} class:pcc-status--neutral={cardStatus === 'unconfigured'}>
        {statusText}
      </span>
      <span class="pcc-caret" class:pcc-caret--open={expanded}>▾</span>
    </span>
  </button>

  {#if expanded}
    {#if info.error}
      <div class="pcc-fail">
        <span>⚠️</span>
        <span>
          {info.error}
          {#if info.agents.length > 0}
            Agents on {RUNTIME_LABELS[info.runtime_type] ?? info.runtime_type} cannot answer until this reconnects.
          {/if}
        </span>
      </div>
    {/if}

    {#if isKeyProvider}
        <div class="pcc-body">
          <div class="pcc-row">
            <div class="pcc-row-main">
              <div class="pcc-row-label">API Key</div>
            </div>
            <div class="pcc-row-ctrl">
              <div class="password-wrapper">
                <input
                  type={showApiKey ? 'text' : 'password'}
                  class="input input-wide"
                  bind:value={apiKeyInput}
                  placeholder={info.masked_credential ?? 'Enter API key'}
                  oninput={resetTransientState}
                />
                <!-- #2783: apiKeyInput always starts empty (masked_credential only
                     ever renders as the input's placeholder, never its value — see
                     `masked_credential` above and commands.rs's mask_secret(), which
                     never round-trips a raw stored secret to the renderer). Under
                     type="password" vs type="text", browsers render placeholder text
                     identically — no dots either way — so with nothing typed this
                     session, the toggle fires and flips `type` correctly but there's
                     no visible content for that to affect, reading as "does nothing"
                     (docs/audits/2026-08-31-2794-2783-formal-diagnosis.md, #2783).
                     Disabling the button when apiKeyInput is empty removes the false
                     "reveal my stored key" affordance instead of leaving a toggle that
                     looks broken; the button re-enables the moment the user types. -->
                <button
                  class="password-toggle"
                  disabled={apiKeyInput.trim() === ''}
                  onclick={() => (showApiKey = !showApiKey)}
                >{showApiKey ? 'Hide' : 'Show'}</button>
              </div>
            </div>
          </div>
          {#if info.runtime_type === 'venice'}
            <!-- D8 (ADR-0065): Venice-only system prompt toggle. -->
            <div class="pcc-row">
              <div class="pcc-row-main">
                <div class="pcc-row-label">Use Venice system prompt</div>
              </div>
              <div class="pcc-row-ctrl pcc-row-ctrl--end">
                <input
                  type="checkbox"
                  bind:checked={veniceSystemPromptEnabled}
                  onchange={handleVeniceSystemPromptToggle}
                  data-testid="venice-system-prompt-toggle"
                />
              </div>
            </div>
          {/if}
        </div>
      {:else if isOllama}
        <div class="pcc-body">
          <div class="pcc-row">
            <div class="pcc-row-main">
              <div class="pcc-row-label">Endpoint</div>
            </div>
            <div class="pcc-row-ctrl">
              <input type="text" class="input input-wide" bind:value={ollamaEndpointInput} placeholder="http://localhost:11434" oninput={resetTransientState} />
            </div>
          </div>
        </div>
      {:else if isOpenClaw}
        {#if ocUnpaired}
          <!-- §12: unpaired is a state, not an error — grey, "Not paired" in badge + status.
               Note + button copy is driven by hasOcAgentRows (Commit 1, #2642/#2630 fix),
               not ocUnpaired — a disconnected-but-previously-paired install (rows still
               exist, is_active = 0) must read "Re-pair", never "Pair gateway". Button now
               calls onPairRequested (Commit 3) instead of opening an in-card ceremony —
               the ceremony itself moved to NewProviderModal.svelte. -->
          <div class="pcc-body">
            <p class="pcc-note">
              {hasOcAgentRows ? 'This OpenClaw slot was disconnected.' : 'This OpenClaw slot is not paired with a gateway yet.'}
            </p>
            <div class="pcc-ceremony-actions">
              <button class="btn btn-sm btn-primary" data-testid="oc-pair-gateway" onclick={() => onPairRequested?.(hasOcAgentRows ? 're-pair' : 'pair')}>{ocPairButtonLabel}</button>
            </div>
          </div>
        {:else}
          <!-- Paired card config rows (design §3): URL, token, fingerprint, file sync -->
          <div class="pcc-body">
            <div class="pcc-row">
              <div class="pcc-row-main">
                <div class="pcc-row-label">Gateway URL</div>
              </div>
              <div class="pcc-row-ctrl pcc-readonly">{info.endpoint ?? 'Not configured'}</div>
            </div>
            <div class="pcc-row">
              <div class="pcc-row-main">
                <div class="pcc-row-label">Gateway Token</div>
              </div>
              <div class="pcc-row-ctrl pcc-row-ctrl--split">
                <span class="pcc-readonly pcc-readonly--inline">••••••••••••</span>
                <button class="btn btn-sm" data-testid="oc-re-pair" onclick={() => onPairRequested?.(hasOcAgentRows ? 're-pair' : 'pair')}>{ocPairButtonLabel}</button>
              </div>
            </div>
            {#if pairingFingerprint}
              <div class="pcc-row">
                <div class="pcc-row-main">
                  <div class="pcc-row-label">TLS Fingerprint</div>
                </div>
                <div class="pcc-row-ctrl pcc-row-ctrl--split">
                  <span class="pcc-readonly pcc-readonly--inline">{truncateFingerprint(pairingFingerprint)}</span>
                  <button class="pcc-btn-ghost" onclick={copyFingerprint} title="Copy full fingerprint">Copy</button>
                </div>
              </div>
            {/if}
            <div class="pcc-row">
              <div class="pcc-row-main">
                <div class="pcc-row-label">File sync</div>
                <div class="pcc-row-sublabel">{formatSyncTimestamp(syncTimestamp)}</div>
              </div>
              <div class="pcc-row-ctrl pcc-row-ctrl--end">
                <button class="pcc-btn-ghost" onclick={handleSyncNow} disabled={syncNowRunning}>
                  {syncNowRunning ? 'Syncing…' : 'Sync now'}
                </button>
              </div>
            </div>
            {#if syncNowError}
              <div class="pcc-error pcc-error--inline">{syncNowError}</div>
            {/if}
          </div>
        {/if}
        {#if pairSyncError}
          <!-- #2635 (D11 fix), Matt's ruling (c): now an EXTERNAL prop (Panel-threaded,
               fed by the modal's onConnectionSaved(lastImportError), Commits 4/5) instead
               of internal ceremony state — this component has no ceremony knowledge left.
               Rendered regardless of ocUnpaired since onChange()'s reload (which flips
               ocUnpaired to false) races this same tick — a reader must be able to see
               the warning whichever body renders first. "Sync now" (paired body, above)
               can be retried once the gateway is reachable again. -->
          <div class="pcc-body">
            <div class="pcc-error pcc-error--inline" data-testid="oc-pair-sync-error">
              Paired, but importing agents from the gateway failed: {pairSyncError}
            </div>
          </div>
        {/if}
      {/if}

      <div class="pcc-roster">
        <div class="pcc-roster-head">
          <span class="pcc-roster-label">Agents</span>
          <span class="pcc-roster-count">{info.agents.length}</span>
          <span class="pcc-roster-note">Manage on the Agents tab</span>
        </div>
        {#each info.agents as agent (agent.id)}
          <div class="pcc-agent" class:pcc-agent--off={!info.connected}>
            <span class="pcc-agent-name">
              {agent.name}
              {#if agent.is_default}<span class="pcc-badge pcc-badge--default">Default</span>{/if}
            </span>
            <span class="pcc-agent-model">{formatAgentModel(agent.model)}</span>
          </div>
        {/each}
      </div>

      {#if saveError}
        <div class="pcc-error">{saveError}</div>
      {/if}
      {#if testStatus === 'success'}
        <div class="pcc-test-ok">✓ {testMessage}</div>
      {:else if testStatus === 'error'}
        <div class="pcc-test-err">✗ {testMessage}</div>
      {/if}
      {#if saveSuccess}
        <div class="pcc-test-ok">✓ Saved</div>
      {/if}
      {#if isOllama && ollamaDisconnectAck}
        <!-- #2522 design polish batch, item 5: presentation-only feedback for the
             Ollama Disconnect click. B3: Disconnect semantics TBD. -->
        <div class="pcc-test-ok" data-testid="ollama-disconnect-ack">✓ Disconnect requested</div>
      {/if}

      <div class="pcc-foot">
        {#if canTest}
          <button
            class="btn btn-sm"
            onclick={testConnection}
            disabled={testStatus === 'testing'}
            data-testid={isOpenClaw ? 'oc-test-connection' : undefined}
          >
            {testStatus === 'testing' ? 'Testing…' : 'Test connection'}
          </button>
        {/if}
        <span class="pcc-spacer"></span>
        {#if cardStatus !== 'unconfigured'}
          <button class="pcc-btn-danger" onclick={handleDisconnect} disabled={disconnecting || !info.connected}>
            {disconnecting ? 'Disconnecting…' : 'Disconnect'}
          </button>
        {/if}
        {#if !isOpenClaw}
          <button class="btn btn-sm btn-primary" onclick={saveConnection} disabled={!canSave || saving}>
            {saving ? 'Saving…' : 'Save'}
          </button>
        {/if}
      </div>
    {/if}
  </div>

{#if showDisconnectConfirm}
  <!-- PR2 (#2647/#2648): replaces the native confirm() for OpenClaw disconnect —
       disconnect is a soft-deactivate (unpair), not a delete: agent rows go inactive,
       but chat_sessions.agent_id and workspace directories/files stay untouched (D21,
       #2649 — sessions bound to the now-inactive agent render read-only instead).
       Copy names the concrete consequence (N agents unavailable) instead of the old
       generic "will stop answering" text. -->
  <ConfirmDialog
    title="Disconnect OpenClaw"
    message={`Disconnecting OpenClaw. Your ${info.agents.length} agents will be unavailable until you pair again. Their files stay on this machine.`}
    confirmLabel="Disconnect"
    confirmColor="#EF4444"
    onConfirm={confirmOpenClawDisconnect}
    onCancel={cancelOpenClawDisconnect}
  />
{/if}

{#if showApiDisconnectConfirm}
  <!-- #2892: replaces the native confirm() that never renders in Tauri's
       WebView (silently resolves false per Apple's documented WKUIDelegate
       default). Non-OC sibling to showDisconnectConfirm above — same
       performDisconnect() shared action body, separate flag since the OC
       dialog's copy is OC-specific. -->
  <ConfirmDialog
    title="Disconnect {RUNTIME_LABELS[info.runtime_type] ?? info.runtime_type}"
    message={`Disconnect ${RUNTIME_LABELS[info.runtime_type] ?? info.runtime_type}? This will deactivate all agents on this provider and delete the saved key. You can reconnect later through New Provider.`}
    confirmLabel="Disconnect"
    confirmColor="#EF4444"
    onConfirm={confirmApiDisconnect}
    onCancel={cancelApiDisconnect}
  />
{/if}

<!-- #2642/#2630 Commit 3: the D20 re-pair-collision ConfirmDialog and the
     post-import ImportedAgentsModal both moved to NewProviderModal.svelte,
     which now owns the entire pairing ceremony end-to-end. This card retains
     only its own Disconnect ConfirmDialog above (a genuinely card-owned
     "manage an existing pairing" action, not ceremony state). -->

<style>
  .pcc-card {
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 10px;
    background: rgba(255, 255, 255, 0.02);
    overflow: hidden;
  }

  .pcc-card--attn {
    border-color: rgba(239, 68, 68, 0.3);
  }

  .pcc-top {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
    padding: 14px 16px;
    background: none;
    border: none;
    cursor: pointer;
    text-align: left;
    color: inherit;
    font-family: inherit;
  }

  .pcc-logo {
    width: 32px;
    height: 32px;
    border-radius: 8px;
    background: rgba(255, 255, 255, 0.08);
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 11px;
    font-weight: 700;
    flex-shrink: 0;
  }

  .pcc-logo[data-runtime='openclaw'] {
    background: rgba(0, 228, 228, 0.15);
    color: #00e4e4;
  }

  .pcc-id {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .pcc-name {
    font-size: 14px;
    font-weight: 600;
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .pcc-sub {
    font-size: 12px;
    color: rgba(255, 255, 255, 0.4);
    font-family: monospace;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .pcc-actions {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-shrink: 0;
  }

  .pcc-status {
    font-size: 12px;
    color: #34c759;
  }

  .pcc-status--bad {
    color: #ef4444;
  }

  /* #2640 D13 FIX: collapsed-header error indicator for a failed post-pair
     agent import (pairSyncError set). Distinct from .pcc-status--bad so it
     doesn't get confused with the Connected/Disconnected status text itself
     — this can appear alongside "Connected" since pairing and import are
     separate steps. */
  .pcc-status-warn {
    font-size: 11px;
    font-weight: 600;
    color: #fbbf24;
    white-space: nowrap;
  }

  /* §12: unpaired is grey, never red — a state, not an error */
  .pcc-status--neutral {
    color: rgba(255, 255, 255, 0.5);
  }

  .pcc-badge--unpaired {
    background: rgba(255, 255, 255, 0.06);
    color: rgba(255, 255, 255, 0.55);
    border: 1px solid rgba(255, 255, 255, 0.12);
  }

  .pcc-caret {
    font-size: 11px;
    color: rgba(255, 255, 255, 0.4);
    transition: transform 0.15s;
  }

  .pcc-caret--open {
    transform: rotate(180deg);
  }

  .pcc-fail {
    display: flex;
    gap: 10px;
    padding: 12px 16px;
    background: rgba(239, 68, 68, 0.08);
    border-top: 1px solid rgba(239, 68, 68, 0.2);
    font-size: 13px;
    color: #fca5a5;
    line-height: 1.5;
  }

  .pcc-body {
    padding: 12px 16px;
    border-top: 1px solid rgba(255, 255, 255, 0.06);
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .pcc-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
  }

  .pcc-row-label {
    font-size: 13px;
    color: rgba(255, 255, 255, 0.8);
  }

  .pcc-row-ctrl {
    flex: 1;
    max-width: 320px;
  }

  .pcc-row-ctrl--split {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 8px;
    max-width: none;
  }

  .pcc-row-ctrl--end {
    display: flex;
    justify-content: flex-end;
    max-width: none;
  }

  .pcc-row-sublabel {
    font-size: 12px;
    color: rgba(255, 255, 255, 0.4);
    margin-top: 2px;
  }

  .pcc-readonly {
    font-family: monospace;
    font-size: 13px;
    color: #a1a1aa;
    padding: 8px 12px;
    background: rgba(255, 255, 255, 0.03);
    border: 1px solid rgba(255, 255, 255, 0.05);
    border-radius: 6px;
    text-align: right;
  }

  .pcc-readonly--inline {
    text-align: left;
    max-width: 220px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .pcc-note {
    margin: 0;
    font-size: 12px;
    color: rgba(255, 255, 255, 0.4);
    line-height: 1.5;
  }

  .pcc-ceremony-actions {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 10px;
  }

  .pcc-btn-ghost {
    background: none;
    border: 1px solid rgba(255, 255, 255, 0.12);
    color: rgba(255, 255, 255, 0.7);
    font-size: 12px;
    cursor: pointer;
    padding: 6px 10px;
    border-radius: 6px;
    font-family: inherit;
  }

  .pcc-btn-ghost:hover:not(:disabled) {
    border-color: rgba(255, 255, 255, 0.25);
    color: #fff;
  }

  .pcc-btn-ghost:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .pcc-roster {
    border-top: 1px solid rgba(255, 255, 255, 0.06);
    padding: 10px 16px;
  }

  .pcc-roster-head {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: rgba(255, 255, 255, 0.4);
    margin-bottom: 6px;
  }

  .pcc-roster-count {
    color: rgba(255, 255, 255, 0.6);
  }

  .pcc-roster-note {
    margin-left: auto;
    text-transform: none;
    font-weight: 400;
    letter-spacing: 0;
    font-size: 12px;
  }

  .pcc-agent {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 6px 0;
    font-size: 13px;
  }

  .pcc-agent--off {
    opacity: 0.45;
  }

  .pcc-agent-name {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .pcc-agent-model {
    color: rgba(255, 255, 255, 0.4);
    font-size: 12px;
  }

  .pcc-badge {
    font-size: 10px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.4px;
    padding: 2px 6px;
    border-radius: 4px;
  }

  .pcc-badge--default {
    background: rgba(20, 184, 166, 0.15);
    color: #2dd4bf;
    border: 1px solid rgba(20, 184, 166, 0.25);
  }

  .pcc-badge--local {
    background: rgba(255, 255, 255, 0.08);
    color: rgba(255, 255, 255, 0.6);
    border: 1px solid rgba(255, 255, 255, 0.12);
  }

  .pcc-error {
    margin: 0 16px 8px;
    padding: 8px 12px;
    background: rgba(239, 68, 68, 0.08);
    border: 1px solid rgba(239, 68, 68, 0.25);
    border-radius: 6px;
    font-size: 12px;
    color: #fca5a5;
  }

  /* Same treatment, but flush inside a .pcc-body that already carries its own padding
     (the sync-now error row) instead of double-indenting. */
  .pcc-error--inline {
    margin: 0;
  }

  .pcc-test-ok {
    margin: 0 16px 8px;
    font-size: 12px;
    color: #34c759;
  }

  .pcc-test-err {
    margin: 0 16px 8px;
    font-size: 12px;
    color: #ef4444;
  }

  .pcc-foot {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 12px 16px;
    border-top: 1px solid rgba(255, 255, 255, 0.06);
  }

  .pcc-spacer {
    flex: 1;
  }

  .pcc-btn-danger {
    background: none;
    border: none;
    color: #ef4444;
    font-size: 13px;
    cursor: pointer;
    padding: 6px 10px;
    font-family: inherit;
  }

  .pcc-btn-danger:hover:not(:disabled) {
    text-decoration: underline;
  }

  .pcc-btn-danger:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  /* ---------------------------------------------------------------------
     Item #2 (#2522 design polish): API Key / Base URL / pairing-ceremony
     fields (.input/.input-wide/.password-wrapper/.password-toggle/.btn
     family) were referenced in the template above but never defined in
     this component's own scoped <style> block — Svelte scopes styles per
     component, so these classes never inherited Settings.svelte's
     definitions and rendered as unstyled browser defaults (tiny, white).
     Values below are ported verbatim from Settings.svelte's .input/.btn
     rules (dark theme, #0a0a0f context) so this card matches the rest of
     the design system instead of introducing a second one.
     --------------------------------------------------------------------- */
  .input {
    background: rgba(255, 255, 255, 0.06);
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 6px;
    padding: 8px 12px;
    font-size: 13px;
    color: #e8e8ed;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', system-ui, sans-serif;
    outline: none;
    transition: border-color 0.2s;
  }

  .input:focus {
    border-color: #00e4e4;
  }

  .input::placeholder {
    color: rgba(255, 255, 255, 0.3);
  }

  .input-wide {
    width: 100%;
  }

  .password-wrapper {
    position: relative;
    display: flex;
    align-items: center;
    width: 100%;
  }

  .password-wrapper .input {
    padding-right: 60px;
    flex: 1;
    min-width: 0;
  }

  .password-toggle {
    position: absolute;
    right: 8px;
    background: none;
    border: none;
    color: rgba(255, 255, 255, 0.3);
    cursor: pointer;
    font-size: 12px;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', system-ui, sans-serif;
  }

  .password-toggle:hover:not(:disabled) {
    color: rgba(255, 255, 255, 0.5);
  }

  .password-toggle:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn {
    padding: 7px 16px;
    font-size: 13px;
    font-weight: 500;
    border-radius: 6px;
    border: 1px solid rgba(255, 255, 255, 0.08);
    background: rgba(255, 255, 255, 0.05);
    color: #e8e8ed;
    cursor: pointer;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', system-ui, sans-serif;
    transition: all 0.15s;
    white-space: nowrap;
  }

  .btn:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.08);
  }

  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn:active:not(:disabled) {
    transform: scale(0.98);
  }

  .btn-primary {
    background: #00e4e4;
    color: #0a0a0f;
    border-color: #00e4e4;
    font-weight: 600;
  }

  .btn-primary:hover:not(:disabled) {
    background: #00e4e4;
    color: #0a0a0f;
    border-color: #00e4e4;
    opacity: 0.9;
  }

  .btn-primary:active:not(:disabled) {
    background: #00e4e4;
    color: #0a0a0f;
    border-color: #00e4e4;
    opacity: 0.85;
    transform: scale(0.98);
  }

  .btn-sm {
    padding: 5px 10px;
    font-size: 12px;
  }
</style>

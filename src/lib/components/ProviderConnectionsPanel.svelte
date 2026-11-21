<!--
  ProviderConnectionsPanel.svelte — ADR-0063 D8, #2522 Stage 5b/8b

  Container for the Settings › AI "Provider connections" section. Fetches
  `ai_list_runtime_connections` and renders one ProviderConnectionCard per runtime
  type in the backend's fixed set (openclaw, openrouter, anthropic, ollama, venice —
  see commands.rs module comment) that passes the `configured_this_install` filter
  below (Blocker 3 / F1, #2455: a provider with no card is always addable via
  NewProviderModal — see the INVARIANT comment on that filter).

  D8: "Every connected provider is live at the same time." This component owns NO
  concept of an "active" provider — it is a flat list, not a radio group.

  New Provider hand-off (#2642/#2630 Commit 5, 2026-08-22): the old `driveOcCard()`
  DOM-query hack (Reed finding 6, #2522 Stage 8b) is deleted. That mechanism existed
  because NewProviderModal.svelte's OpenClaw Step 2 used to hand off pairing entirely
  to ProviderConnectionCard.svelte's own in-card ceremony, and the card was out of
  scope to modify at that stage. Both constraints are gone: the modal now owns the
  OC pairing ceremony end-to-end (NewProviderModal.svelte's module comment), and the
  card is manage-only, exposing a real `onPairRequested` callback prop instead of a
  DOM-clickable button. The Panel now wires that callback to `ocDeepLink` state, which
  it threads into the modal's `startCeremony` prop — a real prop-based deep link,
  not a DOM query. `pairSyncError` is similarly threaded: the modal's
  `onConnectionSaved?.(lastImportError?: string)` (fired when a post-pairing import
  fails non-blockingly) feeds the OC card's `pairSyncError` prop so the #2640 D13
  fix's collapsed-card "⚠️ Import failed" badge keeps working with the ceremony now
  living outside the card.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { listRuntimeConnections, type RuntimeConnectionInfo } from '$lib/api';
  import ProviderConnectionCard from './ProviderConnectionCard.svelte';
  import NewProviderModal from './NewProviderModal.svelte';

  let {
    pairingFingerprint = $bindable(''),
    onAgentsChanged,
  }: {
    /** Threaded down to the OpenClaw ProviderConnectionCard so it shares Settings.svelte's
     *  `pairingFingerprint` state var instead of the card independently re-fetching
     *  `gateway_tls_fingerprint`. See ProviderConnectionCard.svelte module comment. */
    pairingFingerprint?: string;
    /** D24 (#2649): fired from `handleConnectionSaved` after a successful OC pairing
     *  import so Settings.svelte can re-fetch its agents list. Without this, the
     *  Agents tab's `agentsLoaded` gate (Settings.svelte's `loadAgents()` $effect)
     *  stays `true` from its first load and the newly-imported agents don't appear
     *  until the user navigates away from and back to the Agents tab.
     *  D25 (#2649): also fired from `handleCardChange` — every card's `onChange`
     *  prop — so card-level mutations (disconnect, reconnect, credential save)
     *  trigger the same refresh, not just the modal's post-import path. */
    onAgentsChanged?: () => void;
  } = $props();

  let connections = $state<RuntimeConnectionInfo[]>([]);
  let loading = $state(true);
  let loadError = $state('');
  let showNewProvider = $state(false);
  // #2642/#2630 Commit 5: deep-link entry mode for the OC pairing ceremony this
  // modal now owns. Set by the OC card's `onPairRequested` callback below (driven
  // by the card's own `hasOcAgentRows` check, Commit 1); threaded straight into
  // NewProviderModal's `startCeremony` prop. Reset to null when the modal closes
  // so the next "+ New Provider" click starts at Step 1, not mid-ceremony.
  let ocDeepLink = $state<'pair' | 're-pair' | null>(null);
  // #2642/#2630 Commit 5, Matt's ruling (c): fed by the modal's
  // onConnectionSaved(lastImportError) at the point a post-pairing import fails
  // non-blockingly — threaded into the OC card's pairSyncError prop so the #2640
  // D13 fix's collapsed-card badge survives the ceremony moving out of the card.
  let pairSyncError = $state('');

  const ocConnectionInfo = $derived(connections.find((c) => c.runtime_type === 'openclaw') ?? null);

  async function loadConnections() {
    loading = true;
    loadError = '';
    try {
      connections = await listRuntimeConnections();
    } catch (e: any) {
      loadError = e?.toString?.() ?? String(e);
      console.error('[ProviderConnectionsPanel] Failed to load runtime connections:', e);
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    loadConnections();
  });

  /** #2642/#2630 Commit 5: called by the modal (`onConnectionSaved` prop) after a
   *  successful Add connection / Disconnect / OC ceremony resolution. `lastImportError`
   *  is set only on a non-blocking phase-2 (import) failure — cleared (`''`) on every
   *  other call site, matching the card's own previous per-open-instance reset
   *  behavior for `pairSyncError`. Always re-fetches so the OC card's `hasOcAgentRows`
   *  check and every card's roster/status pick up the change immediately. */
  async function handleConnectionSaved(lastImportError?: string) {
    pairSyncError = lastImportError ?? '';
    await loadConnections();
    // D24 (#2649): loadConnections() above only refreshes this panel's provider
    // cards — it has no reach into Settings.svelte's separate `agentsList`/
    // `agentsLoaded` state, so newly-imported agents wouldn't show on the Agents
    // tab without this notification.
    onAgentsChanged?.();
  }

  /** D25 (#2649): every ProviderConnectionCard's `onChange` prop — fired on any
   *  card-level mutation (disconnect, reconnect, credential save), not just the
   *  modal's post-import `handleConnectionSaved` path above. D24 fixed the import
   *  path; disconnect goes through this separate `onChange` wiring and was still
   *  missing the `onAgentsChanged` call, so the Settings default-agent block and
   *  Agents tab kept showing stale data after an OC disconnect promoted a new
   *  default agent backend-side. Mirrors `handleConnectionSaved`'s ordering:
   *  refresh this panel's cards first, then notify Settings.svelte. */
  async function handleCardChange() {
    await loadConnections();
    onAgentsChanged?.();
  }

  /** #2642/#2630 Commit 5: OC card's Pair/Re-pair button callback — opens the modal
   *  deep-linked straight into the ceremony's URL step instead of the old
   *  DOM-query-driven in-card ceremony. */
  function handleOcPairRequested(mode: 'pair' | 're-pair') {
    ocDeepLink = mode;
    showNewProvider = true;
  }

  function closeNewProvider() {
    showNewProvider = false;
    ocDeepLink = null;
  }
</script>

{#if showNewProvider}
  <NewProviderModal
    onClose={closeNewProvider}
    {ocConnectionInfo}
    {connections}
    bind:pairingFingerprint
    startCeremony={ocDeepLink}
    onConnectionSaved={handleConnectionSaved}
  />
{/if}

<div class="pcp-panel">
  <div class="pcp-head">
    <span class="pcp-title">Provider connections</span>
    <button class="pcp-new-btn" onclick={() => (showNewProvider = true)}>
      <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"
        stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <path d="M12 5v14M5 12h14"/>
      </svg>
      New Provider
    </button>
  </div>
  <p class="pcp-desc">
    Every connected provider is live at the same time. Each agent belongs to the provider it was created on and stays there.
  </p>

  {#if loading}
    <div class="pcp-loading">Loading connections…</div>
  {:else if loadError}
    <div class="pcp-error">
      Failed to load provider connections: {loadError}
      <button class="btn btn-sm" onclick={loadConnections}>Retry</button>
    </div>
  {:else}
    <div class="pcp-cards">
      <!-- Blocker 3 fix (#2777 venice live-test scoping): cards render ONLY for
           providers configured this install (`configured_this_install`), not "always
           all five, connected or not" (the prior ADR-0065 D8 / ADR-0063 D8 contract
           this correction supersedes — see docs/adr/0065-venice-runtime.md D8 and
           docs/adr/0063-runtime-per-agent.md D8 for the corrected text). "+ New
           Provider" remains the sole entry point for connecting an unconfigured
           provider — there is nothing left for an unconfigured card to show.
           INVARIANT: must match NewProviderModal.isProviderConfigured — a provider
           without a card here is always addable there (F1, v1.0.36 live-test round 2).
           If this filter's field ever changes, the Modal's predicate must change
           with it, or the two surfaces will diverge again (see NewProviderModal.svelte). -->
      {#each connections.filter((c) => c.configured_this_install) as info (info.runtime_type)}
        {#if info.runtime_type === 'openclaw'}
          <ProviderConnectionCard
            {info}
            onChange={handleCardChange}
            bind:pairingFingerprint
            onPairRequested={handleOcPairRequested}
            {pairSyncError}
          />
        {:else}
          <ProviderConnectionCard {info} onChange={handleCardChange} />
        {/if}
      {/each}
    </div>
  {/if}
</div>

<style>
  .pcp-panel {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .pcp-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .pcp-title {
    font-size: 12px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.6px;
    color: rgba(255, 255, 255, 0.5);
  }

  .pcp-desc {
    font-size: 13px;
    color: #a0a0a0;
    margin: 0 0 16px;
  }

  .pcp-new-btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-family: var(--font-sans);
    font-size: 12px;
    font-weight: 600;
    padding: 5px 10px;
    border-radius: var(--radius-md);
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg-2);
    cursor: pointer;
    transition: background 0.15s, color 0.15s, border-color 0.15s;
  }

  .pcp-new-btn:hover {
    background: var(--overlay-hover);
    color: var(--fg-1);
    border-color: var(--border-strong);
  }

  .pcp-cards {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .pcp-loading {
    font-size: 13px;
    color: rgba(255, 255, 255, 0.4);
    padding: 16px 0;
  }

  .pcp-error {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px 16px;
    background: rgba(239, 68, 68, 0.08);
    border: 1px solid rgba(239, 68, 68, 0.25);
    border-radius: 8px;
    font-size: 13px;
    color: #fca5a5;
  }
</style>

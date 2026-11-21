<!--
  AgentPickerModal.svelte — D3 provider-switch picker modal (#2030 PR B)
  Spec: §24.10, ADR-0059 D3
  Pattern: Scrim + centered modal card

  Shown when: user switches from OpenClaw → API provider AND ≥1 local agent exists (ADR-0059 D3).
  Contents:
    - Local agents (runtime_ref IS NULL) sorted created_at DESC (newest pre-selected)
    - "Create new agent" row as last item (opens AgentCreateOverlay)
    - Cancel: reverts — no DB changes, provider dropdown reverts

  CSS tokens: all from app.css; no hardcoded colors.
  §12 must-fix #6: "Create new" radio slot uses .ag-pick__radio--none modifier, NOT style={{ border: 0 }}
-->
<script lang="ts">
  import type { Agent } from '$lib/types';
  import { setMainAgent } from '$lib/api';

  // ── Props ─────────────────────────────────────────────────────────────────
  let {
    agents,         // local agents only (runtime_ref IS NULL), caller pre-filters + sorts by created_at DESC
    onConfirm,      // (agent: Agent) => void — user picked an agent
    onCreateNew,    // () => void — user chose "Create new agent"
    onCancel,       // () => void — user cancelled; caller reverts provider dropdown
  }: {
    agents: Agent[];
    onConfirm: (agent: Agent) => void;
    onCreateNew: () => void;
    onCancel: () => void;
  } = $props();

  // ── State ─────────────────────────────────────────────────────────────────
  // Newest pre-selected (agents sorted created_at DESC by caller → first item = newest)
  let selectedId = $state(agents[0]?.id ?? '');
  let confirming = $state(false);
  let errorMessage = $state('');

  // ── Actions ───────────────────────────────────────────────────────────────
  async function handleConfirm() {
    if (!selectedId) return;
    confirming = true;
    errorMessage = '';
    try {
      const agent = await setMainAgent(selectedId);
      window.dispatchEvent(new CustomEvent('pane-agent-switched'));
      onConfirm(agent);
    } catch (e: any) {
      errorMessage = e?.message || String(e);
    } finally {
      confirming = false;
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') onCancel();
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<!-- Scrim: rgba(0,0,0,0.6) — no hardcoded off-color per §12 -->
<div class="ag-scrim" onclick={onCancel} role="presentation">
  <div
    class="ag-modal"
    role="dialog"
    aria-modal="true"
    aria-label="Choose an agent"
    onclick={(e) => e.stopPropagation()}
  >
    <div class="ag-modal__header">
      <h2 class="ag-modal__title">Switch to API provider</h2>
      <p class="ag-modal__subtitle">
        Which agent should be active on this provider?
      </p>
    </div>

    <div class="ag-pick-list">
      {#each agents as agent, i}
        <label class="ag-pick" class:ag-pick--selected={agent.id === selectedId}>
          <input
            type="radio"
            class="ag-pick__radio-input"
            name="agent-picker"
            value={agent.id}
            bind:group={selectedId}
          />
          <span class="ag-pick__radio"></span>
          <div class="ag-pick__body">
            <span class="ag-pick__name">{agent.name}</span>
            {#if i === 0}
              <!-- §12 WARN fix: use --accent or --fg-3, NOT --fg-4 (placeholder/disabled color) -->
              <span class="ag-pick__tag">Newest</span>
            {/if}
          </div>
          <span class="ag-pick__meta">{agent.slug}</span>
        </label>
      {/each}

      <!-- "Create new agent" row — always last -->
      <!-- §12 must-fix #6: radio slot uses .ag-pick__radio--none modifier, NOT style={{ border: 0 }} -->
      <button class="ag-pick ag-pick--create" onclick={onCreateNew}>
        <span class="ag-pick__radio ag-pick__radio--none"></span>
        <div class="ag-pick__body">
          <span class="ag-pick__name">Create new agent</span>
        </div>
        <!-- Chevron icon (Lucide style) -->
        <svg class="ag-pick__chevron" width="16" height="16" viewBox="0 0 24 24" fill="none"
          stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M9 18l6-6-6-6"/>
        </svg>
      </button>
    </div>

    {#if errorMessage}
      <div class="ag-modal__error">{errorMessage}</div>
    {/if}

    <div class="ag-modal__revert">
      Cancel keeps you on OpenClaw — nothing changes.
    </div>

    <div class="ag-modal__actions">
      <button class="ag-btn ag-btn--ghost" onclick={onCancel} disabled={confirming}>Cancel</button>
      <button
        class="ag-btn ag-btn--primary"
        onclick={handleConfirm}
        disabled={!selectedId || confirming}
      >
        {confirming ? 'Switching…' : 'Use this agent →'}
      </button>
    </div>
  </div>
</div>

<style>
  /* ── Scrim ── */
  /* §12: rgba(0,0,0,0.6) — correct scrim color (not rgba(5,5,7,0.6)) */
  .ag-scrim {
    position: fixed;
    inset: 0;
    z-index: 9998;
    background: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: center;
    justify-content: center;
  }

  /* ── Modal card ── */
  /* §12: uses --shadow-pop token; non-standard shadow avoided */
  .ag-modal {
    background: var(--surface-1);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);     /* 12px — nearest token to design's 14px */
    box-shadow: var(--shadow-pop);
    width: 100%;
    max-width: 440px;
    padding: 24px;
  }

  .ag-modal__title {
    font-size: 18px;
    font-weight: 700;
    color: var(--fg-1);
    margin: 0 0 4px;
  }

  .ag-modal__subtitle {
    font-size: 14px;
    color: var(--fg-2);
    margin: 0 0 20px;
  }

  /* ── Agent pick list ── */
  .ag-pick-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-bottom: 16px;
  }

  .ag-pick {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px 14px;
    border-radius: var(--radius-md);
    border: 1px solid var(--border);
    background: transparent;
    cursor: pointer;
    transition: border-color 0.15s, background 0.15s;
    text-align: left;
    width: 100%;
    color: var(--fg-1);
    font-size: 14px;
  }

  label.ag-pick {
    cursor: pointer;
  }

  .ag-pick:hover {
    border-color: var(--border-strong);
    background: var(--bg-hover);
  }

  .ag-pick--selected {
    border-color: var(--accent);
    /* §12: use --accent-soft token (0.12–0.18 range), not ad-hoc 0.06 */
    background: var(--accent-soft);
  }

  .ag-pick__radio-input {
    display: none;
  }

  /* Radio indicator */
  .ag-pick__radio {
    width: 16px;
    height: 16px;
    border-radius: var(--radius-pill);
    border: 2px solid var(--border-strong);
    flex-shrink: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    transition: border-color 0.15s;
  }

  .ag-pick--selected .ag-pick__radio {
    border-color: var(--accent);
    background: var(--accent);
  }

  /* §12 must-fix #6: modifier class instead of style={{ border: 0 }} */
  .ag-pick__radio--none {
    border: none;
    background: transparent;
    width: 16px;
    height: 16px;
  }

  .ag-pick__body {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .ag-pick__name {
    font-weight: 500;
    color: var(--fg-1);
  }

  /* §12 WARN fix: "Newest" tag uses --fg-3 or --accent, NOT --fg-4 (placeholder/disabled) */
  .ag-pick__tag {
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--accent);
    padding: 2px 6px;
    border-radius: var(--radius-pill);
    background: var(--accent-soft);
  }

  .ag-pick__meta {
    font-size: 12px;
    color: var(--fg-3);
    font-family: monospace;
  }

  /* ── Create new row ── */
  .ag-pick--create {
    border-style: dashed;
    border-color: var(--border-strong);
    color: var(--fg-2);
  }

  .ag-pick--create:hover {
    border-color: var(--accent);
    color: var(--fg-1);
  }

  .ag-pick__chevron {
    color: var(--fg-3);
    flex-shrink: 0;
  }

  /* ── Modal footer ── */
  .ag-modal__error {
    padding: 10px 14px;
    background: var(--status-error-soft, rgba(239, 68, 68, 0.08));
    border: 1px solid var(--status-error-border, rgba(239, 68, 68, 0.25));
    border-radius: var(--radius-md);
    font-size: 13px;
    color: var(--fg-2);
    margin-bottom: 12px;
  }

  .ag-modal__revert {
    font-size: 12px;
    color: var(--fg-3);
    margin-bottom: 16px;
  }

  .ag-modal__actions {
    display: flex;
    justify-content: flex-end;
    gap: 10px;
  }

  /* ── ag-modal__header wrapper ── */
  .ag-modal__header {
    margin-bottom: 4px;
  }

  /* ── Button system: self-contained so overlay isn't relying on Settings.svelte scoped .btn ── */
  .ag-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    font-family: var(--font-sans);
    font-size: 13.5px;
    font-weight: 500;
    padding: 10px 17px;
    border-radius: var(--radius-md);
    border: 1px solid transparent;
    cursor: pointer;
    transition: background 0.15s, color 0.15s, border-color 0.15s;
    white-space: nowrap;
  }

  .ag-btn:disabled {
    cursor: not-allowed;
  }

  .ag-btn--primary {
    background: var(--accent);
    color: var(--fg-on-accent);
    font-weight: 600;
  }

  .ag-btn--primary:disabled {
    background: var(--surface-2);
    color: var(--fg-4);
    border-color: var(--border-faint);
  }

  .ag-btn--primary:hover:not(:disabled) {
    background: var(--accent-hover);
  }

  .ag-btn--ghost {
    background: transparent;
    color: var(--fg-2);
    border-color: var(--border);
  }

  .ag-btn--ghost:hover:not(:disabled) {
    background: var(--overlay-hover);
    color: var(--fg-1);
    border-color: var(--border-strong);
  }
</style>

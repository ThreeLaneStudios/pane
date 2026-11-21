<!--
  DefaultAgentPickerModal.svelte — ADR-0063 D8, #2522 Stage 5
  Design: docs/design/agent-sessions-sprint/pc-b.jsx `PcDefaultPicker`

  Cross-provider "Default agent" picker for Settings › AI. Replaces the per-agent
  "Set as Main" buttons that used to live inline in the Agents-tab roster (removed as
  part of the dark-predicate cleanup, #2522 Task 4) — this modal is now the ONE place
  Settings offers a cross-provider default-agent comparison. (Agent detail pages may
  separately offer "Set as default" for the single agent already in view — that is a
  property of the agent, not a comparison across providers, and is out of this
  component's scope.)

  Agents are grouped by runtime_type (same grouping the New Session picker uses).
  Agents on a disconnected provider are listed, dimmed, and not selectable — hiding
  them would make the roster look wrong (design rationale, pc-b.jsx).
-->
<script lang="ts">
  import type { Agent } from '$lib/types';
  import { setMainAgent } from '$lib/api';

  let {
    agents,             // all active agents (any runtime_type), caller does not pre-filter
    connectedRuntimes,  // Set<string> of runtime_types currently connected (from ai_list_runtime_connections)
    onConfirm,          // (agent: Agent) => void — new default was set
    onCancel,           // () => void
  }: {
    agents: Agent[];
    connectedRuntimes: Set<string>;
    onConfirm: (agent: Agent) => void;
    onCancel: () => void;
  } = $props();

  const RUNTIME_LABELS: Record<string, string> = {
    openclaw: 'OpenClaw',
    openrouter: 'OpenRouter',
    anthropic: 'Anthropic',
    venice: 'Venice',
    ollama: 'Ollama',
  };

  // Group agents by runtime_type, preserving a stable provider order; agents with a
  // null runtime_type (legacy-unpinned, D4.3) are excluded — they cannot be resolved
  // to a runtime yet, so they can't be a meaningful default. Agents with role='sub'
  // are excluded too (#2567): the old per-agent "Set as Main" buttons carried a
  // `role !== 'sub'` filter (#2271) that this modal dropped when it replaced those
  // buttons in Stage 5. Subagents are not eligible defaults, so they don't render as
  // options at all — not disabled rows, absent.
  //
  // #B5 (Reed's PR #2577 audit): role='sub' alone is too broad — locally-created
  // agents can share role='sub' but have runtime_ref = null (they were never
  // imported from OC, so they aren't real subagents). Only exclude role='sub'
  // agents that ALSO have a runtime_ref set.
  // F4 fix (#2455 venice live-test v2): 'venice' added — Venice agents were
  // otherwise invisible to this roster (filtered out entirely, not just
  // unselectable) even though they are `is_active = 1` in the DB, because this
  // list controls which runtime groups render at all. Placed after 'anthropic',
  // before 'ollama' — matches the New Session picker's provider ordering.
  const groupOrder = ['openclaw', 'openrouter', 'anthropic', 'venice', 'ollama'];
  const eligibleAgents = $derived(agents.filter((a) => a.role !== 'sub' || a.runtime_ref == null));
  const groups = $derived(
    groupOrder
      .map((rt) => ({
        runtimeType: rt,
        label: RUNTIME_LABELS[rt] ?? rt,
        agents: eligibleAgents.filter((a) => a.runtime_type === rt),
      }))
      .filter((g) => g.agents.length > 0)
  );

  const currentDefaultId = $derived(eligibleAgents.find((a) => a.is_default)?.id ?? '');
  let selectedId = $state(currentDefaultId);
  let confirming = $state(false);
  let errorMessage = $state('');

  function isSelectable(agent: Agent): boolean {
    return !!agent.runtime_type && connectedRuntimes.has(agent.runtime_type);
  }

  function selectAgent(agent: Agent) {
    if (!isSelectable(agent)) return;
    selectedId = agent.id;
  }

  async function handleConfirm() {
    if (!selectedId || selectedId === currentDefaultId) {
      onCancel();
      return;
    }
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

  function initials(name: string): string {
    return name
      .split(/\s+/)
      .filter(Boolean)
      .slice(0, 2)
      .map((p) => p[0]?.toUpperCase() ?? '')
      .join('') || '?';
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="dap-scrim" onclick={onCancel} role="presentation">
  <div
    class="dap-modal"
    role="dialog"
    aria-modal="true"
    aria-label="Default agent"
    onclick={(e) => e.stopPropagation()}
  >
    <div class="dap-modal__head">
      <div class="dap-modal__eyebrow">Settings</div>
      <h3 class="dap-modal__title">Default agent</h3>
      <p class="dap-modal__sub">Preselected for new sessions, and used for sessions Pane creates on its own.</p>
    </div>

    <div class="dap-modal__list">
      {#each groups as group (group.runtimeType)}
        <div class="dap-provlabel">{group.label}</div>
        {#each group.agents as agent (agent.id)}
          {@const selectable = isSelectable(agent)}
          <button
            class="dap-pick"
            class:dap-pick--sel={agent.id === selectedId}
            class:dap-pick--off={!selectable}
            disabled={!selectable}
            onclick={() => selectAgent(agent)}
          >
            <span class="dap-pick__radio"></span>
            <span class="dap-pick__av">{initials(agent.name)}</span>
            <span class="dap-pick__body">
              <span class="dap-pick__name">
                {agent.name}
                {#if agent.id === currentDefaultId}
                  <span class="dap-badge">Current</span>
                {/if}
              </span>
              <span class="dap-pick__meta">{agent.model || '—'}</span>
            </span>
            {#if !selectable}
              <span class="dap-reason">Provider disconnected</span>
            {/if}
          </button>
        {/each}
      {/each}
    </div>

    {#if errorMessage}
      <div class="dap-modal__error">{errorMessage}</div>
    {/if}

    <div class="dap-modal__foot">
      <button class="dap-btn dap-btn--ghost" onclick={onCancel} disabled={confirming}>Cancel</button>
      <button class="dap-btn dap-btn--primary" onclick={handleConfirm} disabled={!selectedId || confirming}>
        {confirming ? 'Setting…' : 'Set default'}
      </button>
    </div>
  </div>
</div>

<style>
  .dap-scrim {
    position: fixed;
    inset: 0;
    z-index: 1000;
    background: rgba(0, 0, 0, 0.7);
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .dap-modal {
    background: #1a1a24;
    border: 1px solid rgba(255, 255, 255, 0.12);
    border-radius: 12px;
    padding: 24px;
    max-width: 460px;
    width: 90%;
    max-height: 80vh;
    display: flex;
    flex-direction: column;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', system-ui, sans-serif;
  }

  .dap-modal__head {
    margin-bottom: 16px;
  }

  .dap-modal__eyebrow {
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.8px;
    color: rgba(255, 255, 255, 0.4);
    margin-bottom: 6px;
  }

  .dap-modal__title {
    font-size: 18px;
    font-weight: 700;
    color: #e8e8ed;
    margin: 0 0 6px;
  }

  .dap-modal__sub {
    font-size: 13px;
    color: rgba(255, 255, 255, 0.5);
    margin: 0;
    line-height: 1.5;
  }

  .dap-modal__list {
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-bottom: 16px;
  }

  .dap-provlabel {
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.8px;
    color: rgba(255, 255, 255, 0.35);
    margin: 10px 0 4px;
  }

  .dap-provlabel:first-child {
    margin-top: 0;
  }

  .dap-pick {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 12px;
    border-radius: 8px;
    border: 1px solid rgba(255, 255, 255, 0.08);
    background: rgba(255, 255, 255, 0.03);
    cursor: pointer;
    text-align: left;
    width: 100%;
    color: #e8e8ed;
    font-family: inherit;
    transition: border-color 0.15s, background 0.15s;
  }

  .dap-pick:hover:not(:disabled) {
    border-color: rgba(255, 255, 255, 0.2);
    background: rgba(255, 255, 255, 0.06);
  }

  .dap-pick--sel {
    border-color: #00e4e4;
    background: rgba(0, 228, 228, 0.08);
  }

  .dap-pick--off {
    opacity: 0.45;
    cursor: not-allowed;
  }

  .dap-pick__radio {
    width: 16px;
    height: 16px;
    border-radius: 50%;
    border: 2px solid rgba(255, 255, 255, 0.25);
    flex-shrink: 0;
  }

  .dap-pick--sel .dap-pick__radio {
    border-color: #00e4e4;
    background: #00e4e4;
  }

  .dap-pick__av {
    width: 28px;
    height: 28px;
    border-radius: 50%;
    background: rgba(255, 255, 255, 0.12);
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 11px;
    font-weight: 600;
    flex-shrink: 0;
  }

  .dap-pick__body {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .dap-pick__name {
    font-size: 14px;
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .dap-pick__meta {
    font-size: 12px;
    color: rgba(255, 255, 255, 0.4);
  }

  .dap-badge {
    font-size: 10px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.4px;
    padding: 2px 6px;
    border-radius: 4px;
    background: rgba(0, 228, 228, 0.15);
    color: #00e4e4;
    border: 1px solid rgba(0, 228, 228, 0.2);
  }

  .dap-reason {
    font-size: 11px;
    color: rgba(255, 255, 255, 0.35);
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
  }

  .dap-modal__error {
    padding: 10px 14px;
    background: rgba(239, 68, 68, 0.08);
    border: 1px solid rgba(239, 68, 68, 0.25);
    border-radius: 8px;
    font-size: 13px;
    color: #e8e8ed;
    margin-bottom: 12px;
  }

  .dap-modal__foot {
    display: flex;
    justify-content: flex-end;
    gap: 10px;
  }

  .dap-btn {
    padding: 7px 16px;
    font-size: 13px;
    font-weight: 500;
    border-radius: 6px;
    border: 1px solid rgba(255, 255, 255, 0.08);
    background: rgba(255, 255, 255, 0.05);
    color: #e8e8ed;
    cursor: pointer;
    font-family: inherit;
  }

  .dap-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .dap-btn--ghost:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.08);
  }

  .dap-btn--primary {
    background: #00e4e4;
    color: #0a0a0f;
    border-color: #00e4e4;
    font-weight: 600;
  }

  .dap-btn--primary:hover:not(:disabled) {
    opacity: 0.9;
  }
</style>

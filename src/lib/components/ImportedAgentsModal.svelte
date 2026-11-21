<!--
  ImportedAgentsModal.svelte — #2635 (D11 fix)

  Shown after a successful OpenClaw pairing via ProviderConnectionCard.svelte's
  "New Provider" / Settings pairing ceremony (`completePairing()`), once
  `sync_initial_import` has returned the list of agents pulled from the gateway.
  This is the Settings-context equivalent of Onboarding.svelte's full-screen
  "Your Agents" review step (`screen === 'agent-names'`, Onboarding.svelte
  ~line 2376) — same underlying event (agents just got imported from the
  gateway), different context: Settings is a panel, not a wizard, so this is a
  lightweight dismissable popup rather than a full-screen step. Unlike
  Onboarding's screen, this modal does NOT support renaming agents — that
  capability lives in the Agents tab (per the roster's own "Manage on the
  Agents tab" note in ProviderConnectionCard.svelte), so duplicating it here
  would just be two places to rename the same agent.

  Styled after DefaultAgentPickerModal.svelte's scrim + dark modal shell
  (same literal color values) rather than NewProviderModal.svelte's CSS-custom-
  property theme, since DefaultAgentPickerModal is the closer precedent for a
  small Settings-triggered informational modal.
-->
<script lang="ts">
  let {
    agentNames,
    agentsMissingModel = [],
    onDismiss,
  }: {
    /** Display names returned by `sync_initial_import`/`reimport_oc_agents`, in gateway order. */
    agentNames: string[];
    /** #2647 D17 fix: subset of `agentNames` that landed with `model = NULL` because
     *  `fetch_oc_agent_model` soft-failed during import. A log-only soft-fail is not
     *  acceptable — this must be a visible, actionable warning naming the affected
     *  agent(s), not silence (the exact D17 symptom this surfacing closes). */
    agentsMissingModel?: string[];
    /** () => void — called on Done, scrim click, or Escape. */
    onDismiss: () => void;
  } = $props();

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') onDismiss();
  }

  function initials(name: string): string {
    return (
      name
        .split(/\s+/)
        .filter(Boolean)
        .slice(0, 2)
        .map((p) => p[0]?.toUpperCase() ?? '')
        .join('') || '?'
    );
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="iam-scrim" onclick={onDismiss} role="presentation">
  <div
    class="iam-modal"
    role="dialog"
    aria-modal="true"
    aria-label="Agents imported"
    onclick={(e) => e.stopPropagation()}
  >
    <div class="iam-modal__head">
      <div class="iam-modal__eyebrow">OpenClaw</div>
      <h3 class="iam-modal__title">
        {agentNames.length === 1 ? 'Agent imported' : `${agentNames.length} agents imported`}
      </h3>
      <p class="iam-modal__sub">
        Pane pulled these agents from your gateway. You can rename or manage them anytime from the Agents tab.
      </p>
    </div>

    <div class="iam-modal__list">
      {#each agentNames as name (name)}
        <div class="iam-row">
          <span class="iam-row__av">{initials(name)}</span>
          <span class="iam-row__name">{name}</span>
        </div>
      {/each}
    </div>

    {#if agentsMissingModel.length > 0}
      <!-- #2647 D17 fix: soft-fail surface. fetch_oc_agent_model couldn't resolve a
           model for these agent(s) during import (gateway briefly unreachable for
           /v1/agents specifically) — the import still succeeded, but the model is
           NULL until the user sets one. Naming the agent(s) here is the whole point:
           a log-only soft-fail is exactly the D17 symptom this PR fixes. -->
      <div class="iam-modal__warn" data-testid="imported-agents-missing-model-warning">
        {agentsMissingModel.length === 1
          ? `Agent "${agentsMissingModel[0]}" imported without a model`
          : `${agentsMissingModel.length} agents imported without a model`} — set one in Settings → AI.
      </div>
    {/if}

    <div class="iam-modal__foot">
      <button class="iam-btn iam-btn--primary" onclick={onDismiss} data-testid="imported-agents-done">Done</button>
    </div>
  </div>
</div>

<style>
  .iam-scrim {
    position: fixed;
    inset: 0;
    z-index: 1000;
    background: rgba(0, 0, 0, 0.7);
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .iam-modal {
    background: #1a1a24;
    border: 1px solid rgba(255, 255, 255, 0.12);
    border-radius: 12px;
    padding: 24px;
    max-width: 420px;
    width: 90%;
    max-height: 80vh;
    display: flex;
    flex-direction: column;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', system-ui, sans-serif;
  }

  .iam-modal__head {
    margin-bottom: 16px;
  }

  .iam-modal__eyebrow {
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.8px;
    color: rgba(255, 255, 255, 0.4);
    margin-bottom: 6px;
  }

  .iam-modal__title {
    font-size: 18px;
    font-weight: 700;
    color: #e8e8ed;
    margin: 0 0 6px;
  }

  .iam-modal__sub {
    font-size: 13px;
    color: rgba(255, 255, 255, 0.5);
    margin: 0;
    line-height: 1.5;
  }

  .iam-modal__list {
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-bottom: 16px;
  }

  .iam-row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 12px;
    border-radius: 8px;
    border: 1px solid rgba(255, 255, 255, 0.08);
    background: rgba(255, 255, 255, 0.03);
  }

  .iam-row__av {
    width: 28px;
    height: 28px;
    border-radius: 50%;
    background: rgba(255, 255, 255, 0.12);
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 11px;
    font-weight: 600;
    color: #e8e8ed;
    flex-shrink: 0;
  }

  .iam-row__name {
    font-size: 14px;
    color: #e8e8ed;
  }

  .iam-modal__warn {
    font-size: 12px;
    line-height: 1.5;
    color: #f5c542;
    background: rgba(245, 197, 66, 0.1);
    border: 1px solid rgba(245, 197, 66, 0.3);
    border-radius: 8px;
    padding: 10px 12px;
    margin-bottom: 16px;
  }

  .iam-modal__foot {
    display: flex;
    justify-content: flex-end;
    gap: 10px;
  }

  .iam-btn {
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

  .iam-btn--primary {
    background: #00e4e4;
    color: #0a0a0f;
    border-color: #00e4e4;
    font-weight: 600;
  }

  .iam-btn--primary:hover {
    opacity: 0.9;
  }
</style>

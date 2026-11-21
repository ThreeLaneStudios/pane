<script lang="ts">
  /**
   * ReadOnlySessionNotice.svelte — ADR-0062 D5's read-only session notice.
   *
   * Shown as the last item in the message column, directly above the
   * composer, when the active session's bound agent is unavailable: either
   * deactivated, or outside the currently active provider class. Quiet by
   * design (design pc-a.jsx PcReadOnly/PcNoticeAnatomy): standard surface
   * card, no accent rail, no red — the condition is temporary and clears on
   * its own once the provider reconnects or the agent reactivates. Never
   * re-binds the session; "Start new session" always creates a fresh one
   * bound to the current designated default.
   */

  import type { Agent } from '$lib/types';

  interface Props {
    reason: 'agent' | 'provider';
    agent: Agent;
    provider: string;
    onStartNewSession: () => void;
    onOpenCause: () => void;
  }

  let { reason, agent, provider, onStartNewSession, onOpenCause }: Props = $props();

  // F4 fix (#2455 venice live-test v2 grep sweep): venice was missing from this
  // map, so a Venice-bound session going read-only (ADR-0062 D5) would show the
  // raw string "venice" instead of "Venice" in this notice's provider label.
  const PROVIDER_LABELS: Record<string, string> = {
    openclaw: 'OpenClaw',
    openrouter: 'OpenRouter',
    anthropic: 'Anthropic',
    ollama: 'Ollama',
    venice: 'Venice',
  };
  let providerLabel = $derived(PROVIDER_LABELS[provider] ?? provider);
  let causeLabel = $derived(reason === 'provider' ? `Open ${providerLabel} connection` : `Open ${agent.name}`);
</script>

<div class="ron-wrap">
  <div class="ron-card">
    <span class="ron-icon" aria-hidden="true">
      {#if reason === 'provider'}
        <svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M6 2.5v4" /><path d="M10 2.5v4" /><path d="M4 6.5h8v2a4 4 0 01-8 0v-2z" /><path d="M8 12.5v2" /></svg>
      {:else}
        <svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><circle cx="8" cy="8" r="5.4" /><path d="M4.4 4.4l7.2 7.2" /></svg>
      {/if}
    </span>
    <div class="ron-body">
      <div class="ron-title">This session is read-only</div>
      {#if reason === 'provider'}
        <p class="ron-txt">It is bound to <b>{agent.name}</b> on <b>{providerLabel}</b>, and the {providerLabel} connection is disconnected. History stays here. The session resumes on its own once {providerLabel} reconnects.</p>
      {:else}
        <p class="ron-txt">It is bound to <b>{agent.name}</b>, and {agent.name} is deactivated. History stays here. The session resumes on its own once {agent.name} is active again.</p>
      {/if}
      <div class="ron-actions">
        <button class="ron-btn ron-btn--primary" onclick={onStartNewSession}>Start new session</button>
        <button class="ron-btn ron-btn--ghost" onclick={onOpenCause}>{causeLabel}</button>
      </div>
    </div>
  </div>
</div>

<style>
  .ron-wrap {
    padding: 0 12px 6px;
  }

  .ron-card {
    display: flex;
    gap: 10px;
    padding: 14px;
    background: var(--surface-1);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg, 12px);
  }

  .ron-icon {
    flex-shrink: 0;
    color: var(--fg-3);
    margin-top: 1px;
  }

  .ron-body {
    flex: 1;
    min-width: 0;
  }

  .ron-title {
    font-size: 13.5px;
    font-weight: 600;
    color: var(--fg-1);
    margin-bottom: 4px;
  }

  .ron-txt {
    font-size: 12.5px;
    line-height: 1.5;
    color: var(--fg-2);
    margin: 0 0 10px;
  }

  .ron-txt b {
    color: var(--fg-1);
    font-weight: 600;
  }

  .ron-actions {
    display: flex;
    gap: 8px;
  }

  .ron-btn {
    font-size: 12.5px;
    font-weight: 500;
    padding: 6px 12px;
    border-radius: var(--radius-md, 8px);
    cursor: pointer;
    border: 1px solid transparent;
  }

  .ron-btn--primary {
    background: var(--accent);
    color: var(--fg-on-accent, #0A0A0F);
    font-weight: 600;
  }

  .ron-btn--primary:hover {
    background: var(--accent-hover, var(--accent));
  }

  .ron-btn--ghost {
    background: transparent;
    color: var(--fg-2);
    border-color: var(--border);
  }

  .ron-btn--ghost:hover {
    background: var(--overlay-hover, rgba(255, 255, 255, 0.03));
    color: var(--fg-1);
  }
</style>

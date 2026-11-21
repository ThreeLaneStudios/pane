<script lang="ts">
  /**
   * AgentPickerRows.svelte — grouped-by-provider agent radio list.
   *
   * Shared row markup for the two ADR-0062 D2 explicit-picker surfaces:
   * the New Session split-control popover and the New Project dialog's
   * agent row (design: pc-a.jsx PcPickerDropdown, pc-d.jsx PcNewProject).
   * #2563 B2: callers pre-filter `agents` to connected-runtime agents only
   * (`filterConnectedAgents`, via `ai_list_runtime_connections`) before
   * passing them in, so this can render agents grouped across multiple
   * connected runtimes at once — the grouping code makes no single-class
   * assumption.
   */

  import type { Agent } from '$lib/types';
  import { agentInitials, groupAgentsByProvider } from '$lib/utils/agentDisplay';

  interface Props {
    agents: Agent[];
    selectedId: string;
    onSelect: (agentId: string) => void;
  }

  let { agents, selectedId, onSelect }: Props = $props();

  // §36.17.4 (#3050 PR 10): grouping logic extracted to `groupAgentsByProvider`
  // (agentDisplay.ts) — `NewSessionSheet.svelte`'s captain list reuses the
  // SAME function rather than a second copy. Byte-identical behavior to the
  // inline version this replaced (#2563 B2's own grouping rules unchanged).
  let groups = $derived(groupAgentsByProvider(agents));
</script>

<div class="apr-list" role="radiogroup" aria-label="Choose an agent">
  {#each groups as [label, groupAgents] (label)}
    <div class="apr-provlabel">{label}</div>
    {#each groupAgents as agent (agent.id)}
      <button
        type="button"
        class="apr-pick"
        class:apr-pick--sel={agent.id === selectedId}
        role="radio"
        aria-checked={agent.id === selectedId}
        onclick={() => onSelect(agent.id)}
      >
        <span class="apr-pick__radio" aria-hidden="true"></span>
        <span class="apr-pick__av" aria-hidden="true">{agentInitials(agent.name)}</span>
        <span class="apr-pick__body">
          <span class="apr-pick__name">
            {agent.name}
            {#if agent.is_default}<span class="apr-badge">Default</span>{/if}
          </span>
          {#if agent.model}<span class="apr-pick__meta">{agent.model}</span>{/if}
        </span>
      </button>
    {/each}
  {/each}
</div>

<style>
  .apr-list {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .apr-provlabel {
    font-size: 10.5px;
    font-weight: 600;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--fg-4);
    padding: 8px 10px 4px;
  }

  .apr-pick {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 8px 10px;
    background: transparent;
    border: none;
    border-radius: var(--radius-md, 8px);
    cursor: pointer;
    text-align: left;
    color: var(--fg-1);
    transition: background 0.12s;
  }

  .apr-pick:hover {
    background: var(--overlay-hover, rgba(255, 255, 255, 0.03));
  }

  .apr-pick--sel {
    background: var(--accent-soft);
  }

  .apr-pick__radio {
    width: 14px;
    height: 14px;
    border-radius: 50%;
    border: 2px solid var(--border-strong);
    flex-shrink: 0;
    box-sizing: border-box;
  }

  .apr-pick--sel .apr-pick__radio {
    border-color: var(--accent);
    background: var(--accent);
    box-shadow: inset 0 0 0 2.5px var(--surface-1, var(--bg));
  }

  .apr-pick__av {
    flex-shrink: 0;
    width: 26px;
    height: 26px;
    border-radius: 50%;
    background: var(--surface-2);
    color: var(--fg-2);
    font-size: 10.5px;
    font-weight: 600;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .apr-pick__body {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  .apr-pick__name {
    font-size: 13px;
    font-weight: 500;
    color: var(--fg-1);
    display: flex;
    align-items: center;
    gap: 6px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .apr-badge {
    font-size: 10px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--accent);
    background: var(--accent-soft);
    padding: 1px 6px;
    border-radius: var(--radius-pill, 999px);
    flex-shrink: 0;
  }

  .apr-pick__meta {
    font-size: 11.5px;
    font-family: var(--font-mono);
    color: var(--fg-4);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>

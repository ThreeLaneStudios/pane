<script lang="ts">
  /**
   * EditCrewPopover.svelte — the header's edit-crew popover (§36.17.4,
   * #3050 PR 10, Captain & Crew UI). Opened from the session header's
   * crew button/"+ Crew" (never from an OpenClaw captain's "own crew"
   * label, which has no click handler at all).
   *
   * Captain is locked — no toggle, no checkbox, this component has no
   * `agentId` write path at all (the backend command it calls,
   * `update_session_crew`, has no `agentId` parameter either — see
   * `api.ts`). Crew chips toggle add/remove against the FULL crew pool
   * (API agents minus the captain), capped at `max-height:102px` (same
   * pinned value the New Session sheet uses) and scrollable past that.
   *
   * "Applies to the next job <Captain> hands off. Running jobs keep their
   * agent." — the exact design copy (3b), not a paraphrase. Save crew
   * calls `onSave` with the exact toggled set (additions AND removals).
   */

  import type { Agent } from '$lib/types';
  import { agentInitials } from '$lib/utils/agentDisplay';

  interface Props {
    /** Locked — never changeable from this popover. */
    captain: Agent;
    /** The session's CURRENT crew (pre-checks these chips). */
    crew: Agent[];
    /** Every selectable candidate — API agents minus the captain. */
    crewPool: Agent[];
    top: number;
    left: number;
    onSave: (crewAgentIds: string[]) => void;
    onCancel: () => void;
  }

  let { captain, crew, crewPool, top, left, onSave, onCancel }: Props = $props();

  let selected = $state<Set<string>>(new Set(crew.map(a => a.id)));

  function toggle(id: string) {
    const next = new Set(selected);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    selected = next;
  }

  function handleSave() {
    onSave(Array.from(selected));
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.stopPropagation();
      onCancel();
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
<div class="ecp-backdrop" onclick={onCancel} aria-hidden="true" data-testid="ecp-backdrop"></div>
<div
  class="ecp-pop"
  style="top:{top}px; left:{left}px;"
  role="dialog"
  aria-label="Edit crew"
  data-testid="ecp-pop"
  onclick={(e) => e.stopPropagation()}
>
  <div class="ecp-h">
    <b>Crew</b>
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <button type="button" class="ecp-ib" onclick={onCancel} aria-label="Close">
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M18 6 6 18" /><path d="m6 6 12 12" /></svg>
    </button>
  </div>

  <div class="ecp-cap" data-testid="ecp-captain-row">
    <span class="ecp-av ecp-av--cap" aria-hidden="true">{agentInitials(captain.name)}</span>
    <div>
      <b>{captain.name}</b>
      <small>Captain · can't be changed</small>
    </div>
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <rect x="3.5" y="7.5" width="9" height="6.5" rx="1.6" /><path d="M5.6 7.5V5.6a2.4 2.4 0 014.8 0v1.9" />
    </svg>
  </div>

  <p class="ecp-help">Agents {captain.name} can hand jobs to.</p>

  <div class="ecp-ccs" data-testid="ecp-chip-list">
    {#each crewPool as agent (agent.id)}
      <button
        type="button"
        class="ecp-cc"
        class:ecp-cc--on={selected.has(agent.id)}
        aria-pressed={selected.has(agent.id)}
        onclick={() => toggle(agent.id)}
      >
        <span class="ecp-av" aria-hidden="true">{agentInitials(agent.name)}</span>
        {agent.name}
        {#if selected.has(agent.id)}
          <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M20 6 9 17l-5-5" /></svg>
        {/if}
      </button>
    {/each}
    {#if crewPool.length === 0}
      <div class="ecp-empty">No other API agents available for crew.</div>
    {/if}
  </div>

  <p class="ecp-note" data-testid="ecp-note">
    Applies to the next job {captain.name} hands off. Running jobs keep their agent.
  </p>

  <div class="ecp-f">
    <button type="button" class="ecp-btn-g" onclick={onCancel}>Cancel</button>
    <button type="button" class="ecp-btn-p" onclick={handleSave} data-testid="ecp-save-btn">Save crew</button>
  </div>
</div>

<style>
  .ecp-backdrop {
    position: fixed;
    inset: 0;
    z-index: 199;
  }

  .ecp-pop {
    position: fixed;
    z-index: 200;
    width: 360px;
    background: var(--surface-2);
    border: 1px solid var(--border-strong);
    border-radius: 12px;
    box-shadow: var(--shadow-pop);
    padding: 12px 14px 14px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .ecp-h { display: flex; align-items: center; }
  .ecp-h b { flex: 1; font-size: 13.5px; color: var(--fg-1); }
  .ecp-ib { background: transparent; border: none; color: var(--fg-3); cursor: pointer; width: 24px; height: 24px; border-radius: 6px; }
  .ecp-ib:hover { background: var(--overlay-hover, rgba(255, 255, 255, 0.06)); }

  .ecp-cap {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 10px;
    border-radius: 8px;
    background: var(--surface-3, #141418);
  }
  .ecp-cap > div { flex: 1; }
  .ecp-cap b { display: block; font-size: 13px; font-weight: 500; color: var(--fg-1); }
  .ecp-cap small { font-size: 11px; color: var(--fg-4); }
  .ecp-cap svg { color: var(--fg-4); flex-shrink: 0; }

  .ecp-av {
    width: 22px;
    height: 22px;
    border-radius: 50%;
    background: #26262b;
    color: var(--fg-2);
    font-size: 9.5px;
    font-weight: 600;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }
  .ecp-av--cap { box-shadow: 0 0 0 1.5px var(--surface-3, #141418), 0 0 0 2.5px var(--accent); color: var(--fg-1); }

  .ecp-help { margin: 0; font-size: 11.5px; color: var(--fg-4); line-height: 1.45; }

  .ecp-ccs {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    max-height: 102px;
    overflow-y: auto;
    align-content: flex-start;
    padding-right: 4px;
  }

  .ecp-cc {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    height: 30px;
    padding: 0 11px 0 4px;
    border-radius: 15px;
    border: 1px solid var(--border);
    font-size: 12.5px;
    color: var(--fg-2);
    background: transparent;
    cursor: pointer;
  }
  .ecp-cc--on { border-color: rgba(0, 228, 228, 0.6); background: var(--accent-soft); color: var(--fg-1); }
  .ecp-empty { font-size: 12px; color: var(--fg-4); padding: 6px 2px; }

  .ecp-note { margin: 0; font-size: 11.5px; color: var(--fg-4); line-height: 1.45; }

  .ecp-f { display: flex; justify-content: flex-end; gap: 8px; }
  .ecp-btn-g, .ecp-btn-p {
    height: 28px;
    padding: 0 12px;
    font-size: 12.5px;
    border-radius: 7px;
    cursor: pointer;
  }
  .ecp-btn-g { background: transparent; border: 1px solid var(--border); color: var(--fg-2); }
  .ecp-btn-p { background: var(--accent); border: none; color: #04202b; font-weight: 600; }
</style>

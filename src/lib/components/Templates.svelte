<!-- spec-covered-by: #2121 (§32, ADR-0058) -->
<script lang="ts">
  // Templates picker — command-palette modal (§32.3.1)
  // Surfaces: populated, empty, search-no-results
  // Arrow-key nav (↑/↓/Enter). Esc closes. Select applies immediately.
  import { onMount } from 'svelte';
  import * as api from '$lib/api';
  import type { Template } from '$lib/types';

  interface Props {
    onClose: () => void;
    onSelectTemplate: (templateId: string) => void;
  }

  let { onClose, onSelectTemplate }: Props = $props();

  let templates: Template[] = $state([]);
  let loading = $state(true);
  let query = $state('');
  let cursor = $state(0);
  let searchInput: HTMLInputElement;

  onMount(async () => {
    try {
      templates = await api.listTemplates();
    } catch (error) {
      console.error('[Templates] Failed to load:', error);
    } finally {
      loading = false;
    }
    // Focus search input on open
    setTimeout(() => searchInput?.focus(), 30);
  });

  let filtered = $derived(
    query.trim()
      ? templates.filter(t => t.name.toLowerCase().includes(query.trim().toLowerCase()))
      : templates
  );

  // Reset cursor when filtered list changes
  $effect(() => {
    filtered; // reactive dep
    cursor = 0;
  });

  function formatDate(iso: string): string {
    try {
      return new Date(iso).toLocaleDateString('en-US', { month: 'short', day: 'numeric', year: 'numeric' });
    } catch {
      return iso;
    }
  }

  function applySelected() {
    if (filtered.length === 0) return;
    const t = filtered[cursor];
    if (t) {
      onSelectTemplate(t.id);
      onClose();
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      onClose();
      return;
    }
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      cursor = Math.min(cursor + 1, filtered.length - 1);
      return;
    }
    if (e.key === 'ArrowUp') {
      e.preventDefault();
      cursor = Math.max(cursor - 1, 0);
      return;
    }
    if (e.key === 'Enter') {
      e.preventDefault();
      applySelected();
    }
  }

  function highlightMatch(name: string, q: string): { pre: string; match: string; post: string } | null {
    if (!q) return null;
    const i = name.toLowerCase().indexOf(q.toLowerCase());
    if (i < 0) return null;
    return { pre: name.slice(0, i), match: name.slice(i, i + q.length), post: name.slice(i + q.length) };
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="tpl-overlay" role="button" tabindex="-1" onclick={onClose}></div>

<div class="tpl-modal tp-cmd" role="dialog" aria-modal="true" aria-label="Template picker">
  <!-- Search row — pinned top -->
  <div class="tp-cmd__search" class:is-focus={!!query}>
    <!-- search icon -->
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
      <circle cx="11" cy="11" r="7"/><path d="m21 21-4.3-4.3"/>
    </svg>
    <input
      bind:this={searchInput}
      bind:value={query}
      placeholder="Search templates…"
      autocomplete="off"
      spellcheck="false"
    />
    <span class="tp-cmd__hint">esc</span>
  </div>

  {#if loading}
    <div class="tp-cmd__empty">
      <div class="tp-cmd__empty-art">
        <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
          <rect x="3" y="3" width="18" height="18" rx="2"/><path d="M3 9h18M9 21V9"/>
        </svg>
      </div>
      <p class="tp-cmd__empty-title">Loading…</p>
    </div>
  {:else if templates.length === 0}
    <!-- Empty state -->
    <div class="tp-cmd__empty">
      <div class="tp-cmd__empty-art">
        <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
          <rect x="3" y="3" width="18" height="18" rx="2"/><path d="M3 9h18M9 21V9"/>
        </svg>
      </div>
      <p class="tp-cmd__empty-title">No templates yet.</p>
      <p class="tp-cmd__empty-sub">Save any note as a template from the note menu.</p>
    </div>
  {:else if filtered.length === 0}
    <!-- No-results state -->
    <div class="tp-cmd__empty">
      <div class="tp-cmd__empty-art">
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
          <circle cx="11" cy="11" r="7"/><path d="m21 21-4.3-4.3"/>
        </svg>
      </div>
      <p class="tp-cmd__empty-title">No templates match "{query}"</p>
      <p class="tp-cmd__empty-sub">Try a different search, or clear it to see all templates.</p>
    </div>
  {:else}
    <!-- Results list -->
    <div class="tp-cmd__list">
      <div class="tp-cmd__label">{query.trim() ? 'Results' : 'All templates'}</div>
      {#each filtered as t, i}
        {@const hl = highlightMatch(t.name, query.trim())}
        <div
          class="tp-trow"
          class:is-cursor={i === cursor}
          role="button"
          tabindex="-1"
          onclick={() => { cursor = i; applySelected(); }}
          onmouseenter={() => { cursor = i; }}
        >
          <span class="tp-trow__ico">
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
              <rect x="3" y="3" width="18" height="18" rx="2"/><path d="M3 9h18M9 21V9"/>
            </svg>
          </span>
          <span class="tp-trow__body">
            <span class="tp-trow__name">
              {#if hl}
                {hl.pre}<span class="tp-hl">{hl.match}</span>{hl.post}
              {:else}
                {t.name}
              {/if}
            </span>
            <span class="tp-trow__meta">Updated {formatDate(t.updated_at)}</span>
          </span>
          <span class="tp-trow__go">
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
              <path d="M5 12h14M13 6l6 6-6 6"/>
            </svg>
          </span>
        </div>
      {/each}
    </div>
    <div class="tp-cmd__foot">
      <span><span class="k">↑</span><span class="k">↓</span>navigate</span>
      <span><span class="k">↵</span>apply to this note</span>
    </div>
  {/if}
</div>

<style>
  .tpl-overlay {
    position: fixed;
    inset: 0;
    z-index: 999;
    background: rgba(5, 5, 7, 0.6);
    backdrop-filter: blur(3px);
  }

  .tpl-modal {
    position: fixed;
    z-index: 1000;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
  }

  /* ---- palette shell ---- */
  .tp-cmd {
    width: 560px;
    max-width: calc(100vw - 48px);
    background: var(--surface-1);
    border: 1px solid var(--border);
    border-radius: 14px;
    box-shadow: 0 12px 32px rgba(0,0,0,0.6), 0 2px 6px rgba(0,0,0,0.4);
    overflow: hidden;
  }

  /* search row */
  .tp-cmd__search {
    display: flex;
    align-items: center;
    gap: 11px;
    padding: 15px 18px;
    border-bottom: 1px solid rgba(240,237,232,0.06);
  }
  .tp-cmd__search svg { color: #5A5A5E; flex-shrink: 0; }
  .tp-cmd__search.is-focus svg { color: #00E4E4; }
  .tp-cmd__search input {
    flex: 1;
    min-width: 0;
    background: transparent;
    border: 0;
    outline: none;
    color: #F0EDE8;
    font-family: var(--font-sans, system-ui);
    font-size: 15.5px;
    letter-spacing: -0.01em;
  }
  .tp-cmd__search input::placeholder { color: #5A5A5E; }
  .tp-cmd__hint {
    font-family: var(--font-mono, monospace);
    font-size: 10.5px;
    color: #5A5A5E;
    flex-shrink: 0;
  }

  /* list area */
  .tp-cmd__list { max-height: 356px; overflow-y: auto; padding: 6px; }
  .tp-cmd__label {
    padding: 9px 10px 5px;
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.07em;
    text-transform: uppercase;
    color: #5A5A5E;
  }

  /* row */
  .tp-trow {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
    padding: 11px 12px;
    border: 0;
    background: transparent;
    border-radius: 8px;
    cursor: pointer;
    text-align: left;
    transition: background 120ms cubic-bezier(0.2,0,0,1);
  }
  .tp-trow:hover { background: rgba(240,237,232,0.03); }
  .tp-trow.is-cursor { background: rgba(0,228,228,0.12); }

  .tp-trow__ico {
    width: 30px;
    height: 30px;
    border-radius: 6px;
    flex-shrink: 0;
    background: #232327;
    color: #A0A0A0;
    display: inline-flex;
    align-items: center;
    justify-content: center;
  }
  .tp-trow.is-cursor .tp-trow__ico { background: rgba(0,228,228,0.12); color: #00E4E4; }

  .tp-trow__body { flex: 1; min-width: 0; }
  .tp-trow__name {
    display: block;
    font-size: 14px;
    font-weight: 500;
    color: #F0EDE8;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .tp-trow__meta {
    display: block;
    font-size: 12px;
    color: #5A5A5E;
    margin-top: 2px;
  }
  .tp-trow__go {
    flex-shrink: 0;
    color: #5A5A5E;
    opacity: 0;
    transition: opacity 120ms;
  }
  .tp-trow:hover .tp-trow__go,
  .tp-trow.is-cursor .tp-trow__go { opacity: 1; }

  .tp-hl { color: #00E4E4; font-weight: 600; }

  /* footer */
  .tp-cmd__foot {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 9px 16px;
    border-top: 1px solid rgba(240,237,232,0.06);
    color: #5A5A5E;
    font-size: 11.5px;
  }
  .tp-cmd__foot .k {
    font-family: var(--font-mono, monospace);
    font-size: 10px;
    background: #232327;
    border: 1px solid rgba(240,237,232,0.10);
    border-radius: 4px;
    padding: 1px 5px;
    margin-right: 5px;
    color: #A0A0A0;
  }

  /* empty / no-results */
  .tp-cmd__empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
    padding: 46px 30px 42px;
  }
  .tp-cmd__empty-art {
    width: 52px;
    height: 52px;
    border-radius: 14px;
    margin-bottom: 15px;
    background: #232327;
    color: #5A5A5E;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .tp-cmd__empty-title {
    font-size: 14px;
    font-weight: 600;
    color: #C9C5BE;
    margin: 0 0 6px;
  }
  .tp-cmd__empty-sub {
    font-size: 12.5px;
    color: #5A5A5E;
    line-height: 1.55;
    margin: 0;
    max-width: 300px;
  }
</style>

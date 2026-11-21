<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import * as api from '$lib/api';
  import type { NoteMeta } from '$lib/types';
  import { highlightText } from '$lib/utils/markdown';

  interface Props {
    onClose: () => void;
    onSelect: (note: NoteMeta) => void;
  }

  let { onClose, onSelect }: Props = $props();
  let query = $state('');
  let allNotes: NoteMeta[] = $state([]);
  let filteredNotes: NoteMeta[] = $state([]);
  let selectedIndex = $state(0);
  let searchInput: HTMLInputElement;
  let debounceTimer: any = null;

  // Load all notes on mount
  $effect(() => {
    loadAllNotes();
    // Focus after a tick
    setTimeout(() => searchInput?.focus(), 50);
  });

  async function loadAllNotes() {
    try {
      const projects: any[] = await invoke('get_projects');
      const rootProjects = projects.filter((p: any) => !p.parent_id);
      const noteArrays: NoteMeta[] = [];
      
      for (const project of rootProjects) {
        try {
          const notes = await api.listNotesByProject(project.id);
          noteArrays.push(...notes);
        } catch {}
      }
      
      allNotes = noteArrays.sort((a, b) => 
        new Date(b.updated_at).getTime() - new Date(a.updated_at).getTime()
      );
      filteredNotes = allNotes.slice(0, 30);
    } catch (e) {
      console.error('Failed to load notes for search:', e);
    }
  }

  // Search with FTS backend + client fallback
  $effect(() => {
    const q = query;
    if (debounceTimer) clearTimeout(debounceTimer);
    
    if (!q.trim()) {
      filteredNotes = allNotes.slice(0, 30);
      selectedIndex = 0;
      return;
    }
    
    debounceTimer = setTimeout(async () => {
      try {
        // Try FTS search first
        const results: any[] = await invoke('search_notes', { request: { query: q } });
        if (results.length > 0) {
          // Map search results to NoteMeta-like objects
          filteredNotes = results.map(r => ({
            id: r.note_id,
            slug: r.note_id,
            title: r.title || 'Untitled',
            preview: r.content_preview || '',
            notebook: r.project_name || '',
            project_name: r.project_name || '',
            folder_name: r.folder_name,
            updated_at: '',
            created_at: '',
            word_count: 0,
            pinned: false,
            task_status: '',
            task_priority: '',
            due_date: '',
            assignee: '',
            file_path: '',
            folder_id: '',
            project_id: '',
          } as any));
          selectedIndex = 0;
          return;
        }
      } catch {
        // FTS might fail if table doesn't exist — fall through to client filter
      }
      
      // Client-side fallback
      const qLower = q.toLowerCase();
      filteredNotes = allNotes.filter(note => 
        note.title.toLowerCase().includes(qLower) ||
        (note.preview || '').toLowerCase().includes(qLower) ||
        (note.notebook || '').toLowerCase().includes(qLower)
      ).slice(0, 20);
      selectedIndex = 0;
    }, 150);
  });

  function handleKeydown(e: KeyboardEvent) {
    switch (e.key) {
      case 'Escape':
        e.preventDefault();
        onClose();
        break;
      case 'ArrowDown':
        e.preventDefault();
        selectedIndex = Math.min(selectedIndex + 1, filteredNotes.length - 1);
        break;
      case 'ArrowUp':
        e.preventDefault();
        selectedIndex = Math.max(selectedIndex - 1, 0);
        break;
      case 'Enter':
        e.preventDefault();
        if (filteredNotes[selectedIndex]) {
          onSelect(filteredNotes[selectedIndex]);
          onClose();
        }
        break;
    }
  }

  function handleSelect(note: NoteMeta) {
    onSelect(note);
    onClose();
  }

  function formatDate(dateString: string): string {
    if (!dateString) return '';
    try {
      return new Date(dateString).toLocaleDateString();
    } catch {
      return dateString;
    }
  }


</script>

<div class="quick-switcher-modal">
  <div class="modal-overlay" role="button" tabindex="-1" onclick={onClose}></div>
  <div class="modal-content">
    <div class="search-container">
      
      <input
        bind:this={searchInput}
        bind:value={query}
        onkeydown={handleKeydown}
        placeholder="Jump to note..."
        class="search-input"
        autocomplete="off"
      />
    </div>
    
    <div class="results-container">
      {#if filteredNotes.length === 0}
        <div class="empty-results">
          {query ? 'No notes found' : 'No notes available'}
        </div>
      {:else}
        <div class="results-list">
          {#each filteredNotes as note, index}
            <button
              class="result-item"
              class:selected={index === selectedIndex}
              onclick={() => handleSelect(note)}
            >
              <div class="result-row-1">
                <div class="result-title">{@html highlightText(note.title, query)}</div>
                <div class="result-meta-right">
                  {#if note.word_count}<span class="result-words">{note.word_count}w</span>{/if}
                  {#if note.updated_at}<span class="result-date">{formatDate(note.updated_at)}</span>{/if}
                </div>
              </div>
              <div class="result-row-2">
                <span class="result-notebook">{note.notebook}</span>
              </div>
              {#if note.preview}
                <div class="result-preview">{@html highlightText(note.preview, query)}</div>
              {/if}
            </button>
          {/each}
        </div>
      {/if}
    </div>
    
    <div class="footer">
      <div class="footer-hints">
        <span><kbd>↑</kbd><kbd>↓</kbd> Navigate</span>
        <span><kbd>Enter</kbd> Select</span>
        <span><kbd>Esc</kbd> Close</span>
      </div>
    </div>
  </div>
</div>

<style>
  .quick-switcher-modal {
    position: fixed; top: 0; left: 0; width: 100vw; height: 100vh;
    z-index: 10000; display: flex; align-items: flex-start; justify-content: center;
    padding-top: 18vh;
  }
  .modal-overlay {
    position: absolute; top: 0; left: 0; width: 100%; height: 100%;
    background: rgba(0,0,0,0.7);
  }
  .modal-content {
    position: relative; width: 90%; max-width: 560px;
    background: #1a1a2e; border: 1px solid rgba(255,255,255,0.1); border-radius: 4px;
    box-shadow: 0 12px 40px rgba(0,0,0,0.7); overflow: hidden;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
  }
  .search-container {
    display: flex; align-items: center; padding: 14px 16px;
    border-bottom: 1px solid rgba(255,255,255,0.06);
  }
  .search-input {
    flex: 1; border: none; outline: none; background: transparent;
    color: #e0e0e0; font-size: 15px; font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
  }
  .search-input::placeholder { color: #52525B; }
  .results-container { max-height: 380px; overflow-y: auto; }
  .empty-results {
    padding: 32px 16px; text-align: center; color: #666666; font-size: 13px;
  }
  .results-list { padding: 4px 0; }
  .result-item {
    display: block; width: 100%; padding: 14px 16px 16px; border: none;
    background: transparent; text-align: left; cursor: pointer;
    transition: all 0.15s; border-left: 2px solid transparent;
  }
  .result-item:hover { background: rgba(255,255,255,0.05); border-left-color: rgba(0,228,228,0.3); }
  .result-item.selected { background: rgba(255,255,255,0.05); border-left-color: #00E4E4; }
  .result-row-1 {
    display: flex; align-items: baseline; justify-content: space-between;
    gap: 12px; margin-bottom: 4px;
  }
  .result-title {
    font-size: 14px; font-weight: 500; color: #e0e0e0; line-height: 1.3;
    white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
  }
  .result-meta-right {
    display: flex; align-items: center; gap: 8px; flex-shrink: 0;
    font-size: 11px; color: #52525B;
  }
  .result-row-2 { margin-bottom: 8px; }
  .result-notebook { font-size: 12px; font-weight: 500; color: #666666; }
  .result-preview {
    font-size: 12px; color: #52525B; line-height: 1.5;
    overflow: hidden; display: -webkit-box;
    -webkit-line-clamp: 2; -webkit-box-orient: vertical;
  }
  .result-preview :global(mark) {
    background: rgba(0,228,228,0.15); color: #00E4E4;
    border-radius: 2px; padding: 0 2px;
  }
  .result-title :global(mark) {
    background: rgba(0,228,228,0.15); color: #00E4E4;
    border-radius: 2px; padding: 0 2px;
  }
  .footer {
    padding: 8px 16px; border-top: 1px solid rgba(255,255,255,0.06);
  }
  .footer-hints {
    display: flex; gap: 16px; font-size: 11px; color: #52525B;
  }
  .footer-hints kbd {
    background: rgba(255,255,255,0.06); color: #888888; padding: 1px 5px;
    border-radius: 3px; font-size: 10px; font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
    border: 1px solid rgba(255,255,255,0.10);
  }
</style>
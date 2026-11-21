<script lang="ts">
  // ShVersionHistoryPanel.svelte — the sheet-native version-history panel
  // (#2339 PR 15b-beta, `docs/design/sheets/sheets-surfaces.jsx:150-176`
  // `ShHistory`, `sheets.css:282-308` `.sh-vh*`/`.sh-snap*`/`.sh-src*`).
  //
  // This is a REPLACEMENT for the generic markdown `.vh-overlay`/
  // `.vh-panel` modal when the active note is a sheet — Editor.svelte
  // branches on `isSheet` and mounts THIS component instead (see that
  // file's `{#if showVersionHistory}` site). The existing markdown modal
  // is untouched, so plain-note version history keeps working exactly as
  // before (regression-tested in `Editor.restore-markdown.test.ts` and
  // this file's own sibling test asserting the two paths never cross).
  //
  // Self-contained: owns its own fetch (`get_note_versions`/
  // `get_note_version` — reused verbatim from the generic note-version
  // commands, since a sheet snapshot IS an ordinary `note_versions` row
  // per `sheets_snapshots.rs`'s own doc comment) and its own restore call
  // — `api.restoreSheetSnapshot`, NEVER `restore_note_version`. On a
  // successful restore this component fires `onRestored()` so the caller
  // (Editor.svelte) can refresh its mounted `SheetGrid` via its exported
  // `reload()`, then closes itself via `onClose()`.
  //
  // "N columns · M rows · K computed" (design's `.sh-vh__pmeta`) is
  // computed from the SAME previewed envelope's own pivot stats
  // (`pivotSnapshotEnvelope`) — no separate backend call, no diffing
  // against a prior version.

  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import * as api from '$lib/api';
  import ShSrcLabel from '$lib/components/ShSrcLabel.svelte';
  import ShSnapshotPreview from '$lib/components/ShSnapshotPreview.svelte';
  import { parseSheetSnapshotEnvelope, pivotSnapshotEnvelope } from '$lib/utils/sheetSnapshotEnvelope';

  interface VersionMeta {
    id: string;
    note_id: string;
    created_at: string;
    source: string;
  }

  interface VersionFull extends VersionMeta {
    content: string;
  }

  interface Props {
    noteId: string;
    onClose: () => void;
    /** Fired after a successful restore, BEFORE `onClose` — the caller
     *  reloads its mounted `SheetGrid` (`sheetGridRef.reload()`). */
    onRestored: () => void;
  }

  let { noteId, onClose, onRestored }: Props = $props();

  let versionList = $state<VersionMeta[]>([]);
  let previewVersion = $state<VersionFull | null>(null);
  let previewComputedValues = $state<Record<string, string>>({});
  let loadingVersions = $state(false);
  let restoring = $state(false);

  /** Monotonic request-token guard (item 32, #2874): a user can click
   *  through versions faster than either fetch resolves. Only the
   *  latest-issued `previewVersionContent` call is allowed to write to
   *  `previewVersion`/`previewComputedValues` — an older, still-in-flight
   *  call whose response arrives after a newer one is simply dropped. */
  let previewRequestToken = 0;

  async function loadVersions() {
    loadingVersions = true;
    try {
      versionList = await invoke('get_note_versions', { noteId });
      if (versionList.length > 0) {
        await previewVersionContent(versionList[0].id);
      }
    } catch (e) {
      console.error('[SHEET_VERSIONS] Failed to load:', e);
    } finally {
      loadingVersions = false;
    }
  }

  async function previewVersionContent(versionId: string) {
    const myToken = ++previewRequestToken;
    try {
      const [version, computed] = await Promise.all([
        invoke('get_note_version', { versionId }) as Promise<VersionFull>,
        api.getSheetSnapshotComputedValues(versionId).catch(() => ({})), // never blocks the raw preview on a compute failure
      ]);
      if (myToken !== previewRequestToken) return; // a newer click superseded this one
      previewVersion = version;
      previewComputedValues = computed;
    } catch (e) {
      console.error('[SHEET_VERSIONS] Failed to load version:', e);
    }
  }

  /** #2339 PR 15b-beta hard requirement: sheet restore calls ONLY
   *  `api.restoreSheetSnapshot` (which itself wraps `restore_sheet_
   *  snapshot`) — never `restore_note_version`. See
   *  `ShVersionHistoryPanel.test.ts`'s dedicated regression test. */
  async function handleRestore() {
    if (!previewVersion || restoring) return;
    restoring = true;
    try {
      await api.restoreSheetSnapshot(noteId, previewVersion.id);
      onRestored();
      onClose();
    } catch (e) {
      console.error('[SHEET_VERSIONS] Failed to restore:', e);
    } finally {
      restoring = false;
    }
  }

  function formatVersionDate(dateStr: string): string {
    try {
      const d = new Date(dateStr);
      return d.toLocaleString('en-US', { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' });
    } catch {
      return dateStr;
    }
  }

  let previewStats = $derived.by(() => {
    if (!previewVersion) return null;
    const envelope = parseSheetSnapshotEnvelope(previewVersion.content);
    if (!envelope) return null;
    const pivoted = pivotSnapshotEnvelope(envelope);
    return { columns: pivoted.columns.length, rows: pivoted.rows.length, computed: pivoted.computedCount };
  });

  onMount(() => {
    loadVersions();
  });
</script>

<div class="sh-vh" data-testid="sheet-version-history-panel">
  <div class="sh-vh__top">
    <span class="sh-vh__hico" aria-hidden="true"><svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/></svg></span>
    <span class="sh-vh__title">Version History</span>
    <button type="button" class="sh-vh__x" onclick={onClose} data-testid="sheet-version-history-close" aria-label="Close"><svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M6 6l12 12M18 6L6 18"/></svg></button>
  </div>
  <div class="sh-vh__body">
    <div class="sh-vh__list" data-testid="sheet-version-history-list">
      {#if loadingVersions}
        <p class="sh-vh__empty">Loading…</p>
      {:else if versionList.length === 0}
        <p class="sh-vh__empty">No versions saved yet</p>
      {:else}
        {#each versionList as v (v.id)}
          <button
            type="button"
            class="sh-snap"
            class:is-on={previewVersion?.id === v.id}
            onclick={() => previewVersionContent(v.id)}
            data-testid="sheet-version-history-item"
          >
            <div class="sh-snap__t">{formatVersionDate(v.created_at)}</div>
            <ShSrcLabel source={v.source} />
          </button>
        {/each}
      {/if}
    </div>
    <div class="sh-vh__pane">
      {#if previewVersion}
        <div class="sh-vh__phead">
          <span class="sh-vh__pt" data-testid="sheet-version-history-preview-date"><b>{formatVersionDate(previewVersion.created_at)}</b></span>
          {#if previewStats}
            <span class="sh-vh__pmeta">{previewStats.columns} columns · {previewStats.rows} rows · {previewStats.computed} computed</span>
          {/if}
          <span class="sh-vh__sp"></span>
          <span class="sh-vh__undo">Restoring takes a snapshot first</span>
          <button
            type="button"
            class="sh-restore"
            onclick={handleRestore}
            disabled={restoring}
            data-testid="sheet-version-history-restore"
          ><svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M23 4v6h-6 M1 20v-6h6 M3.5 9a9 9 0 0 1 14.85-3.36L23 10 M1 14l4.64 4.36A9 9 0 0 0 20.5 15"/></svg><span>Restore</span></button>
        </div>
        <div class="sh-vh__preview-body">
          <ShSnapshotPreview content={previewVersion.content} computedValues={previewComputedValues} />
        </div>
      {:else}
        <p class="sh-vh__preview-empty">Select a version to preview</p>
      {/if}
    </div>
  </div>
</div>

<style>
  /* Ported verbatim from docs/design/sheets/sheets.css (`.sh-vh*`/
     `.sh-snap*`/`.sh-src*` block). `.sh-src*` itself lives in
     `src/app.css` as global rules (same extraction pattern as `.sh-pop`) —
     `ShSrcLabel.svelte` (the component that renders `.sh-src*` markup) has
     no `<style>` block of its own. Kept here is everything else in that
     design CSS block. */
  .sh-vh { width: 100%; height: 100%; box-sizing: border-box; background: #141417; border: 1px solid var(--border-strong); border-radius: var(--radius-lg); display: flex; flex-direction: column; overflow: hidden; }
  .sh-vh__top { display: flex; align-items: center; gap: 10px; padding: 15px 18px; border-bottom: 1px solid var(--border-faint); flex-shrink: 0; }
  .sh-vh__hico { color: var(--fg-3); }
  .sh-vh__title { font-size: 15px; font-weight: 600; color: var(--fg-1); }
  .sh-vh__x { margin-left: auto; color: var(--fg-4); display: flex; background: transparent; border: 0; cursor: pointer; padding: 2px; }
  .sh-vh__x:hover { color: var(--fg-1); }
  .sh-vh__body { flex: 1; min-height: 0; display: flex; }
  .sh-vh__list { width: 246px; flex-shrink: 0; border-right: 1px solid var(--border-faint); overflow: auto; padding: 7px; box-sizing: border-box; }
  .sh-vh__empty { padding: 12px 9px; font-size: 12px; color: var(--fg-4); }
  .sh-snap { display: block; width: 100%; text-align: left; padding: 10px 11px; border-radius: var(--radius-md); border: 0; background: transparent; cursor: pointer; margin-bottom: 2px; }
  .sh-snap:hover { background: var(--overlay-hover); }
  .sh-snap.is-on { background: var(--surface-2); }
  .sh-snap__t { font-size: 12.5px; font-weight: 500; color: var(--fg-1); font-variant-numeric: tabular-nums; margin-bottom: 5px; }
  .sh-vh__pane { flex: 1; min-width: 0; display: flex; flex-direction: column; background: var(--bg); }
  .sh-vh__phead { display: flex; align-items: center; gap: 10px; padding: 12px 16px; border-bottom: 1px solid var(--border-faint); flex-shrink: 0; }
  .sh-vh__pt { font-size: 12.5px; color: var(--fg-2); font-variant-numeric: tabular-nums; }
  .sh-vh__pt b { color: var(--fg-1); font-weight: 600; }
  .sh-vh__pmeta { font-size: 11.5px; color: var(--fg-4); font-variant-numeric: tabular-nums; }
  .sh-vh__sp { flex: 1; }
  .sh-restore { display: inline-flex; align-items: center; gap: 7px; background: var(--accent); color: var(--fg-on-accent); border: 0; border-radius: var(--radius-md); padding: 8px 14px; font-family: var(--font-sans); font-size: 12.5px; font-weight: 600; cursor: pointer; }
  .sh-restore:hover:not(:disabled) { background: var(--accent-hover); }
  .sh-restore:disabled { opacity: 0.6; cursor: default; }
  .sh-vh__undo { font-size: 11px; color: var(--fg-4); white-space: nowrap; }
  .sh-vh__preview-body { flex: 1; min-height: 0; overflow: auto; }
  .sh-vh__preview-empty { padding: 24px; font-size: 12.5px; color: var(--fg-4); }
</style>

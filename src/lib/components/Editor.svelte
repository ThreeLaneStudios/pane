<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { notes, activeNote } from '$lib/stores/app';
  import { SYSTEM_NOTE_TYPES } from '$lib/constants/systemNoteTypes';
  import { get } from 'svelte/store';
  import { Editor } from '@tiptap/core';
  // probe: instrument all $activeNote write sites — investigating #2214 delete-close latch
  const DEBUG = typeof localStorage !== 'undefined' && localStorage.getItem('pane-debug') === '1';
  function logAnSet(caller: string, newVal: unknown): void {
    if (!DEBUG) return;
    const oldId = get(activeNote)?.meta?.id ?? 'null';
    const newId = (newVal as any)?.meta?.id ?? 'null';
    console.log(`[PANE-DEBUG] $activeNote SET in ${caller} | old="${oldId}" new="${newId}"`);
    console.log('[PANE-DEBUG] stack:', new Error().stack?.split('\n').slice(1, 5).join('\n'));
  }
  import { StarterKit } from '@tiptap/starter-kit';
  import { Placeholder } from '@tiptap/extension-placeholder';
  import { TaskList } from '@tiptap/extension-task-list';
  import { TaskItem } from '@tiptap/extension-task-item';
  import { Link } from '@tiptap/extension-link';
  import { CodeBlockLowlight } from '@tiptap/extension-code-block-lowlight';
  import { Table } from '@tiptap/extension-table';
  import { TableRow } from '@tiptap/extension-table-row';
  import { TableCell } from '@tiptap/extension-table-cell';
  import { TableHeader } from '@tiptap/extension-table-header';
  import { Typography } from '@tiptap/extension-typography';
  import { Underline } from '@tiptap/extension-underline';
  import { common, createLowlight } from 'lowlight';
  import { TextStyle } from '@tiptap/extension-text-style';
  import { WikiLink } from '$lib/extensions/WikiLink';
  import { Attachment, defaultRenderMode } from '$lib/extensions/Attachment';
  import { Checklist } from '$lib/extensions/Checklist';
  import { DueToday } from '$lib/extensions/DueToday';
  import { buildAttachmentRawHtmlBlock } from '$lib/utils/attachmentSerializer';
  import { DiffHighlight, applyDiffHighlight } from '$lib/extensions/DiffHighlight';
  import { FontSize } from '$lib/extensions/FontSize';
  import { editorPreEditText } from '$lib/stores/editor';
  import { setEditorSavePromise } from '$lib/stores/editorSave';
  import Backlinks from '$lib/components/Backlinks.svelte';
  import LinkInsertModal from '$lib/components/LinkInsertModal.svelte';
  import PromptDialog from '$lib/components/PromptDialog.svelte';
  import SheetGrid from '$lib/components/SheetGrid.svelte';
  import type { FormulaBarInfo } from '$lib/components/SheetGrid.svelte';
  import SheetToolbar from '$lib/components/SheetToolbar.svelte';
  import SheetStatusBar from '$lib/components/SheetStatusBar.svelte';
  import FormulaBar from '$lib/components/FormulaBar.svelte';
  import ShVersionHistoryPanel from '$lib/components/ShVersionHistoryPanel.svelte';
  import { getStageColor } from '$lib/utils/stageColors';
  import { filterSupported } from '$lib/utils/sheetView';

  import * as api from '$lib/api';
  import { autoSync } from '$lib/sync';
  import { invoke } from '@tauri-apps/api/core';
  import {
    ENTITY_NOTE,
    ENTITY_SHEET,
    REASON_EDITED,
    REASON_MOVED,
    subscribeDataChanged,
    dispatchDataChanged,
    makeOriginTag,
    registerOwnOriginTag,
    isOwnOriginTag,
    type DataChangedPayload,
  } from '$lib/refreshEvent';
  import type { NoteMeta, Project } from '$lib/types';

  const lowlight = createLowlight(common);

  interface Props {
    content?: string;
    title?: string;
    slug?: string; // NEW: Editor needs slug to save itself
    onUpdate?: (markdown: string, jsonContent?: string) => void; // OPTIONAL: for parent to listen to changes
    onTitleUpdate?: (title: string) => void;
    placeholder?: string;
    onNavigate?: (title: string) => void;
    currentProject?: string;
    noteId?: string;
    noteMeta?: NoteMeta;
    jsonContent?: string;
    isSaving?: boolean;
    lastSaved?: string;
    onDelete?: () => void;
    onMoved?: () => void;
  }

  let { content = '', title = '', slug = '', onUpdate, onTitleUpdate, placeholder = 'Start writing...', onNavigate, currentProject = 'Notes', noteId, noteMeta, jsonContent, isSaving = false, lastSaved = '', onDelete, onMoved }: Props = $props();

  let isReadOnly = $derived(!!(noteMeta?.note_type && (SYSTEM_NOTE_TYPES as readonly string[]).includes(noteMeta.note_type)));
  // #2339 PR9: sheets are NOT part of the SYSTEM_NOTE_TYPES read-only-doc
  // machinery above — they render an entirely different body (SheetGrid,
  // no TipTap instance) rather than a read-only version of the doc editor.
  // Deliberately its own derived flag, not folded into isReadOnly.
  let isSheet = $derived(noteMeta?.note_type === 'sheet');
  // #2339 PR9: populated by SheetGrid's onLoaded callback once get_sheet_data
  // resolves, and consumed by SheetStatusBar — avoids a second fetch just for counts.
  let sheetColsCount = $state(0);
  let sheetRowsCount = $state(0);
  // #2339 PR14 (Matt ruling 2026-09-05 #4): populated from the SAME
  // onLoaded/refetch payload — SheetData.formula_count/formulas_paused —
  // and consumed by SheetStatusBar's formulas/paused props and the
  // ShPaused banner below. No second fetch, same reasoning as cols/rows.
  let sheetFormulaCount = $state(0);
  let sheetFormulasPaused = $state(false);
  // #2339 PR14: the live formula-bar feed pushed reactively by SheetGrid's
  // onSelectionInfo callback — anchor cell address/value/editing/error,
  // consumed by the mounted FormulaBar below.
  let formulaBarInfo: FormulaBarInfo = $state({ cref: '', value: '', editing: false, error: null, editable: false, selectionStart: null, selectionEnd: null, selectedColumnIndex: null, selectedColumnType: null });
  // #2339 PR10 (ruling 5): bind:this ref so SheetToolbar's insert-row/
  // insert-column buttons can call straight through to SheetGrid's
  // exported insertRow()/insertColumn() — same imperative `export
  // function` + `bind:this` pattern already used for toolbarEl/editor
  // elsewhere in this file. #2339 PR14 extends the same bind:this handle
  // with the barBeginEdit/barSetValue/barCommit/barCancel exports
  // FormulaBar drives.
  let sheetGridRef: {
    insertRow: () => Promise<void>;
    insertColumn: () => Promise<void>;
    barBeginEdit: () => void;
    barSetValue: (value: string) => void;
    barSetSelection: (start: number, end: number) => void;
    barCommit: (cause: 'enter' | 'blur') => void;
    barCancel: () => void;
    // #2339 PR 15b-beta: reload() re-fetches sheet data after a
    // sheet-native version restore, without resetting selection/scroll.
    reload: () => Promise<void>;
    // #2339 PR 15d-beta: opens ShStylePanel — SheetToolbar's fill/text/
    // border buttons call THIS through the bind:this handle (the toolbar
    // itself stays ignorant of selection/unanimity, Matt's callback-
    // pattern ruling), same as insertRow/insertColumn above.
    //
    // #2874 item 16 Part B: now accepts the REAL clicked button element
    // (an `HTMLElement`) instead of only a raw `{x,y}` point —
    // `SheetToolbar`'s fill/text/border onclick handlers below pass their
    // own `bind:this` button ref, so `SheetGrid.svelte` can anchor the
    // panel under the actual button (`getBoundingClientRect()`) and thread
    // it into `ShStylePanel`'s new `anchorEl` prop for the same-click
    // self-close fix. The `{x,y}` overload survives for any future
    // non-element caller; nothing in this file uses it anymore.
    openStylePanel: (anchor?: HTMLElement | { x: number; y: number }) => void;
    // #2874 item 16 Part C: toolbar Sort/Filter/Formula wiring — same
    // imperative bind:this pattern as every export above. `toolbarSort`
    // takes no argument: the toolbar door cycles asc → desc → cleared on
    // successive presses (Matt's ruling, item 16).
    toolbarSort: () => void;
    toolbarFilter: (anchorEl: HTMLElement) => void;
    openFormulaOnAnchor: () => void;
  } | undefined = undefined;

  /** #2339 PR14: routes a keystroke typed directly into the mounted
   *  FormulaBar back through SheetGrid's shared edit buffer — opens an
   *  edit on the anchor cell first (via `barBeginEdit`) if one isn't
   *  already in flight, matching "one formula, two places to edit it." */
  function handleFormulaBarInput(next: string) {
    sheetGridRef?.barSetValue(next);
  }
  let currentFolderName = $derived(noteMeta?.notebook?.split('/')?.slice(1)?.join('/') || 'Folder');

  let element: HTMLDivElement;
  let editor: Editor | null = $state(null);
  let isFocused = $state(false);
  let titleInput: HTMLInputElement;
  let noteTitle = $state(title || '');
  let isInternalUpdate = false;
  let isSettingContent = false;
  let isDestroyed = false;

  // Version history state
  let showVersionHistory = $state(false);
  let versionList = $state<Array<{ id: string; note_id: string; created_at: string; source: string }>>([]);
  let previewVersion = $state<{ id: string; content: string; created_at: string; source: string } | null>(null);
  let loadingVersions = $state(false);

  // Pin to context state
  let isPinned = $state(false);
  let loadingPin = $state(false);

  // Link insert modal state (#1898 PR8)
  let linkModalOpen = $state(false);
  let linkPromptOpen = $state(false);

  // ⋯ note menu state (§32.2, §32.3)
  // spec-covered-by: #2121 (§32, ADR-0058)
  let moreMenuOpen = $state(false);
  // Responsive overflow state — ResizeObserver triggers on measured content overflow
  let isNarrow = $state(false);
  let toolbarEl: HTMLElement | null = $state(null);
  let moveToTrashLoading = $state(false);
  // Save-as-template prompt
  let savePromptOpen = $state(false);
  let savePromptName = $state('');
  let savePromptSaving = $state(false);
  // Collision: single match
  let collisionSingleOpen = $state(false);
  let collisionSingleMatch: import('$lib/types').Template | null = $state(null);
  let collisionSingleName = $state('');
  let collisionSingleContent = $state('');
  let collisionSingleChecklist = $state('');
  // Collision: multi match
  let collisionMultiOpen = $state(false);
  let collisionMultiMatches: import('$lib/types').Template[] = $state([]);
  let collisionMultiName = $state('');
  let collisionMultiContent = $state('');
  let collisionMultiChecklist = $state('');
  let collisionMultiSel: string | null = $state(null);

  function openMoreMenu() { moreMenuOpen = !moreMenuOpen; }

  function openApplyPicker() {
    moreMenuOpen = false;
    window.dispatchEvent(new CustomEvent('pane-show-templates'));
  }

  function openSaveAsTemplate() {
    moreMenuOpen = false;
    savePromptName = noteTitle || '';
    savePromptOpen = true;
  }

  function closeSavePrompt() {
    savePromptOpen = false;
    savePromptSaving = false;
  }

  async function confirmSaveTemplate() {
    const name = savePromptName.trim();
    if (!name || !noteId) return;
    savePromptSaving = true;
    try {
      // Capture note body as markdown; no frontend date-stamping
      const bodyContent = (editor?.storage as any)?.markdown?.getMarkdown?.() ?? editor?.getHTML() ?? '';
      // Capture checklist items from subtasks
      let checklistJson = '[]';
      try {
        const allSubtasks = await invoke('get_subtasks_for_note', { noteId });
        const subtasks = (allSubtasks as any[]).map((s: any) => ({ text: s.text, sort_order: s.sort_order }));
        checklistJson = JSON.stringify(subtasks);
      } catch { /* no subtasks or command not yet available */ }

      // Check for name collision (case-insensitive, §32.2.2)
      const matches = await api.listTemplatesByName(name);
      if (matches.length === 0) {
        await api.saveTemplate(name, bodyContent, checklistJson);
        savePromptOpen = false;
      } else if (matches.length === 1) {
        // Single match → collision single prompt
        savePromptOpen = false;
        collisionSingleMatch = matches[0];
        collisionSingleName = name;
        collisionSingleContent = bodyContent;
        collisionSingleChecklist = checklistJson;
        collisionSingleOpen = true;
      } else {
        // Multi match → collision multi prompt
        savePromptOpen = false;
        collisionMultiMatches = matches;
        collisionMultiName = name;
        collisionMultiContent = bodyContent;
        collisionMultiChecklist = checklistJson;
        collisionMultiSel = null;
        collisionMultiOpen = true;
      }
    } catch (e) {
      console.error('[Templates] Save failed:', e);
    } finally {
      savePromptSaving = false;
    }
  }

  async function collisionOverwriteSingle() {
    if (!collisionSingleMatch) return;
    try {
      await api.overwriteTemplate(collisionSingleMatch.id, collisionSingleContent, collisionSingleChecklist);
    } catch (e) { console.error('[Templates] Overwrite failed:', e); }
    collisionSingleOpen = false;
    collisionSingleMatch = null;
  }

  function collisionRenameSingle() {
    collisionSingleOpen = false;
    collisionSingleMatch = null;
    // Re-open save prompt with existing name for user to edit
    savePromptName = collisionSingleName;
    savePromptOpen = true;
  }

  async function collisionOverwriteMulti() {
    if (!collisionMultiSel) return;
    try {
      await api.overwriteTemplate(collisionMultiSel, collisionMultiContent, collisionMultiChecklist);
    } catch (e) { console.error('[Templates] Overwrite (multi) failed:', e); }
    collisionMultiOpen = false;
    collisionMultiSel = null;
  }

  function collisionRenameMulti() {
    collisionMultiOpen = false;
    collisionMultiSel = null;
    savePromptName = collisionMultiName;
    savePromptOpen = true;
  }

  function formatTemplateDate(iso: string): string {
    try {
      return new Date(iso).toLocaleDateString('en-US', { month: 'short', day: 'numeric', year: 'numeric' });
    } catch { return iso; }
  }

  // Close ⋯ menu on outside click
  function handleMoreMenuOutside(e: MouseEvent) {
    if (!moreMenuOpen) return;
    const target = e.target as HTMLElement;
    if (!target.closest('.tpl-more-wrap')) {
      moreMenuOpen = false;
    }
  }

  function handleLinkModalSubmit(e: CustomEvent<string>) {
    const normalizedUrl = e.detail;
    editor?.chain().focus().insertAttachment({
      attachmentType: 'link',
      src: normalizedUrl,
      renderMode: 'preview',
      // attachmentId intentionally omitted — link nodes have no file upload (per §5 audit)
    }).run();
    linkModalOpen = false;

    // Fire-and-forget og fetch (#1780, ADR-0049 Decision 2).
    // Card renders immediately with domain stub; og data fills in when fetch resolves.
    // Result is cached in og_cache — AttachmentNodeView reads cache on next render.
    api.fetchLinkPreview(normalizedUrl).catch((err) => {
      console.warn('[Editor] og prefetch failed (non-fatal):', err);
    });
  }

  function handleLinkModalCancel() {
    linkModalOpen = false;
  }

  // Metadata pill dropdown state
  let editorStatusOpen = $state(false);
  let editorPriorityOpen = $state(false);
  let editorDueDateOpen = $state(false);
  let editorFolderOpen = $state(false);
  let projectFolders: Project[] = $state([]);
  // Fixed viewport anchors for pill dropdowns — escape overflow:hidden on
  // .title-row / .editor-wrapper / .fv-note ancestors (#2205).
  // position:fixed is viewport-relative and bypasses overflow clipping.
  let pillStatusAnchor = $state<{ top: number; left: number } | null>(null);
  let pillPriorityAnchor = $state<{ top: number; left: number } | null>(null);
  let pillDueDateAnchor = $state<{ top: number; left: number } | null>(null);
  // Folder picker aligns its right edge to the pill button's right edge.
  let pillFolderAnchor = $state<{ top: number; right: number } | null>(null);

  async function openVersionHistory() {
    if (!noteId) return;
    // #2339 PR 15b-beta: sheets get the sheet-native `ShVersionHistoryPanel`
    // below, which fetches its own version list/preview content on mount
    // (it needs to parse snapshot envelopes, not markdown) — nothing to
    // preload here. Markdown notes keep the existing eager-fetch-then-open
    // flow so the list is ready the instant the overlay renders.
    if (isSheet) {
      showVersionHistory = true;
      return;
    }
    loadingVersions = true;
    try {
      versionList = await invoke('get_note_versions', { noteId });
      showVersionHistory = true;
    } catch (e) {
      console.error('[VERSIONS] Failed to load:', e);
    } finally {
      loadingVersions = false;
    }
  }

  async function loadPinState() {
    if (!noteId) return;
    try {
      isPinned = await invoke('is_note_pinned', { noteId });
    } catch (e) {
      console.error('[PIN] Failed to check pin state:', e);
      isPinned = false;
    }
  }

  async function handlePinToggle() {
    if (!noteId || loadingPin) return;
    loadingPin = true;
    try {
      if (isPinned) {
        await invoke('unpin_note_from_context', { noteId });
        isPinned = false;
      } else {
        await invoke('pin_note_to_context', { noteId });
        isPinned = true;
      }
    } catch (e) {
      console.error('[PIN] Failed to toggle pin:', e);
      alert('Failed to toggle pin: ' + e);
    } finally {
      loadingPin = false;
    }
  }

  // ── Overflow toolbar handlers (shared between toolbar buttons and ⋯ menu) ──

  function handleToolbarLink() {
    moreMenuOpen = false;
    linkPromptOpen = true;
  }

  function submitToolbarLink(url: string) {
    linkPromptOpen = false;
    editor?.chain().focus().setLink({ href: url }).run();
  }

  function handleToolbarCodeBlock() {
    moreMenuOpen = false;
    editor?.chain().focus().toggleCodeBlock().run();
  }

  function handleToolbarUndo() {
    moreMenuOpen = false;
    editor?.chain().focus().undo().run();
  }

  function handleToolbarRedo() {
    moreMenuOpen = false;
    editor?.chain().focus().redo().run();
  }

  function handleToolbarInsertLink() {
    moreMenuOpen = false;
    linkModalOpen = true;
  }

  function handleToolbarChecklist() {
    moreMenuOpen = false;
    editor?.chain().focus().insertChecklist().run();
  }

  function handleToolbarDueToday() {
    moreMenuOpen = false;
    editor?.chain().focus().insertDueToday().run();
  }

  async function handleToolbarAttachFile() {
    moreMenuOpen = false;
    const { open } = await import('@tauri-apps/plugin-dialog');
    const sel = await open({ multiple: true });
    if (!sel) return;
    const paths = Array.isArray(sel) ? sel : [sel];
    for (const filePath of paths) {
      const filename = filePath.split('/').pop() || filePath;
      try {
        const attachment = await api.copyAttachmentFromPath(filename, filePath, noteMeta?.project_id || undefined);
        const detectedType = /\.(jpg|jpeg|png|gif|webp|svg)$/i.test(filename) ? 'image'
          : /\.(mp3|wav|ogg|m4a|aac|flac)$/i.test(filename) ? 'audio'
          : /\.(mp4|webm|mov|avi|mkv)$/i.test(filename) ? 'video'
          : 'file';
        const renderMode = defaultRenderMode(detectedType);
        editor?.commands.insertAttachment({
          src: '',
          filename: attachment.filename,
          attachmentId: attachment.id,
          attachmentType: detectedType,
          renderMode,
          size: attachment.size,
        });
      } catch (error) {
        console.error('Failed to attach file via picker:', error);
      }
    }
  }

  async function handleMoveToTrash() {
    if (!noteId || moveToTrashLoading) return;
    moreMenuOpen = false;
    moveToTrashLoading = true;
    try {
      await api.moveToTrash(noteId);
      // #2192: Notify parent (e.g. folder view) so it can call goBackToCards().
      // Falls back to direct null-set for contexts without an onDelete handler.
      if (onDelete) {
        onDelete();
      } else {
        logAnSet('Editor.moveToTrash[no-onDelete]', null);
        activeNote.set(null);
      }
      // #2972 stage 3 (item 1): duplicate dispatch deleted. `api.moveToTrash()`
      // above already dispatches the single data-changed event itself
      // (§34.1.2) — this was a second, bare, payload-less dispatch for the
      // exact same `delete_note` mutation (confirmed duplicate at main,
      // §34.1.3/§34.2's "Replaced — duplicate dispatch removed" verdict).
    } catch (e) {
      console.error('[EDITOR] Failed to move note to trash:', e);
    } finally {
      moveToTrashLoading = false;
    }
  }

  // === #2339 PR 15b-alpha — Sheet "More" overflow menu handlers ===
  // These back `ShSheetMenu.svelte`'s rows (wired via `SheetToolbar`'s
  // `onExportCsv`/`onVersionHistory`/`onDeleteSheet` props below).
  // Version history and delete reuse the exact same mechanisms every
  // other note type already uses (`openVersionHistory`/
  // `handleMoveToTrash` above) — sheets are notes (`note_type: 'sheet'`),
  // so nothing note-level needed changing. "Rename sheet" was removed
  // (#2874 item 33) — the sheet title `<input>` is already directly
  // click-to-edit, so a dedicated rename menu item/handler was redundant
  // and has been deleted. "Move to…" was removed (#2874 item 49) — the
  // folder pill in this file's title row already covers moving a sheet
  // between folders (and the menu row was non-functional besides), so
  // `handleMoveToSheet` was deleted along with it. The folder pill's own
  // mechanism (`toggleFolderPicker`/`handleFolderMove` below, plus
  // `folderPillBtn`) is unrelated to the removed menu row and is
  // untouched.
  let folderPillBtn: HTMLButtonElement | null = $state(null);

  /** Export as CSV…: calls the existing read-only `export_sheet_csv`
   *  backend command (#2339 PR 5c — already implemented, tested, and
   *  registered; ships callable-but-unwired per that PR's own docs), then
   *  opens the native OS save dialog defaulted to the sanitized filename
   *  the backend suggests, then writes the returned CSV text to the
   *  chosen path via `@tauri-apps/plugin-fs`'s `writeTextFile`. Every
   *  step is wrapped in one try/catch — a cancelled save dialog resolves
   *  `path` to `null` and is treated as a silent no-op (not an error). */
  async function handleExportSheetCsv() {
    if (!noteId) return;
    try {
      const result = await invoke<{ csv_content: string; filename: string }>('export_sheet_csv', { noteId });
      const { save } = await import('@tauri-apps/plugin-dialog');
      const path = await save({
        defaultPath: result.filename,
        filters: [{ name: 'CSV', extensions: ['csv'] }],
      });
      if (!path) return; // user cancelled the save dialog
      const { writeTextFile } = await import('@tauri-apps/plugin-fs');
      await writeTextFile(path, result.csv_content);
    } catch (e) {
      console.error('[SHEET] Failed to export CSV:', e);
      showErrorToast('Failed to export sheet as CSV.');
    }
  }

  // ResizeObserver for toolbar overflow — measurement-based collapse (#2135)
  // Replaces the fixed 330px threshold with actual content-overflow detection.
  // Two-pass strategy: temporarily un-collapse to measure full content width so
  // we always know whether ALL toolbar items would fit (not just the visible ones).
  // Does not cause infinite loops because the container's clientWidth doesn't
  // change when we toggle overflow items — only scrollWidth does.
  $effect(() => {
    if (!toolbarEl) return;
    const obs = new ResizeObserver(() => {
      if (!toolbarEl) return;
      // Guard: zero-width container (unmounting or hidden panel) — stay in current state
      if (toolbarEl.clientWidth === 0) return;

      // Pass 1: temporarily reveal overflow items so we measure the FULL natural width.
      // We manipulate the class directly on the element (not through Svelte state) so
      // there is no render cycle and no visible flash — layout is read synchronously.
      const wasNarrow = toolbarEl.classList.contains('toolbar--narrow');
      if (wasNarrow) toolbarEl.classList.remove('toolbar--narrow');

      // Force layout read — browsers compute synchronously when scrollWidth is accessed
      const shouldBeNarrow = toolbarEl.scrollWidth > toolbarEl.clientWidth;

      // Pass 2: if we revealed items only to measure and content still overflows,
      // immediately re-hide them before paint so there is no flash.
      if (wasNarrow && shouldBeNarrow) {
        toolbarEl.classList.add('toolbar--narrow');
      }

      // Sync Svelte state — Svelte's class: binding will align on next microtask
      isNarrow = shouldBeNarrow;
    });
    obs.observe(toolbarEl);
    return () => obs.disconnect();
  });

  function formatEditorStatus(s: string): string {
    return s.split('_').map(w => w.charAt(0).toUpperCase() + w.slice(1)).join(' ');
  }



  function getEditorPriorityColor(priority: string | null | undefined): string {
    switch(priority) {
      case 'urgent': return '#FF4444';
      case 'high': return '#FF8C00';
      case 'medium': return '#00E4E4';
      case 'low': return 'rgba(255,255,255,0.3)';
      default: return '#00E4E4';
    }
  }

  async function updateEditorMeta(field: string, value: string) {
    if (!noteId) return;
    // #2972 stage 3 (§34.4): mint and register the tag BEFORE the save
    // starts (synchronously — registerOwnOriginTag's own comment explains
    // why this ordering is required), so this Editor instance's own
    // handleDataChanged (below) can recognize the echo this call's
    // dispatch produces and skip the resulting redundant refetch.
    const originTag = makeOriginTag();
    registerOwnOriginTag(originTag);
    try {
      const request: Record<string, any> = { note_id: noteId };
      if (field === 'task_status') request.status = value;
      else if (field === 'task_priority') request.priority = value;
      else if (field === 'task_due_date') request.due_date = value;

      await invoke('update_note', { request });

      if (noteMeta) {
        (noteMeta as any)[field] = value;
        noteMeta = { ...noteMeta };
      }
      editorStatusOpen = false;
      editorPriorityOpen = false;
      editorDueDateOpen = false;
      editorFolderOpen = false;
      // #2972 stage 3 (item 2, ruling 4): folds the old `pane-task-meta-
      // changed` dispatch into the single data-changed event. This call
      // site bypasses api.ts entirely (direct invoke() above), so unlike
      // saveNote()/saveSheetTitle() it needs its own dispatch here — every
      // sibling screen's already-built Stage 2 handler does a full
      // single-entity read-and-patch on any `reason: "edited"` event
      // regardless of which field changed, so no field-level detail needs
      // to travel in this payload the way the old event's `{noteId, field,
      // value}` shape carried it.
      dispatchDataChanged(noteId, isSheet ? ENTITY_SHEET : ENTITY_NOTE, REASON_EDITED, originTag);
    } catch(e) {
      console.error('Failed to update note metadata:', e);
    }
  }

  async function toggleFolderPicker(btn: HTMLElement) {
    if (!noteMeta?.project_id) return;
    if (editorFolderOpen) {
      editorFolderOpen = false;
      pillFolderAnchor = null;
      return;
    }
    // Compute fixed viewport anchor so the dropdown escapes overflow:hidden ancestors (#2205).
    const r = btn.getBoundingClientRect();
    pillFolderAnchor = { top: r.bottom + 6, right: window.innerWidth - r.right };
    // Lazy-load folders on first open
    try {
      const folders = await api.listFolders(noteMeta.project_id);
      projectFolders = folders.filter((f: any) => !f.is_system);
    } catch (e) {
      console.error('[FOLDER_PICKER] Failed to load folders:', e);
      pillFolderAnchor = null;
      return;
    }
    editorFolderOpen = true;
  }

  async function handleFolderMove(folder: Project) {
    if (folder.id === noteMeta?.folder_id) {
      editorFolderOpen = false;
      return;
    }
    if (!noteMeta?.id || !noteMeta?.project_id) return;
    // #3053 item 2 (docs/audits/2026-10-20-scope-1061-fixes.md, D1/D2 part
    // 2): capture the id/entity-type this call will dispatch BEFORE
    // calling `onMoved()` below. `onMoved()` can synchronously null the
    // parent's `$activeNote` (both existing callers do this —
    // `AllTasksView.handleCloseEditor` and `NotesBrowser.goBackToCards`),
    // and both render sites gate `<Editor>` itself on `$activeNote`
    // (`{#if showEditor && $activeNote}`) — so `onMoved()` can trigger a
    // synchronous destroy of this very component while this async
    // function's closure is still running past it. Reading `noteMeta.id`
    // AFTER that call (the old order) risked reading off a component
    // already mid-teardown; these two consts are read while `noteMeta` is
    // still guaranteed live.
    const movedNoteId = noteMeta.id;
    const movedEntityType = isSheet ? ENTITY_SHEET : ENTITY_NOTE;
    try {
      await invoke('update_note', {
        request: {
          note_id: noteMeta.id,
          project_id: noteMeta.project_id,
          folder_id: folder.id
        }
      });
      // Update local state
      noteMeta.folder_id = folder.id;
      noteMeta.notebook = noteMeta.notebook?.split('/')[0] + '/' + folder.name;
      noteMeta = { ...noteMeta };
      editorFolderOpen = false;
      // #2972 stage 3 (§34.1.3): migrated from the bare, payload-less
      // `pane-data-changed` CustomEvent to the single entity-identified
      // event, `reason: "moved"` — so the source folder's screen patches
      // this one note/sheet out in place rather than needing a reload.
      // Dispatched BEFORE `onMoved()` now (see this function's #3053
      // comment above) — using the consts captured before this component
      // can be torn down, not a re-read of `noteMeta` after the fact.
      dispatchDataChanged(movedNoteId, movedEntityType, REASON_MOVED);
      // #2221: Note left the current folder — close the panel (mirrors delete path).
      // Falls back to clearing activeNote directly for contexts without an onMoved handler.
      if (onMoved) {
        onMoved();
      } else {
        logAnSet('Editor.moveToFolder[no-onMoved]', null);
        activeNote.set(null);
      }
    } catch (e) {
      // #3053 item 2: this catch used to be console-only — a failed move
      // (network/DB error, a guard rejection, etc.) was invisible to the
      // user; the folder picker just silently closed as if it had worked.
      // Surfaced via the same error-toast mechanism this file already uses
      // for upload/export failures (`showErrorToast`, above).
      console.error('[FOLDER_PICKER] Failed to move note:', e);
      showErrorToast('Failed to move note to folder.');
    }
  }

  async function previewVersionContent(versionId: string) {
    try {
      previewVersion = await invoke('get_note_version', { versionId });
    } catch (e) {
      console.error('[VERSIONS] Failed to load version:', e);
    }
  }

  /**
   * Shared helper: strip markdown title, convert to HTML, set editor content synchronously,
   * then resolve attachment URLs in the background (non-blocking).
   *
   * Call sites: restoreFromVersion(), loadInitialContent() markdown path.
   *
   * Intentionally NOT used by:
   *  - $effect late-arrival handler: defers initial setContent until after URL resolution
   *    (avoids a flash of unresolved content) and has a catch fallback path.
   *  - handleReload (AI-reload): awaits resolution synchronously, then applies diff
   *    highlighting + reloads metadata - too different to share cleanly.
   *  - setContent() export: no title-strip by design.
   *
   * @param markdown   Raw markdown string (may include leading `# Title` line).
   * @param logTag     Prefix for console messages, e.g. 'VERSIONS' or 'EDITOR-MOUNT'.
   * @param afterSync  Optional callback invoked right after the synchronous setContent
   *                   but before background URL resolution starts.
   */
  function applyMarkdownContent(markdown: string, logTag: string, afterSync?: () => void): void {
    if (!editor) return;
    const html = markdownToHtml(markdown);
    isSettingContent = true;
    editor.commands.setContent(html);
    console.log(`[${logTag}] Set content in editor, html length:`, html.length);
    isSettingContent = false;
    dirty = false;
    if (afterSync) afterSync();
    // Attachment display is handled by the NodeView (resolveAttachmentSrc via asset protocol).
    // No post-load URL resolution step needed here.
  }

  async function restoreFromVersion(versionId: string) {
    if (!noteId || !currentProject) return;
    try {
      await invoke('restore_note_version', { noteId, versionId });
      showVersionHistory = false;
      previewVersion = null;
      // Reload the note content
      const noteContent = await api.readNote(currentProject, noteId);
      if (noteContent && editor) {
        const restoredMd = noteContent.content || '';
        applyMarkdownContent(restoredMd, 'VERSIONS');
      }
    } catch (e) {
      console.error('[VERSIONS] Failed to restore:', e);
    }
  }

  function formatVersionDate(dateStr: string): string {
    try {
      const d = new Date(dateStr);
      return d.toLocaleString('en-US', { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' });
    } catch { return dateStr; }
  }

  function sourceLabel(source: string): string {
    const labels: Record<string, string> = {
      ai_edit: 'AI Edit',
      ai_append: 'AI Append',
      ai_synthesis: 'AI Synthesis',
      synthesis: 'Synthesis',
      pending_synthesis: 'Pending Synthesis',
      pending_ai_edit: 'Pending AI Edit',
      pre_restore: 'Pre-Restore',
      manual: 'Manual',
    };
    return labels[source] || source;
  }
  let linkRebuildTimeout: number;

  // Linked tasks
  import type { Task } from '$lib/types';
  let linkedTasks: Task[] = $state([]);
  let showNewTaskInput = $state(false);
  let newTaskTitle = $state('');
  let newTaskInputEl: HTMLInputElement | null = $state(null);

  async function loadLinkedTasks() {
    if (!noteId) return;
    try {
      linkedTasks = await api.listTasksByNote(noteId);
    } catch (e) {
      console.error('[EDITOR] Failed to load linked tasks:', e);
    }
  }

  async function createLinkedTask() {
    if (!newTaskTitle.trim() || !noteId) return;
    if (!noteMeta?.project_id) return;
    try {
      await api.createNoteInProject(
        noteMeta?.project_id || '',
        noteMeta?.folder_id || undefined,
        newTaskTitle.trim(),
        'todo',
      );
      newTaskTitle = '';
      showNewTaskInput = false;
      await loadLinkedTasks();
    } catch (e) {
      console.error('[EDITOR] Failed to create linked task:', e);
    }
  }

  async function toggleLinkedTask(task: Task) {
    try {
      const newStatus = task.status === 'done' ? 'todo' : 'done';
      await api.updateTask({
        ...task,
        status: newStatus,
        completed_at: newStatus === 'done' ? new Date().toISOString() : undefined,
        updated_at: new Date().toISOString(),
      });
      await loadLinkedTasks();
    } catch (e) {
      console.error('[EDITOR] Failed to toggle task:', e);
    }
  }

  // CRITICAL: Capture slug and notebook at mount time as plain strings.
  // Svelte 5 props are reactive getters - if $activeNote changes before onDestroy,
  // reading `slug` would return the NEW note's slug, causing saves to wrong file.
  let mountedSlug = '';
  let mountedProject = '';

  // Cache latest content for onDestroy - editor.getHTML() may return empty if DOM is detached
  let lastMd = '';
  let dirty = $state(false); // Track if user has edited content
  let editorUpdateTimeout: ReturnType<typeof setTimeout>;

  // === Attachment upload size constant (#1799) ===
  // Paste/drag-drop path — clipboard/DOM files have no FS path; base64 through JSC is the only option.
  // 50 MB → ~270 MB peak heap (×1.33 base64 × 2 UTF-16 × ~1 transient copy), safely under OOM threshold.
  // File-picker bypasses this via copy_attachment_from_path (1 GB Rust cap).
  const PASTE_MAX_BYTES = 50 * 1024 * 1024; // 50 MB

  // Error toast state (#1815)
  let errorToastMsg = $state<string | null>(null);
  let errorToastTimer: ReturnType<typeof setTimeout> | null = null;

  function showErrorToast(msg: string) {
    errorToastMsg = msg;
    if (errorToastTimer) clearTimeout(errorToastTimer);
    errorToastTimer = setTimeout(() => { errorToastMsg = null; }, 5000);
  }

  // Handle file uploads — paste and DOM-drop paths.
  // File picker is handled by the toolbar button via plugin-dialog open().
  async function handleFileUpload(file: File): Promise<{ id: string; url: string; filename: string; size?: number | null }> {
    // Guard: reject files over 50 MB before allocating any FileReader memory.
    if (file.size > PASTE_MAX_BYTES) {
      return Promise.reject(
        new Error(
          `For files over 50 MB, please use the file picker (+ button).`
        )
      );
    }

    // Standard path: FileReader + base64 + IPC (paste has no filesystem path)
    return new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = async () => {
        try {
          const base64Full = reader.result as string;
          const base64Data = base64Full.split(',')[1];
          const attachment = await api.uploadAttachment(file.name, base64Data, noteMeta?.project_id || undefined);
          resolve({
            id: attachment.id,
            url: base64Full,
            filename: attachment.filename,
            size: attachment.size,
          });
        } catch (error) {
          reject(error);
        }
      };
      reader.onerror = () => {
        const errMsg = reader.error?.message || 'Unknown FileReader error';
        reject(new Error(`Failed to read file "${file.name}": ${errMsg}`));
      };
      reader.onabort = () => {
        reject(new Error(`File read aborted: "${file.name}"`));
      };
      reader.readAsDataURL(file);
    });
  }

  // Handle note navigation from wiki links
  function handleNoteNavigation(title: string) {
    if (onNavigate) {
      onNavigate(title);
    }
  }

  // Editor saves directly using its own props, not parent callbacks
  async function saveContent(md: string) {
    if (isDestroyed) return; // Don't save after component destroyed
    // CRITICAL: Use mountedSlug/mountedProject (captured at mount), NOT reactive props.
    // Props may reflect the NEW note if $activeNote changed before onDestroy fires.
    const saveSlug = mountedSlug;
    const saveProject = mountedProject;
    console.log('[EDITOR-SAVE] saveContent called. slug:', saveSlug, 'currentProject:', saveProject, 'md length:', md.length);
    if (!saveProject || !saveSlug) {
      console.warn('[EDITOR-SAVE] SKIPPED - missing slug or currentProject');
      return;
    }

    // Don't overwrite Memory notes with empty content (synthesis manages these)
    if (md.length === 0 && noteMeta?.note_type === 'memory') {
      console.log('[EDITOR-SAVE] Skipping empty save for Memory note (synthesis-managed)');
      return;
    }
    const finalTitle = noteTitle || 'Untitled';
    const fullContent = md;
    console.log('[EDITOR-SAVE] Saving to:', saveProject, '/', saveSlug, 'content length:', fullContent.length);
    // #2972 stage 3 (§34.4): mint and register the tag BEFORE the save
    // starts, same ordering requirement as updateEditorMeta() above — by
    // the time api.saveNote()'s own dispatch fires (inside its async body,
    // after its invoke() resolves), this Editor's handleDataChanged (below)
    // must already be able to recognize the echo as its own.
    const originTag = makeOriginTag();
    registerOwnOriginTag(originTag);
    try {
      await api.saveNote(saveProject, saveSlug, fullContent, finalTitle, originTag);
      console.log('[EDITOR-SAVE] Save SUCCESS');
      // Notify NotesBrowser to refresh card titles (separate event to avoid Editor reload loop)
      if (typeof window !== 'undefined') window.dispatchEvent(new CustomEvent('pane-note-saved'));

    } catch (e) {
      console.error('[EDITOR-SAVE] Save FAILED:', e);
    }
  }

  // Sheet-only title save: NEVER include `content` in the update_note
  // request. Sheets have no TipTap instance (isSheet skip at line ~1665),
  // so any `content` this component could send is stale/regenerated —
  // exactly the write #2901's sheet-body guard (notes.rs:1520-1528)
  // exists to refuse. Sending title-only means the guard is never
  // triggered and the title update always reaches its own branch in
  // update_note (notes.rs:1581+). See #2874 item 17.
  async function saveSheetTitle() {
    if (isDestroyed) return;
    const saveSlug = mountedSlug;
    const saveProject = mountedProject;
    console.log('[EDITOR-SAVE] saveSheetTitle called. slug:', saveSlug, 'currentProject:', saveProject);
    if (!saveProject || !saveSlug) {
      console.warn('[EDITOR-SAVE] saveSheetTitle SKIPPED - missing slug or currentProject');
      return;
    }
    const finalTitle = noteTitle || 'Untitled';
    // #2972 stage 3 (§34.4) — see saveContent()'s identical comment above.
    const originTag = makeOriginTag();
    registerOwnOriginTag(originTag);
    try {
      const result = await api.saveSheetTitle(saveProject, saveSlug, finalTitle, originTag);
      console.log('[EDITOR-SAVE] saveSheetTitle SUCCESS');
      if (typeof window !== 'undefined') window.dispatchEvent(new CustomEvent('pane-note-saved'));
      return result;
    } catch (e) {
      console.error('[EDITOR-SAVE] Sheet title save FAILED:', e);
    }
  }

  // Remove the old autocomplete function since we're using the new WikiLink extension

  // ---------------------------------------------------------------------------
  // Attachment serialization helpers
  // ---------------------------------------------------------------------------

  // ATTACHMENT_DEFAULT_MODES removed (#401 PR5b).
  // Was a stale literal map ('collapsed-card' for audio/video/pdf/html/file/link) that
  // mismatched Attachment.ts:defaultRenderMode() which returns 'card' for those types.
  // The mismatch caused every drag/paste upload to take the non-default-mode path in
  // serializeAttachmentWrapper, emitting a raw-HTML block that dropped data-filename
  // and data-size — so the card showed generic "File / AUDIO" on remount.
  // Now using defaultRenderMode() directly (imported from Attachment.ts) as the
  // single source of truth.

  /**
   * Compute the relative path prefix for Assets/ based on how deep the note is
   * within its project folder.
   *
   * - root note (`Projects/Foo/note.md`) → '' (no prefix)
   * - one folder deep → '../'
   * - two folders deep → '../../'
   *
   * Depth = number of path segments between the project-root folder and the
   * note file.  With `file_path` stored as `Projects/{Project}/{...}/{file}.md`:
   *   segments.length - 3 = depth  (subtract 'Projects', '{Project}', '{file}.md')
   *
   * Falls back to '' (no prefix) when `noteMeta.file_path` is unavailable.
   */
  function computeAttachmentPrefix(): string {
    const filePath = noteMeta?.file_path;
    if (!filePath) return '';
    const parts = filePath.replace(/\\/g, '/').split('/');
    const depth = Math.max(0, parts.length - 3);
    return '../'.repeat(depth);
  }

  /** Extract the file extension (including dot) from a filename, e.g. 'photo.jpg' → '.jpg'. */
  function extOf(filename: string): string {
    const dot = filename.lastIndexOf('.');
    return dot >= 0 ? filename.substring(dot) : '';
  }

  /**
   * Serialize an attachment-wrapper div to markdown or a raw-HTML block.
   *
   * - Default-mode attachments → plain portable markdown using file-relative
   *   `Assets/<uuid>.<ext>` paths (spec §2.1 canonical format).
   * - Non-default-mode attachments → raw-HTML block so render-mode is preserved
   *   in the flat .md file (spec Decision #2 / §2.1A), with relative paths in
   *   any src/href attributes.
   *
   * Reads `data-attachment-id` first; falls back to legacy `data-id`.
   */
  function serializeAttachmentWrapper(el: HTMLElement): string {
    // Read identity / render-mode attributes from the wrapper div (canonical
    // location post-PR1) or from child elements (legacy pre-PR1 HTML).
    const attachmentId =
      el.getAttribute('data-attachment-id') ||
      el.getAttribute('data-id') ||
      el.querySelector('[data-attachment-id]')?.getAttribute('data-attachment-id') ||
      el.querySelector('[data-id]')?.getAttribute('data-id') ||
      null;

    const renderMode = el.getAttribute('data-render-mode') || null;
    const attachmentType = el.getAttribute('data-attachment-type') || null;

    // PR4: read width/height dimension attrs (image resize persistence).
    const widthAttr = el.getAttribute('data-width') || null;
    const heightAttr = el.getAttribute('data-height') || null;
    const hasDimensions = !!(widthAttr || heightAttr);

    // Determine whether this attachment is in its type-specific default mode.
    const isDefaultMode =
      !renderMode ||
      (attachmentType != null && defaultRenderMode(attachmentType) === renderMode);

    const prefix = computeAttachmentPrefix();

    // Emit a raw-HTML block when:
    //  (a) renderMode is non-default (preserves non-default-mode round-trip), OR
    //  (b) the image has explicit dimensions (preserves width/height round-trip — PR4).
    // This is the burn area for #1838-class bugs: every new attr must be emitted here.
    if ((!isDefaultMode || hasDimensions) && attachmentId && attachmentType) {
      // Non-default mode OR image with dimensions — emit raw-HTML block so all
      // data-* attrs survive the round-trip through the flat .md file.
      const img = el.querySelector('img');
      let filenameForExt = '';
      if (img) {
        filenameForExt = img.getAttribute('data-filename') || img.getAttribute('alt') || '';
      } else {
        filenameForExt = el.querySelector('.attachment-filename')?.textContent ||
          el.getAttribute('data-filename') || '';
      }
      const relPath = `${prefix}Assets/${attachmentId}${extOf(filenameForExt)}`;
      let inner = el.innerHTML;
      // Replace attachment://uuid occurrences with the relative path
      inner = inner.replace(new RegExp(`attachment://${attachmentId}`, 'g'), relPath);
      // Build the wrapper opening tag with all persisted attrs.
      // Belt-and-suspenders (#401 PR5b): read filename + size from the wrapper element
      // (populated by Attachment.ts renderHTML) so they survive the raw-HTML round-trip.
      // This is the burn area for #1838-class bugs — see comment above this block.
      const filenameAttr = el.getAttribute('data-filename') || null;
      const sizeAttr = el.getAttribute('data-size') || null;
      return buildAttachmentRawHtmlBlock({
        attachmentId, renderMode, attachmentType,
        filenameAttr, sizeAttr, widthAttr, heightAttr,
        inner,
      });
    }

    // Default mode (no explicit dimensions) — file-relative markdown (spec §2.1).
    if (!attachmentId) return '';

    const img = el.querySelector('img');
    // #3161 PR1 round-trip fix (Reed audit finding 1): TipTap's actual schema-based
    // getHTML() output for the Attachment node is a CHILDLESS wrapper div (see
    // Attachment.ts renderHTML — it emits only the wrapper's own data-* attrs, never
    // an inner <img>; the NodeView's live `.attachment-wrapper`-classed DOM with real
    // children is a separate, editing-only rendering that getHTML() never reads from).
    // So for every real, non-legacy image attachment, `el.querySelector('img')` above
    // is always null and `attachmentType === 'image'` is the only reliable signal —
    // confirmed empirically against a real mounted TipTap Editor + getHTML(), not just
    // read from source. Without this branch, every default-mode image degraded from
    // `![alt](path)` to a plain `[alt](path)` link on the very first save after load.
    if (img || attachmentType === 'image') {
      const source = img || el;
      // Bracket display text: prefer the real caption (`alt`) over `data-filename`.
      // Post-fix, `data-filename` for the canonical Assets/ form is always the
      // machine-oriented `<uuid>.<ext>` (set at parse time — see processInline),
      // never the caption, so it must not clobber a generate_image caption here.
      // File-picker uploads never set `alt` distinctly from `data-filename`, so this
      // falls through to the same `data-filename` value as before for that case —
      // no behavior change there.
      const altText = source.getAttribute('alt') || source.getAttribute('data-filename') || 'image';
      // Matt's ruling: the wrapper's own `src` carries the real extension verbatim from
      // the original Assets/<uuid>.<ext> markdown path (or upload) and is never rewritten
      // by TipTap — use THAT first. data-filename/alt is the fallback, not the source:
      // generate_image's alt is free-text model prose that may contain '.' (garbage ext)
      // or lack an extension entirely. Legacy attachment:// src has no extension, so this
      // correctly falls back to the filename-derived extension for that form.
      const filenameForExt = source.getAttribute('data-filename') || source.getAttribute('alt') || 'image';
      const srcAttr = source.getAttribute('src') || '';
      const ext = extOf(srcAttr) || extOf(filenameForExt);
      const relPath = `${prefix}Assets/${attachmentId}${ext}`;
      return `![${altText}](${relPath})\n\n`;
    }

    const audio = el.querySelector('audio');
    if (audio) {
      const filename = el.querySelector('.attachment-filename')?.textContent || 'audio';
      const relPath = `${prefix}Assets/${attachmentId}${extOf(filename)}`;
      return `[${filename}](${relPath})\n\n`;
    }

    const video = el.querySelector('video');
    if (video) {
      const filename = el.querySelector('.attachment-filename')?.textContent || 'video';
      const relPath = `${prefix}Assets/${attachmentId}${extOf(filename)}`;
      return `[${filename}](${relPath})\n\n`;
    }

    // File / PDF / HTML / Link (card types)
    const filename = el.querySelector('.attachment-filename')?.textContent ||
      el.getAttribute('data-filename') || 'file';
    const relPath = `${prefix}Assets/${attachmentId}${extOf(filename)}`;
    return `[${filename}](${relPath})\n\n`;
  }

  // Convert HTML content to markdown-ish for saving
  function htmlToMarkdown(html: string): string {
    if (!editor) return '';
    // Use the editor's text content with some basic conversion
    const div = document.createElement('div');
    div.innerHTML = html;
    return convertNodeToMarkdown(div).trim();
  }

  function convertNodeToMarkdown(node: Node): string {
    let result = '';

    for (const child of Array.from(node.childNodes)) {
      if (child.nodeType === Node.TEXT_NODE) {
        result += child.textContent || '';
        continue;
      }

      if (child.nodeType !== Node.ELEMENT_NODE) continue;

      const el = child as HTMLElement;
      const tag = el.tagName.toLowerCase();

      switch (tag) {
        case 'h1':
          result += `# ${el.textContent}\n\n`;
          break;
        case 'h2':
          result += `## ${el.textContent}\n\n`;
          break;
        case 'h3':
          result += `### ${el.textContent}\n\n`;
          break;
        case 'h4':
          result += `#### ${el.textContent}\n\n`;
          break;
        case 'h5':
          result += `##### ${el.textContent}\n\n`;
          break;
        case 'h6':
          result += `###### ${el.textContent}\n\n`;
          break;
        case 'p': {
          // Preserve <br> as actual newlines within the paragraph
          let pContent = '';
          for (const pChild of Array.from(el.childNodes)) {
            if (pChild.nodeType === Node.ELEMENT_NODE && (pChild as HTMLElement).tagName.toLowerCase() === 'br') {
              pContent += '\n';
            } else if (pChild.nodeType === Node.TEXT_NODE) {
              pContent += pChild.textContent || '';
            } else if (pChild.nodeType === Node.ELEMENT_NODE) {
              pContent += convertInlineToMarkdown(pChild as Element);
            }
          }
          // Only add the double newline if this paragraph has actual content
          if (pContent.trim()) {
            result += pContent + '\n\n';
          }
          break;
        }
        case 'br':
          result += '\n';
          break;
        case 'blockquote':
          const bqContent = convertNodeToMarkdown(el).trim();
          result += bqContent.split('\n').map((l: string) => `> ${l}`).join('\n') + '\n\n';
          break;
        case 'ul':
          for (const li of Array.from(el.children)) {
            if (li.getAttribute('data-type') === 'taskItem') {
              const checked = li.getAttribute('data-checked') === 'true';
              result += `- [${checked ? 'x' : ' '}] ${li.textContent}\n`;
            } else {
              result += `- ${convertInlineToMarkdown(li)}\n`;
            }
          }
          result += '\n';
          break;
        case 'ol':
          let idx = 1;
          for (const li of Array.from(el.children)) {
            result += `${idx}. ${convertInlineToMarkdown(li)}\n`;
            idx++;
          }
          result += '\n';
          break;
        case 'pre':
          const code = el.querySelector('code');
          const lang = code?.className?.replace('language-', '') || '';
          result += `\`\`\`${lang}\n${code?.textContent || el.textContent}\n\`\`\`\n\n`;
          break;
        case 'hr':
          result += '---\n\n';
          break;
        case 'table':
          result += convertTableToMarkdown(el) + '\n\n';
          break;
        case 'div':
          // Generic data-component block serializer (fixes #2111).
          // Any <div data-component="..."> is stored as a raw-HTML block so custom
          // node types (checklist, due-today, etc.) survive the round-trip without
          // needing per-type handling here. The raw block is wrapped in blank lines
          // so markdownToHtml's block splitter keeps it as its own block, and the
          // existing C5 guard in markdownToHtml already passes <div…> blocks through
          // without wrapping them in <p>. TipTap parseHTML rules on each extension
          // (e.g. Checklist.ts:58-64) then reconstruct the node on reload.
          // IMPORTANT: must run BEFORE the attachment check so data-attachment divs
          // that also happen to carry data-component still hit serializeAttachmentWrapper.
          if (el.hasAttribute('data-component') && !el.hasAttribute('data-attachment') && !el.classList.contains('attachment-wrapper')) {
            result += '\n' + el.outerHTML + '\n\n';
            break;
          }
          // Handle attachment wrappers.
          // Check for data-attachment attribute first (PR #1834 renderHTML emits this
          // without the .attachment-wrapper class), then fall back to the class check
          // for backward-compat with older markup. Both forms route to the same
          // serializeAttachmentWrapper() which reads data-* attrs for all paths.
          // Fixes #1838: attachments were silently dropped on re-serialization because
          // getHTML() uses schema-based serialization (renderHTML) which emits
          // data-attachment but NOT .attachment-wrapper.
          if (el.hasAttribute('data-attachment') || el.classList.contains('attachment-wrapper')) {
            result += serializeAttachmentWrapper(el);
            break;
          }
          // Fall through for non-attachment divs
          result += convertNodeToMarkdown(el);
          break;
        case 'img': {
          // Handle standalone images (in case attachment wrapper is missing)
          // Accept both new data-attachment-id and legacy data-id for backward compat.
          const attachmentId = el.getAttribute('data-attachment-id') || el.getAttribute('data-id');
          const alt = el.getAttribute('alt') || 'image';
          if (attachmentId) {
            // Use same canonical relative path as serializeAttachmentWrapper:
            // computeAttachmentPrefix() + Assets/<uuid>.<ext>
            const prefix = computeAttachmentPrefix();
            const filename = el.getAttribute('data-filename') || alt;
            // #3161 PR1 round-trip fix: prefer the img's own `src` extension over
            // data-filename/alt — see matching fix + rationale in serializeAttachmentWrapper.
            const srcAttr = el.getAttribute('src') || '';
            const ext = extOf(srcAttr) || extOf(filename);
            const relPath = `${prefix}Assets/${attachmentId}${ext}`;
            result += `![${alt}](${relPath})\n\n`;
          } else {
            // External image with URL src
            const src = el.getAttribute('src') || '';
            result += `![${alt}](${src})\n\n`;
          }
          break;
        }
        default:
          result += convertNodeToMarkdown(el);
      }
    }

    return result;
  }

  // #2419 round-2 fix (Finding 1, launch-blocking): convertInlineToMarkdown used to
  // ONLY iterate el's children and test each CHILD against the mark cases below —
  // it never tested `el` itself. Call sites at convertNodeToMarkdown's <p> case
  // (line ~960) and the <ul>/<ol> <li> cases (~973/~981) pass the inline-mark
  // element ITSELF (e.g. a wiki-link <span>, a <strong>) as `el` when it is a
  // DIRECT child of <p>/<li> — which is exactly what real TipTap output looks
  // like (`<p>This is <strong>bold</strong> text.</p>`). Because the old code
  // only tested el's children, not el, the mark was silently dropped to plain
  // text on every save. Fixed by testing `el`'s own tag/attributes FIRST
  // (mirroring convertNodeToMarkdown's own top-level dispatch pattern just
  // above); only when `el` itself is NOT a recognized mark do we fall through
  // to iterating its children via convertInlineChildren. This keeps the
  // existing call sites (960/973/981, which may pass either a mark element or
  // a plain container like <li>) working for both shapes without touching
  // convertNodeToMarkdown itself.
  function convertInlineToMarkdown(el: Element): string {
    const tag = el.tagName.toLowerCase();

    switch (tag) {
      case 'wiki-link': {
        const wikiTitle = el.getAttribute('data-title') || el.textContent || '';
        return `[[${wikiTitle}]]`;
      }
      case 'span':
        if (el.hasAttribute('data-wiki-link') || el.hasAttribute('data-title')) {
          const wikiTitle = el.getAttribute('data-title') || el.textContent || '';
          return `[[${wikiTitle}]]`;
        }
        // Not a wiki-link span — fall through to generic child iteration below.
        break;
      case 'strong':
      case 'b':
        return `**${convertInlineChildren(el)}**`;
      case 'em':
      case 'i':
        return `*${convertInlineChildren(el)}*`;
      case 'code':
        return `\`${el.textContent}\``;
      case 'a':
        return `[${el.textContent}](${el.getAttribute('href') || ''})`;
      case 's':
        return `~~${convertInlineChildren(el)}~~`;
      case 'u':
        // No dedicated markdown syntax for underline in this dialect — passthrough,
        // same behavior as before this fix (underline text is preserved, marker isn't).
        return convertInlineChildren(el);
      default:
        break;
    }

    // el is a plain container (e.g. <li>, a non-wiki-link <span>, or any
    // unrecognized wrapper) — iterate its children instead of treating el
    // itself as a mark.
    return convertInlineChildren(el);
  }

  // Iterates el's children, converting text nodes verbatim and recursively
  // testing each ELEMENT child against the mark cases via convertInlineToMarkdown.
  function convertInlineChildren(el: Element): string {
    let result = '';
    for (const child of Array.from(el.childNodes)) {
      if (child.nodeType === Node.TEXT_NODE) {
        result += child.textContent || '';
        continue;
      }
      if (child.nodeType !== Node.ELEMENT_NODE) continue;
      result += convertInlineToMarkdown(child as Element);
    }
    return result;
  }

  function convertTableToMarkdown(table: Element): string {
    const rows = Array.from(table.querySelectorAll('tr'));
    if (rows.length === 0) return '';

    let result = '';
    rows.forEach((row, i) => {
      const cells = Array.from(row.querySelectorAll('th, td'));
      result += '| ' + cells.map(c => c.textContent || '').join(' | ') + ' |\n';
      if (i === 0) {
        result += '| ' + cells.map(() => '---').join(' | ') + ' |\n';
      }
    });
    return result;
  }

  // Convert markdown to HTML for loading into editor
  function markdownToHtml(md: string): string {
    let html = md;

    // Code blocks first (before other processing)
    html = html.replace(/```(\w*)\n([\s\S]*?)```/g, (_, lang, code) => {
      return `<pre><code class="language-${lang}">${escapeHtml(code.trimEnd())}</code></pre>`;
    });

    // Split into blocks (but preserve code blocks)
    const blocks: string[] = [];
    let current = '';
    let inCodeBlock = false;

    for (const line of html.split('\n')) {
      if (line.startsWith('<pre>')) {
        if (current.trim()) blocks.push(current.trim());
        current = '';
        inCodeBlock = true;
        current += line + '\n';
        continue;
      }
      if (line.includes('</pre>')) {
        current += line + '\n';
        blocks.push(current.trim());
        current = '';
        inCodeBlock = false;
        continue;
      }
      if (inCodeBlock) {
        current += line + '\n';
        continue;
      }
      if (line.trim() === '') {
        if (current.trim()) blocks.push(current.trim());
        current = '';
      } else {
        current += line + '\n';
      }
    }
    if (current.trim()) blocks.push(current.trim());

    // Process each block
    html = blocks.map(block => {
      if (block.startsWith('<pre>')) return block;

      // Headers
      if (block.match(/^#{1,6} /)) {
        const level = block.match(/^(#{1,6}) /)![1].length;
        const text = block.replace(/^#{1,6} /, '');
        return `<h${level}>${processInline(text.trim())}</h${level}>`;
      }

      // Horizontal rule
      if (block.match(/^(-{3,}|\*{3,}|_{3,})$/)) {
        return '<hr>';
      }

      // Blockquote
      if (block.startsWith('> ')) {
        const content = block.split('\n').map(l => l.replace(/^> ?/, '')).join('\n');
        return `<blockquote><p>${processInline(content)}</p></blockquote>`;
      }

      // Task list
      if (block.match(/^- \[[ x]\] /m)) {
        const items = block.split('\n').filter(l => l.trim());
        const lis = items.map(item => {
          const checked = item.match(/^- \[x\] /i);
          const text = item.replace(/^- \[[ x]\] /i, '');
          return `<li data-type="taskItem" data-checked="${checked ? 'true' : 'false'}"><label><input type="checkbox" ${checked ? 'checked' : ''}><span></span></label><div><p>${processInline(text)}</p></div></li>`;
        });
        return `<ul data-type="taskList">${lis.join('')}</ul>`;
      }

      // Unordered list
      if (block.match(/^[-*+] /m)) {
        const items = block.split('\n').filter(l => l.trim());
        const lis = items.map(item => {
          const text = item.replace(/^[-*+] /, '');
          return `<li><p>${processInline(text)}</p></li>`;
        });
        return `<ul>${lis.join('')}</ul>`;
      }

      // Ordered list
      if (block.match(/^\d+\. /m)) {
        const items = block.split('\n').filter(l => l.trim());
        const lis = items.map(item => {
          const text = item.replace(/^\d+\. /, '');
          return `<li><p>${processInline(text)}</p></li>`;
        });
        return `<ol>${lis.join('')}</ol>`;
      }

      // Table
      if (block.includes('|')) {
        const rows = block.split('\n').filter(r => r.trim() && !r.match(/^\|[\s-|]+\|$/));
        if (rows.length >= 1) {
          const headerCells = rows[0].split('|').filter(c => c.trim()).map(c => `<th>${c.trim()}</th>`);
          let tableHtml = `<table><tr>${headerCells.join('')}</tr>`;
          for (let i = 1; i < rows.length; i++) {
            const cells = rows[i].split('|').filter(c => c.trim()).map(c => `<td>${c.trim()}</td>`);
            tableHtml += `<tr>${cells.join('')}</tr>`;
          }
          tableHtml += '</table>';
          return tableHtml;
        }
      }

      // Paragraph — preserve single line breaks as <br>
      const lines = block.split('\n');
      if (lines.length > 1) {
        const processedLines = lines
          .map(l => l.trim())
          .filter(l => l.length > 0)
          .map(l => processInline(l));
        const joined = processedLines.join('<br>');
        // C5: never wrap a block-level element in <p> — that's invalid HTML and causes
        // browsers to reparent the <div>, producing stray empty <p></p> before attachments.
        // Safe for inline content: <strong>, <em>, <a>, <code> etc. don't start with <div|blockquote|pre|ul|ol|table.
        const startsWithBlock = /^<(div|blockquote|pre|ul|ol|table)\b/i.test(joined.trimStart());
        return startsWithBlock ? joined : `<p>${joined}</p>`;
      }
      const processed = processInline(block.trim());
      if (!processed) return '';
      // C5: same guard for single-line paragraphs that processInline resolved to a block element.
      const isBlockElement = /^<(div|blockquote|pre|ul|ol|table)\b/i.test(processed.trimStart());
      return isBlockElement ? processed : `<p>${processed}</p>`;
    }).join('');

    return html;
  }

  function escapeHtml(text: string): string {
    return text
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#039;');
  }

  function processInline(text: string): string {
    // Bold
    text = text.replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>');
    text = text.replace(/__(.+?)__/g, '<strong>$1</strong>');
    // Italic
    text = text.replace(/\*(.+?)\*/g, '<em>$1</em>');
    text = text.replace(/_(.+?)_/g, '<em>$1</em>');
    // Strikethrough
    text = text.replace(/~~(.+?)~~/g, '<s>$1</s>');
    // Inline code
    text = text.replace(/`(.+?)`/g, '<code>$1</code>');
    // Wiki links (before regular links to avoid conflict)
    text = text.replace(/\[\[(.+?)\]\]/g, '<span data-wiki-link="true" class="wiki-link" data-title="$1">$1</span>');
    // Images (before links to avoid conflict with ![...](url) syntax)
    // Parses both legacy `attachment://uuid` form and canonical `Assets/<uuid>.<ext>` form.
    text = text.replace(/!\[([^\]]*)\]\(([^)]+)\)/g, (match, alt, src) => {
      // Legacy attachment:// form
      if (src.startsWith('attachment://')) {
        const attachmentId = src.replace('attachment://', '');
        const safeAlt = escapeHtml(alt);
        const safeSrc = escapeHtml(src);
        const safeId = escapeHtml(attachmentId);
        return `<img src="${safeSrc}" alt="${safeAlt}" data-attachment="" data-attachment-id="${safeId}" data-attachment-type="image" data-filename="${safeAlt}">`;
      }
      // Canonical relative Assets/ form: Assets/<uuid>.<ext> or ../Assets/<uuid>.<ext> etc.
      const assetsMatch = src.match(/^(?:\.\.\/)*Assets\/([^./]+)(\.[^)]+)$/);
      if (assetsMatch) {
        const attachmentId = assetsMatch[1];
        const ext = assetsMatch[2]; // e.g. '.jpg'
        const safeAlt = escapeHtml(alt);
        const safeSrc = escapeHtml(src);
        const safeId = escapeHtml(attachmentId);
        // #3161 PR1 round-trip fix (Reed audit finding 1, Matt's ruling): data-filename
        // must always be self-describing (<uuid>.<ext>, derived from the src path's own
        // extension), NEVER derived from `alt`. `alt` is free-text caption prose (e.g.
        // generate_image's model-authored prompt/caption) — it may contain '.' (producing
        // a garbage extension) or no '.' at all (producing none). The src path's own `ext`
        // capture is the authoritative source; data-filename/alt become a fallback used
        // only where no extension-bearing src exists (e.g. the legacy attachment:// form
        // above, which has no ext to borrow from). This mirrors the matching fix in
        // serializeAttachmentWrapper/convertNodeToMarkdown below, which also prefers the
        // wrapper's own `src` attribute over data-filename/alt when recovering the
        // extension on save.
        const safeFilename = escapeHtml(attachmentId + ext);
        // W-1: stamp data-attachment + data-attachment-type so TipTap parseHTML restores
        // attachmentType='image' on reload. Without this, renderMode stays null AND
        // attachmentType is null → NodeView falls to card mode for non-resized images.
        return `<img src="${safeSrc}" alt="${safeAlt}" data-attachment="" data-attachment-id="${safeId}" data-attachment-type="image" data-filename="${safeFilename}">`;
      }
      const safeAlt = escapeHtml(alt);
      const safeSrc = escapeHtml(src);
      return `<img src="${safeSrc}" alt="${safeAlt}">`;
    });
    // Attachment links (audio, video, files) - BEFORE regular links to avoid conflict.
    // Parses both legacy `attachment://uuid` and canonical `Assets/<uuid>.<ext>` forms.
    // attachment:// form
    text = text.replace(/\[([^\]]*)\]\(attachment:\/\/([^)]+)\)/g, (match, filename, attachmentId) => {
      const safeName = escapeHtml(filename);
      const safeId = escapeHtml(attachmentId);
      // Determine type from extension
      const ext = filename.split('.').pop()?.toLowerCase() || '';
      const audioExts = ['mp3', 'wav', 'ogg', 'flac', 'aac', 'm4a'];
      const videoExts = ['mp4', 'webm', 'mov', 'avi'];

      if (audioExts.includes(ext)) {
        // C3: stamp data-attachment sentinel + data-filename + data-attachment-type so TipTap
        // parseHTML reconstructs the node with correct attrs on reload (#1801 fix).
        return `<div class="attachment-wrapper" data-attachment="" data-attachment-id="${safeId}" data-filename="${safeName}" data-attachment-type="audio"><div class="attachment-filename">${safeName}</div><audio data-attachment-id="${safeId}" controls></audio></div>`;
      } else if (videoExts.includes(ext)) {
        return `<div class="attachment-wrapper" data-attachment="" data-attachment-id="${safeId}" data-filename="${safeName}" data-attachment-type="video"><div class="attachment-filename">${safeName}</div><video data-attachment-id="${safeId}" controls></video></div>`;
      } else {
        const imageExts = ['jpg', 'jpeg', 'png', 'gif', 'webp', 'svg'];
        const attachType = ext === 'pdf' ? 'pdf' : imageExts.includes(ext) ? 'image' : 'file';
        return `<div class="attachment-wrapper" data-attachment="" data-attachment-id="${safeId}" data-filename="${safeName}" data-attachment-type="${attachType}"><div class="attachment-card"><span class="attachment-filename">${safeName}</span><a class="attachment-download" data-attachment-id="${safeId}">Download</a></div></div>`;
      }
    });
    // Canonical relative Assets/ form: [filename](Assets/<uuid>.<ext>) or [filename](../Assets/...)
    text = text.replace(/\[([^\]]*)\]\((?:\.\.\/)*Assets\/([^./]+)(\.[^)]+)\)/g, (match, filename, attachmentId, ext) => {
      const safeName = escapeHtml(filename);
      const safeId = escapeHtml(attachmentId);
      const extLower = ext.toLowerCase().slice(1); // strip leading dot
      const audioExts = ['mp3', 'wav', 'ogg', 'flac', 'aac', 'm4a'];
      const videoExts = ['mp4', 'webm', 'mov', 'avi'];

      if (audioExts.includes(extLower)) {
        // C3: stamp data-attachment sentinel + data-filename + data-attachment-type on wrapper div
        // so the TipTap node reconstructed from this HTML knows its type on reload (#1801 fix).
        return `<div class="attachment-wrapper" data-attachment="" data-attachment-id="${safeId}" data-filename="${safeName}" data-attachment-type="audio"><div class="attachment-filename">${safeName}</div><audio data-attachment-id="${safeId}" controls></audio></div>`;
      } else if (videoExts.includes(extLower)) {
        return `<div class="attachment-wrapper" data-attachment="" data-attachment-id="${safeId}" data-filename="${safeName}" data-attachment-type="video"><div class="attachment-filename">${safeName}</div><video data-attachment-id="${safeId}" controls></video></div>`;
      } else {
        const imageExts = ['jpg', 'jpeg', 'png', 'gif', 'webp', 'svg'];
        const attachType = extLower === 'pdf' ? 'pdf' : imageExts.includes(extLower) ? 'image' : 'file';
        return `<div class="attachment-wrapper" data-attachment="" data-attachment-id="${safeId}" data-filename="${safeName}" data-attachment-type="${attachType}"><div class="attachment-card"><span class="attachment-filename">${safeName}</span><a class="attachment-download" data-attachment-id="${safeId}">Download</a></div></div>`;
      }
    });
    // Links (regular URLs)
    text = text.replace(/\[(.+?)\]\((.+?)\)/g, (match, linkText, href) => {
      const safeText = escapeHtml(linkText);
      const safeHref = escapeHtml(href);
      return `<a href="${safeHref}">${safeText}</a>`;
    });
    return text;
  }

  export function setContent(markdown: string) {
    if (editor) {
      const html = markdownToHtml(markdown);
      isSettingContent = true;
      editor.commands.setContent(html);
      isSettingContent = false;
    }
  }

  export function getMarkdown(): string {
    if (!editor) return '';
    return htmlToMarkdown(editor.getHTML());
  }

  export function focus() {
    editor?.commands.focus();
  }

  export function setTitle(newTitle: string) {
    noteTitle = newTitle;
  }

  export function getTitle(): string {
    return noteTitle;
  }

  let titleFocused = false;

  function handleTitleChange() {
    titleEditedByUser = true;
    dirty = true; // Title change should trigger save on destroy
    if (onTitleUpdate) {
      onTitleUpdate(noteTitle);
    }
    // #2972 stage 3 (item 2): the old per-keystroke `pane-task-meta-changed`
    // dispatch here is deleted, not migrated. It fired on every `oninput`
    // tick with the LIVE, not-yet-saved typed value — title isn't actually
    // persisted until blur/destroy (see the title <input>'s onblur handler
    // and onDestroy below). Dispatching the new entity-identified event
    // here would be actively wrong: every sibling screen's Stage 2 handler
    // reacts to an "edited" event by re-reading the note from the backend
    // (api.readNote()/entityService.getNote()), which would still return
    // the OLD, not-yet-saved title — one or more DB reads per keystroke for
    // a value that doesn't exist in the DB yet. The real save (blur/
    // destroy -> saveContent()/saveSheetTitle() -> api.saveNote()/
    // api.saveSheetTitle(), both migrated, §34.1.2) already dispatches the
    // correct event once the title is actually persisted — nothing is lost
    // except the live, mid-keystroke preview on other screens, which this
    // event was never a safe mechanism for in the first place.
  }

  // Track if user has edited the title locally
  let titleEditedByUser = false;

  // Update title when prop changes (only on initial load or external changes, not during user editing)
  $effect(() => {
    const newTitle = title || '';
    // Never update while focused - prevents cursor position reset
    if (!titleEditedByUser && !titleFocused && newTitle !== noteTitle) {
      noteTitle = newTitle;
    }
  });

  // Toggle editor editable state when read-only status changes
  //
  // #2950: TipTap's setEditable(editable, emitUpdate = true) unconditionally
  // emits an 'update' event (with an EMPTY transaction — zero steps; this
  // doesn't go through dispatchTransaction/docChanged/preventUpdate at all,
  // it's a direct this.emit('update', ...) inside setEditable itself)
  // whenever emitUpdate is left at its default. This $effect runs on every
  // Editor mount (as soon as `editor` is first assigned), and because that
  // can happen after loadInitialContent's isSettingContent guard has already
  // reset to false, it was tripping onUpdate -> dirty = true -> a spurious
  // debounced save + pane-data-changed dispatch on EVERY note opened,
  // independent of attachments. Toggling editable state is never itself a
  // content change, so it should never mark the editor dirty — pass
  // emitUpdate: false to suppress TipTap's built-in (and here, incorrect)
  // assumption that editable-state changes are user edits.
  $effect(() => {
    if (editor) {
      editor.setEditable(!isReadOnly, false);
    }
  });

  // Watch for late-arriving content (e.g., async load after card click)
  $effect(() => {
    const note = $activeNote;
    if (!note || !editor || !mountedSlug) return;
    if (note.meta?.id !== mountedSlug && note.meta?.slug !== mountedSlug) return;

    const newContent = note.content;
    if (!newContent || newContent.trim() === '') return;

    // Only update if editor is currently empty and new content arrived
    const currentContent = editor.getHTML();
    const editorIsEmpty = !currentContent || currentContent === '<p></p>' || currentContent.trim() === '';

    if (editorIsEmpty && newContent.length > 1) {
      console.log('[EDITOR] Late content arrived, updating editor. Length:', newContent.length);
      // Parse markdown to HTML; NodeView handles attachment display resolution.
      const html = markdownToHtml(newContent);
      isSettingContent = true;
      editor.commands.setContent(html);
      console.log('[EDITOR-RELOAD] Set content in editor, html length:', html.length);
      isSettingContent = false;
      dirty = false; // Don't mark as dirty - this is a load, not an edit
    }
  });

  // Content loading is handled in onMount for initial load.
  // The {#key} block ensures a fresh Editor per note when note ID changes.
  // The $effect above handles the race condition when content arrives late (after card click).


  onMount(() => {
    // CRITICAL: Snapshot slug and notebook as plain strings at mount time.
    // These NEVER change for this Editor instance, even if $activeNote changes.
    mountedSlug = slug;
    mountedProject = currentProject;
    console.log('[EDITOR-MOUNT] Captured slug:', mountedSlug, 'project:', mountedProject);
    loadLinkedTasks();
    loadPinState();

    // Auto-focus and select title for new notes
    if ((!title || title === 'Untitled') && titleInput) {
      // Use tick to let DOM settle, then focus+select
      setTimeout(() => {
        if (titleInput && (noteTitle === '' || noteTitle === 'Untitled')) {
          if (!noteTitle) noteTitle = 'Untitled';
          titleFocused = true;
          titleInput.focus();
          titleInput.select();
        }
      }, 50);
    }

    // #2339 PR9 / #3053 item 1 (docs/audits/2026-10-20-scope-1061-fixes.md,
    // D1/D2 part 1): sheets have no TipTap instance at all — `element`
    // isn't bound to a rendered DOM node when isSheet is true (the
    // editor-container div is swapped for <SheetGrid> in the template
    // below), so constructing a TipTap Editor here would throw. The guard
    // used to be `if (isSheet) { return () => {}; }` — an early return out
    // of the WHOLE `onMount` body, which also skipped every listener
    // registration below this point (`pane-reload-editor`,
    // `subscribeDataChanged`, font-size, click-outside, pill-close,
    // keydown), not just the TipTap construction it was written to guard.
    // That was never a deliberate exclusion of sheets from refresh events
    // (see `git log -S` on this guard, 0bb4ea7 vs. #2972's later addition
    // of `subscribeDataChanged` — it landed below an early return that
    // already existed for an unrelated reason). Narrowed to wrap ONLY the
    // TipTap-construction block + its initial-content load — every
    // listener registration from here down now runs for sheets too, same
    // as plain notes; each one already internally guards on
    // `if (editor)`/`if (content && editor)` (see the comment on
    // `onDestroy`'s own isSheet branch below for the same established
    // pattern), so running them for a sheet is a harmless no-op except for
    // the one that matters: `subscribeDataChanged`, which a sheet now
    // actually reaches.
    if (!isSheet) {
      editor = new Editor({
        element: element,
        editable: !isReadOnly,
        extensions: [
          StarterKit.configure({
            codeBlock: false,
            heading: { levels: [1, 2, 3, 4, 5, 6] },
            link: false, // Disable StarterKit's link to avoid duplicate with explicit Link extension
            hardBreak: {
              keepMarks: false,
              HTMLAttributes: {
                'data-preserve-breaks': 'true',
              },
            },
          }),
          Placeholder.configure({ placeholder }),
          TaskList,
          TaskItem.configure({ nested: true }),
          Link.configure({
            openOnClick: false,
            HTMLAttributes: { class: 'editor-link' },
          }),
          CodeBlockLowlight.configure({ lowlight }),
          Table.configure({ resizable: true }),
          TableRow,
          TableCell,
          TableHeader,
          Typography,
          Underline,
          TextStyle,
          FontSize,
          WikiLink.configure({
            onNavigate: handleNoteNavigation,
            getCurrentProject: () => currentProject,
          }),
          Attachment.configure({
            onUpload: handleFileUpload,
            onUploadError: showErrorToast,
          }),
          Checklist.configure({}),
          DueToday.configure({}),
          DiffHighlight,
        ],
        content: '', // Content will be set by $effect
        editorProps: {
          attributes: {
            class: 'pane-editor',
            spellcheck: 'true',
          },
          handleKeyDown: (view, event) => {
            // Handle Cmd+Shift+T for templates
            if ((event.metaKey || event.ctrlKey) && event.shiftKey && event.key === 'T') {
              window.dispatchEvent(new CustomEvent('pane-show-templates'));
              return true; // Prevent TipTap from handling it
            }
            return false; // Let TipTap handle other keys
          },
        },
        onUpdate: ({ editor: e }) => {
          // Skip updates triggered by setContent (loading content)
          if (isSettingContent) return;

          // Mark as dirty - user has made changes
          dirty = true;

          // Debounce the expensive markdown conversion + save
          clearTimeout(editorUpdateTimeout);
          editorUpdateTimeout = setTimeout(async () => {
            const hasValidTitle = noteTitle && noteTitle !== 'Untitled';
            const html = e.getHTML();
            const md = htmlToMarkdown(html);
            const hasBodyContent = md.trim().length > 0;

            if (hasValidTitle || hasBodyContent) {
              isInternalUpdate = true;

              // Cache latest content - onDestroy uses this if editor DOM is detached
              lastMd = md;

              // Editor saves directly - guaranteed persistence
              await saveContent(md);

              // OPTIONAL: notify parent of content changes (for store updates, etc.)
              if (onUpdate) {
                onUpdate(md);
              }

              // #2972 stage 3 (item 2): duplicate dispatch deleted. The
              // `pane-task-meta-changed` {field: 'preview', value} dispatch
              // that used to fire here is now redundant \u2014 `saveContent()`
              // above (via the now-migrated `api.saveNote()`, \u00a734.1.2)
              // already dispatches the single data-changed event for this
              // exact save. Every sibling screen's Stage 2 handler reacts to
              // that one event with a full single-entity read-and-patch
              // (picking up the just-persisted preview from the backend),
              // superseding the old hand-carried preview-string shortcut.

              // Move autoSync AFTER save completes to prevent concurrent operations
              autoSync();

              if (noteId) {
                clearTimeout(linkRebuildTimeout);
                linkRebuildTimeout = setTimeout(async () => {
                  try {
                    await api.rebuildNoteLinks(noteId);
                  } catch (error) {
                    console.error('Failed to rebuild note links:', error);
                  }
                }, 5000);
              }
            }
          }, 500); // Reduced from 2000ms - save quickly to prevent data loss on navigate
        },
        onFocus: () => { isFocused = true; },
        onBlur: () => { isFocused = false; },
      });

      // Rebind editor to element after creation (Svelte 5 + TipTap)
      editor.on('transaction', () => {
        editor = editor;
      });

      // Load initial content (ONE TIME, on mount only - no reactive $effect)
      const loadInitialContent = () => {
        console.log('[EDITOR-MOUNT] Content prop length:', content?.length, 'first 50 chars:', content?.substring(0, 50));

        if (jsonContent) {
          try {
            const json = JSON.parse(jsonContent);
            isSettingContent = true;
            editor?.commands.setContent(json);
            isSettingContent = false;
            console.log('[EDITOR-MOUNT] Loaded JSON content, length:', JSON.stringify(json).length);
            return;
          } catch (e) {
            console.warn('[EDITOR-MOUNT] Failed to parse JSON content:', e);
            // Fall through to markdown path
          }
        }
        if (content && editor) {
          console.log('[EDITOR-MOUNT] Loaded markdown content, length:', content.length);
          applyMarkdownContent(content, 'EDITOR-MOUNT', () => {
            editorPreEditText.set(editor!.state.doc.textContent);
          });
        } else {
          console.log('[EDITOR-MOUNT] No content to load');
        }
      };

      // Load content synchronously on mount (A7: remove setTimeout - served no purpose and creates window for stale content)
      loadInitialContent();
    }


    // Listen for AI-triggered editor reloads
    const handleReload = async (event?: Event) => {
      // GUARD (#2240): Early-exit before any store or await when the panel was
      // intentionally closed. Ensures we fail-safe by policy, not by accident.
      if (!get(activeNote)) {
        if (DEBUG) console.log('[PANE-DEBUG] handleReload SKIPPED — $activeNote is null (intentional close)');
        return;
      }
      const eventOldText = (event as CustomEvent)?.detail?.oldText;
      console.log('[EDITOR-RELOAD] pane-reload-editor received. noteId:', noteId, 'currentProject:', currentProject, 'editor exists:', !!editor, 'eventOldText:', eventOldText?.length);
      if (noteId && currentProject && editor) {
        try {
          const noteData = await api.readNote(currentProject, noteId);
          console.log('[EDITOR-RELOAD] readNote result:', noteData?.content?.length, 'chars. currentProject:', currentProject, 'noteId:', noteId);
          if (noteData) {
            // Use pre-edit text from store (captured before AI edit)
            const oldText = eventOldText || $editorPreEditText || editor.state.doc.textContent;

            const content = noteData.content || '';
            const html = markdownToHtml(content);
            isSettingContent = true;
            // NodeView handles attachment display resolution; set content directly.
            editor.commands.setContent(html);
            console.log('[EDITOR-RELOAD] Set content in editor, html length:', html.length);
            isSettingContent = false;
            dirty = false; // Content reloaded from disk after AI edit
            // Update store with current text for next diff
            editorPreEditText.set(editor.state.doc.textContent);

            // Apply diff highlight decorations
            try {
              console.log('[DIFF] oldText length:', oldText.length, 'newText length:', editor.state.doc.textContent.length);
              applyDiffHighlight(editor, oldText);
              console.log('[DIFF] Applied diff highlight');
            } catch (diffErr) {
              console.error('[DIFF] Diff highlight failed:', diffErr);
            }


            // Reload metadata (priority, due date, status) after AI changes.
            // GUARD (#2237, class fix): same resurrection protection as handleDataChanged.
            // An AI reload event could race with an intentional close — never repopulate
            // $activeNote from null.
            if (noteData.meta) {
              const currentActive = get(activeNote);
              if (!currentActive || currentActive.meta?.id !== noteId) {
                if (DEBUG) console.log(`[PANE-DEBUG] handleReload[reload-meta] SKIPPED — $activeNote is ${currentActive ? 'mismatched' : 'null'} (intentional close or note switched)`);
              } else {
                logAnSet('Editor.handleAiEdit[reload-meta]', noteData);
                activeNote.set(noteData);
                console.log('[EDITOR-RELOAD] Reloaded note metadata after AI edit');
              }
            }
          }
        } catch (e) {
          console.error('[EDITOR] Failed to reload after AI edit:', e);
        }
      }
    };

    // Listen for data changes that affect task metadata.
    // #2972 stage 3 (§34.4): `payload.entity_id` gates this to ONLY this
    // Editor's own open note. `origin_tag` is then checked against this
    // instance's own in-flight saves (updateEditorMeta/saveContent/
    // saveSheetTitle all register a tag before dispatching, above) —
    // §34.4's first rule: "a listener that is the thing that just saved...
    // skips." The check is non-destructive (isOwnOriginTag), so a second,
    // independent change to the SAME note (an agent edit, a different
    // window) with a DIFFERENT tag always falls through and refreshes —
    // §34.4's second rule, required to be proven in the same test as the
    // first (not broader than the exact save call that produced the tag).
    const handleDataChanged = async (payload: DataChangedPayload) => {
      const { entity_id: entityId, origin_tag: originTag } = payload;
      if (entityId !== noteId) return;
      if (isOwnOriginTag(originTag)) {
        if (DEBUG) console.log('[PANE-DEBUG] handleDataChanged SKIPPED — own echo (§34.4)', { noteId, originTag });
        return;
      }
      // GUARD (#2240): Early-exit before the await when the panel was intentionally
      // closed. The prior fix (#2237) placed the guard *after* api.readNote(), so a
      // crash on null.meta inside the async body could swallow the guard entirely.
      // Rule: refresh handlers no-op on empty state — never repopulate a deliberately
      // cleared store. Belt: this pre-await check. Suspenders: the post-await check below.
      if (!get(activeNote)) {
        if (DEBUG) console.log('[PANE-DEBUG] handleDataChanged SKIPPED — $activeNote is null (intentional close)');
        return;
      }
      // Only reload metadata (title, status, etc.) - NOT content
      // Content reload is handled by handleReload (pane-reload-editor) with diff highlighting
      if (noteId && currentProject) {
        try {
          const updated = await api.readNote(currentProject, noteId);
          if (updated && updated.meta) {
            // GUARD (#2237): Do not resurrect $activeNote if it was intentionally cleared.
            // Race window: the destroy-save dispatches pane-note-saved (and callers may
            // dispatch pane-data-changed), our async handler is already in flight, and
            // api.readNote() resolves AFTER goBackToCards() has set activeNote to null.
            // Writing back here would reopen the panel the user just closed.
            // Rule: a refresh handler may only update $activeNote when it is already
            // populated AND matches this Editor's own note. Null is an intentional close
            // signal — never repopulate from null.
            const currentActive = get(activeNote);
            if (!currentActive || currentActive.meta?.id !== noteId) {
              if (DEBUG) console.log(`[PANE-DEBUG] handleDataChanged[reload-meta] SKIPPED — $activeNote is ${currentActive ? 'mismatched (' + currentActive.meta?.id + ')' : 'null'} for noteId=${noteId} (intentional close or note switched)`);
              return;
            }
            const newTitle = updated.meta.title || '';
            if (newTitle && newTitle !== noteTitle && newTitle !== 'Untitled') {
              noteTitle = newTitle;
            }
            logAnSet('Editor.handleDataChanged[reload-meta]', updated);
            activeNote.set(updated);
          }
        } catch (e) {
          console.error('[EDITOR] Failed to reload metadata after data change:', e);
        }
      }
    };

    window.addEventListener('pane-reload-editor', handleReload);

    // #2972 stage 3 (item 4): one shared helper hearing BOTH channels —
    // backend `listen()` AND the app `CustomEvent` — replacing Stage 2's
    // backend-only `listen()` registration PLUS the hand-paired DOM
    // `addEventListener` this file carried as a Stage-2-era placeholder
    // (exactly the "hand-written double registration" item 4 forbids).
    const unlistenDataChanged = subscribeDataChanged((payload) => {
      void handleDataChanged(payload);
    });

    // Apply saved font size
    const applyFontSize = (size?: string) => {
      const fs = size || localStorage.getItem('pane_editor_font_size') || '15.5';
      const editorEl = element?.querySelector('.pane-editor') as HTMLElement;
      if (editorEl) editorEl.style.fontSize = `${fs}px`;
    };
    applyFontSize();

    const handleFontSizeChange = (e: Event) => {
      applyFontSize((e as CustomEvent).detail);
    };
    window.addEventListener('pane-editor-font-size-changed', handleFontSizeChange);

    const handleEditorClickOutside = (e: MouseEvent) => {
      if (!(e.target as HTMLElement).closest('.pill-wrapper')) {
        editorStatusOpen = false;
        editorPriorityOpen = false;
        editorDueDateOpen = false;
        editorFolderOpen = false;
      }
      // Close ⋯ menu on outside click
      if (!(e.target as HTMLElement).closest('.tpl-more-wrap')) {
        moreMenuOpen = false;
      }
    };
    document.addEventListener('click', handleEditorClickOutside);

    // Close fixed-position pill dropdowns on scroll or resize so they don't
    // float detached from their anchor pill (#2205).
    const handlePillClose = () => {
      editorStatusOpen = false;
      editorPriorityOpen = false;
      editorDueDateOpen = false;
      editorFolderOpen = false;
    };
    window.addEventListener('scroll', handlePillClose, true);
    window.addEventListener('resize', handlePillClose);

    const handleEditorKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && editorFolderOpen) {
        editorFolderOpen = false;
        e.stopPropagation();
      }
    };
    document.addEventListener('keydown', handleEditorKeyDown);

    return () => {
      window.removeEventListener('pane-reload-editor', handleReload);
      unlistenDataChanged.then(fn => fn());
      window.removeEventListener('pane-editor-font-size-changed', handleFontSizeChange);
      document.removeEventListener('click', handleEditorClickOutside);
      window.removeEventListener('scroll', handlePillClose, true);
      window.removeEventListener('resize', handlePillClose);
      document.removeEventListener('keydown', handleEditorKeyDown);
    };
  });

  onDestroy(() => {
    // Flush any pending save before destroying
    clearTimeout(editorUpdateTimeout); // Cancel any pending debounced saves

    if (isSheet) {
      // Sheets have no TipTap instance, so there is no body to derive/save
      // here — only the title, via a dedicated title-only save path that
      // never sends a `content` field (see #2874 item 17).
      if (dirty) {
        const hasValidTitle = noteTitle && noteTitle !== 'Untitled' && noteTitle.trim();
        if (hasValidTitle) {
          console.log('[EDITOR-DESTROY] Saving sheet title before destroy. Title:', noteTitle);
          const saveP = saveSheetTitle();
          setEditorSavePromise((saveP ?? Promise.resolve()).then(() => undefined));
        } else {
          console.log('[EDITOR-DESTROY] No valid sheet title to save');
        }
      } else {
        console.log('[EDITOR-DESTROY] SKIPPED sheet title save - content not dirty (no user edits)');
      }
    } else if (editor) {
      let md: string;

      try {
        // getJSON() is pure JS traversal - works even on detached DOM.
        // getHTML() uses DOMSerializer and may throw when DOM is detached.
        // Attempt both; fall back to lastMd string cache only if getHTML throws.
        JSON.stringify(editor.getJSON()); // validates state is readable
        md = htmlToMarkdown(editor.getHTML());
      } catch (e) {
        // Editor DOM detached - use cached markdown
        console.log('[EDITOR-DESTROY] getHTML failed, using cached lastMd (should be current)');
        md = lastMd;
      }

      // BUG FIX v10.2.1: Only save if user has edited content (dirty flag)
      // This prevents overwriting AI-written content when editor is destroyed before reload completes
      if (!dirty) {
        console.log('[EDITOR-DESTROY] SKIPPED save - content not dirty (no user edits)');
      } else {
        // Only save if there's meaningful content
        const hasValidTitle = noteTitle && noteTitle !== 'Untitled' && noteTitle.trim();
        const hasBodyContent = md.trim().length > 0;

        if (hasValidTitle || hasBodyContent) {
          console.log('[EDITOR-DESTROY] Saving content before destroy. Title:', noteTitle, 'MD length:', md.length);
          // Direct save, not through parent - guaranteed persistence
          // Expose the save promise so noteService can await it instead of using a heuristic delay
          const saveP = saveContent(md);
          setEditorSavePromise(saveP ?? Promise.resolve());
        } else {
          console.log('[EDITOR-DESTROY] No meaningful content to save');
        }
      }
    }

    // Mark as destroyed AFTER flush save - blocks any subsequent stale saves
    isDestroyed = true;

    editor?.destroy();
    clearTimeout(linkRebuildTimeout);
  });
</script>

<div class="editor-wrapper" class:focused={isFocused}>
  {#if isReadOnly}
    <div class="read-only-bar">
      Read-only -
      {#if noteMeta?.note_type === 'memory'}
        Edit in Memory Settings
      {:else if noteMeta?.note_type === 'daily'}
        Managed by the memory pipeline
      {:else}
        Created during onboarding. View in Agents settings.
      {/if}
    </div>
  {/if}
  <div class="title-row">
    <input
      bind:this={titleInput}
      bind:value={noteTitle}
      oninput={handleTitleChange}
      readonly={isReadOnly}
      onfocus={() => {
        titleFocused = true;
        if (noteTitle === 'Untitled') {
          titleInput?.select();
        }
      }}
      onblur={() => {
        titleFocused = false;
        // Save immediately when title field loses focus
        if (dirty && isSheet) {
          // Sheets have no TipTap instance — title-only save, no `content`
          // field, so the #2901 sheet-body guard is never triggered.
          // See #2874 item 17.
          saveSheetTitle();
          dirty = false;
        } else if (dirty && editor) {
          const html = editor.getHTML();
          const md = htmlToMarkdown(html);
          saveContent(md);
          dirty = false;
        }
      }}
      placeholder="Note title..."
      class="title-input"
      data-testid="note-title-input"
    />

    {#if noteId && noteMeta}
      <div class="metadata-pills-inline">
        <!-- Folder pill -->
        {#if noteMeta?.project_id}
          <div class="pill-wrapper">
            <button
              bind:this={folderPillBtn}
              class="pill folder-pill"
              class:open={editorFolderOpen}
              onclick={(e) => { e.stopPropagation(); toggleFolderPicker(e.currentTarget as HTMLElement); }}
            >
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M22 19a2 2 0 01-2 2H4a2 2 0 01-2-2V5a2 2 0 012-2h5l2 3h9a2 2 0 012 2z"/>
              </svg>
              <span class="folder-pill-name">{currentFolderName}</span>
              <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style="opacity:.7">
                {#if editorFolderOpen}
                  <path d="M18 15l-6-6-6 6"/>
                {:else}
                  <path d="M6 9l6 6 6-6"/>
                {/if}
              </svg>
            </button>
            {#if editorFolderOpen && pillFolderAnchor}
              <div class="folder-picker-dropdown" style="top: {pillFolderAnchor.top}px; right: {pillFolderAnchor.right}px">
                <div class="folder-picker-header">Move to folder</div>
                {#each projectFolders as folder}
                  <button
                    class="folder-picker-item"
                    class:current={folder.id === noteMeta.folder_id}
                    onclick={(e) => { e.stopPropagation(); handleFolderMove(folder); }}
                  >
                    <span class="folder-picker-item-left">
                      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                        <path d="M22 19a2 2 0 01-2 2H4a2 2 0 01-2-2V5a2 2 0 012-2h5l2 3h9a2 2 0 012 2z"/>
                      </svg>
                      <span class="folder-picker-name">{folder.name}</span>
                    </span>
                    {#if folder.id === noteMeta.folder_id}
                      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                        <path d="M20 6L9 17l-5-5"/>
                      </svg>
                    {/if}
                  </button>
                {/each}
              </div>
            {/if}
          </div>
        {/if}

        <!-- Status pill -->
        <div class="pill-wrapper">
          <button
            class="pill"
            style="color: {getStageColor(noteMeta.task_status)}; border-color: {getStageColor(noteMeta.task_status)}40"
            onclick={(e) => { const r = (e.currentTarget as HTMLElement).getBoundingClientRect(); pillStatusAnchor = { top: r.bottom + 6, left: r.left }; editorStatusOpen = !editorStatusOpen; }}
          >
            {formatEditorStatus(noteMeta.task_status || 'cabinet')}
          </button>
          {#if editorStatusOpen && pillStatusAnchor}
            <div class="pill-dropdown" style="top: {pillStatusAnchor.top}px; left: {pillStatusAnchor.left}px">
              {#each ['cabinet', 'todo', 'in_progress', 'review', 'done'] as s}
                <button class="pill-dropdown-option" class:selected={noteMeta.task_status === s} onclick={() => updateEditorMeta('task_status', s)}>
                  {formatEditorStatus(s)}
                </button>
              {/each}
            </div>
          {/if}
        </div>

        <!-- Priority pill -->
        <div class="pill-wrapper">
          <button
            class="pill"
            style="color: {getEditorPriorityColor(noteMeta.task_priority)}; border-color: {getEditorPriorityColor(noteMeta.task_priority)}40"
            onclick={(e) => { const r = (e.currentTarget as HTMLElement).getBoundingClientRect(); pillPriorityAnchor = { top: r.bottom + 6, left: r.left }; editorPriorityOpen = !editorPriorityOpen; }}
          >
            {(noteMeta.task_priority || 'medium').charAt(0).toUpperCase() + (noteMeta.task_priority || 'medium').slice(1)}
          </button>
          {#if editorPriorityOpen && pillPriorityAnchor}
            <div class="pill-dropdown" style="top: {pillPriorityAnchor.top}px; left: {pillPriorityAnchor.left}px">
              {#each ['low', 'medium', 'high', 'urgent'] as p}
                <button class="pill-dropdown-option" class:selected={noteMeta.task_priority === p} onclick={() => updateEditorMeta('task_priority', p)}>
                  {p.charAt(0).toUpperCase() + p.slice(1)}
                </button>
              {/each}
            </div>
          {/if}
        </div>

        <!-- Due date pill -->
        <div class="pill-wrapper">
          <button
            class="pill due"
            onclick={(e) => { const r = (e.currentTarget as HTMLElement).getBoundingClientRect(); pillDueDateAnchor = { top: r.bottom + 6, left: r.left }; editorDueDateOpen = !editorDueDateOpen; }}
          >
            {noteMeta.task_due_date ? new Date(noteMeta.task_due_date + 'T00:00:00').toLocaleDateString() : 'No due date'}
          </button>
          {#if editorDueDateOpen && pillDueDateAnchor}
            <input type="date" class="pill-date-picker" style="position: fixed; top: {pillDueDateAnchor.top}px; left: {pillDueDateAnchor.left}px" value={noteMeta.task_due_date || ''} onchange={(e) => updateEditorMeta('task_due_date', e.currentTarget.value)} />
          {/if}
        </div>
      </div>
    {/if}
  </div>

  {#if isSheet}
    <!-- #2339 PR10: insert-row/insert-column buttons wired through to the
         bound SheetGrid instance (ruling 5); #2874 item 20: the unified
         "Style cells" button follows the SAME callback pattern — it just
         calls `sheetGridRef.openStylePanel()` (the toolbar itself never
         sees the selection or any unanimity state, per Matt's
         callback-pattern ruling). Replaces the old three-button
         fill/text/border wiring (#2339 PR 15d-beta) now that a single
         popover exposes every style control. All other toolbar buttons
         remain disabled inside SheetToolbar itself. -->
    <SheetToolbar
      onInsertRow={() => sheetGridRef?.insertRow()}
      onInsertColumn={() => sheetGridRef?.insertColumn()}
      onExportCsv={handleExportSheetCsv}
      onVersionHistory={openVersionHistory}
      onDeleteSheet={handleMoveToTrash}
      onStyleClick={(anchorEl) => sheetGridRef?.openStylePanel(anchorEl)}
      onFormulaClick={() => sheetGridRef?.openFormulaOnAnchor()}
      onSortClick={() => sheetGridRef?.toolbarSort()}
      onFilterClick={(anchorEl) => { if (anchorEl) sheetGridRef?.toolbarFilter(anchorEl); }}
      sortEnabled={formulaBarInfo.selectedColumnIndex !== null}
      filterEnabled={formulaBarInfo.selectedColumnType !== null && filterSupported(formulaBarInfo.selectedColumnType)}
      formulaEnabled={formulaBarInfo.selectedColumnIndex !== null}
    />
    <!-- #2339 PR14 (Matt ruling 2026-09-05 #4): the ShPaused banner sits
         between the toolbar and the formula bar — "in the path of anyone
         about to type a formula, out of the path of anyone scrolling
         data" (design Cap prose). Neutral surface, no warning color;
         same clock glyph shape as version history's SVG elsewhere in this
         file. No action button (ratified in review — v1 has no "compute
         once" escape hatch). -->
    {#if sheetFormulasPaused}
      <div class="sh-paused" data-testid="sheet-paused-banner">
        <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="12" cy="12" r="9"/><polyline points="12 7 12 12 16 14"/></svg>
        <span class="sh-paused__t"><b>{sheetFormulaCount} formulas</b> — computation paused for size.</span>
        <span class="sh-paused__sub">Cells show what you wrote.</span>
        <span class="sh-paused__sp"></span>
      </div>
    {/if}
  {:else if editor}
    <!-- bind:this feeds the ResizeObserver that collapses overflow items on measured overflow (#2123, #2135) -->
    <div class="editor-toolbar" class:toolbar--narrow={isNarrow} bind:this={toolbarEl}>
      <!-- Always-visible: B I S | T H1 H2 H3 | • 1. -->
      <button class="toolbar-btn" class:active={editor.isActive('bold')} onclick={() => editor?.chain().focus().toggleBold().run()} title="Bold (Cmd+B)">B</button>
      <button class="toolbar-btn" class:active={editor.isActive('italic')} onclick={() => editor?.chain().focus().toggleItalic().run()} title="Italic (Cmd+I)">I</button>
      <button class="toolbar-btn" class:active={editor.isActive('strike')} onclick={() => editor?.chain().focus().toggleStrike().run()} title="Strikethrough">S</button>
      <span class="toolbar-sep"></span>
      <button class="toolbar-btn" class:active={!editor.isActive('heading') && !editor.isActive('bulletList') && !editor.isActive('orderedList') && !editor.isActive('taskList') && !editor.isActive('codeBlock') && !editor.isActive('blockquote')} onclick={() => editor?.chain().focus().setParagraph().run()} title="Normal Text">T</button>
      <button class="toolbar-btn" class:active={editor.isActive('heading', { level: 1 })} onclick={() => editor?.chain().focus().toggleHeading({ level: 1 }).run()} title="Heading 1">H1</button>
      <button class="toolbar-btn" class:active={editor.isActive('heading', { level: 2 })} onclick={() => editor?.chain().focus().toggleHeading({ level: 2 }).run()} title="Heading 2">H2</button>
      <button class="toolbar-btn" class:active={editor.isActive('heading', { level: 3 })} onclick={() => editor?.chain().focus().toggleHeading({ level: 3 }).run()} title="Heading 3">H3</button>
      <span class="toolbar-sep"></span>
      <button class="toolbar-btn" class:active={editor.isActive('bulletList')} onclick={() => editor?.chain().focus().toggleBulletList().run()} title="Bullet List">•</button>
      <button class="toolbar-btn" class:active={editor.isActive('orderedList')} onclick={() => editor?.chain().focus().toggleOrderedList().run()} title="Numbered List">1.</button>
      <!-- Overflow group: hidden when toolbar--narrow; appears in ⋯ Formatting group instead -->
      <span class="toolbar-sep toolbar-sep--overflow"></span>
      <button class="toolbar-btn toolbar-btn--overflow" onclick={handleToolbarLink} title="Link"><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"/><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"/></svg></button>
      <button class="toolbar-btn toolbar-btn--overflow" class:active={editor.isActive('codeBlock')} onclick={handleToolbarCodeBlock} title="Code Block">Code</button>
      <span class="toolbar-sep toolbar-sep--overflow"></span>
      <button class="toolbar-btn toolbar-btn--overflow" onclick={handleToolbarUndo} title="Undo"><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M3 10h10a5 5 0 0 1 0 10H3M3 10l4-4M3 10l4 4"/></svg></button>
      <button class="toolbar-btn toolbar-btn--overflow" onclick={handleToolbarRedo} title="Redo"><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M21 10H11a5 5 0 0 0 0 10h10M21 10l-4-4M21 10l-4 4"/></svg></button>
      <button class="toolbar-btn toolbar-btn--overflow" onclick={openVersionHistory} title="Version History"><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/></svg></button>
      <button
        class="toolbar-btn toolbar-btn--overflow pin-btn"
        class:pinned={isPinned}
        onclick={handlePinToggle}
        title={isPinned ? "Unpin note" : "Pin note"}
        disabled={!noteId || loadingPin}
      >
        <svg width="14" height="14" viewBox="0 0 24 24" fill={isPinned ? "currentColor" : "none"} stroke="currentColor" stroke-width="2">
          <path d="M12 17v5"/>
          <path d="M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z"/>
        </svg>
      </button>
      <span class="toolbar-sep toolbar-sep--overflow"></span>
      <!-- #1898 PR8: chain-glyph trigger for link-attachment insertion -->
      <button
        class="toolbar-btn toolbar-btn--overflow"
        class:active={linkModalOpen}
        onclick={handleToolbarInsertLink}
        title="Insert link"
        data-testid="toolbar-insert-link"
      >
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
          <path d="M10 13a5 5 0 0 0 7.5.5l3-3a5 5 0 0 0-7-7l-1.5 1.5"/>
          <path d="M14 11a5 5 0 0 0-7.5-.5l-3 3a5 5 0 0 0 7 7L12 19"/>
        </svg>
      </button>
      <button
        class="toolbar-btn toolbar-btn--overflow"
        onclick={handleToolbarChecklist}
        title="Insert Checklist"
      >
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
          <path d="m3 17 2 2 4-4"/>
          <path d="m3 7 2 2 4-4"/>
          <path d="M13 6h8"/>
          <path d="M13 12h8"/>
          <path d="M13 18h8"/>
        </svg>
      </button>
      <button
        class="toolbar-btn toolbar-btn--overflow"
        onclick={handleToolbarDueToday}
        title="Insert Due Today block"
        data-testid="toolbar-insert-due-today"
      >
        <!-- Calendar-check icon for Due Today block -->
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
          <rect x="3" y="4.5" width="18" height="16" rx="2"/>
          <path d="M3 9h18M8 2.5v4M16 2.5v4M9 14l2 2 4-4"/>
        </svg>
      </button>
      <button class="toolbar-btn toolbar-btn--overflow" onclick={handleToolbarAttachFile} title="Attach File"><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 0 1 5.66 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48"/></svg></button>
      <div class="toolbar-autosave">
        {#if isSaving}
          <span class="saving">Saving...</span>
        {:else if lastSaved}
          <span class="saved">Saved {lastSaved}</span>
        {/if}
      </div>
      <!-- ⋯ note menu (§32.2/§32.3) -->
      <div class="tpl-more-wrap">
        <button
          class="toolbar-btn tpl-more-btn"
          class:tpl-more-btn--open={moreMenuOpen}
          onclick={openMoreMenu}
          title="More actions"
          aria-label="More actions"
        >
          <svg width="17" height="17" viewBox="0 0 24 24" fill="currentColor" stroke="none">
            <circle cx="5" cy="12" r="1.7"/><circle cx="12" cy="12" r="1.7"/><circle cx="19" cy="12" r="1.7"/>
          </svg>
        </button>
        {#if moreMenuOpen}
          <div class="tpl-menu" role="menu">
            {#if isNarrow}
              <!-- Formatting group: collapsed toolbar items (only at narrow width) -->
              <div class="tpl-menu__label">Formatting</div>
              <button class="tpl-mi" onclick={handleToolbarLink} role="menuitem">
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
                  <path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"/>
                  <path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"/>
                </svg>
                Link
              </button>
              <button class="tpl-mi" onclick={handleToolbarInsertLink} role="menuitem">
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
                  <path d="M10 13a5 5 0 0 0 7.5.5l3-3a5 5 0 0 0-7-7l-1.5 1.5"/>
                  <path d="M14 11a5 5 0 0 0-7.5-.5l-3 3a5 5 0 0 0 7 7L12 19"/>
                </svg>
                Insert link
              </button>
              <button class="tpl-mi" onclick={handleToolbarCodeBlock} role="menuitem">
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
                  <polyline points="16 18 22 12 16 6"/><polyline points="8 6 2 12 8 18"/>
                </svg>
                Code block
              </button>
              <button class="tpl-mi" onclick={handleToolbarUndo} role="menuitem">
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
                  <path d="M3 10h10a5 5 0 0 1 0 10H3M3 10l4-4M3 10l4 4"/>
                </svg>
                Undo
              </button>
              <button class="tpl-mi" onclick={handleToolbarRedo} role="menuitem">
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
                  <path d="M21 10H11a5 5 0 0 0 0 10h10M21 10l-4-4M21 10l-4 4"/>
                </svg>
                Redo
              </button>
              <button class="tpl-mi" onclick={openVersionHistory} role="menuitem">
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
                  <circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/>
                </svg>
                Version history
              </button>
              <button class="tpl-mi" onclick={() => { moreMenuOpen = false; handlePinToggle(); }} role="menuitem" disabled={!noteId || loadingPin}>
                <svg width="16" height="16" viewBox="0 0 24 24" fill={isPinned ? 'currentColor' : 'none'} stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
                  <path d="M12 17v5"/>
                  <path d="M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z"/>
                </svg>
                {isPinned ? 'Unpin note' : 'Pin note'}
              </button>
              <button class="tpl-mi" onclick={handleToolbarChecklist} role="menuitem">
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
                  <path d="m3 17 2 2 4-4"/><path d="m3 7 2 2 4-4"/><path d="M13 6h8"/><path d="M13 12h8"/><path d="M13 18h8"/>
                </svg>
                Checklist
              </button>
              <button class="tpl-mi" onclick={handleToolbarDueToday} role="menuitem">
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
                  <rect x="3" y="4.5" width="18" height="16" rx="2"/>
                  <path d="M3 9h18M8 2.5v4M16 2.5v4M9 14l2 2 4-4"/>
                </svg>
                Due today
              </button>
              <button class="tpl-mi" onclick={handleToolbarAttachFile} role="menuitem">
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
                  <path d="M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 0 1 5.66 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48"/>
                </svg>
                Attach file
              </button>
              <hr class="tpl-sep" />
            {/if}
            <div class="tpl-menu__label">Templates</div>
            <button class="tpl-mi" onclick={openApplyPicker} role="menuitem">
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
                <rect x="3" y="3" width="18" height="18" rx="2"/><path d="M3 9h18M9 21V9"/>
              </svg>
              Apply template…
              <span class="tpl-mi__key">⌘⇧T</span>
            </button>
            <button class="tpl-mi" onclick={openSaveAsTemplate} role="menuitem">
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
                <path d="M19 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11l5 5v11a2 2 0 0 1-2 2Z"/><path d="M17 21v-8H7v8M7 3v5h8"/>
              </svg>
              Save as template…
            </button>
            <hr class="tpl-sep" />
            <!-- Pin note: always in ⋯ at all widths; at wide width, ALSO shown in toolbar -->
            <button
              class="tpl-mi"
              onclick={() => { moreMenuOpen = false; handlePinToggle(); }}
              role="menuitem"
              disabled={!noteId || loadingPin}
            >
              <svg width="16" height="16" viewBox="0 0 24 24" fill={isPinned ? 'currentColor' : 'none'} stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
                <path d="M12 17v5"/>
                <path d="M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z"/>
              </svg>
              {isPinned ? 'Unpin note' : 'Pin note'}
            </button>
            <!-- Move to trash: danger styling -->
            <button
              class="tpl-mi tpl-mi--danger"
              onclick={handleMoveToTrash}
              role="menuitem"
              disabled={!noteId || moveToTrashLoading}
            >
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
                <polyline points="3 6 5 6 21 6"/>
                <path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/>
                <path d="M10 11v6M14 11v6"/>
                <path d="M9 6V4a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2"/>
              </svg>
              Move to trash
            </button>
          </div>
        {/if}
      </div>
    </div>
  {/if}

  <!-- Template modals (§32) -->

  <!-- Save-as-template prompt (§32.2) -->
  {#if savePromptOpen}
    <div class="tpl-scrim" role="button" tabindex="-1" onclick={closeSavePrompt}></div>
    <div class="tpl-dialog" role="dialog" aria-modal="true" aria-label="Save as template">
      <div class="tpl-dialog__head">
        <span class="tpl-dialog__icon">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
            <path d="M19 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11l5 5v11a2 2 0 0 1-2 2Z"/><path d="M17 21v-8H7v8M7 3v5h8"/>
          </svg>
        </span>
        <h3 class="tpl-dialog__title">Save as template</h3>
        <button class="tpl-dialog__x" onclick={closeSavePrompt}>
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
            <path d="M18 6 6 18M6 6l12 12"/>
          </svg>
        </button>
      </div>
      <div class="tpl-dialog__gap"></div>
      <label class="tpl-field-label" for="tpl-save-name">Template name</label>
      <div class="tpl-input-wrap tpl-input-wrap--focus">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
          <rect x="3" y="3" width="18" height="18" rx="2"/><path d="M3 9h18M9 21V9"/>
        </svg>
        <!-- svelte-ignore a11y_autofocus -->
        <input
          id="tpl-save-name"
          bind:value={savePromptName}
          autofocus
          onkeydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); confirmSaveTemplate(); } if (e.key === 'Escape') closeSavePrompt(); }}
        />
      </div>
      <div class="tpl-hint-line">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
          <path d="M12 9v4M12 17h.01M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0Z"/>
        </svg>
        Only the note’s structure is saved — no due dates, statuses, or check-states. Items copy unchecked.
      </div>
      <div class="tpl-dialog__gap"></div>
      <div class="tpl-actions">
        <button class="tpl-btn tpl-btn--ghost" onclick={closeSavePrompt}>Cancel</button>
        <button class="tpl-btn tpl-btn--primary" onclick={confirmSaveTemplate} disabled={savePromptSaving || !savePromptName.trim()}>
          Save template <span class="tpl-enter">↵</span>
        </button>
      </div>
    </div>
  {/if}

  <!-- Collision: single match (§32.2.2) -->
  {#if collisionSingleOpen}
    <div class="tpl-scrim" role="button" tabindex="-1" onclick={() => { collisionSingleOpen = false; }}></div>
    <div class="tpl-dialog" role="dialog" aria-modal="true" aria-label="Template already exists">
      <div class="tpl-dialog__head">
        <span class="tpl-dialog__icon tpl-dialog__icon--danger">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
            <path d="M12 9v4M12 17h.01M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0Z"/>
          </svg>
        </span>
        <h3 class="tpl-dialog__title">Template already exists</h3>
      </div>
      <div class="tpl-dialog__gap"></div>
      <p class="tpl-dialog__body">A template named <strong>"{collisionSingleName}"</strong> already exists. Overwrite it, or save this one under a different name?</p>
      <div class="tpl-dialog__gap"></div>
      <div class="tpl-actions">
        <button class="tpl-btn tpl-btn--ghost" onclick={collisionRenameSingle}>Rename</button>
        <button class="tpl-btn tpl-btn--danger" onclick={collisionOverwriteSingle}>Overwrite</button>
      </div>
    </div>
  {/if}

  <!-- Collision: multi match (§32.2.2) -->
  {#if collisionMultiOpen}
    <div class="tpl-scrim" role="button" tabindex="-1" onclick={() => { collisionMultiOpen = false; }}></div>
    <div class="tpl-dialog tpl-dialog--wide" role="dialog" aria-modal="true" aria-label="Multiple templates match">
      <div class="tpl-dialog__head">
        <span class="tpl-dialog__icon tpl-dialog__icon--danger">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
            <path d="M12 9v4M12 17h.01M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0Z"/>
          </svg>
        </span>
        <h3 class="tpl-dialog__title">Multiple templates match</h3>
      </div>
      <div class="tpl-dialog__gap"></div>
      <p class="tpl-dialog__body"><strong>{collisionMultiMatches.length} templates</strong> named "{collisionMultiName}" exist (from sync). Choose exactly one to overwrite, or rename this save.</p>
      <div style="height:12px"></div>
      <div class="tpl-match-list">
        {#each collisionMultiMatches as m}
          <div
            class="tpl-match"
            class:tpl-match--sel={collisionMultiSel === m.id}
            role="button"
            tabindex="0"
            onclick={() => { collisionMultiSel = collisionMultiSel === m.id ? null : m.id; }}
            onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); collisionMultiSel = collisionMultiSel === m.id ? null : m.id; } }}
          >
            <span class="tpl-radio"></span>
            <span class="tpl-match__body">
              <span class="tpl-match__name">{m.name}</span>
              <span class="tpl-match__meta">Updated {formatTemplateDate(m.updated_at)}</span>
            </span>
            {#if collisionMultiSel === m.id}<span class="tpl-match__badge">will overwrite</span>{/if}
          </div>
        {/each}
      </div>
      <div class="tpl-hint-line">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
          <path d="M12 9v4M12 17h.01M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0Z"/>
        </svg>
        Nothing is pre-selected — pick a template to enable Overwrite.
      </div>
      <div class="tpl-dialog__gap"></div>
      <div class="tpl-actions">
        <button class="tpl-btn tpl-btn--ghost" onclick={collisionRenameMulti}>Rename</button>
        <button class="tpl-btn tpl-btn--danger" onclick={collisionOverwriteMulti} disabled={!collisionMultiSel}>Overwrite selected</button>
      </div>
    </div>
  {/if}

  {#if isSheet}
    {#if noteId}
      <!-- #2339 PR9: read-only grid render, selection + keyboard nav only.
           No cell editing, no update_sheet_cell call anywhere in this tree.
           #2339 PR14: FormulaBar mounts BETWEEN the toolbar/paused-banner
           above and SheetGrid below — design's flex-column order
           (Toolbar → ShPaused notice → FormulaBar → SheetGrid →
           StatusBar). It reads/writes SheetGrid's shared edit buffer via
           the bind:this handle's barBeginEdit/barSetValue/barCommit/
           barCancel exports — the same imperative pattern PR10 already
           established for insertRow/insertColumn. -->
      <FormulaBar
        cref={formulaBarInfo.cref}
        value={formulaBarInfo.value}
        editing={formulaBarInfo.editing}
        error={formulaBarInfo.error}
        selectionStart={formulaBarInfo.selectionStart}
        selectionEnd={formulaBarInfo.selectionEnd}
        onInput={handleFormulaBarInput}
        onCommit={(cause) => sheetGridRef?.barCommit(cause)}
        onCancel={() => sheetGridRef?.barCancel()}
        onSelectionChange={(start, end) => sheetGridRef?.barSetSelection(start, end)}
        onBarClick={() => sheetGridRef?.barBeginEdit()}
      />
      <SheetGrid
        bind:this={sheetGridRef}
        noteId={noteId}
        onLoaded={(data) => { sheetColsCount = data.columns.length; sheetRowsCount = data.rows.length; sheetFormulaCount = data.formula_count; sheetFormulasPaused = data.formulas_paused; }}
        onSelectionInfo={(info) => { formulaBarInfo = info; }}
        onExportCsv={handleExportSheetCsv}
      />
      <SheetStatusBar cols={sheetColsCount} rows={sheetRowsCount} formulas={sheetFormulaCount} paused={sheetFormulasPaused} />
    {/if}
  {:else}
    <div bind:this={element} class="editor-container" data-testid="note-editor"></div>
  {/if}

  <!-- #1898 PR8: Link insert modal -->
  {#if linkModalOpen}
    <LinkInsertModal
      on:submit={handleLinkModalSubmit}
      on:cancel={handleLinkModalCancel}
    />
  {/if}

  {#if linkPromptOpen}
    <PromptDialog
      title="Insert Link"
      placeholder="https://example.com"
      confirmLabel="Insert"
      onConfirm={submitToolbarLink}
      onCancel={() => { linkPromptOpen = false; }}
    />
  {/if}

  <!-- Linked Tasks Section -->
  {#if linkedTasks.length > 0 || showNewTaskInput}
  <div class="linked-tasks-section">
    <div class="linked-tasks-header">
      <span class="linked-tasks-title">TASKS ({linkedTasks.length})</span>
      <button class="add-task-btn" onclick={() => { showNewTaskInput = true; setTimeout(() => newTaskInputEl?.focus(), 50); }} title="Add task">+</button>
    </div>
    {#if showNewTaskInput}
    <div class="new-task-row">
      <input
        class="new-task-input"
        bind:this={newTaskInputEl}
        bind:value={newTaskTitle}
        placeholder="Task title..."
        onkeydown={(e) => { if (e.key === 'Enter' && newTaskTitle.trim()) createLinkedTask(); if (e.key === 'Escape') { showNewTaskInput = false; newTaskTitle = ''; } }}
      />
    </div>
    {/if}
    {#each linkedTasks as task (task.id)}
    <div class="linked-task-row" class:done={task.status === 'done'}>
      <button class="task-checkbox" onclick={() => toggleLinkedTask(task)}>
        {#if task.status === 'done'}
          <svg width="16" height="16" viewBox="0 0 16 16" fill="none"><rect x="1" y="1" width="14" height="14" rx="3" fill="#00E4E4" stroke="#00E4E4" stroke-width="1.5"/><path d="M4.5 8L7 10.5L11.5 5.5" stroke="white" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>
        {:else}
          <svg width="16" height="16" viewBox="0 0 16 16" fill="none"><rect x="1.5" y="1.5" width="13" height="13" rx="2.5" stroke="#666666" stroke-width="1.5"/></svg>
        {/if}
      </button>
      <span class="linked-task-title" class:done={task.status === 'done'}>{task.title}</span>
      {#if task.priority && task.priority !== 'medium'}
        <span class="task-priority-badge {task.priority}">{task.priority}</span>
      {/if}
      {#if task.due_date}
        <span class="task-due-date">{task.due_date}</span>
      {/if}
    </div>
    {/each}
  </div>
  {/if}

  <div class="backlinks-container">
    <Backlinks {noteId} onNavigate={handleNoteNavigation} />
  </div>


</div>

<!-- Error toast (#1815) -->
{#if errorToastMsg}
<div class="error-toast" role="alert">
  <span class="error-toast-msg">{errorToastMsg}</span>
  <button class="error-toast-dismiss" onclick={() => { errorToastMsg = null; }}>✕</button>
</div>
{/if}

<!-- Version History Modal -->
{#if showVersionHistory}
  {#if isSheet}
    <!-- #2339 PR 15b-beta: sheet-native panel — never the markdown
         overlay below. Restores via `restore_sheet_snapshot` only
         (ShVersionHistoryPanel itself enforces this; see its own tests). -->
    <ShVersionHistoryPanel
      noteId={noteId ?? ''}
      onClose={() => { showVersionHistory = false; }}
      onRestored={() => { sheetGridRef?.reload(); }}
    />
  {:else}
    <div class="vh-overlay" role="dialog" aria-modal="true">
      <div class="vh-panel">
        <div class="vh-header">
          <h3 class="vh-title">Version History</h3>
          <button class="vh-close" onclick={() => { showVersionHistory = false; previewVersion = null; }}>
            <svg width="16" height="16" viewBox="0 0 16 16" fill="none"><path d="M4 4l8 8M12 4l-8 8" stroke="#999" stroke-width="1.5" stroke-linecap="round"/></svg>
          </button>
        </div>
        <div class="vh-body">
          <div class="vh-list">
            {#if versionList.length === 0}
              <p class="vh-empty">No versions saved yet</p>
            {:else}
              {#each versionList as v (v.id)}
                <button class="vh-item" class:active={previewVersion?.id === v.id} onclick={() => previewVersionContent(v.id)}>
                  <span class="vh-date">{formatVersionDate(v.created_at)}</span>
                  <span class="vh-source">{sourceLabel(v.source)}</span>
                </button>
              {/each}
            {/if}
          </div>
          <div class="vh-preview">
            {#if previewVersion}
              <div class="vh-preview-header">
                <span class="vh-preview-date">{formatVersionDate(previewVersion.created_at)}</span>
                <button class="vh-restore-btn" onclick={() => previewVersion && restoreFromVersion(previewVersion.id)}>Restore</button>
              </div>
              <pre class="vh-preview-content">{previewVersion.content}</pre>
            {:else}
              <p class="vh-preview-empty">Select a version to preview</p>
            {/if}
          </div>
        </div>
      </div>
    </div>
  {/if}
{/if}

<style>
  .read-only-bar {
    height: 28px;
    display: flex;
    align-items: center;
    padding: 0 16px;
    background: rgba(255, 255, 255, 0.03);
    color: #666;
    font-size: 12px;
    letter-spacing: 0.02em;
    border-bottom: 1px solid rgba(255, 255, 255, 0.04);
    flex-shrink: 0;
  }

  /* Diff highlight decorations */
  :global(.diff-added) {
    background: rgba(16, 185, 129, 0.2);
    border-radius: 2px;
    transition: opacity 0.5s ease;
  }
  :global(.diff-removed) {
    background: rgba(239, 68, 68, 0.2);
    color: #EF4444;
    text-decoration: line-through;
    border-radius: 2px;
    transition: opacity 0.5s ease;
  }
  :global(.diff-fading) {
    opacity: 0;
  }

  .editor-wrapper {
    display: flex;
    flex-direction: column;
    height: 100%;
    overflow: hidden;
    position: relative;
    background: #0a0a0f;
    color: #E4E4E7;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
    -webkit-font-smoothing: antialiased;
    font-weight: 400;
  }

  /* #2339 PR14 — ShPaused banner, ported verbatim from
     docs/design/sheets/sheets.css. Neutral surface-1, no warning color
     ("a size fact, not a failure", design Cap prose). */
  .sh-paused { display: flex; align-items: center; gap: 10px; padding: 9px 14px; background: var(--surface-1); border-bottom: 1px solid var(--border-faint); flex-shrink: 0; }
  .sh-paused svg { color: var(--fg-3); flex-shrink: 0; }
  .sh-paused__t { font-size: 12.5px; color: var(--fg-2); }
  .sh-paused__t b { color: var(--fg-1); font-weight: 500; font-variant-numeric: tabular-nums; }
  .sh-paused__sub { font-size: 12.5px; color: var(--fg-4); }
  .sh-paused__sp { flex: 1; }

  .title-row {
    display: flex;
    align-items: center;
    padding: 0 32px;
    height: 48px;
    min-width: 0;
    overflow: hidden;
    border-bottom: 1px solid rgba(255,255,255,0.06);
    flex-shrink: 0;
    gap: 12px;
  }

  .title-input {
    flex: 1;
    font-size: 22px;
    font-weight: 600;
    color: #fff;
    border: none;
    outline: none;
    background: transparent;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    letter-spacing: -0.3px;
    padding: 0;
  }

  .title-input::placeholder {
    color: #52525B;
  }

  .metadata-pills-inline {
    display: flex;
    gap: 6px;
    align-items: center;
    flex-shrink: 0;
  }

  .pill-wrapper {
    position: relative;
  }

  .pill {
    font-size: 11px;
    padding: 4px 10px;
    border-radius: 10px;
    background: rgba(255,255,255,0.05);
    border: 1px solid rgba(255,255,255,0.1);
    cursor: pointer;
    transition: all 0.15s;
    color: #e0e0e0;
    font-family: inherit;
  }

  .pill:hover {
    background: rgba(255,255,255,0.08);
    border-color: rgba(255,255,255,0.15);
  }

  .folder-pill {
    font-size: 12px;
    padding: 4px 11px;
    border-radius: 999px;
    border: 0.5px solid #33333c;
    color: #9a9a93;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    white-space: nowrap;
    cursor: pointer;
    background: transparent;
  }
  .folder-pill.open {
    border-color: #00E4E4;
    color: #00E4E4;
  }
  .folder-pill-name {
    max-width: 160px;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .folder-picker-dropdown {
    /* position:fixed escapes overflow:hidden ancestors (#2205).
       top + right are set via inline style from getBoundingClientRect(). */
    position: fixed;
    width: 240px;
    background: #14141a;
    border: 0.5px solid #33333c;
    border-radius: 8px;
    padding: 5px;
    z-index: 100;
  }
  .folder-picker-header {
    font-size: 10px;
    color: #6f6f68;
    padding: 4px 8px 6px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .folder-picker-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 7px;
    font-size: 13px;
    padding: 7px 8px;
    border-radius: 6px;
    color: #ECEAE4;
    background: transparent;
    border: none;
    width: 100%;
    cursor: pointer;
    min-width: 0;
  }
  .folder-picker-item:hover {
    background: rgba(255,255,255,0.06);
  }
  .folder-picker-item.current {
    background: rgba(0,228,228,0.12);
    color: #00E4E4;
  }
  .folder-picker-item-left {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    min-width: 0;
  }
  .folder-picker-name {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .pill.priority-high {
    color: #ff6b35;
    border-color: rgba(255,107,53,0.3);
  }

  .pill.due {
    color: #f0c000;
    border-color: rgba(240,192,0,0.25);
  }

  .pill-dropdown {
    /* position:fixed escapes overflow:hidden on .title-row / .editor-wrapper / .fv-note (#2205).
       top + left are set via inline style on each instance from getBoundingClientRect(). */
    position: fixed;
    min-width: 160px;
    background: rgba(20,20,30,0.98);
    border: 1px solid rgba(255,255,255,0.15);
    border-radius: 8px;
    padding: 6px;
    z-index: 1000;
    box-shadow: 0 8px 24px rgba(0,0,0,0.6);
  }

  .pill-dropdown-option {
    display: block;
    width: 100%;
    padding: 8px 12px;
    font-size: 13px;
    color: #e0e0e0;
    background: none;
    border: none;
    border-radius: 5px;
    cursor: pointer;
    text-align: left;
    font-family: inherit;
    transition: background 0.1s;
  }

  .pill-dropdown-option:hover {
    background: rgba(255,255,255,0.1);
  }

  .pill-dropdown-option.selected {
    color: #00E4E4;
    background: rgba(0,228,228,0.12);
  }

  .pill-date-picker {
    /* position:fixed via inline style — escapes overflow:hidden ancestors (#2205) */
    z-index: 1000;
    background: #1a1a2e;
    color: #e0e0e0;
    border: 1px solid rgba(255,255,255,0.15);
    border-radius: 6px;
    padding: 6px 10px;
    font-size: 13px;
    font-family: inherit;
    outline: none;
    color-scheme: dark;
  }

  .editor-toolbar {
    display: flex;
    align-items: center;
    gap: 2px;
    padding: 8px 32px;
    border-bottom: 1px solid rgba(255,255,255,0.05);
    flex-shrink: 0;
  }

  .toolbar-btn {
    background: transparent;
    border: none;
    color: #666;
    font-size: 13px;
    padding: 5px 8px;
    border-radius: 5px;
    cursor: pointer;
    font-weight: 500;
    transition: all 0.12s;
    font-family: inherit;
    line-height: 1;
    letter-spacing: 0.02em;
    white-space: nowrap;
  }

  .toolbar-btn:hover {
    background: rgba(255,255,255,0.08);
    color: #aaa;
  }

  .toolbar-btn.active {
    color: #00E4E4;
    background: rgba(0,228,228,0.1);
  }

  .toolbar-btn.pin-btn {
    color: #555;
  }

  .toolbar-btn.pin-btn:hover {
    color: #00E4E4;
  }

  .toolbar-btn.pin-btn.pinned {
    color: #00E4E4;
  }

  .toolbar-btn.pin-btn:disabled {
    opacity: 0.3;
    cursor: not-allowed;
  }

  .toolbar-sep {
    width: 1px;
    height: 14px;
    background: rgba(255,255,255,0.06);
    margin: 0 4px;
    flex-shrink: 0;
  }

  .toolbar-select {
    background: transparent;
    color: #888;
    border: 1px solid rgba(255,255,255,0.1);
    border-radius: 4px;
    padding: 2px 4px;
    font-size: 11px;
    font-family: inherit;
    cursor: pointer;
    outline: none;
  }
  .toolbar-select:hover {
    color: #e0e0e0;
    border-color: rgba(255,255,255,0.2);
  }
  .toolbar-select option {
    background: #1a1a2e;
    color: #e0e0e0;
  }

  .toolbar-autosave {
    margin-left: auto;
    font-size: 11px;
    color: #52525B;
    font-weight: 400;
  }

  .toolbar-autosave .saving {
    color: #00E4E4;
  }

  .toolbar-autosave .saved {
    color: #52525B;
  }

  .editor-container {
    flex: 1;
    overflow-y: auto;
    padding: 20px 32px;
    background: #0a0a0f;
  }

  .backlinks-container {
    padding: 0 56px 16px;
    background: #0a0a0f;
  }

  .editor-container :global(.pane-editor) {
    outline: none;
    min-height: 100%;
    font-size: 15.5px;
    line-height: 1.7;
    color: #E4E4E7;
    font-weight: 400;
    letter-spacing: 0;
    max-width: 720px;
    margin: 0 auto;
  }

  .editor-container :global(.pane-editor > * + *) {
    margin-top: 0.75em;
  }

  .editor-container :global(.pane-editor h1) {
    font-size: 1.75em;
    font-weight: 600;
    line-height: 1.3;
    margin-top: 1.5em;
    margin-bottom: 0.5em;
    letter-spacing: -0.3px;
    color: #e0e0e0;
  }

  .editor-container :global(.pane-editor h1:first-child) {
    margin-top: 0;
  }

  .editor-container :global(.pane-editor h2) {
    font-size: 20px;
    font-weight: 600;
    line-height: 1.35;
    margin: 36px 0 16px;
    letter-spacing: -0.2px;
    color: #fff;
  }

  .editor-container :global(.pane-editor h3) {
    font-size: 1.15em;
    font-weight: 600;
    line-height: 1.4;
    margin-top: 1em;
    margin-bottom: 0.3em;
    letter-spacing: 0;
    color: #E4E4E7;
  }

  .editor-container :global(.pane-editor p.is-editor-empty:first-child::before) {
    content: attr(data-placeholder);
    float: left;
    color: #52525B;
    pointer-events: none;
    height: 0;
  }

  .editor-container :global(.pane-editor strong) {
    font-weight: 600;
  }

  .editor-container :global(.pane-editor em) {
    font-style: italic;
  }

  .editor-container :global(.pane-editor code) {
    background: rgba(0, 229, 204, 0.1);
    border: 1px solid rgba(0, 229, 204, 0.3);
    border-radius: 2px;
    padding: 2px 6px;
    font-size: 0.9em;
    font-family: 'SF Mono', 'Fira Code', monospace;
    color: #00E4E4;
  }

  .editor-container :global(.pane-editor pre) {
    background: rgba(0, 229, 204, 0.05);
    border: 1px solid rgba(0, 229, 204, 0.2);
    border-radius: 16px;
    padding: 16px 20px;
    overflow-x: auto;
    margin: 1em 0;
  }

  .editor-container :global(.pane-editor pre code) {
    background: none;
    padding: 0;
    border-radius: 0;
    border: none;
    color: #E0E0E0;
    font-size: 13px;
    line-height: 1.6;
  }

  .editor-container :global(.pane-editor blockquote) {
    border-left: 2px solid #00E4E4;
    padding-left: 16px;
    margin-left: 0;
    color: #E0E0E0;
    font-style: italic;
    background: rgba(0, 229, 204, 0.05);
    padding: 12px 0 12px 16px;
    border-radius: 0 8px 8px 0;
  }

  .editor-container :global(.pane-editor ul),
  .editor-container :global(.pane-editor ol) {
    padding-left: 24px;
  }

  .editor-container :global(.pane-editor li) {
    margin: 4px 0;
  }

  .editor-container :global(.pane-editor li p) {
    margin: 0;
  }

  .editor-container :global(.pane-editor ul[data-type="taskList"]) {
    list-style: none;
    padding-left: 0;
  }

  .editor-container :global(.pane-editor ul[data-type="taskList"] li) {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    line-height: 1.75;
  }

  .editor-container :global(.pane-editor ul[data-type="taskList"] li label) {
    margin-top: 2px;
    flex-shrink: 0;
  }

  .editor-container :global(.pane-editor ul[data-type="taskList"] li label input[type="checkbox"]) {
    -webkit-appearance: none;
    appearance: none;
    width: 16px;
    height: 16px;
    margin: 0;
    border: 1.5px solid rgba(255,255,255,0.4);
    border-radius: 3px;
    background: transparent;
    cursor: pointer;
    position: relative;
    flex-shrink: 0;
  }

  .editor-container :global(.pane-editor ul[data-type="taskList"] li label input[type="checkbox"]:hover) {
    border-color: #00E4E4;
  }

  .editor-container :global(.pane-editor ul[data-type="taskList"] li label input[type="checkbox"]:checked) {
    background: #00E4E4;
    border-color: #00E4E4;
  }

  .editor-container :global(.pane-editor ul[data-type="taskList"] li label input[type="checkbox"]:checked::after) {
    content: '';
    position: absolute;
    left: 4px;
    top: 1px;
    width: 5px;
    height: 9px;
    border: solid #0a0a0f;
    border-width: 0 2px 2px 0;
    transform: rotate(45deg);
  }

  .editor-container :global(.pane-editor ul[data-type="taskList"] li[data-checked="true"]) {
    text-decoration: line-through;
    color: rgba(0, 229, 204, 0.4);
    opacity: 0.6;
  }

  .editor-container :global(.pane-editor hr) {
    border: none;
    border-top: 1px solid transparent;
    background: linear-gradient(90deg, transparent, #00E4E4, transparent);
    height: 1px;
    margin: 2em 0;
  }

  .editor-container :global(.pane-editor a),
  .editor-container :global(.pane-editor .editor-link) {
    color: #00E4E4;
    text-decoration: none;
    text-underline-offset: 2px;
    cursor: pointer;
    padding: 1px 2px;
    border-radius: 2px;
    transition: all 0.15s ease;
  }

  .editor-container :global(.pane-editor a:hover),
  .editor-container :global(.pane-editor .editor-link:hover) {
    background: rgba(0, 229, 204, 0.1);
    box-shadow: 0 0 6px rgba(0, 229, 204, 0.3);
  }

  .editor-container :global(.pane-editor table) {
    border-collapse: collapse;
    margin: 1em 0;
    width: 100%;
    border: 1px solid rgba(0, 229, 204, 0.3);
    border-radius: 8px;
    overflow: hidden;
  }

  .editor-container :global(.pane-editor th),
  .editor-container :global(.pane-editor td) {
    border: 1px solid rgba(0, 229, 204, 0.1);
    padding: 8px 12px;
    text-align: left;
  }

  .editor-container :global(.pane-editor th) {
    background: rgba(0, 229, 204, 0.1);
    font-weight: 400;
    color: #00E4E4;
    text-transform: uppercase;
    letter-spacing: 1px;
    font-size: 12px;
  }

  /* Syntax highlighting */
  .editor-container :global(.hljs-comment),
  .editor-container :global(.hljs-quote) {
    color: var(--syntax-comment);
  }

  .editor-container :global(.hljs-keyword),
  .editor-container :global(.hljs-selector-tag),
  .editor-container :global(.hljs-addition) {
    color: var(--syntax-keyword);
  }

  .editor-container :global(.hljs-string),
  .editor-container :global(.hljs-doctag) {
    color: var(--syntax-string);
  }

  .editor-container :global(.hljs-number),
  .editor-container :global(.hljs-literal) {
    color: var(--syntax-number);
  }

  .editor-container :global(.hljs-title),
  .editor-container :global(.hljs-section),
  .editor-container :global(.hljs-built_in) {
    color: var(--syntax-function);
  }

  .editor-container :global(.hljs-attr),
  .editor-container :global(.hljs-attribute) {
    color: var(--syntax-attr);
  }

  /* Wiki Link Styles */
  .editor-container :global(.wiki-link),
  .editor-container :global(.wiki-link-text) {
    color: #00E4E4;
    text-decoration: none;
    background: rgba(0, 229, 204, 0.1);
    padding: 2px 4px;
    border: 1px solid rgba(0, 229, 204, 0.3);
    border-radius: 2px;
    cursor: pointer;
    font-weight: 400;
    transition: all 0.15s ease;
  }

  .editor-container :global(.wiki-link:hover),
  .editor-container :global(.wiki-link-text:hover) {
    background: rgba(0, 229, 204, 0.2);
    box-shadow: 0 0 6px rgba(0, 229, 204, 0.3);
    color: #FFFFFF;
  }

  /*
   * Old .attachment-* CSS blocks removed in PR2 (#401).
   * The NodeView (AttachmentNodeView.svelte) owns its own styles now.
   * PR1 design tokens (--at-card-h, --accent, --surface-*, --border-*, etc.)
   * remain in app.css — only the old component selectors are gone.
   */

  /* Linked Tasks */
  .linked-tasks-section {
    border-top: 1px solid rgba(255,255,255,0.06);
    padding: 12px 16px;
    margin-top: 8px;
  }
  .linked-tasks-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 8px;
  }
  .linked-tasks-title {
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.05em;
    color: #666666;
  }
  .add-task-btn {
    background: none;
    border: 1px solid rgba(255,255,255,0.06);
    color: #888888;
    width: 22px;
    height: 22px;
    border-radius: 4px;
    cursor: pointer;
    font-size: 14px;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 0;
    font-family: inherit;
  }
  .add-task-btn:hover {
    border-color: #00E4E4;
    color: #00E4E4;
  }
  .new-task-row {
    margin-bottom: 6px;
  }
  .new-task-input {
    width: 100%;
    background: rgba(255,255,255,0.05);
    border: 1px solid rgba(255,255,255,0.06);
    border-radius: 4px;
    padding: 6px 10px;
    color: #e0e0e0;
    font-size: 13px;
    font-family: inherit;
    outline: none;
  }
  .new-task-input:focus {
    border-color: #00E4E4;
  }
  .linked-task-row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 0;
  }
  .linked-task-row.done {
    opacity: 0.5;
  }
  .task-checkbox {
    background: none;
    border: none;
    cursor: pointer;
    padding: 0;
    display: flex;
    flex-shrink: 0;
  }
  .linked-task-title {
    font-size: 13px;
    color: #e0e0e0;
    flex: 1;
  }
  .linked-task-title.done {
    text-decoration: line-through;
    color: #666666;
  }
  .task-priority-badge {
    font-size: 10px;
    padding: 1px 6px;
    border-radius: 3px;
    font-weight: 500;
    text-transform: capitalize;
  }
  .task-priority-badge.high { background: rgba(239,68,68,0.15); color: #EF4444; }
  .task-priority-badge.urgent { background: rgba(239,68,68,0.25); color: #F87171; }
  .task-priority-badge.low { background: rgba(161,161,170,0.15); color: #888888; }
  .task-due-date {
    font-size: 11px;
    color: #666666;
  }
  .add-linked-task-hint {
    background: none;
    border: none;
    color: #52525B;
    font-size: 12px;
    cursor: pointer;
    padding: 8px 16px;
    font-family: inherit;
  }
  .add-linked-task-hint:hover {
    color: #00E4E4;
  }

  /* Autocomplete Styles */
  :global(.wiki-link-autocomplete) {
    background: var(--bg-secondary);
    border: 1px solid var(--border-color);
    border-radius: 8px;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.15);
    max-height: 200px;
    overflow-y: auto;
    z-index: 1000;
    min-width: 200px;
  }

  :global(.autocomplete-popup) {
    padding: 4px 0;
  }

  :global(.autocomplete-item) {
    padding: 8px 12px;
    cursor: pointer;
    font-size: 14px;
    color: var(--text-primary);
    border-radius: 4px;
    margin: 0 4px;
  }

  :global(.autocomplete-item:hover),
  :global(.autocomplete-item.selected) {
    background: var(--bg-hover);
  }

  /* Scrollbar styling */
  .editor-container::-webkit-scrollbar {
    width: 8px;
  }

  .editor-container::-webkit-scrollbar-track {
    background: transparent;
  }

  .editor-container::-webkit-scrollbar-thumb {
    background: rgba(255, 255, 255, 0.15);
    border-radius: 4px;
  }

  .editor-container::-webkit-scrollbar-thumb:hover {
    background: rgba(255, 255, 255, 0.25);
  }

  /* Version History Modal */
  .vh-overlay {
    position: fixed;
    top: 0; left: 0; right: 0; bottom: 0;
    background: rgba(0, 0, 0, 0.7);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 9999;
  }
  .vh-panel {
    background: #0a0a0f;
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 4px;
    width: 90%;
    max-width: 800px;
    height: 70vh;
    display: flex;
    flex-direction: column;
  }
  .vh-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 20px;
    border-bottom: 1px solid rgba(255, 255, 255, 0.05);
  }
  .vh-title {
    font-size: 15px;
    font-weight: 600;
    color: #e0e0e0;
    margin: 0;
  }
  .vh-close {
    background: none;
    border: none;
    cursor: pointer;
    padding: 4px;
    border-radius: 4px;
  }
  .vh-close:hover { background: rgba(255, 255, 255, 0.05); }
  .vh-body {
    display: flex;
    flex: 1;
    min-height: 0;
  }
  .vh-list {
    width: 220px;
    border-right: 1px solid rgba(255, 255, 255, 0.05);
    overflow-y: auto;
    padding: 8px;
  }
  .vh-empty {
    color: #666;
    font-size: 13px;
    padding: 20px 12px;
    text-align: center;
  }
  .vh-item {
    display: flex;
    flex-direction: column;
    gap: 2px;
    width: 100%;
    padding: 10px 12px;
    border: none;
    background: none;
    border-radius: 4px;
    cursor: pointer;
    text-align: left;
  }
  .vh-item:hover { background: rgba(255, 255, 255, 0.05); }
  .vh-item.active { background: rgba(0, 228, 228, 0.1); }
  .vh-date {
    font-size: 13px;
    color: #e0e0e0;
    font-weight: 400;
  }
  .vh-source {
    font-size: 11px;
    color: #00E4E4;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.3px;
  }
  .vh-preview {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .vh-preview-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 16px;
    border-bottom: 1px solid rgba(255, 255, 255, 0.05);
  }
  .vh-preview-date {
    font-size: 13px;
    color: #999;
  }
  .vh-restore-btn {
    padding: 6px 14px;
    background: #00E4E4;
    color: #0a0a0f;
    border: none;
    border-radius: 4px;
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
  }
  .vh-restore-btn:hover { opacity: 0.85; }
  .vh-preview-content {
    flex: 1;
    overflow-y: auto;
    padding: 16px;
    font-size: 13px;
    color: #ccc;
    white-space: pre-wrap;
    word-break: break-word;
    margin: 0;
    font-family: system-ui, -apple-system, sans-serif;
    line-height: 1.6;
  }
  .vh-preview-empty {
    color: #666;
    font-size: 13px;
    padding: 40px 20px;
    text-align: center;
  }

  /* Error toast (#1815) */
  .error-toast {
    position: fixed;
    bottom: 24px;
    right: 24px;
    background: rgba(239,68,68,0.12);
    border: 1px solid rgba(239,68,68,0.35);
    color: #f87171;
    padding: 10px 14px;
    border-radius: 6px;
    font-size: 13px;
    z-index: 9999;
    display: flex;
    align-items: center;
    gap: 10px;
    max-width: 380px;
    animation: errorToastIn 0.2s ease;
  }
  .error-toast-msg {
    flex: 1;
    line-height: 1.4;
  }
  .error-toast-dismiss {
    background: none;
    border: none;
    color: #f87171;
    cursor: pointer;
    font-size: 14px;
    padding: 0;
    opacity: 0.7;
    flex-shrink: 0;
  }
  .error-toast-dismiss:hover { opacity: 1; }
  @keyframes errorToastIn {
    from { opacity: 0; transform: translateY(8px); }
    to { opacity: 1; transform: translateY(0); }
  }

  /* ==========================================================
     Template UI: ⋯ menu, modals (§32.2, §32.3)
     Tokens from design: colors_and_type.css / templates.css
     ========================================================== */

  /* ⋯ wrap keeps menu in layout flow */
  /* ==========================================================
     Responsive overflow (#2123, #2135)
     When measured content overflows container, overflow group hides;
     \u22ef stays pinned right. Trigger is measurement-based (not fixed 330px).
     ========================================================== */
  .editor-toolbar.toolbar--narrow .toolbar-btn--overflow,
  .editor-toolbar.toolbar--narrow .toolbar-sep--overflow {
    display: none;
  }

  .tpl-sep {
    border: none;
    border-top: 1px solid rgba(240,237,232,0.08);
    margin: 5px 0;
  }

  /* Danger/trash item */
  .tpl-mi--danger { color: #FF4444; }
  .tpl-mi--danger svg { color: #FF4444; }
  .tpl-mi--danger:hover { background: rgba(255,68,68,0.08); color: #FF6666; }
  .tpl-mi--danger:hover svg { color: #FF6666; }
  .tpl-mi:disabled { opacity: 0.4; cursor: not-allowed; }

  .tpl-more-wrap { position: relative; flex-shrink: 0; margin-left: 2px; }

  /* ⋯ trigger button */
  .tpl-more-btn {
    height: 30px;
    width: 30px;
    border-radius: 6px;
    background: transparent;
    border: 0;
    cursor: pointer;
    color: #A0A0A0;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    transition: background 120ms, color 120ms;
  }
  .tpl-more-btn:hover,
  .tpl-more-btn--open { background: rgba(240,237,232,0.06); color: #F0EDE8; }

  /* dropdown menu */
  .tpl-menu {
    position: absolute;
    z-index: 200;
    top: 38px;
    right: 0;
    min-width: 232px;
    padding: 5px;
    background: #2C2C30;
    border: 1px solid rgba(240,237,232,0.16);
    border-radius: 8px;
    box-shadow: 0 12px 32px rgba(0,0,0,0.6), 0 2px 6px rgba(0,0,0,0.4);
  }
  .tpl-menu__label {
    padding: 8px 10px 4px;
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.07em;
    text-transform: uppercase;
    color: #5A5A5E;
  }
  .tpl-mi {
    display: flex;
    align-items: center;
    gap: 11px;
    width: 100%;
    padding: 8px 10px;
    border: 0;
    background: transparent;
    border-radius: 6px;
    font-family: var(--font-sans, system-ui);
    font-size: 13.5px;
    font-weight: 500;
    color: #C9C5BE;
    cursor: pointer;
    text-align: left;
    transition: background 120ms, color 120ms;
  }
  .tpl-mi:hover { background: rgba(240,237,232,0.03); color: #F0EDE8; }
  .tpl-mi svg { color: #A0A0A0; flex-shrink: 0; }
  .tpl-mi:hover svg { color: #C9C5BE; }
  .tpl-mi__key { margin-left: auto; font-family: var(--font-mono, monospace); font-size: 10.5px; color: #5A5A5E; }

  /* scrim: sits behind dialogs, covers the editor area */
  .tpl-scrim {
    position: fixed;
    inset: 0;
    z-index: 299;
    background: rgba(5,5,7,0.6);
    backdrop-filter: blur(3px);
  }

  /* dialog shell */
  .tpl-dialog {
    position: fixed;
    z-index: 300;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    width: 452px;
    max-width: calc(100vw - 48px);
    padding: 20px;
    background: #1A1A1C;
    border: 1px solid rgba(240,237,232,0.10);
    border-radius: 14px;
    box-shadow: 0 12px 32px rgba(0,0,0,0.6), 0 2px 6px rgba(0,0,0,0.4);
  }
  .tpl-dialog--wide { width: 500px; }

  .tpl-dialog__head {
    display: flex;
    align-items: center;
    gap: 11px;
  }
  .tpl-dialog__icon {
    width: 30px;
    height: 30px;
    border-radius: 8px;
    flex-shrink: 0;
    background: rgba(0,228,228,0.12);
    color: #00E4E4;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .tpl-dialog__icon--danger { background: rgba(239,68,68,0.12); color: #EF4444; }
  .tpl-dialog__title {
    font-size: 15px;
    font-weight: 600;
    color: #F0EDE8;
    letter-spacing: -0.01em;
    margin: 0;
    flex: 1;
  }
  .tpl-dialog__x {
    width: 28px;
    height: 28px;
    border-radius: 7px;
    border: 0;
    background: transparent;
    color: #A0A0A0;
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    transition: background 120ms, color 120ms;
  }
  .tpl-dialog__x:hover { background: rgba(240,237,232,0.03); color: #F0EDE8; }
  .tpl-dialog__body {
    font-size: 13.5px;
    line-height: 1.55;
    color: #A0A0A0;
    margin: 0;
  }
  .tpl-dialog__body strong { color: #F0EDE8; font-weight: 600; }
  .tpl-dialog__gap { height: 16px; }

  /* field label + input */
  .tpl-field-label { display: block; font-size: 12px; color: #A0A0A0; margin: 0 2px 7px; }
  .tpl-input-wrap {
    display: flex;
    align-items: center;
    gap: 9px;
    border: 1px solid rgba(240,237,232,0.10);
    border-radius: 9px;
    padding: 0 12px;
    background: #0A0A0F;
    transition: border-color 120ms, box-shadow 120ms;
  }
  .tpl-input-wrap--focus { border-color: #00E4E4; box-shadow: 0 0 0 3px rgba(0,228,228,0.12); }
  .tpl-input-wrap svg { color: #5A5A5E; flex-shrink: 0; }
  .tpl-input-wrap--focus svg { color: #00E4E4; }
  .tpl-input-wrap input {
    flex: 1;
    min-width: 0;
    background: transparent;
    border: 0;
    outline: none;
    color: #F0EDE8;
    font-family: var(--font-sans, system-ui);
    font-size: 14px;
    padding: 11px 0;
  }

  /* hint line */
  .tpl-hint-line {
    font-size: 12px;
    color: #5A5A5E;
    margin: 12px 2px 0;
    display: flex;
    align-items: center;
    gap: 7px;
  }
  .tpl-hint-line svg { flex-shrink: 0; color: #5A5A5E; }

  /* action row */
  .tpl-actions { display: flex; justify-content: flex-end; gap: 10px; }
  .tpl-btn {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    font-family: var(--font-sans, system-ui);
    font-size: 13.5px;
    font-weight: 600;
    padding: 9px 16px;
    border-radius: 9px;
    border: 1px solid transparent;
    cursor: pointer;
    transition: background 120ms, color 120ms;
  }
  .tpl-btn:active { transform: scale(0.97); }
  .tpl-btn--ghost { background: transparent; color: #C9C5BE; border-color: rgba(240,237,232,0.10); }
  .tpl-btn--ghost:hover { background: rgba(240,237,232,0.03); color: #F0EDE8; }
  .tpl-btn--primary { background: #00E4E4; color: #0A0A0F; }
  .tpl-btn--primary:hover { background: #14F0F0; }
  .tpl-btn--primary:disabled { background: rgba(0,228,228,0.28); color: rgba(10,10,15,0.5); cursor: not-allowed; }
  .tpl-btn--danger { background: #EF4444; color: #fff; }
  .tpl-btn--danger:hover { background: #f05252; }
  .tpl-btn--danger:disabled { background: rgba(239,68,68,0.35); cursor: not-allowed; }
  .tpl-enter { font-family: var(--font-mono, monospace); font-size: 10.5px; opacity: 0.7; }

  /* multi-match list */
  .tpl-match-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin: 4px 0 2px;
    max-height: 260px;
    overflow-y: auto;
  }
  .tpl-match {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 11px 13px;
    border: 1px solid rgba(240,237,232,0.10);
    border-radius: 10px;
    background: #0A0A0F;
    cursor: pointer;
    transition: border-color 120ms, background 120ms;
  }
  .tpl-match:hover { border-color: rgba(240,237,232,0.16); }
  .tpl-match--sel { border-color: #EF4444; background: rgba(239,68,68,0.06); }
  .tpl-radio {
    width: 18px;
    height: 18px;
    border-radius: 50%;
    flex-shrink: 0;
    border: 1.5px solid rgba(240,237,232,0.16);
    position: relative;
    transition: border-color 120ms;
  }
  .tpl-match--sel .tpl-radio { border-color: #EF4444; }
  .tpl-match--sel .tpl-radio::after {
    content: "";
    position: absolute;
    inset: 3px;
    border-radius: 50%;
    background: #EF4444;
  }
  .tpl-match__body { flex: 1; min-width: 0; }
  .tpl-match__name { display: block; font-size: 13.5px; font-weight: 500; color: #F0EDE8; }
  .tpl-match__meta { display: block; font-size: 12px; color: #5A5A5E; margin-top: 2px; }
  .tpl-match__badge {
    flex-shrink: 0;
    font-family: var(--font-mono, monospace);
    font-size: 10px;
    color: #EF4444;
  }

</style>

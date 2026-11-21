// Pane v9.0.0 — Unified Entity Service
// Handles both notes and tasks (unified model)

import { invoke } from '@tauri-apps/api/core';

// === Types ===

export interface NoteMeta {
  id: string;
  title: string;
  content_preview: string;
  file_path: string;
  project_id: string;
  folder_id?: string;
  note_type?: string;

  // Task fields (all optional)
  status?: TaskStatus;
  priority?: TaskPriority;
  due_date?: string;
  assignee?: string;
  completed_at?: string;
  sort_order: number;
  
  // Metadata
  word_count: number;
  pinned: boolean;
  owner_id?: string;
  tags: string[];
  
  created_at: string;
  updated_at: string;
  deleted_at?: string;
}

export interface NoteContent {
  meta: NoteMeta;
  content: string;
}

export type TaskStatus = 'cabinet' | 'todo' | 'in_progress' | 'review' | 'done';
export type TaskPriority = 'low' | 'medium' | 'high' | 'urgent';

export interface CreateNoteRequest {
  project_id: string;
  folder_id?: string;
  title: string;
  content?: string;
  tags?: string[];
  status?: TaskStatus;
  priority?: TaskPriority;
  due_date?: string;
}

export interface UpdateNoteRequest {
  note_id: string;
  title?: string;
  content?: string;
  status?: TaskStatus;
  priority?: TaskPriority;
  due_date?: string;
  assignee?: string;
  pinned?: boolean;
}

export interface SearchNotesRequest {
  query: string;
  project_id?: string;
  status?: TaskStatus;
  tags?: string[];
  has_due_date?: boolean;
  limit?: number;
}

export interface SearchResult {
  note_id: string;
  title: string;
  content_preview: string;
  project_name: string;
  folder_name?: string;
  score: number;
}

// === Note Operations ===

export async function createNote(request: CreateNoteRequest): Promise<NoteMeta> {
  return await invoke('create_note', { request });
}

export async function createTask(
  request: Omit<CreateNoteRequest, 'status'> & { status: TaskStatus }
): Promise<NoteMeta> {
  return await invoke('create_note', { request });
}

export async function getNote(noteId: string): Promise<NoteContent> {
  return await invoke('get_note', { noteId });
}

export async function getNotes(
  projectId: string,
  folderId?: string,
  includeDeleted?: boolean
): Promise<NoteMeta[]> {
  return await invoke('get_notes', {
    projectId,
    folderId,
    includeDeleted,
  });
}

export async function getTasks(
  projectId?: string,
  status?: TaskStatus,
  dueBefore?: string
): Promise<NoteMeta[]> {
  return await invoke('get_tasks', {
    projectId,
    status,
    dueBefore,
  });
}

export async function searchNotes(request: SearchNotesRequest): Promise<SearchResult[]> {
  return await invoke('search_notes', { request });
}

export async function updateNote(request: UpdateNoteRequest): Promise<NoteMeta> {
  return await invoke('update_note', { request });
}

export async function deleteNote(noteId: string): Promise<void> {
  await invoke('delete_note', { noteId });
}

// #2874 item 1, Matt's ruling 5: `restore_note` now returns
// `{ meta, content_warning }` instead of a bare `NoteMeta` — a trashed
// note's file may have been missing (rebuilt from a note_versions
// snapshot, or genuinely lost) at restore time, and that must be a
// user-facing signal, never silent. This function is currently unused by
// any screen (Trash.svelte calls `api.restoreFromTrash` instead, which
// already surfaces `content_warning` — see api.ts) but is kept in sync with
// the real backend shape.
export async function restoreNote(noteId: string): Promise<{ meta: NoteMeta; content_warning?: string }> {
  return await invoke('restore_note', { noteId });
}

export async function purgeNote(noteId: string): Promise<void> {
  await invoke('purge_note', { noteId });
}

export async function getTrash(): Promise<NoteMeta[]> {
  return await invoke('get_trash');
}

// #2874 item 2 (#3037 follow-up): `empty_trash` now returns
// `{ purged, failed }` instead of a bare count — kept in sync with the
// real backend shape (see `api.ts`'s `emptyTrash`, which is the one this
// screen actually calls); this function is currently unused by any screen.
export async function emptyTrash(): Promise<{ purged: number; failed: number }> {
  return await invoke('empty_trash');
}

export async function reorderNotes(noteOrders: [string, number][]): Promise<void> {
  await invoke('reorder_notes', { noteOrders });
}

// === Helper Functions ===

export function isTask(note: NoteMeta): boolean {
  // Under NOT NULL DEFAULT 'cabinet' schema: every note has a non-null status.
  // A task is a note whose status is a workflow value (not 'cabinet').
  // See ADR-0003 / core-data-model.md §1.1.
  return note.status !== undefined && note.status !== null && note.status !== 'cabinet';
}

export function isNote(note: NoteMeta): boolean {
  return !isTask(note);
}

export function filterTasks(notes: NoteMeta[]): NoteMeta[] {
  return notes.filter(isTask);
}

export function filterNotes(notes: NoteMeta[]): NoteMeta[] {
  return notes.filter(isNote);
}

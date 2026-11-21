/**
 * api.checklist.test.ts — unit tests for the Checklist Item API layer (§31.3)
 *
 * Tests cover:
 *  - createChecklistItem
 *  - listChecklistItems
 *  - updateChecklistItem
 *  - deleteChecklistItem
 *
 * These functions wrap the Tauri `create_subtask`, `get_subtasks_for_note`,
 * `update_subtask`, and `delete_subtask` commands via the mock in vitest-setup.ts.
 */

import { describe, it, expect, beforeEach } from 'vitest';
import {
  createChecklistItem,
  listChecklistItems,
  updateChecklistItem,
  deleteChecklistItem,
} from '$lib/api';
import { mockData } from '../../vitest-setup';

const NOTE_ID = 'note-checklist-test';

// ── Helpers ────────────────────────────────────────────────────────────────

function resetSubtasks() {
  mockData.subtasks = [];
  mockData.nextId = 1;
}

// ── createChecklistItem ────────────────────────────────────────────────────

describe('createChecklistItem', () => {
  beforeEach(resetSubtasks);

  it('creates an item with text and returns a ChecklistItem', async () => {
    const item = await createChecklistItem(NOTE_ID, 'Buy milk');
    expect(item.text).toBe('Buy milk');
    expect(item.note_id).toBe(NOTE_ID);
    expect(item.done).toBe(false);
    expect(item.due_date).toBeNull();
    expect(item.id).toBeTruthy();
  });

  it('persists due_date when provided', async () => {
    const item = await createChecklistItem(NOTE_ID, 'Submit report', '2026-08-15');
    expect(item.due_date).toBe('2026-08-15');
  });

  it('uses provided sortOrder', async () => {
    const item = await createChecklistItem(NOTE_ID, 'Third', undefined, 3);
    expect(item.sort_order).toBe(3);
  });

  it('defaults sort_order to 0 when omitted', async () => {
    const item = await createChecklistItem(NOTE_ID, 'No order');
    expect(item.sort_order).toBe(0);
  });
});

// ── listChecklistItems ─────────────────────────────────────────────────────

describe('listChecklistItems', () => {
  beforeEach(resetSubtasks);

  it('returns empty array for a note with no items', async () => {
    const items = await listChecklistItems(NOTE_ID);
    expect(items).toEqual([]);
  });

  it('returns only items belonging to the requested note', async () => {
    await createChecklistItem(NOTE_ID, 'A');
    await createChecklistItem('other-note', 'B');
    const items = await listChecklistItems(NOTE_ID);
    expect(items).toHaveLength(1);
    expect(items[0].text).toBe('A');
  });

  it('returns items sorted by sort_order ascending', async () => {
    await createChecklistItem(NOTE_ID, 'Third', undefined, 3);
    await createChecklistItem(NOTE_ID, 'First',  undefined, 1);
    await createChecklistItem(NOTE_ID, 'Second', undefined, 2);
    const items = await listChecklistItems(NOTE_ID);
    expect(items.map(i => i.text)).toEqual(['First', 'Second', 'Third']);
  });

  it('returns empty array when noteId is falsy', async () => {
    const items = await listChecklistItems('');
    expect(items).toEqual([]);
  });
});

// ── updateChecklistItem ────────────────────────────────────────────────────

describe('updateChecklistItem', () => {
  beforeEach(resetSubtasks);

  it('updates text', async () => {
    const created = await createChecklistItem(NOTE_ID, 'Original text');
    const updated = await updateChecklistItem(created.id, { text: 'Updated text' });
    expect(updated.text).toBe('Updated text');
  });

  it('toggles done to true and sets completed_at', async () => {
    const created = await createChecklistItem(NOTE_ID, 'Task');
    const updated = await updateChecklistItem(created.id, { done: true });
    expect(updated.done).toBe(true);
    expect(updated.completed_at).toBeTruthy();
  });

  it('toggles done to false and clears completed_at', async () => {
    const created = await createChecklistItem(NOTE_ID, 'Task');
    const checked = await updateChecklistItem(created.id, { done: true });
    expect(checked.done).toBe(true);
    const unchecked = await updateChecklistItem(checked.id, { done: false });
    expect(unchecked.done).toBe(false);
    expect(unchecked.completed_at).toBeNull();
  });

  it('updates due_date', async () => {
    const created = await createChecklistItem(NOTE_ID, 'With due date');
    const updated = await updateChecklistItem(created.id, { dueDate: '2026-09-01' });
    expect(updated.due_date).toBe('2026-09-01');
  });

  it('clears due_date when set to null', async () => {
    const created = await createChecklistItem(NOTE_ID, 'Has date', '2026-09-01');
    const updated = await updateChecklistItem(created.id, { dueDate: null });
    expect(updated.due_date).toBeNull();
  });

  it('updates sort_order', async () => {
    const created = await createChecklistItem(NOTE_ID, 'Item', undefined, 1);
    const updated = await updateChecklistItem(created.id, { sortOrder: 5 });
    expect(updated.sort_order).toBe(5);
  });
});

// ── deleteChecklistItem ────────────────────────────────────────────────────

describe('deleteChecklistItem', () => {
  beforeEach(resetSubtasks);

  it('removes the item so it no longer appears in list', async () => {
    const created = await createChecklistItem(NOTE_ID, 'To delete');
    await deleteChecklistItem(created.id);
    const items = await listChecklistItems(NOTE_ID);
    expect(items.find(i => i.id === created.id)).toBeUndefined();
  });

  it('does not remove other items', async () => {
    const a = await createChecklistItem(NOTE_ID, 'Keep me');
    const b = await createChecklistItem(NOTE_ID, 'Delete me');
    await deleteChecklistItem(b.id);
    const items = await listChecklistItems(NOTE_ID);
    expect(items).toHaveLength(1);
    expect(items[0].id).toBe(a.id);
  });
});

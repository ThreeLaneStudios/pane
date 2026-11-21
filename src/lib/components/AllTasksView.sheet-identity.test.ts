/**
 * AllTasksView.sheet-identity.test.ts — #2874 item 15 regression coverage.
 *
 * Per the item-15 scope audit (§1 inventory, row 5), `handleCardClick` was
 * traced and found to be ALREADY SAFE: it calls `api.readNote()` (which
 * itself spreads the backend meta — see api.ts's `readNote`) and then merges
 * task fields via `{ ...noteContent.meta, ...taskFields }`, a spread-first
 * pattern that preserves every field the backend sends, including
 * `note_type`. This file exists to REGRESSION-GUARD that finding: the PR's
 * correctness claim is "sheet identity survives every open path," which
 * requires proving the ALREADY-safe paths stay safe too, not just fixing the
 * one broken site (KanbanBoard.svelte, covered by
 * KanbanBoard.sheet-identity.test.ts).
 *
 * Source-text-extraction / regex assertions, matching the house pattern for
 * this heavy Tauri/store-dependent component — see
 * AllTasksView.sheet-card-face.test.ts's own docstring on why full mounts
 * aren't used for this file's class of assertion.
 */

import { describe, it, expect } from 'vitest';
import { readFileSync } from 'fs';
import { resolve } from 'path';

const COMPONENT_PATH = resolve(__dirname, 'AllTasksView.svelte');
const SOURCE = readFileSync(COMPONENT_PATH, 'utf-8');

describe('AllTasksView — sheet identity on open (item 15)', () => {
  it('handleCardClick spreads the FULL backend meta from api.readNote before merging task fields — note_type is not hand-picked', () => {
    const fnMatch = SOURCE.match(/async function handleCardClick\([\s\S]*?\n  \}/);
    expect(fnMatch).toBeTruthy();
    const body = fnMatch![0];
    // Red-on-revert: if this ever becomes a hand-built `{ id: ..., title: ... }`
    // object instead of a spread, note_type silently drops again — the exact
    // bug class this whole PR fixes.
    expect(body).toMatch(/await api\.readNote\(/);
    expect(body).toMatch(/noteContent\.meta = \{ \.\.\.noteContent\.meta, \.\.\.taskFields \};?/);
    // Must NOT hand-construct a meta object field-by-field in this function.
    expect(body).not.toMatch(/meta:\s*\{\s*\n\s*id:/);
  });

  it('the editor panel derives isActiveSheet from $activeNote.meta.note_type and threads noteMeta through a spread (not a hand-built object)', () => {
    expect(SOURCE).toMatch(/const isActiveSheet = \$activeNote\.meta\?\.note_type === 'sheet'/);
    const noteMetaMatch = SOURCE.match(/\{@const noteMeta = \{[\s\S]*?\}\}/);
    expect(noteMetaMatch).toBeTruthy();
    // Spread-first: `...$activeNote.meta` must appear before any field overrides,
    // so note_type (an untouched field) survives automatically.
    expect(noteMetaMatch![0]).toMatch(/\.\.\.\$activeNote\.meta,/);
  });

  it('Editor receives noteMeta (which carries note_type) as a prop — Editor.svelte is the sole place that derives isSheet and swaps in SheetGrid', () => {
    expect(SOURCE).toMatch(/<Editor\s*\n[\s\S]*?noteMeta=\{noteMeta\}/);
  });
});

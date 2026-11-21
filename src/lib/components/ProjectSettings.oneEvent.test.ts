/**
 * ProjectSettings.svelte — #2972 stage 3 (`docs/spec/data-refresh.md`
 * §34.1's "one event per change", item 5).
 *
 * The backend `update_project_settings` command (`ai/commands.rs`)
 * already emits the single data-changed event itself, unconditionally
 * (`emit_data_changed(&app, &project_id, ENTITY_PROJECT, REASON_EDITED)`,
 * #2972 stage 1). `ProjectSettings.svelte`'s save handler used to ALSO
 * `emit('pane-project-changed', ...)` on the frontend side for the exact
 * same mutation — a second, redundant emit. Stage 3 deletes it outright
 * (not converts it, per item 5's explicit instruction) rather than
 * reimplement it as a `dispatchDataChanged` call, since the backend
 * already covers it.
 *
 * Source-scan, not a mount test (same convention this codebase already
 * uses for a backend equivalent — `src-tauri/src/projects.rs`'s
 * `test_create_folder_command_wrapper_emits_data_changed_exactly_once`):
 * fully mounting ProjectSettings in edit mode requires faking four
 * separate onMount load calls (settings/pinned-notes/budget/memory) for a
 * save-flow assertion this file's own pre-existing test docstring already
 * disclaims as "NOT tested here (live DMG tests)" — a direct source
 * assertion is the precise, fast proof that the specific duplicate-emit
 * call site is gone, without taking on that unrelated mounting cost.
 *
 * RED-ON-REVERT: if a future change re-adds
 *   await emit('pane-project-changed', ...)
 * (or any `@tauri-apps/api/event` emit / `dispatchDataChanged` call)
 * inside this file's save handler, this test fails — a project-settings
 * save would then fire twice (once from the backend it already calls via
 * `invoke('update_project_settings', ...)`, once more from here).
 */

import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const __dirname = dirname(fileURLToPath(import.meta.url));
const SOURCE_PATH = join(__dirname, 'ProjectSettings.svelte');

describe('ProjectSettings.svelte — exactly one event per project-settings save (item 5)', () => {
  it('contains no `emit(` call anywhere (the duplicate pane-project-changed emit is deleted, not migrated)', () => {
    const source = readFileSync(SOURCE_PATH, 'utf-8');
    // Deliberately a broad "no emit(" scan, not scoped to the one
    // `handleSave` function body: there is no OTHER legitimate reason for
    // this file to emit a Tauri event at all (its only backend write is
    // `invoke('update_project_settings', ...)`, already covered §34.1.1).
    expect(source).not.toMatch(/\bemit\(/);
  });

  it("imports no `emit` from '@tauri-apps/api/event' (the import is dead without a call site; dead code is deleted, not left unused)", () => {
    const source = readFileSync(SOURCE_PATH, 'utf-8');
    expect(source).not.toMatch(/from\s+['"]@tauri-apps\/api\/event['"]/);
  });

  it('still calls update_project_settings via invoke exactly once in the save handler (the backend IS the sole remaining sender)', () => {
    const source = readFileSync(SOURCE_PATH, 'utf-8');
    const fnStart = source.indexOf('async function handleSave()');
    expect(fnStart).toBeGreaterThan(-1);
    const afterFn = source.slice(fnStart);
    const bodyEndRelative = afterFn.indexOf("\n  }\n");
    const body = afterFn.slice(0, bodyEndRelative === -1 ? undefined : bodyEndRelative);
    const occurrences = body.match(/invoke\(\s*['"]update_project_settings['"]/g) ?? [];
    expect(occurrences).toHaveLength(1);
  });
});

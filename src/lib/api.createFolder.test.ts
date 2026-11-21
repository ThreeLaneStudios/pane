/**
 * api.createFolder — #2972 stage 3 (`docs/spec/data-refresh.md` §34.1.2,
 * item 5: "one event per change").
 *
 * The backend `create_folder` Tauri command (`projects.rs`) already emits
 * the single data-changed event itself, unconditionally, since #2972
 * stage 1 (proven backend-side by `test_create_folder_command_wrapper_
 * emits_data_changed_exactly_once` / `test_sidebar_create_folder_path_
 * fires_data_changed_exactly_once`, src-tauri/src/projects.rs +
 * ai/commands.rs). This file proves the OTHER half of "exactly one event
 * for New Folder": the frontend wrapper must NOT also dispatch one —
 * together, backend-fires-once + frontend-fires-zero = exactly one total.
 *
 * RED-ON-REVERT: if a future change adds a
 *   window.dispatchEvent(new CustomEvent('pane-data-changed', ...))
 * (or any `dispatchDataChanged(...)` call) inside `createFolder()`, the
 * second assertion below fails — New Folder would then fire twice.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { createFolder } from '$lib/api';
import * as tauriCore from '@tauri-apps/api/core';

describe('api.createFolder — zero app-side dispatch (item 5)', () => {
  let invokeSpy: ReturnType<typeof vi.spyOn>;
  let dispatchSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    invokeSpy = vi.spyOn(tauriCore, 'invoke').mockResolvedValue({
      id: 'folder-1', name: 'New Folder', parent_id: 'project-1',
    });
    dispatchSpy = vi.spyOn(window, 'dispatchEvent');
  });

  afterEach(() => {
    invokeSpy.mockRestore();
    dispatchSpy.mockRestore();
  });

  it('calls the create_folder Tauri command exactly once', async () => {
    await createFolder('project-1', 'New Folder');
    expect(invokeSpy).toHaveBeenCalledTimes(1);
    expect(invokeSpy).toHaveBeenCalledWith('create_folder', {
      request: { project_id: 'project-1', name: 'New Folder', parent_id: null },
    });
  });

  it('dispatches NO pane-data-changed event on the app side — the backend command is the sole sender', async () => {
    await createFolder('project-1', 'New Folder');

    const dataChangedDispatches = dispatchSpy.mock.calls
      .map((c) => c[0])
      .filter((e) => e instanceof CustomEvent && (e as CustomEvent).type === 'pane-data-changed');
    expect(dataChangedDispatches).toHaveLength(0);
  });
});

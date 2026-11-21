/**
 * ProjectSettings.defaultFolderDropdown.test.ts — #2874 item 9, Lane 1 PR 3.
 *
 * Matt's ruling: "Project Settings gets a default-folder dropdown," sourced
 * from `get_folders` excluding system and hidden folders (ruling 3), and a
 * selection persists via `update_project`'s new `default_folder_id` field.
 *
 * Shown failing before the fix: before this PR, `ProjectSettings.svelte`
 * never called `get_folders`/`get_hidden_folder_ids` at all and had no
 * "Default folder" label/select anywhere in its markup — the first
 * assertion below (`getByText('Default folder')`) fails outright against
 * pre-fix code with "Unable to find an element with the text: Default
 * folder."
 */

import { describe, it, expect, vi, beforeAll, beforeEach, afterEach } from 'vitest';
import { render, screen, waitFor, fireEvent } from '@testing-library/svelte/svelte5';
import ProjectSettings from './ProjectSettings.svelte';

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: invokeMock,
}));

vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(),
  listen: vi.fn().mockResolvedValue(() => {}),
}));

vi.mock('$lib/api', async (importOriginal) => {
  const original = (await importOriginal()) as any;
  return {
    ...original,
    listProjects: vi.fn().mockResolvedValue([]),
  };
});

const PROJECT_ID = 'proj-1';
const GENERAL_FOLDER = { id: 'folder-general', name: 'General', parent_id: PROJECT_ID, is_system: false, sort_order: 0, created_at: '', updated_at: '' };
const WORK_FOLDER = { id: 'folder-work', name: 'Work', parent_id: PROJECT_ID, is_system: false, sort_order: 1, created_at: '', updated_at: '' };
const SYSTEM_FOLDER = { id: 'folder-system', name: 'memory', parent_id: PROJECT_ID, is_system: true, sort_order: 2, created_at: '', updated_at: '' };
const HIDDEN_FOLDER = { id: 'folder-hidden', name: 'Archive', parent_id: PROJECT_ID, is_system: false, sort_order: 3, created_at: '', updated_at: '' };

function installInvokeMock() {
  invokeMock.mockImplementation(async (cmd: string, args?: any) => {
    switch (cmd) {
      case 'list_categories':
        return [];
      case 'get_project_settings':
        return {
          id: PROJECT_ID,
          name: 'Test Project',
          color: null,
          category: null,
          is_favorite: false,
          description: '',
        };
      case 'get_project_or_folder':
        return {
          id: PROJECT_ID,
          name: 'Test Project',
          description: '',
          color: null,
          parent_id: null,
          sort_order: 0,
          created_at: '',
          updated_at: '',
          category: null,
          is_system: false,
          is_favorite: false,
          default_folder_id: GENERAL_FOLDER.id,
        };
      case 'get_folders':
        return [GENERAL_FOLDER, WORK_FOLDER, SYSTEM_FOLDER, HIDDEN_FOLDER];
      case 'get_hidden_folder_ids':
        return [HIDDEN_FOLDER.id];
      case 'list_context_pins':
        return [];
      case 'get_context_budget_usage':
        return null;
      case 'get_project_memory':
        return { exists: false, content: '', word_count: 0, token_estimate: 0, id: '' };
      case 'update_project':
        return {
          id: PROJECT_ID,
          name: 'Test Project',
          default_folder_id: args?.request?.default_folder_id ?? GENERAL_FOLDER.id,
        };
      case 'update_project_settings':
        return null;
      default:
        return null;
    }
  });
}

beforeAll(() => {
  const win = globalThis.window as any;
  if (!win.addEventListener) {
    win.addEventListener = globalThis.addEventListener?.bind(globalThis) ?? (() => {});
  }
  if (!win.removeEventListener) {
    win.removeEventListener = globalThis.removeEventListener?.bind(globalThis) ?? (() => {});
  }
  if (!win.document) {
    win.document = globalThis.document;
  }
});

beforeEach(() => {
  installInvokeMock();
});

afterEach(() => {
  vi.clearAllMocks();
});

describe('ProjectSettings — default-folder dropdown (#2874 item 9)', () => {
  it('lists folders from get_folders, excluding system and hidden folders', async () => {
    render(ProjectSettings, { projectId: PROJECT_ID });

    await waitFor(() => {
      expect(screen.getByText('Default folder')).toBeTruthy();
    });

    const select = await waitFor(() => {
      const el = screen.getByText('Default folder').closest('.input-group')?.querySelector('select');
      expect(el).toBeTruthy();
      return el as HTMLSelectElement;
    });

    const optionLabels = Array.from(select.querySelectorAll('option')).map((o) => o.textContent);
    expect(optionLabels).toContain('General');
    expect(optionLabels).toContain('Work');
    // Ruling 3: system folder and hidden folder must NOT be offered.
    expect(optionLabels).not.toContain('memory');
    expect(optionLabels).not.toContain('Archive');
  });

  it('pre-selects the project\'s CURRENT default folder', async () => {
    render(ProjectSettings, { projectId: PROJECT_ID });

    const select = await waitFor(() => {
      const el = screen.getByText('Default folder').closest('.input-group')?.querySelector('select') as HTMLSelectElement;
      expect(el.value).toBe(GENERAL_FOLDER.id);
      return el;
    });
    expect(select.value).toBe(GENERAL_FOLDER.id);
  });

  it('selecting a different folder and saving persists it via update_project', async () => {
    render(ProjectSettings, { projectId: PROJECT_ID });

    const select = await waitFor(() => {
      const el = screen.getByText('Default folder').closest('.input-group')?.querySelector('select') as HTMLSelectElement;
      expect(el).toBeTruthy();
      return el;
    });

    await fireEvent.change(select, { target: { value: WORK_FOLDER.id } });

    const saveButton = await waitFor(() => {
      const btn = screen.getByText('Save All Changes');
      expect(btn).toBeTruthy();
      return btn;
    });
    await fireEvent.click(saveButton);

    await waitFor(() => {
      const updateCall = invokeMock.mock.calls.find(
        (c) => c[0] === 'update_project' && c[1]?.request?.default_folder_id === WORK_FOLDER.id
      );
      expect(updateCall).toBeTruthy();
    });
  });
});

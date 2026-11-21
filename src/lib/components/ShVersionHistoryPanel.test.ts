// ShVersionHistoryPanel.test.ts — component tests for the sheet-native
// version-history panel (#2339 PR 15b-beta). The single most important
// test in this file is the one proving the sheet restore path calls
// `restore_sheet_snapshot` and NEVER `restore_note_version` — a mixup
// here would silently corrupt a sheet note (see `api.ts`'s
// `restoreSheetSnapshot` doc comment).

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte/svelte5';
import ShVersionHistoryPanel from './ShVersionHistoryPanel.svelte';

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: invokeMock,
}));

// vitest-setup.ts replaces `globalThis.window` with a plain mock object
// lacking a `document` reference — `waitFor` (used throughout this file for
// the async fetch-on-mount flow) needs `window.document` to resolve its
// default container. Same patch as ChatPanel.test.ts.
if (!(globalThis.window as any).document) {
  (globalThis.window as any).document = globalThis.document;
}

function sheetContent(overrides: Record<string, unknown> = {}) {
  return JSON.stringify({
    v: 1,
    columns: [{ id: 'col-a', name: 'Line item', col_type: 'text', options_json: '{}', width: 164, sort_order: 0 }],
    rows: [{ id: 'row-1', sort_order: 0, height: 32 }],
    cells: [{ row_id: 'row-1', column_id: 'col-a', value: 'Essay', style_json: '{}' }],
    ...overrides,
  });
}

const VERSIONS = [
  { id: 'v-2', note_id: 'note-1', created_at: '2026-08-20T16:31:00Z', source: 'sheet_snapshot' },
  { id: 'v-1', note_id: 'note-1', created_at: '2026-08-19T09:02:00Z', source: 'pre_restore' },
];

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockImplementation(async (cmd: string, args: any) => {
    if (cmd === 'get_note_versions') return VERSIONS;
    if (cmd === 'get_note_version') {
      const v = VERSIONS.find((x) => x.id === args.versionId);
      return { ...v, content: sheetContent() };
    }
    if (cmd === 'get_sheet_snapshot_computed_values') return {};
    if (cmd === 'restore_sheet_snapshot') return { note_id: args.noteId, columns: [], rows: [], cells: [] };
    if (cmd === 'restore_note_version') throw new Error('restore_note_version must NEVER be called for a sheet');
    throw new Error(`unexpected invoke: ${cmd}`);
  });
});

describe('ShVersionHistoryPanel — loading', () => {
  it('fetches versions via get_note_versions on mount', async () => {
    render(ShVersionHistoryPanel, { props: { noteId: 'note-1', onClose: vi.fn(), onRestored: vi.fn() } });
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith('get_note_versions', { noteId: 'note-1' });
    });
  });

  it('renders one row per version with its source label', async () => {
    render(ShVersionHistoryPanel, { props: { noteId: 'note-1', onClose: vi.fn(), onRestored: vi.fn() } });
    await waitFor(() => {
      expect(screen.getAllByTestId('sheet-version-history-item')).toHaveLength(2);
    });
    const labels = screen.getAllByTestId('sheet-src-label');
    expect(labels[0].textContent).toContain('Sheet edit');
    expect(labels[1].textContent).toContain('Before restore');
  });

  it('auto-previews the first (newest) version', async () => {
    render(ShVersionHistoryPanel, { props: { noteId: 'note-1', onClose: vi.fn(), onRestored: vi.fn() } });
    await waitFor(() => {
      expect(screen.getByTestId('sheet-snapshot-preview')).toBeTruthy();
    });
  });

  it('shows "N columns · M rows · K computed" from the previewed envelope', async () => {
    render(ShVersionHistoryPanel, { props: { noteId: 'note-1', onClose: vi.fn(), onRestored: vi.fn() } });
    await waitFor(() => {
      expect(screen.getByTestId('sheet-version-history-panel').textContent).toContain('1 columns · 1 rows · 0 computed');
    });
  });

  it('clicking a different version fetches and previews it', async () => {
    render(ShVersionHistoryPanel, { props: { noteId: 'note-1', onClose: vi.fn(), onRestored: vi.fn() } });
    await waitFor(() => expect(screen.getAllByTestId('sheet-version-history-item')).toHaveLength(2));
    invokeMock.mockClear();
    await fireEvent.click(screen.getAllByTestId('sheet-version-history-item')[1]);
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith('get_note_version', { versionId: 'v-1' });
    });
  });

  it('previewVersionContent fetches both get_note_version and get_sheet_snapshot_computed_values for the same versionId (item 32, #2874)', async () => {
    render(ShVersionHistoryPanel, { props: { noteId: 'note-1', onClose: vi.fn(), onRestored: vi.fn() } });
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith('get_note_version', { versionId: 'v-2' });
    });
    expect(invokeMock).toHaveBeenCalledWith('get_sheet_snapshot_computed_values', { versionId: 'v-2' });
  });

  it('a getSheetSnapshotComputedValues failure does not block the raw preview from rendering (item 32, #2874)', async () => {
    invokeMock.mockImplementation(async (cmd: string, args: any) => {
      if (cmd === 'get_note_versions') return VERSIONS;
      if (cmd === 'get_note_version') {
        const v = VERSIONS.find((x) => x.id === args.versionId);
        return { ...v, content: sheetContent() };
      }
      if (cmd === 'get_sheet_snapshot_computed_values') throw new Error('backend compute failed');
      throw new Error(`unexpected invoke: ${cmd}`);
    });
    render(ShVersionHistoryPanel, { props: { noteId: 'note-1', onClose: vi.fn(), onRestored: vi.fn() } });
    await waitFor(() => {
      expect(screen.getByTestId('sheet-snapshot-preview')).toBeTruthy();
    });
  });

  it('out-of-order computed-values response does not clobber a newer version\'s preview (item 32, #2874)', async () => {
    let resolveSlowComputed: ((v: Record<string, string>) => void) | null = null;
    let v2ComputeCallCount = 0;
    invokeMock.mockImplementation(async (cmd: string, args: any) => {
      if (cmd === 'get_note_versions') return VERSIONS;
      if (cmd === 'get_note_version') {
        const v = VERSIONS.find((x) => x.id === args.versionId);
        return { ...v, content: sheetContent() };
      }
      if (cmd === 'get_sheet_snapshot_computed_values') {
        if (args.versionId === 'v-2') {
          v2ComputeCallCount++;
          // The mount-time auto-preview's v-2 call (the 1st) resolves
          // immediately so the panel finishes loading; a LATER re-click
          // on v-2 (the 2nd call) is the one held open — it must lose the
          // race to v-1's (immediately resolving) click fetch.
          if (v2ComputeCallCount >= 2) {
            return new Promise((resolve) => {
              resolveSlowComputed = resolve;
            });
          }
        }
        return {};
      }
      throw new Error(`unexpected invoke: ${cmd}`);
    });

    render(ShVersionHistoryPanel, { props: { noteId: 'note-1', onClose: vi.fn(), onRestored: vi.fn() } });
    await waitFor(() => expect(screen.getAllByTestId('sheet-version-history-item')).toHaveLength(2));
    await waitFor(() => expect(screen.getByTestId('sheet-snapshot-preview')).toBeTruthy());

    // Re-click v-2 (index 0) — this is the 2nd v-2 compute call, held open.
    await fireEvent.click(screen.getAllByTestId('sheet-version-history-item')[0]);
    // Immediately click v-1 (index 1) — its compute fetch resolves right away.
    await fireEvent.click(screen.getAllByTestId('sheet-version-history-item')[1]);
    await waitFor(() => {
      const items = screen.getAllByTestId('sheet-version-history-item');
      expect(items[1].className).toContain('is-on');
    });

    // Now let the stale re-click's compute fetch resolve — it must be ignored.
    resolveSlowComputed?.({ 'stale:cell': '999' });
    await new Promise((r) => setTimeout(r, 0));

    // The preview should still reflect v-1 (the second, newer click).
    const items = screen.getAllByTestId('sheet-version-history-item');
    expect(items[1].className).toContain('is-on');
    expect(items[0].className).not.toContain('is-on');
  });

  it('shows an empty-list message when there are no versions', async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'get_note_versions') return [];
      throw new Error(`unexpected invoke: ${cmd}`);
    });
    render(ShVersionHistoryPanel, { props: { noteId: 'note-1', onClose: vi.fn(), onRestored: vi.fn() } });
    await waitFor(() => {
      expect(screen.getByText('No versions saved yet')).toBeTruthy();
    });
  });
});

describe('ShVersionHistoryPanel — restore (the critical path)', () => {
  it('Restore calls restore_sheet_snapshot with the previewed version id, and NEVER restore_note_version', async () => {
    const onRestored = vi.fn();
    const onClose = vi.fn();
    render(ShVersionHistoryPanel, { props: { noteId: 'note-1', onClose, onRestored } });
    await waitFor(() => expect(screen.getByTestId('sheet-version-history-restore')).toBeTruthy());

    await fireEvent.click(screen.getByTestId('sheet-version-history-restore'));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith('restore_sheet_snapshot', { noteId: 'note-1', versionId: 'v-2' });
    });
    expect(invokeMock).not.toHaveBeenCalledWith('restore_note_version', expect.anything());
    // Also assert across ALL calls made during this test, not just the
    // asserted one — belt-and-suspenders against a future refactor
    // sneaking a second call in.
    const calledCommands = invokeMock.mock.calls.map((c) => c[0]);
    expect(calledCommands).not.toContain('restore_note_version');
  });

  it('fires onRestored then onClose after a successful restore', async () => {
    const onRestored = vi.fn();
    const onClose = vi.fn();
    render(ShVersionHistoryPanel, { props: { noteId: 'note-1', onClose, onRestored } });
    await waitFor(() => expect(screen.getByTestId('sheet-version-history-restore')).toBeTruthy());
    await fireEvent.click(screen.getByTestId('sheet-version-history-restore'));
    await waitFor(() => {
      expect(onRestored).toHaveBeenCalledOnce();
      expect(onClose).toHaveBeenCalledOnce();
    });
  });

  it('does not call onRestored/onClose if the restore call rejects', async () => {
    invokeMock.mockImplementation(async (cmd: string, args: any) => {
      if (cmd === 'get_note_versions') return VERSIONS;
      if (cmd === 'get_note_version') {
        const v = VERSIONS.find((x) => x.id === args.versionId);
        return { ...v, content: sheetContent() };
      }
      if (cmd === 'restore_sheet_snapshot') throw new Error('backend rejected restore');
      throw new Error(`unexpected invoke: ${cmd}`);
    });
    const onRestored = vi.fn();
    const onClose = vi.fn();
    render(ShVersionHistoryPanel, { props: { noteId: 'note-1', onClose, onRestored } });
    await waitFor(() => expect(screen.getByTestId('sheet-version-history-restore')).toBeTruthy());
    await fireEvent.click(screen.getByTestId('sheet-version-history-restore'));
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith('restore_sheet_snapshot', expect.anything());
    });
    expect(onRestored).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
  });

  it('close button calls onClose without ever touching restore', async () => {
    const onClose = vi.fn();
    render(ShVersionHistoryPanel, { props: { noteId: 'note-1', onClose, onRestored: vi.fn() } });
    await waitFor(() => expect(screen.getByTestId('sheet-version-history-close')).toBeTruthy());
    await fireEvent.click(screen.getByTestId('sheet-version-history-close'));
    expect(onClose).toHaveBeenCalledOnce();
    expect(invokeMock.mock.calls.map((c) => c[0])).not.toContain('restore_sheet_snapshot');
    expect(invokeMock.mock.calls.map((c) => c[0])).not.toContain('restore_note_version');
  });
});

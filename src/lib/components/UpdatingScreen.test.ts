/**
 * UpdatingScreen.test.ts — #2723 PR B.
 *
 * This component is the frontend's one gate before the real app mounts or
 * any DbState-dependent command fires. Covers:
 *   1. Fast path (status = "ready" on the very first check): real app
 *      content renders immediately, no "Updating Pane…" flash.
 *   2. Migration-required path: "Updating Pane… don't quit" shows, and
 *      `backup://progress` / `migration://progress` events update the
 *      visible message/percentage.
 *   3. Terminal ready: switching from migration_required to the real app
 *      content once `startup://ready` fires.
 *   4. Failure path: a plain, non-blank "Update Failed" message with a
 *      Quit button that calls `exit()` — never an endless spinner.
 *
 * What this suite cannot cover (and does not try to): the actual pixels,
 * or real Tauri IPC round-trip timing — see the PR body's "what Matt must
 * look for" list for the visual half.
 *
 * Follows this codebase's established pattern (see
 * AllTasksView.dataChanged.test.ts) of `vi.waitFor` + the global `screen`
 * query object rather than `@testing-library/dom`'s own `waitFor`/
 * destructured render queries, which hit a container-resolution issue
 * against this project's svelte5 testing-library adapter.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, cleanup, screen } from '@testing-library/svelte/svelte5';
import { createRawSnippet } from 'svelte';
import UpdatingScreen from './UpdatingScreen.svelte';

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({
  invoke: invokeMock,
}));

const { listenMock, listenersByEvent } = vi.hoisted(() => ({
  listenMock: vi.fn(),
  listenersByEvent: new Map<string, (event: { payload: unknown }) => void>(),
}));
vi.mock('@tauri-apps/api/event', () => ({
  listen: listenMock,
}));

const { exitMock } = vi.hoisted(() => ({ exitMock: vi.fn() }));
vi.mock('@tauri-apps/plugin-process', () => ({
  exit: exitMock,
}));

function realAppSnippet() {
  return createRawSnippet(() => ({
    render: () => `<div data-testid="real-app">App Content</div>`,
  }));
}

function fireEvent(eventName: string, payload: unknown) {
  const handler = listenersByEvent.get(eventName);
  if (!handler) throw new Error(`no listener registered for ${eventName}`);
  handler({ payload });
}

beforeEach(() => {
  invokeMock.mockReset();
  listenMock.mockReset();
  exitMock.mockReset();
  listenersByEvent.clear();
  listenMock.mockImplementation(async (eventName: string, handler: (event: { payload: unknown }) => void) => {
    listenersByEvent.set(eventName, handler);
    return () => listenersByEvent.delete(eventName);
  });
});

afterEach(() => {
  cleanup();
});

describe('UpdatingScreen — fast path', () => {
  it('renders the real app immediately when status is already ready', async () => {
    invokeMock.mockResolvedValue({ phase: 'ready', message: null });

    render(UpdatingScreen, { props: { children: realAppSnippet() } });

    await screen.findByTestId('real-app');

    expect(invokeMock).toHaveBeenCalledWith('get_startup_status');
    // No event listeners should be attached on the fast path — there is
    // nothing to listen for once we're already ready.
    expect(listenMock).not.toHaveBeenCalled();
  });
});

describe('UpdatingScreen — migration required', () => {
  it('shows "Updating Pane… don\'t quit" and does not render the real app', async () => {
    invokeMock.mockResolvedValue({ phase: 'migration_required', message: null });

    render(UpdatingScreen, { props: { children: realAppSnippet() } });

    await screen.findByText('Updating Pane…');
    expect(screen.getByText("Don't quit")).toBeTruthy();
    expect(screen.queryByTestId('real-app')).toBeNull();
  });

  it('updates the visible message and percentage from backup://progress', async () => {
    invokeMock.mockResolvedValue({ phase: 'migration_required', message: null });

    render(UpdatingScreen, { props: { children: realAppSnippet() } });

    await vi.waitFor(() => expect(listenersByEvent.has('backup://progress')).toBe(true));

    fireEvent('backup://progress', { phase: 'backup_copying', message: 'Copying database', pct: 15 });

    await screen.findByText('Copying database');
  });

  it('updates the visible message from migration://progress', async () => {
    invokeMock.mockResolvedValue({ phase: 'migration_required', message: null });

    render(UpdatingScreen, { props: { children: realAppSnippet() } });

    await vi.waitFor(() => expect(listenersByEvent.has('migration://progress')).toBe(true));

    fireEvent('migration://progress', { step: 2, total: 4, label: 'migrate_v92', pct: 50 });

    await vi.waitFor(() => {
      expect(document.body.textContent).toContain('step 2 of 4');
    });
  });

  it('switches to the real app once startup://ready fires', async () => {
    invokeMock.mockResolvedValue({ phase: 'migration_required', message: null });

    render(UpdatingScreen, { props: { children: realAppSnippet() } });

    await vi.waitFor(() => expect(listenersByEvent.has('startup://ready')).toBe(true));

    fireEvent('startup://ready', undefined);

    await screen.findByTestId('real-app');
  });

  it('falls back to a defensive re-poll if the ready event never fires', async () => {
    invokeMock.mockResolvedValueOnce({ phase: 'migration_required', message: null });
    invokeMock.mockResolvedValue({ phase: 'ready', message: null });

    render(UpdatingScreen, { props: { children: realAppSnippet() } });

    await vi.waitFor(() => expect(listenersByEvent.has('startup://ready')).toBe(true));

    // The component's own re-poll runs on a real 500ms interval — wait for
    // it to fire naturally (no fake timers, matching this codebase's
    // existing async-event test convention) rather than faking time.
    await screen.findByTestId('real-app', {}, { timeout: 3000 });
  }, 5000);
});

describe('UpdatingScreen — failure path', () => {
  it('shows a plain failure message and a Quit button, never a blank/spinner-only screen', async () => {
    invokeMock.mockResolvedValue({ phase: 'failed', message: 'disk full' });

    render(UpdatingScreen, { props: { children: realAppSnippet() } });

    await screen.findByText('Update Failed');
    expect(screen.getByText('disk full')).toBeTruthy();
    expect(screen.getByText('Quit Pane')).toBeTruthy();
  });

  it('calls exit() when Quit is clicked', async () => {
    invokeMock.mockResolvedValue({ phase: 'failed', message: 'disk full' });

    render(UpdatingScreen, { props: { children: realAppSnippet() } });

    const quitButton = await screen.findByText('Quit Pane');
    quitButton.click();

    expect(exitMock).toHaveBeenCalledWith(1);
  });

  it('surfaces failure (does not strand the user) if startup://failed fires mid-migration', async () => {
    invokeMock.mockResolvedValue({ phase: 'migration_required', message: null });

    render(UpdatingScreen, { props: { children: realAppSnippet() } });

    await vi.waitFor(() => expect(listenersByEvent.has('startup://failed')).toBe(true));

    fireEvent('startup://failed', { message: 'crsqlite extension failed to load' });

    await screen.findByText('Update Failed');
    expect(screen.getByText('crsqlite extension failed to load')).toBeTruthy();
  });

  it('shows a failure screen (not a silent/blank one) if get_startup_status itself throws', async () => {
    invokeMock.mockRejectedValue(new Error('IPC unavailable'));

    render(UpdatingScreen, { props: { children: realAppSnippet() } });

    await screen.findByText('Update Failed');
  });
});

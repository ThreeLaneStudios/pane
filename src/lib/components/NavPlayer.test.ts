/**
 * NavPlayer.test.ts — Component tests for NavPlayer.svelte
 *
 * Tests:
 *  NP-1  Renders nothing when $playerTrack === null
 *  NP-2  Renders expanded shape when track set + status='playing'
 *  NP-3  Renders spinner + "buffering…" when status='loading'
 *  NP-4  Renders red block + Retry button when status='failed'
 *  NP-5  Renders play icon when status='paused', pause icon when status='playing'
 *  NP-6  Go-to-note button is DISABLED when noteId === null, ENABLED when non-null
 *  NP-7  Click Retry → status transitions to 'loading'
 *  NP-8  Scrubber width reflects position / duration
 *  NP-9  handleGoToNote — null noteId no-op (PR5b)
 *  NP-10 handleGoToNote — same-note guard (PR5b)
 *  NP-11 handleGoToNote — IPC error → toast shown, selectNote not called (PR5b)
 *  NP-12 handleGoToNote — resolves via readNote IPC and calls selectNote (PR5b)
 *  NP-16 Cross-project ↗ opens source note in its project view (#1876-fix)
 *  NP-17 Rapid-click guard — button disabled during IPC (#1876-fix)
 *  NP-18 Toast on IPC error (#1876-fix)
 *  NP-19 Same-note guard regression — readNote not called (#1876-fix)
 *
 * PR5a — spec-skip: spec note added in docs/spec/ui-layout.md
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte/svelte5';

import { tick } from 'svelte';
import { get } from 'svelte/store';
import NavPlayer from './NavPlayer.svelte';
import {
  playerTrack,
  playerStatus,
  playerPosition,
  playerDuration,
  type PlayerTrack,
} from '$lib/stores/player';
import { activeNote } from '$lib/stores/app';
import type { NoteMeta, NoteContent } from '$lib/types';

// ─── Mock noteService ─────────────────────────────────────────────────────────
vi.mock('$lib/services/noteService', () => ({
  selectNote: vi.fn().mockResolvedValue(undefined),
  createNote: vi.fn(),
}));
import * as noteService from '$lib/services/noteService';

// ─── Mock $lib/api (readNote IPC) ────────────────────────────────────────────────────
vi.mock('$lib/api', () => ({
  readNote: vi.fn().mockResolvedValue(null),
  fetchLinkPreview: vi.fn().mockResolvedValue(null),
  getCachedLinkPreview: vi.fn().mockResolvedValue(null),
}));
import * as api from '$lib/api';

// ─── Mock $lib/services/navigation ───────────────────────────────────────────────
vi.mock('$lib/services/navigation', () => ({
  navigateTo: vi.fn().mockResolvedValue(undefined),
}));
import * as navigation from '$lib/services/navigation';

// ─── Mock @tauri-apps/api/core (needed by noteService imports) ────────────────
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn().mockResolvedValue(null),
}));

// ─── Helpers ──────────────────────────────────────────────────────────────────

function mkTrack(overrides: Partial<PlayerTrack> = {}): PlayerTrack {
  return {
    attachmentId: 'att-test-001',
    noteId: null,
    title: 'test-audio.mp3',
    breadcrumb: 'Project › Note',
    src: 'asset://localhost/attachments/att-test-001.mp3',
    ...overrides,
  };
}

async function flush() {
  await Promise.resolve();
  await tick();
}

// ─── Global HTMLMediaElement stubs ──────────────────────────────────────────
// jsdom throws "Not implemented: HTMLMediaElement.prototype.load/pause" for any
// audio/video element that exercises the real media stack. Stub them globally
// so test output is not flooded with irrelevant stderr noise. The fix under
// test (NavPlayer.svelte $effect → audio.load()) now fires on every render
// that has a valid src, which makes these stubs necessary for *all* NavPlayer
// tests, not just the NP-13/14/15 load-gate tests.
//
// `mediaLoadSpy` is also used in NP-13/14/15 to assert on call count without
// needing a second spy layer.
let mediaLoadSpy: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
  mediaLoadSpy = vi.spyOn(HTMLMediaElement.prototype, 'load').mockImplementation(() => {});
  vi.spyOn(HTMLMediaElement.prototype, 'pause').mockImplementation(() => {});
});

afterEach(() => {
  vi.restoreAllMocks();
});

// ─── Reset stores before each test ───────────────────────────────────────────

beforeEach(() => {
  playerTrack.set(null);
  playerStatus.set('idle');
  playerPosition.set(0);
  playerDuration.set(0);
});

// ─── NP-1: Empty state ────────────────────────────────────────────────────────

describe('NP-1 — renders nothing when $playerTrack === null', () => {
  it('container has no child nodes when track is null', () => {
    const { container } = render(NavPlayer);
    // No audio element, no player UI
    expect(container.querySelector('audio')).toBeNull();
    expect(container.querySelector('.np-block')).toBeNull();
  });

  it('no visible text content when track is null', () => {
    const { container } = render(NavPlayer);
    expect(container.textContent?.trim()).toBe('');
  });
});

// ─── NP-2: Playing state (expanded shape) ─────────────────────────────────────

describe('NP-2 — renders expanded shape when track set + status="playing"', () => {
  beforeEach(() => {
    playerTrack.set(mkTrack());
    playerStatus.set('playing');
    playerDuration.set(120);
    playerPosition.set(30);
  });

  it('renders the np-block container', async () => {
    const { container } = render(NavPlayer);
    await flush();
    expect(container.querySelector('.np-block')).not.toBeNull();
  });

  it('renders the audio element with the track src', async () => {
    const { container } = render(NavPlayer);
    await flush();
    const audio = container.querySelector('audio');
    expect(audio).not.toBeNull();
    expect(audio?.getAttribute('src')).toBe('asset://localhost/attachments/att-test-001.mp3');
  });

  it('renders the track title in the meta area', async () => {
    const { container } = render(NavPlayer);
    await flush();
    const title = container.querySelector('.np-title');
    expect(title).not.toBeNull();
    expect(title!.textContent).toContain('test-audio.mp3');
  });

  it('renders the breadcrumb', async () => {
    const { container } = render(NavPlayer);
    await flush();
    const bc = container.querySelector('.np-breadcrumb');
    expect(bc).not.toBeNull();
    expect(bc!.textContent).toContain('Project › Note');
  });

  it('renders the scrubber', async () => {
    const { container } = render(NavPlayer);
    await flush();
    expect(container.querySelector('.np-scrubber')).not.toBeNull();
  });
});

// ─── NP-3: Loading state ──────────────────────────────────────────────────────

describe('NP-3 — renders spinner + "buffering…" when status="loading"', () => {
  beforeEach(() => {
    playerTrack.set(mkTrack());
    playerStatus.set('loading');
  });

  it('shows "buffering…" text in breadcrumb area', async () => {
    const { container } = render(NavPlayer);
    await flush();
    expect(container.textContent).toContain('buffering…');
  });

  it('renders loading state (no failed-card)', async () => {
    const { container } = render(NavPlayer);
    await flush();
    expect(container.querySelector('.np-failed-card')).toBeNull();
  });

  it('np-block--loading class is present on loading', async () => {
    const { container } = render(NavPlayer);
    await flush();
    expect(container.querySelector('.np-block--loading')).not.toBeNull();
  });
});

// ─── NP-4: Failed state ───────────────────────────────────────────────────────

describe('NP-4 — renders red block + Retry button when status="failed"', () => {
  beforeEach(() => {
    playerTrack.set(mkTrack({ title: 'broken.mp3' }));
    playerStatus.set('failed');
  });

  it('renders .np-block--failed', async () => {
    const { container } = render(NavPlayer);
    await flush();
    expect(container.querySelector('.np-block--failed')).not.toBeNull();
  });

  it('shows "Couldn\'t play this file" message', async () => {
    const { container } = render(NavPlayer);
    await flush();
    expect(container.textContent).toContain("Couldn't play this file");
  });

  it('shows the filename as the sub-label', async () => {
    const { container } = render(NavPlayer);
    await flush();
    expect(container.textContent).toContain('broken.mp3');
  });

  it('renders a Retry button', async () => {
    const { container } = render(NavPlayer);
    await flush();
    const retry = container.querySelector('.np-retry-btn');
    expect(retry).not.toBeNull();
    expect(retry!.textContent).toContain('Retry');
  });
});

// ─── NP-5: Play/Pause icon ────────────────────────────────────────────────────

describe('NP-5 — play icon when paused, pause icon when playing', () => {
  it('play button container is present when status="paused"', async () => {
    playerTrack.set(mkTrack());
    playerStatus.set('paused');
    const { container } = render(NavPlayer);
    await flush();
    // Transport row should exist
    expect(container.querySelector('.np-play-btn')).not.toBeNull();
    expect(container.querySelector('.np-transport')).not.toBeNull();
  });

  it('play button container is present when status="playing"', async () => {
    playerTrack.set(mkTrack());
    playerStatus.set('playing');
    const { container } = render(NavPlayer);
    await flush();
    expect(container.querySelector('.np-play-btn')).not.toBeNull();
  });

  it('play button is disabled (loading) when status="loading"', async () => {
    playerTrack.set(mkTrack());
    playerStatus.set('loading');
    const { container } = render(NavPlayer);
    await flush();
    const btn = container.querySelector('.np-play-btn') as HTMLButtonElement | null;
    expect(btn).not.toBeNull();
    expect(btn!.disabled).toBe(true);
  });
});

// ─── NP-6: Go-to-note button ─────────────────────────────────────────────────

describe('NP-6 — go-to-note button disabled when noteId=null, enabled when non-null', () => {
  it('go-to-note button is disabled when noteId is null', async () => {
    playerTrack.set(mkTrack({ noteId: null }));
    playerStatus.set('playing');
    const { container } = render(NavPlayer);
    await flush();
    const btn = container.querySelector('.np-goto-btn') as HTMLButtonElement | null;
    expect(btn).not.toBeNull();
    expect(btn!.disabled).toBe(true);
  });

  it('go-to-note button is enabled when noteId is non-null', async () => {
    playerTrack.set(mkTrack({ noteId: 'note-abc-123' }));
    playerStatus.set('playing');
    const { container } = render(NavPlayer);
    await flush();
    const btn = container.querySelector('.np-goto-btn') as HTMLButtonElement | null;
    expect(btn).not.toBeNull();
    expect(btn!.disabled).toBe(false);
  });

  it('go-to-note button has title="Go to note"', async () => {
    playerTrack.set(mkTrack({ noteId: null }));
    playerStatus.set('playing');
    const { container } = render(NavPlayer);
    await flush();
    const btn = container.querySelector('.np-goto-btn');
    expect(btn?.getAttribute('title')).toBe('Go to note');
  });
});

// ─── NP-7: Retry button transitions status → 'loading' ────────────────────────

describe('NP-7 — click Retry → status transitions to "loading"', () => {
  it('clicking Retry sets playerStatus to "loading"', async () => {
    playerTrack.set(mkTrack());
    playerStatus.set('failed');
    const { container } = render(NavPlayer);
    await flush();

    const retryBtn = container.querySelector('.np-retry-btn') as HTMLElement;
    expect(retryBtn).not.toBeNull();

    // Ensure we start in 'failed'
    expect(get(playerStatus)).toBe('failed');

    fireEvent.click(retryBtn);
    await flush();

    expect(get(playerStatus)).toBe('loading');
  });
});

// ─── NP-8: Scrubber width reflects position / duration ────────────────────────

describe('NP-8 — scrubber fill width reflects position / duration', () => {
  it('scrubber fill is 0% when position=0 and duration=0', async () => {
    playerTrack.set(mkTrack());
    playerStatus.set('paused');
    playerPosition.set(0);
    playerDuration.set(0);
    const { container } = render(NavPlayer);
    await flush();

    const fill = container.querySelector('.np-scrubber-fill') as HTMLElement | null;
    expect(fill).not.toBeNull();
    // 0/0 → 0%
    expect(fill!.style.width).toBe('0%');
  });

  it('scrubber fill is 50% when position=30 and duration=60', async () => {
    playerTrack.set(mkTrack());
    playerStatus.set('playing');
    playerPosition.set(30);
    playerDuration.set(60);
    const { container } = render(NavPlayer);
    await flush();

    const fill = container.querySelector('.np-scrubber-fill') as HTMLElement | null;
    expect(fill).not.toBeNull();
    expect(fill!.style.width).toBe('50%');
  });

  it('scrubber fill is 100% when position equals duration', async () => {
    playerTrack.set(mkTrack());
    playerStatus.set('paused');
    playerPosition.set(120);
    playerDuration.set(120);
    const { container } = render(NavPlayer);
    await flush();

    const fill = container.querySelector('.np-scrubber-fill') as HTMLElement | null;
    expect(fill).not.toBeNull();
    expect(fill!.style.width).toBe('100%');
  });
});

// ─── Helpers for NP-9/10/11/12 ───────────────────────────────────────────────

function mkNoteMeta(overrides: Partial<NoteMeta> = {}): NoteMeta {
  return {
    id: 'note-uuid-001',
    title: 'My Note',
    notebook: 'Research',
    slug: 'my-note',
    created_at: '2026-07-02T00:00:00Z',
    updated_at: '2026-07-02T00:00:00Z',
    preview: '',
    ...overrides,
  };
}

function mkNoteContent(meta: NoteMeta): NoteContent {
  return { meta, content: '' };
}

// ─── NP-9: handleGoToNote — null noteId no-op ────────────────────────────────

describe('NP-9 — handleGoToNote: null noteId is a no-op', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    playerTrack.set(mkTrack({ noteId: null }));
    playerStatus.set('playing');
    activeNote.set(null);
  });

  it('does not call selectNote when noteId is null', async () => {
    const { container } = render(NavPlayer);
    await flush();

    const btn = container.querySelector('.np-goto-btn') as HTMLButtonElement;
    expect(btn).not.toBeNull();
    // Button is disabled when noteId=null; we can still fireEvent to test the guard
    fireEvent.click(btn);
    await flush();

    expect(noteService.selectNote).not.toHaveBeenCalled();
    expect(api.readNote).not.toHaveBeenCalled();
  });
});

// ─── NP-10: handleGoToNote — same-note guard ─────────────────────────────────

describe('NP-10 — handleGoToNote: same-note guard (no-op when already on that note)', () => {
  const noteId = 'note-uuid-same';
  const meta = mkNoteMeta({ id: noteId, title: 'Open Note', notebook: 'Research' });

  beforeEach(() => {
    vi.clearAllMocks();
    playerTrack.set(mkTrack({ noteId }));
    playerStatus.set('playing');
    // activeNote already IS the track's note
    activeNote.set(mkNoteContent(meta));
  });

  it('does not call selectNote when already on the track note', async () => {
    const { container } = render(NavPlayer);
    await flush();

    const btn = container.querySelector('.np-goto-btn') as HTMLButtonElement;
    expect(btn).not.toBeNull();
    expect(btn.disabled).toBe(false);

    fireEvent.click(btn);
    await flush();

    expect(noteService.selectNote).not.toHaveBeenCalled();
  });
});

// ─── NP-11: handleGoToNote ─── IPC error → toast + no selectNote ─────

describe('NP-11 — handleGoToNote: IPC error → toast shown, selectNote not called', () => {
  const noteId = 'note-uuid-ipc-error';

  beforeEach(() => {
    vi.clearAllMocks();
    (api.readNote as ReturnType<typeof vi.fn>).mockRejectedValue(new Error('IPC fail'));
    playerTrack.set(mkTrack({ noteId }));
    playerStatus.set('playing');
    activeNote.set(null);
  });

  it('does not call selectNote when readNote throws', async () => {
    const { container } = render(NavPlayer);
    await flush();

    const btn = container.querySelector('.np-goto-btn') as HTMLButtonElement;
    expect(btn).not.toBeNull();
    expect(btn.disabled).toBe(false);

    fireEvent.click(btn);
    await flush();

    expect(noteService.selectNote).not.toHaveBeenCalled();
  });

  it('shows error toast when readNote throws', async () => {
    const { container } = render(NavPlayer);
    await flush();

    const btn = container.querySelector('.np-goto-btn') as HTMLButtonElement;
    fireEvent.click(btn);
    await flush();

    const toast = container.querySelector('.error-toast');
    expect(toast).not.toBeNull();
    expect(toast!.textContent).toContain('Could not open source note');
  });
});

// ─── NP-13: audio.load() fires on initial mount ────────────────────────────────

describe('NP-13 — audio.load() is called on initial mount when src is set', () => {
  it('calls audio.load() after bind:this resolves and track.src is non-empty', async () => {
    // mediaLoadSpy is set up in the module-level beforeEach (HTMLMediaElement.prototype.load)
    playerTrack.set(mkTrack({ src: 'asset://localhost/attachments/att-test-001.mp3' }));
    playerStatus.set('loading');
    render(NavPlayer);
    await flush();
    expect(mediaLoadSpy).toHaveBeenCalled();
  });

  it('audio element is present in DOM after mount with valid src', async () => {
    playerTrack.set(mkTrack());
    playerStatus.set('loading');
    const { container } = render(NavPlayer);
    await flush();
    const audioEl = container.querySelector('audio');
    expect(audioEl).not.toBeNull();
    expect(audioEl?.getAttribute('src')).toBe('asset://localhost/attachments/att-test-001.mp3');
  });
});

// ─── NP-14: audio.load() re-fires when track changes ────────────────────────

describe('NP-14 — audio.load() re-fires when track changes to a new attachment', () => {
  it('calls audio.load() again after track swaps to a new attachmentId', async () => {
    playerTrack.set(mkTrack({ attachmentId: 'att-001', src: 'asset://localhost/att-001.mp3' }));
    playerStatus.set('loading');
    render(NavPlayer);
    await flush();
    const callsAfterFirstMount = mediaLoadSpy.mock.calls.length;
    expect(callsAfterFirstMount).toBeGreaterThan(0);

    // Swap to a different track
    playerTrack.set(mkTrack({ attachmentId: 'att-002', src: 'asset://localhost/att-002.mp3' }));
    playerStatus.set('loading');
    await flush();

    expect(mediaLoadSpy.mock.calls.length).toBeGreaterThan(callsAfterFirstMount);
  });
});

// ─── NP-15: empty src does NOT trigger audio.load() ─────────────────────────

describe('NP-15 — audio.load() is NOT called when track.src is empty string', () => {
  it('does not call audio.load() when src is an empty string (guard)', async () => {
    playerTrack.set(mkTrack({ src: '' }));
    playerStatus.set('loading');
    render(NavPlayer);
    await flush();
    expect(mediaLoadSpy).not.toHaveBeenCalled();
  });
});

// ─── NP-12: handleGoToNote ─── resolves via IPC and calls selectNote ───

describe('NP-12 — handleGoToNote: resolves NoteMeta via readNote IPC and calls selectNote', () => {
  const noteId = 'note-uuid-target';
  const targetMeta = mkNoteMeta({ id: noteId, title: 'Target Note', notebook: 'Bitcoin', project_id: 'project-b' });
  const otherMeta = mkNoteMeta({ id: 'note-uuid-other', title: 'Other', notebook: 'Inbox', project_id: 'project-a' });

  beforeEach(() => {
    vi.clearAllMocks();
    (api.readNote as ReturnType<typeof vi.fn>).mockResolvedValue(mkNoteContent(targetMeta));
    playerTrack.set(mkTrack({ noteId }));
    playerStatus.set('playing');
    // activeNote is a DIFFERENT note (not same-note guard)
    activeNote.set(mkNoteContent(otherMeta));
  });

  it('calls readNote with empty notebook and the note ID', async () => {
    const { container } = render(NavPlayer);
    await flush();

    const btn = container.querySelector('.np-goto-btn') as HTMLButtonElement;
    fireEvent.click(btn);
    await flush();

    expect(api.readNote).toHaveBeenCalledWith('', noteId);
  });

  it('calls selectNote with the NoteMeta from readNote result', async () => {
    const { container } = render(NavPlayer);
    await flush();

    const btn = container.querySelector('.np-goto-btn') as HTMLButtonElement;
    expect(btn.disabled).toBe(false);

    fireEvent.click(btn);
    await flush();

    expect(noteService.selectNote).toHaveBeenCalledTimes(1);
    expect(noteService.selectNote).toHaveBeenCalledWith(
      expect.objectContaining({ id: noteId, title: 'Target Note' })
    );
  });
});

// ─── NP-16: Cross-project ↗ opens source note in its project view ───

describe('NP-16 — cross-project ↗ opens source note in its project view', () => {
  const noteId = 'note-x';
  const noteMeta = mkNoteMeta({ id: noteId, title: 'Source Note', notebook: 'Bitcoin', project_id: 'project-b' });
  const otherMeta = mkNoteMeta({ id: 'other-note', title: 'Other', notebook: 'Inbox', project_id: 'project-a' });

  beforeEach(() => {
    vi.clearAllMocks();
    (api.readNote as ReturnType<typeof vi.fn>).mockResolvedValue(mkNoteContent(noteMeta));
    playerTrack.set(mkTrack({ noteId }));
    playerStatus.set('playing');
    // activeNote is a DIFFERENT note (not same-note guard)
    activeNote.set(mkNoteContent(otherMeta));
  });

  it('calls navigateTo with notes_browser view and projectId from readNote result', async () => {
    const { container } = render(NavPlayer);
    await flush();

    const btn = container.querySelector('.np-goto-btn') as HTMLButtonElement;
    expect(btn).not.toBeNull();
    fireEvent.click(btn);
    await flush();

    expect(navigation.navigateTo).toHaveBeenCalledWith(
      expect.objectContaining({ view: 'notes_browser', projectId: 'project-b' })
    );
  });

  it('calls selectNote after navigateTo (ordering via invocationCallOrder)', async () => {
    const { container } = render(NavPlayer);
    await flush();

    const btn = container.querySelector('.np-goto-btn') as HTMLButtonElement;
    fireEvent.click(btn);
    await flush();

    const navOrder = (navigation.navigateTo as ReturnType<typeof vi.fn>).mock.invocationCallOrder[0];
    const selectOrder = (noteService.selectNote as ReturnType<typeof vi.fn>).mock.invocationCallOrder[0];
    expect(navOrder).toBeLessThan(selectOrder);
  });

  it('calls selectNote with NoteMeta shape (has .id not .meta.id)', async () => {
    const { container } = render(NavPlayer);
    await flush();

    const btn = container.querySelector('.np-goto-btn') as HTMLButtonElement;
    fireEvent.click(btn);
    await flush();

    const arg = (noteService.selectNote as ReturnType<typeof vi.fn>).mock.calls[0][0];
    expect(arg.id).toBe(noteId);
    expect(arg).not.toHaveProperty('meta');
  });
});

// ─── NP-17: Rapid-click guard ───

describe('NP-17 — rapid-click guard disables button during IPC', () => {
  const noteId = 'note-guard';
  const noteMeta = mkNoteMeta({ id: noteId, project_id: 'project-x' });
  const otherMeta = mkNoteMeta({ id: 'other-note', project_id: 'project-y' });

  it('button is disabled while readNote IPC is in flight, re-enabled after resolve', async () => {
    let resolve!: (val: NoteContent) => void;
    const pending = new Promise<NoteContent>((r) => { resolve = r; });
    (api.readNote as ReturnType<typeof vi.fn>).mockReturnValue(pending);

    playerTrack.set(mkTrack({ noteId }));
    playerStatus.set('playing');
    activeNote.set(mkNoteContent(otherMeta));

    const { container } = render(NavPlayer);
    await flush();

    const btn = container.querySelector('.np-goto-btn') as HTMLButtonElement;
    expect(btn.disabled).toBe(false);

    fireEvent.click(btn);
    await flush();

    expect(btn.disabled).toBe(true);

    resolve(mkNoteContent(noteMeta));
    // flush multiple times to drain: readNote resolve → navigateTo → selectNote → finally
    await flush();
    await flush();
    await flush();

    expect(btn.disabled).toBe(false);
  });
});

// ─── NP-18: Toast on IPC error ───

describe('NP-18 — toast on IPC error, navigateTo and selectNote not called', () => {
  const noteId = 'note-ipc-fail';

  beforeEach(() => {
    vi.clearAllMocks();
    (api.readNote as ReturnType<typeof vi.fn>).mockRejectedValue(new Error('IPC fail'));
    playerTrack.set(mkTrack({ noteId }));
    playerStatus.set('playing');
    activeNote.set(null);
  });

  it('renders error toast text after readNote rejects', async () => {
    const { container } = render(NavPlayer);
    await flush();

    const btn = container.querySelector('.np-goto-btn') as HTMLButtonElement;
    fireEvent.click(btn);
    await flush();

    const toast = container.querySelector('.error-toast');
    expect(toast).not.toBeNull();
    expect(toast!.textContent).toContain('Could not open source note');
  });

  it('does not call navigateTo when readNote throws', async () => {
    const { container } = render(NavPlayer);
    await flush();

    fireEvent.click(container.querySelector('.np-goto-btn') as HTMLButtonElement);
    await flush();

    expect(navigation.navigateTo).not.toHaveBeenCalled();
  });

  it('does not call selectNote when readNote throws', async () => {
    const { container } = render(NavPlayer);
    await flush();

    fireEvent.click(container.querySelector('.np-goto-btn') as HTMLButtonElement);
    await flush();

    expect(noteService.selectNote).not.toHaveBeenCalled();
  });
});

// ─── NP-19: Same-note guard regression ───

describe('NP-19 — same-note guard regression: readNote not called when already on that note', () => {
  const noteId = 'note-same';
  const meta = mkNoteMeta({ id: noteId, title: 'Open Note', project_id: 'project-z' });

  beforeEach(() => {
    vi.clearAllMocks();
    playerTrack.set(mkTrack({ noteId }));
    playerStatus.set('playing');
    // activeNote IS the same note as the track
    activeNote.set(mkNoteContent(meta));
  });

  it('does not call readNote when activeNote matches track noteId', async () => {
    const { container } = render(NavPlayer);
    await flush();

    const btn = container.querySelector('.np-goto-btn') as HTMLButtonElement;
    expect(btn.disabled).toBe(false);

    fireEvent.click(btn);
    await flush();

    expect(api.readNote).not.toHaveBeenCalled();
    expect(noteService.selectNote).not.toHaveBeenCalled();
  });
});

import { readFileSync } from 'fs';
import { resolve } from 'path';

describe('body prefetch attribute', () => {
  it('app.html does not enable sveltekit-preload-data on body (perf: no global hover-prefetch)', () => {
    // Pane uses canvasState swaps, not SvelteKit route transitions.
    // hover-prefetch installs a global mousemove listener with a 20ms setTimeout reset loop
    // that burns CPU with no user-facing benefit. Live-measured: -22% CPU during NavPlayer playback.
    const html = readFileSync(resolve('src/app.html'), 'utf-8');
    expect(html).not.toMatch(/<body[^>]*data-sveltekit-preload-data/);
  });
});

describe('EQ bar animation is composite-only', () => {
  it('keyframes animate transform, not height', () => {
    const src = readFileSync(resolve('src/lib/components/NavPlayer.svelte'), 'utf-8');
    // Extract each @keyframes block
    const eqKeyframes = src.match(/@keyframes np-eq\d\s*\{[^}]+\}/g) || [];
    expect(eqKeyframes.length).toBe(4);
    for (const kf of eqKeyframes) {
      expect(kf).toMatch(/transform:\s*scaleY/);
      expect(kf).not.toMatch(/height:/);
    }
  });
});

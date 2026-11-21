/**
 * player.test.ts — Unit tests for src/lib/stores/player.ts
 *
 * Tests:
 *  S-1  Default values (playerTrack=null, playerStatus='idle',
 *         playerPosition=0, playerDuration=0)
 *  S-2  Setting playerTrack to a track object
 *  S-3  Clearing with playerTrack.set(null)
 *  S-4  Status transitions through all 5 states
 *  S-5  Position and duration are settable and readable
 *  S-6  loadTrack helper (PR5b)
 *  S-7  clearTrack helper (PR5b)
 *
 * PR5a — spec-skip: spec note added in docs/spec/ui-layout.md
 */

import { describe, it, expect, beforeEach } from 'vitest';
import { get } from 'svelte/store';
import {
  playerTrack,
  playerStatus,
  playerPosition,
  playerDuration,
  loadTrack,
  clearTrack,
  type PlayerStatus,
  type PlayerTrack,
} from './player';

// ─── Reset all stores before each test ────────────────────────────────────────

beforeEach(() => {
  playerTrack.set(null);
  playerStatus.set('idle');
  playerPosition.set(0);
  playerDuration.set(0);
});

// ─── S-1: Default values ─────────────────────────────────────────────────────

describe('S-1 — default store values', () => {
  it('playerTrack defaults to null', () => {
    expect(get(playerTrack)).toBeNull();
  });

  it('playerStatus defaults to "idle"', () => {
    expect(get(playerStatus)).toBe('idle');
  });

  it('playerPosition defaults to 0', () => {
    expect(get(playerPosition)).toBe(0);
  });

  it('playerDuration defaults to 0', () => {
    expect(get(playerDuration)).toBe(0);
  });
});

// ─── S-2: Setting playerTrack ────────────────────────────────────────────────

describe('S-2 — setting playerTrack to a track object', () => {
  const track: PlayerTrack = {
    attachmentId: 'att-abc-123',
    noteId: null,
    title: 'my-song.mp3',
    breadcrumb: 'Music › Audio Test',
    src: 'asset://localhost/attachments/att-abc-123.mp3',
  };

  it('stores the track and exposes correct attachmentId', () => {
    playerTrack.set(track);
    const t = get(playerTrack);
    expect(t).not.toBeNull();
    expect(t!.attachmentId).toBe('att-abc-123');
  });

  it('stores the track title', () => {
    playerTrack.set(track);
    expect(get(playerTrack)!.title).toBe('my-song.mp3');
  });

  it('stores the track breadcrumb', () => {
    playerTrack.set(track);
    expect(get(playerTrack)!.breadcrumb).toBe('Music › Audio Test');
  });

  it('stores the track src', () => {
    playerTrack.set(track);
    expect(get(playerTrack)!.src).toBe('asset://localhost/attachments/att-abc-123.mp3');
  });

  it('noteId is null (PR5b will thread it)', () => {
    playerTrack.set(track);
    expect(get(playerTrack)!.noteId).toBeNull();
  });

  it('accepts a non-null noteId', () => {
    playerTrack.set({ ...track, noteId: 'note-xyz' });
    expect(get(playerTrack)!.noteId).toBe('note-xyz');
  });
});

// ─── S-3: Clearing the track ──────────────────────────────────────────────────

describe('S-3 — clearing with playerTrack.set(null)', () => {
  it('returns to null after being set', () => {
    playerTrack.set({
      attachmentId: 'att-to-clear',
      noteId: null,
      title: 'temp.mp3',
      breadcrumb: 'Project › Note',
      src: 'asset://localhost/attachments/att-to-clear.mp3',
    });
    expect(get(playerTrack)).not.toBeNull();

    playerTrack.set(null);
    expect(get(playerTrack)).toBeNull();
  });
});

// ─── S-4: Status transitions ─────────────────────────────────────────────────

describe('S-4 — status transitions through all 5 states', () => {
  const STATUSES: PlayerStatus[] = ['idle', 'loading', 'playing', 'paused', 'failed'];

  for (const s of STATUSES) {
    it(`can set status to "${s}"`, () => {
      playerStatus.set(s);
      expect(get(playerStatus)).toBe(s);
    });
  }

  it('transitions idle → loading → playing → paused → idle', () => {
    playerStatus.set('idle');
    expect(get(playerStatus)).toBe('idle');

    playerStatus.set('loading');
    expect(get(playerStatus)).toBe('loading');

    playerStatus.set('playing');
    expect(get(playerStatus)).toBe('playing');

    playerStatus.set('paused');
    expect(get(playerStatus)).toBe('paused');

    playerStatus.set('idle');
    expect(get(playerStatus)).toBe('idle');
  });

  it('transitions loading → failed', () => {
    playerStatus.set('loading');
    playerStatus.set('failed');
    expect(get(playerStatus)).toBe('failed');
  });

  it('transitions paused → playing', () => {
    playerStatus.set('paused');
    playerStatus.set('playing');
    expect(get(playerStatus)).toBe('playing');
  });
});

// ─── S-5: Position and duration ──────────────────────────────────────────────

describe('S-5 — position and duration are settable and readable', () => {
  it('playerPosition reflects the set value', () => {
    playerPosition.set(42.5);
    expect(get(playerPosition)).toBe(42.5);
  });

  it('playerDuration reflects the set value', () => {
    playerDuration.set(180);
    expect(get(playerDuration)).toBe(180);
  });

  it('position can be updated multiple times', () => {
    playerPosition.set(10);
    expect(get(playerPosition)).toBe(10);
    playerPosition.set(99.9);
    expect(get(playerPosition)).toBe(99.9);
  });

  it('both stores are independent', () => {
    playerPosition.set(30);
    playerDuration.set(300);
    expect(get(playerPosition)).toBe(30);
    expect(get(playerDuration)).toBe(300);
  });
});

// ─── S-6: loadTrack helper ────────────────────────────────────────────────────

describe('S-6 — loadTrack helper (PR5b)', () => {
  const track: PlayerTrack = {
    attachmentId: 'att-load-001',
    noteId: 'note-uuid-abc',
    title: 'interview.mp3',
    breadcrumb: 'Research › Q3 Call',
    src: 'asset://localhost/attachments/att-load-001.mp3',
  };

  it('sets playerStatus to "loading"', () => {
    loadTrack(track);
    expect(get(playerStatus)).toBe('loading');
  });

  it('resets playerPosition to 0', () => {
    playerPosition.set(42);
    loadTrack(track);
    expect(get(playerPosition)).toBe(0);
  });

  it('resets playerDuration to 0', () => {
    playerDuration.set(180);
    loadTrack(track);
    expect(get(playerDuration)).toBe(0);
  });

  it('sets playerTrack to the given track', () => {
    loadTrack(track);
    const t = get(playerTrack);
    expect(t).not.toBeNull();
    expect(t!.attachmentId).toBe('att-load-001');
    expect(t!.noteId).toBe('note-uuid-abc');
    expect(t!.title).toBe('interview.mp3');
    expect(t!.breadcrumb).toBe('Research › Q3 Call');
    expect(t!.src).toBe('asset://localhost/attachments/att-load-001.mp3');
  });

  it('sets status="loading" BEFORE writing track (atomic-write ordering)', () => {
    // Subscribe to playerTrack and capture playerStatus at the moment
    // the track value first becomes non-null. Status must already be 'loading'.
    const statusAtTrackWrite: string[] = [];

    const unsub = playerTrack.subscribe((t) => {
      if (t !== null) {
        // Capture the status synchronously at the moment track arrives
        statusAtTrackWrite.push(get(playerStatus));
      }
    });

    loadTrack(track);
    unsub();

    // The subscriber fired at least once with a non-null track
    expect(statusAtTrackWrite.length).toBeGreaterThanOrEqual(1);
    // Every time the track was non-null, status was already 'loading'
    for (const s of statusAtTrackWrite) {
      expect(s).toBe('loading');
    }
  });

  it('accepts a track with noteId=null', () => {
    loadTrack({ ...track, noteId: null });
    expect(get(playerTrack)!.noteId).toBeNull();
  });
});

// ─── S-7: clearTrack helper ───────────────────────────────────────────────────

describe('S-7 — clearTrack helper (PR5b)', () => {
  const track: PlayerTrack = {
    attachmentId: 'att-clear-002',
    noteId: null,
    title: 'to-clear.mp3',
    breadcrumb: 'Inbox › Note',
    src: 'asset://localhost/attachments/att-clear-002.mp3',
  };

  it('sets playerTrack to null', () => {
    playerTrack.set(track);
    expect(get(playerTrack)).not.toBeNull();
    clearTrack();
    expect(get(playerTrack)).toBeNull();
  });

  it('does not touch playerStatus (NavPlayer $effect handles that)', () => {
    playerTrack.set(track);
    playerStatus.set('playing');
    clearTrack();
    // clearTrack() only nulls the track — status management is NavPlayer's job
    expect(get(playerStatus)).toBe('playing');
  });

  it('does not touch playerPosition', () => {
    playerTrack.set(track);
    playerPosition.set(30);
    clearTrack();
    expect(get(playerPosition)).toBe(30);
  });

  it('is idempotent when track is already null', () => {
    expect(get(playerTrack)).toBeNull();
    clearTrack();
    expect(get(playerTrack)).toBeNull();
  });
});

/**
 * player.ts — Global audio player store (PR5a, #401 / #1801).
 *
 * Domain-isolated store following the canvas.ts pattern: flat writable exports,
 * no classes, no singletons.
 *
 * The <audio> element is owned by NavPlayer.svelte which lives inside
 * Sidebar.svelte (never unmounts on view navigation). This structural
 * decision is what closes #1801 by construction.
 */

import { writable } from 'svelte/store';

// ─── Types ─────────────────────────────────────────────────────────────────

export type PlayerStatus = 'idle' | 'loading' | 'playing' | 'paused' | 'failed';

export type PlayerTrack = {
  attachmentId: string;
  noteId: string | null;   // null in PR5a; PR5b threads it through
  title: string;           // filename
  breadcrumb: string;      // e.g. "Bitcoin › Audio Test" — computed by caller in PR5b
  src: string;             // asset:// URL from resolveAttachmentSrc
};

// ─── Stores ────────────────────────────────────────────────────────────────

/** The currently loaded track. null = empty state (player hidden). */
export const playerTrack = writable<PlayerTrack | null>(null);

/** Current playback status. */
export const playerStatus = writable<PlayerStatus>('idle');

/** Current playback position in seconds. */
export const playerPosition = writable<number>(0);

/** Track duration in seconds. 0 until known. */
export const playerDuration = writable<number>(0);

// ─── Helpers ──────────────────────────────────────────────────────────────────

/**
 * loadTrack — atomic load sequence for the NavPlayer.
 *
 * Write order (defensive):
 *   1. Reset position + duration to 0
 *   2. Set status='loading' (BEFORE track — so NavPlayer's onCanPlay gate reads
 *      'loading' when the audio src update fires and canplay is emitted)
 *   3. Set track (triggers reactive <audio src> update in NavPlayer)
 *
 * This is safe because Svelte 5 flushes reactive effects after all synchronous
 * .set() calls complete, so all three writes are visible together when effects run.
 */
export function loadTrack(track: PlayerTrack): void {
  playerPosition.set(0);
  playerDuration.set(0);
  playerStatus.set('loading');
  playerTrack.set(track);
}

/**
 * clearTrack — remove the current track.
 * NavPlayer's $effect on $playerTrack handles status/position reset when null.
 */
export function clearTrack(): void {
  playerTrack.set(null);
}

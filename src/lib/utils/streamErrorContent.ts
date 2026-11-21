/**
 * streamErrorContent.ts — Fix #1358 (Fix A / Fix B)
 *
 * Pure utility for building the display content when a stream terminates with
 * an error.  Extracted from `ChatSidebar.svelte` so that both the component
 * AND the test suite call the same implementation — no more parallel
 * reimplementations that can drift.
 *
 * Format when partial text exists:
 *   {partial text}\n\n---\n⚠ {error message}
 *
 * Format when partial text is absent:
 *   ⚠ {error message}
 */

/**
 * Build the message content string for a stream that terminated with an error.
 *
 * @param errorMessage  The human-readable error string forwarded from the backend.
 * @param partial       The partial payload from the error event.
 * @returns             Display-ready string to set on the message and persist to the DB.
 */
export function buildStreamErrorContent(
  errorMessage: string,
  partial: { fullText: string }
): string {
  const hasPartial = partial.fullText && partial.fullText.trim().length > 0;
  return hasPartial
    ? `${partial.fullText}\n\n---\n\u26A0 ${errorMessage}`
    : `\u26A0 ${errorMessage}`;
}

/**
 * Return true if the partial object contains non-empty text that should be
 * persisted and shown alongside the error banner.
 */
export function hasPartialText(partial: { fullText: string }): boolean {
  return Boolean(partial.fullText && partial.fullText.trim().length > 0);
}

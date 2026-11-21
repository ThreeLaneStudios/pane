/**
 * Extraction trigger logic tests (#155)
 *
 * Validates the hybrid extraction trigger: structural tools OR sufficient word count.
 */
import { describe, it, expect } from 'vitest';
import { shouldExtractMemory } from './extractionTrigger';

describe('shouldExtractMemory', () => {
  it('skips extraction for short messages with no tools', () => {
    // 3 words, no tools → no extraction
    expect(shouldExtractMemory('hey how are', [])).toBe(false);
  });

  it('triggers extraction for long messages', () => {
    // 10 words → extraction runs
    const msg = 'I really want to tell you about my new project idea';
    expect(shouldExtractMemory(msg, [])).toBe(true);
  });

  it('triggers extraction when AI uses a structural tool', () => {
    // 2 words + create_note tool → extraction runs
    expect(
      shouldExtractMemory('create note', [{ name: 'create_note' }]),
    ).toBe(true);
  });

  it('does not trigger for non-structural tools with short message', () => {
    // 2 words + read_note tool → no extraction
    expect(
      shouldExtractMemory('read note', [{ name: 'read_note' }]),
    ).toBe(false);
  });

  it('triggers extraction for exactly 8 words', () => {
    const msg = 'one two three four five six seven eight';
    expect(shouldExtractMemory(msg, [])).toBe(true);
  });

  it('skips extraction for exactly 7 words', () => {
    const msg = 'one two three four five six seven';
    expect(shouldExtractMemory(msg, [])).toBe(false);
  });
});

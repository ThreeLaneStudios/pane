import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import {
  parseSoulAndMemory,
  buildUploadExtractionPrompt,
  buildInterviewExtractionPrompt,
  streamAndCollect,
} from './seedGeneration';

const { invokeMock, listenMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  listenMock: vi.fn(),
}));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: invokeMock,
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: listenMock,
}));

describe('streamAndCollect (#2673, D26/PR-B)', () => {
  beforeEach(() => {
    invokeMock.mockReset();
    listenMock.mockReset();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('rejects a call with an empty sessionId without invoking ai_chat_stream', async () => {
    await expect(streamAndCollect('a prompt', '')).rejects.toThrow(/sessionId/);
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it('passes sessionId as a TOP-LEVEL invoke param, not nested in context', async () => {
    invokeMock.mockResolvedValue('stream-id-1');
    let capturedCallback: ((event: any) => void) | undefined;
    listenMock.mockImplementation(async (_eventName: string, cb: (event: any) => void) => {
      capturedCallback = cb;
      return () => {};
    });

    const promise = streamAndCollect('a prompt', 'sess-bound-to-agent-1');
    // Allow the invoke() + listen() microtasks to settle before firing the chunk.
    await new Promise((r) => setTimeout(r, 0));
    capturedCallback?.({ payload: { id: 'stream-id-1', text: 'hello', done: true } });
    const result = await promise;

    expect(result).toBe('hello');
    expect(invokeMock).toHaveBeenCalledWith('ai_chat_stream', expect.objectContaining({
      sessionId: 'sess-bound-to-agent-1',
    }));
    const [, callArgs] = invokeMock.mock.calls[0];
    expect(callArgs.context).not.toHaveProperty('sessionId');
  });

  it('uses the agent-seeding currentView literal, not the old onboarding literal', async () => {
    invokeMock.mockResolvedValue('stream-id-2');
    let capturedCallback: ((event: any) => void) | undefined;
    listenMock.mockImplementation(async (_eventName: string, cb: (event: any) => void) => {
      capturedCallback = cb;
      return () => {};
    });

    const promise = streamAndCollect('a prompt', 'sess-2');
    await new Promise((r) => setTimeout(r, 0));
    capturedCallback?.({ payload: { id: 'stream-id-2', text: 'ok', done: true } });
    await promise;

    const [, callArgs] = invokeMock.mock.calls[0];
    expect(callArgs.context.currentView).toBe('agent-seeding');
  });

  it('rejects when ai_chat_stream invoke throws', async () => {
    invokeMock.mockRejectedValue(new Error('backend exploded'));
    await expect(streamAndCollect('a prompt', 'sess-3')).rejects.toThrow('backend exploded');
  });
});

describe('parseSoulAndMemory', () => {
  it('splits a well-formed response containing both markers (soul first)', () => {
    const response = '---SOUL---\nBe warm and encouraging.\n---MEMORY---\nUser is a product designer in Austin.';
    const result = parseSoulAndMemory(response, 'fallback soul');
    expect(result.soul).toBe('Be warm and encouraging.');
    expect(result.memory).toBe('User is a product designer in Austin.');
  });

  it('splits a well-formed response containing both markers (memory first)', () => {
    const response = '---MEMORY---\nUser works in fintech.\n---SOUL---\nBe concise and direct.';
    const result = parseSoulAndMemory(response, 'fallback soul');
    expect(result.memory).toBe('User works in fintech.');
    expect(result.soul).toBe('Be concise and direct.');
  });

  it('falls back to ## marker variant when present', () => {
    const response = '## SOUL\nBe thorough.\n## MEMORY\nUser likes hiking.';
    const result = parseSoulAndMemory(response, 'fallback soul');
    expect(result.soul).toBe('Be thorough.');
    expect(result.memory).toBe('User likes hiking.');
  });

  it('falls back to # marker variant when present', () => {
    const response = '# SOUL\nBe playful.\n# MEMORY\nUser is a musician.';
    const result = parseSoulAndMemory(response, 'fallback soul');
    expect(result.soul).toBe('Be playful.');
    expect(result.memory).toBe('User is a musician.');
  });

  it('uses the fallback soul when the SOUL marker is missing entirely', () => {
    const response = 'Just some unmarked text, no MEMORY marker present either.';
    const result = parseSoulAndMemory(response, 'fallback soul text');
    expect(result.soul).toBe('fallback soul text');
    expect(result.memory).toBe(response);
  });

  it('uses the fallback soul when only MEMORY marker is present (no SOUL match)', () => {
    // Per parseSoulAndMemory contract, BOTH markers of a variant must be present
    // to split — a lone ---MEMORY--- with no ---SOUL--- (and no ## /# variants
    // matching both) falls through to the fallback/raw-response path.
    const response = '---MEMORY---\nSome memory-only content.';
    const result = parseSoulAndMemory(response, 'fallback soul');
    expect(result.soul).toBe('fallback soul');
    expect(result.memory).toBe(response);
  });

  it('strips leading newlines and separator lines from each section', () => {
    const response = '---SOUL---\n\n===\nBe helpful.\n---MEMORY---\n\n---\nUser is a teacher.';
    const result = parseSoulAndMemory(response, 'fallback soul');
    expect(result.soul).toBe('Be helpful.');
    expect(result.memory).toBe('User is a teacher.');
  });
});

describe('buildUploadExtractionPrompt', () => {
  it('includes the combined content, style preset reference, and marker instructions', () => {
    const combined = '--- resume.md ---\nSenior engineer with 10 years experience.';
    const prompt = buildUploadExtractionPrompt(combined, 'balanced', 'Be helpful and conversational.');

    expect(prompt).toContain(combined);
    expect(prompt).toContain('"balanced" communication style');
    expect(prompt).toContain('Be helpful and conversational.');
    expect(prompt).toContain('---SOUL---');
    expect(prompt).toContain('---MEMORY---');
    expect(prompt).toContain('Use the exact markers ---SOUL--- and ---MEMORY--- to separate sections.');
  });

  it('includes the no-fabrication rules', () => {
    const prompt = buildUploadExtractionPrompt('some docs', 'detailed', 'preset text');
    expect(prompt).toContain('NEVER guess, infer, or fabricate details');
  });
});

describe('buildInterviewExtractionPrompt', () => {
  it('includes the answers, communication style, and marker instructions', () => {
    const answers = 'Name: Jordan\nWhat they do and what matters: Runs a design studio.\n';
    const prompt = buildInterviewExtractionPrompt(answers, 'detailed', 'Be thorough and proactive.');

    expect(prompt).toContain(answers);
    expect(prompt).toContain('thorough and detailed');
    expect(prompt).toContain('Be thorough and proactive.');
    expect(prompt).toContain('---SOUL---');
    expect(prompt).toContain('---MEMORY---');
    expect(prompt).toContain('Use the exact markers ---SOUL--- and ---MEMORY--- to separate sections.');
  });

  it('maps "short" communication preference to concise and direct', () => {
    const prompt = buildInterviewExtractionPrompt('Name: Sam\n', 'short', 'preset text');
    expect(prompt).toContain('concise and direct');
  });

  it('maps unknown communication preference to balanced', () => {
    const prompt = buildInterviewExtractionPrompt('Name: Sam\n', 'something-else', 'preset text');
    expect(prompt).toContain('balanced');
  });
});

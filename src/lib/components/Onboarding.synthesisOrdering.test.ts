/**
 * Onboarding.synthesisOrdering.test.ts — regression coverage for #2634 (D10).
 *
 * Root cause (docs/audits/2026-08-21-d10-upload-synthesis-ordering.md):
 * `processUpload`/`processQuestions` used to call `streamAndCollect()` (which
 * invokes the `ai_chat_stream` Tauri command) BEFORE `ensureMainAgent()`
 * (which calls `upsert_agent`). On a fresh install, zero `ai_agents` rows
 * exist at that point, so the backend's session→agent resolution
 * (`resolve_and_selfheal_session_agent`, agents.rs) returned a hard
 * `Err("No agent is set up yet...")`, which the catch block then mislabeled
 * as "Processing took too long."
 *
 * This suite proves the fix at the call-order level: `upsert_agent` must be
 * invoked (and resolved) strictly before `ai_chat_stream` is invoked, for
 * both the upload path and the questions path. Unlike
 * Onboarding.workshopSession.test.ts (which mocks `$lib/onboarding/seedGeneration`
 * entirely), this suite deliberately does NOT mock seedGeneration — it needs
 * `streamAndCollect`'s real `invoke('ai_chat_stream', ...)` call to observe
 * true invoke-call ordering between `upsert_agent` and `ai_chat_stream`.
 *
 * Also verifies synthesis output (the parsed SOUL/MEMORY content) is written
 * via `write_system_file` scoped to the agent id `upsert_agent` returned —
 * not a stale/different agent id.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, fireEvent } from '@testing-library/svelte/svelte5';
import { tick } from 'svelte';
import Onboarding from './Onboarding.svelte';

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: invokeMock,
}));

// Capture the listen() callback so tests can manually fire the
// 'ai-stream-chunk' event that streamAndCollect() awaits to resolve.
const { listenCallbacks } = vi.hoisted(() => ({
  listenCallbacks: [] as Array<(event: any) => void>,
}));
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn((_eventName: string, cb: (event: any) => void) => {
    listenCallbacks.push(cb);
    return Promise.resolve(() => {});
  }),
}));

vi.mock('$lib/onboarding/workshopSession', () => ({
  createWorkshopSession: vi.fn(),
}));

if (typeof (globalThis.window as any).addEventListener !== 'function') {
  (globalThis.window as any).addEventListener = () => {};
}
if (typeof (globalThis.window as any).removeEventListener !== 'function') {
  (globalThis.window as any).removeEventListener = () => {};
}
if (typeof (globalThis.window as any).dispatchEvent !== 'function') {
  (globalThis.window as any).dispatchEvent = () => true;
}

const CREATED_AGENT_ID = 'agent-created-during-onboarding-d10';
const STREAM_ID = 'stream-abc-123';

/** Tracks the order commands were INVOKED (call start), not the order they
 *  RESOLVED — this is what actually distinguishes the D10 bug: the prior
 *  code started (and awaited) the ai_chat_stream invoke call before ever
 *  starting the upsert_agent call at all. */
let callOrder: string[] = [];

function defaultInvokeImpl(overrides: Record<string, (args: any) => any> = {}) {
  return async (cmd: string, args: any = {}) => {
    callOrder.push(cmd);
    if (overrides[cmd]) return overrides[cmd](args);
    switch (cmd) {
      case 'check_fresh_install':
        return false;
      case 'ai_get_default_soul':
        return 'Be helpful.';
      case 'validate_anthropic_key':
      case 'validate_openrouter_key':
        return null;
      case 'ai_configure_and_save':
        return null;
      case 'set_ai_setting':
        return null;
      case 'get_agents':
        return [];
      case 'get_main_agent':
        throw new Error('no main agent');
      case 'upsert_agent':
        return { id: CREATED_AGENT_ID, name: args?.request?.name || 'My Agent' };
      case 'ai_chat_stream':
        return STREAM_ID;
      case 'write_system_file':
        return null;
      case 'get_notes':
        return [];
      case 'create_note':
        return { id: `note-${args?.request?.title ?? 'x'}` };
      case 'ollama_probe_two_step':
        return { error_class: null, models: [] };
      default:
        return null;
    }
  };
}

/** Fires the collected ai-stream-chunk listener(s) with a done payload for
 *  STREAM_ID, resolving streamAndCollect()'s pending promise. */
function completeStream(text = '---SOUL---\nSoul text\n---MEMORY---\nMemory text') {
  for (const cb of listenCallbacks) {
    cb({ payload: { id: STREAM_ID, text, done: true } });
  }
}

async function navigateToMainScreen(container: HTMLElement, agentName: string) {
  await tick();
  await tick();

  const providerCard = Array.from(container.querySelectorAll('.provider-card')).find((el) =>
    el.textContent?.includes('Anthropic')
  ) as HTMLElement;
  await fireEvent.click(providerCard);
  await tick();

  const keyInput = container.querySelector('input[type="password"]') as HTMLInputElement;
  await fireEvent.input(keyInput, { target: { value: 'sk-test-key' } });
  await tick();

  const testBtn = Array.from(container.querySelectorAll('button')).find((b) =>
    b.textContent?.includes('Test Connection')
  ) as HTMLButtonElement;
  await fireEvent.click(testBtn);
  await tick();
  await tick();

  const continueBtn = Array.from(container.querySelectorAll('button')).find((b) =>
    b.textContent?.trim() === 'Continue' && b.classList.contains('connect-continue')
  ) as HTMLButtonElement;
  await fireEvent.click(continueBtn);
  await tick();
  await tick();
  await tick();
  await new Promise((r) => setTimeout(r, 0));
  await tick();
  await tick();

  // 'model-select' screen — pick a model, then Continue.
  const modelInput = container.querySelector('.model-input') as HTMLInputElement;
  await fireEvent.focus(modelInput);
  await tick();
  const item = Array.from(container.querySelectorAll('.dropdown-item'))[0] as HTMLElement;
  await fireEvent.mouseDown(item);
  await tick();

  const modelContinueBtn = Array.from(container.querySelectorAll('button')).find((b) =>
    b.textContent?.trim() === 'Continue'
  ) as HTMLButtonElement;
  await fireEvent.click(modelContinueBtn);
  await new Promise((r) => setTimeout(r, 0));
  await tick();
  await new Promise((r) => setTimeout(r, 0));
  await tick();
  await tick();

  // 'smart-features-picker' screen — skip it.
  const skipBtn = Array.from(container.querySelectorAll('button')).find((b) =>
    b.textContent?.includes("Skip — I'll set this up later")
  ) as HTMLButtonElement;
  if (!skipBtn) {
    throw new Error('DEBUG smart-features-picker skip button not found. HTML:\n' + container.innerHTML.slice(0, 3000));
  }
  await fireEvent.click(skipBtn);
  await tick();
  await tick();

  // 'main' screen — name the agent.
  const nameInput = container.querySelector('.agent-name-input') as HTMLInputElement;
  await fireEvent.input(nameInput, { target: { value: agentName } });
  await tick();
}

beforeEach(async () => {
  invokeMock.mockReset();
  callOrder = [];
  listenCallbacks.length = 0;
  // D26/PR-B (#2673): createWorkshopSession is now awaited BEFORE
  // streamAndCollect(), so its resolved id must be armed every test — plain
  // vi.fn()s created inside a vi.mock() factory get wiped back to a no-op by
  // afterEach's vi.restoreAllMocks() (same footgun documented in
  // Onboarding.workshopSession.test.ts's beforeEach for the `listen` mock).
  const { createWorkshopSession } = await import('$lib/onboarding/workshopSession');
  (createWorkshopSession as any).mockResolvedValue('workshop-sess-mock-1');
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('Onboarding synthesis ordering (#2634 D10 fix)', () => {
  it('upload path: upsert_agent is invoked (and resolves) before ai_chat_stream is invoked', async () => {
    invokeMock.mockImplementation(
      defaultInvokeImpl({
        fetch_available_models: async () => [
          { id: 'claude-opus-4-5', name: 'Claude Opus 4.5', provider: 'anthropic' },
        ],
      })
    );

    const { container } = render(Onboarding, { props: { onComplete: vi.fn() } });
    await navigateToMainScreen(container, 'Atlas');

    const uploadCard = Array.from(container.querySelectorAll('.path-card')).find((el) =>
      el.textContent?.includes('Upload Documents')
    ) as HTMLElement;
    await fireEvent.click(uploadCard);
    await tick();

    const fileInput = container.querySelector('#file-input') as HTMLInputElement;
    const file = {
      name: 'about-me.txt',
      text: () => Promise.resolve('some content about me'),
    };
    Object.defineProperty(fileInput, 'files', { value: [file] });
    await fireEvent.change(fileInput);
    await new Promise((r) => setTimeout(r, 0));
    await tick();
    await tick();

    const processBtn = Array.from(container.querySelectorAll('button')).find((b) =>
      b.textContent?.trim() === 'Process'
    ) as HTMLButtonElement;
    await fireEvent.click(processBtn);

    // Let ensureMainAgent() (upsert_agent) resolve and streamAndCollect() start
    // (ai_chat_stream invoked, listener registered) before completing the stream.
    await new Promise((r) => setTimeout(r, 0));
    await tick();
    await new Promise((r) => setTimeout(r, 0));
    await tick();

    completeStream();
    await new Promise((r) => setTimeout(r, 0));
    await tick();
    await new Promise((r) => setTimeout(r, 0));
    await tick();

    expect(callOrder).toContain('upsert_agent');
    expect(callOrder).toContain('ai_chat_stream');
    const upsertIdx = callOrder.indexOf('upsert_agent');
    const streamIdx = callOrder.indexOf('ai_chat_stream');
    expect(upsertIdx).toBeGreaterThanOrEqual(0);
    expect(streamIdx).toBeGreaterThan(upsertIdx);

    // D26/PR-B (#2673): ai_chat_stream must be invoked with the real,
    // agent-bound workshop sessionId (not undefined/omitted) as a top-level
    // param — createWorkshopSession (mocked above) is now called BEFORE
    // streamAndCollect, so its resolved id must be threaded through.
    const streamCall = invokeMock.mock.calls.find(([cmd]) => cmd === 'ai_chat_stream');
    expect(streamCall?.[1].sessionId).toBe('workshop-sess-mock-1');

    // Synthesis output written to the correct (just-created) agent's files.
    const writeCalls = invokeMock.mock.calls.filter(([cmd]) => cmd === 'write_system_file');
    expect(writeCalls.length).toBeGreaterThan(0);
    for (const [, args] of writeCalls) {
      expect(args.agentId).toBe(CREATED_AGENT_ID);
    }
    const memoryWrite = writeCalls.find(([, args]) => args.filename === 'MEMORY.md');
    const soulWrite = writeCalls.find(([, args]) => args.filename === 'SOUL.md');
    expect(memoryWrite?.[1].content).toBe('Memory text');
    expect(soulWrite?.[1].content).toBe('Soul text');
  });

  it('questions path: upsert_agent is invoked (and resolves) before ai_chat_stream is invoked', async () => {
    invokeMock.mockImplementation(
      defaultInvokeImpl({
        fetch_available_models: async () => [
          { id: 'claude-opus-4-5', name: 'Claude Opus 4.5', provider: 'anthropic' },
        ],
      })
    );

    const { container } = render(Onboarding, { props: { onComplete: vi.fn() } });
    await navigateToMainScreen(container, 'Atlas');

    const questionsCard = Array.from(container.querySelectorAll('.path-card')).find((el) =>
      el.textContent?.includes('Answer a Few Questions')
    ) as HTMLElement;
    await fireEvent.click(questionsCard);
    await tick();

    const nameQInput = container.querySelector('#q-name') as HTMLInputElement;
    await fireEvent.input(nameQInput, { target: { value: 'Britt' } });
    await tick();

    const continueBtn = Array.from(container.querySelectorAll('button')).find((b) =>
      b.textContent?.trim() === 'Continue'
    ) as HTMLButtonElement;
    await fireEvent.click(continueBtn);

    // Extra await cycle vs. the upload path: D26/PR-B interposes an awaited
    // createWorkshopSession() call before streamAndCollect() now runs.
    await new Promise((r) => setTimeout(r, 0));
    await tick();
    await new Promise((r) => setTimeout(r, 0));
    await tick();
    await new Promise((r) => setTimeout(r, 0));
    await tick();

    completeStream();
    await new Promise((r) => setTimeout(r, 0));
    await tick();
    await new Promise((r) => setTimeout(r, 0));
    await tick();

    expect(callOrder).toContain('upsert_agent');
    expect(callOrder).toContain('ai_chat_stream');
    const upsertIdx = callOrder.indexOf('upsert_agent');
    const streamIdx = callOrder.indexOf('ai_chat_stream');
    expect(upsertIdx).toBeGreaterThanOrEqual(0);
    expect(streamIdx).toBeGreaterThan(upsertIdx);

    // D26/PR-B (#2673): same sessionId-threading check as the upload path.
    const streamCall = invokeMock.mock.calls.find(([cmd]) => cmd === 'ai_chat_stream');
    expect(streamCall?.[1].sessionId).toBe('workshop-sess-mock-1');

    const writeCalls = invokeMock.mock.calls.filter(([cmd]) => cmd === 'write_system_file');
    expect(writeCalls.length).toBeGreaterThan(0);
    for (const [, args] of writeCalls) {
      expect(args.agentId).toBe(CREATED_AGENT_ID);
    }
  });
});

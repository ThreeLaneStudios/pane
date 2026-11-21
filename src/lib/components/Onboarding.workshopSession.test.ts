/**
 * Onboarding.workshopSession.test.ts — regression coverage for #2618, ruling 5.
 *
 * Matt's ruling: "Write a test asserting the workshop session's agent_id
 * equals the created agent's id, in both onboarding and create-agent flows."
 * This file covers the onboarding half; AgentCreateOverlay.workshopSession.test.ts
 * covers the create-agent half.
 *
 * WorkshopReview.test.ts already proves sessionId threads into ai_chat_stream
 * given a sessionId prop. workshopSession.test.ts already proves
 * createWorkshopSession() calls api.createSession(WORKSHOP_SESSION_SCOPE_ID,
 * agentId) with whatever agentId it's given. The gap this suite closes: proving
 * Onboarding.svelte itself calls createWorkshopSession() with the SAME agent id
 * that ensureMainAgent()/upsert_agent just created — not a stale id, not the
 * empty string, not some other agent's id — for both the upload path
 * (processUpload → screen='workshop') and the questions path
 * (processQuestions → screen='workshop').
 *
 * Approach: mock `$lib/onboarding/seedGeneration` and
 * `$lib/onboarding/workshopSession` entirely (both are already unit-tested in
 * their own suites) so this test only has to drive Onboarding.svelte's screen
 * transitions and assert the wiring between ensureMainAgent()'s returned id and
 * createWorkshopSession()'s call arg — not re-verify generation/session-creation
 * internals.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, fireEvent } from '@testing-library/svelte/svelte5';
import { tick } from 'svelte';
import Onboarding from './Onboarding.svelte';

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: invokeMock,
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

const { createWorkshopSessionMock } = vi.hoisted(() => ({
  createWorkshopSessionMock: vi.fn(),
}));
vi.mock('$lib/onboarding/workshopSession', () => ({
  createWorkshopSession: createWorkshopSessionMock,
}));

const { streamAndCollectMock } = vi.hoisted(() => ({
  streamAndCollectMock: vi.fn(),
}));
vi.mock('$lib/onboarding/seedGeneration', () => ({
  streamAndCollect: streamAndCollectMock,
  parseSoulAndMemory: vi.fn(() => ({ soul: 'Soul text', memory: 'Memory text' })),
  buildUploadExtractionPrompt: vi.fn(() => 'upload prompt'),
  buildInterviewExtractionPrompt: vi.fn(() => 'interview prompt'),
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

const CREATED_AGENT_ID = 'agent-created-during-onboarding-42';

function defaultInvokeImpl(overrides: Record<string, (args: any) => any> = {}) {
  return async (cmd: string, args: any = {}) => {
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
        // Fresh install throughout — never short-circuits into agent-names.
        return [];
      case 'get_main_agent':
        // No pre-existing main agent — ensureMainAgent() must create one.
        throw new Error('no main agent');
      case 'upsert_agent':
        return { id: CREATED_AGENT_ID, name: args?.request?.name || 'My Agent' };
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
  createWorkshopSessionMock.mockReset();
  createWorkshopSessionMock.mockResolvedValue('workshop-sess-mock-1');
  streamAndCollectMock.mockReset();
  streamAndCollectMock.mockResolvedValue('---SOUL---\nSoul text\n---MEMORY---\nMemory text');
  // `vi.restoreAllMocks()` in afterEach resets `listen`'s mockResolvedValue
  // back to undefined (restoreAllMocks behaves like mockReset for plain
  // vi.fn()s, not just vi.spyOn ones) — re-arm it every test so
  // WorkshopReview.svelte's stream-listener $effect doesn't call an
  // undefined-returning `listen()` after the first test.
  const { listen } = await import('@tauri-apps/api/event');
  (listen as any).mockResolvedValue(() => {});
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('Onboarding → workshop session agent_id binding (#2618 ruling 5)', () => {
  it('upload path: createWorkshopSession is called with the just-created main agent id', async () => {
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

    // Drop a file in via the hidden file input's onchange handler equivalent —
    // simplest is to directly dispatch a change event with a File-like object.
    // jsdom's File lacks a working .text() in this test env, so stub it (same
    // shape handleFileInput needs: .name + .text()).
    const fileInput = container.querySelector('#file-input') as HTMLInputElement;
    const file = {
      name: 'about-me.txt',
      text: () => Promise.resolve('some content about me'),
    };
    Object.defineProperty(fileInput, 'files', { value: [file] });
    await fireEvent.change(fileInput);
    // file.text() resolves async — allow the uploadedFiles state update to land.
    await new Promise((r) => setTimeout(r, 0));
    await tick();
    await tick();

    const processBtn = Array.from(container.querySelectorAll('button')).find((b) =>
      b.textContent?.trim() === 'Process'
    ) as HTMLButtonElement;
    await fireEvent.click(processBtn);

    // processUpload() is async (streamAndCollect, ensureMainAgent, ensureNote,
    // createWorkshopSession all awaited in sequence) — flush the microtask queue.
    await new Promise((r) => setTimeout(r, 0));
    await tick();
    await new Promise((r) => setTimeout(r, 0));
    await tick();

    expect(createWorkshopSessionMock).toHaveBeenCalledTimes(1);
    expect(createWorkshopSessionMock).toHaveBeenCalledWith(CREATED_AGENT_ID);

    // D26/PR-B (#2673): streamAndCollect must receive the workshop session id
    // createWorkshopSession just returned, AND createWorkshopSession must have
    // run first (its mock order index must precede streamAndCollect's).
    expect(streamAndCollectMock).toHaveBeenCalledWith('upload prompt', 'workshop-sess-mock-1');
    const createOrder = createWorkshopSessionMock.mock.invocationCallOrder[0];
    const streamOrder = streamAndCollectMock.mock.invocationCallOrder[0];
    expect(createOrder).toBeLessThan(streamOrder);
  });

  it('questions path: createWorkshopSession is called with the just-created main agent id', async () => {
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

    await new Promise((r) => setTimeout(r, 0));
    await tick();
    await new Promise((r) => setTimeout(r, 0));
    await tick();

    expect(createWorkshopSessionMock).toHaveBeenCalledTimes(1);
    expect(createWorkshopSessionMock).toHaveBeenCalledWith(CREATED_AGENT_ID);

    expect(streamAndCollectMock).toHaveBeenCalledWith('interview prompt', 'workshop-sess-mock-1');
    const createOrder = createWorkshopSessionMock.mock.invocationCallOrder[0];
    const streamOrder = streamAndCollectMock.mock.invocationCallOrder[0];
    expect(createOrder).toBeLessThan(streamOrder);
  });
});

/**
 * Onboarding.processingError.test.ts — regression coverage for #2676.
 *
 * processUpload()/processQuestions() previously called ensureMainAgent() and
 * createWorkshopSession() as bare awaits with no surrounding try/catch. A
 * throw from either call was an unhandled promise rejection that left the
 * user stuck on the 'processing' screen forever — no error message, no Back
 * button, no recovery path.
 *
 * This suite proves that a failure from either call:
 *   1. is caught (no unhandled rejection escapes the component),
 *   2. populates `processingError` with a user-visible message, and
 *   3. resets `screen` back to the originating screen ('upload' or
 *      'questions') so the user isn't stranded and can retry.
 *
 * Mirrors the mock-setup pattern from Onboarding.workshopSession.test.ts
 * (mocks $lib/onboarding/workshopSession and $lib/onboarding/seedGeneration
 * wholesale) and the try/catch assertion shape from
 * AgentCreateOverlay.workshopSession.test.ts.
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

const CREATED_AGENT_ID = 'agent-created-during-onboarding-2676';

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
        return [];
      case 'get_main_agent':
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

  const skipBtn = Array.from(container.querySelectorAll('button')).find((b) =>
    b.textContent?.includes("Skip — I'll set this up later")
  ) as HTMLButtonElement;
  if (!skipBtn) {
    throw new Error('DEBUG smart-features-picker skip button not found. HTML:\n' + container.innerHTML.slice(0, 3000));
  }
  await fireEvent.click(skipBtn);
  await tick();
  await tick();

  const nameInput = container.querySelector('.agent-name-input') as HTMLInputElement;
  await fireEvent.input(nameInput, { target: { value: agentName } });
  await tick();
}

beforeEach(async () => {
  invokeMock.mockReset();
  createWorkshopSessionMock.mockReset();
  createWorkshopSessionMock.mockResolvedValue('workshop-sess-mock-err');
  streamAndCollectMock.mockReset();
  streamAndCollectMock.mockResolvedValue('---SOUL---\nSoul text\n---MEMORY---\nMemory text');
  const { listen } = await import('@tauri-apps/api/event');
  (listen as any).mockResolvedValue(() => {});
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('Onboarding → processing error handling (#2676)', () => {
  it('processUpload: ensureMainAgent() failure sets processingError and resets screen to upload', async () => {
    invokeMock.mockImplementation(
      defaultInvokeImpl({
        fetch_available_models: async () => [
          { id: 'claude-opus-4-5', name: 'Claude Opus 4.5', provider: 'anthropic' },
        ],
        // ensureMainAgent()'s fallback chain: get_main_agent throws (no
        // agent), get_agents returns empty (fresh install), then it falls
        // through to upsert_agent — make THAT throw to simulate the
        // unguarded-await failure this issue is about.
        upsert_agent: async () => {
          throw new Error('upsert_agent: disk full');
        },
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
    const file = { name: 'about-me.txt', text: () => Promise.resolve('some content about me') };
    Object.defineProperty(fileInput, 'files', { value: [file] });
    await fireEvent.change(fileInput);
    await new Promise((r) => setTimeout(r, 0));
    await tick();
    await tick();

    const processBtn = Array.from(container.querySelectorAll('button')).find((b) =>
      b.textContent?.trim() === 'Process'
    ) as HTMLButtonElement;
    await fireEvent.click(processBtn);

    // processUpload() is async — flush the microtask queue for the outer
    // try/catch to run and settle.
    await new Promise((r) => setTimeout(r, 0));
    await tick();
    await new Promise((r) => setTimeout(r, 0));
    await tick();

    // Should NOT have advanced to the workshop screen.
    expect(container.querySelector('.upload-intro')).toBeTruthy();
    expect(container.textContent).not.toContain('This is your');

    // Error message is rendered inline via the shared .connect-error-msg class
    // on the 'upload' screen (matches the pattern used 9+ other places in
    // this file), and the user can see + retry via the still-present Process
    // button.
    const errorEl = container.querySelector('.connect-error-msg');
    expect(errorEl).toBeTruthy();
    expect(errorEl?.textContent).toContain('disk full');

    // createWorkshopSession must never have been reached — ensureMainAgent()
    // threw before it.
    expect(createWorkshopSessionMock).not.toHaveBeenCalled();

    // Back button is present and clears the error.
    const backBtn = Array.from(container.querySelectorAll('button')).find((b) =>
      b.textContent?.trim() === 'Back'
    ) as HTMLButtonElement;
    expect(backBtn).toBeTruthy();
    await fireEvent.click(backBtn);
    await tick();
    // Navigated away from 'upload' — error state shouldn't resurface if the
    // user comes back without retrying.
    expect(container.querySelector('.connect-error-msg')).toBeFalsy();
  });

  it('processUpload: createWorkshopSession() failure sets processingError and resets screen to upload', async () => {
    invokeMock.mockImplementation(
      defaultInvokeImpl({
        fetch_available_models: async () => [
          { id: 'claude-opus-4-5', name: 'Claude Opus 4.5', provider: 'anthropic' },
        ],
      })
    );
    createWorkshopSessionMock.mockRejectedValueOnce(new Error('createWorkshopSession: db locked'));

    const { container } = render(Onboarding, { props: { onComplete: vi.fn() } });
    await navigateToMainScreen(container, 'Atlas');

    const uploadCard = Array.from(container.querySelectorAll('.path-card')).find((el) =>
      el.textContent?.includes('Upload Documents')
    ) as HTMLElement;
    await fireEvent.click(uploadCard);
    await tick();

    const fileInput = container.querySelector('#file-input') as HTMLInputElement;
    const file = { name: 'about-me.txt', text: () => Promise.resolve('some content about me') };
    Object.defineProperty(fileInput, 'files', { value: [file] });
    await fireEvent.change(fileInput);
    await new Promise((r) => setTimeout(r, 0));
    await tick();
    await tick();

    const processBtn = Array.from(container.querySelectorAll('button')).find((b) =>
      b.textContent?.trim() === 'Process'
    ) as HTMLButtonElement;
    await fireEvent.click(processBtn);

    await new Promise((r) => setTimeout(r, 0));
    await tick();
    await new Promise((r) => setTimeout(r, 0));
    await tick();

    expect(container.querySelector('.upload-intro')).toBeTruthy();
    const errorEl = container.querySelector('.connect-error-msg');
    expect(errorEl).toBeTruthy();
    expect(errorEl?.textContent).toContain('db locked');

    // streamAndCollect must never have been reached — createWorkshopSession
    // threw before it could run.
    expect(streamAndCollectMock).not.toHaveBeenCalled();
  });

  it('processQuestions: ensureMainAgent() failure sets processingError and resets screen to questions', async () => {
    invokeMock.mockImplementation(
      defaultInvokeImpl({
        fetch_available_models: async () => [
          { id: 'claude-opus-4-5', name: 'Claude Opus 4.5', provider: 'anthropic' },
        ],
        upsert_agent: async () => {
          throw new Error('upsert_agent: connection refused');
        },
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

    // Should still be on the questions screen, not stuck on 'processing'.
    expect(container.querySelector('#q-name')).toBeTruthy();

    const errorEl = container.querySelector('.connect-error-msg');
    expect(errorEl).toBeTruthy();
    expect(errorEl?.textContent).toContain('connection refused');

    expect(createWorkshopSessionMock).not.toHaveBeenCalled();
  });

  it('processQuestions: createWorkshopSession() failure sets processingError and resets screen to questions', async () => {
    invokeMock.mockImplementation(
      defaultInvokeImpl({
        fetch_available_models: async () => [
          { id: 'claude-opus-4-5', name: 'Claude Opus 4.5', provider: 'anthropic' },
        ],
      })
    );
    createWorkshopSessionMock.mockRejectedValueOnce(new Error('createWorkshopSession: db locked'));

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

    expect(container.querySelector('#q-name')).toBeTruthy();
    const errorEl = container.querySelector('.connect-error-msg');
    expect(errorEl).toBeTruthy();
    expect(errorEl?.textContent).toContain('db locked');
    expect(streamAndCollectMock).not.toHaveBeenCalled();
  });
});

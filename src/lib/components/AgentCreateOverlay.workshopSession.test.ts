/**
 * AgentCreateOverlay.workshopSession.test.ts — regression coverage for #2618,
 * ruling 5.
 *
 * Matt's ruling: "Write a test asserting the workshop session's agent_id
 * equals the created agent's id, in both onboarding and create-agent flows."
 * This file covers the create-agent half; Onboarding.workshopSession.test.ts
 * covers the onboarding half.
 *
 * As with the onboarding suite, `$lib/onboarding/workshopSession` and
 * `$lib/onboarding/seedGeneration` are mocked wholesale (both already have
 * their own unit-test coverage) — this suite only proves
 * AgentCreateOverlay.svelte calls createWorkshopSession() with the SAME
 * agent id createLocalAgent() just returned, for both the upload seeding path
 * and the interview seeding path.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, fireEvent } from '@testing-library/svelte/svelte5';
import { tick } from 'svelte';
import AgentCreateOverlay from './AgentCreateOverlay.svelte';
import type { RuntimeConnectionInfo } from '$lib/api';
import type { Agent } from '$lib/types';

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: invokeMock,
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

vi.mock('$lib/api', async (importOriginal) => {
  const original = (await importOriginal()) as any;
  return {
    ...original,
    listRuntimeConnections: vi.fn(),
    createLocalAgent: vi.fn(),
    checkNameCollision: vi.fn().mockResolvedValue(false),
  };
});

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

import { listRuntimeConnections, createLocalAgent } from '$lib/api';

if (typeof (globalThis.window as any).addEventListener !== 'function') {
  (globalThis.window as any).addEventListener = () => {};
}
if (typeof (globalThis.window as any).removeEventListener !== 'function') {
  (globalThis.window as any).removeEventListener = () => {};
}
if (typeof (globalThis.window as any).dispatchEvent !== 'function') {
  (globalThis.window as any).dispatchEvent = () => true;
}

function makeConn(overrides: Partial<RuntimeConnectionInfo> = {}): RuntimeConnectionInfo {
  return {
    runtime_type: 'anthropic',
    connected: true,
    masked_credential: 'sk-ant-...abcd',
    endpoint: null,
    error: null,
    agents: [],
    ...overrides,
  };
}

const CREATED_AGENT_ID = 'agent-created-during-create-flow-77';

function makeAgent(overrides: Partial<Agent> = {}): Agent {
  return {
    id: CREATED_AGENT_ID,
    name: 'Research',
    slug: 'research',
    role: 'sub',
    model: null,
    is_default: true,
    is_active: true,
    workspace_path: 'ABC123',
    config_json: '{}',
    runtime_ref: null,
    runtime_type: 'anthropic',
    source: 'ai_created',
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    ...overrides,
  };
}

async function pickSearchableModel(container: HTMLElement, label: string) {
  const input = container.querySelector('.model-input') as HTMLInputElement;
  await fireEvent.focus(input);
  await tick();
  const items = Array.from(container.querySelectorAll('.dropdown-item'));
  const item = items.find((el) => el.textContent?.includes(label)) as HTMLElement;
  await fireEvent.mouseDown(item);
  await tick();
}

async function createAgentUpToSeedingChooser(container: HTMLElement) {
  await tick();
  await tick();

  await fireEvent.click(container.querySelector('[data-runtime="anthropic"]') as HTMLElement);
  await tick();
  await tick();
  await pickSearchableModel(container, 'Claude Opus 4.5');

  const nameInput = container.querySelector('.agent-name-input') as HTMLInputElement;
  await fireEvent.input(nameInput, { target: { value: 'New Agent' } });
  await tick();

  const createBtn = Array.from(container.querySelectorAll('.ag-btn--primary')).find((b) =>
    b.textContent?.includes('Create New Agent')
  ) as HTMLButtonElement;
  await fireEvent.click(createBtn);
  await tick();
  await tick();
}

beforeEach(async () => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(null);
  (listRuntimeConnections as any).mockReset();
  (listRuntimeConnections as any).mockResolvedValue([makeConn({ runtime_type: 'anthropic', connected: true })]);
  (createLocalAgent as any).mockReset();
  (createLocalAgent as any).mockResolvedValue(makeAgent());
  createWorkshopSessionMock.mockReset();
  createWorkshopSessionMock.mockResolvedValue('workshop-sess-mock-2');
  streamAndCollectMock.mockReset();
  streamAndCollectMock.mockResolvedValue('---SOUL---\nSoul text\n---MEMORY---\nMemory text');
  // See Onboarding.workshopSession.test.ts's beforeEach comment: vi.restoreAllMocks()
  // in afterEach wipes the plain vi.fn() `listen` mock's return value too.
  const { listen } = await import('@tauri-apps/api/event');
  (listen as any).mockResolvedValue(() => {});
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('AgentCreateOverlay → workshop session agent_id binding (#2618 ruling 5)', () => {
  it('upload path: createWorkshopSession is called with the just-created agent id', async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'fetch_available_models') {
        return [{ id: 'claude-opus-4-5', name: 'Claude Opus 4.5', provider: 'anthropic' }];
      }
      if (cmd === 'get_notes') return [];
      if (cmd === 'create_note') return { id: 'note-x' };
      return null;
    });

    const { container } = render(AgentCreateOverlay, { props: { onClose: vi.fn() } });
    await createAgentUpToSeedingChooser(container);

    const uploadCard = Array.from(container.querySelectorAll('.path-card')).find((el) =>
      el.textContent?.includes('Upload Documents')
    ) as HTMLElement;
    await fireEvent.click(uploadCard);
    await tick();

    const fileInput = container.querySelector('#agent-file-input') as HTMLInputElement;
    const file = { name: 'about-agent.txt', text: () => Promise.resolve('agent context') };
    Object.defineProperty(fileInput, 'files', { value: [file] });
    await fireEvent.change(fileInput);
    await new Promise((r) => setTimeout(r, 0));
    await tick();
    await tick();

    const saveBtn = Array.from(container.querySelectorAll('.ag-btn--primary')).find((b) =>
      b.textContent?.includes('& continue')
    ) as HTMLButtonElement;
    await fireEvent.click(saveBtn);

    await new Promise((r) => setTimeout(r, 0));
    await tick();
    await new Promise((r) => setTimeout(r, 0));
    await tick();

    expect(createWorkshopSessionMock).toHaveBeenCalledTimes(1);
    expect(createWorkshopSessionMock).toHaveBeenCalledWith(CREATED_AGENT_ID);

    // D26/PR-B (#2673): streamAndCollect must receive the workshop session id
    // createWorkshopSession just returned, AND must run AFTER createWorkshopSession.
    expect(streamAndCollectMock).toHaveBeenCalledWith('upload prompt', 'workshop-sess-mock-2');
    const createOrder = createWorkshopSessionMock.mock.invocationCallOrder[0];
    const streamOrder = streamAndCollectMock.mock.invocationCallOrder[0];
    expect(createOrder).toBeLessThan(streamOrder);
  });

  it('interview path: createWorkshopSession is called with the just-created agent id', async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'fetch_available_models') {
        return [{ id: 'claude-opus-4-5', name: 'Claude Opus 4.5', provider: 'anthropic' }];
      }
      if (cmd === 'get_notes') return [];
      if (cmd === 'create_note') return { id: 'note-x' };
      return null;
    });

    const { container } = render(AgentCreateOverlay, { props: { onClose: vi.fn() } });
    await createAgentUpToSeedingChooser(container);

    const interviewCard = Array.from(container.querySelectorAll('.path-card')).find((el) =>
      el.textContent?.includes('Answer a Few Questions')
    ) as HTMLElement;
    await fireEvent.click(interviewCard);
    await tick();

    const answerBox = container.querySelector('.agent-interview-answer') as HTMLTextAreaElement;
    await fireEvent.input(answerBox, { target: { value: 'Research and writing.' } });
    await tick();

    const finishBtn = Array.from(container.querySelectorAll('.ag-btn--primary')).find((b) =>
      b.textContent?.includes('Done')
    ) as HTMLButtonElement;
    expect(finishBtn).toBeFalsy(); // Not yet on last question — Next appears first instead.

    // Advance through remaining questions with Next.
    for (let i = 0; i < 4; i++) {
      const nextBtn = Array.from(container.querySelectorAll('.ag-btn--primary')).find((b) =>
        b.textContent?.includes('Next')
      ) as HTMLButtonElement;
      await fireEvent.click(nextBtn);
      await tick();
    }

    const doneBtn = Array.from(container.querySelectorAll('.ag-btn--primary')).find((b) =>
      b.textContent?.includes('Done')
    ) as HTMLButtonElement;
    await fireEvent.click(doneBtn);

    await new Promise((r) => setTimeout(r, 0));
    await tick();
    await new Promise((r) => setTimeout(r, 0));
    await tick();

    expect(createWorkshopSessionMock).toHaveBeenCalledTimes(1);
    expect(createWorkshopSessionMock).toHaveBeenCalledWith(CREATED_AGENT_ID);

    expect(streamAndCollectMock).toHaveBeenCalledWith('interview prompt', 'workshop-sess-mock-2');
    const createOrder = createWorkshopSessionMock.mock.invocationCallOrder[0];
    const streamOrder = streamAndCollectMock.mock.invocationCallOrder[0];
    expect(createOrder).toBeLessThan(streamOrder);
  });
});

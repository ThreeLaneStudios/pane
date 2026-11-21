import { describe, it, expect, vi, beforeEach } from 'vitest';

// Mock $lib/api so we can assert exactly what createSession is called with,
// without touching the real Tauri invoke bridge.
const createSessionMock = vi.fn();
vi.mock('$lib/api', () => ({
  createSession: (...args: unknown[]) => createSessionMock(...args),
}));

import { createWorkshopSession } from './workshopSession';
import { WORKSHOP_SESSION_SCOPE_ID } from '$lib/constants';

describe('createWorkshopSession (#2618)', () => {
  beforeEach(() => {
    createSessionMock.mockReset();
  });

  it('calls api.createSession with WORKSHOP_SESSION_SCOPE_ID and the given agentId', async () => {
    createSessionMock.mockResolvedValue({ id: 'sess-abc-123', title: '' });

    const result = await createWorkshopSession('agent-xyz');

    expect(createSessionMock).toHaveBeenCalledTimes(1);
    expect(createSessionMock).toHaveBeenCalledWith(WORKSHOP_SESSION_SCOPE_ID, 'agent-xyz');
    expect(result).toBe('sess-abc-123');
  });

  it('never passes the notes-only WORKSHOP_SCOPE_ID sentinel as the session scope', async () => {
    createSessionMock.mockResolvedValue({ id: 'sess-1', title: '' });
    await createWorkshopSession('agent-1');
    const [scopeArg] = createSessionMock.mock.calls[0];
    expect(scopeArg).not.toBe('__workshop__');
    expect(scopeArg).toBe('__workshop_session__');
  });

  it('rejects an empty agentId without calling api.createSession', async () => {
    await expect(createWorkshopSession('')).rejects.toThrow(/agentId/);
    expect(createSessionMock).not.toHaveBeenCalled();
  });

  it('propagates errors from api.createSession rather than swallowing them', async () => {
    createSessionMock.mockRejectedValue(new Error('backend exploded'));
    await expect(createWorkshopSession('agent-1')).rejects.toThrow('backend exploded');
  });
});

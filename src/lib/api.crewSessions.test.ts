/**
 * api.crewSessions.test.ts — §36.17.4 (#3050 PR 10, Captain & Crew UI).
 *
 * Pins the exact wire shape of the two new session-crew wrapper functions:
 * `createSessionWithCrew` (New Session sheet, captain + optional crew) and
 * `updateSessionCrew` (edit-crew popover's "Save crew"). Both are additive
 * siblings of the existing `createSession` — unaffected, not touched here.
 */

import { describe, it, expect, vi, afterEach } from 'vitest';
import { createSessionWithCrew, updateSessionCrew } from '$lib/api';
import * as tauriCore from '@tauri-apps/api/core';

afterEach(() => {
  vi.restoreAllMocks();
});

describe('api.createSessionWithCrew', () => {
  it('invokes create_session_with_crew with the exact param shape, including an empty crew array', async () => {
    const invokeSpy = vi.spyOn(tauriCore, 'invoke').mockResolvedValue({
      id: 'sess-1', project_id: 'workspace', title: '', origin: 'user',
      created_at: '', last_active_at: '', hidden_at: null, is_active: true,
      agent_id: 'agent-1', crew_agent_ids: null,
    });

    await createSessionWithCrew('workspace', 'agent-1', []);

    // Shape-asserting, not just call-count — a camelCase/snake_case param
    // key mismatch (e.g. `crew_agent_ids` instead of `crewAgentIds`) would
    // red this, matching Reed's scope audit's explicit caution against a
    // call-count-only test for this kind of wrapper.
    expect(invokeSpy).toHaveBeenCalledWith('create_session_with_crew', {
      projectId: 'workspace',
      agentId: 'agent-1',
      crewAgentIds: [],
    });
  });

  it('passes a non-empty crew array through verbatim, order preserved', async () => {
    const invokeSpy = vi.spyOn(tauriCore, 'invoke').mockResolvedValue({
      id: 'sess-2', project_id: 'workspace', title: '', origin: 'user',
      created_at: '', last_active_at: '', hidden_at: null, is_active: true,
      agent_id: 'agent-1', crew_agent_ids: '["c2","c1"]',
    });

    await createSessionWithCrew('workspace', 'agent-1', ['c2', 'c1']);

    expect(invokeSpy).toHaveBeenCalledWith('create_session_with_crew', {
      projectId: 'workspace',
      agentId: 'agent-1',
      crewAgentIds: ['c2', 'c1'],
    });
  });

  it('resolves with the Session the backend returns', async () => {
    const backendSession = {
      id: 'sess-3', project_id: 'workspace', title: '', origin: 'user',
      created_at: '', last_active_at: '', hidden_at: null, is_active: true,
      agent_id: 'agent-1', crew_agent_ids: '["c1"]',
    };
    vi.spyOn(tauriCore, 'invoke').mockResolvedValue(backendSession);

    const result = await createSessionWithCrew('workspace', 'agent-1', ['c1']);
    expect(result).toEqual(backendSession);
  });
});

describe('api.updateSessionCrew', () => {
  it('invokes update_session_crew with the exact param shape — no agentId parameter at all (captain is immutable)', async () => {
    const invokeSpy = vi.spyOn(tauriCore, 'invoke').mockResolvedValue({
      id: 'sess-1', project_id: 'workspace', title: '', origin: 'user',
      created_at: '', last_active_at: '', hidden_at: null, is_active: true,
      agent_id: 'agent-1', crew_agent_ids: '["c1","c2"]',
    });

    await updateSessionCrew('sess-1', ['c1', 'c2']);

    expect(invokeSpy).toHaveBeenCalledWith('update_session_crew', {
      sessionId: 'sess-1',
      crewAgentIds: ['c1', 'c2'],
    });
    // Mutation-style assertion: no call anywhere ever carried an agentId key.
    for (const call of invokeSpy.mock.calls) {
      if (call[0] === 'update_session_crew') {
        expect(call[1]).not.toHaveProperty('agentId');
      }
    }
  });

  it('passes an empty crew array through (clearing a session\'s crew)', async () => {
    const invokeSpy = vi.spyOn(tauriCore, 'invoke').mockResolvedValue({
      id: 'sess-1', project_id: 'workspace', title: '', origin: 'user',
      created_at: '', last_active_at: '', hidden_at: null, is_active: true,
      agent_id: 'agent-1', crew_agent_ids: null,
    });

    await updateSessionCrew('sess-1', []);

    expect(invokeSpy).toHaveBeenCalledWith('update_session_crew', {
      sessionId: 'sess-1',
      crewAgentIds: [],
    });
  });
});

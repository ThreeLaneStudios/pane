/**
 * ocPairingCeremony.test.ts — unit tests for the shared OC pairing ceremony (#2649, PR 3A).
 *
 * Mocks `@tauri-apps/api/core`'s `invoke` and asserts:
 *  - runOcPairingExchange: happy path writes settings in canonical order, writes
 *    gateway_paired_at, and surfaces mapPairingError on failure.
 *  - runOcImportPhase: first-pair routes to sync_initial_import; re-pair routes
 *    through check_oc_agent_collisions -> confirmCollision (D20, #2649: prompted
 *    UNCONDITIONALLY, even with zero collisions) -> reimport_oc_agents; a
 *    declined confirmation resolves `{ ok: true, declined: true }` with empty
 *    imports (not an error, but structurally distinct from a successful
 *    reimport); sync-phase errors get parsed via parseGatewayError.
 *  - runOcPairingCeremony: composes both phases, calls onPaired at the boundary,
 *    and never runs the import phase when the exchange phase fails.
 *  - parseGatewayError: JSON envelope vs plain-string passthrough.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));

import {
  runOcPairingExchange,
  runOcImportPhase,
  runOcPairingCeremony,
  parseGatewayError,
  normalizeOcGatewayUrl,
} from './ocPairingCeremony';

beforeEach(() => {
  invokeMock.mockReset();
});

describe('normalizeOcGatewayUrl', () => {
  it.each([
    ['gateway.tailnet.ts.net', 'https://gateway.tailnet.ts.net:18789/v1'],
    ['https://100.64.0.1:8443', 'https://100.64.0.1:8443/v1'],
    ['https://127.0.0.1:18789/v1/', 'https://127.0.0.1:18789/v1'],
    ['https://[fd7a:115c:a1e0::1]:18789', 'https://[fd7a:115c:a1e0::1]:18789/v1'],
  ])('normalizes %s', (input, expected) => {
    expect(normalizeOcGatewayUrl(input)).toBe(expected);
  });

  it.each([
    'http://gateway.tailnet.ts.net:18789',
    'https://user:pass@gateway.tailnet.ts.net',
    'https://gateway.tailnet.ts.net?redirect=evil',
    'https://gateway.tailnet.ts.net#fragment',
    'https://gateway.tailnet.ts.net/v1/health',
  ])('rejects unsafe or unexpected input %s', (input) => {
    expect(() => normalizeOcGatewayUrl(input)).toThrow();
  });
});

describe('parseGatewayError', () => {
  it('parses a JSON envelope with error_code + message', () => {
    const result = parseGatewayError('{"error_code":"oc_handshake_timeout","message":"timed out"}');
    expect(result).toEqual({ code: 'oc_handshake_timeout', message: 'timed out' });
  });

  it('falls back to plain message when JSON parse fails', () => {
    const result = parseGatewayError('{not valid json');
    expect(result).toEqual({ code: null, message: '{not valid json' });
  });

  it('returns null code for a plain non-JSON string', () => {
    const result = parseGatewayError('Gateway sync timeout');
    expect(result).toEqual({ code: null, message: 'Gateway sync timeout' });
  });
});

describe('runOcPairingExchange', () => {
  it('happy path: writes settings in canonical order and returns token + versionWarning', async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'gateway_exchange_pairing_code') {
        return { token: 'tok-123', gateway_version: '0.70.0', version_warning: null };
      }
      return undefined;
    });

    const outcome = await runOcPairingExchange({
      url: 'https://gw:18789',
      code: '123456',
      fingerprint: 'AB:CD',
      certPem: '-----BEGIN CERT-----',
    });

    expect(outcome.ok).toBe(true);
    if (outcome.ok) {
      expect(outcome.token).toBe('tok-123');
      expect(outcome.versionWarning).toBeNull();
    }

    expect(invokeMock.mock.calls[0]).toEqual([
      'gateway_exchange_pairing_code',
      { url: 'https://gw:18789/v1', code: '123456' },
    ]);

    const calls = invokeMock.mock.calls.map((c) => c[0]);
    expect(calls).toEqual([
      'gateway_exchange_pairing_code',
      'set_ai_setting', // gateway_tls_fingerprint
      'set_ai_setting', // gateway_tls_cert_pem
      'set_ai_setting', // tier3_topology
      'ai_configure_and_save',
      'set_ai_setting', // gateway_paired_at
    ]);

    // Assert exact key/value ordering + content per docs/spec/sync.md §22C.22.4.
    expect(invokeMock.mock.calls[1][1]).toEqual({ key: 'gateway_tls_fingerprint', value: 'AB:CD' });
    expect(invokeMock.mock.calls[2][1]).toEqual({ key: 'gateway_tls_cert_pem', value: '-----BEGIN CERT-----' });
    expect(invokeMock.mock.calls[3][1]).toEqual({ key: 'tier3_topology', value: 'split_machine' });
    expect(invokeMock.mock.calls[4][1]).toEqual({
      provider: 'openclaw',
      apiKey: 'tok-123',
      model: null,
      endpoint: 'https://gw:18789/v1',
    });
    expect(invokeMock.mock.calls[5][1].key).toBe('gateway_paired_at');
    expect(typeof invokeMock.mock.calls[5][1].value).toBe('string');
  });

  it('rejects explicit HTTP before invoking the native exchange', async () => {
    const outcome = await runOcPairingExchange({
      url: 'http://gw:18789',
      code: '654321',
      fingerprint: 'FP',
      certPem: 'PEM',
    });

    expect(outcome).toEqual({
      ok: false,
      errorCode: null,
      errorMessage: 'Gateway URL must use HTTPS.',
    });
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it('surfaces the version_warning when present', async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'gateway_exchange_pairing_code') {
        return { token: 'tok-abc', gateway_version: '0.10.0', version_warning: 'Gateway is out of date.' };
      }
      return undefined;
    });

    const outcome = await runOcPairingExchange({
      url: 'https://gw:18789',
      code: '654321',
      fingerprint: 'FP',
      certPem: 'PEM',
    });

    expect(outcome.ok).toBe(true);
    if (outcome.ok) {
      expect(outcome.versionWarning).toBe('Gateway is out of date.');
    }
  });

  it('maps a bad-code failure through mapPairingError and writes nothing further', async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'gateway_exchange_pairing_code') {
        throw new Error('Invalid pairing code: nonsense');
      }
      return undefined;
    });

    const outcome = await runOcPairingExchange({
      url: 'https://gw:18789',
      code: '000000',
      fingerprint: 'FP',
      certPem: 'PEM',
    });

    expect(outcome.ok).toBe(false);
    if (!outcome.ok) {
      expect(outcome.errorMessage).toBe("Code didn't match. Try again or generate a new one.");
      expect(outcome.errorCode).toBeNull();
    }
    // Only the failed call was made — no settings writes on a pairing-code rejection.
    expect(invokeMock).toHaveBeenCalledTimes(1);
  });
});

describe('runOcImportPhase', () => {
  it('first pair: routes to sync_initial_import and returns imported names + missing-model list', async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'oc_agent_row_exists') return false;
      if (cmd === 'sync_initial_import') {
        return { agent_names: ['Brit Freed', 'Stia'], agents_missing_model: ['Stia'] };
      }
      throw new Error(`unexpected invoke: ${cmd}`);
    });

    const confirmCollision = vi.fn();
    const outcome = await runOcImportPhase({
      url: 'https://gw:18789',
      token: 'tok-123',
      confirmCollision,
    });

    expect(outcome.ok).toBe(true);
    if (outcome.ok) {
      expect(outcome.isFirstPair).toBe(true);
      expect(outcome.importedAgentNames).toEqual(['Brit Freed', 'Stia']);
      expect(outcome.agentsMissingModel).toEqual(['Stia']);
    }
    expect(confirmCollision).not.toHaveBeenCalled();
    expect(invokeMock).not.toHaveBeenCalledWith('reimport_oc_agents', expect.anything());
  });

  it('re-pair with zero collisions: still prompts confirmCollision with an empty array (D20, #2649)', async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'oc_agent_row_exists') return true;
      if (cmd === 'check_oc_agent_collisions') return { collisions: [], new_agents: [] };
      if (cmd === 'reimport_oc_agents') return ['Brit Freed'];
      throw new Error(`unexpected invoke: ${cmd}`);
    });

    const confirmCollision = vi.fn().mockResolvedValue(true);
    const outcome = await runOcImportPhase({
      url: 'https://gw:18789',
      token: 'tok-123',
      confirmCollision,
    });

    // D20 ruling #2: the dialog is now unconditional on every re-pair, even
    // with zero detected collisions — this assertion was previously
    // `not.toHaveBeenCalled()`; it must flip once the gate is removed.
    expect(confirmCollision).toHaveBeenCalledWith([]);
    expect(outcome.ok).toBe(true);
    if (outcome.ok) {
      expect(outcome.isFirstPair).toBe(false);
      expect(outcome.importedAgentNames).toEqual(['Brit Freed']);
      expect(outcome.agentsMissingModel).toEqual([]);
      expect(outcome.declined).toBeFalsy();
    }
  });

  it('re-pair with collisions: prompts confirmCollision, proceeds on true, declined is falsy', async () => {
    const collisions = [{ oc_agent_id: 'main', local_name: 'Old Name', gateway_name: 'New Name' }];
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'oc_agent_row_exists') return true;
      if (cmd === 'check_oc_agent_collisions') return { collisions, new_agents: [] };
      if (cmd === 'reimport_oc_agents') return ['New Name'];
      throw new Error(`unexpected invoke: ${cmd}`);
    });

    const confirmCollision = vi.fn().mockResolvedValue(true);
    const outcome = await runOcImportPhase({
      url: 'https://gw:18789',
      token: 'tok-123',
      confirmCollision,
    });

    expect(confirmCollision).toHaveBeenCalledWith(collisions);
    expect(invokeMock).toHaveBeenCalledWith('reimport_oc_agents', expect.anything());
    expect(outcome.ok).toBe(true);
    if (outcome.ok) {
      expect(outcome.importedAgentNames).toEqual(['New Name']);
      expect(outcome.declined).toBeFalsy();
    }
  });

  it('re-pair with collisions declined: resolves with declined:true, does not call reimport_oc_agents (D20, #2649)', async () => {
    const collisions = [{ oc_agent_id: 'main', local_name: 'Old Name', gateway_name: 'New Name' }];
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'oc_agent_row_exists') return true;
      if (cmd === 'check_oc_agent_collisions') return { collisions, new_agents: [] };
      if (cmd === 'reimport_oc_agents') throw new Error('must not be called');
      throw new Error(`unexpected invoke: ${cmd}`);
    });

    const confirmCollision = vi.fn().mockResolvedValue(false);
    const outcome = await runOcImportPhase({
      url: 'https://gw:18789',
      token: 'tok-123',
      confirmCollision,
    });

    expect(outcome.ok).toBe(true);
    if (outcome.ok) {
      // Structurally distinct from success — not just "empty arrays", the
      // explicit `declined` flag (D20 ruling #3).
      expect(outcome.declined).toBe(true);
      expect(outcome.importedAgentNames).toEqual([]);
      expect(outcome.agentsMissingModel).toEqual([]);
    }
    expect(invokeMock).not.toHaveBeenCalledWith('reimport_oc_agents', expect.anything());
  });

  it('sync-phase failure: returns ok:false with parsed structured error', async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'oc_agent_row_exists') return false;
      if (cmd === 'sync_initial_import') {
        throw new Error('{"error_code":"oc_handshake_timeout","message":"handshake timed out"}');
      }
      throw new Error(`unexpected invoke: ${cmd}`);
    });

    const outcome = await runOcImportPhase({
      url: 'https://gw:18789',
      token: 'tok-123',
      confirmCollision: vi.fn(),
    });

    expect(outcome.ok).toBe(false);
    if (!outcome.ok) {
      expect(outcome.errorCode).toBe('oc_handshake_timeout');
      expect(outcome.errorMessage).toBe('handshake timed out');
    }
  });
});

describe('runOcPairingCeremony', () => {
  it('composes both phases and fires onPaired at the boundary before the import phase runs', async () => {
    const callOrder: string[] = [];
    invokeMock.mockImplementation(async (cmd: string) => {
      callOrder.push(cmd);
      if (cmd === 'gateway_exchange_pairing_code') {
        return { token: 'tok-999', gateway_version: '0.70.0', version_warning: null };
      }
      if (cmd === 'oc_agent_row_exists') return false;
      if (cmd === 'sync_initial_import') return { agent_names: ['Brit'], agents_missing_model: [] };
      return undefined;
    });

    const onPaired = vi.fn(() => callOrder.push('onPaired'));

    const outcome = await runOcPairingCeremony({
      url: 'https://gw:18789',
      code: '111111',
      fingerprint: 'FP',
      certPem: 'PEM',
      confirmCollision: vi.fn(),
      onPaired,
    });

    expect(outcome.ok).toBe(true);
    if (outcome.ok) {
      expect(outcome.token).toBe('tok-999');
      expect(outcome.importedAgentNames).toEqual(['Brit']);
      expect(outcome.isFirstPair).toBe(true);
    }
    expect(onPaired).toHaveBeenCalledWith('tok-999', null);

    // onPaired fires strictly between the exchange call and the import-phase calls.
    const pairedIndex = callOrder.indexOf('onPaired');
    const rowExistsIndex = callOrder.indexOf('oc_agent_row_exists');
    expect(pairedIndex).toBeGreaterThan(callOrder.indexOf('gateway_exchange_pairing_code'));
    expect(rowExistsIndex).toBeGreaterThan(pairedIndex);
  });

  it('never runs the import phase when the exchange phase fails', async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'gateway_exchange_pairing_code') {
        throw new Error('Pairing code expired');
      }
      throw new Error(`unexpected invoke: ${cmd}`);
    });

    const onPaired = vi.fn();
    const outcome = await runOcPairingCeremony({
      url: 'https://gw:18789',
      code: '222222',
      fingerprint: 'FP',
      certPem: 'PEM',
      confirmCollision: vi.fn(),
      onPaired,
    });

    expect(outcome.ok).toBe(false);
    if (!outcome.ok) {
      expect(outcome.phase).toBe('pairing');
      expect(outcome.errorMessage).toBe('Code expired. Generate a new code on your OC machine.');
    }
    expect(onPaired).not.toHaveBeenCalled();
    expect(invokeMock).not.toHaveBeenCalledWith('oc_agent_row_exists');
  });

  it('marks phase "sync" when pairing succeeds but the import step fails', async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'gateway_exchange_pairing_code') {
        return { token: 'tok-1', gateway_version: '0.70.0', version_warning: null };
      }
      if (cmd === 'oc_agent_row_exists') return false;
      if (cmd === 'sync_initial_import') throw new Error('Gateway sync timeout');
      return undefined;
    });

    const outcome = await runOcPairingCeremony({
      url: 'https://gw:18789',
      code: '333333',
      fingerprint: 'FP',
      certPem: 'PEM',
      confirmCollision: vi.fn(),
    });

    expect(outcome.ok).toBe(false);
    if (!outcome.ok) {
      expect(outcome.phase).toBe('sync');
      expect(outcome.errorMessage).toBe('Gateway sync timeout');
    }
  });
});

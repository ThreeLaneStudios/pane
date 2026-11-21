/**
 * externalLink.test.ts — Unit tests for openExternalUrl (#2311)
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';

const openMock = vi.fn();

vi.mock('@tauri-apps/plugin-shell', () => ({
  open: (...args: unknown[]) => openMock(...args),
}));

import { openExternalUrl } from './externalLink';

describe('openExternalUrl', () => {
  beforeEach(() => {
    openMock.mockReset();
  });

  it('happy path: calls Tauri shell open() with the given URL', async () => {
    openMock.mockResolvedValueOnce(undefined);
    await openExternalUrl('https://paneapp.ai/system-files-creation', 'TEST');
    expect(openMock).toHaveBeenCalledWith('https://paneapp.ai/system-files-creation');
    expect(openMock).toHaveBeenCalledTimes(1);
  });

  it('error path: logs to console and does not throw when open() rejects', async () => {
    const consoleSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
    openMock.mockRejectedValueOnce(new Error('boom'));

    await expect(openExternalUrl('https://paneapp.ai/system-files-creation', 'TEST')).resolves.toBeUndefined();

    expect(consoleSpy).toHaveBeenCalledWith(
      expect.stringContaining('[TEST] Failed to open external URL: https://paneapp.ai/system-files-creation'),
      expect.any(Error)
    );
    consoleSpy.mockRestore();
  });

  it('edge case: propagates the context tag into the log prefix for diagnosability', async () => {
    const consoleSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
    openMock.mockRejectedValueOnce(new Error('nope'));

    await openExternalUrl('https://example.com', 'ONBOARD');

    expect(consoleSpy).toHaveBeenCalledWith(
      expect.stringContaining('[ONBOARD]'),
      expect.any(Error)
    );
    consoleSpy.mockRestore();
  });
});

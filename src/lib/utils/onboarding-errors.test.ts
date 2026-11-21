import { describe, it, expect } from 'vitest';
import { mapPairingError } from './onboarding-errors';

describe('mapPairingError', () => {
  it('maps "Pairing code expired" to the code-expired message', () => {
    expect(mapPairingError('Pairing code expired')).toBe(
      'Code expired. Generate a new code on your OC machine.'
    );
  });

  it('maps "No active pairing code" to the same code-expired message', () => {
    expect(mapPairingError('No active pairing code')).toBe(
      'Code expired. Generate a new code on your OC machine.'
    );
  });

  it('maps "Invalid pairing code: nonsense" to the code-mismatch message', () => {
    expect(mapPairingError('Invalid pairing code: nonsense')).toBe(
      "Code didn't match. Try again or generate a new one."
    );
  });

  it('passes through an actionable Rust error string verbatim', () => {
    expect(mapPairingError('Pairing failed: connection refused')).toBe(
      'Pairing failed: connection refused'
    );
  });

  it('returns the fallback copy when the error string is empty', () => {
    expect(mapPairingError('')).toBe(
      'Pairing failed — check that the gateway is reachable and the pairing code is correct.'
    );
  });
});

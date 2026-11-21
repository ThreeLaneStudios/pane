/**
 * mapPairingError — pure error-string classifier for the OC pairing catch block.
 *
 * Extracts what was previously inline logic in Onboarding.svelte lines 423–431
 * so it can be unit-tested independently of Tauri/Svelte infrastructure.
 *
 * Related: #1341 — pairing catch was swallowing real Rust error in the else branch.
 */
export function mapPairingError(msg: string): string {
  if (msg.includes('Pairing code expired') || msg.includes('No active pairing code')) {
    return 'Code expired. Generate a new code on your OC machine.';
  }
  if (msg.includes('Invalid pairing code')) {
    return "Code didn't match. Try again or generate a new one.";
  }
  return msg || 'Pairing failed — check that the gateway is reachable and the pairing code is correct.';
}

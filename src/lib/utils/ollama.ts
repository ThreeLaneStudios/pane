/**
 * Shared Ollama utility functions.
 *
 * Extracted from Onboarding.svelte so both Onboarding and Settings can import
 * from the same canonical source. (ride-along refactor, PR fix/1627-1628-settings-ollama-probe)
 */

/**
 * Normalise a raw Ollama endpoint string entered by the user:
 *   - Prepends `http://` if no scheme is present
 *   - Appends `:11434` if no port is specified
 *   - Strips trailing slashes
 *
 * Returns the (possibly unchanged) string unchanged if it is empty.
 */
export function normalizeOllamaEndpoint(raw: string): string {
  let url = raw.trim();
  if (!url) return url;
  // Auto-prepend http:// if no scheme
  if (!url.match(/^https?:\/\//i)) {
    url = 'http://' + url;
  }
  // Default port :11434 if no port specified
  try {
    const parsed = new URL(url);
    if (!parsed.port) {
      parsed.port = '11434';
      url = parsed.toString();
      // Remove trailing slash added by URL constructor
      url = url.replace(/\/$/, '');
    }
  } catch {
    // Not a valid URL yet — just return as-is
  }
  // Strip trailing slash
  url = url.replace(/\/$/, '');
  return url;
}

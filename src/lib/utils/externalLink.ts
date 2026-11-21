/**
 * externalLink.ts — Shared helper for opening external URLs in the system
 * browser instead of Tauri's in-app WebView.
 *
 * #2311: "What should I upload?" help links on the onboarding and
 * agent-create upload screens both route through this helper so they share
 * identical error handling and are independently testable.
 *
 * Pattern matches existing call sites (ChatPanel.svelte,
 * AttachmentVideoCard.svelte): use
 * `@tauri-apps/plugin-shell`'s `open()` and log failures to the console
 * rather than swallowing them silently.
 */
import { open } from '@tauri-apps/plugin-shell';

/**
 * Opens `url` in the user's default system browser via Tauri's shell plugin.
 * Never throws — logs failures to the console with the given `context` tag
 * so they're diagnosable without crashing the calling UI flow.
 */
export async function openExternalUrl(url: string, context: string): Promise<void> {
  try {
    await open(url);
  } catch (e) {
    console.error(`[${context}] Failed to open external URL: ${url}`, e);
  }
}

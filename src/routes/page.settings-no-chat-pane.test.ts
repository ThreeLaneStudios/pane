/**
 * #2536 / ADR-0063 Stage 5a — Settings page no longer hosts a chat/session pane.
 *
 * Per Matt's ruling (issue #2536): the Settings page must not host a chat session at
 * all. This also removes the trigger surface for the 8/14 stale-projectId leak, where
 * navigating to Settings from within a project context carried a stale `projectId` into
 * the Settings-hosted chat session, causing the AI to resolve models against the wrong
 * project.
 *
 * Source-level assertions (same house pattern as Settings.provider-switch-model-refresh.test.ts
 * and CanvasRenderer.reload-guard.test.ts) — `+page.svelte` requires a full Tauri window
 * context and heavy child components (ChatSidebar, SessionsDrawer, CanvasRenderer), so this
 * asserts against the source directly rather than mounting the component.
 */

import { describe, it, expect } from 'vitest';
import { readFileSync } from 'fs';
import { resolve } from 'path';

const pageSource = readFileSync(resolve(__dirname, '+page.svelte'), 'utf-8');
const settingsSource = readFileSync(
  resolve(__dirname, '../lib/components/Settings.svelte'),
  'utf-8'
);

describe('#2536 — Settings page hosts no chat pane', () => {
  it('the chat-panel block is gated behind a check that excludes the settings view', () => {
    // The ChatSidebar / resize-handle / chat-panel-collapsed block must be wrapped in a
    // conditional that skips rendering when the canvas view is 'settings'.
    expect(pageSource).toMatch(/\{#if \$canvasState\.view !== 'settings'\}/);
  });

  it('the settings-exclusion gate wraps the ChatSidebar mount, not just part of the chat UI', () => {
    const gateIdx = pageSource.indexOf("{#if $canvasState.view !== 'settings'}");
    expect(gateIdx).toBeGreaterThan(-1);

    const chatSidebarIdx = pageSource.indexOf('<ChatSidebar', gateIdx);
    const resizeHandleIdx = pageSource.indexOf('chat-resize-handle', gateIdx);
    const collapsedBtnIdx = pageSource.indexOf('chat-panel-collapsed', gateIdx);

    // All three chat-hosting pieces must appear AFTER the gate opens (i.e. are inside it).
    expect(chatSidebarIdx).toBeGreaterThan(gateIdx);
    expect(resizeHandleIdx).toBeGreaterThan(gateIdx);
    expect(collapsedBtnIdx).toBeGreaterThan(gateIdx);

    // And the gate's matching {/if} must close after all three (find the block end by
    // locating the final closing brace of the top-level app div: {/if}\n</div> in source).
    const closeIdx = pageSource.indexOf('{/if}\n</div>', gateIdx);
    expect(closeIdx).toBeGreaterThan(collapsedBtnIdx);
  });

  it('Settings.svelte itself does not import or reference ChatSidebar or SessionsDrawer', () => {
    expect(settingsSource).not.toMatch(/ChatSidebar/);
    expect(settingsSource).not.toMatch(/SessionsDrawer/);
  });

  it('Settings.svelte contains no chat-message send/stream affordances (no chat pane hosted inline)', () => {
    // Guards against a chat pane being re-introduced directly inside Settings.svelte
    // rather than via +page.svelte's ChatSidebar mount.
    expect(settingsSource).not.toMatch(/streamManager/);
    expect(settingsSource).not.toMatch(/startStream/);
  });
});

describe('#2536 / ADR-0063 D8 — Settings single-column layout, content measure 860', () => {
  it('.tab-content max-width is 860px (was 600px pre-dissolution)', () => {
    const match = settingsSource.match(/\.tab-content\s*\{[^}]*max-width:\s*(\d+)px/);
    expect(match).not.toBeNull();
    expect(match?.[1]).toBe('860');
  });

  it('.tab-content stays horizontally centred (margin: 0 auto)', () => {
    const match = settingsSource.match(/\.tab-content\s*\{([^}]*)\}/);
    expect(match).not.toBeNull();
    expect(match?.[1]).toMatch(/margin:\s*0\s*auto/);
  });
});

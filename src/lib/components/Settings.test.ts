/**
 * Fix #1426 — Settings.svelte dead SameMachine UI residue
 *
 * PR #1109 (#1108 Phase 1, merged 2026-05-15) removed most of the SameMachine
 * topology UI but left three survivors in Settings.svelte:
 *
 *   1. Topology badge <div> with isRemoteUrl(openclawUrl) branch that fires
 *      "Same machine — reading local config" label
 *   2. URL-field guard {#if isRemoteUrl(openclawUrl) || manualEditMode} that
 *      hides the gateway URL input unless explicitly triggered
 *   3. openclawUrl default of 'http://127.0.0.1:18789' which triggers the dead
 *      SameMachine branch the moment provider is switched back to OpenClaw
 *      without an existing saved endpoint
 *
 * Per docs/spec/sync.md:1898 OpenClaw uses SplitMachine exclusively. The URL
 * field must always render; openclawUrl defaults to empty string so the input
 * placeholder shows the example URL without misleading pre-filled data.
 *
 * DOM render was not used because Settings.svelte requires the full Tauri window
 * context and is ~4400 lines. Source-level assertions are used instead (same
 * pattern as Settings.ctx-window-override.test.ts which explains this constraint).
 * Integration-level proof comes from cold-launch test post-DMG.
 *
 * Harness: Vitest (jsdom) — matches existing pattern in this directory.
 */

import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

describe('About version', () => {
  const source = readFileSync(resolve(__dirname, 'Settings.svelte'), 'utf-8');

  it('reads the packaged version from Tauri instead of hardcoding a number', () => {
    expect(source).toMatch(/import \{ getVersion \} from '@tauri-apps\/api\/app'/);
    expect(source).toMatch(/appVersion = await getVersion\(\)/);
    expect(source).not.toMatch(/let appVersion = \$state\(['"]\d/);
  });
});

describe('#1426 — SameMachine UI residue removed (source-level)', () => {
  const source = readFileSync(resolve(__dirname, 'Settings.svelte'), 'utf-8');

  it('does not contain "Same machine — reading local config" label', () => {
    expect(source).not.toMatch(/Same machine — reading local config/);
  });

  it('does not contain "Split machine — connected via gateway" label (topology badge fully deleted)', () => {
    expect(source).not.toMatch(/Split machine — connected via gateway/);
  });

  it('does not contain manualEditMode guard on URL field', () => {
    expect(source).not.toMatch(/isRemoteUrl\(openclawUrl\) \|\| manualEditMode/);
  });

  it('does not declare manualEditMode state (dead variable)', () => {
    expect(source).not.toMatch(/let manualEditMode = \$state\(false\)/);
  });

  it('does not reference manualEditMode at all', () => {
    expect(source).not.toMatch(/manualEditMode/);
  });

  it('openclawUrl defaults to empty string, not 127.0.0.1', () => {
    expect(source).toMatch(/let openclawUrl = \$state\(''\)/);
    expect(source).not.toMatch(/let openclawUrl = \$state\('http:\/\/127\.0\.0\.1:18789'\)/);
  });

  it('topology badge comment §22C.11A is removed', () => {
    expect(source).not.toMatch(/§22C\.11A: Topology badge/);
  });
});

describe('#2522 design polish batch, item 3 — Default agent row names the current default (source-level)', () => {
  const source = readFileSync(resolve(__dirname, 'Settings.svelte'), 'utf-8');

  // Same DOM-render constraint as the SameMachine suite above: Settings.svelte
  // requires the full Tauri window context, so this asserts against source
  // rather than a rendered component (see file header for the established
  // rationale/pattern).

  it('derives currentDefaultAgent from agentsList.is_default', () => {
    expect(source).toMatch(/const currentDefaultAgent = \$derived\(agentsList\.find\(a => a\.is_default\) \?\? null\)/);
  });

  it('the AI tab loads agents so the default-agent name is available before Agents tab is ever opened', () => {
    expect(source).toMatch(/activeTab === 'ai' && !agentsLoaded && !loadingAgents/);
  });

  it('renders the default agent name and runtime label instead of only the sublabel copy when a default is loaded', () => {
    expect(source).toMatch(/\{#if currentDefaultAgent\}/);
    expect(source).toMatch(/class="default-agent-name"\>\{currentDefaultAgent\.name\}/);
    expect(source).toMatch(/class="default-agent-runtime"\>\{RUNTIME_TYPE_LABELS\[currentDefaultAgent\.runtime_type/);
  });

  it('falls back to the original "Preselected for new sessions…" copy when there is no default agent yet', () => {
    expect(source).toMatch(/\{:else\}\s*<div class="form-row-sublabel">Preselected for new sessions/);
  });

  it('the "Change default" button is still present alongside the new name/runtime label', () => {
    expect(source).toMatch(/onclick=\{openDefaultAgentPicker\} disabled=\{defaultPickerLoading\}/);
  });
});

describe('#2641 D14 Commit 5 — Smart Features status dot (source-level)', () => {
  const source = readFileSync(resolve(__dirname, 'Settings.svelte'), 'utf-8');

  // Same DOM-render constraint as the suites above: Settings.svelte requires the
  // full Tauri window context, so this asserts against source rather than a
  // rendered component.

  it('derives internalStatusDotClass as grey/green/red from internalConfiguredAt + internalConnectionStatus', () => {
    expect(source).toMatch(/const internalStatusDotClass = \$derived\(/);
    expect(source).toMatch(/!internalConfiguredAt\s*\n\s*\? 'grey'/);
    expect(source).toMatch(/internalConnectionStatus === 'error'\s*\n\s*\? 'red'/);
    expect(source).toMatch(/: 'green'/);
  });

  it('renders the status dot next to the SMART FEATURES section title using internalStatusDotClass', () => {
    expect(source).toMatch(/<span class="status-dot \{internalStatusDotClass\}" data-testid="smart-features-status-dot"/);
  });

  it('defines a grey variant of .status-dot for the never-configured-this-install state', () => {
    expect(source).toMatch(/\.status-dot\.grey \{/);
  });

  it('tracks internalConfiguredAt as component state, read back from internal_configured_at on load', () => {
    expect(source).toMatch(/let internalConfiguredAt = \$state<string \| null>\(null\)/);
    expect(source).toMatch(/key: 'internal_configured_at' \}\) as string \| null/);
    expect(source).toMatch(/internalConfiguredAt = intConfiguredAtResult \?\? null/);
  });

  it('does NOT auto-set internalConnectionStatus to success on load just because a provider is already selected (closes the #2631 false-green shape)', () => {
    // C50 (#3171 PR 1): the old bug was gating on a locally-read key; now there
    // is no key field at all, so this asserts the load path still unconditionally
    // resets to 'not-tested' before any gated reload of the model pickers.
    expect(source).toMatch(/internalConnectionStatus = 'not-tested';\s*\n\s*internalConnectionMessage = 'Not tested';\s*\n/);
  });

  it('handleInternalProviderChange clears internalConfiguredAt locally and in ai_settings', () => {
    expect(source).toMatch(/async function handleInternalProviderChange\(\) \{[\s\S]*?internalConfiguredAt = null;[\s\S]*?invoke\('set_ai_setting', \{ key: 'internal_configured_at', value: '' \}\);/);
  });

  it('saveInternalProvider writes internal_configured_at on successful save (Ollama path)', () => {
    expect(source).toMatch(/async function saveInternalProvider\(\)[\s\S]*?internalConfiguredAt = nowIso;/);
  });

  it('C50 (#3171 PR 1): handleInternalProviderChange marks openrouter/anthropic/venice configured immediately — no second key entry gates it', () => {
    expect(source).toMatch(/internalProvider === 'openrouter' \|\| internalProvider === 'anthropic' \|\| internalProvider === 'venice'/);
  });
});

describe('#3074 PR 2 — model picker wired into Settings → Agents (source-level)', () => {
  const source = readFileSync(resolve(__dirname, 'Settings.svelte'), 'utf-8');

  // Same DOM-render constraint as every suite above: Settings.svelte requires the
  // full Tauri window context and is not rendered in-suite (see file header).
  // This asserts the gate is wired to the imported predicate, the save payload
  // is built correctly, and runtime_type cannot be smuggled through this path —
  // all source-level, not re-deriving #3074 part 1's own tested boolean.

  it('imports isAgentModelEditable from agentDisplay.ts instead of re-deriving the gate', () => {
    expect(source).toMatch(/import \{ isAgentModelEditable \} from '\$lib\/utils\/agentDisplay'/);
  });

  it('gates the model-edit pencil on the imported predicate, not an inline runtime_type check', () => {
    expect(source).toMatch(/\{#if isAgentModelEditable\(agent\) && !\(agent\.role === 'sub' && agent\.runtime_ref != null\)\}/);
  });

  it('does not re-derive the editable condition with its own runtime_type allow-list', () => {
    // The only runtime_type-literal comparisons touching the model-edit affordance
    // should live in agentDisplay.ts (part 1), not be re-typed here.
    expect(source).not.toMatch(/agent\.runtime_type === 'anthropic'[\s\S]{0,80}agent\.runtime_type === 'openrouter'/);
  });

  it('startEditAgentModel loads models via fetch_available_models filtered by the agent\'s own runtime_type', () => {
    expect(source).toMatch(/async function startEditAgentModel\(idx: number, agent: Agent\)[\s\S]*?invoke<[\s\S]*?>\('fetch_available_models'\)/);
    expect(source).toMatch(/agentModelOptions = models\.filter\(\(m\) => m\.provider === agent\.runtime_type\)/);
  });

  it('saveAgentModel sends exactly { agent_id, model } — never a spread of agent, never runtime_type', () => {
    expect(source).toMatch(/async function saveAgentModel\(agent: Agent, modelId: string\)[\s\S]*?invoke\('update_agent', \{\s*request: \{\s*agent_id: agent\.id,\s*model: modelId\s*\}\s*\}\)/);
  });

  it('renders SearchableModelSelect for the staged model edit with the filtered options and save-on-select', () => {
    // #3083: `provider={agent.runtime_type ?? undefined}` now gates the
    // custom-model-entry box (SearchableModelSelect only shows it for the
    // OpenRouter namespace) — passing the same field startEditAgentModel
    // already filters `agentModelOptions` by, not a newly invented value.
    expect(source).toMatch(/<SearchableModelSelect\s*\n\s*models=\{agentModelOptions\}\s*\n\s*provider=\{agent\.runtime_type \?\? undefined\}\s*\n\s*selectedModel=\{agent\.model \?\? ''\}\s*\n\s*placeholder="Search models\.\.\."\s*\n\s*onSelect=\{\(modelId\) => saveAgentModel\(agent, modelId\)\}/);
  });

  it('cancelEditAgentModel exists and clears the edit state without saving', () => {
    expect(source).toMatch(/function cancelEditAgentModel\(\) \{\s*editingAgentModelIdx = null;/);
  });
});

describe('ADR-0070 Decision 3 (#3195 PR B) — T3: no Account block when account_enabled is off (source-level)', () => {
  // Same DOM-render constraint as the suites above (Settings.svelte needs a
  // full Tauri window context) — source-level assertions, same established
  // pattern.
  //
  // Red today (pre-PR-B): Settings.svelte reads $isAuthenticated / $userEmail
  // and renders the ACCOUNT section's sign-in-or-account-info content
  // unconditionally whenever activeTab === 'account' — there is no
  // `accountEnabled` state, no `isAccountEnabled` import, and no `{#if
  // accountEnabled}` gate around the ACCOUNT section. Every assertion below
  // fails against that source.
  const source = readFileSync(resolve(__dirname, 'Settings.svelte'), 'utf-8');

  it('imports isAccountEnabled from authStore.ts instead of re-deriving the flag read', () => {
    expect(source).toMatch(/import \{ isAuthenticated, userEmail, signIn, signUp, signOut, isAccountEnabled \} from '\$lib\/stores\/authStore'/);
  });

  it('declares accountEnabled state, defaulting to off', () => {
    expect(source).toMatch(/let accountEnabled = \$state\(false\)/);
  });

  it('populates accountEnabled from isAccountEnabled() during mount-time load', () => {
    expect(source).toMatch(/accountEnabled = await isAccountEnabled\(\)/);
  });

  it('gates the entire ACCOUNT section on accountEnabled — not just the inner sign-in-vs-account-info branch', () => {
    // #3195 PR D update: the panel-opening condition literal text changed
    // from "{#if activeTab === 'account'}" to "{#if activeTab === 'account'
    // && accountEnabled}" (the whole tab is now also gated, not just this
    // inner ACCOUNT sub-block) — the regex is updated to match the new
    // literal source text; the assertion's intent (ACCOUNT section-title is
    // still reachable only inside an accountEnabled gate) is unchanged.
    expect(source).toMatch(/\{#if activeTab === 'account' && accountEnabled\}[\s\S]{0,400}\{#if accountEnabled\}[\s\S]{0,120}<div class="section">\s*<div class="section-title">ACCOUNT<\/div>/);
  });

  it('does not render the ACCOUNT section-title unconditionally (only reachable inside the accountEnabled gate)', () => {
    // The SYNC section-title is still reachable from this test's positional
    // check below, but as of #3195 PR D the whole Account & Sync tab (button
    // and panel) is now also gated on accountEnabled — see the T5 describe
    // block below. This test's own assertions are unaffected: they only
    // check relative ordering of indices, not whether the tab itself is
    // reachable without the flag.
    // #3195 PR D added a second literal "{#if accountEnabled}" occurrence
    // earlier in the file (the segmented-control tab button gate) — anchor
    // the search from the panel region ("{#if activeTab === 'account' &&
    // accountEnabled}") so this still resolves to the inner ACCOUNT
    // sub-block gate it was written against, not the tab button gate.
    const panelGateIdx = source.indexOf("{#if activeTab === 'account' && accountEnabled}");
    const accountGateIdx = source.indexOf("{#if accountEnabled}", panelGateIdx);
    const accountTitleIdx = source.indexOf('<div class="section-title">ACCOUNT</div>');
    const syncTitleIdx = source.indexOf('<div class="section-title">SYNC</div>');
    expect(panelGateIdx).toBeGreaterThan(-1);
    expect(accountGateIdx).toBeGreaterThan(-1);
    expect(accountTitleIdx).toBeGreaterThan(accountGateIdx);
    expect(syncTitleIdx).toBeGreaterThan(accountTitleIdx);
  });
});

describe('ADR-0070 Decision 4 (#3195 PR C) — T4b: Account block renders the sign-in form when accountEnabled is on (source-level)', () => {
  const source = readFileSync(resolve(__dirname, 'Settings.svelte'), 'utf-8');
  // #3195 PR D added a second literal "{#if accountEnabled}" occurrence
  // earlier in the file (the segmented-control tab button gate). Anchor
  // from the panel region ("{#if activeTab === 'account' && accountEnabled}")
  // so the gate lookups below resolve to the inner ACCOUNT sub-block gate
  // they were written against, not the tab button gate.
  const panelGateIdx = source.indexOf("{#if activeTab === 'account' && accountEnabled}");

  it('renders email and password inputs inside the accountEnabled gate, for the signed-out branch', () => {
    expect(panelGateIdx).toBeGreaterThan(-1);
    const gateIdx = source.indexOf('{#if accountEnabled}', panelGateIdx);
    expect(gateIdx).toBeGreaterThan(-1);
    const emailInputIdx = source.indexOf('type="email" class="input input-wide" placeholder="Email"', gateIdx);
    const passwordInputIdx = source.indexOf('type="password" class="input input-wide" placeholder="Password"', gateIdx);
    expect(emailInputIdx).toBeGreaterThan(gateIdx);
    expect(passwordInputIdx).toBeGreaterThan(emailInputIdx);
  });

  it('wires the Sign In button to handleSignIn, which calls the authStore signIn() function', () => {
    expect(source).toMatch(/onclick=\{handleSignIn\} disabled=\{loading\}/);
    expect(source).toMatch(/async function handleSignIn\(\) \{[\s\S]{0,400}await signIn\(email, password\);/);
  });

  it('the signed-out branch (email+password form) only renders when $isAuthenticated is false, nested under the same accountEnabled gate as the signed-in branch', () => {
    // Both branches of the {#if $isAuthenticated}...{:else}...{/if} ternary
    // live inside the single {#if accountEnabled} gate from T3 above —
    // accountEnabled decides whether the Account block exists at all;
    // isAuthenticated only decides which of its two branches shows.
    const gateIdx = source.indexOf('{#if accountEnabled}', panelGateIdx);
    const authTernaryIdx = source.indexOf('{#if $isAuthenticated}', gateIdx);
    const formIdx = source.indexOf('Sign in to sync across devices', gateIdx);
    expect(authTernaryIdx).toBeGreaterThan(gateIdx);
    expect(formIdx).toBeGreaterThan(authTernaryIdx);
  });
});

describe("ADR-0070 Decision 2 (#3195 PR D) — T5: the Account & Sync tab is not rendered at all when account_enabled is off", () => {
  const source = readFileSync(resolve(__dirname, 'Settings.svelte'), 'utf-8');

  it('gates the Account & Sync tab button on accountEnabled — the segmented control does not render it when the flag is off', () => {
    expect(source).toMatch(/\{#if accountEnabled\}\s*<button class="seg-btn" class:active=\{activeTab === 'account'\}/);
  });

  it('gates the Account & Sync panel on accountEnabled, not just the inner ACCOUNT sub-block — SYNC is reachable today without the flag and must not be', () => {
    expect(source).toMatch(/\{#if activeTab === 'account' && accountEnabled\}/);
  });
});

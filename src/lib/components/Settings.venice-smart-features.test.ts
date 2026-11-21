/**
 * #2795 — Venice as Smart Features internal provider (Settings.svelte)
 * Superseded in part by C50 (#3171 PR 1): Smart Features no longer has a
 * key-entry field for ANY provider — openrouter, anthropic, and venice all
 * read their key from the matching Provider Connection instead. This file
 * now asserts the C50 shape directly; the original #2795 key-entry-field
 * assertions (B, parts of C) are gone because that field is gone.
 *
 * Same DOM-render constraint as Settings.test.ts / Settings.ctx-window-override.test.ts:
 * Settings.svelte requires the full Tauri window context and is thousands of
 * lines — source-level assertions are used instead of a rendered component.
 * Integration-level proof comes from the Rust round-trip tests
 * (dual_provider_tests.rs::venice_integration_tests,
 * dual_provider_tests.rs::tests::test_get_internal_provider_venice*) and
 * cold-launch smoke.
 *
 * Coverage:
 *  A. Provider dropdown renders Venice as an <option> when connected
 *  B. Selecting any of openrouter/anthropic/venice renders NO key-entry field
 *     and NO status row — #3175 removed the redundant "Connected via Provider
 *     Connections" row; the Provider row's own sublabel already says the key
 *     is reused
 *  C. handleInternalProviderChange persists internal_provider immediately and
 *     marks openrouter/anthropic/venice "configured" without ever writing
 *     internal_api_key (the retired shared OR/Anthropic slot) for ANY provider,
 *     Venice included
 *  D. providerLabel-equivalent text includes a Venice arm
 *  E. internalProviderOptions filters to CONNECTED providers only — Ollama is
 *     always offered regardless of connection state
 */
import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const SOURCE = readFileSync(resolve(__dirname, 'Settings.svelte'), 'utf-8');

describe('#2795 / C50 (#3171 PR 1) A — Smart Features provider dropdown is connection-filtered', () => {
  it('the internalProvider <select> iterates internalProviderOptions (connected openrouter/anthropic/venice) plus a static Ollama option', () => {
    const selectMatch = SOURCE.match(
      /<select class="select-input" bind:value=\{internalProvider\}[\s\S]*?<\/select>/
    );
    expect(selectMatch).toBeTruthy();
    expect(selectMatch![0]).toContain('{#each internalProviderOptions as opt}');
    expect(selectMatch![0]).toContain('<option value="ollama">Ollama</option>');
  });

  it('internalProviderOptions derives from settingsConnections filtered to openrouter/anthropic/venice AND connected', () => {
    expect(SOURCE).toMatch(
      /let internalProviderOptions = \$derived\(\s*settingsConnections\.filter\(c => INTERNAL_FEATURES_KEY_PROVIDER_TYPES\.includes\(c\.runtime_type\) && c\.connected\)\s*\);/
    );
  });

  it('the option label ternary resolves openrouter/anthropic/venice to their human-readable names', () => {
    const selectMatch = SOURCE.match(
      /<select class="select-input" bind:value=\{internalProvider\}[\s\S]*?<\/select>/
    );
    expect(selectMatch![0]).toMatch(/opt\.runtime_type === 'openrouter' \? 'OpenRouter' : opt\.runtime_type === 'anthropic' \? 'Anthropic' : 'Venice'/);
  });
});

describe('C50 (#3171 PR 1) B — selecting openrouter/anthropic/venice renders NO key-entry field', () => {
  it('no password-wrapper key input exists anywhere in the Smart Features section for openrouter/anthropic/venice', () => {
    const sectionMatch = SOURCE.match(
      /SMART FEATURES[\s\S]*?Save & Test Connection/
    );
    expect(sectionMatch, 'Smart Features section must exist').toBeTruthy();
    expect(sectionMatch![0]).not.toMatch(/bind:value=\{internalApiKey\}/);
    expect(sectionMatch![0]).not.toContain('OpenRouter API Key');
    expect(sectionMatch![0]).not.toContain('Anthropic API Key');
    expect(sectionMatch![0]).not.toContain('Venice API Key');
  });

  it('the else branch (openrouter/anthropic/venice) renders no status row — the Provider row\'s own sublabel already says the key is reused (#3175)', () => {
    expect(SOURCE).not.toMatch(/Connected via Provider Connections/);
  });

  it('RED-ON-REVERT: internalApiKey must not exist anywhere in the file — the state it fed is retired', () => {
    expect(SOURCE).not.toMatch(/\binternalApiKey\b/);
  });
});

describe('C50 (#3171 PR 1) C — handleInternalProviderChange never writes the retired internal_api_key slot', () => {
  it("handleInternalProviderChange persists internal_provider immediately on change", () => {
    const fnMatch = SOURCE.match(/async function handleInternalProviderChange\(\)[\s\S]*?\n  \}/);
    expect(fnMatch).toBeTruthy();
    expect(fnMatch![0]).toMatch(/invoke\('set_ai_setting', \{ key: 'internal_provider', value: internalProvider \}\)/);
  });

  it('handleInternalProviderChange marks openrouter/anthropic/venice configured without any key round-trip', () => {
    const fnMatch = SOURCE.match(/async function handleInternalProviderChange\(\)[\s\S]*?\n  \}/);
    const fn = fnMatch![0];
    expect(fn).toMatch(/internalProvider === 'openrouter' \|\| internalProvider === 'anthropic' \|\| internalProvider === 'venice'/);
    expect(fn).toMatch(/invoke\('set_ai_setting', \{ key: 'internal_configured_at', value: nowIso \}\)/);
  });

  it("RED-ON-REVERT: neither handleInternalProviderChange nor saveInternalProvider may write 'internal_api_key' for ANY provider (the retired shared OR/Anthropic slot)", () => {
    const changeFn = SOURCE.match(/async function handleInternalProviderChange\(\)[\s\S]*?\n  \}/)![0];
    const saveFn = SOURCE.match(/async function saveInternalProvider\(\)[\s\S]*?\n  \}/)![0];
    expect(changeFn).not.toContain("key: 'internal_api_key'");
    expect(saveFn).not.toContain("key: 'internal_api_key'");
  });

  it("saveInternalProvider is now Ollama-only — it returns early for any other provider", () => {
    const fnMatch = SOURCE.match(/async function saveInternalProvider\(\)[\s\S]*?\n  \}/);
    expect(fnMatch![0]).toMatch(/if \(internalProvider !== 'ollama'\) \{\s*return;/);
  });
});

describe('C50 (#3171 PR 1) D — provider-label text includes a Venice arm', () => {
  it('handleInternalProviderChange\u2019s providerLabel ternary resolves venice to "Venice"', () => {
    const fnMatch = SOURCE.match(/async function handleInternalProviderChange\(\)[\s\S]*?\n  \}/);
    expect(fnMatch![0]).toMatch(/internalProvider === 'anthropic' \? 'Anthropic' : 'Venice'/);
  });
});

describe('C50 (#3171 PR 1) E — Ollama is always offered, independent of Provider Connections', () => {
  it('the dropdown always appends a static Ollama <option>, never gated by settingsConnections', () => {
    const selectMatch = SOURCE.match(
      /<select class="select-input" bind:value=\{internalProvider\}[\s\S]*?<\/select>/
    );
    // The static <option value="ollama"> must sit OUTSIDE the {#each} block.
    const eachEnd = selectMatch![0].indexOf('{/each}');
    const ollamaIdx = selectMatch![0].indexOf('<option value="ollama">Ollama</option>');
    expect(eachEnd).toBeGreaterThan(-1);
    expect(ollamaIdx).toBeGreaterThan(eachEnd);
  });
});

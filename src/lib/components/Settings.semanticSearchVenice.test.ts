/**
 * Settings.semanticSearchVenice.test.ts — #2988 PR1 (Venice as a Semantic
 * Search engine). Updated for C50 (#3171 PR 1): OpenRouter's Semantic Search
 * arm now reuses the SAME no-key shape Venice already had — both read their
 * key from the matching Provider Connection; neither renders a key-entry
 * field, and `fetch_embedding_models`/`switch_embedding_engine` are no
 * longer called with an `apiKey` argument at all.
 *
 * Harness: source-level assertions, matching the established pattern for
 * this file (see `Settings.categoryRefresh.test.ts`'s doc comment —
 * `Settings.svelte` requires the full Tauri window context and is 5000+
 * lines; no test in this directory renders it via `render()`).
 */

import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const source = readFileSync(resolve(__dirname, 'Settings.svelte'), 'utf-8');

describe('Settings.svelte — Semantic Search Venice engine (#2988 PR1, C50-updated)', () => {
  it('the provider dropdown iterates ssProviderOptions (connected openrouter/venice) plus a static Ollama option', () => {
    const match = source.match(
      /<select class="select-input" value=\{ssProvider\}[\s\S]{0,600}?<\/select>/
    );
    expect(match, 'Semantic Search provider <select> must exist').toBeTruthy();
    expect(match![0]).toContain('{#each ssProviderOptions as opt}');
    expect(match![0]).toContain('<option value="ollama">Ollama</option>');
  });

  it('ssProviderOptions derives from settingsConnections filtered to openrouter/venice AND connected', () => {
    expect(source).toMatch(
      /let ssProviderOptions = \$derived\(\s*settingsConnections\.filter\(c => SEMANTIC_SEARCH_KEY_PROVIDER_TYPES\.includes\(c\.runtime_type\) && c\.connected\)\s*\);/
    );
  });

  it('loadEmbeddingModels never sends an apiKey argument \u2014 only endpoint, Ollama-only', () => {
    const match = source.match(
      /async function loadEmbeddingModels\(\)[\s\S]{0,1200}?\n  \}/
    );
    expect(match, 'loadEmbeddingModels must exist').toBeTruthy();
    expect(match![0]).not.toMatch(/apiKey:/);
    expect(match![0]).toMatch(/endpoint: ssProvider === 'ollama' \? \(ssEndpoint \|\| null\) : null/);
  });

  it('handleSsSaveTest never sends an apiKey argument \u2014 only endpoint, Ollama-only', () => {
    const match = source.match(
      /async function handleSsSaveTest\(\)[\s\S]{0,1800}?\n  \}/
    );
    expect(match, 'handleSsSaveTest must exist').toBeTruthy();
    expect(match![0]).not.toMatch(/apiKey:/);
    expect(match![0]).toMatch(/endpoint: ssProvider === 'ollama' \? \(ssEndpoint \|\| null\) : null/);
  });

  it('ssProviderLabel maps venice to the human-readable "Venice" label', () => {
    const match = source.match(/function ssProviderLabel\(provider: string\): string \{([\s\S]{0,200}?)\n  \}/);
    expect(match, 'ssProviderLabel helper must exist').toBeTruthy();
    expect(match![1]).toMatch(/provider === 'venice'.*return 'Venice'/);
  });

  it('the switch-confirm modal uses ssProviderLabel instead of a hardcoded OpenRouter/Ollama ternary', () => {
    expect(source).toMatch(/Switch to \{ssProviderLabel\(ssPendingProvider\)\}\?/);
    expect(source).not.toMatch(/ssPendingProvider === 'openrouter' \? 'OpenRouter' : 'Ollama'/);
  });

  it('neither the openrouter nor the venice branch renders an API key input field', () => {
    const match = source.match(/\{#if ssProvider === 'openrouter' \|\| ssProvider === 'venice'\}[\s\S]{0,2200}?\{\/if\}/);
    expect(match, "the merged openrouter/venice {#if} block must exist").toBeTruthy();
    expect(match![0]).not.toMatch(/<input[^>]*bind:value=\{ssApiKey\}/);
    expect(match![0]).not.toContain('OpenRouter API Key');
    expect(match![0]).not.toContain('Venice API Key');
  });

  it('RED-ON-REVERT: ssApiKey/ssShowApiKey/ssVeniceKeyMasked (the retired key-field state) must not exist anywhere in the file', () => {
    expect(source).not.toMatch(/\bssApiKey\b/);
    expect(source).not.toMatch(/\bssShowApiKey\b/);
    expect(source).not.toMatch(/\bssVeniceKeyMasked\b/);
  });
});

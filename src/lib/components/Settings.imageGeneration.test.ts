/**
 * Settings.imageGeneration.test.ts — #3161 PR 1 (§39.2: Settings -> AI ->
 * Image generation).
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

describe('Settings.svelte — Image generation (§39.2, #3161 PR 1)', () => {
  it('the section exists with no API key field', () => {
    // #3166 C51 widened this block's own char budget (6000 -> 7500) to
    // also cover the new model-search input + filtered-list wrapper
    // (igModelSearch/igFilteredModels) added right inside this section—
    // same block, same section, just a few more lines of markup.
    const match = source.match(/<div class="section-title">IMAGE GENERATION<\/div>[\s\S]{0,7500}?<!-- §32: LINK EMBEDS/);
    expect(match, 'the IMAGE GENERATION section must exist, directly before §32 LINK EMBEDS').toBeTruthy();
    expect(match![0]).not.toMatch(/bind:value=\{igApiKey\}/);
    expect(match![0]).not.toMatch(/type="password"/);
  });

  it('both ai_settings keys are null-default state, not pre-filled with a default provider/model', () => {
    const match = source.match(/let igProvider = \$state\(([^)]*)\);/);
    expect(match, 'igProvider state must exist').toBeTruthy();
    expect(match![1].trim()).toBe("''");
    const modelMatch = source.match(/let igModel = \$state\(([^)]*)\);/);
    expect(modelMatch, 'igModel state must exist').toBeTruthy();
    expect(modelMatch![1].trim()).toBe("''");
  });

  it('the provider dropdown is populated from igProviderOptions, filtered to connected + image-capable runtimes', () => {
    const match = source.match(/let igProviderOptions = \$derived\(([\s\S]{0,300}?)\);/);
    expect(match, 'igProviderOptions derived value must exist').toBeTruthy();
    expect(match![1]).toMatch(/IMAGE_CAPABLE_PROVIDER_TYPES\.includes\(c\.runtime_type\)/);
    expect(match![1]).toMatch(/c\.connected/);
  });

  it('IMAGE_CAPABLE_PROVIDER_TYPES includes both openrouter and venice (#3161 PR 2)', () => {
    const match = source.match(/const IMAGE_CAPABLE_PROVIDER_TYPES = (\[[^\]]*\]);/);
    expect(match, 'IMAGE_CAPABLE_PROVIDER_TYPES must exist').toBeTruthy();
    expect(JSON.parse(match![1].replace(/'/g, '"'))).toEqual(['openrouter', 'venice']);
  });

  it('loadImageModels fetches lazily — only called after a provider is picked, never on mount unconditionally', () => {
    // #3161 PR 2 widened this block's own char budget (1200 -> 1800) to
    // also cover the new max_images_per_turn load added right after the
    // existing loadImageModels() call — same block, same try/catch, just
    // a few more lines inside it.
    const mountMatch = source.match(/Load Image generation settings[\s\S]{0,1800}?Failed to load image generation settings/);
    expect(mountMatch, 'the mount-time image generation load block must exist').toBeTruthy();
    expect(mountMatch![0]).toMatch(/if \(igProvider\) \{\s*await loadImageModels\(\);\s*\}/);
  });

  it('handleIgProviderChange fetches the model list only after persisting the new provider, mirroring handleSsProviderChange/loadEmbeddingModels', () => {
    const match = source.match(/async function handleIgProviderChange\(newProvider: string\) \{([\s\S]{0,500}?)\n  \}/);
    expect(match, 'handleIgProviderChange must exist').toBeTruthy();
    expect(match![1]).toMatch(/set_ai_setting.*image_provider/s);
    expect(match![1]).toMatch(/if \(newProvider\) \{\s*await loadImageModels\(\);\s*\}/);
  });

  it('handleIgModelSelect persists image_model via set_ai_setting', () => {
    const match = source.match(/async function handleIgModelSelect\(modelId: string\) \{([\s\S]{0,300}?)\n  \}/);
    expect(match, 'handleIgModelSelect must exist').toBeTruthy();
    expect(match![1]).toMatch(/set_ai_setting.*image_model/s);
  });

  it('loadImageModels never fires for an empty provider (no speculative fetch)', () => {
    const match = source.match(/async function loadImageModels\(\) \{\s*\n\s*(if \(!igProvider\) return;)/);
    expect(match, 'loadImageModels must guard on an empty provider as its first statement').toBeTruthy();
  });

  it('the Test button calls test_generate_image and is disabled until both provider and model are chosen', () => {
    const match = source.match(/<button class="btn btn-sm" onclick=\{handleTestImageGen\}[\s\S]{0,100}?>/);
    expect(match, 'the Test button must exist').toBeTruthy();
    expect(match![0]).toMatch(/disabled=\{igTesting \|\| !igProvider \|\| !igModel\}/);

    const handlerMatch = source.match(/async function handleTestImageGen\(\) \{([\s\S]{0,900}?)\n  \}/);
    expect(handlerMatch, 'handleTestImageGen must exist').toBeTruthy();
    expect(handlerMatch![1]).toMatch(/invoke\('test_generate_image'\)/);
  });

  // #3171 PR 2: the image-model picker now reuses SearchableModelSelect
  // (Matt's ruling — same component as the other model pickers), so the
  // price is folded into `igSearchableModels`' mapped `name` field rather
  // than a `<option>` text interpolation — see
  // Settings.imageModelSearch.test.ts for that mapping's own coverage.
  it('a missing price never renders as $0.00 — igSearchableModels only folds in a price when price_usd is non-null', () => {
    const match = source.match(/name: m\.price_usd != null[^,]*,/);
    expect(match, 'the igSearchableModels price mapping must guard on price_usd != null').toBeTruthy();
  });
});

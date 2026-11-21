/**
 * Settings.imageModelSearch.test.ts — #3171 PR 2 (C51/C57).
 *
 * The image-model picker (Settings.svelte, §39.2) was its own ad-hoc
 * search-box-above-a-native-`<select>` (#3166 C51) — the EXACT anti-
 * pattern #3168 introduced elsewhere, which this picker was supposed to
 * be the model for (Reed's finding: "the image-model picker currently
 * has the same bug it's meant to fix"). Matt's ruling: "it has to be
 * the same component the model pickers use." This picker now reuses
 * SearchableModelSelect (backed by the generic SearchableSelect) — the
 * same component Smart Features/Semantic Search already use — instead
 * of a hand-rolled `igModelSearch`/`igFilteredModels` pair.
 *
 * Harness: source-level assertions — `Settings.svelte` requires the
 * full Tauri window context and cannot be DOM-rendered directly (same
 * constraint `Settings.imageGeneration.test.ts`/
 * `Settings.agents-tab-refresh-after-oc-import.test.ts` already
 * document).
 */
import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const source = readFileSync(resolve(__dirname, 'Settings.svelte'), 'utf-8');

describe('Settings.svelte — image-model picker uses the shared SearchableModelSelect (#3171 PR 2)', () => {
  it('the old ad-hoc search box above a native <select> is gone', () => {
    expect(source).not.toMatch(/data-testid="ig-model-search"/);
    expect(source).not.toMatch(/class="ig-model-search"/);
    expect(source).not.toMatch(/let igModelSearch/);
    expect(source).not.toMatch(/let igFilteredModels/);
  });

  it('the image-model picker renders SearchableModelSelect, not a native <select>', () => {
    const match = source.match(/\{:else\}\s*<!-- #3171 PR 2[\s\S]{0,400}?<SearchableModelSelect[\s\S]{0,400}?\/>\s*\{\/if\}/);
    expect(match, 'the SearchableModelSelect usage for the image-model picker must exist').toBeTruthy();
    expect(match![0]).toMatch(/models=\{igSearchableModels\}/);
    expect(match![0]).toMatch(/provider=\{igProvider\}/);
    expect(match![0]).toMatch(/selectedModel=\{igModel\}/);
    expect(match![0]).toMatch(/onSelect=\{handleIgModelSelect\}/);
  });

  it('igSearchableModels maps the bare {id, name, price_usd} shape into the Model shape SearchableModelSelect requires', () => {
    const match = source.match(/let igSearchableModels = \$derived\(([\s\S]{0,400}?)\n  \);/);
    expect(match, 'igSearchableModels derived value must exist').toBeTruthy();
    expect(match![1]).toMatch(/igModels\.map/);
    expect(match![1]).toMatch(/provider: igProviderLabel\(\)/);
  });

  it('a missing price never renders as $0.00 — the price is only folded into the name when price_usd is non-null', () => {
    const match = source.match(/name: m\.price_usd != null \? `[^`]*\$\{m\.price_usd\.toFixed\(2\)\}[^`]*` : m\.name,/);
    expect(match, 'the price interpolation inside igSearchableModels must guard on price_usd != null').toBeTruthy();
  });

  it('handleIgProviderChange no longer resets a search field that no longer exists', () => {
    const match = source.match(/async function handleIgProviderChange\(newProvider: string\) \{([\s\S]{0,400}?)\n  \}/);
    expect(match, 'handleIgProviderChange must exist').toBeTruthy();
    expect(match![1]).not.toMatch(/igModelSearch/);
  });

  it('no longer imports matchesSearchQuery for this picker (SearchableModelSelect does its own filtering)', () => {
    // #3171 PR 2 removed the only Settings.svelte use of matchesSearchQuery
    // (the image-model picker's ad-hoc filter) — confirm the import itself
    // is gone, not merely unused, so a future re-add is a visible diff.
    expect(source).not.toMatch(/import \{ matchesSearchQuery \} from '\$lib\/utils\/searchMatch';/);
  });
});

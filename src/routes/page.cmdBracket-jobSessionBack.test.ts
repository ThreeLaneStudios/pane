/**
 * page.cmdBracket-jobSessionBack.test.ts — #3151 PR1 Commit F.
 *
 * `+page.svelte` is a large root route component — this repo's own
 * convention (see `page.cmdN-shared-function.test.ts`'s own header
 * comment) for testing its keydown-handler branches without mounting
 * the whole thing is to assert against the REAL source text, proving
 * the shortcut calls the one real shared function
 * (`goToJobSessionBackTarget`, `$lib/services/jobSessionBack.ts`) —
 * which has its own full behavioral test suite
 * (`jobSessionBack.test.ts`) — rather than re-deriving/mocking the
 * entire keydown dispatch chain.
 */
import { describe, it, expect } from 'vitest';
import { readFileSync } from 'fs';
import { resolve } from 'path';

const PAGE_SOURCE = readFileSync(resolve(__dirname, '+page.svelte'), 'utf-8');

describe('#3151 PR1 Commit F — Cmd+[ calls the shared back-target action', () => {
  it("the Cmd+[ branch exists, calls goToJobSessionBackTarget(), and preventDefaults", () => {
    const branchSrc = PAGE_SOURCE.match(/if \(meta && e\.key === '\[\'\) \{[\s\S]*?\n    \}/);
    expect(branchSrc, "a `meta && e.key === '['` branch must exist in handleKeydown").toBeTruthy();
    expect(branchSrc![0]).toMatch(/goToJobSessionBackTarget\(\)/);
    expect(branchSrc![0]).toMatch(/e\.preventDefault\(\)/);
  });

  it('goToJobSessionBackTarget is imported from the one shared module, not reimplemented inline', () => {
    expect(PAGE_SOURCE).toMatch(/import\s*\{\s*goToJobSessionBackTarget\s*\}\s*from\s*'\$lib\/services\/jobSessionBack'/);
  });
});

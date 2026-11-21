/**
 * #2208 Contract Guard: pill-dropdown positioning
 *
 * Editor.svelte owns `position: fixed` on `.pill-dropdown`, `.folder-picker-dropdown`,
 * and `.pill-date-picker`. No other component stylesheet may set `position` on these
 * selectors — doing so overrides the fixed positioning and renders dropdowns off-screen.
 *
 * This test catches any future regression. It must stay red if either override from
 * #2378 (KanbanBoard.svelte, AllTasksView.svelte) is re-added.
 *
 * Red-on-revert: adding `position: absolute` back to either file will fail this test.
 */

import { describe, it, expect } from 'vitest';
import { readdirSync, readFileSync, statSync } from 'fs';
import { join, resolve } from 'path';

const REPO_ROOT = resolve(__dirname, '../../../');
const SRC_DIR = join(REPO_ROOT, 'src');
const EDITOR_SVELTE = join(SRC_DIR, 'lib/components/Editor.svelte');

/** Recursively collect all .svelte files under a directory */
function collectSvelteFiles(dir: string): string[] {
  const results: string[] = [];
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    const stat = statSync(full);
    if (stat.isDirectory()) {
      results.push(...collectSvelteFiles(full));
    } else if (entry.endsWith('.svelte')) {
      results.push(full);
    }
  }
  return results;
}

/**
 * Returns an array of { file, line, text } for any line that:
 *   1. contains a :global() selector targeting one of the pill dropdown classes AND
 *   2. has `position:` on the same or adjacent lines within the same CSS rule block
 *
 * Strategy: look for `:global(.pill-dropdown)`, `:global(.folder-picker-dropdown)`,
 * `:global(.pill-date-picker)` selector lines, then scan the following lines until
 * the closing `}` for any `position:` declaration.
 */
interface Violation {
  file: string;
  selectorLine: number;
  positionLine: number;
  selectorText: string;
  positionText: string;
}

const GUARDED_SELECTORS = [
  ':global(.pill-dropdown)',
  ':global(.folder-picker-dropdown)',
  ':global(.pill-date-picker)',
];

function findPositionOverrides(sveltePath: string): Violation[] {
  const content = readFileSync(sveltePath, 'utf-8');
  const lines = content.split('\n');
  const violations: Violation[] = [];

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const matchedSelector = GUARDED_SELECTORS.find((sel) => line.includes(sel));
    if (!matchedSelector) continue;

    // Scan forward for `position:` within this rule block (until closing `}`)
    // Handles both single-line rules and multi-line blocks
    let depth = 0;
    for (let j = i; j < Math.min(i + 20, lines.length); j++) {
      const ruleL = lines[j];
      depth += (ruleL.match(/{/g) || []).length;
      depth -= (ruleL.match(/}/g) || []).length;

      // Check this line for a position declaration
      if (j > i && /position\s*:/.test(ruleL)) {
        violations.push({
          file: sveltePath,
          selectorLine: i + 1, // 1-indexed
          positionLine: j + 1,
          selectorText: line.trim(),
          positionText: ruleL.trim(),
        });
        break;
      }

      // Once we've closed the block, stop scanning
      if (j > i && depth <= 0) break;
    }
  }

  return violations;
}

describe('#2208 contract — pill-dropdown positioning must not be overridden outside Editor.svelte', () => {
  it('no .svelte file outside Editor.svelte sets position on :global(.pill-dropdown), :global(.folder-picker-dropdown), or :global(.pill-date-picker)', () => {
    const allSvelteFiles = collectSvelteFiles(SRC_DIR);
    const nonEditorFiles = allSvelteFiles.filter((f) => f !== EDITOR_SVELTE);

    const allViolations: Violation[] = [];
    for (const file of nonEditorFiles) {
      allViolations.push(...findPositionOverrides(file));
    }

    if (allViolations.length > 0) {
      const report = allViolations
        .map(
          (v) =>
            `  ${v.file.replace(REPO_ROOT, '')}:${v.positionLine}\n` +
            `    selector: ${v.selectorText}\n` +
            `    override: ${v.positionText}`
        )
        .join('\n');
      throw new Error(
        `#2208 contract violated — ${allViolations.length} position override(s) found on pill dropdown selectors outside Editor.svelte:\n\n${report}\n\n` +
          `These overrides break pill dropdown positioning (issue #2378). Remove them; Editor.svelte owns position:fixed on these elements.`
      );
    }

    expect(allViolations).toHaveLength(0);
  });
});

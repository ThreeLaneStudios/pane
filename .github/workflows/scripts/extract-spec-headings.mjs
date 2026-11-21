// extract-spec-headings.mjs
//
// Pure functions: docs/spec/*.md -> (a) the flat heading-list text sent to
// Call 1 (classify), and (b) a lookup table of heading text -> section
// text, used to pull the matched section's body for Call 2 (spec-check).
//
// No network, no GitHub, no Anthropic calls in this file, same reasoning
// Reed's scope gave for splitting it out (#3227 scope comment, §1 item 3):
// "a pure function with one input (the checkout) and one output (a
// string); independently testable the same way scripts/check-spec-first.sh
// is." Tested by .github/workflows/scripts/tests/extract-spec-headings.test.mjs.
//
// Only "##" and "###" lines are offered as selectable spec sections. The
// file-title "#" line is excluded, it spans the whole file, it is not a
// section (measured in Reed's scope comment, §3: 767 selectable ##/###
// sections across docs/spec/, median ~1.3KB, p95 ~16.3KB).

import fs from "node:fs";
import path from "node:path";

const HEADING_RE = /^(#{1,3})\s+(.*)$/;

/**
 * Reads every *.md file directly under specDir (non-recursive, matches
 * the flat docs/spec/ layout) and returns an array of
 * { file, level, text, startLine } for every heading line found, in file
 * order, files sorted alphabetically for deterministic output.
 */
export function collectHeadings(specDir) {
  const files = fs
    .readdirSync(specDir)
    .filter((f) => f.endsWith(".md"))
    .sort();

  const headings = [];
  for (const file of files) {
    const fullPath = path.join(specDir, file);
    const lines = fs.readFileSync(fullPath, "utf8").split("\n");
    lines.forEach((line, idx) => {
      const m = HEADING_RE.exec(line);
      if (m) {
        headings.push({
          file,
          level: m[1].length,
          text: line, // full line, including the leading #/##/### markers
          startLine: idx,
          totalLines: lines.length,
          lines, // kept per-heading for buildSections' span walk below
        });
      }
    });
  }
  return headings;
}

/**
 * The flat "file.md: heading text" list sent to Call 1, one line per
 * selectable (##/###) heading. Matches the sample format in Reed's scope
 * comment (§3) exactly, including each file's own numbering convention
 * (§X.Y, plain numbers, or none) verbatim, no reformatting.
 */
export function buildHeadingListText(headings) {
  return headings
    .filter((h) => h.level === 2 || h.level === 3)
    .map((h) => `${h.file}: ${h.text}`)
    .join("\n");
}

/**
 * Builds a lookup table from a selectable heading's exact text (the part
 * after "file.md: ", i.e. h.text above, what the model is instructed to
 * copy verbatim into spec_section) to its section span: from that heading
 * line to the next heading of equal-or-higher level, or end of file.
 *
 * Returns a Map<headingText, Array<{file, text}>>, an array because two
 * files can share identical heading text (e.g. "## Overview" in multiple
 * spec files, confirmed in Reed's scope comment §3); callers resolve the
 * ambiguity (lookupSection below picks the alphabetically-first file and
 * reports the collision count).
 */
export function buildSections(headings) {
  const byFile = new Map();
  for (const h of headings) {
    if (!byFile.has(h.file)) byFile.set(h.file, []);
    byFile.get(h.file).push(h);
  }

  const sections = new Map();
  for (const [file, fileHeadings] of byFile) {
    const lines = fileHeadings[0].lines;
    for (let i = 0; i < fileHeadings.length; i++) {
      const h = fileHeadings[i];
      if (h.level !== 2 && h.level !== 3) continue; // title line excluded
      // Find the next heading at level <= h.level to close this span.
      let endLine = lines.length;
      for (let j = i + 1; j < fileHeadings.length; j++) {
        if (fileHeadings[j].level <= h.level) {
          endLine = fileHeadings[j].startLine;
          break;
        }
      }
      const text = lines.slice(h.startLine, endLine).join("\n").trimEnd();
      if (!sections.has(h.text)) sections.set(h.text, []);
      sections.get(h.text).push({ file, text });
    }
  }
  return sections;
}

/**
 * Resolves a model-returned spec_section string (expected to be an exact
 * copy of a heading-list line's text, after the "file.md: " prefix)
 * against the sections map. Returns null if there is no exact match (the
 * caller treats that the same as a null spec_section, see validation
 * rules in issue-triage-ai.mjs, §8a of the scope).
 *
 * On a collision (identical heading text in >1 file), picks the
 * alphabetically-first file deterministically and reports the collision
 * count so it can be logged.
 */
export function lookupSection(sections, headingText) {
  if (typeof headingText !== "string") return null;
  const matches = sections.get(headingText);
  if (!matches || matches.length === 0) return null;
  const sorted = [...matches].sort((a, b) => a.file.localeCompare(b.file));
  return { file: sorted[0].file, text: sorted[0].text, collisionCount: sorted.length };
}

/**
 * Truncates text to at most maxChars Unicode codepoints (mirrors
 * src-tauri/src/ai/web.rs's truncate_at_char_boundary, which counts
 * `chars()`, Unicode scalar values, not UTF-16 code units; Array.from
 * on a string iterates by codepoint for the same reason). Returns
 * { text, wasTruncated, shownChars, totalChars } and, when truncated,
 * appends the same marker line web_fetch's own truncation path uses:
 * "[... truncated, N of M chars shown]" (src-tauri/src/ai/web.rs:388).
 */
export function truncateSection(text, maxChars) {
  const codepoints = Array.from(text);
  const totalChars = codepoints.length;
  if (totalChars <= maxChars) {
    return { text, wasTruncated: false, shownChars: totalChars, totalChars };
  }
  const shown = codepoints.slice(0, maxChars).join("");
  return {
    text: `${shown}\n[... truncated, ${maxChars} of ${totalChars} chars shown]`,
    wasTruncated: true,
    shownChars: maxChars,
    totalChars,
  };
}

/**
 * Convenience wrapper used by issue-triage-ai.mjs: reads specDir once and
 * returns everything Call 1 and Call 2 need.
 */
export function loadSpec(specDir) {
  const headings = collectHeadings(specDir);
  return {
    headingListText: buildHeadingListText(headings),
    sections: buildSections(headings),
  };
}

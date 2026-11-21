// Unit tests for extract-spec-headings.mjs. No network, no real docs/spec/
// checkout required, fixtures are synthetic markdown files written to a
// temp directory per test. Run: node --test .github/workflows/scripts/tests/
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import {
  collectHeadings,
  buildHeadingListText,
  buildSections,
  lookupSection,
  truncateSection,
  loadSpec,
} from "../extract-spec-headings.mjs";

function writeFixture(files) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "spec-fixture-"));
  for (const [name, content] of Object.entries(files)) {
    fs.writeFileSync(path.join(dir, name), content);
  }
  return dir;
}

test("collectHeadings finds #, ##, ### lines in file order, files sorted", () => {
  const dir = writeFixture({
    "b.md": "# B Title\n## B Section\nbody\n",
    "a.md": "# A Title\n## A Section\nbody\n### A Sub\nmore\n",
  });
  const headings = collectHeadings(dir);
  assert.deepEqual(
    headings.map((h) => [h.file, h.level, h.text]),
    [
      ["a.md", 1, "# A Title"],
      ["a.md", 2, "## A Section"],
      ["a.md", 3, "### A Sub"],
      ["b.md", 1, "# B Title"],
      ["b.md", 2, "## B Section"],
    ],
  );
});

test("buildHeadingListText excludes the file-title (#) line, keeps ##/### only", () => {
  const dir = writeFixture({ "x.md": "# Title\n## Keep\nbody\n### Also keep\nmore\n" });
  const text = buildHeadingListText(collectHeadings(dir));
  assert.equal(text, "x.md: ## Keep\nx.md: ### Also keep");
});

test("buildSections spans a heading to the next heading of equal-or-higher level (a ## section includes its ### children)", () => {
  const dir = writeFixture({
    "x.md": ["# Title", "## One", "line A", "line B", "### One.a", "line C", "## Two", "line D", ""].join("\n"),
  });
  const headings = collectHeadings(dir);
  const sections = buildSections(headings);
  // "## One" does not stop at its own "###" child, only at the next ##/# heading.
  const one = sections.get("## One")[0];
  assert.equal(one.text, "## One\nline A\nline B\n### One.a\nline C");
  const oneA = sections.get("### One.a")[0];
  assert.equal(oneA.text, "### One.a\nline C");
  const two = sections.get("## Two")[0];
  assert.equal(two.text, "## Two\nline D");
});

test("buildSections / lookupSection handle duplicate heading text across files deterministically", () => {
  const dir = writeFixture({
    "zz.md": "# Z\n## Overview\nzz body\n",
    "aa.md": "# A\n## Overview\naa body\n",
  });
  const sections = buildSections(collectHeadings(dir));
  const resolved = lookupSection(sections, "## Overview");
  assert.equal(resolved.file, "aa.md"); // alphabetically first
  assert.equal(resolved.collisionCount, 2);
});

test("lookupSection returns null for an unknown or non-string heading", () => {
  const dir = writeFixture({ "x.md": "# Title\n## Real\nbody\n" });
  const sections = buildSections(collectHeadings(dir));
  assert.equal(lookupSection(sections, "## Not Real"), null);
  assert.equal(lookupSection(sections, null), null);
  assert.equal(lookupSection(sections, 42), null);
});

test("truncateSection leaves short text untouched", () => {
  const r = truncateSection("short text", 100);
  assert.equal(r.wasTruncated, false);
  assert.equal(r.text, "short text");
});

test("truncateSection caps at maxChars codepoints and appends the web_fetch-style marker", () => {
  const long = "x".repeat(50);
  const r = truncateSection(long, 10);
  assert.equal(r.wasTruncated, true);
  assert.equal(r.shownChars, 10);
  assert.equal(r.totalChars, 50);
  assert.ok(r.text.endsWith("[... truncated, 10 of 50 chars shown]"));
  assert.equal(r.text.startsWith("x".repeat(10)), true);
});

test("truncateSection counts Unicode codepoints, not UTF-16 code units (emoji safety)", () => {
  // Each of these emoji is one codepoint but two UTF-16 code units.
  const text = "ab" + "\u{1F600}".repeat(3); // 'a','b', then 3 emoji codepoints = 5 codepoints
  const r = truncateSection(text, 3);
  assert.equal(r.totalChars, 5);
  assert.equal(r.wasTruncated, true);
});

test("loadSpec wires collectHeadings/buildHeadingListText/buildSections together", () => {
  const dir = writeFixture({ "x.md": "# Title\n## Section\nbody\n" });
  const spec = loadSpec(dir);
  assert.equal(spec.headingListText, "x.md: ## Section");
  assert.equal(spec.sections.get("## Section")[0].text, "## Section\nbody");
});

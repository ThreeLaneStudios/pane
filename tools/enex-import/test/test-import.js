/**
 * Tests for the ENEX import tool.
 * 
 * Run: node --test test/test-import.js
 */

import { describe, it, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { readFile, rm, stat, readdir } from 'fs/promises';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';

import { EnexParser } from '../src/enex-parser.js';
import { convertEnmlToMarkdown } from '../src/enml-converter.js';
import { makeSlug, makeNoteId, buildFrontmatter, buildResourceMap } from '../src/note-writer.js';
import { runImport } from '../src/importer.js';

const __dirname = dirname(fileURLToPath(import.meta.url));
const SAMPLE_ENEX = join(__dirname, 'sample.enex');
const TEST_VAULT = join(__dirname, '_test-vault');

// --- Unit Tests ---

describe('makeSlug', () => {
  it('converts titles to URL-safe slugs', () => {
    assert.equal(makeSlug('Hello World'), 'hello-world');
    assert.equal(makeSlug('Meeting Notes - 2026-02-10'), 'meeting-notes-2026-02-10');
    assert.equal(makeSlug('  Spaces Everywhere  '), 'spaces-everywhere');
  });

  it('handles empty/missing titles', () => {
    assert.equal(makeSlug(''), 'untitled');
    assert.equal(makeSlug(null), 'untitled');
    assert.equal(makeSlug(undefined), 'untitled');
  });

  it('handles special characters', () => {
    const slug = makeSlug('Notes & Ideas: A "Test"');
    assert.ok(slug.length > 0);
    assert.ok(!slug.includes('&'));
    assert.ok(!slug.includes('"'));
  });

  it('truncates long titles', () => {
    const longTitle = 'A'.repeat(200);
    const slug = makeSlug(longTitle);
    assert.ok(slug.length <= 80);
  });
});

describe('makeNoteId', () => {
  it('generates consistent IDs', () => {
    const date = new Date('2026-01-01T00:00:00Z');
    const id1 = makeNoteId('Test Note', date);
    const id2 = makeNoteId('Test Note', date);
    assert.equal(id1, id2);
  });

  it('generates different IDs for different notes', () => {
    const date = new Date('2026-01-01T00:00:00Z');
    const id1 = makeNoteId('Note A', date);
    const id2 = makeNoteId('Note B', date);
    assert.notEqual(id1, id2);
  });

  it('returns 12-char hex string', () => {
    const id = makeNoteId('Test', new Date());
    assert.match(id, /^[a-f0-9]{12}$/);
  });
});

describe('buildFrontmatter', () => {
  it('includes title, tags, dates', () => {
    const fm = buildFrontmatter({
      title: 'Test Note',
      tags: ['tag1', 'tag2'],
      created: new Date('2026-01-01T09:00:00Z'),
      updated: new Date('2026-02-10T12:00:00Z'),
      sourceUrl: 'https://example.com',
    });

    assert.ok(fm.startsWith('---\n'));
    assert.ok(fm.endsWith('---\n'));
    assert.ok(fm.includes('title: Test Note'));
    assert.ok(fm.includes('tag1'));
    assert.ok(fm.includes('tag2'));
    assert.ok(fm.includes('source_url: https://example.com'));
  });

  it('handles notes with no tags', () => {
    const fm = buildFrontmatter({ title: 'Minimal' });
    assert.ok(fm.includes('title: Minimal'));
    assert.ok(!fm.includes('tags'));
  });
});

// --- ENEX Parser Tests ---

describe('EnexParser', () => {
  it('parses sample ENEX and emits correct note count', async () => {
    const parser = new EnexParser();
    const notes = [];
    parser.on('note', (note) => notes.push(note));

    const count = await parser.parse(SAMPLE_ENEX);
    assert.equal(count, 5);
    assert.equal(notes.length, 5);
  });

  it('parses note titles correctly', async () => {
    const parser = new EnexParser();
    const notes = [];
    parser.on('note', (note) => notes.push(note));

    await parser.parse(SAMPLE_ENEX);

    assert.equal(notes[0].title, 'Welcome to Trove');
    assert.equal(notes[1].title, 'Meeting Notes - Product Sync');
    assert.equal(notes[2].title, 'Quick Capture');
    assert.equal(notes[3].title, 'Note with Image');
    assert.equal(notes[4].title, '');  // untitled note
  });

  it('parses tags correctly', async () => {
    const parser = new EnexParser();
    const notes = [];
    parser.on('note', (note) => notes.push(note));

    await parser.parse(SAMPLE_ENEX);

    assert.deepEqual(notes[0].tags, ['welcome', 'getting-started']);
    assert.deepEqual(notes[1].tags, ['meetings', 'product', 'trove']);
    assert.deepEqual(notes[2].tags, ['ideas']);
  });

  it('parses dates correctly', async () => {
    const parser = new EnexParser();
    const notes = [];
    parser.on('note', (note) => notes.push(note));

    await parser.parse(SAMPLE_ENEX);

    assert.equal(notes[0].created.toISOString(), '2026-01-01T09:00:00.000Z');
    assert.equal(notes[0].updated.toISOString(), '2026-02-10T12:00:00.000Z');
  });

  it('parses note attributes', async () => {
    const parser = new EnexParser();
    const notes = [];
    parser.on('note', (note) => notes.push(note));

    await parser.parse(SAMPLE_ENEX);

    assert.equal(notes[0].sourceUrl, 'https://trove.example.com');
    assert.equal(notes[0].author, 'Matt');
  });

  it('parses resources', async () => {
    const parser = new EnexParser();
    const notes = [];
    parser.on('note', (note) => notes.push(note));

    await parser.parse(SAMPLE_ENEX);

    const imageNote = notes[3];
    assert.equal(imageNote.resources.length, 1);
    assert.equal(imageNote.resources[0].mime, 'image/png');
    assert.equal(imageNote.resources[0].filename, 'test-image.png');
    assert.ok(imageNote.resources[0].data.length > 0);
  });
});

// --- ENML Converter Tests ---

describe('ENML Converter', () => {
  it('converts basic formatting', () => {
    const enml = '<en-note><p>This is <b>bold</b> and <i>italic</i>.</p></en-note>';
    const md = convertEnmlToMarkdown(enml, new Map(), './attachments');
    assert.ok(md.includes('**bold**'));
    assert.ok(md.includes('*italic*'));
  });

  it('converts headings', () => {
    const enml = '<en-note><h1>Title</h1><h2>Subtitle</h2><h3>Section</h3></en-note>';
    const md = convertEnmlToMarkdown(enml, new Map(), './attachments');
    assert.ok(md.includes('# Title'));
    assert.ok(md.includes('## Subtitle'));
    assert.ok(md.includes('### Section'));
  });

  it('converts unordered lists', () => {
    const enml = '<en-note><ul><li>One</li><li>Two</li></ul></en-note>';
    const md = convertEnmlToMarkdown(enml, new Map(), './attachments');
    assert.ok(md.includes('One'));
    assert.ok(md.includes('Two'));
    // Turndown uses - or * for bullet markers
    assert.ok(md.includes('-') || md.includes('*'));
  });

  it('converts ordered lists', () => {
    const enml = '<en-note><ol><li>First</li><li>Second</li></ol></en-note>';
    const md = convertEnmlToMarkdown(enml, new Map(), './attachments');
    assert.ok(md.includes('1.'));
    assert.ok(md.includes('First'));
  });

  it('converts checkboxes', () => {
    const enml = '<en-note><div><en-todo checked="true"/>Done</div><div><en-todo checked="false"/>Not done</div></en-note>';
    const md = convertEnmlToMarkdown(enml, new Map(), './attachments');
    assert.ok(md.includes('[x]'));
    assert.ok(md.includes('[ ]'));
  });

  it('converts links', () => {
    const enml = '<en-note><a href="https://example.com">Example</a></en-note>';
    const md = convertEnmlToMarkdown(enml, new Map(), './attachments');
    assert.ok(md.includes('[Example](https://example.com)'));
  });

  it('converts code blocks', () => {
    const enml = '<en-note><pre><code>const x = 1;</code></pre></en-note>';
    const md = convertEnmlToMarkdown(enml, new Map(), './attachments');
    assert.ok(md.includes('```'));
    assert.ok(md.includes('const x = 1;'));
  });

  it('converts blockquotes', () => {
    const enml = '<en-note><blockquote>Wise words</blockquote></en-note>';
    const md = convertEnmlToMarkdown(enml, new Map(), './attachments');
    assert.ok(md.includes('> Wise words'));
  });

  it('converts horizontal rules', () => {
    const enml = '<en-note><p>Above</p><hr/><p>Below</p></en-note>';
    const md = convertEnmlToMarkdown(enml, new Map(), './attachments');
    assert.ok(md.includes('---'));
  });

  it('converts en-media to image references', () => {
    const resourceMap = new Map();
    resourceMap.set('abc123', {
      mime: 'image/png',
      filename: 'photo.png',
      savedFilename: 'photo.png',
    });

    const enml = '<en-note><en-media type="image/png" hash="abc123"/></en-note>';
    const md = convertEnmlToMarkdown(enml, resourceMap, './attachments');
    assert.ok(md.includes('photo.png'), `Expected image ref in: ${md}`);
    assert.ok(md.includes('./attachments/photo.png'), `Expected path in: ${md}`);
  });

  it('handles missing resources gracefully', () => {
    const enml = '<en-note><en-media type="image/png" hash="missing123"/></en-note>';
    const md = convertEnmlToMarkdown(enml, new Map(), './attachments');
    assert.ok(md.includes('missing attachment') || md.includes('missing123'), `Expected missing marker, got: ${md}`);
  });

  it('handles empty content', () => {
    const md = convertEnmlToMarkdown('', new Map(), './attachments');
    assert.equal(md, '');
  });

  it('converts tables', () => {
    const enml = '<en-note><table><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr></table></en-note>';
    const md = convertEnmlToMarkdown(enml, new Map(), './attachments');
    assert.ok(md.includes('A'));
    assert.ok(md.includes('B'));
    assert.ok(md.includes('1'));
    assert.ok(md.includes('2'));
  });
});

// --- Full Integration Test ---

describe('Full Import Integration', () => {
  before(async () => {
    // Clean up test vault
    await rm(TEST_VAULT, { recursive: true, force: true });
  });

  after(async () => {
    // Clean up
    await rm(TEST_VAULT, { recursive: true, force: true });
  });

  it('imports sample ENEX to vault', async () => {
    const report = await runImport({
      input: SAMPLE_ENEX,
      vault: TEST_VAULT,
      notebook: 'Test Notebook',
      quiet: true,
    });

    assert.equal(report.totalNotes, 5, `Expected 5 notes, got ${report.totalNotes}`);
    assert.equal(report.successCount, 5, `Expected 5 success, got ${report.successCount}. Errors: ${report.errors.join('; ')}`);
    assert.equal(report.errorCount, 0, `Unexpected errors: ${report.errors.join('; ')}`);
    assert.ok(report.totalTags > 0, `Expected tags, got ${report.totalTags}`);
    assert.ok(report.elapsedMs >= 0);
  });

  it('creates notebook directory', async () => {
    const nbDir = join(TEST_VAULT, 'notebooks', 'test-notebook');
    const s = await stat(nbDir);
    assert.ok(s.isDirectory());
  });

  it('creates markdown files', async () => {
    const nbDir = join(TEST_VAULT, 'notebooks', 'test-notebook');
    const files = await readdir(nbDir);
    assert.ok(files.length >= 5);
    assert.ok(files.some(f => f.endsWith('.md')));
  });

  it('markdown files have correct frontmatter', async () => {
    const nbDir = join(TEST_VAULT, 'notebooks', 'test-notebook');
    const files = await readdir(nbDir);
    const welcomeFile = files.find(f => f.includes('welcome'));
    assert.ok(welcomeFile, 'welcome file should exist');

    const content = await readFile(join(nbDir, welcomeFile), 'utf8');
    assert.ok(content.startsWith('---'));
    assert.ok(content.includes('title: Welcome to Trove'));
    assert.ok(content.includes('welcome'));
    assert.ok(content.includes('getting-started'));
    assert.ok(content.includes('source_url'));
  });

  it('markdown content is well-formed', async () => {
    const nbDir = join(TEST_VAULT, 'notebooks', 'test-notebook');
    const files = await readdir(nbDir);
    const welcomeFile = files.find(f => f.includes('welcome'));
    const content = await readFile(join(nbDir, welcomeFile), 'utf8');

    // Should contain converted Markdown elements
    assert.ok(content.includes('# Welcome to Trove'));
    assert.ok(content.includes('**test note**'));
    assert.ok(content.includes('*various*'));
    assert.ok(content.includes('Bullet point'));
    assert.ok(content.includes('[x]'));
    assert.ok(content.includes('[ ]'));
    assert.ok(content.includes('```'));
    assert.ok(content.includes('[link to example.com](https://example.com)'));
    assert.ok(content.includes('---'));
    assert.ok(content.includes('> This is a blockquote'));
  });

  it('extracts attachments', async () => {
    const attachDir = join(TEST_VAULT, '.trove', 'attachments');
    const s = await stat(attachDir);
    assert.ok(s.isDirectory());

    // There should be at least one attachment directory
    const dirs = await readdir(attachDir);
    assert.ok(dirs.length >= 1);
  });

  it('handles untitled notes', async () => {
    const nbDir = join(TEST_VAULT, 'notebooks', 'test-notebook');
    const files = await readdir(nbDir);
    const untitledFile = files.find(f => f.includes('untitled'));
    assert.ok(untitledFile, 'untitled file should exist');

    const content = await readFile(join(nbDir, untitledFile), 'utf8');
    assert.ok(content.includes('title: Untitled'));
  });

  it('dry run does not create files', async () => {
    const dryVault = join(TEST_VAULT, '_dry-run');
    const report = await runImport({
      input: SAMPLE_ENEX,
      vault: dryVault,
      quiet: true,
      dryRun: true,
    });

    assert.equal(report.totalNotes, 5);
    assert.equal(report.successCount, 5);

    // Vault should not exist
    const s = await stat(dryVault).catch(() => null);
    assert.equal(s, null);
  });
});

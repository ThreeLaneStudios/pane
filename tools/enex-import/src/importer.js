/**
 * Main importer — orchestrates parsing, conversion, and writing.
 * 
 * Coordinates:
 *   1. EnexParser (streaming XML parser)
 *   2. ENML converter (HTML/ENML → Markdown)
 *   3. NoteWriter (saves to disk)
 * 
 * Handles progress reporting and generates an import report.
 */

import { basename, extname, resolve } from 'path';
import { stat, mkdir } from 'fs/promises';
import { EnexParser } from './enex-parser.js';
import { convertEnmlToMarkdown } from './enml-converter.js';
import { writeNote, buildResourceMap, makeNoteId, makeSlug } from './note-writer.js';
import cliProgress from 'cli-progress';

/**
 * @typedef {Object} ImportOptions
 * @property {string} input - Path to .enex file
 * @property {string} vault - Path to Trove vault root
 * @property {string} [notebook] - Override notebook name
 * @property {boolean} [dryRun] - Parse and convert but don't write
 * @property {boolean} [verbose] - Show per-note output
 * @property {boolean} [quiet] - Suppress progress bar
 */

/**
 * @typedef {Object} ImportReport
 * @property {number} totalNotes - Total notes found in ENEX
 * @property {number} successCount - Successfully imported
 * @property {number} warningCount - Imported with warnings
 * @property {number} errorCount - Failed to import
 * @property {number} totalAttachments - Attachments extracted
 * @property {number} totalTags - Unique tags found
 * @property {string[]} errors - Error details
 * @property {string[]} warnings - Warning details
 * @property {number} elapsedMs - Time taken
 */

/**
 * Run the ENEX import.
 * 
 * @param {ImportOptions} options 
 * @returns {Promise<ImportReport>}
 */
export async function runImport(options) {
  const startTime = Date.now();
  const { input, vault, notebook, dryRun, verbose, quiet } = options;

  // Validate input file
  const inputPath = resolve(input);
  const inputStat = await stat(inputPath).catch(() => null);
  if (!inputStat || !inputStat.isFile()) {
    throw new Error(`Input file not found: ${inputPath}`);
  }

  // Determine notebook name
  const notebookName = notebook || basename(inputPath, extname(inputPath));

  // Ensure vault directory exists
  const vaultPath = resolve(vault);
  if (!dryRun) {
    await mkdir(vaultPath, { recursive: true });
    await mkdir(`${vaultPath}/notebooks`, { recursive: true });
    await mkdir(`${vaultPath}/.trove/attachments`, { recursive: true });
  }

  // Estimate note count for progress bar (rough: count <note> occurrences)
  // We'll just show an indeterminate bar or update total as we go
  const report = {
    totalNotes: 0,
    successCount: 0,
    warningCount: 0,
    errorCount: 0,
    totalAttachments: 0,
    totalTags: 0,
    uniqueTags: new Set(),
    errors: [],
    warnings: [],
    elapsedMs: 0,
    notes: [],  // summary of each note
  };

  // Progress bar
  let progressBar = null;
  if (!quiet && !dryRun) {
    progressBar = new cliProgress.SingleBar({
      format: '  Importing │{bar}│ {value} notes │ {duration_formatted}',
      barCompleteChar: '█',
      barIncompleteChar: '░',
      hideCursor: true,
      barsize: 30,
    });
    progressBar.start(0, 0);
  }

  // Track slugs per notebook to handle collisions
  const slugCounts = new Map();

  // Collect notes synchronously, then process
  const parsedNotes = [];

  // Set up parser
  const parser = new EnexParser();

  parser.on('note', (note) => {
    parsedNotes.push(note);
  });

  parser.on('warning', (msg) => {
    report.warnings.push(msg);
  });

  // Run the parser (collects all notes)
  try {
    await parser.parse(inputPath, notebookName);
  } catch (err) {
    if (progressBar) progressBar.stop();
    throw new Error(`Failed to parse ENEX file: ${err.message}`);
  }

  report.totalNotes = parsedNotes.length;

  if (progressBar) {
    progressBar.setTotal(report.totalNotes);
  }

  // Process each note sequentially
  for (const note of parsedNotes) {
    try {
      // Build resource map for this note
      const resourceMap = buildResourceMap(note.resources || []);
      const noteId = makeNoteId(note.title, note.created);
      const attachmentDir = `.trove/attachments/${noteId}`;

      // Pre-assign filenames to resources before conversion
      for (const [hash, res] of resourceMap) {
        if (!res.savedFilename) {
          const ext = getExtForMime(res.mime);
          res.savedFilename = res.filename || `${hash}${ext}`;
        }
      }

      // Convert ENML to Markdown
      const markdown = convertEnmlToMarkdown(
        note.content,
        resourceMap,
        attachmentDir
      );

      // Handle slug collisions
      let slug = makeSlug(note.title);
      const nbSlug = makeSlug(notebookName);
      const key = `${nbSlug}/${slug}`;
      const count = slugCounts.get(key) || 0;
      if (count > 0) {
        slug = `${slug}-${count}`;
      }
      slugCounts.set(key, count + 1);

      if (!dryRun) {
        // Write to disk
        const result = await writeNote({
          note,
          markdown,
          vaultPath,
          notebookName,
          resourceMap,
        });

        report.totalAttachments += result.attachmentsWritten;

        if (result.warnings.length > 0) {
          report.warningCount++;
          for (const w of result.warnings) {
            report.warnings.push(`${note.title}: ${w}`);
          }
        }
      }

      // Track tags
      for (const tag of note.tags) {
        report.uniqueTags.add(tag);
      }

      report.successCount++;

      if (verbose) {
        const tagStr = note.tags.length > 0 ? ` [${note.tags.join(', ')}]` : '';
        console.log(`  ✓ ${note.title}${tagStr}`);
      }
    } catch (err) {
      report.errorCount++;
      report.errors.push(`${note.title || 'Untitled'}: ${err.message}`);
      if (verbose) {
        console.error(`  ✗ ${note.title || 'Untitled'}: ${err.message}`);
      }
    }

    if (progressBar) {
      progressBar.update(report.successCount + report.errorCount);
    }
  }

  if (progressBar) {
    progressBar.update(report.successCount + report.errorCount);
    progressBar.stop();
  }

  report.totalTags = report.uniqueTags.size;
  report.elapsedMs = Date.now() - startTime;

  return report;
}

/**
 * Format an import report for console output.
 */
export function formatReport(report, inputPath) {
  const lines = [];
  const elapsed = (report.elapsedMs / 1000).toFixed(1);

  lines.push('');
  lines.push('┌──────────────────────────────────────┐');
  lines.push('│         📋 Import Report              │');
  lines.push('├──────────────────────────────────────┤');
  lines.push(`│  Source:       ${basename(inputPath).slice(0, 21).padEnd(21)} │`);
  lines.push(`│  Total notes:  ${String(report.totalNotes).padEnd(21)} │`);
  lines.push(`│  ✅ Imported:  ${String(report.successCount).padEnd(21)} │`);
  if (report.warningCount > 0) {
    lines.push(`│  ⚠️  Warnings:  ${String(report.warningCount).padEnd(21)} │`);
  }
  if (report.errorCount > 0) {
    lines.push(`│  ❌ Errors:    ${String(report.errorCount).padEnd(21)} │`);
  }
  lines.push(`│  📎 Attachments: ${String(report.totalAttachments).padEnd(19)} │`);
  lines.push(`│  🏷️  Tags:       ${String(report.totalTags).padEnd(19)} │`);
  lines.push(`│  ⏱️  Time:       ${(elapsed + 's').padEnd(19)} │`);
  lines.push('└──────────────────────────────────────┘');

  if (report.errors.length > 0) {
    lines.push('');
    lines.push('Errors:');
    for (const err of report.errors.slice(0, 20)) {
      lines.push(`  ❌ ${err}`);
    }
    if (report.errors.length > 20) {
      lines.push(`  ... and ${report.errors.length - 20} more`);
    }
  }

  if (report.warnings.length > 0) {
    lines.push('');
    lines.push('Warnings:');
    for (const w of report.warnings.slice(0, 10)) {
      lines.push(`  ⚠️  ${w}`);
    }
    if (report.warnings.length > 10) {
      lines.push(`  ... and ${report.warnings.length - 10} more`);
    }
  }

  lines.push('');
  return lines.join('\n');
}

function getExtForMime(mime) {
  const map = {
    'image/jpeg': '.jpg', 'image/png': '.png', 'image/gif': '.gif',
    'image/webp': '.webp', 'image/svg+xml': '.svg', 'application/pdf': '.pdf',
    'audio/mpeg': '.mp3', 'audio/wav': '.wav', 'video/mp4': '.mp4',
    'text/plain': '.txt', 'text/html': '.html',
  };
  return map[mime] || '';
}

export default { runImport, formatReport };

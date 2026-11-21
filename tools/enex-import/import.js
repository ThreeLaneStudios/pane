#!/usr/bin/env node

/**
 * Trove ENEX Import CLI
 * 
 * Usage:
 *   node import.js --input export.enex --vault ~/Trove
 *   node import.js -i my-notes.enex -v ~/Trove --notebook "Personal"
 */

import { program } from 'commander';
import { resolve } from 'path';
import { runImport, formatReport } from './src/importer.js';

program
  .name('trove-import')
  .description('Import Evernote ENEX exports into Trove')
  .version('0.1.0')
  .requiredOption('-i, --input <path>', 'Path to .enex file')
  .requiredOption('-v, --vault <path>', 'Path to Trove vault (e.g. ~/Trove)')
  .option('-n, --notebook <name>', 'Notebook name (defaults to filename)')
  .option('--dry-run', 'Parse and convert without writing files')
  .option('--verbose', 'Show per-note output')
  .option('-q, --quiet', 'Suppress progress bar')
  .parse();

const opts = program.opts();

// Resolve home directory
function expandHome(p) {
  if (p.startsWith('~/') || p === '~') {
    return p.replace('~', process.env.HOME);
  }
  return p;
}

async function main() {
  const inputPath = resolve(expandHome(opts.input));
  const vaultPath = resolve(expandHome(opts.vault));

  console.log('');
  console.log('  📥 Trove ENEX Importer');
  console.log(`  Input:    ${inputPath}`);
  console.log(`  Vault:    ${vaultPath}`);
  if (opts.notebook) console.log(`  Notebook: ${opts.notebook}`);
  if (opts.dryRun) console.log('  Mode:     DRY RUN (no files written)');
  console.log('');

  try {
    const report = await runImport({
      input: inputPath,
      vault: vaultPath,
      notebook: opts.notebook,
      dryRun: opts.dryRun,
      verbose: opts.verbose,
      quiet: opts.quiet,
    });

    console.log(formatReport(report, inputPath));

    if (report.errorCount > 0) {
      process.exit(1);
    }
  } catch (err) {
    console.error(`\n  ❌ Import failed: ${err.message}\n`);
    if (opts.verbose) {
      console.error(err.stack);
    }
    process.exit(2);
  }
}

main();

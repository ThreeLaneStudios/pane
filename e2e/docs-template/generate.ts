#!/usr/bin/env node
// #470 docs-template PR: CLI entry point. Reads one journey's
// manifest.json (written by e2e/journeys/_runner.ts) plus its sibling
// screenshots, and writes:
//
//   <out>/+page.svelte   -- the real deliverable (page.ts)
//   <out>/preview.html   -- self-contained HTML proof, images copied in (preview.ts)
//   <out>/images/*.png   -- the screenshots preview.html references by relative path
//
// Run directly via `node` (Node 26, native TypeScript type-stripping --
// no ts-node/tsx dependency added for this), not through Playwright --
// this is a standalone build step, not a test. Example:
//
//   node e2e/docs-template/generate.ts \
//     --manifest e2e/journeys-output/create-an-agent/manifest.json \
//     --slug creating-an-agent \
//     --out e2e/docs-template-output/creating-an-agent
//
// Why `--slug` is a separate, explicit argument from the manifest's own
// `journey` field: Matt's ruling on #470 is that the generated page takes
// over the EXISTING hand-written page's address, `docs/creating-an-agent`
// -- which is NOT this journey's own name, `create-an-agent` (Reed's scope
// audit named this exact near-miss: "create-an-agent" != "creating-an-agent").
// Publishing at the journey's own slug would be wrong; publishing at the
// wrong slug by accident would be worse. Passing it explicitly, once, here
// in the CI step (not derived, not guessed) makes that mapping visible in
// a diff instead of embedded as an assumption in code.
//
// Determinism: every input this script reads is the manifest file plus
// the exact image filenames `manifest.steps` already names (never a
// directory listing of the images folder) -- see page.ts/preview.ts for
// the rendering functions' own determinism notes. Running this script
// twice against the same manifest + images produces byte-identical
// +page.svelte and preview.html output (verified; see PR description).
import fs from 'node:fs';
import path from 'node:path';
import { parseArgs } from 'node:util';
import { renderPageSvelte } from './page.ts';
import { renderPreviewHtml } from './preview.ts';
import type { JourneyManifest } from '../journeys/_runner.ts';

function parseCliArgs(argv: string[]): { manifestPath: string; slug: string; outDir: string } {
  const { values } = parseArgs({
    args: argv,
    options: {
      manifest: { type: 'string' },
      slug: { type: 'string' },
      out: { type: 'string' },
    },
  });
  if (!values.manifest || !values.slug || !values.out) {
    throw new Error('usage: generate.ts --manifest <path/to/manifest.json> --slug <publish-slug> --out <output-dir>');
  }
  return { manifestPath: values.manifest, slug: values.slug, outDir: values.out };
}

function main(): void {
  const { manifestPath, slug, outDir } = parseCliArgs(process.argv.slice(2));

  const manifestRaw = fs.readFileSync(manifestPath, 'utf8');
  const manifest: JourneyManifest = JSON.parse(manifestRaw);
  const manifestDir = path.dirname(manifestPath);

  // The only journey known to resolve to a real page in this run is the
  // one being rendered itself -- no second journey exists yet (confirmed:
  // e2e/journeys/ has exactly one *.journey.ts file today). Not a
  // directory listing; a literal one-element set built from the manifest
  // that was just parsed.
  const knownJourneys = new Set<string>([manifest.journey]);

  fs.mkdirSync(outDir, { recursive: true });

  const pageSvelte = renderPageSvelte(manifest, slug, knownJourneys);
  fs.writeFileSync(path.join(outDir, '+page.svelte'), pageSvelte);

  const imagesOutDir = path.join(outDir, 'images');
  fs.mkdirSync(imagesOutDir, { recursive: true });
  // Iterates manifest.steps in its own array order -- never `fs.readdir`s
  // manifestDir -- so the copy order (and therefore nothing about it that
  // could vary) depends only on the manifest's own already-deterministic
  // step order.
  for (const step of manifest.steps) {
    fs.copyFileSync(path.join(manifestDir, step.image), path.join(imagesOutDir, step.image));
  }

  const previewHtml = renderPreviewHtml(manifest, 'images', knownJourneys);
  fs.writeFileSync(path.join(outDir, 'preview.html'), previewHtml);

  console.log(`docs-template: wrote ${outDir}/+page.svelte, ${outDir}/preview.html, and ${manifest.steps.length} image(s) to ${imagesOutDir}/`);
}

main();

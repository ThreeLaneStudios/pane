#!/usr/bin/env node
// #470 docs-template PR: CLI entry point. Reads one journey's
// manifest.json (written by e2e/journeys/_runner.ts) plus its sibling
// screenshots, and writes:
//
//   <out>/+page.svelte   -- the real deliverable (page.ts)
//   <out>/images/*.png   -- the screenshots the generated page references
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
// #470 PR 1 (publish1): the standalone-HTML preview.ts/preview.html this
// script used to also write is REMOVED (not just unused) -- Matt's
// ruling on #470 replaces it with a CI step that builds the REAL
// paneapp-ai site with this script's `+page.svelte` output dropped in
// and screenshots the actual rendered page (see
// .github/workflows/e2e-linux.yml's "Build paneapp-ai with the generated
// docs page" job). The old preview.html's own header comment said
// plainly why it was only ever an approximation (its CSS was hand-rolled,
// not copied from paneapp-ai's real docs.css) -- that's exactly the gap
// the real-site-build proof closes, so keeping preview.ts around as dead
// code once nothing calls it would just be unexercised surface.
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
// directory listing of the images folder) -- see page.ts's own
// determinism notes. Running this script twice against the same
// manifest + images produces byte-identical `+page.svelte` output
// (re-verified for this PR's markup rewrite; see PR description).
import fs from 'node:fs';
import path from 'node:path';
import { parseArgs } from 'node:util';
import { renderPageSvelte } from './page.ts';
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

  console.log(`docs-template: wrote ${outDir}/+page.svelte and ${manifest.steps.length} image(s) to ${imagesOutDir}/`);
}

main();

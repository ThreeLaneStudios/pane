import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import zlib from 'node:zlib';
import { test, expect } from '../fixtures';

// Resolved relative to this file rather than process.cwd(), so the path is
// correct regardless of where `playwright test` is invoked from -- it
// always lands at e2e/test-results/smoke.png, matching
// .github/workflows/e2e-linux.yml's "Upload smoke screenshot" step
// (artifact `pane-smoke-screenshot`, path `e2e/test-results/smoke.png`)
// character-for-character. Playwright's own default `outputDir` is not
// used here because it nests per-test subfolders; the workflow uploads a
// single fixed path, so the test writes to that fixed path directly.
const SCREENSHOT_PATH = path.join(
  path.dirname(fileURLToPath(import.meta.url)),
  '..',
  'test-results',
  'smoke.png',
);

/**
 * Minimal, dependency-free PNG decoder (8-bit depth, color types
 * 0/2/3/4/6) -- just enough to defilter scanlines back to raw pixel
 * bytes. No PNG-decoding package exists in this project's dependencies,
 * and this fix intentionally doesn't add one (see smoke.spec.ts's
 * CHANGELOG entry). Throws on anything it doesn't understand (e.g. a
 * 16-bit-depth or interlaced PNG) rather than silently passing.
 */
function decodePng(buf: Buffer): { width: number; height: number; channels: number; pixels: Buffer } {
  let offset = 8; // skip the 8-byte PNG signature
  let width = 0;
  let height = 0;
  let bitDepth = 0;
  let colorType = -1;
  const idatParts: Buffer[] = [];
  while (offset < buf.length) {
    const len = buf.readUInt32BE(offset);
    const type = buf.toString('ascii', offset + 4, offset + 8);
    const data = buf.subarray(offset + 8, offset + 8 + len);
    if (type === 'IHDR') {
      width = data.readUInt32BE(0);
      height = data.readUInt32BE(4);
      bitDepth = data[8];
      colorType = data[9];
    } else if (type === 'IDAT') {
      idatParts.push(data);
    } else if (type === 'IEND') {
      break;
    }
    offset += 8 + len + 4;
  }
  if (bitDepth !== 8) {
    throw new Error(`smoke.png: unsupported PNG bit depth ${bitDepth} (expected 8)`);
  }
  const channelsByColorType: Record<number, number> = { 0: 1, 2: 3, 3: 1, 4: 2, 6: 4 };
  const channels = channelsByColorType[colorType];
  if (!channels) {
    throw new Error(`smoke.png: unsupported PNG color type ${colorType}`);
  }
  const inflated = zlib.inflateSync(Buffer.concat(idatParts));
  const rowBytes = width * channels;
  const pixels = Buffer.alloc(rowBytes * height);
  let prevRowStart = -1;
  for (let y = 0; y < height; y++) {
    const inRowStart = y * (rowBytes + 1);
    const filterType = inflated[inRowStart];
    const outRowStart = y * rowBytes;
    for (let x = 0; x < rowBytes; x++) {
      const raw = inflated[inRowStart + 1 + x];
      const a = x >= channels ? pixels[outRowStart + x - channels] : 0;
      const b = prevRowStart >= 0 ? pixels[prevRowStart + x] : 0;
      const c = x >= channels && prevRowStart >= 0 ? pixels[prevRowStart + x - channels] : 0;
      let value: number;
      switch (filterType) {
        case 0:
          value = raw;
          break;
        case 1:
          value = (raw + a) & 0xff;
          break;
        case 2:
          value = (raw + b) & 0xff;
          break;
        case 3:
          value = (raw + ((a + b) >> 1)) & 0xff;
          break;
        case 4: {
          const p = a + b - c;
          const pa = Math.abs(p - a);
          const pb = Math.abs(p - b);
          const pc = Math.abs(p - c);
          const pred = pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
          value = (raw + pred) & 0xff;
          break;
        }
        default:
          throw new Error(`smoke.png: unsupported PNG filter type ${filterType} on row ${y}`);
      }
      pixels[outRowStart + x] = value;
    }
    prevRowStart = outRowStart;
  }
  return { width, height, channels, pixels };
}

/**
 * Proves the frame isn't a blank/uniform capture -- the exact failure
 * mode a byte-size check alone misses, since a solid-color 1280x800 PNG
 * still compresses to several KB (verified locally: ~5.9KB), comfortably
 * past any "non-empty file" size threshold.
 */
function analyzeVariation({ width, height, channels, pixels }: { width: number; height: number; channels: number; pixels: Buffer }) {
  const totalPixels = width * height;
  const sampleStep = Math.max(1, Math.floor(totalPixels / 20000));
  const distinctColors = new Set<string>();
  let minR = 255, maxR = 0, minG = 255, maxG = 0, minB = 255, maxB = 0;
  for (let p = 0; p < totalPixels; p += sampleStep) {
    const i = p * channels;
    const r = pixels[i];
    const g = channels >= 2 ? pixels[i + 1] : r;
    const b = channels >= 3 ? pixels[i + 2] : r;
    if (r < minR) minR = r;
    if (r > maxR) maxR = r;
    if (g < minG) minG = g;
    if (g > maxG) maxG = g;
    if (b < minB) minB = b;
    if (b > maxB) maxB = b;
    distinctColors.add(`${r},${g},${b}`);
  }
  return {
    distinctSampledColors: distinctColors.size,
    maxChannelRange: Math.max(maxR - minR, maxG - minG, maxB - minB),
  };
}

test('Pane app loads and shows content', async ({ tauriPage }) => {
  // Verify the app loads at all
  const title = await tauriPage.title();
  console.log('Page title:', title);

  // Check if any visible text exists on the page
  const bodyText = await tauriPage.textContent('body');
  console.log('Body text length:', bodyText?.length);

  // Take a screenshot for visual verification. The workflow's
  // "Upload smoke screenshot" step uploads this exact path as the
  // pane-smoke-screenshot artifact -- `screenshot()` with no `path` wrote
  // nothing to disk, which is why that step previously warned with
  // "no file found at e2e/test-results/smoke.png" even on a green run.
  fs.mkdirSync(path.dirname(SCREENSHOT_PATH), { recursive: true });
  const screenshotBuffer = await tauriPage.screenshot({ path: SCREENSHOT_PATH });
  console.log('Screenshot captured:', screenshotBuffer ? 'yes' : 'no', 'bytes:', screenshotBuffer?.length);

  // tauri-plugin-playwright 0.4.1 writes the PNG to disk and returns an
  // EMPTY buffer whenever a `path` is passed to screenshot() -- the
  // returned buffer is only populated when no `path` is given (hence
  // "bytes: 0" in the log line above on a successful capture). So the
  // returned buffer is not a usable signal here; everything below is
  // checked against the file on disk instead.

  // It must actually have landed on disk at the path the workflow
  // uploads from.
  expect(fs.existsSync(SCREENSHOT_PATH)).toBe(true);
  const onDisk = fs.readFileSync(SCREENSHOT_PATH);

  // A sane byte size rules out an empty or truncated file. This alone
  // does NOT rule out a blank frame (see analyzeVariation below) -- a
  // solid-color screenshot this size still compresses well past this
  // floor -- so it's a minimum-sanity check, not the blank-frame proof.
  expect(onDisk.length).toBeGreaterThan(1024);

  // Decode the actual uploaded PNG and prove it isn't a uniform
  // single-colour frame (the specific failure mode that a file-exists +
  // byte-size check cannot catch): a real capture of the Pane window must
  // show many distinct sampled colors and real spread in at least one
  // color channel. A blank/solid frame decodes to exactly one distinct
  // color and zero channel range, as verified locally against
  // synthetically generated solid-black and solid-white 1280x800 PNGs.
  const decoded = decodePng(onDisk);
  const variation = analyzeVariation(decoded);
  console.log('Screenshot variation:', JSON.stringify(variation), 'dimensions:', decoded.width, 'x', decoded.height);
  expect(variation.distinctSampledColors).toBeGreaterThan(10);
  expect(variation.maxChannelRange).toBeGreaterThan(20);
});

test('Pane sidebar is visible', async ({ tauriPage }) => {
  // Wait for the app to initialize
  await tauriPage.waitForSelector('.sidebar', 10000);
  const sidebarVisible = await tauriPage.isVisible('.sidebar');
  console.log('Sidebar visible:', sidebarVisible);
});

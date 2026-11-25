import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';
import { expect } from '@playwright/test';
import type { TauriPage } from '@srsholmes/tauri-playwright';

// Extracted from e2e/tests/smoke.spec.ts (#470 PR 3) per the convention
// posted on issue #470 (comment 7169240575): "every screenshot call passes
// an explicit path, and every assertion reads from the file on disk --
// never from the returned buffer. This belongs in one shared helper... so
// no journey author can reintroduce either mistake." Moved verbatim, not
// rewritten -- the decode/validate logic itself was already correct (PR
// #472's final audit, §5).
//
// Why disk-only: tauri-plugin-playwright 0.4.1's screenshot() returns a
// populated Buffer only when called with NO `path`; when a `path` IS
// passed, it writes the PNG to disk and returns an EMPTY buffer. PR #472
// hit both failure modes in sequence (no file ever written, then a correct
// capture read as a false "bytes: 0" failure) before landing on
// disk-only validation -- see that PR's own history for the full story.

/**
 * Minimal, dependency-free PNG decoder (8-bit depth, color types
 * 0/2/3/4/6) -- just enough to defilter scanlines back to raw pixel
 * bytes. No PNG-decoding package exists in this project's dependencies,
 * and this helper intentionally doesn't add one. Throws on anything it
 * doesn't understand (e.g. a 16-bit-depth or interlaced PNG) rather than
 * silently passing.
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
    throw new Error(`screenshot PNG: unsupported bit depth ${bitDepth} (expected 8)`);
  }
  const channelsByColorType: Record<number, number> = { 0: 1, 2: 3, 3: 1, 4: 2, 6: 4 };
  const channels = channelsByColorType[colorType];
  if (!channels) {
    throw new Error(`screenshot PNG: unsupported color type ${colorType}`);
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
          throw new Error(`screenshot PNG: unsupported filter type ${filterType} on row ${y}`);
      }
      pixels[outRowStart + x] = value;
    }
    prevRowStart = outRowStart;
  }
  return { width, height, channels, pixels };
}

/**
 * Proves the frame isn't a blank/uniform capture -- the exact failure mode
 * a byte-size check alone misses, since a solid-color 1280x800 PNG still
 * compresses to several KB (verified locally: ~5.9KB), comfortably past
 * any "non-empty file" size threshold.
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

export type ScreenshotVariation = { distinctSampledColors: number; maxChannelRange: number };

export type ScreenshotThresholds = {
  /** Minimum on-disk byte size. Default 1024. */
  minBytes?: number;
  /** Minimum distinct sampled colors (blank-frame rejection). Default 10. */
  minDistinctColors?: number;
  /** Minimum max-channel range (blank-frame rejection). Default 20. */
  minChannelRange?: number;
};

/**
 * Captures a screenshot to `screenshotPath` and validates it from disk --
 * never from `tauriPage.screenshot()`'s returned buffer, which
 * tauri-plugin-playwright 0.4.1 returns empty (`Buffer.alloc(0)`)
 * unconditionally whenever a `path` is passed.
 *
 * Validation carries all four checks posted as the shared-helper
 * convention on issue #470: file exists, on-disk size > threshold,
 * distinctSampledColors > threshold, maxChannelRange > threshold.
 */
export async function captureAndValidateScreenshot(
  tauriPage: TauriPage,
  screenshotPath: string,
  thresholds: ScreenshotThresholds = {},
): Promise<{ onDisk: Buffer; variation: ScreenshotVariation }> {
  const { minBytes = 1024, minDistinctColors = 10, minChannelRange = 20 } = thresholds;

  fs.mkdirSync(path.dirname(screenshotPath), { recursive: true });
  await tauriPage.screenshot({ path: screenshotPath });

  // It must actually have landed on disk at the given path.
  expect(fs.existsSync(screenshotPath)).toBe(true);
  const onDisk = fs.readFileSync(screenshotPath);

  // A sane byte size rules out an empty or truncated file. This alone does
  // NOT rule out a blank frame (see analyzeVariation below) -- a
  // solid-color screenshot this size still compresses well past this
  // floor -- so it's a minimum-sanity check, not the blank-frame proof.
  expect(onDisk.length).toBeGreaterThan(minBytes);

  // Decode the actual uploaded PNG and prove it isn't a uniform
  // single-colour frame: a real capture must show many distinct sampled
  // colors and real spread in at least one color channel. A blank/solid
  // frame decodes to exactly one distinct color and zero channel range, as
  // verified locally against synthetically generated solid-black and
  // solid-white 1280x800 PNGs.
  const decoded = decodePng(onDisk);
  const variation = analyzeVariation(decoded);
  expect(variation.distinctSampledColors).toBeGreaterThan(minDistinctColors);
  expect(variation.maxChannelRange).toBeGreaterThan(minChannelRange);

  return { onDisk, variation };
}

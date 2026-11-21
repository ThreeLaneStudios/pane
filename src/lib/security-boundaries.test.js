import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

const projectFile = (path) => readFileSync(resolve(process.cwd(), path), 'utf8');
const tauriConfig = JSON.parse(projectFile('src-tauri/tauri.conf.json'));
const productionCsp = tauriConfig.app.security.csp;
const commandRegistration = projectFile('src-tauri/src/lib.rs');
const systemFilesSource = projectFile('src-tauri/src/system_files.rs');

describe('production WebView security boundaries', () => {
  it('does not grant arbitrary script or connection sources', () => {
    expect(productionCsp).not.toContain("'unsafe-eval'");
    expect(productionCsp).toContain("script-src 'self'");
    expect(productionCsp).toContain("connect-src 'self' ipc: http://ipc.localhost");
    const connectDirective = productionCsp.match(/connect-src[^;]*/)?.[0] ?? '';
    const nonIpcConnectSources = connectDirective.replace('http://ipc.localhost', '');
    expect(nonIpcConnectSources).not.toMatch(/\bhttps?:/);
    expect(nonIpcConnectSources).not.toMatch(/\bwss?:/);
  });

  it('blocks passive arbitrary image, form, base, and frame egress', () => {
    const imgDirective = productionCsp.match(/img-src[^;]*/)?.[0] ?? '';
    expect(imgDirective).toContain('https://img.youtube.com');
    const nonAllowlistedImageSources = imgDirective
      .replace('https://asset.localhost', '')
      .replace('https://img.youtube.com', '');
    expect(nonAllowlistedImageSources).not.toMatch(/\bhttps?:/);
    expect(productionCsp).toContain("frame-src 'self'");
    expect(productionCsp).toContain("form-action 'none'");
    expect(productionCsp).toContain("base-uri 'none'");
  });

  it('does not expose generic arbitrary-URL HTTP proxy commands', () => {
    expect(commandRegistration).not.toContain('system_files::proxy_http_get');
    expect(commandRegistration).not.toContain('system_files::proxy_http_post');
    expect(systemFilesSource).not.toContain('fn proxy_http_get');
    expect(systemFilesSource).not.toContain('fn proxy_http_post');
    expect(commandRegistration).toContain('system_files::gateway_chat_completions');
  });
});

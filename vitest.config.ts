import { defineConfig } from 'vitest/config';
import { resolve } from 'path';
import { compile } from 'svelte/compiler';
import type { Plugin } from 'vite';

/**
 * Minimal Svelte compiler transform for Vitest.
 *
 * @sveltejs/vite-plugin-svelte v6 requires vite 6+ (server.environments API),
 * but vitest bundles vite 5. This lightweight plugin uses svelte/compiler
 * directly, which works with any vite version and avoids the incompatibility.
 *
 * Covers: .svelte file compile → ESM JS (client output).
 * Does NOT cover: HMR, CSS extraction, SSR, preprocessors — not needed for unit tests.
 */
// TODO: Replace with @sveltejs/vite-plugin-svelte v6 once vitest bundles Vite 6 (currently stuck on Vite 5).
const svelteLite: Plugin = {
  name: 'svelte-lite-test',
  transform(code: string, id: string) {
    if (!id.endsWith('.svelte')) return null;
    try {
      const result = compile(code, {
        filename: id,
        generate: 'client',
        dev: false,
        css: 'injected',
      });
      return {
        code: result.js.code,
        map: result.js.map,
      };
    } catch (e: any) {
      this.error(`svelte-lite-test: failed to compile ${id}: ${e.message}`);
    }
  },
};

export default defineConfig({
  plugins: [svelteLite],
  resolve: {
    // Use browser exports (index-client.js) for packages that distinguish
    // server vs browser (e.g. svelte's mount/unmount are browser-only).
    conditions: ['browser', 'import', 'module', 'default'],
    alias: {
      '$lib': resolve(__dirname, './src/lib'),
      '$app': resolve(__dirname, './src/app-mocks'),
    },
  },
  test: {
    include: ['src/**/*.{test,spec}.{js,ts}'],
    environment: 'jsdom',
    globals: true,
    setupFiles: ['./vitest-setup.ts'],
  },
});

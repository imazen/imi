import { createHash } from 'node:crypto';
import { dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { defineConfig, devices } from '@playwright/test';

// Three static servers, same tree (tools/imij-wasm/web), three header profiles —
// see scripts/serve.mjs for exactly what each sends. Ports in the required
// 3000-3999 range. `webServer` is an array: Playwright starts and health-checks
// all three before any test.
//
// The port base is derived deterministically from this checkout's absolute
// path (3000..3997, three consecutive ports), so sibling jj workspaces get
// disjoint servers; IMIJ_WEB_PORT_BASE shifts all three.
const webDir = dirname(fileURLToPath(import.meta.url)); // <checkout>/tools/imij-wasm/web
const derived = 3000 + (createHash('sha256').update(webDir).digest().readUInt16LE(0) % 997);
const PORT_BASE = Number(process.env.IMIJ_WEB_PORT_BASE ?? derived);
export const PORTS = { plain: PORT_BASE, isolated: PORT_BASE + 1, strict: PORT_BASE + 2 };

// The timing harness only runs when asked for (IMIJ_WASM_BENCH=1) and only on
// a host the caller has checked (see the `web-bench` just recipe); it is
// chromium-only (the wasm bytes are identical across engines). Parity and
// variant tests always run.
const benchIgnore = process.env.IMIJ_WASM_BENCH ? [] : ['**/benchmark.spec.ts'];
// This supplement requires an explicitly generated current-grammar set.
// The caller selects it through `just web-current-test`; missing files fail.
const v4Ignore = [...(process.env.IMIJ_V4 ? [] : ['**/v4-integration.spec.ts']), ...(process.env.IMIJ_V4_VIEWER ? [] : ['**/v4-viewer.spec.ts'])];
const currentIgnore = process.env.IMIJ_WEB_CURRENT ? [] : ['**/current-grammar.spec.ts'];

export default defineConfig({
  testDir: 'tests',
  timeout: 120_000,
  fullyParallel: false, // decode workers share this box's CPU with other agents; avoid pileup
  workers: 1,
  retries: 0,
  reporter: 'list',
  webServer: Object.entries(PORTS).map(([profile, port]) => ({
    command: `node scripts/serve.mjs ${profile} ${port}`,
    port,
    reuseExistingServer: !process.env.CI && !process.env.IMIJ_V4,
    timeout: 20_000,
  })),
  projects: [
    { name: 'chromium', use: { ...devices['Desktop Chrome'] }, testIgnore: [...benchIgnore, ...currentIgnore, ...v4Ignore] },
    { name: 'firefox', use: { ...devices['Desktop Firefox'] }, testIgnore: ['**/benchmark.spec.ts', ...currentIgnore, ...v4Ignore] },
    { name: 'webkit', use: { ...devices['Desktop Safari'] }, testIgnore: ['**/benchmark.spec.ts', ...currentIgnore, ...v4Ignore] },
  ],
});

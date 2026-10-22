// Shared by the browser checks: a headless Chromium against a test-mode gateway, and a record of everything that
// went wrong. See README.md.
import { createRequire } from 'node:module';
import path from 'node:path';

const require = createRequire(process.env.PLAYWRIGHT_DIR ? path.join(process.env.PLAYWRIGHT_DIR, 'x.js') : import.meta.url);
const { chromium } = require('playwright');

export const BASE = process.env.AGAVE_BASE || 'http://127.0.0.1:4171';
export const problems = [];
export const external = [];

export async function launch() {
  const opts = process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {};
  return chromium.launch(opts);
}

// A browser context that skips the first-launch setup sheet, and a page whose console errors, failed requests,
// HTTP errors and requests that leave the studio are all recorded.
export async function studioPage(browser, tag, viewport = { width: 1440, height: 900 }) {
  const ctx = await browser.newContext({ viewport });
  await ctx.addInitScript(() => localStorage.setItem('agave.setup.v1', 'done'));
  const page = await ctx.newPage();
  page.on('console', (m) => { if (m.type() === 'error' || m.type() === 'warning') problems.push(`${tag} console.${m.type()}: ${m.text()}`); });
  page.on('pageerror', (e) => problems.push(`${tag} pageerror: ${e.message}`));
  page.on('request', (r) => {
    const u = r.url();
    if (!u.startsWith(BASE + '/') && !u.startsWith('data:') && !u.startsWith('blob:')) external.push(`${tag} ${u}`);
  });
  page.on('response', (r) => { if (r.status() >= 400) problems.push(`${tag} HTTP ${r.status()} ${r.request().method()} ${r.url()}`); });
  page.on('requestfailed', (r) => problems.push(`${tag} requestfailed: ${r.url()} ${r.failure() && r.failure().errorText}`));
  return page;
}

// The page's own fetch, so it carries the launch cookie.
export function api(page, url, body) {
  return page.evaluate(async ([u, b]) => {
    const init = b === undefined ? {} : { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(b) };
    const r = await fetch(u, init);
    return { status: r.status, data: await r.json().catch(() => null) };
  }, [url, body]);
}

export async function openBoardView(page) {
  await page.goto(`${BASE}/app/#board`);
  await page.waitForSelector('.excalidraw .excalidraw__canvas', { timeout: 30000 });
  await page.waitForTimeout(500);
}

export async function deleteAllBoards(page) {
  const r = await api(page, '/api/boards');
  for (const b of r.data.boards) await api(page, '/api/boards/delete', { name: b.name });
}

export function check(ok, what) {
  if (!ok) problems.push(`FAILED: ${what}`);
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${what}`);
}

// Prints the result and exits non-zero on any problem.
export function finish() {
  console.log('requests that left the studio:', external.length ? '\n  ' + external.join('\n  ') : 'none');
  console.log('problems:', problems.length ? '\n  ' + problems.join('\n  ') : 'none');
  process.exit(external.length || problems.length ? 1 : 0);
}

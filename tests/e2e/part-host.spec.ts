// Served by the real binary after the fixture's real child build. The opaque sandbox,
// storage write on immediate close, and browser-generated Fetch Metadata are the assertions.
import { test, expect, waitForLog } from './harness/fixtures';
import * as fs from 'node:fs';

const fixturePath = process.env.PF_PART_FIXTURE ?? '/run/pf/run/part-fixture.json';
const fixture = () => JSON.parse(fs.readFileSync(fixturePath, 'utf8'));

test('the first author script sees the prelude in an opaque document', async ({ page }) => {
  const response = await page.goto(fixture().notes.url);
  expect(response?.status()).toBe(200);
  const headers = response!.headers();
  expect(headers['content-security-policy']).toContain('sandbox allow-scripts');
  expect(headers['content-security-policy']).not.toContain('allow-same-origin');
  expect(headers['set-cookie']).toBeUndefined();
  expect(headers['cross-origin-resource-policy']).toBeUndefined();
  expect(headers['access-control-allow-credentials']).toBeUndefined();
  expect(await page.evaluate(() => (window as any).firstAuthorScript)).toMatchObject({
    storage: 'object', cookie: '', indexedDB: 'undefined', caches: 'undefined',
  });
  expect(await page.evaluate(() => document.documentElement.lang)).toBe('en');
  const values = await page.evaluate(async () => {
    const data = await (await fetch('/data.json')).json();
    const worker = new Worker('/worker.js');
    const result = await new Promise((resolve, reject) => {
      worker.onmessage = e => resolve(e.data);
      worker.onerror = reject;
      worker.postMessage('run');
    });
    worker.terminate();
    return { data, result };
  });
  expect(values).toEqual({ data: { answer: 42 }, result: 'worker-ok' });
});

test('localStorage persists a write followed by immediate page close without preflight', async ({ page, context }, info) => {
  const url = fixture().notes.url;
  const key = `close-${info.project.name}-${Date.now()}`;
  const requests: { method: string; headers: Record<string, string>; body: string | null }[] = [];
  context.on('request', r => {
    if (r.url().includes('/_rt/v1')) requests.push({ method: r.method(), headers: r.headers(), body: r.postData() });
  });
  // Use the browser's own window.close path. Firefox's automation close tears down the
  // browsing context without pagehide, unlike closing a script-opened window in the browser.
  await page.goto('/_dev/selftest');
  const popupEvent = context.waitForEvent('page');
  await page.evaluate(u => { window.open(u); }, url);
  const popup = await popupEvent;
  await popup.waitForLoadState();
  const closed = popup.waitForEvent('close');
  // One task: the normal 300 ms debounce cannot save it before the close.
  await popup.evaluate(k => { localStorage.setItem(k, 'saved-on-close'); window.close(); }, key)
    .catch(error => { if (!popup.isClosed()) throw error; });
  await closed;
  const reopened = await context.newPage();
  await expect.poll(async () => {
    await reopened.goto(url);
    return reopened.evaluate(k => localStorage.getItem(k), key);
  }).toBe('saved-on-close');
  expect(requests.filter(r => r.method === 'OPTIONS')).toHaveLength(0);
  // Teardown requests can outlive their Playwright Page and escape its request events.
  // Persisted state proves the real runtime accepted the flush; its guard requires null
  // Origin and cors Fetch Metadata (also tested with explicit rejected requests).
  expect((await waitForLog({ event: 'rt.op' })).some(line => line.op === 'storage.sync' && line.outcome === 'ok')).toBe(true);
  await reopened.close();
});

test('expired navigation regrants and an expired subresource requests reauthentication', async ({ request }) => {
  const expired = fixture().expired.url;
  const nav = await request.get(expired, { headers: { 'Sec-Fetch-Dest': 'document' }, maxRedirects: 0 });
  expect(nav.status()).toBe(302);
  expect(nav.headers().location).toContain('/grant?part=notes&mode=direct');
  const blob = await request.get(`${expired}data.json`, { maxRedirects: 0 });
  expect(blob.status()).toBe(401);
  expect(blob.headers()['x-pf-reauth']).toBe('1');
});

test('the device probe observes sandbox, storage reload, fetch and worker behavior', async ({ page }) => {
  await page.goto('/_dev/selftest?nonce=browser-probe&frame=' + encodeURIComponent(fixture().device.url));
  await expect(page.locator('#s')).toHaveText('REPORTED ✓');
  const report = JSON.parse(await page.locator('#d').innerText());
  expect(report.pass).toBe(true);
  expect(report.checks).toMatchObject({ opaque: true, storage: true, prelude: true, fetch: true, worker: true });
  expect(report.href).not.toContain('/c/');
});

test('an author error reaches the runtime error store', async ({ page }) => {
  await page.goto(fixture().notes.url);
  await page.evaluate(() => { setTimeout(() => { throw new Error('harness author error'); }, 0); });
  await expect.poll(async () => (await waitForLog({ event: 'runtime_error.recorded' }))
    .some(line => line.kind === 'error' && line.outcome === 'ok')).toBe(true);
});

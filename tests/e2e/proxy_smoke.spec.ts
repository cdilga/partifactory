// The shipped Caddyfile through the harness (pf-kurh): routing by Host, a 60 s SSE stream held
// open, 8 MiB bodies accepted and 8 MiB + 1 rejected, and :9090 never reachable through the
// proxy. The maintenance page is the `@origin-down` test, which ci/e2e.sh runs after stopping
// partifactory. API-level checks: they run once, on chromium-desktop.
import { test, expect } from './harness/fixtures';
import * as https from 'node:https';

const MiB = 1024 * 1024;

test.describe('proxy smoke', () => {
  test.skip(({ browserName, isMobile }) => browserName !== 'chromium' || !!isMobile, 'API-level: one project');

  test('routes by Host', async ({ request }) => {
    const ws = await request.get('https://parts.pf.test/_dev/whoami');
    expect(await ws.json()).toMatchObject({ host_class: 'workspace' });
    const part = await request.get('https://demo-part.pf.test/_dev/whoami');
    expect(await part.json()).toMatchObject({ host_class: 'part', part: 'demo' });
    // The workspace listener does not serve /metrics; nothing routes to :9090.
    const m = await request.get('https://parts.pf.test/metrics');
    expect(m.status()).toBe(404);
  });

  test('accepts 8 MiB and rejects 8 MiB + 1', async ({ request }) => {
    const ok = await request.post('https://parts.pf.test/_dev/echo', {
      data: Buffer.alloc(8 * MiB, 0x61),
      headers: { 'content-type': 'application/octet-stream' },
    });
    expect(ok.status()).toBe(200);
    expect(await ok.json()).toEqual({ bytes: 8 * MiB });
    const tooBig = await request.post('https://parts.pf.test/_dev/echo', {
      data: Buffer.alloc(8 * MiB + 1, 0x61),
      headers: { 'content-type': 'application/octet-stream' },
    });
    expect(tooBig.status()).toBe(413);
  });

  test('holds an SSE stream for 60 s', async () => {
    test.setTimeout(120_000);
    const started = Date.now();
    const events: string[] = await new Promise((resolve, reject) => {
      const seen: string[] = [];
      const req = https.get(
        'https://parts.pf.test/_dev/sse?seconds=65',
        { headers: { accept: 'text/event-stream' } },
        (res) => {
          if (res.statusCode !== 200) reject(new Error(`status ${res.statusCode}`));
          res.setEncoding('utf8');
          let buf = '';
          res.on('data', (chunk: string) => {
            buf += chunk;
            for (const m of buf.matchAll(/^event: (\w+)$/gm)) seen.push(m[1]);
            buf = buf.slice(buf.lastIndexOf('\n') + 1);
          });
          res.on('end', () => resolve(seen));
          res.on('error', reject);
        },
      );
      req.on('error', reject);
    });
    const held = (Date.now() - started) / 1000;
    expect(held).toBeGreaterThanOrEqual(60);
    expect(events.filter((e) => e === 'tick').length).toBeGreaterThanOrEqual(12);
    expect(events).toContain('done');
  });
});

test('@origin-down shows the maintenance page', async ({ page }) => {
  test.skip(process.env.PF_ORIGIN_DOWN !== '1', 'run by ci/e2e.sh with partifactory stopped');
  const resp = await page.goto('https://parts.pf.test/');
  expect(resp?.status()).toBe(503);
  await expect(page.locator('h1')).toHaveText('503 · DOWN FOR MAINTENANCE');
  const part = await page.goto('https://notes-part.pf.test/');
  expect(part?.status()).toBe(503);
});

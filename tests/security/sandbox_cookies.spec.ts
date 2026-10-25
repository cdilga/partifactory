// Real browser-generated requests to a sibling service, through the deployed ingress.
import { test, expect } from '../e2e/harness/fixtures';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { signIn, nativePasskey } from '../e2e/lib/public-auth';

test('SEC sandbox cookie writes are inert and Lax sibling canaries receive no cookies', async ({ page, context }, info) => {
  const fixture = JSON.parse(fs.readFileSync(process.env.PF_PART_FIXTURE ?? '/run/pf/run/part-fixture.json', 'utf8'));
  const content = new URL(fixture.notes.url);
  const own = content.origin;
  const blob = process.env.PF_SECURITY_CANARY_ORIGIN ?? `https://blobs-part.${content.hostname.split('.').slice(1).join('.')}`;
  const nonce = `sec-${info.project.name}-${Date.now()}`;
  const sibling = `https://dex.${content.hostname.split('.').slice(1).join('.')}`;
  await signIn(page);
  const sessionCookies = await context.cookies('https://parts.pf.test');
  expect(sessionCookies.some(c => c.name === '__Host-pf_session' && c.secure && c.sameSite === 'Lax')).toBe(true);
  const workspace = process.env.PF_BASE_URL ?? 'https://parts.pf.test';
  await context.addCookies([
    { name: 'sec_lax_probe', value: nonce, url: sibling, secure: true, sameSite: 'Lax' },
    { name: 'sec_workspace_lax', value: nonce, url: workspace, secure: true, sameSite: 'Lax' },
  ]);
  // This control proves the canary actually observes cookies in a non-sandboxed context.
  const control = await context.request.get(`${sibling}/__sec_canary?nonce=control-${nonce}`);
  expect(control.status()).toBe(200);
  expect(await control.json()).toMatchObject({ cookie_present: true, method: 'GET' });

  const cap = content.pathname.match(/^\/c\/([^/]+)\//)![1];
  await page.goto(`${own}/#pf=${cap}`);
  await expect(page.locator('#pf-frame')).toBeVisible();
  await expect.poll(() => page.frames().some(f => f.url().includes('/c/'))).toBe(true);
  const frame = page.frames().find(f => f.url().includes('/c/'))!;
  await frame.waitForLoadState();
  expect(page.url()).toBe(`${own}/`);
  const result = await frame.evaluate(async ({ blob, nonce, workspace, sibling }) => {
    const write = 'sec_cookie_write=' + nonce + '; Secure; SameSite=None; Path=/';
    try { document.cookie = write; } catch { /* opaque origins may throw */ }
    try { await (window as any).cookieStore?.set({ name: 'sec_cookie_store', value: nonce, secure: true, sameSite: 'none' }); } catch { /* unavailable or rejected */ }
    const key = 'sec-storage-' + nonce;
    localStorage.setItem(key, 'shim-value');
    let parentDocumentBlocked = false, parentStorageBlocked = false;
    try { void parent.document.body; } catch { parentDocumentBlocked = true; }
    try { void parent.sessionStorage.length; } catch { parentStorageBlocked = true; }
    const reports = [];
    for (const method of ['GET', 'POST']) {
      const response = await fetch(`${blob}/__sec_canary?nonce=${nonce}`, {
        method, credentials: 'include', ...(method === 'POST' ? { body: 'canary' } : {}),
      });
      reports.push({ status: response.status, ...(await response.json()) });
    }
    const violations: string[] = [];
    const violation = (event: SecurityPolicyViolationEvent) => {
      if (['connect-src', 'img-src'].includes(event.effectiveDirective)) violations.push(event.blockedURI);
    };
    document.addEventListener('securitypolicyviolation', violation);
    const deniedRequests = [];
    for (const target of [workspace, sibling]) {
      for (const method of ['GET', 'POST']) {
        try {
          await fetch(`${target}/__sec_canary?nonce=${nonce}`, { method, credentials: 'include' });
          deniedRequests.push('transmitted');
        } catch { deniedRequests.push('blocked'); }
      }
    }
    // Use native elements as well as fetch: a JavaScript credential wrapper would not prove
    // the security property. Images are governed by img-src rather than connect-src.
    for (const target of [workspace, sibling]) {
      const image = document.createElement('img');
      const result = await new Promise(resolve => {
        image.onload = () => resolve('transmitted');
        image.onerror = () => resolve('blocked');
        image.src = `${target}/__sec_canary?nonce=${nonce}&image=1`;
        document.body.appendChild(image);
      });
      deniedRequests.push(result);
      image.remove();
    }
    // CSP violation delivery is queued after fetch rejection.
    await new Promise(resolve => setTimeout(resolve, 100));
    document.removeEventListener('securitypolicyviolation', violation);
    let cookie = '';
    try { cookie = document.cookie; } catch { /* engines may reject even the shim */ }
    let nativeCookieBlocked = false;
    try { Object.getOwnPropertyDescriptor(Document.prototype, 'cookie')!.get!.call(document); }
    catch (error) { nativeCookieBlocked = (error as DOMException).name === 'SecurityError'; }
    return { cookie, storage: localStorage.getItem(key), parentDocumentBlocked, parentStorageBlocked,
      origin: window.origin, reports, deniedRequests, violations, nativeCookieBlocked };
  }, { blob, nonce, workspace, sibling });
  // PLAN §2 deliberately supplies an inert in-memory cookie string for compatibility.
  // Its readback is not a browser cookie; the native accessor and real jar remain unavailable.
  expect(result).toMatchObject({ cookie: `sec_cookie_write=${nonce}`, nativeCookieBlocked: true,
    storage: 'shim-value', origin: 'null', parentDocumentBlocked: true, parentStorageBlocked: true });
  expect(result.reports).toEqual([
    { status: 200, method: 'GET', cookie_present: false, origin: 'null' },
    { status: 200, method: 'POST', cookie_present: false, origin: 'null' },
  ]);
  expect(result.deniedRequests).toEqual(Array(6).fill('blocked'));
  expect(result.violations.filter(url => url.startsWith(workspace))).toHaveLength(3);
  expect(result.violations.filter(url => url.startsWith(sibling))).toHaveLength(3);
  const reached = await context.request.get(`${sibling}/__sec_canary/observations?nonce=${nonce}`);
  expect(await reached.json()).toEqual({ count: 0, methods: [] });
  expect((await context.cookies(own)).some(c => ['sec_cookie_write', 'sec_cookie_store'].includes(c.name))).toBe(false);
  await frame.goto(fixture.notes.url);
  expect(await frame.evaluate(() => document.cookie)).toBe('');
  console.log(JSON.stringify({ event: 'sec.check', id: 'SEC-003', lane: info.project.name, outcome: 'pass' }));
});

test('SEC device consent warns for stale different-network requests and limits manual lookup', async ({ page }, info) => {
  test.skip(info.project.name !== 'chromium-desktop', 'One real source-network comparison lane');
  test.skip(!process.env.PF_SECURITY_DEVICE_FILE, 'Dedicated live device fixture from external raw client');
  test.setTimeout(180_000);
  const device = JSON.parse(fs.readFileSync(process.env.PF_SECURITY_DEVICE_FILE!, 'utf8'));
  const previous = process.env.PF_BROWSER_AUTH_DIR;
  process.env.PF_BROWSER_AUTH_DIR = path.join(info.outputDir, 'private-auth');
  try { await signIn(page); } finally {
    if (previous === undefined) delete process.env.PF_BROWSER_AUTH_DIR;
    else process.env.PF_BROWSER_AUTH_DIR = previous;
  }
  // A live elapsed-time test; no test clock or fabricated network headers.
  const remaining = 122_000 - (Date.now() - device.issued_at_ms);
  if (remaining > 0) await page.waitForTimeout(remaining);
  await page.goto('/device?code=' + encodeURIComponent(device.user_code));
  await page.getByRole('button', { name: 'Continue', exact: true }).click();
  await expect(page.locator('#network-warning')).toBeVisible();
  await expect(page.locator('#network-warning')).toContainText('different network');
  await expect(page.locator('#warning')).toBeVisible();
  await expect(page.locator('#origin')).toContainText('Different network');
  const lookups = await page.evaluate(async code => {
    const results = [];
    for (let i = 0; i < 5; i++) {
      const response = await fetch('/api/agents/consent?code=' + encodeURIComponent(code));
      const body = await response.json();
      results.push({ status: response.status, error: body.error ?? null });
    }
    return results;
  }, device.user_code);
  expect(lookups.slice(0, 4)).toEqual(Array(4).fill({ status: 200, error: null }));
  expect(lookups[4]).toEqual({ status: 400, error: 'slow_down' });
  await page.screenshot({ path: info.outputPath('device-network-stale-warnings.png') });
  console.log(JSON.stringify({ event: 'sec.check', id: 'SEC-025', lane: info.project.name, outcome: 'pass' }));
});

test('SEC real guest non-data wall view-only writes forged and replayed guest ids', async ({ page, context, browser }, info) => {
  test.skip(info.project.name !== 'chromium-desktop', 'Native virtual-authenticator admin lane');
  test.setTimeout(90_000);
  await signIn(page);
  await nativePasskey(page, context);
  const fixture = JSON.parse(fs.readFileSync(process.env.PF_PART_FIXTURE ?? '/run/pf/run/part-fixture.json', 'utf8'));
  const own = new URL(fixture.notes.url).origin;
  const name = new URL(own).hostname.split('.')[0].replace(/-part$/, '');
  const headers = { Origin: own, 'Sec-Fetch-Site': 'same-site' };
  const links: string[] = [];
  const guestContext = await browser.newContext();
  try {
    const urls = [];
    for (const mode of ['contribute', 'view']) {
      const response = await context.request.post(`/api/chrome/v1/${name}/links`, { headers, data: { name, mode } });
      expect(response.status()).toBe(200);
      const body = await response.json();
      links.push(body.link.id); urls.push(body.url);
    }
    const guest = await guestContext.newPage();
    await guest.goto(urls[0]);
    await expect(guest.frameLocator('#pf-frame').getByRole('heading', { name: 'Serving fixture' })).toBeVisible();
    for (const label of ['Share', 'History', 'Source']) {
      await expect(guest.getByRole('button', { name: label, exact: true })).toHaveCount(0);
      await expect(guest.getByRole('link', { name: label, exact: true })).toHaveCount(0);
    }
    const runtime = guest.frames().find(f => f.url().includes('/c/'))!;
    const firstCap = new URL(runtime.url()).pathname.split('/')[2];
    const wall = await runtime.evaluate(async () => {
      const ops = ['comments.list', 'comments.create', 'artifact.publish', 'assets.put', 'user', 'errors.report', 'refresh', 'history', 'source', 'share'];
      const results = [];
      for (const op of ops) {
        const response = await fetch('/_rt/v1', { method: 'POST', headers: { 'Content-Type': 'text/plain' }, body: JSON.stringify({ op, args: {} }) });
        results.push(response.status);
      }
      results.push((await fetch('/api/private')).status);
      results.push((await fetch('/mcp')).status);
      return results;
    });
    expect(wall).toEqual(Array(12).fill(403));
    await guest.reload();
    await expect(guest.frameLocator('#pf-frame').getByRole('heading', { name: 'Serving fixture' })).toBeVisible();
    const secondCap = new URL(guest.frames().find(f => f.url().includes('/c/'))!.url()).pathname.split('/')[2];
    const guestBytes = (cap: string) => {
      const payload = Buffer.from(cap.split('.')[2], 'base64url');
      const key = payload.indexOf(Buffer.from([0x61, 0x67])); // canonical CBOR text key "g"
      expect(key).toBeGreaterThan(0);
      const bytes = payload.indexOf(0x50, key + 2); // 16-byte compact guest ULID
      expect(bytes).toBeGreaterThan(key);
      return { payload, offset: bytes + 1 };
    };
    const oldGuest = guestBytes(firstCap);
    for (const replay of [false, true]) {
      const segments = secondCap.split('.');
      const { payload, offset } = guestBytes(secondCap);
      if (replay) oldGuest.payload.copy(payload, offset, oldGuest.offset, oldGuest.offset + 16);
      else payload[offset] ^= 1;
      segments[2] = payload.toString('base64url');
      const forged = await guestContext.request.post(`${own}/c/${segments.join('.')}/_rt/v1`, {
        headers: { Origin: 'null', 'Content-Type': 'text/plain' }, data: JSON.stringify({ op: 'storage.list', args: { shared: true } }),
      });
      expect(forged.status()).toBe(404);
    }
    await guest.goto(urls[1]);
    await expect(guest.frameLocator('#pf-frame').getByRole('heading', { name: 'Serving fixture' })).toBeVisible();
    const view = guest.frames().find(f => f.url().includes('/c/'))!;
    expect(await view.evaluate(async () => (await fetch('/_rt/v1', { method: 'POST', headers: { 'Content-Type': 'text/plain' },
      body: JSON.stringify({ op: 'storage.set', args: { key: 'SEC_VIEW_MUST_NOT_WRITE', value: 'denied', shared: true } }) })).status)).toBe(403);
    for (const endpoint of ['/api/v1/part_get', '/mcp']) {
      const response = await guestContext.request.post('https://parts.pf.test' + endpoint, {
        headers: { Accept: 'application/json, text/event-stream' }, data: {},
      });
      expect(response.status()).toBe(401);
    }
    for (const id of ['SEC-029', 'SEC-030', 'SEC-032']) console.log(JSON.stringify({ event: 'sec.check', id, outcome: 'pass' }));
  } finally {
    await guestContext.close();
    for (const id of links) {
      const response = await context.request.post(`/api/chrome/v1/${name}/links/${id}`, { headers, data: { action: 'revoke' } });
      expect(response.status()).toBe(200);
    }
  }
});

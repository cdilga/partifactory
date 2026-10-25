// The harness self-test (pf-kurh): every project reaches https://parts.pf.test over a valid
// TLS chain from the run's CA, gets a __Host- cookie, resolves a part host, the Chromium
// projects can attach a virtual authenticator, and the click-budget helper bites.
import { test, expect, clickBudget, waitForLog } from './fixtures';

test('TLS chain is valid and the workspace answers', async ({ page, browser }, testInfo) => {
  const version = browser.version();
  testInfo.annotations.push({ type: 'browser', description: version });
  console.log(`${testInfo.project.name}: ${version}`);
  const resp = await page.goto('/healthz');
  expect(resp?.status()).toBe(200);
  expect(await resp?.json()).toMatchObject({ status: 'ok' });
  const sec = await resp?.securityDetails();
  if (sec?.issuer) {
    // Firefox and Chromium report the chain; WebKit may not. Either way the load succeeded with
    // no certificate-error override, which is the check.
    expect(sec.issuer).toContain('partifactory harness CA');
    expect(sec.subjectName ?? '').toContain('pf.test');
  }
  const rid = resp?.headers()['x-request-id'];
  expect(rid).toMatch(/^[0-9a-f-]{8,}$/);
  // The access line names the request id and a client address (the browser's, via Caddy).
  const lines = await waitForLog({ event: 'http.request', request_id: rid });
  expect(lines.length).toBe(1);
  expect(lines[0].client_ip).toBeTruthy();
  expect(lines[0].route_class).toBe('ops');
});

test('__Host- cookie is set over TLS', async ({ page, context }) => {
  const resp = await page.goto('/_dev/cookie');
  expect(resp?.status()).toBe(200);
  const cookies = await context.cookies('https://parts.pf.test');
  const c = cookies.find((c) => c.name === '__Host-pf_harness');
  expect(c, 'the __Host- cookie').toBeTruthy();
  expect(c?.secure).toBe(true);
  expect(c?.path).toBe('/');
});

test('a <name>-part.pf.test host resolves and reaches the content listener', async ({ page }) => {
  const resp = await page.goto('https://notes-part.pf.test/_dev/whoami');
  expect(resp?.status()).toBe(200);
  expect(await resp?.json()).toMatchObject({ host_class: 'part', part: 'notes' });
  const blob = await page.goto('https://blobs-part.pf.test/nothing');
  expect(blob?.status()).toBe(404);
  const other = await page.goto('https://forge.pf.test/');
  expect(other?.status()).toBe(404);
});

test('error pages carry the request id', async ({ page }) => {
  const resp = await page.goto('/no-such-page');
  expect(resp?.status()).toBe(404);
  const rid = resp?.headers()['x-request-id'];
  await expect(page.locator('#request-id')).toHaveText(rid ?? 'missing');
});

test('a virtual authenticator can be attached (Chromium projects)', async ({ page, browserName }) => {
  test.skip(browserName !== 'chromium', 'virtual authenticators are a Chromium CDP feature');
  await page.goto('/healthz');
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('WebAuthn.enable');
  const { authenticatorId } = await cdp.send('WebAuthn.addVirtualAuthenticator', {
    options: {
      protocol: 'ctap2',
      transport: 'internal',
      hasResidentKey: true,
      hasUserVerification: true,
      isUserVerified: true,
    },
  });
  expect(authenticatorId).toBeTruthy();
  const isSecure = await page.evaluate(() => window.isSecureContext && !!window.PublicKeyCredential);
  expect(isSecure).toBe(true);
});

test('the click-budget helper fails an over-budget path', async ({ page }) => {
  await page.setContent('<button id="b">go</button>');
  const within = clickBudget(page, 2, 'within');
  await within.click(page.locator('#b'));
  await within.click(page.locator('#b'));
  expect(() => within.check()).not.toThrow();
  const over = clickBudget(page, 1, 'over');
  await over.click(page.locator('#b'));
  await over.click(page.locator('#b'));
  expect(() => over.check()).toThrow(/click budget exceeded/);
});

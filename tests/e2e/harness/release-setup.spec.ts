// Production-image setup: real Dex, browser consent and normal API operations only.
import { test, expect } from './fixtures';
import { signIn, nativePasskey } from '../lib/public-auth';
import * as fs from 'node:fs';
import * as path from 'node:path';

test('release compose setup uses real OIDC and scoped agent operations', async ({ page, context, request, browser }, info) => {
  test.skip(process.env.PF_HARNESS_RELEASE !== '1', 'Production-image lane only');
  test.skip(info.project.name !== 'chromium-desktop', 'One real setup session');
  test.setTimeout(120_000);
  const base=process.env.PF_BASE_URL??'https://parts.pf.test';
  const apex=new URL(base).hostname.split('.').slice(1).join('.');
  const run = path.join('/repo/target/e2e-runs', process.env.PF_E2E_RUN ?? 'local');
  const privateDir = path.join(run, 'release-private');
  fs.mkdirSync(privateDir, {recursive: true, mode: 0o700});
  await signIn(page);
  const scopes = ['parts:read', 'parts:write', 'parts:delete', 'data:read', 'data:write'];
  const device = await (await request.post('/oauth/device_authorization', {form: {
    client_id: 'part-cli', agent_name: 'Release fixture setup', scope: scopes.join(' '),
  }})).json();
  await page.goto(device.verification_uri_complete);
  await page.getByRole('button', {name: 'Continue', exact: true}).click();
  await page.getByRole('button', {name: 'Allow', exact: true}).click();
  await expect(page.getByRole('status')).toHaveText('Agent connected. You can return to your terminal.');
  const grant = await request.post('/oauth/token', {form: {
    client_id: 'part-cli', grant_type: 'urn:ietf:params:oauth:grant-type:device_code', device_code: device.device_code,
  }});
  expect(grant.status()).toBe(200);
  const token = (await grant.json()).access_token;
  fs.writeFileSync(path.join(privateDir, 'agent-token'), token, {mode: 0o600});
  fs.writeFileSync(path.join(privateDir, 'redaction-canaries.json'), JSON.stringify({token}), {mode: 0o600});
  const headers = {Authorization: `Bearer ${token}`};
  const content = '<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Serving fixture</title></head><body><h1>Serving fixture</h1><div id="result">ready</div></body></html>';
  const fixtures: Record<string, {url: string}> = {};
  for (const name of ['notes', 'pixel']) {
    const created = await request.post('/api/v1/part_create', {headers, data: {
      name, title: 'Serving fixture', kind: 'html', request_id: `release-fixture-${name}`,
      files: {'index.html': content, 'data.json': '{"answer":42}', 'worker.js': 'self.onmessage=function(){postMessage("worker-ok")}'},
    }});
    expect(created.status()).toBe(200);
    await expect.poll(async () => {
      const response = await request.post('/api/v1/part_get', {headers, data: {name}});
      expect(response.status()).toBe(200);
      return (await response.json()).build?.status;
    }, {timeout: 45_000}).toBe('ok');
    const direct = await context.request.get(`/grant?part=${name}&mode=direct`, {maxRedirects: 0});
    expect([302, 303]).toContain(direct.status());
    const url = direct.headers().location;
    expect(new URL(url).pathname.startsWith('/c/')).toBe(true);
    fixtures[name] = {url};
  }
  fs.writeFileSync(path.join(run, 'part-fixture.json'), JSON.stringify(fixtures), {mode: 0o600});
  await context.storageState({path: path.join(privateDir, 'session.json')});
  fs.chmodSync(path.join(privateDir, 'session.json'), 0o600);
  // The simulator cannot provide a virtual passkey. Create its view-only link through the
  // same actual passkey-guarded browser API used by the Share UI, never by writing SQLite.
  await page.goto(base+'/');
  await nativePasskey(page, context, path.join(privateDir, 'authenticator.json'));
  const link = await context.request.post('/api/chrome/v1/notes/links', {
    headers: {Origin: `https://notes-part.${apex}`, 'Sec-Fetch-Site': 'same-site'}, data: {name: 'notes', mode: 'view'},
  });
  expect(link.status()).toBe(200);
  const linkBody = await link.json();
  const deviceDir = path.join(run, 'device-pass');
  fs.mkdirSync(deviceDir, {recursive: true, mode: 0o700});
  fs.writeFileSync(path.join(deviceDir, 'guest-link.json'), JSON.stringify({url: linkBody.url}), {mode: 0o600});
  // Obtain the actual guest capability through the signed-out wrapper/go flow.
  // An authenticated owner's direct capability cannot prove the guest wall.
  const guestContext = await browser.newContext();
  try {
    const guest = await guestContext.newPage();
    await guest.goto(linkBody.url);
    await expect(guest.frameLocator('#pf-frame').getByRole('heading', {name: 'Serving fixture'})).toBeVisible();
    const guestContent = guest.frames().find(frame => new URL(frame.url()).pathname.startsWith('/c/'))?.url();
    expect(guestContent).toBeTruthy();
    fs.writeFileSync(path.join(privateDir, 'guest-content.json'), JSON.stringify({url: guestContent}), {mode: 0o600});
  } finally {
    await guestContext.close();
  }
  console.log(JSON.stringify({event: 'release.fixture', outcome: 'pass', auth: 'real_oidc_device_consent', parts: Object.keys(fixtures)}));
});

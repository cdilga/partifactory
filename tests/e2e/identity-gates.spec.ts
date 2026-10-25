// Isolated provider/directory mutations run after the ordinary e2e suite, never in parallel
// with it. identity-gates.py drives the real directory API and local dev clock at the marker.
import { test, expect, serverLog } from './harness/fixtures';
import { signIn } from './lib/public-auth';
import * as fs from 'node:fs';

const phase = process.env.PF_IDENTITY_PHASE;
const run = '/repo/target/e2e-runs/' + process.env.PF_E2E_RUN;
test.skip(!phase, 'isolated real identity lane');
test.use({ trace: 'off', screenshot: 'off' }); // recovery credentials stay out of artifacts

test('second provider, real directory refresh, and one-use console recovery', async ({ page }, info) => {
  test.setTimeout(90_000);
  process.env.PF_BROWSER_AUTH_DIR = `/tmp/identity-${phase}-${info.project.name}`;
  if (phase === 'recovery') {
    const text = fs.readFileSync(run + '/identity-private/recovery.txt', 'utf8');
    const code = text.match(/#code=([a-f0-9]{64})/)?.[1];
    expect(!!code).toBe(true);
    const response = await page.request.post('/admin/recover', {
      headers: { Origin: 'https://parts.pf.test' }, data: { code },
    });
    expect(response.status()).toBe(200);
    const session = await (await page.request.get('/api/session')).json();
    expect(session.admin).toBe(true);
    const reused = await page.request.post('/admin/recover', {
      headers: { Origin: 'https://parts.pf.test' }, data: { code },
    });
    expect(reused.status()).toBe(403);
    expect(JSON.stringify(await serverLog())).not.toContain(code!);
    expect((await serverLog({ event: 'admin.break_glass.used' })).length).toBe(1);
    return;
  }
  if (phase === 'group') {
    await page.goto('/signin?return=/');
    await page.locator('input[name=login]').fill('admin');
    await page.locator('input[name=password]').fill('test-directory-password');
    await page.locator('button[type=submit]').click();
    await page.waitForURL('https://parts.pf.test/**');
  } else {
    await signIn(page);
  }
  const session = await (await page.request.get('/api/session')).json();
  expect(session.user).toMatch(/^u_/);
  if (phase === 'second') expect(session.admin).toBe(true);
  const cookie = (await page.context().cookies()).find(c => c.name === '__Host-pf_session');
  expect(cookie).toMatchObject({ httpOnly: true, secure: true, sameSite: 'Lax', path: '/' });
  expect((await serverLog({ event: 'session.created' })).length).toBeGreaterThan(0);
  if (phase === 'second') return;

  const allowed = await page.request.get('/grant?part=identity&mode=wrap', { maxRedirects: 0 });
  expect(allowed.status()).toBe(303);
  const location = new URL(allowed.headers().location);
  expect(location.hash.startsWith('#pf=')).toBe(true);
  expect(location.search).not.toContain('pf=');
  const canary = location.hash.slice(4);
  const mintedBefore = (await serverLog({ event: 'grant.minted' })).length;
  // Opaque fixture user id only. The host runner must read a file written by container root;
  // the enclosing directory remains private, and this marker carries no credential.
  fs.writeFileSync(run + '/identity-private/group-before.json', JSON.stringify({ user: session.user }), { mode: 0o644 });
  await expect.poll(() => fs.existsSync(run + '/identity-private/group-after.ready'), { timeout: 45_000 }).toBe(true);
  const denied = await page.request.get('/grant?part=identity&mode=wrap', { maxRedirects: 0 });
  expect(denied.status()).toBe(403);
  expect(denied.headers().location).toBeUndefined();
  expect(await denied.text()).not.toContain('Identity group fixture');
  const lines = await serverLog();
  expect(lines.some(l => l.event === 'directory.refresh' && l.outcome === 'ok')).toBe(true);
  expect(lines.some(l => l.event === 'epoch.bump' && l.reason === 'directory_refresh')).toBe(true);
  expect(lines.filter(l => l.event === 'grant.minted')).toHaveLength(mintedBefore);
  expect(JSON.stringify(lines)).not.toContain(canary);
});

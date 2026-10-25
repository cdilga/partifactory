import { test, expect, waitForLog } from './harness/fixtures';
import { signIn } from './lib/public-auth';

test('real sign-in consents device and personal agent tokens, refresh reuse and revoke fail closed', async ({ page, request }, info) => {
  test.skip(!['chromium-desktop', 'webkit-mobile'].includes(info.project.name), 'Device approval desktop and mobile WebKit lanes');
  await signIn(page, 'member-a');
  const deviceResponse = await request.post('/oauth/device_authorization', { form: {
    client_id: 'part-cli', agent_name: 'Browser gate agent', scope: 'parts:read parts:write data:read data:write',
    resource: 'https://parts.pf.test/',
  } });
  expect(deviceResponse.status()).toBe(200);
  const device = await deviceResponse.json();
  expect(device.expires_in).toBe(600); expect(device.interval).toBe(5);
  await page.goto(device.verification_uri_complete);
  await page.getByRole('button', { name: 'Continue', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'This device says it is Browser gate agent' })).toBeVisible();
  await expect(page.getByText('Agent names are unverified.')).toBeVisible();
  await page.getByRole('button', { name: 'Allow', exact: true }).click();
  await expect(page.getByRole('status')).toHaveText('Agent connected. You can return to your terminal.');
  const tokenResponse = await request.post('/oauth/token', { form: {
    client_id: 'part-cli', grant_type: 'urn:ietf:params:oauth:grant-type:device_code', device_code: device.device_code,
  } });
  expect(tokenResponse.status()).toBe(200);
  const tokens = await tokenResponse.json();
  expect(tokens.access_token).toMatch(/^pf_at_[A-Z2-7]{32}$/);
  expect(tokens.refresh_token).toMatch(/^pf_rt_[A-Z2-7]{32}$/);
  expect(tokens.expires_in).toBe(3600);
  const name = `device-${info.project.name}`;
  const created = await request.post('/api/v1/part_create', { headers: { Authorization: `Bearer ${tokens.access_token}` }, data: {
    name, kind: 'text', content: 'Made by the consenting person’s agent.', request_id: `${name}-create`,
  } });
  expect(created.status(), await created.text()).toBe(200);
  const get = (token: string) => request.post('/api/v1/part_get', { headers: { Authorization: `Bearer ${token}` }, data: { name } });
  expect((await get(tokens.access_token)).status()).toBe(200);
  const rotate = () => request.post('/oauth/token', { form: { grant_type: 'refresh_token', refresh_token: tokens.refresh_token } });
  const freshResponse = await rotate(); expect(freshResponse.status()).toBe(200);
  const fresh = await freshResponse.json(); expect((await get(fresh.access_token)).status()).toBe(200);
  expect((await rotate()).status()).toBe(400);
  expect((await get(fresh.access_token)).status()).toBe(401);
  const personal = await page.evaluate(async () => {
    const session = await (await fetch('/api/session')).json();
    const response = await fetch('/api/agents/personal', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({
      agent_name: 'Static browser gate', scopes: ['parts:read'], spaces: [`~${session.username}`], lifetime_days: 30,
    }) });
    return { status: response.status, body: await response.json() };
  });
  expect(personal.status, JSON.stringify(personal.body)).toBe(200);
  expect(personal.body.token).toMatch(/^pf_pat_[A-Z2-7]{32}$/);
  expect((await get(personal.body.token)).status()).toBe(200);
  await page.evaluate(async (id) => {
    const response = await fetch(`/api/agents/${id}/revoke`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' });
    if (!response.ok) throw new Error(await response.text());
  }, personal.body.connection_id);
  const revoked = await get(personal.body.token);
  expect(revoked.status()).toBe(401);
  expect(revoked.headers()['www-authenticate']).toContain('resource_metadata=');
  const metadata = await (await request.get('/.well-known/oauth-authorization-server')).json();
  expect(metadata.grant_types_supported).toEqual(['urn:ietf:params:oauth:grant-type:device_code', 'refresh_token']);
  expect(metadata.authorization_endpoint).toBeUndefined(); expect(metadata.registration_endpoint).toBeUndefined();
  await expect.poll(async () => (await waitForLog({ event: 'token.issue' })).length).toBeGreaterThan(0);
  expect((await waitForLog({ event: 'auth.reject' })).some(line => line.reason === 'revoked')).toBe(true);
  await page.screenshot({ path: info.outputPath('device-consent-completed.png') });
});

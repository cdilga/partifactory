import { expect, type APIRequestContext, type Page } from '@playwright/test';
import * as crypto from 'node:crypto';

// Agents connect only through short-lived OAuth credentials. These helpers drive the real
// flows a signed-in person sees; the page must already be signed in.

const b64url = (b: Buffer) => b.toString('base64').replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');

/** A standards MCP client: dynamic registration, browser consent, PKCE code exchange. */
export async function oauthToken(page: Page, request: APIRequestContext, clientName: string, scopes: string[]) {
  const redirect = 'http://127.0.0.1:47123/callback';
  const registered = await request.post('/oauth/register', { data: { client_name: clientName, redirect_uris: [redirect], grant_types: ['authorization_code', 'refresh_token'], response_types: ['code'], token_endpoint_auth_method: 'none' } });
  expect(registered.status(), await registered.text()).toBe(201);
  const client = await registered.json();
  const verifier = b64url(crypto.randomBytes(32));
  const authorize = new URL('/oauth/authorize', page.url());
  for (const [k, v] of Object.entries({ response_type: 'code', client_id: client.client_id, redirect_uri: redirect, code_challenge: b64url(crypto.createHash('sha256').update(verifier).digest()), code_challenge_method: 'S256', scope: scopes.join(' '), state: 'e2e' })) authorize.searchParams.set(k, v);
  await page.route(`${redirect}**`, route => route.fulfill({ status: 200, contentType: 'text/plain', body: 'You can return to your agent.' }));
  const callback = page.waitForRequest(r => r.url().startsWith(redirect));
  await page.goto(authorize.toString());
  await page.getByRole('button', { name: 'Allow', exact: true }).click();
  const code = new URL((await callback).url()).searchParams.get('code')!;
  await page.unroute(`${redirect}**`);
  const granted = await request.post('/oauth/token', { form: { grant_type: 'authorization_code', client_id: client.client_id, code, redirect_uri: redirect, code_verifier: verifier } });
  expect(granted.status(), await granted.text()).toBe(200);
  return (await granted.json()) as { access_token: string; refresh_token: string; expires_in: number };
}

/** A headless agent: device authorization, approved by the person in the browser. */
export async function deviceToken(page: Page, request: APIRequestContext, agentName: string, scopes: string[]) {
  const device = await (await request.post('/oauth/device_authorization', { form: { client_id: 'part-cli', agent_name: agentName, scope: scopes.join(' ') } })).json();
  await page.goto(device.verification_uri_complete);
  await page.getByRole('button', { name: 'Continue', exact: true }).click();
  await page.getByRole('button', { name: 'Allow', exact: true }).click();
  await expect(page.getByRole('status')).toHaveText('Agent connected. You can return to your terminal.');
  const grant = await request.post('/oauth/token', { form: { client_id: 'part-cli', grant_type: 'urn:ietf:params:oauth:grant-type:device_code', device_code: device.device_code } });
  expect(grant.ok(), await grant.text()).toBeTruthy();
  return (await grant.json()) as { access_token: string; refresh_token: string; expires_in: number };
}

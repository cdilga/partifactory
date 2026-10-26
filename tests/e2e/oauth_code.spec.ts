import { test, expect, waitForLog } from './harness/fixtures';
import { signIn } from './lib/public-auth';
import * as crypto from 'node:crypto';

const b64url = (b: Buffer) => b.toString('base64').replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');

// A standards MCP client: it learns everything from the 401 and the metadata documents,
// registers itself, sends the person to the browser once, then creates and edits directly.
test('MCP client discovers, registers, gets browser consent and edits parts directly until revoked', async ({ page, request }, info) => {
  test.skip(!['chromium-desktop', 'webkit-mobile'].includes(info.project.name), 'Consent desktop and mobile WebKit lanes');
  test.setTimeout(120000);
  const meta = { 'io.modelcontextprotocol/protocolVersion': '2026-07-28', 'io.modelcontextprotocol/clientInfo': { name: 'oauth-e2e', version: '1' }, 'io.modelcontextprotocol/clientCapabilities': {} };
  const rpc = (token: string | null, method: string, params: Record<string, unknown> = {}) => request.post('/mcp', { headers: { Accept: 'application/json, text/event-stream', 'MCP-Protocol-Version': '2026-07-28', 'Mcp-Method': method, ...(params.name ? { 'Mcp-Name': String(params.name) } : {}), ...(token ? { Authorization: `Bearer ${token}` } : {}) }, data: { jsonrpc: '2.0', id: 1, method, params: { _meta: meta, ...params } } });

  const unauthenticated = await rpc(null, 'tools/list');
  expect(unauthenticated.status()).toBe(401);
  const challenge = unauthenticated.headers()['www-authenticate'];
  const prmUrl = /resource_metadata="([^"]+)"/.exec(challenge)![1];
  const prm = await (await request.get(prmUrl)).json();
  expect((await (await request.get('/.well-known/oauth-protected-resource/mcp')).json()).resource).toBe(prm.resource);
  const as = await (await request.get(`${prm.authorization_servers[0]}/.well-known/oauth-authorization-server`)).json();
  expect(as.code_challenge_methods_supported).toEqual(['S256']);
  expect(as.authorization_response_iss_parameter_supported).toBe(true);
  // claude.ai and Codex identify themselves by a metadata document URL (pf-di2).
  expect(as.client_id_metadata_document_supported).toBe(true);

  const redirect = 'http://127.0.0.1:47123/callback';
  const registered = await request.post(as.registration_endpoint, { data: { client_name: 'OAuth gate agent', redirect_uris: [redirect], grant_types: ['authorization_code', 'refresh_token'], response_types: ['code'], token_endpoint_auth_method: 'none' } });
  expect(registered.status(), await registered.text()).toBe(201);
  const client = await registered.json();

  const verifier = b64url(crypto.randomBytes(32));
  const state = b64url(crypto.randomBytes(16));
  const authorize = new URL(as.authorization_endpoint);
  for (const [k, v] of Object.entries({ response_type: 'code', client_id: client.client_id, redirect_uri: redirect, code_challenge: b64url(crypto.createHash('sha256').update(verifier).digest()), code_challenge_method: 'S256', resource: `${prm.authorization_servers[0]}/mcp`, scope: prm.scopes_supported.join(' '), state })) authorize.searchParams.set(k, v);

  // PKCE downgrade and an unregistered redirect are refused before sign-in and never redirect.
  const plain = new URL(authorize); plain.searchParams.set('code_challenge_method', 'plain');
  expect((await request.get(plain.toString(), { maxRedirects: 0 })).status()).toBe(400);
  const elsewhere = new URL(authorize); elsewhere.searchParams.set('redirect_uri', 'https://evil.example.com/callback');
  expect((await request.get(elsewhere.toString(), { maxRedirects: 0 })).status()).toBe(400);

  await signIn(page, 'member-a');
  const callback = page.waitForRequest(r => r.url().startsWith(redirect));
  await page.route(`${redirect}**`, route => route.fulfill({ status: 200, contentType: 'text/plain', body: 'You can return to your agent.' }));
  await page.goto(authorize.toString());
  await expect(page.getByRole('heading', { name: 'Allow an agent', exact: true })).toBeVisible();
  await expect(page.locator('#agent')).toHaveText('OAuth gate agent');
  await expect(page.locator('#redirect-host')).toHaveText('127.0.0.1');
  // Lifetimes are fixed (decision 55): no picker on the sheet (pf-vp3).
  await expect(page.getByText('Connection lifetime')).toHaveCount(0);
  await expect(page.locator('select#days')).toHaveCount(0);
  await page.screenshot({ path: info.outputPath('oauth-consent.png'), fullPage: true });
  await page.getByRole('button', { name: 'Allow', exact: true }).click();
  const returned = new URL((await callback).url());
  expect(returned.searchParams.get('iss')).toBe(as.issuer);
  expect(returned.searchParams.get('state')).toBe(state);
  const code = returned.searchParams.get('code')!;

  const exchange = (overrides: Record<string, string> = {}) => request.post(as.token_endpoint, { form: { grant_type: 'authorization_code', client_id: client.client_id, code, redirect_uri: redirect, code_verifier: verifier, resource: `${prm.authorization_servers[0]}/mcp`, ...overrides } });
  expect((await exchange({ code_verifier: b64url(crypto.randomBytes(32)) })).status()).toBe(400);
  const granted = await exchange();
  expect(granted.status(), await granted.text()).toBe(200);
  const tokens = await granted.json();
  // A code is single use.
  expect((await exchange()).status()).toBe(400);
  // Replay revoked the connection it created, so connect once more for the direct-edit check.
  expect((await rpc(tokens.access_token, 'tools/list')).status()).toBe(401);

  const again = new URL(authorize); const verifier2 = b64url(crypto.randomBytes(32));
  again.searchParams.set('code_challenge', b64url(crypto.createHash('sha256').update(verifier2).digest()));
  const callback2 = page.waitForRequest(r => r.url().startsWith(redirect));
  await page.goto(again.toString());
  await page.getByRole('button', { name: 'Allow', exact: true }).click();
  const code2 = new URL((await callback2).url()).searchParams.get('code')!;
  const fresh = await (await exchange({ code: code2, code_verifier: verifier2 })).json();
  const token = fresh.access_token as string;

  const call = async (name: string, args: unknown) => {
    const response = await rpc(token, 'tools/call', { name, arguments: args });
    expect(response.status(), await response.text()).toBe(200);
    return (await response.json()).result;
  };
  const name = `oauth-${info.project.name}-${Date.now()}`;
  // No separate approval: the connection's consent covers creating and editing.
  expect((await call('part_create', { name, title: name, kind: 'text', content: 'Made over OAuth.', request_id: `${name}-create` })).isError).toBe(false);
  expect((await call('part_edit', { name, edits: [{ op: 'replace', old_str: 'Made over OAuth.', new_str: 'Edited over OAuth.' }], request_id: `${name}-edit` })).isError).toBe(false);

  // The page is still on the client's loopback callback; return to the workspace to revoke.
  await page.goto('/connected-agents');
  const connection = await page.evaluate(async () => (await (await fetch('/api/agents')).json()).find((c: any) => c.kind === 'oauth_code' && c.agent_name === 'OAuth gate agent' && !c.revoked_at));
  await page.evaluate(async (id) => {
    const response = await fetch(`/api/agents/${id}/revoke`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' });
    if (!response.ok) throw new Error(await response.text());
  }, connection.id);
  const after = await rpc(token, 'tools/list');
  expect(after.status()).toBe(401);
  expect(after.headers()['www-authenticate']).toContain('resource_metadata=');

  expect((await waitForLog({ event: 'oauth.token' })).some(l => l.grant_type === 'authorization_code' && l.outcome === 'ok')).toBe(true);
  expect((await waitForLog({ event: 'connection.create' })).some(l => l.kind === 'oauth_code')).toBe(true);
});

import { test, expect, waitForLog } from './harness/fixtures';
import { signIn } from './lib/public-auth';
import * as fs from 'node:fs';
import * as path from 'node:path';

test('MCP real PAT and device connections discover check image and isolate screenshot ownership', async ({ page, request }, info) => {
  test.skip(info.project.name !== 'chromium-desktop', 'The renderer owns its browser; one protocol lane is sufficient.');
  test.setTimeout(180000);
  await signIn(page, 'member-a');
  const scopes = ['parts:read', 'parts:write', 'parts:delete', 'data:read', 'data:write'];
  const device = await (await request.post('/oauth/device_authorization', { form: { client_id: 'part-cli', agent_name: 'MCP conformance', scope: scopes.join(' ') } })).json();
  await page.goto(device.verification_uri_complete);
  await page.getByRole('button', { name: 'Continue', exact: true }).click();
  await page.getByRole('button', { name: 'Allow', exact: true }).click();
  await expect(page.getByRole('status')).toHaveText('Agent connected. You can return to your terminal.');
  const grant = await request.post('/oauth/token', { form: { client_id: 'part-cli', grant_type: 'urn:ietf:params:oauth:grant-type:device_code', device_code: device.device_code } });
  expect(grant.ok(), await grant.text()).toBeTruthy();
  const deviceToken = (await grant.json()).access_token;
  const personal = await page.evaluate(async (scopes) => {
    const session = await (await fetch('/api/session')).json();
    const response = await fetch('/api/agents/personal', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ agent_name: 'MCP conformance', scopes, spaces: [`~${session.username}`], lifetime_days: 30 }) });
    if (!response.ok) throw new Error(await response.text());
    return response.json();
  }, scopes);
  const tokens = { pat: personal.token as string, device: deviceToken as string };
  const privateDir = path.join('/repo/target/e2e-runs', process.env.PF_E2E_RUN ?? 'local', 'agent-surface-private');
  fs.mkdirSync(privateDir, { recursive: true, mode: 0o700 });
  fs.writeFileSync(path.join(privateDir, 'tokens.json'), JSON.stringify(tokens), { mode: 0o600 });
  const meta = { 'io.modelcontextprotocol/protocolVersion': '2026-07-28', 'io.modelcontextprotocol/clientInfo': { name: 'e2e', version: '1' }, 'io.modelcontextprotocol/clientCapabilities': {} };
  const rpc = (token: string | null, method: string, params: Record<string, unknown> = {}) => request.post('/mcp', { headers: { Accept: 'application/json, text/event-stream', 'MCP-Protocol-Version': '2026-07-28', 'Mcp-Method': method, ...(params.name ? { 'Mcp-Name': String(params.name) } : {}), ...(token ? { Authorization: `Bearer ${token}` } : {}) }, data: { jsonrpc: '2.0', id: 1, method, params: { _meta: meta, ...params } } });
  const missing = await rpc(null, 'tools/list');
  expect(missing.status()).toBe(401); expect(missing.headers()['www-authenticate']).toContain('resource_metadata');
  expect((await missing.json()).error.details.ways.map((w: any) => w.kind)).toEqual(['oauth', 'device_code', 'personal_token']);
  const skill = await (await request.get('/skill.md')).text();
  const guide = skill.slice(skill.indexOf('\n---\n') + 6).trimStart();
  for (const endpoint of ['/', '/llms.txt', '/llms-full.txt', '/agents.md', '/.well-known/mcp/server.json']) {
    const response = await request.get(endpoint, { headers: { Accept: 'text/markdown' } });
    expect(response.ok(), endpoint).toBeTruthy();
    if (endpoint === '/') expect(response.headers().vary).toContain('Accept');
  }
  for (const [kind, token] of Object.entries(tokens)) {
    const discovery = await rpc(token, 'server/discover');
    expect(discovery.headers()['mcp-session-id']).toBeUndefined();
    expect((await discovery.json()).result.instructions).toBe(guide);
    const call = async (name: string, args: unknown) => {
      const response = await rpc(token, 'tools/call', { name, arguments: args });
      expect(response.status(), await response.text()).toBe(200);
      return (await response.json()).result;
    };
    const name = `mcp-real-${kind}-${Date.now()}`;
    expect((await call('part_create', { name, title: name, kind: 'html', content: '<!doctype html><body>MCP renderer gate<script>partifactory.ready()</script></body>', request_id: `${name}-create` })).isError).toBe(false);
    const check = await call('part_check', { name });
    if (process.env.PF_E2E_RENDER === '1') expect(check.isError, JSON.stringify(check)).toBe(false);
    if (!check.isError) {
      const image = check.content.find((c: any) => c.type === 'image');
      expect(image.mimeType).toBe('image/png'); expect(Buffer.from(image.data, 'base64').length).toBeLessThanOrEqual(1000000);
      const url = check.structuredContent.screenshot.url;
      expect((await request.get(url, { headers: { Authorization: `Bearer ${token}` } })).status()).toBe(200);
      const other = kind === 'pat' ? tokens.device : tokens.pat;
      expect((await request.get(url, { headers: { Authorization: `Bearer ${other}` } })).status()).toBe(404);
      fs.writeFileSync(info.outputPath(`mcp-${kind}.png`), Buffer.from(image.data, 'base64'));
    }
    expect((await call('part_edit', { name, edits: [{ op: 'replace', old_str: 'MCP renderer gate', new_str: 'MCP edited gate' }], request_id: `${name}-edit` })).isError).toBe(false);
    expect((await call('part_data', { name, action: 'list' })).isError).toBe(false);
  }
  const lines = await waitForLog({ event: 'mcp.tool' });
  expect(lines.some(l => l.surface === 'mcp' && l.op === 'part_check' && l.request_id)).toBe(true);
  expect((await waitForLog({ event: 'auth.reject' })).some(l => l.surface === 'mcp')).toBe(true);
});

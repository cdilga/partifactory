import { test, expect, waitForLog } from './harness/fixtures';
import * as fs from 'node:fs';
import * as crypto from 'node:crypto';
const tokenPath = process.env.PF_DEV_API_TOKEN_FILE ?? '/run/pf/run/secrets/api_token';

test('registry HTTPS gate: create, read, rebase, replay, scopes and OpenAPI', async ({ request }, info) => {
  const token = fs.readFileSync(tokenPath, 'utf8').trim();
  const headers = { Authorization: `Bearer ${token}` };
  const name = `ops-${info.project.name.replace(/[^a-z0-9]/g, '-').slice(0, 35)}`;
  const call = async (op: string, data: unknown) => request.post(`/api/v1/${op}`, { headers, data });
  let r = await call('part_create', { name, title: 'Packing', kind: 'text', content: 'alpha beta', request_id: `${name}-create` });
  expect(r.status()).toBe(200);
  const created = await r.json();
  expect(created.head).toBe(1);
  for (const [rid, old, value] of [['a', 'alpha', 'one'], ['b', 'beta', 'two']]) {
    const input = { name, base_version: 1, request_id: `${name}-${rid}`, wait: false, edits: [{ op: 'replace', old_str: old, new_str: value }] };
    r = await call('part_edit', input);
    expect(r.status()).toBe(200);
    const result = await r.json();
    if (rid === 'b') expect(result.rebased_from).toBe(1);
    r = await call('part_edit', input);
    expect((await r.json()).replayed).toBe(true);
  }
  r = await call('part_get', { name });
  expect((await r.json()).content['index.txt']).toBe('one two');
  r = await call('part_edit', { name, request_id: `${name}-bad`, edits: [{ op: 'replace', old_str: 'missing anchor', new_str: 'x' }] });
  expect(r.status()).toBe(422);
  const error = await r.json();
  expect(error.error.code).toBe('anchor_not_found');
  expect(error.error.details.request_id).toBeTruthy();
  r = await call('part_get', { name, histroy: true });
  expect(r.status()).toBe(400);
  expect((await r.json()).error.hint).toContain('history');
  r = await request.get('/openapi.json');
  expect(r.ok()).toBeTruthy();
  const spec = await r.json();
  expect(spec.openapi).toBe('3.1.0');
  expect(spec.components.schemas.Error.properties.error.properties.code.enum).toContain('anchor_not_found');
  expect(r.headers().etag).toBeTruthy();
  expect((await request.get('/openapi.json', { headers: { 'If-None-Match': r.headers().etag } })).status()).toBe(304);
  const id = (await call('part_get', { name })).headers()['x-request-id'];
  const logs = await waitForLog({ request_id: id, event: 'http.request' });
  expect(logs.some(l => l.route_class === 'api' && l.op === 'part_get')).toBeTruthy();
  await call('part_edit', { name, request_id: `${name}-delete`, delete: true });
});

test('blob have/need deduplicates without a second blob.put', async ({ request }, info) => {
  const token = fs.readFileSync(tokenPath, 'utf8').trim();
  const headers = { Authorization: `Bearer ${token}` };
  const bytes = Buffer.from(`blob-dedupe-${info.project.name}`);
  const hash = crypto.createHash('sha256').update(bytes).digest('hex');
  const put = () => request.post('/api/v1/blobs', { headers, data: bytes });
  let r = await put(); expect(r.status()).toBe(200); expect((await r.json()).created).toBe(true);
  r = await request.post('/api/v1/blobs/missing', { headers, data: { hashes: [hash] } });
  expect((await r.json()).missing).toEqual([]);
  r = await put(); expect((await r.json()).created).toBe(false);
  const rid = r.headers()['x-request-id'];
  const logs = await waitForLog({ request_id: rid, event: 'http.request' });
  expect(logs.length).toBeGreaterThan(0);
  const { serverLog } = await import('./harness/fixtures');
  expect(await serverLog({ request_id: rid, event: 'blob.put' })).toHaveLength(0);
});

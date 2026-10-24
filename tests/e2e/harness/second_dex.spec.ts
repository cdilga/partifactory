// Both Dex configs boot and serve discovery through the harness proxy. Signing in with the
// second one is the identity bead's check.
import { test, expect } from './fixtures';

for (const [name, issuer] of [
  ['first', 'https://dex.pf.test/dex'],
  ['second', 'https://dex2.pf.test/dex'],
] as const) {
  test(`${name} Dex serves its discovery document`, async ({ request }) => {
    const r = await request.get(`${issuer}/.well-known/openid-configuration`);
    expect(r.status()).toBe(200);
    const doc = await r.json();
    expect(doc.issuer).toBe(issuer);
    expect(doc.authorization_endpoint).toBe(`${issuer}/auth`);
    expect(doc.scopes_supported).toEqual(expect.arrayContaining(['openid', 'email']));
  });
}

import { expect, type Page, type BrowserContext } from '@playwright/test';

export async function signIn(page: Page, username = 'admin') {
  await page.goto('https://parts.pf.test/signin?return=/');
  const email = page.locator('input[name=login]');
  await expect(email).toBeVisible();
  await email.fill(`${username}@example.com`);
  await page.locator('input[name=password]').fill('password');
  await page.locator('button[type=submit]').click();
  await page.waitForURL('https://parts.pf.test/**');
  await expect.poll(async () => (await page.request.get('https://parts.pf.test/api/session')).status()).toBe(200);
}

// This is a real browser authenticator: the server validates attestation and every later
// assertion. Automation only supplies the hardware and its successful user-verification signal.
export async function nativePasskey(page: Page, context: BrowserContext) {
  const cdp = await context.newCDPSession(page);
  await cdp.send('WebAuthn.enable');
  const { authenticatorId } = await cdp.send('WebAuthn.addVirtualAuthenticator', {
    options: { protocol: 'ctap2', transport: 'internal', hasResidentKey: true, hasUserVerification: true, isUserVerified: true, automaticPresenceSimulation: true },
  });
  const result = await page.evaluate(async () => {
    const decode = (s: string) => Uint8Array.from(atob(s.replace(/-/g, '+').replace(/_/g, '/')), c => c.charCodeAt(0));
    const encode = (b: ArrayBuffer) => btoa(String.fromCharCode(...new Uint8Array(b))).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
    const start = await fetch('/admin/passkeys/register/start', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' });
    const challenge = await start.json();
    if (!start.ok) throw new Error(JSON.stringify(challenge));
    const p = challenge.publicKey;
    p.challenge = decode(p.challenge); p.user.id = decode(p.user.id);
    (p.excludeCredentials || []).forEach((c: any) => c.id = decode(c.id));
    const credential = await navigator.credentials.create({ publicKey: p }) as PublicKeyCredential;
    const response = credential.response as AuthenticatorAttestationResponse;
    const finish = await fetch('/admin/passkeys/register/finish', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({
      flow: challenge.flow, credential: { id: credential.id, rawId: encode(credential.rawId), type: credential.type, response: { attestationObject: encode(response.attestationObject), clientDataJSON: encode(response.clientDataJSON), transports: response.getTransports() }, extensions: credential.getClientExtensionResults() },
    }) });
    return { status: finish.status, body: await finish.json() };
  });
  expect(result.status, JSON.stringify(result.body)).toBe(200);
  return { cdp, authenticatorId };
}

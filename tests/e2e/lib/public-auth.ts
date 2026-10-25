import { expect, type Page, type BrowserContext } from '@playwright/test';

import * as fs from 'node:fs';
import * as path from 'node:path';

// These are real OIDC sessions, shared across browser workers like repeat household visits.
// The sign-in + callback limiter allows five full logins per IP per minute. Re-authenticating
// for every isolated test would test that limiter instead of the feature under observation.
export async function signIn(page: Page, username = 'admin') {
  const directory=process.env.PF_BROWSER_AUTH_DIR??path.join('/repo/target/e2e-runs',process.env.PF_E2E_RUN??'local','browser-auth');
  fs.mkdirSync(directory,{recursive:true,mode:0o700});
  const state=path.join(directory,username+'.json'),lock=state+'.lock';
  const restore=async()=>{const prior=JSON.parse(fs.readFileSync(state,'utf8'));await page.context().addCookies(prior.cookies);await page.goto('https://parts.pf.test/');};
  if(fs.existsSync(state)){await restore();return;}
  let acquired=false;
  for(let i=0;i<200;i++){
    try{fs.mkdirSync(lock);acquired=true;break;}catch(e){if((e as NodeJS.ErrnoException).code!=='EEXIST')throw e;}
    if(fs.existsSync(state)){await restore();return;}
    await new Promise(r=>setTimeout(r,100));
  }
  if(!acquired)throw new Error('Timed out waiting for the real OIDC session setup');
  try {
    if(fs.existsSync(state)){await restore();return;}
    await page.goto('https://parts.pf.test/signin?return=/');
    const email = page.locator('input[name=login]');
    await expect(email).toBeVisible();
    await email.fill(`${username}@example.com`);
    await page.locator('input[name=password]').fill('password');
    await page.locator('button[type=submit]').click();
    await page.waitForURL('https://parts.pf.test/**');
    await expect.poll(async () => (await page.request.get('https://parts.pf.test/api/session')).status()).toBe(200);
    fs.writeFileSync(state,JSON.stringify(await page.context().storageState()),{mode:0o600});
  } finally {fs.rmdirSync(lock);}
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
  // Enrollment proves possession when adding the credential; an admin action separately
  // requires a fresh assertion. Exercise that real ceremony with this authenticator.
  const stepup = await page.evaluate(async () => {
    const decode = (s: string) => Uint8Array.from(atob(s.replace(/-/g, '+').replace(/_/g, '/')), c => c.charCodeAt(0));
    const encode = (b: ArrayBuffer) => btoa(String.fromCharCode(...new Uint8Array(b))).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
    const start = await fetch('/admin/stepup/start', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' });
    const challenge = await start.json();
    if (!start.ok) throw new Error(JSON.stringify(challenge));
    const publicKey = challenge.publicKey;
    publicKey.challenge = decode(publicKey.challenge);
    for (const credential of publicKey.allowCredentials ?? []) credential.id = decode(credential.id);
    const credential = await navigator.credentials.get({ publicKey }) as PublicKeyCredential;
    const response = credential.response as AuthenticatorAssertionResponse;
    const finish = await fetch('/admin/stepup/finish', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({
      flow: challenge.flow, credential: { id: credential.id, rawId: encode(credential.rawId), type: credential.type,
        response: { authenticatorData: encode(response.authenticatorData), clientDataJSON: encode(response.clientDataJSON),
          signature: encode(response.signature), userHandle: response.userHandle ? encode(response.userHandle) : null },
        extensions: credential.getClientExtensionResults() },
    }) });
    return { status: finish.status, body: await finish.json() };
  });
  expect(stepup.status, JSON.stringify(stepup.body)).toBe(200);
  expect(stepup.body.step_up_status).toBe(true);
  return { cdp, authenticatorId };
}

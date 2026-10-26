import { test, expect, waitForLog } from './harness/fixtures';
import { signIn, nativePasskey } from './lib/public-auth';
import * as fs from 'node:fs';
import * as path from 'node:path';

async function noPageOverflow(page: import('@playwright/test').Page) {
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
}

test('populated gallery opens permanent OAuth Connect and migrates old bookmarks', async ({page}, info) => {
  await signIn(page);
  await expect(page.getByRole('link', {name:'Open notes',exact:true})).toBeVisible();
  await expect(page.locator('html')).toHaveAttribute('data-pf-hydrated','1');
  await page.getByLabel('Account menu').click();
  await page.getByRole('link',{name:'Connect an agent',exact:true}).click();
  await expect(page).toHaveURL('https://parts.pf.test/connect');
  await expect(page.getByRole('heading',{name:'Connect an agent',exact:true})).toBeVisible();
  await expect(page.locator('#connect-command')).toHaveText('https://parts.pf.test/mcp');
  await expect(page.locator('main')).toContainText('OAuth');
  await expect(page.locator('main')).not.toContainText('personal token');
  await expect(page.getByRole('link',{name:'Connected agents',exact:true}).last()).toHaveAttribute('href','/connected-agents');
  await expect(page.getByRole('link',{name:'Enter a device code',exact:true})).toHaveAttribute('href','/device');
  // Step-by-step setup for each agent, all OAuth or device code.
  for (const client of ['Claude Code','Codex','Cursor','Gemini CLI','Hermes']) await expect(page.locator('summary',{hasText:client}).first()).toBeVisible();
  await expect(page.locator('.clients code.command').first()).toHaveText('claude mcp add --transport http partifactory https://parts.pf.test/mcp');
  await page.locator('summary',{hasText:'Hermes'}).click();
  await expect(page.locator('.clients')).toContainText('auth: oauth');
  await page.goto('/#connect');
  await expect(page).toHaveURL('https://parts.pf.test/connect');
  await expect(page.locator('html')).toHaveAttribute('data-pf-hydrated','1');
  const evidence=path.resolve('../../target/evidence/pf-c5a',info.project.name);fs.mkdirSync(evidence,{recursive:true});
  await page.screenshot({path:path.join(evidence,'connect-desktop.png'),fullPage:true});
  await page.setViewportSize({width:360,height:740});
  await noPageOverflow(page);
  await page.getByRole('button',{name:'Toggle theme'}).click();
  await page.screenshot({path:path.join(evidence,'connect-360-dark.png'),fullPage:true});
  expect((await waitForLog({event:'ui.render'})).some(l=>l.page==='connect')).toBe(true);
});

test('Public links shares the workspace shell on desktop and a 360px phone', async ({page}, info) => {
  await signIn(page);
  await page.goto('/admin/links');
  await expect(page.locator('html')).toHaveAttribute('data-pf-hydrated','1');
  await expect(page.getByRole('heading',{name:'Public links',exact:true})).toBeVisible();
  await expect(page.locator('#links')).toHaveAttribute('aria-busy','false');
  await expect(page.locator('#status')).not.toHaveText('Loading public links…');
  await expect(page.locator('.topbar .search')).toHaveAttribute('action','/');
  const evidence=path.resolve('../../target/evidence/pf-y4x',info.project.name);fs.mkdirSync(evidence,{recursive:true});
  await page.screenshot({path:path.join(evidence,'links-light.png'),fullPage:true});
  await page.setViewportSize({width:360,height:740});
  await noPageOverflow(page);
  await page.getByRole('button',{name:'Toggle theme'}).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme','dark');
  await page.reload();
  await expect(page.locator('html')).toHaveAttribute('data-theme','dark');
  await page.screenshot({path:path.join(evidence,'links-360-dark.png'),fullPage:true});
  expect((await waitForLog({event:'ui.render'})).some(l=>l.page==='public_links')).toBe(true);
});

test('Public links keeps real individual revocation and cancels a short kill-switch hold', async ({page,context},info) => {
  test.skip(info.project.name!=='chromium-desktop','Native virtual authenticator lane');
  await signIn(page);await nativePasskey(page,context);
  // A real browser fetch, so the API sees genuine Origin and Sec-Fetch-Site headers.
  const post=(path:string,body:unknown)=>page.evaluate(async([path,body])=>{const r=await fetch(path,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)});return {status:r.status,body:await r.json()};},[path,body] as const);
  await page.goto('/admin/links');
  const created=await post('/api/chrome/v1/notes/links',{name:'notes',mode:'view'});
  expect(created.status,JSON.stringify(created.body)).toBe(200);
  const {link,url}=created.body;
  try {
    await page.reload();
    const row=page.getByRole('row').filter({has:page.getByRole('button',{name:'Revoke link for notes',exact:true})}).filter({hasText:'Never'}).last();
    await expect(row).toBeVisible();
    const mutations:string[]=[];
    page.on('request',r=>{if(r.method()==='POST')mutations.push(new URL(r.url()).pathname);});
    const kill=page.getByRole('button',{name:'Hold to revoke all links',exact:true});
    await kill.focus();await page.keyboard.down('Space');await page.waitForTimeout(150);await page.keyboard.up('Space');
    await expect(page.locator('#status')).toHaveText('Cancelled.');
    expect(mutations).not.toContain('/api/admin/v1/links/revoke-all');
    // The latest notes row is this test's link; the response identifies it exactly.
    const revoked=page.waitForResponse(r=>r.url().endsWith('/links/'+link.id)&&r.request().method()==='POST');
    await row.getByRole('button',{name:'Revoke link for notes',exact:true}).click();
    expect((await revoked).status()).toBe(200);
    await expect(page.locator('#status')).toContainText('Link revoked.');
    expect((await page.request.get(url)).status()).toBe(404);
  } finally {
    await page.goto('/admin/links');await post('/api/chrome/v1/notes/links/'+link.id,{action:'revoke'});
  }
});

test('Public links remains administrator-only while Connect is available to members', async ({page}) => {
  await signIn(page,'member-a');
  expect((await page.goto('/admin/links'))?.status()).toBe(403);
  expect((await page.goto('/connect'))?.status()).toBe(200);
  await expect(page.getByRole('heading',{name:'Connect an agent',exact:true})).toBeVisible();
});

test('Security shows how admin actions are confirmed, as a choice', async ({page}, info) => {
  await signIn(page);
  await page.getByLabel('Account menu').click();
  await page.getByRole('link',{name:'Security',exact:true}).click();
  await expect(page).toHaveURL('https://parts.pf.test/admin/security');
  await expect(page.getByRole('heading',{name:'Confirm high-risk actions with',exact:true})).toBeVisible();
  // The checked option is the mode in force; the harness keeps the passkey default.
  await expect(page.getByRole('radio',{name:/Your partifactory passkey/})).toBeChecked();
  await expect(page.getByRole('radio',{name:/Nothing extra/})).not.toBeChecked();
  await expect(page.getByRole('radio',{name:/fresh sign-in/})).toBeEnabled();
  // Passkeys have their own section, and adding one is always offered.
  await expect(page.getByRole('heading',{name:'Your partifactory passkeys',exact:true})).toBeVisible();
  await expect(page.getByRole('button',{name:'Add a passkey',exact:true})).toBeVisible();
  const evidence=path.resolve('../../target/evidence/pf-security',info.project.name);fs.mkdirSync(evidence,{recursive:true});
  await page.screenshot({path:path.join(evidence,'security.png'),fullPage:true});
  await page.setViewportSize({width:360,height:740});
  await noPageOverflow(page);
  expect((await waitForLog({event:'ui.render'})).some(l=>l.page==='security')).toBe(true);
});

test('A provider confirmation is refused unless the provider signs the admin in again', async ({browser}) => {
  // Its own session, so this never touches the shared admin session. The harness's Dex asks
  // for the password again (prompt=login) but its ID token carries no auth_time, so nothing
  // proves a fresh sign-in: the confirmation must be refused, the session stays unconfirmed,
  // and the admin is told why. The accepted path is pf-auth's
  // provider_confirmation_needs_the_same_person_signed_in_again plus the live gate.
  const context=await browser.newContext();const page=await context.newPage();
  try{
    const base=process.env.PF_BASE_URL??'https://parts.pf.test';
    await page.goto(base+'/signin?return=/');
    await page.locator('input[name=login]').fill('admin@example.com');
    await page.locator('input[name=password]').fill('password');
    await page.locator('button[type=submit]').click();
    await page.waitForURL(base+'/**');
    expect((await (await page.request.get(base+'/api/session')).json()).step_up_status).toBe(false);
    await page.goto(base+'/admin/stepup/signin?return=/admin/security');
    await expect(page.locator('input[name=login]')).toBeVisible();
    await page.locator('input[name=login]').fill('admin@example.com');
    await page.locator('input[name=password]').fill('password');
    await page.locator('button[type=submit]').click();
    await page.waitForURL(base+'/admin/security');
    await expect(page.locator('#security-status')).toContainText("didn't ask you to sign in again");
    expect((await (await page.request.get(base+'/api/session')).json()).step_up_status).toBe(false);
    expect((await waitForLog({event:'stepup.verify'})).some(l=>l.via==='provider'&&l.outcome==='refused')).toBe(true);
  }finally{await context.close();}
});

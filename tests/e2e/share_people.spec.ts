import { test, expect, waitForLog } from './harness/fixtures';
import { signIn } from './lib/public-auth';
import { oauthToken } from './lib/agent-tokens';

// Inward sharing (Slice 2, pf-h6y.1): the owner adds a household member from the share sheet,
// the member opens the part, and removing them ends their access on the next request.
test('the owner shares a part with a member, who opens it until removed', async ({ page, browser }, info) => {
  test.skip(!['chromium-desktop', 'webkit-mobile', 'firefox-desktop'].includes(info.project.name), 'Fixed part hosts exist for these lanes');
  test.setTimeout(120000);
  const name = `share-${info.project.name}`;
  const member = await browser.newContext({ ignoreHTTPSErrors: true });
  const memberPage = await member.newPage();
  // The member has signed in once, so the directory knows them.
  await signIn(memberPage, 'member-b');

  await signIn(page, 'admin');
  const token = (await oauthToken(page, page.request, 'Share gate', ['parts:read', 'parts:write'])).access_token;
  const created = await page.request.post('/api/v1/part_create', { headers: { Authorization: `Bearer ${token}` }, data: { name, title: `Shared ${info.project.name}`, kind: 'html', content: '<!doctype html><h1>For the household</h1>', request_id: `${name}-${Date.now()}` } });
  expect([200, 409].includes(created.status()), await created.text()).toBeTruthy();

  const denied = await memberPage.goto(`https://parts.pf.test/grant?part=${name}`);
  expect(denied?.status()).toBe(403);
  await expect(memberPage.getByRole('heading', { name: '403 · NOT SHARED WITH YOU' })).toBeVisible();

  // Share → People you add → pick → Add: the click budget for "Share with someone in the household".
  await page.goto(`https://${name}-part.pf.test/`);
  await page.getByRole('button', { name: 'Share', exact: true }).click();
  await page.getByRole('radio', { name: /People you add/ }).check();
  await page.getByRole('searchbox').fill('Member B');
  await page.getByRole('option', { name: /Member B/ }).click();
  await page.getByLabel('Role for new people').selectOption('view');
  await page.getByRole('button', { name: 'Add', exact: true }).click();
  await expect(page.locator('.pf-sentence')).toContainText('can open this part');
  await expect(page.locator('.pf-people')).toContainText('Can view');
  await expect(page.locator('.pf-audience')).toHaveText('Shared');
  await page.screenshot({ path: info.outputPath('share-people.png'), fullPage: true });

  const opened = await memberPage.goto(`https://${name}-part.pf.test/`);
  expect(opened?.status()).toBeLessThan(400);
  await expect(memberPage.frameLocator('#pf-frame').getByRole('heading', { name: 'For the household' })).toBeVisible();
  // A View member sees who can open it but can't change it.
  await memberPage.getByRole('button', { name: 'Share', exact: true }).click();
  await expect(memberPage.getByText('Only the owner can change who has access.')).toBeVisible();
  await expect(memberPage.getByRole('button', { name: 'Remove', exact: true })).toHaveCount(0);

  await page.getByRole('button', { name: 'Remove', exact: true }).click();
  await expect(page.locator('#pf-status')).toContainText('removed');
  const after = await memberPage.goto(`https://parts.pf.test/grant?part=${name}`);
  expect(after?.status()).toBe(403);
  await page.getByRole('button', { name: 'Undo', exact: true }).click();
  await expect(page.locator('#pf-status')).toHaveText('Restored.');
  const back = await memberPage.goto(`https://${name}-part.pf.test/`);
  expect(back?.status()).toBeLessThan(400);

  const logs = await waitForLog({ event: 'grant.set' });
  expect(logs.some(l => l.surface === 'chrome')).toBe(true);
  expect((await waitForLog({ event: 'grant.remove' })).some(l => l.outcome === 'ok')).toBe(true);
  await member.close();
});

// Move to Family (pf-h6y.2): one tap opens a part to the household group, and only to it.
test('Move to Family opens a part to the household and not to outsiders', async ({ page, browser }, info) => {
  test.skip(!['chromium-desktop', 'webkit-mobile', 'firefox-desktop'].includes(info.project.name), 'Fixed part hosts exist for these lanes');
  test.setTimeout(120000);
  const name = `family-${info.project.name}`;
  const open = async (who: string) => {
    const context = await browser.newContext({ ignoreHTTPSErrors: true });
    const p = await context.newPage();
    await signIn(p, who);
    return { context, page: p };
  };
  const member = await open('member-b');
  const outsider = await open('outsider');
  await signIn(page, 'admin');
  const token = (await oauthToken(page, page.request, 'Family gate', ['parts:read', 'parts:write'])).access_token;
  const created = await page.request.post('/api/v1/part_create', { headers: { Authorization: `Bearer ${token}` }, data: { name, title: `Family ${info.project.name}`, kind: 'html', content: '<!doctype html><h1>For everyone at home</h1>', request_id: `${name}-${Date.now()}` } });
  expect([200, 409].includes(created.status()), await created.text()).toBeTruthy();

  await page.goto(`https://${name}-part.pf.test/`);
  await page.getByRole('button', { name: 'Share', exact: true }).click();
  // Share → Move to Family: the click budget for "Share with the family".
  await page.getByRole('button', { name: 'Move to Family', exact: true }).click();
  await expect(page.locator('#pf-status')).toContainText('Moved to Family');
  await expect(page.locator('.pf-family')).toContainText('In Family');
  await page.screenshot({ path: info.outputPath('move-to-family.png'), fullPage: true });

  expect((await member.page.goto(`https://${name}-part.pf.test/`))?.status()).toBeLessThan(400);
  await expect(member.page.frameLocator('#pf-frame').getByRole('heading', { name: 'For everyone at home' })).toBeVisible();
  expect((await outsider.page.goto(`https://parts.pf.test/grant?part=${name}`))?.status()).toBe(403);

  await page.getByRole('button', { name: 'Move back to my space', exact: true }).click();
  await expect(page.locator('#pf-status')).toHaveText('Moved to your space.');
  expect((await member.page.goto(`https://parts.pf.test/grant?part=${name}`))?.status()).toBe(403);
  expect((await waitForLog({ event: 'part.move' })).filter(l => l.outcome === 'ok').length).toBeGreaterThanOrEqual(2);
  await member.context.close();
  await outsider.context.close();
});

// Ask for access (pf-h6y.3): a member asks from the 403 page; the owner adds them in two taps.
test('a member asks for access and the owner adds them from the share sheet', async ({ page, browser }, info) => {
  test.skip(!['chromium-desktop', 'webkit-mobile', 'firefox-desktop'].includes(info.project.name), 'Fixed part hosts exist for these lanes');
  test.setTimeout(120000);
  const name = `ask-${info.project.name}`;
  const title = `Secret plans ${info.project.name}`;
  const memberContext = await browser.newContext({ ignoreHTTPSErrors: true });
  const member = await memberContext.newPage();
  await signIn(member, 'member-b');
  await signIn(page, 'admin');
  const token = (await oauthToken(page, page.request, 'Ask gate', ['parts:read', 'parts:write'])).access_token;
  const created = await page.request.post('/api/v1/part_create', { headers: { Authorization: `Bearer ${token}` }, data: { name, title, kind: 'html', content: '<!doctype html><h1>Plans</h1>', request_id: `${name}-${Date.now()}` } });
  expect([200, 409].includes(created.status()), await created.text()).toBeTruthy();

  expect((await member.goto(`https://parts.pf.test/grant?part=${name}`))?.status()).toBe(403);
  await expect(member.locator('body')).not.toContainText(title);
  await member.getByLabel('Add a note for the owner (optional)').fill('For the holiday?');
  await member.getByRole('button', { name: 'Ask for access', exact: true }).click();
  await expect(member.getByRole('heading', { name: /ACCESS REQUESTED/ })).toBeVisible();
  await expect(member.locator('body')).not.toContainText(title);

  await page.goto(`https://${name}-part.pf.test/`);
  await page.getByRole('button', { name: 'Share', exact: true }).click();
  await expect(page.locator('.pf-asks')).toContainText('1 person is asking for access');
  await expect(page.locator('.pf-asks')).toContainText('For the holiday?');
  await page.screenshot({ path: info.outputPath('ask-for-access.png'), fullPage: true });
  await page.locator('.pf-asks').getByRole('button', { name: 'Add', exact: true }).click();
  await expect(page.locator('#pf-status')).toContainText('can now open it');
  await expect(page.locator('.pf-asks')).toBeHidden();

  expect((await member.goto(`https://${name}-part.pf.test/`))?.status()).toBeLessThan(400);
  await expect(member.frameLocator('#pf-frame').getByRole('heading', { name: 'Plans' })).toBeVisible();
  expect((await waitForLog({ event: 'access.ask' })).some(l => l.outcome === 'ok')).toBe(true);
  await memberContext.close();
});

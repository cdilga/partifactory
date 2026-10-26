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
  await page.getByRole('searchbox').fill('member-b');
  await page.getByRole('option', { name: /member-b/ }).click();
  await page.getByLabel('Role for new people').selectOption('view');
  await page.getByRole('button', { name: 'Add', exact: true }).click();
  await expect(page.locator('.pf-sentence')).toContainText('1 other can open this part');
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

import { test, expect, waitForLog } from './harness/fixtures';
import { signIn } from './lib/public-auth';
import { oauthToken } from './lib/agent-tokens';

/** The directory username a signed-in page's person has (the IdP may not send one). */
async function usernameOf(p: import('@playwright/test').Page) {
  return (await (await p.request.get('https://parts.pf.test/api/session')).json()).username as string;
}

// Comments (pf-h6y.4): a Comment member posts beside a part and the owner's open rail shows it
// within a second; a View member reads only; bodies stay text; an agent's reply is labelled.
test('household comments arrive live, stay inert and follow roles', async ({ page, browser }, info) => {
  test.skip(!['chromium-desktop', 'webkit-mobile', 'firefox-desktop'].includes(info.project.name), 'Fixed part hosts exist for these lanes');
  test.setTimeout(150000);
  const name = `comments-${info.project.name}`;
  const open = async (who: string) => {
    const context = await browser.newContext({ ignoreHTTPSErrors: true });
    const p = await context.newPage();
    await signIn(p, who);
    return { context, page: p };
  };
  const commenter = await open('member-b');
  const viewer = await open('member-a');
  await signIn(page, 'admin');
  const token = (await oauthToken(page, page.request, 'Comments gate', ['parts:read', 'parts:write', 'share:household', 'comments:write'])).access_token;
  const call = async (op: string, data: unknown) => {
    const r = await page.request.post(`/api/v1/${op}`, { headers: { Authorization: `Bearer ${token}` }, data });
    expect(r.status(), await r.text()).toBe(200);
    return r.json();
  };
  const created = await page.request.post('/api/v1/part_create', { headers: { Authorization: `Bearer ${token}` }, data: { name, title: `Plans ${info.project.name}`, kind: 'html', content: '<!doctype html><h1>Holiday plans</h1>', request_id: `${name}-${Date.now()}` } });
  expect([200, 409].includes(created.status()), await created.text()).toBeTruthy();
  await call('part_share', { name, action: 'set', subject: `user:${await usernameOf(commenter.page)}`, role: 'comment' });
  await call('part_share', { name, action: 'set', subject: `user:${await usernameOf(viewer.page)}`, role: 'view' });

  await page.goto(`https://${name}-part.pf.test/`);
  await page.getByRole('button', { name: /^Comments/ }).click();
  await expect(page.locator('#pf-comments')).toBeVisible();

  await commenter.page.goto(`https://${name}-part.pf.test/`);
  await commenter.page.getByRole('button', { name: /^Comments/ }).click();
  const marker = `Bring snacks ${Date.now()} <script>window.pwned=1</script>`;
  await commenter.page.getByLabel('Add a comment').fill(marker);
  const posted = Date.now();
  await commenter.page.getByRole('button', { name: 'Comment', exact: true }).click();
  // The owner's already-open rail shows it within a second, as text.
  await expect(page.locator('#pf-comments .pf-comment .body').filter({ hasText: 'Bring snacks' })).toHaveText(marker, { timeout: 1500 });
  expect(Date.now() - posted).toBeLessThan(2500);
  expect(await page.evaluate(() => (window as any).pwned)).toBeUndefined();
  await expect(page.locator('#pf-comments script')).toHaveCount(0);
  await page.screenshot({ path: info.outputPath('comments-live.png'), fullPage: true });

  // An agent replies for the admin; the commenter sees the labelled reply.
  const listed = await call('part_comments', { name });
  const thread = listed.threads.find((t: any) => t.comment.body === marker).id;
  await call('part_comments', { name, action: 'reply', id: thread, body: 'Added to the list.' });
  await expect(commenter.page.locator('#pf-comments .reply').filter({ hasText: 'Added to the list.' })).toContainText('Comments gate for The admin');

  // A View member reads but has no composer.
  await viewer.page.goto(`https://${name}-part.pf.test/`);
  await viewer.page.getByRole('button', { name: /^Comments/ }).click();
  await expect(viewer.page.locator('#pf-comments .pf-comment .body').filter({ hasText: 'Bring snacks' })).toBeVisible();
  await expect(viewer.page.getByLabel('Add a comment')).toBeHidden();
  await expect(viewer.page.locator('#pf-comments [role=status]')).toContainText('Ask the owner for Comment access');

  // Resolve from the owner's rail.
  await page.locator('#pf-comments .pf-thread').filter({ hasText: 'Bring snacks' }).getByRole('button', { name: 'Resolve', exact: true }).click();
  await expect(page.locator('#pf-comments details summary')).toContainText('Resolved');
  expect((await waitForLog({ event: 'comment.change' })).filter(l => l.outcome === 'ok').length).toBeGreaterThanOrEqual(3);
  await commenter.context.close();
  await viewer.context.close();
});

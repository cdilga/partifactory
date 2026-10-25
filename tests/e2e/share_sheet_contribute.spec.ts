import { test, expect, clickBudget, waitForLog } from './harness/fixtures';
import { signIn, nativePasskey } from './lib/public-auth';
import { devices } from '@playwright/test';

// One native admin credential lives for this entire flow. Guest traffic uses a separate,
// signed-out WebKit mobile context, including its real opaque-origin fetch metadata.
test('native admin share, mobile guest paint, freeze, undo and revoke', async ({ page, context, playwright, browserName }, info) => {
  test.skip(browserName !== 'chromium' || info.project.name !== 'chromium-desktop', 'Native virtual authenticator lane');
  test.setTimeout(90_000);
  await signIn(page);
  await nativePasskey(page, context);
  await page.goto('https://public-pixel-part.pf.test/');
  await expect(page.frameLocator('#pf-frame').locator('#b')).toBeVisible();
  const budget = clickBudget(page, 4, 'Can contribute + Copy');
  await budget.click(page.getByRole('button', { name: 'Share', exact: true }));
  await budget.click(page.getByRole('button', { name: 'Anyone with the link', exact: true }));
  budget.used++; await page.getByLabel('Link access').selectOption('contribute');
  const topNavigations: string[] = [];
  page.on('framenavigated', f => { if (f === page.mainFrame()) topNavigations.push(f.url()); });
  await budget.click(page.getByRole('button', { name: 'Copy link', exact: true }));
  const linkInput = page.getByLabel('Public link', { exact: true });
  await expect(linkInput).toBeVisible();
  const url = await linkInput.inputValue();
  expect(url).toMatch(/^https:\/\/public-pixel-part\.pf\.test\/l\/[A-Za-z0-9_-]{22}$/);
  budget.check(); expect(topNavigations).toHaveLength(0);
  await expect(page.locator('.pf-audience')).toHaveText('◉ Public link');
  await page.screenshot({ path: info.outputPath('share-contribute.png') });
  const bot = await page.request.get(url, { headers: { 'User-Agent': 'Slackbot' } });
  const human = await page.request.get(url, { headers: { 'User-Agent': 'Mozilla/5.0' } });
  expect(await bot.text()).toBe(await human.text());
  expect(await bot.text()).not.toMatch(/v1\.[0-9]+\.|\/go|http-equiv=.*refresh/);
  expect(bot.headers()['x-robots-tag']).toContain('noindex');

  const webkit = await playwright.webkit.launch();
  const guestContext = await webkit.newContext({ ...devices['iPhone 15'] });
  try {
    const guest = await guestContext.newPage();
    await guest.goto(url);
    const guestBoard = guest.frameLocator('#pf-frame');
    await expect(guestBoard.locator('#b')).toBeVisible();
    await guestBoard.locator('[data-key="px:0:0"]').click();
    await expect(page.frameLocator('#pf-frame').locator('[data-key="px:0:0"]')).toHaveCSS('background-color', 'rgb(228, 87, 46)');
    await guest.screenshot({ path: info.outputPath('guest-painted.png') });
    const runtime = guest.frames().find(f => f.url().includes('/c/'))!;
    expect(await runtime.evaluate(async () => {
      const comments=await fetch('/_rt/v1',{method:'POST',headers:{'Content-Type':'text/plain'},body:JSON.stringify({op:'comments.list',args:{}})});
      const api=await fetch('/api/private');
      return [comments.status,api.status];
    })).toEqual([403,403]);
    // Fresh per-open guest identity, then one sub-second burst. This observes the browser's
    // real 61st request and Retry-After header without waiting for a minute boundary.
    await guest.reload();
    await expect(guestBoard.locator('#b')).toBeVisible();
    const refusals: string[]=[];
    guest.on('response',r=>{if(r.status()===429)refusals.push(r.headers()['retry-after']);});
    const burst=await guest.frames().find(f=>f.url().includes('/c/'))!.evaluate(async()=>{
      const rows=await Promise.allSettled(Array.from({length:61},(_,i)=>(window as any).storage.set('rate:'+i,'blue',true)));
      return {ok:rows.filter(r=>r.status==='fulfilled').length,failed:rows.filter(r=>r.status==='rejected').length};
    });
    expect(burst).toEqual({ok:60,failed:1});
    await expect.poll(()=>refusals.length).toBe(1);expect(Number(refusals[0])).toBeGreaterThan(0);
    expect((await waitForLog({event:'ratelimit.trip'})).some(l=>l.outcome==='rate_limited')).toBe(true);
    await guest.reload();
    await expect(guestBoard.locator('#b')).toBeVisible();
    await page.getByRole('button', { name: 'Freeze', exact: true }).click();
    await guestBoard.locator('[data-key="px:1:0"]').click();
    await expect(guestBoard.locator('#s')).not.toHaveText('');
    await expect(page.frameLocator('#pf-frame').locator('[data-key="px:1:0"]')).not.toHaveCSS('background-color', 'rgb(228, 87, 46)');
    // A fresh go is now view-only and yields the explicit data policy message.
    await guest.reload();
    await guestBoard.locator('[data-key="px:1:0"]').click();
    await expect(guestBoard.locator('#s')).toHaveText('This board is view only.');
    await page.getByRole('button', { name: 'Undo', exact: true }).click();
    await guest.reload();
    await guestBoard.locator('[data-key="px:1:0"]').click();
    await expect(page.frameLocator('#pf-frame').locator('[data-key="px:1:0"]')).toHaveCSS('background-color', 'rgb(228, 87, 46)');
    await page.getByRole('button', { name: 'Revoke', exact: true }).click();
    expect((await guestContext.request.get(url)).status()).toBe(404);
    await expect(page.locator('.pf-audience')).toHaveText('Only me');
    expect((await waitForLog({ event: 'link.go' })).some(l => l.outcome === 'ok')).toBe(true);
    expect((await waitForLog({ event: 'epoch.bump' })).some(l => String(l.key).startsWith('minter:link:'))).toBe(true);
  } finally { await guestContext.close(); await webkit.close(); }
});

import { test, expect } from './harness/fixtures';
import * as fs from 'node:fs';
import * as path from 'node:path';

const fixtures = () => JSON.parse(fs.readFileSync(process.env.PF_PART_FIXTURE ?? '/run/pf/run/part-fixture.json', 'utf8'));

test('authored examples load, comments persist, and two pilots complete Orbit Relay together', async ({ page, browser }, info) => {
  test.skip(info.project.name !== 'chromium-desktop', 'The authored examples use one isolated cooperative board per run.');
  test.setTimeout(60_000);
  const evidence = path.resolve('../../target/e2e-logs', process.env.PF_E2E_RUN ?? 'local', 'examples');
  fs.mkdirSync(evidence, { recursive: true });
  const otherContext = await browser.newContext({ viewport: { width: 390, height: 844 } });
  const other = await otherContext.newPage();
  try {
    await page.goto(fixtures()['example-about'].url);
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
    await page.screenshot({ path: path.join(evidence, 'about.png'), fullPage: true });
    await Promise.all([page.goto(fixtures()['example-workshop'].url), other.goto(fixtures()['example-workshop'].url)]);
    await Promise.all([page.waitForFunction(() => !!(window as any).__pfReadyAt), other.waitForFunction(() => !!(window as any).__pfReadyAt)]);
    const idea = 'When we pack for a weekend, a shared checklist would help.\n<script>literal text stays safe</script>';
    await page.locator('#alias').fill('A household member');
    await page.locator('#message').fill(idea);
    await page.getByRole('button', { name: 'Add to the workbench' }).click();
    await expect(page.locator('#status')).toContainText('Added.');
    await expect(other.locator('.comment .body')).toHaveText(idea);
    await other.reload();
    await expect(other.locator('.comment .body')).toHaveText(idea);
    await expect(other.locator('.comment script')).toHaveCount(0);
    await other.screenshot({ path: path.join(evidence, 'workshop-mobile.png'), fullPage: true });
    await Promise.all([page.goto(fixtures()['example-orbit-relay'].url), other.goto(fixtures()['example-orbit-relay'].url)]);
    await Promise.all([page.waitForFunction(() => !!(window as any).__pfReadyAt), other.waitForFunction(() => !!(window as any).__pfReadyAt)]);
    await expect(page.locator('#connection')).toContainText('Live');
    await expect(other.locator('#pilots')).toHaveText('2 pilots on the relay');
    const solution: [number, number, number][] = [[3,3,1],[3,1,1],[8,1,2],[8,4,2],[10,4,1],[10,1,1],[7,5,1],[7,6,1]];
    for (const [i, [x,y,turns]] of solution.entries()) {
      const author = i % 2 ? other : page;
      const observer = i % 2 ? page : other;
      const selector = `[data-cell="${x},${y}"]`;
      for (let turn=0; turn<turns; turn++) {
        await author.locator(selector).click();
        await expect(author.locator('#message')).toContainText('Mirror saved.');
        await expect(observer.locator(selector + ' .mir')).toHaveText(turn === 0 ? '/' : '\\');
      }
    }
    await expect(page.locator('#celebration')).toBeVisible();
    await expect(other.locator('#celebration')).toBeVisible();
    await other.reload();
    await expect(other.locator('#celebration')).toBeVisible();
    await page.screenshot({ path: path.join(evidence, 'orbit-relay.png'), fullPage: true });
    await other.screenshot({ path: path.join(evidence, 'orbit-relay-mobile.png'), fullPage: true });
    console.log('example.check about=pass comments_live_persist_safe=pass two_pilots_solved_persisted=pass');
  } finally { await otherContext.close(); }
});

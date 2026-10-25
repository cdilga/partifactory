import { test, expect, waitForLog } from './harness/fixtures';
import * as fs from 'node:fs';
const fixtures = () => JSON.parse(fs.readFileSync(process.env.PF_PART_FIXTURE ?? '/run/pf/run/part-fixture.json', 'utf8'));

test('Markdown uses the shared shell with GFM rendering', async ({page}) => {
  await page.goto(fixtures().markdown.url);
  await expect(page.locator('h1')).toHaveText('A working Markdown part');
  await expect(page.locator('del')).toHaveText('gone');
  await expect(page.locator('table td')).toHaveCount(2);
  expect(await page.locator('link[rel=stylesheet]').getAttribute('href')).toContain('https://blobs-part.pf.test/b/');
});

test('text preserves exact characters, CRLF, BOM and line anchors', async ({page}) => {
  await page.goto(fixtures().text.url);
  expect(await page.locator('#pf-text').textContent()).toBe('\uFEFF<tag>\r\n& second\n');
  await expect(page.locator('#L1')).toHaveCount(1);
  await expect(page.locator('#L3')).toHaveCount(1);
  await page.getByRole('button', {name:'Toggle monospace'}).click();
  await expect(page.locator('#pf-text')).toHaveClass('pf-proportional');
});

test('a files bundle loads entry assets and split modules', async ({page}) => {
  await page.goto(fixtures().bundle.url);
  await expect(page.locator('body')).toHaveAttribute('data-bundle','loaded');
});

test('mirrored modules, transitive CSS and media use only the two part origins', async ({page}) => {
  const requests:string[]=[];
  page.on('request',r=>requests.push(r.url()));
  await page.goto(fixtures().mirrored.url);
  await expect.poll(()=>page.evaluate(()=>(window as any).mirroredValue)).toBe(42);
  expect(await page.evaluate(()=>(window as any).unknownMirrored)).toBe(true);
  await expect(page.locator('#mirrored')).toHaveCSS('color','rgb(12, 34, 56)');
  expect(requests.filter(u=>/^https?:/.test(u)).map(u=>new URL(u).hostname).every(h=>['mirrored-part.pf.test','blobs-part.pf.test'].includes(h))).toBe(true);
  expect((await waitForLog({event:'blobhost.serve'})).length).toBeGreaterThan(0);
});

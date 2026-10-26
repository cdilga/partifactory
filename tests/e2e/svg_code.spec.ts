import { test, expect, waitForLog } from './harness/fixtures';
import { signIn } from './lib/public-auth';
import * as fs from 'node:fs';

const golden = (p: string) => fs.readFileSync(`/repo/tests/golden/render/${p}`, 'utf8');

// Both kinds go through the real op registry with a real connection, then open in the browser.
test('svg runs only in the opaque sandbox and code stays literal with working line anchors', async ({ page }, info) => {
  test.skip(!['chromium-desktop', 'webkit-mobile'].includes(info.project.name), 'Desktop and mobile WebKit lanes');
  test.setTimeout(120000);
  await signIn(page, 'member-a');
  const token = await page.evaluate(async () => {
    const session = await (await fetch('/api/session')).json();
    const response = await fetch('/api/agents/personal', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ agent_name: 'Kinds gate', scopes: ['parts:read', 'parts:write'], spaces: [`~${session.username}`], lifetime_days: 30 }) });
    if (!response.ok) throw new Error(await response.text());
    return (await response.json()).token as string;
  });
  const op = async (name: string, body: unknown) => {
    const response = await page.request.post(`/api/v1/${name}`, { headers: { Authorization: `Bearer ${token}` }, data: body });
    expect(response.status(), await response.text()).toBe(200);
    return response.json();
  };
  // Fixed per-project names: WebKit resolves only the part hosts the harness lists.
  const run = info.project.name;

  // The SVG also tries to reach another origin; the canonical CSP must refuse it.
  const svgSource = golden('svg/drawing.svg').replace('</svg>', `<script><![CDATA[fetch('https://example.com/').then(()=>document.documentElement.setAttribute('data-net','reached'),()=>document.documentElement.setAttribute('data-net','refused'));]]></script></svg>`);
  const svg = await op('part_create', { title: `SVG ${run}`, name: `svg-${run}`, kind: 'svg', content: svgSource, request_id: `svg-${run}` });
  const codeSource = golden('code/hostile.rs');
  const code = await op('part_create', { title: `Code ${run}`, name: `code-${run}`, files: { 'hostile.rs': codeSource }, request_id: `code-${run}` });
  for (const name of [svg.name, code.name]) expect((await op('part_check', { name })).build.status).toBe('ok');

  await page.goto(svg.url);
  const drawing = page.frameLocator('#pf-frame').locator('svg');
  await expect(drawing).toHaveAttribute('data-ran', 'yes');
  await expect(drawing).toHaveAttribute('data-net', 'refused');
  // An opaque origin: cookies throw rather than exist.
  expect(await drawing.evaluate(() => { try { document.cookie; return [self.origin, 'readable']; } catch (e) { return [self.origin, (e as Error).name]; } })).toEqual(['null', 'SecurityError']);
  expect(await drawing.evaluate(() => document.querySelector('[id^=pf-]'))).toBeNull();

  await page.goto(code.url);
  const viewer = page.frameLocator('#pf-frame');
  await expect(viewer.locator('#pf-code')).toBeVisible();
  const lines = codeSource.split(/(?<=\n)/);
  for (const [i, line] of lines.entries()) expect(await viewer.locator(`#L${i + 1}`).textContent()).toBe(line);
  expect(await viewer.locator('#pf-code').evaluate(el => [el.querySelectorAll('script').length, (window as any).injected])).toEqual([0, undefined]);
  await expect(viewer.locator('#pf-code')).toContainText('<script>window.injected = true</script> & λ');
  const before = await viewer.locator('#L3').evaluate(el => getComputedStyle(el).backgroundColor);
  await viewer.locator('#L3').evaluate(() => { location.hash = '#L3'; });
  await expect.poll(() => viewer.locator('#L3').evaluate(el => getComputedStyle(el).backgroundColor)).not.toBe(before);
  await page.screenshot({ path: info.outputPath('code-viewer.png'), fullPage: true });

  const builds = await waitForLog({ event: 'build.finished' });
  for (const kind of ['svg', 'code']) expect(builds.some(l => l.kind === kind && l.outcome === 'ok')).toBe(true);
  expect((await waitForLog({ event: 'serve.entry' })).length).toBeGreaterThan(0);
});

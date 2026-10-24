// Drives the spike runner on the five Playwright projects. Expects server.mjs to be running
// (run.sh starts it). Usage: node run.mjs [--lanes a,b] [--items 1,2] [--hold 75] [--proxies 1]
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium, webkit, firefox, devices } from 'playwright';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const OUT = process.env.SPIKE_OUT || path.resolve(HERE, '../../target/sandbox-spike');
const HTTP_PORT = +(process.env.SPIKE_HTTP_PORT || 8080);
const PROXY_PORT = +(process.env.SPIKE_PROXY_PORT || 3128);
const args = Object.fromEntries(process.argv.slice(2).reduce((a, v, i, all) => (v.startsWith('--') ? [...a, [v.slice(2), all[i + 1]]] : a), []));

const LANES = {
  'chromium-desktop': { type: chromium, ctx: { viewport: { width: 1280, height: 900 } }, virt: true },
  'chromium-mobile': { type: chromium, ctx: { ...devices['Pixel 7'] }, virt: true },
  'webkit-desktop': { type: webkit, ctx: { viewport: { width: 1280, height: 900 } } },
  'webkit-mobile': { type: webkit, ctx: { ...devices['iPhone 15'] } },
  'firefox-desktop': { type: firefox, ctx: { viewport: { width: 1280, height: 900 } } },
};
const lanes = (args.lanes || Object.keys(LANES).join(',')).split(',');
const spki = fs.readFileSync(path.join(OUT, 'certs/spki.txt'), 'utf8').trim();
fs.mkdirSync(path.join(OUT, 'screens'), { recursive: true });

async function post(lane, results, ua) {
  await fetch(`http://127.0.0.1:${HTTP_PORT}/_spike/result`, {
    method: 'POST',
    body: JSON.stringify({ type: 'spike:results', lane, run: 'playwright-observer', ua, results }),
  });
}

async function runLane(lane) {
  const L = LANES[lane];
  const isChromium = L.type === chromium;
  const browser = await L.type.launch({
    proxy: { server: `http://127.0.0.1:${PROXY_PORT}` },
    // Chromium: trust the rig leaf by SPKI (a real "valid cert", so WebAuthn works). The other
    // engines have no such switch, so they use ignoreHTTPSErrors.
    args: isChromium ? [`--ignore-certificate-errors-spki-list=${spki}`] : [],
  });
  // Chromium: grant clipboard so the check measures permissions-policy delegation, not headless focus.
  const context = await browser.newContext({ ...L.ctx, ignoreHTTPSErrors: !isChromium, ...(isChromium ? { permissions: ['clipboard-read', 'clipboard-write'] } : {}) });
  const page = await context.newPage();
  const consoleLog = [];
  page.on('console', (m) => consoleLog.push(`[${m.type()}] ${m.text()}`));
  page.on('pageerror', (e) => consoleLog.push(`[pageerror] ${e.message}`));
  if (L.virt) {
    const cdp = await context.newCDPSession(page);
    await cdp.send('WebAuthn.enable');
    await cdp.send('WebAuthn.addVirtualAuthenticator', { options: { protocol: 'ctap2', transport: 'internal',
      hasResidentKey: true, hasUserVerification: true, isUserVerified: true, automaticPresenceSimulation: true } });
  }
  // Activation-gated items: click the buttons inside the item frame (item 6) and observe
  // downloads (own-host <a download> name vs Content-Disposition on ?dl=1).
  const handled = new Set();
  page.on('framenavigated', async (frame) => {
    const u = frame.url();
    try {
      if (/\/items\/6\.html/.test(u) && !handled.has(u)) {
        handled.add(u);
        for (const id of ['act-clip', 'act-fs', 'act-share', 'act-pop']) {
          await frame.locator('#' + id).click({ timeout: 6000 });
          await page.waitForTimeout(700);
        }
      }
    } catch (e) {
      consoleLog.push(`[runner] ${u}: ${e.message.split('\n')[0]}`);
    }
  });
  context.on('page', (p) => p.on('console', (m) => consoleLog.push(`[popup ${m.type()}] ${m.text()}`)));

  const q = new URLSearchParams({ lane, run: `${lane}-${Date.now().toString(36)}`, wa: L.virt ? 'virt' : 'none',
    actwait: '10', hold: args.hold || '75', proxies: args.proxies || '1' });
  if (args.items) q.set('items', args.items);
  const t0 = Date.now();
  await page.goto(`https://spike.pf.test/run?${q}`);
  let ok = true;
  try {
    await page.waitForFunction(() => document.title === 'spike:done' || document.title === 'spike:error', null, { timeout: 8 * 60 * 1000, polling: 1000 });
    if ((await page.title()) !== 'spike:done') ok = false;
  } catch (e) {
    ok = false;
    consoleLog.push(`[runner] timeout: ${e.message.split('\n')[0]}`);
  }
  await page.screenshot({ path: path.join(OUT, 'screens', `${lane}.png`), fullPage: true }).catch(() => {});
  if (!args.items || args.items.split(',').includes('dl')) await downloads(lane, context, consoleLog);
  const ua = await page.evaluate(() => navigator.userAgent).catch(() => '');
  const meta = { lane, browser: L.type.name(), version: browser.version(), ua, os: `${os.type()} ${os.release()} (${os.arch()})`,
    playwright: JSON.parse(fs.readFileSync(path.join(HERE, 'node_modules/playwright/package.json'), 'utf8')).version,
    device: L.ctx.userAgent ? Object.keys(devices).find((k) => devices[k] === L.ctx || devices[k].userAgent === L.ctx.userAgent) : 'desktop',
    seconds: Math.round((Date.now() - t0) / 1000), finished: ok };
  fs.writeFileSync(path.join(OUT, `console-${lane}.log`), consoleLog.join('\n'));
  await browser.close();
  console.log(`lane ${lane}: ${ok ? 'done' : 'DID NOT FINISH'} in ${meta.seconds}s`);
  return meta;
}

// Known loss: own-host <a download> names. The page is opened top-level (still sandboxed, so
// still an opaque origin) and the links are clicked here, where download events are visible.
async function downloads(lane, context, consoleLog) {
  const page = await context.newPage();
  const results = [];
  const base = `https://a-part.pf.test/c/v1.spike.dl-${lane}/items/dl.html?lane=${lane}&observer=1`;
  for (const [id, check, ok] of [
    ['dl2', 'dl1_content_disposition_name', (n) => n === 'pf-report.txt'],
    ['dl1', 'download_attr_name_lost', (n) => n !== 'nice-name.txt'],
  ]) {
    try {
      await page.goto(base);
      const dlP = page.waitForEvent('download', { timeout: 6000 }).then((d) => d.suggestedFilename(), () => null);
      await page.locator('#' + id).click();
      const name = await dlP;
      if (name === null) {
        const where = page.url();
        results.push({ item: 'L', check, outcome: check === 'download_attr_name_lost' && /dl\/file\.txt/.test(where) ? 'pass' : 'fail',
          detail: `no download; the link navigated to ${where.replace(/v1\.[^/]+/, '<cap>')} (download attribute ignored)` });
      } else {
        results.push({ item: 'L', check, outcome: ok(name) ? 'pass' : 'fail', detail: `suggested filename: ${name}` });
      }
    } catch (e) {
      results.push({ item: 'L', check, outcome: 'fail', detail: e.message.split('\n')[0] });
      consoleLog.push(`[downloads] ${e.message}`);
    }
  }
  await post(lane, results, await page.evaluate(() => navigator.userAgent).catch(() => ''));
  await page.close();
}

const metas = await Promise.all(lanes.map((l) => runLane(l).catch((e) => ({ lane: l, error: String(e), finished: false }))));
const lanesFile = path.join(OUT, 'lanes.json');
const prev = fs.existsSync(lanesFile) ? JSON.parse(fs.readFileSync(lanesFile, 'utf8')) : {};
for (const m of metas) prev[m.lane] = m;
fs.writeFileSync(lanesFile, JSON.stringify(prev, null, 2));
if (metas.some((m) => !m.finished)) {
  console.error('some lanes did not finish:', metas.filter((m) => !m.finished).map((m) => m.lane + (m.error ? ': ' + m.error : '')).join('; '));
  process.exit(1);
}

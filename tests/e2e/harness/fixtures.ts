// Shared fixtures for every browser e2e test. Import `test` and `expect` from here, not from
// @playwright/test:
//   - every response's request id is recorded; on failure the ids are printed and the server's
//     JSONL lines for them are written next to the trace, screenshot and HAR in the test's
//     output dir (target/e2e-logs/<run>/<test>/);
//   - `clickBudget(page, n)` counts the clicks and taps of a scripted path and fails the test
//     when the path takes more than its budget (§3a, §4a.7);
//   - `serverLog()` / `waitForLog()` read the server's log lines (for asserting on them).
import { test as base, expect, type Locator, type Page } from '@playwright/test';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';

export type LogLine = Record<string, unknown> & { event?: string; request_id?: string };

/**
 * Server log lines (already redacted), filtered by request id and/or event. Read through the
 * dev-only `/_dev/logs` route (the server keeps a ring with PF_DEV_LOG_RING), because a
 * bind-mounted log file goes stale on Docker Desktop.
 */
export async function serverLog(filter: { request_id?: string; event?: string } = {}): Promise<LogLine[]> {
  const q = new URLSearchParams(filter as Record<string, string>).toString();
  try {
    const r = await fetch(`https://parts.pf.test/_dev/logs?${q}`);
    return r.ok ? ((await r.json()) as LogLine[]) : [];
  } catch {
    return [];
  }
}

/** Server log lines matching `filter`, retried until there is at least one (the access line lands after the response). */
export async function waitForLog(
  filter: { request_id?: string; event?: string },
  timeoutMs = 5000,
): Promise<LogLine[]> {
  const end = Date.now() + timeoutMs;
  for (;;) {
    const hits = await serverLog(filter);
    if (hits.length || Date.now() > end) return hits;
    await new Promise((r) => setTimeout(r, 100));
  }
}

export class ClickBudget {
  used = 0;
  constructor(
    readonly page: Page,
    readonly budget: number,
    readonly label = 'path',
  ) {}
  async click(target: Locator) {
    this.used++;
    await target.click();
  }
  async tap(target: Locator) {
    this.used++;
    await target.tap();
  }
  /** Throws when the path took more clicks than its budget. */
  check() {
    if (this.used > this.budget) {
      throw new Error(`click budget exceeded for ${this.label}: ${this.used} clicks > budget ${this.budget}`);
    }
  }
}

export function clickBudget(page: Page, budget: number, label?: string) {
  return new ClickBudget(page, budget, label);
}

type Fixtures = { requestIds: string[] };

export const test = base.extend<Fixtures>({
  contextOptions: async ({ contextOptions }, use, testInfo) => {
    await use({ ...contextOptions, recordHar: { path: testInfo.outputPath('network.har'), mode: 'minimal' } });
  },
  // Firefox trusts the run's CA through an NSS profile (cert9.db, made by up.sh with certutil),
  // so its tests run in a persistent context on a fresh copy of that profile.
  context: async ({ context, browserName, playwright, contextOptions, viewport, userAgent }, use) => {
    const profile = process.env.PF_FIREFOX_PROFILE;
    if (browserName !== 'firefox' || !profile || !fs.existsSync(profile)) {
      await use(context);
      return;
    }
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'pf-ff-'));
    fs.cpSync(profile, dir, { recursive: true });
    const ctx = await playwright.firefox.launchPersistentContext(dir, {
      ...contextOptions,
      viewport,
      userAgent,
      baseURL: 'https://parts.pf.test',
    });
    await use(ctx);
    await ctx.close();
    fs.rmSync(dir, { recursive: true, force: true });
  },
  requestIds: [
    async ({ page }, use, testInfo) => {
      const ids: string[] = [];
      page.on('response', (r) => {
        const id = r.headers()['x-request-id'];
        if (id) ids.push(id);
      });
      await use(ids);
      if (testInfo.status !== testInfo.expectedStatus) {
        const failing = [...new Set(ids)];
        console.log(`[${testInfo.title}] request ids: ${failing.join(' ') || '(none)'}`);
        const lines = (await Promise.all(failing.map((id) => serverLog({ request_id: id })))).flat();
        fs.writeFileSync(testInfo.outputPath('server.jsonl'), lines.map((l) => JSON.stringify(l)).join('\n') + '\n');
      }
    },
    { auto: true },
  ],
});

export { expect };

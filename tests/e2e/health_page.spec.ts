import { test, expect } from './harness/fixtures';
import { signIn } from './lib/public-auth';

test('admin Health exposes readiness, backup layers, budget and every capped store', async ({ page }, info) => {
  await signIn(page);
  const response = await page.goto('/admin/health');
  expect(response?.status()).toBe(200);
  await expect(page.getByRole('heading', { name: 'Health', exact: true })).toBeVisible();
  for (const name of ['Ready', 'db', 'blobs', 'idp', 'directory', 'backup', 'Host snapshots', 'Hourly database copy', 'Newest export', 'Same disk', 'Last drill', 'Disk budget', 'exports', 'events', 'runtime_errors', 'build_logs', 'audit', 'idempotency', 'data_snapshots', 'previews', 'data_docs', 'shared_blobs', 'mirror_cache', 'quota_hits', 'Scheduled exports', 'Break-glass uses', '5xx responses', 'Version']) {
    await expect(page.locator(`[data-health-row="${name}"]`).first()).toBeVisible();
  }
  await expect(page.locator('[data-health-row="Same disk"]')).toContainText('amber');
  if (process.env.PF_IDP_DOWN === '1') await expect(page.locator('[data-health-row="idp"]')).toContainText('amber');
  await page.screenshot({ path: info.outputPath(process.env.PF_IDP_DOWN === '1' ? 'admin-health-idp-down.png' : 'admin-health.png'), fullPage: true });
  const wrongOrigin = await page.request.post('/admin/health/run-now', { headers: { Origin: 'https://notes-part.pf.test' }, form: { store: 'exports' } });
  expect(wrongOrigin.status()).toBe(403);
});

test('Health requires the pinned administrator, including for read access', async ({ page }) => {
  await signIn(page, 'member-a');
  const response = await page.goto('/admin/health');
  expect(response?.status()).toBe(403);
});

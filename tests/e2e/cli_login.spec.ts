import { readFileSync, existsSync } from 'node:fs';
import { test, expect } from './harness/fixtures';
import { signIn } from './lib/public-auth';

// The host driver runs the actual native binary while this browser supplies the person.
// Ordinary full-suite runs omit the public approval file and exercise device_grant.spec.ts.
test('approve the native CLI device connection', async ({ page }, info) => {
  const approvalFile = `/run/pf/run/cli-approval-${info.project.name}.txt`;
  test.skip(!existsSync(approvalFile), 'Run through tests/device/cli-login.py with a native CLI');
  const approval = readFileSync(approvalFile, 'utf8').trim();
  expect(approval).toMatch(/^https:\/\/parts\.pf\.test\/device\?code=[BCDFGHJKLMNPQRSTVWXZ]{4}-[BCDFGHJKLMNPQRSTVWXZ]{4}$/);
  await signIn(page, 'member-a');
  await page.goto(approval);
  await page.getByRole('button', { name: 'Continue', exact: true }).click();
  await expect(page.getByRole('heading', { name: `This device says it is Native CLI ${info.project.name}` })).toBeVisible();
  await page.getByRole('button', { name: 'Allow', exact: true }).click();
  await expect(page.getByRole('status')).toHaveText('Agent connected. You can return to your terminal.');
  await page.screenshot({ path: info.outputPath('native-cli-consent.png') });
  console.log(`Native CLI approval: ${info.project.name}, browser ${page.context().browser()?.version()}`);
});

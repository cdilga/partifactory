// Playwright for the e2e harness (pf-kurh). Runs inside the harness's Playwright container
// (ci/e2e.sh), where *.pf.test resolves to the harness Caddy and the run's CA is trusted:
//   - Chromium: --host-resolver-rules for the wildcard, and the run CA's key trusted through
//     --ignore-certificate-errors-spki-list (scoped to that one key; never
//     --ignore-certificate-errors, never ignoreHTTPSErrors);
//   - WebKit: the CA in the container's system store (update-ca-certificates), /etc/hosts;
//   - Firefox: the CA installed by an enterprise policy (Certificates.Install), /etc/hosts.
// Artifacts (trace, screenshots, HAR, the matching server JSONL) are kept only for failures,
// under target/e2e-logs/<run>/<test>/.
import { defineConfig, devices } from '@playwright/test';

const spki = process.env.PF_CA_SPKI ?? '';
const harnessIp = process.env.PF_HARNESS_IP ?? '172.30.77.10';
const chromiumArgs = [
  `--host-resolver-rules=MAP *.pf.test ${harnessIp}, EXCLUDE localhost`,
  ...(spki ? [`--ignore-certificate-errors-spki-list=${spki}`] : []),
];
const run = process.env.PF_E2E_RUN ?? 'local';

export default defineConfig({
  testDir: '.',
  testMatch: ['harness/**/*.spec.ts', '*.spec.ts'],
  outputDir: `../../target/e2e-logs/${run}`,
  preserveOutput: 'failures-only',
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: 0,
  workers: process.env.CI ? 2 : 4,
  timeout: 30_000,
  reporter: [['list'], ['json', { outputFile: `../../target/e2e-logs/${run}/results.json` }]],
  use: {
    baseURL: 'https://parts.pf.test',
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    video: 'off',
  },
  projects: [
    {
      name: 'chromium-desktop',
      use: { ...devices['Desktop Chrome'], launchOptions: { args: chromiumArgs } },
    },
    {
      name: 'chromium-mobile',
      use: { ...devices['Pixel 7'], launchOptions: { args: chromiumArgs } },
    },
    {
      name: 'webkit-mobile',
      use: { ...devices['iPhone 15'] },
    },
    {
      name: 'firefox-desktop',
      use: { ...devices['Desktop Firefox'] },
    },
  ],
});

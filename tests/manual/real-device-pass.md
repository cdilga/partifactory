# Optional real-device pass

These checks belong to the `human-only` bead. They never block simulated-device gates or a release. Run them when convenient on an iPhone with Safari, an Android phone with Chrome, and Telegram's in-app browser. Use a fixture part with harmless content.

For each browser:

1. Sign in through the configured identity provider. Where available, complete passkey step-up.
2. Open the part's own host link. Verify its content on the first open, then reload it.
3. Open its standalone content URL and verify it loads. Keep that URL out of records and screenshots because it contains a capability.
4. Open a public `/l/` link in a signed-out browser and verify guest content appears without signing in.
5. If embedding is enabled, open the allowed embedding page and verify its part loads.
6. Accept a disposable invitation when that feature is available; otherwise record `not_applicable`.
7. On Telegram, spend up to five minutes checking sign-in and passkey behavior. On failure, apply [both decided fixes](../../docs/ops/telegram.md), then repeat in the normal browser.

Record only generic device and software versions, item outcomes and a date. Do not record people, deployment domains, internal addresses, tokens, cookies, capability URLs or public-link secrets. Screenshots must hide the address bar and personal content. A failure becomes a `bug` labelled `found-in:<human-checks-bead>`; it blocks no work unless separately triaged.

Optional results live in `tests/manual/results/*.json`. No result file is required. Validate existing files with `scripts/check_manual_records.sh`. Example schema (replace versions and date with the observed values):

```json
{
  "schema": 1,
  "recorded_at": "2026-10-25T00:00:00Z",
  "device": "iPhone",
  "os": "iOS <version>",
  "browser": "Safari <version>",
  "checks": {
    "sign_in": "pass",
    "passkey_step_up": "pass",
    "part_root_first_open": "pass",
    "part_root_reload": "pass",
    "standalone": "pass",
    "guest_link": "pass",
    "embedding": "not_applicable",
    "invite_acceptance": "not_applicable",
    "telegram_sign_in": "not_applicable"
  }
}
```

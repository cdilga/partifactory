# AGENTS.md

## What this is

partifactory: a self-hosted home for the pages, tools, docs and small apps a household and its agents make. Each one is a **part** with its own sandboxed host.
- `PLAN.md` is the design and the source of truth. §6a is the build order, and the "Slice 1" section says what ships first.
- `docs/agent-contract.md` is the agent-facing contract. It is served as `/skill.md`, so keep it in step with the op registry.
- Tasks live only in beads (`br`, beads_rust), never in `PLAN.md`.

## Finding work

```bash
br ready --exclude-label human-only       # open, unblocked work, best first
br show <id>                              # goal, scope, plan refs, tests, verification gate
br update <id> --status in_progress       # claim it
```

- Each bead is a chunky, vertical piece of work. Read the whole bead and the plan sections it cites before you start.
- **Never claim a bead labelled `human-only`.** Those are the admin's own checks on real phones and the example deployment. They block nothing, nothing may ever depend on them, and the swarm's gates use emulated and simulated devices instead (Playwright's mobile projects, the virtual authenticator, Mobile Safari in the iOS Simulator). For a `human-only` bead, `ci_green` means "CI was green on the commit tested". A failure the admin finds there arrives as a `bug` bead labelled `found-in:<that bead>`.
- The beads labelled `later` are phase epics. Each says "Not broken down yet": breaking one into beads is a separate conversion step that the admin starts. Don't do that work ad hoc.

## How we swarm (the admin's rules, binding)

- **No git worktrees and no feature branches.** Every agent works directly on `main`, in the one checkout. Full send.
- **Coordinate through Agent Mail** (the `mcp-agent-mail` server):
  1. at session start, register with `macro_start_session`;
  2. use the bead id as the thread id;
  3. post in that thread when you start the bead and when you finish it.
- **Advisory file locks.** Before you edit anything, call `file_reservation_paths` on the files or globs you'll touch, with the bead id as the reason. Release them (`release_file_reservations`) when you're done.
  - Respect other agents' reservations. If a path you need is held, message the holder, or pick other work.
- **Install the Agent Mail pre-commit guard** (`install_precommit_guard`) in this checkout. It blocks commits that touch paths someone else holds.
- **Commit small and often.** Run `git pull --rebase` before every push.
  - Never force-push, and never rewrite other agents' work.
  - If a rebase conflicts in a file someone else holds, stop and message them.
- **`main` must stay green.** If you break CI, fixing it is your next task, ahead of everything else.

## Closing a bead

A bead is done when its **Verification gate** has been observed, not when its unit tests pass. The gate is the last section of the description, and it is mirrored as the `GATE:` lines at the end of the acceptance criteria.
- **See it run for real.** Watch the outcome the gate names on the binary built from `main`, running in the e2e harness (Dex + Caddy + `pf.test`) or in the compose deployment.
- **CI is green on `main`** with the bead's tests in it.
- **The logs are checked.** The e2e run asserts on the log lines or metrics the gate names.
- **Simulator lanes run on the Mac.** A gate that names Mobile Safari in the iOS Simulator (`tests/device/ios-sim.sh`) can only be observed on the Mac, where the iOS runtime is installed, so claim those beads from a session there.
- **Nothing regressed.** The full e2e suite passes, including every earlier bead's gate tests. Gates accumulate, so once a gate test lands it runs on every PR.
- **Close with evidence.** br enforces this: `.beads/policy.yaml` won't let any bead move to `closed` until the gates `ci_green` and `verified` have passed, and `--bypass-policy` is turned off. Gate results apply to the bead's current status, so record them after your last status change, right before you close:

```bash
br gate report <id> --gate ci_green --provider ci --status pass --note "<CI run id or link>"
br gate report <id> --gate verified --provider <your agent name> --status pass --note "<what you observed>"
br close <id> --reason "<commit SHAs>; CI <run>; <gate commands + output excerpt>; <screenshot path or device + browser versions for UI/device gates>"
```

  A bead closed without that evidence gets reopened.
- **Beads are only edited after conversion, never created mid-task.** The one exception is a defect you find while working: you may file it as a `bug` bead, labelled `found-in:<bead id>` (`br create "..." -t bug -l found-in:<id>`), and link it with `br dep add` if it blocks your bead. If you see work missing from a bead, edit that bead or tell the admin; don't create a new one.

## Disk hygiene (binding: builds have repeatedly filled the admin's disk)

- **Check before you build.** Run `scripts/disk-guard.sh` (until pf-tokl lands it, `df -h .`). If the volume has less than 20 GiB free, or `target/` is over 30 GiB, run the cleanup below first. If that still isn't enough, stop and tell the admin. Never delete anything outside the repo to make room.
- **One `target/`, shared.** Never set your own `CARGO_TARGET_DIR`, and never add sccache or another cache. Build tests with the one documented feature set (`--features dev`), so dependencies build once.
- **Release builds only when needed.** Use `--release` and the musl target only for a gate, a bench or the image.
- **Clean as you go, never all at once.** After you close a bead, and at session end, run `scripts/clean-build.sh --apply` (until it exists, `cargo sweep --time 2`).
  - Never run a full `cargo clean`: other agents are building in the same `target/`. If one is really needed, ask in Agent Mail first.
- **Tear down what you start.** Every harness run ends with `docker compose down -v --remove-orphans`, even on failure. Docker objects you create carry the label `pf.harness=1`, so the cleanup can find them.

## Landing the plane at session end

1. Update the bead statuses: close what passed its gate, and leave the rest `in_progress` with a comment saying where it stands.
   Then run the disk cleanup above (`scripts/clean-build.sh --apply`, or `cargo sweep --time 2` before it exists).
2. Run `br sync --flush-only`.
3. Commit (the bead files under `.beads/` together with your code).
4. Run `git pull --rebase`.
5. Push. Work isn't done until the push succeeds, and `git status` shows you up to date with origin.
6. Release your file reservations, and post a finish note in each bead's Agent Mail thread.

## Secrets and site config

- gitleaks runs before every push, and in CI.
- Never commit household names, internal IPs or the IdP inventory. Write "the admin" or "a family member", and use `<domain>` or `pf.test` in examples.
- Everything specific to one deployment lives in the private `partifactory-site` repo, never here. Secrets come from env vars or `_FILE` paths.

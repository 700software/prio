# prio e2e test suite

End-to-end tests for the `prio` CLI, running inside an isolated Docker container
against a real GitHub repository (`tmp-prio-test`).

## Prerequisites

- [Docker](https://docs.docker.com/get-docker/) — must be running on the host
- `gh` — GitHub CLI, authenticated: `gh auth login`
- Network access to GitHub

## Running

```bash
npm test
```

`npm test` is the only command needed. It:

1. Checks `gh auth status` on the host (fails early if not authenticated).
2. Extracts `GH_TOKEN` from the host gh session and forwards it into the container.
3. Derives the test repo slug as `<your-gh-username>/tmp-prio-test` (or reads `GITHUB_REPO`).
4. Builds the Docker image `prio-e2e` (Rust + Node 20 + git + gh).
5. Runs the full phased e2e scenario inside the container.

## Environment variables

| Variable             | Default                  | Purpose                                   |
| -------------------- | ------------------------ | ----------------------------------------- |
| `GITHUB_REPO`        | `<whoami>/tmp-prio-test` | GitHub repo slug for the test repo        |
| `PRIO_E2E_WORKSPACE` | `/tmp/prio-e2e`          | Directory inside the container for clones |

## Test phases

| Phase | Commands exercised                                 |
| ----- | -------------------------------------------------- |
| 1–3   | `setup`, `status`                                  |
| 4–5   | `mv -c`, `mv`                                      |
| 6–7   | `mv .`, `mv -a -c`                                 |
| 8–9   | `unapply`, `apply`                                 |
| 10    | `stack`, `apply`                                   |
| 11–13 | `push`, `pr`, `prs`                                |
| 14–16 | `sync` (upstream push + PR merge)                  |
| 17–19 | `recover`, `unstack`, `syncs`                      |
| 20–22 | Merge conflict: detection + resolution via prio-mc |

## TDD workflow

The first run may reveal prio bugs. The workflow is:

```
rough in e2e.mjs → npm test → classify failure → fix prio OR fix assertion → repeat
```

- Fix **prio** when the tool behavior is wrong.
- Fix an **assertion** only when the test expectation was incorrect.
- Do not weaken assertions to force a pass.

## Files

```
test-e2e/
  README.md          ← this file
  run.mjs            ← host entry point (preflight + docker build/run)
  e2e.mjs            ← phased scenario flow (readable test script, runs inside container)
  utils.mjs          ← composable helpers (exec, git, gh, prio, assert, step…)
  docker/
    Dockerfile       ← Debian bookworm + Rust + Node 20 + gh
```

## GitHub repo lifecycle

`tmp-prio-test` is persistent — it is created on the first run and reused.
Each test run force-pushes `main` in step 2 to reset to a clean seed state.
This means the repo must be in your own GitHub account (or an org you control).

## Isolation

The container uses `PRIO_CONFIG_DIR=/tmp/prio-e2e/config` so the test's prio
configuration is completely isolated from the developer's installed `prio` instance.

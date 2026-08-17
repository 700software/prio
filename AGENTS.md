# Agent notes for prio

## Merge conflicts: prio-mc only

**All merge conflict resolution must go through the `*-prio-mc` clone.** The main work repository must never be where conflicting merges are resolved.

When prio-mc has a finished result (clean merge chain complete), sync the work clone with `git reset --hard` to the mc HEAD (`sync_work_clone` in `src-tauri/src/services/apply.rs`). prio-mc and the work repo are separate clones with separate object stores — `sync_work_clone` fetches from mc before resetting.

### Why

- Keeps the work branch predictable: either at the last known-good snapshot or exactly matching a completed prio-mc result.
- Avoids half-resolved conflict markers on the branch developers push from.
- Gives one dedicated clone to open in an editor when merges go wrong.

### User workflow on conflict

1. **prio-mc** — User fixes conflicts here, commits, then continues via the mc post-commit hook (`internal-mc-post-commit`) or by re-running `prio apply` / `prio mv`.
2. **Work clone** — Updated only after mc succeeds, via `sync_work_clone`.

### Contributor rules

| Do in **prio-mc**                                 | Do in **work clone**                              |
| ------------------------------------------------- | ------------------------------------------------- |
| `git merge`, `git cherry-pick`                    | `git reset --hard` to mc HEAD (`sync_work_clone`) |
| Conflict resolution + commit                      | Checkout work branch; ordinary feature commits    |
| `reset_mc_to_default` / trial merges for ordering | Hooks record last-good state                      |

Do **not** add code paths that run `git merge` or `git cherry-pick` on the work clone for prio-managed operations (`apply`, `mv`, `sync`, etc.).

**`prio recover`** is an exception for emergency rollback: it may `git reset --hard` the work clone to last-good, then must **re-apply through prio-mc** (not merge on the work clone).

### All-or-nothing sync invariant

For any operation that involves cherry-picks or merges, **all affected branches must be staged in prio-mc before any branch is synced to the work clone**:

1. **Stage all branches in mc first** — source rebases, destination cherry-picks, and the apply merge (work branch rebuild) all complete in prio-mc.
2. **Sync to work clone atomically at the end** — feature branch refs via `git fetch mc +branch:branch` (or `git push origin branch` from mc), then the work branch via `sync_work_clone`.
3. **On conflict** — leave mc dirty for user resolution; the work clone must remain untouched at its last-good state.

This prevents the work clone from ever landing in a partial state where, for example, the destination branch is updated but the source rebase or apply merge has not completed.

Concretely:

- Branch creation (`git branch dest`) happens in prio-mc, not in the work clone.
- Intermediate per-step ref syncs between mc and work are not allowed; sync only once at the end.
- The work branch (`config.work_branch`) is reset via `sync_work_clone` **only** after the full mc pipeline succeeds.

## Cherry-pick vs. merge-up policy

| Branch state                           | `prio mv` strategy                                          | `prio apply` strategy   |
| -------------------------------------- | ----------------------------------------------------------- | ----------------------- |
| Local-only (no `origin/<branch>`)      | Cherry-pick in prio-mc                                      | Merge local ref         |
| Pushed — commit **already on** branch  | Metadata-only — update `commit_assignments`, no cherry-pick | Merge `origin/<branch>` |
| Pushed — commit **not yet on** branch  | Cherry-pick in prio-mc, then push from mc to work repo      | Merge `origin/<branch>` |
| Unassign (`.`) — work-area commit      | Metadata-only — already in work area above baseline         | N/A                     |
| Unassign (`.`) — applied-branch commit | Source rebase + apply merge + cherry-pick on mc; then sync  | N/A                     |

Detection for "already on branch": `git merge-base --is-ancestor <sha> <branch-tip>` where tip is local `branch` or `origin/<branch>` (whichever is ahead).  
`prio status` lists branch commits from that git tip (`git log default..<tip>`), not from `commit_assignments` alone.  
`prio mv` **auto-runs `prio apply`** when every above-baseline commit is assigned; if unassigned commits remain, rebuild is skipped so they are not lost — assign them first, then `prio apply`.  
See [`mv::run`](src-tauri/src/services/mv.rs) and [`docs/design.md`](docs/design.md).

## prio-mc remote rules

**prio-mc is a local-only clone used solely for merge conflict resolution. It is never pushed to github.com (or any external remote).**

- prio-mc's `origin` is the **work repo** (a local filesystem path), not github.com.
- Pushes from prio-mc go to the work repo only (e.g. `git push origin bryan-dev` updates the work repo's local `bryan-dev`, not github.com).
- The user pushes to github.com from the **work clone** via `prio push` / `git push`.
- prio-mc branches can be **freely reset, deleted, or wiped** if the clone gets into a bad state — run `prio apply` afterward to rebuild. The only data that must persist is the conflict history stored in `.git/prio/` (which survives branch resets).

## Command documentation

When adding, renaming, or changing CLI behavior, update **both**:

1. **`--help` text** — clap `about` / `help` strings in `src-tauri/src/cli/commands.rs`
2. **README command reference** — the Command reference table in `README.md`

Keep the two in sync so users see the same descriptions from `prio <cmd> --help` and the README.

## Learned User Preferences

- Never engineer test data to avoid merge conflicts; always test the full conflict-resolution flow (prioCapture mv, assert WARNING text + prio status conflict banner, resolve in prio-mc, verify clean status).
- When verifying conflict behavior in tests, assert both the command output (WARNING containing "Merge conflict in prio-mc") and `prio status` output showing the incoming branch name and resolution instructions.

## Learned Workspace Facts

- `prio mv <sha> .` unassigns cross-branch commits via `run_cross_branch_unassign`: cherry-picks onto the work branch, rebases the source branch in prio-mc to drop the commit, then re-applies. `-c` with `.` is rejected. `-f` is required when the source branch is pushed (rebase rewrites history).
- Cherry-pick conflict asymmetry: work-area → branch moves abort immediately and return `PrioResult::failure` ("resolve why this commit conflicts and retry"); cross-branch moves leave prio-mc dirty and return `PrioResult::warning` + `prio status` resolution flow. These are intentionally different paths.
- Force-push sync invariant: before force-pushing rebased source branches to origin, sync each source branch ref from mc to work via `git fetch mc +branch:branch`. `execute_apply_merge` only syncs refs on success; if it conflicts the work clone's source branch ref is stale.
- Branch tips in prio-mc: at operation start `reset_mc_to_default` deletes all local feature refs except default so tips come from `origin/<branch>` (work clone is source of truth). During an in-flight operation, `merge_ref_for_branch` prefers local mc refs created by rebases over stale `origin/<branch>` (which tracks the last fetch from work, not mc's rebased state).
- `reset_mc_to_default` fetches the default branch from github.com via the work clone at most once per `prio` process (`WORK_ORIGIN_FETCHED` AtomicBool); prio-mc's local `git fetch origin` (filesystem) still runs on every reset.
- If the prio-mc post-commit hook doesn't run after conflict resolution, manually invoke `prio internal-mc-post-commit [--mc-path=<mc-clone>]`.
- CLI `--repo` is a global arg on `CliApp` (`#[arg(long = "repo", global = true)]`). Per-subcommand `--repo-path` was removed. `setup` still accepts a positional path (prefers it over `--repo`).
- Hook subcommands: `internal-work-post-commit` and `internal-mc-post-commit` (no leading underscore). Legacy `_internal-*` aliases kept for existing installs. Hook scripts installed by `prio setup` use the no-underscore names.
- `prio reorder` (Tauri command `prio_reorder`) reapplies with explicit branch order. Validates exact same set of applied branches; rejects reordering at or below branches already merged in an in-progress apply.
- `ensure_branch_for_apply` in `src-tauri/src/git/runner.rs` runs `git fetch` (with log comment "to see if the branch is found at origin") when a branch isn't found locally or as a remote-tracking ref before apply.
- `build:binary` uses `tauri build --no-bundle` (no `--` separator); `--no-bundle` is a Tauri CLI flag, not a cargo arg.
- Tauri Linux build dependencies (Ubuntu/WSL, no version pins): `sudo apt install build-essential pkg-config libglib2.0-dev libgtk-3-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libssl-dev libxdo-dev`.

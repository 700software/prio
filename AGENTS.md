# Agent notes for prio

## Merge conflicts: prio-mc only

**All merge conflict resolution must go through the `*-prio-mc` clone.** The main work repository must never be where conflicting merges are resolved.

When prio-mc has a finished result (clean merge chain complete), sync the work clone with `git reset --hard` to the mc HEAD (`sync_work_clone` in `src-tauri/src/services/apply.rs`). prio-mc and the work repo are separate clones with separate object stores — `sync_work_clone` fetches from mc before resetting.

### Why

- Keeps the work branch predictable: either at the last known-good snapshot or exactly matching a completed prio-mc result.
- Avoids half-resolved conflict markers on the branch developers push from.
- Gives one dedicated clone to open in an editor when merges go wrong.

### User workflow on conflict

1. **prio-mc** — User fixes conflicts here, commits, then continues via the mc post-commit hook (`_internal-mc-post-commit`) or by re-running `prio apply` / `prio mv`.
2. **Work clone** — Updated only after mc succeeds, via `sync_work_clone`.

### Contributor rules

| Do in **prio-mc**                                 | Do in **work clone**                              |
| ------------------------------------------------- | ------------------------------------------------- |
| `git merge`, `git cherry-pick`                    | `git reset --hard` to mc HEAD (`sync_work_clone`) |
| Conflict resolution + commit                      | Checkout work branch; ordinary feature commits    |
| `reset_mc_to_default` / trial merges for ordering | Hooks record last-good state                      |

Do **not** add code paths that run `git merge` or `git cherry-pick` on the work clone for prio-managed operations (`apply`, `mv`, `sync`, etc.).

**`prio recover`** is an exception for emergency rollback: it may `git reset --hard` the work clone to last-good, then must **re-apply through prio-mc** (not merge on the work clone).

## Cherry-pick vs. merge-up policy

**Cherry-pick / rebase is only allowed for local-only branches** (no `origin/<branch>` exists yet).  
**Pushed branches** (or branches with a PR) must go through merge-up — no rewriting shared history.

| Branch state            | `prio mv` strategy                                          | `prio apply` strategy   |
| ----------------------- | ----------------------------------------------------------- | ----------------------- |
| Local-only (not pushed) | Cherry-pick in prio-mc                                      | Merge local ref         |
| Pushed or in PR         | Metadata-only — update `commit_assignments`, no cherry-pick | Merge `origin/<branch>` |
| Unassign (`.`)          | Cherry-pick always fine                                     | N/A                     |

Detection: `git rev-parse --verify origin/<branch>` — if it succeeds, the branch is pushed.  
See [`mv::run`](src-tauri/src/services/mv.rs) and [`docs/design.md`](docs/design.md).

## Command documentation

When adding, renaming, or changing CLI behavior, update **both**:

1. **`--help` text** — clap `about` / `help` strings in `src-tauri/src/cli/commands.rs`
2. **README command reference** — the Command reference table in `README.md`

Keep the two in sync so users see the same descriptions from `prio <cmd> --help` and the README.

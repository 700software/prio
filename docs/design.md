# prio design principles

## Merge conflicts: prio-mc only, then hard-reset the work clone

**All merge conflict resolution happens in the merge-conflicts clone (`*-prio-mc`).**  
The main work repository must never be the place where conflicting merges are resolved.

### Workflow

1. **prio-mc** — Run merges, cherry-picks, and manual conflict resolution here. On failure, the user fixes conflicts in this clone, commits, and continues (e.g. via the mc post-commit hook or re-running `prio apply` / `prio mv`).
2. **Work clone** — When prio-mc has a finished result (clean merge chain complete), update the work branch with:
   ```bash
   git checkout <work-branch>
   git reset --hard <head-from-prio-mc>
   ```
   This is implemented as [`sync_work_clone`](../src-tauri/src/services/apply.rs) in the Rust service layer.

### Why

- Keeps the work branch in a predictable state: either at the last known-good snapshot or exactly matching a completed prio-mc result.
- Avoids half-resolved conflict markers on the branch developers push from.
- One dedicated place to open in an editor when merges go wrong.

### Implementation rules (for contributors)

| Do in **prio-mc**                                 | Do in **work clone**                                                |
| ------------------------------------------------- | ------------------------------------------------------------------- |
| `git merge`                                       | `git reset --hard` to mc HEAD (via `sync_work_clone`)               |
| `git cherry-pick`                                 | Checkout work branch, record last-good state (hooks)                |
| Conflict resolution + commit                      | Ordinary feature commits on the work branch                         |
| `reset_mc_to_default` / trial merges for ordering | `prio recover` may reset to last-good, then **re-apply through mc** |

Do **not** add new code paths that run `git merge` or `git cherry-pick` on the work clone for prio-managed operations.

## Cherry-pick vs. merge-up policy

prio's handling of commits depends on whether their target branch has been shared:

| Branch state                                         | `prio mv` behavior                                              | `prio apply` behavior   |
| ---------------------------------------------------- | --------------------------------------------------------------- | ----------------------- |
| **Local-only** (no `origin/<branch>`)                | Cherry-pick in prio-mc — safe to rewrite local history          | Merge local branch ref  |
| **Pushed** (`origin/<branch>` exists) or **in a PR** | **Metadata-only** — update `commit_assignments`, no cherry-pick | Merge `origin/<branch>` |
| **Unassigned** (destination `.`)                     | Cherry-pick is always fine                                      | N/A                     |

### Why the distinction matters

Once a branch has been pushed, its commit SHAs are part of shared history. Cherry-picking or rebasing would create new SHAs and force collaborators to reconcile diverged histories. Instead:

- **Pushed branches** go through _merge-up_: when `prio apply` merges `origin/<branch>` into the work area, all commits on that remote branch arrive via a merge commit — no SHAs are rewritten.
- **Local-only branches** have no shared history yet, so cherry-pick/rebase is safe and can produce cleaner linear history.

### Practical consequence for `prio mv`

```
prio mv <sha> my-local-branch   # branch is local only → cherry-picks sha onto my-local-branch in prio-mc
prio mv <sha> my-pushed-branch  # origin/my-pushed-branch exists → updates metadata only, no cherry-pick
prio mv <sha> .                 # unassign → cherry-pick is fine
```

For pushed branches, `prio mv` records which branch the commit _belongs to_ for status display purposes. The commit is physically included in the work area via the branch's merge commit when `prio apply` runs.

### Current entry points

- `prio apply` / `prio unapply` — [`apply::run`](../src-tauri/src/services/apply.rs)
- `prio mv` — [`mv::run`](../src-tauri/src/services/mv.rs) (cherry-pick + merge in mc, then sync)
- `prio sync` — purges merged branches, then re-applies remaining branches through mc
- `prio recover` — hard-reset work clone to backup, then `apply` through mc
- Mc post-commit hook — continues pending merges in mc, then `sync_work_clone`

# prio (PR I/O)

Stacked PR manager that uses a **merge-up** workflow instead of rebase.
Also lets you apply multiple branches to your work tree at once!
Merge conflicts are handled in a separate directory so you don't have to stash all your changes.

The benefits of stacked PRs are described well at [stacking.dev](https://www.stacking.dev/). An up and coming tool to consider is [GitButler](https://gitbutler.com/).

**prio** design principles:

- Most tools in that space rely heavily on rebase. **prio** aims to support a merge-up approach.
- **prio** also aims to rely only on stable `git` and GitHub (`gh`) CLI operations. It aims to never lose your work.
- Rather than replacing your git client, it aims to coexist with it.
- Supports both CLI and Desktop UI (implemented with Tauri).
- Works cross-platform: macOS, Windows, Linux.
- A dedicated merge-conflicts clone (`*-prio-mc`) for conflict resolution

Noteworthy features:

- Assign commits to other branches without losing them from your work tree.
- Stack one branch to be dependent upon / stacked after a dependency.
- Update your GitHub PRs to clarify dependency PRs in the same repo.

See also: [Why not rebase?](docs/why-not-rebase.md) (draft).

## Requirements

- [Git](https://git-scm.com/download)
- [GitHub CLI](https://cli.github.com/) (`gh auth login`)

## Install & run

```bash
npx prio@latest # UI
npx prio@latest --help # CLI features
```

## Quick start

With no arguments, `prio` opens the **desktop app** (GUI). Subcommands (`prio setup`, etc.) run the CLI.

```bash
prio setup
# or: prio setup /path/to/your/repo
prio apply feature/my-branch
prio status
prio push feature/my-branch
prio pr feature/my-branch
```

## Command reference

| Command                                  | Description                                                                                                                                                                                                                                                           |
| ---------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `prio setup [repo] [mc-clone]`           | Register repo (default: current directory), create/use `*-prio-mc` clone, set work branch                                                                                                                                                                             |
| `prio unsetup [-y]`                      | Archive `.git/prio`, rename work branch to `prio/backup/<ts>`, backup mc clone (`-y` skips prompt)                                                                                                                                                                    |
| `prio status`                            | Applied branches, assigned commits, and unassigned commits on the work branch                                                                                                                                                                                         |
| `prio apply <branch\|pr-N>...`           | Merge branches into work area (via prio-mc)                                                                                                                                                                                                                           |
| `prio unapply <branch\|pr-N>...`         | Remove branches from work area                                                                                                                                                                                                                                        |
| `prio mv <sha>... <dest> [-c] [-a] [-f]` | Assign commits to a branch or `.`; `-c` creates and applies; `-a` applies destination; for commits on applied branches, removes them from the source branch in prio-mc. `-f` force-pushes the source branch after rebase (required when source is pushed to origin).  |
| `prio cp <sha>... <dest> [-c] [-a]`      | Copy commits to a branch non-destructively — the source branch is unchanged. `-c` creates and applies destination; `-a` applies it.                                                                                                                                   |
| `prio push <branch> [-p]`                | Push branch to origin. `-p` / `--push-deps` also pushes any stacked dependency branches that are not yet on origin; without `-p`, the command fails if any dependency is unpushed.                                                                                    |
| `prio pr <branch>`                       | Push and open draft PR (uses `PR.md` if present)                                                                                                                                                                                                                      |
| `prio stack <branch> [dep1 dep2 ...]`    | Record that `branch` is stacked after the listed dependencies; re-applies in dependency order                                                                                                                                                                         |
| `prio unstack <branch> [-k\|-f]`         | Remove stack relationship; rebases branch off dependencies back onto default branch by default. `-k` keeps upstream commits (metadata-only, safe for pushed branches). `-f` rebases and force-pushes (rewrites remote history). Pushed branches require `-k` or `-f`. |
| `prio sync`                              | Purge merged branches from apply list                                                                                                                                                                                                                                 |
| `prio syncs`                             | Run `prio sync` for all registered repos                                                                                                                                                                                                                              |
| `prio abort`                             | Abort an in-progress `prio mv` conflict: cleans up the partial destination branch in prio-mc and WORK, and clears conflict state. Faster and more targeted than `prio recover`.                                                                                       |
| `prio recover`                           | Reset work branch to last known good state                                                                                                                                                                                                                            |

When you checkout a branch other than your work branch, prio becomes **inactive** — checkout the work branch or run `prio setup`.

## Data locations

- **Per-repo:** `.git/prio/` (config including default branch, state, hooks, backups)
- **User config:** `%APPDATA%\prio` (Windows), `~/Library/Application Support/prio` (macOS), `~/.config/prio` (Linux) — work branch prefix, registered repos, UI tab order

## Development

```bash
npm install
npx tauri dev          # GUI
npm run cli -- --help  # CLI (debug build; pass subcommands after --)
npm run build:binary   # build release binary
```

## Acknowledgments

Thank you to workplaces everywhere for taking way too long to merge my PRs, 😀 thereby inspiring me to continue working on follow-up branches in parallel even before I had any tooling.

Thanks to Jessie White for informing me the proper term for this whole thing is "Stacked PRs".

Thanks to [@scull7](https://github.com/scull7) for being a great mentor, and for finding [stacking.dev](https://www.stacking.dev/) which explains the stacked PR workflow way better than I could.

Thanks to [GitButler](https://gitbutler.com/) for inspiring me to the idea that there could be a proper tool for applying multiple branches to a single worktree.

Thank you to my family business for getting me entrenched with Windows so I bothered to make prio cross-platform from the start.

Thank you to AI companies everywhere for making this possible to do quickly in my spare time. (vibe coding v1)

Thanks to the creators of Rust and Tauri.

Thanks to you for reading this far! Contributions welcome! (If it helps your workflow it will likely help others!)

We need to create a rather large plan.

# prio (PR I/O 😃)

## purpose and general design

The purpose of this app is to manage Stacked PRs without using Rebase.

In the readme, we can say the benefits of Stacked PRs are described well at https://www.stacking.dev/.

The listed tools mostly use Rebase. On the other hand, `prio` aims to support primarily a ‘merge-up’ approach. There should be also a separate markdown file explaining why ‘merging in main’ is better than rebasing, but leave that file empty for now. It can be `docs/why-not-rebase.md` and the `README.md` can have a link to it. Another feature we should describe is stacking one branch upon multiple other branches. This is something GitButler aims to achieve also. However at the time of this writing, it still heavily relies on Rebase, and last time I tried to use it on Windows I ran into stability problems. `prio` aims to do less. Just live alongside your `git` and `gh` tools and perform only stable operations.

The UI should be styled such that it shows the CLI equivalent for each command.

Note that we want each feature to support both CLI or UI based interaction. So there should be a separation of layers. There should be a CLI parsing layer separate from the service function layer. The service function should return a consistent format. Success/fail/warning, conclusion message. And there should be a log function that, if run via the UI will store the log messages to an array that can be rendered with an optional toggle block. The in progress action can show the ability to toggle open the log message list array. Also the conclusion div success/fail/warning should have that same toggle block.

Note that we want the CLI entry point to be rust. Keep in mind that the primary to use this tool on the cli will be `npx prio`. The `package.json` will probably need a `main` entry to map to the Rust binary.

Note this project should build cross-platform, Mac, Windows, Linux.

`npm publish` should have `prepublish` and `postpublish` scripts inspired by `C:\GitHub\my-npm\stuple`

Note that the version from `package.json` I think is automatically overwritten by the value in `Cargo.toml` if I am not mistaken.

## Data storage.

Repo-specific data should be in the repo in `.git/prio` directory.

For user data, Please recommend a directory location for these. Note this is cross-platform and will need to work on Mac/Linux as well as Windows.

Inside that directory there will be these files:

- ui-state (order of tabs)
- config (default branch name)
- repos (list of all the repos for which prio setup was run. This will be needed for the `prio syncs`)

## first feature - `prio setup`

First command is prio setup. The first positional parameter is repo path. The 2nd positional parameter is optional, and that is 2nd clone for merge conflicts. It will default to the same as the name repo but with the suffix `-prio-mc` (shorthand for prio merge conflicts)

The user preferences of this app should have a record of all the repositories that have been added to the app, including the git remote origin path (normalized for https vs git: URLs).

If the origin is not github.com, print a warning to say the tool has never been tested with non-GitHub repositories, so their milage may vary.

If there is an existing clone already added to prio with the same origin, then it can reuse the same `-prio-mc` directory that was already created. Also it should congratulate the user for creating multiple clones so that they can effectively have multiple working trees as needed 🎉

The UI for it should be ‘setup repository’ (again, show code block `prio setup` so they know the CLI version of it.

The implementation of prio will require `gh` and `git` to be installed on the CLI. So every time that it runs a CLI, it should go through a generic wrapper that detects if the tool is not installed. If it’s not installed, it should mention the official install method of the tool. Detect the OS and provide a one-line CLI if possible. Otherwise link to the official install web page. I know that for Windows, the official install page will also install Git Bash. Look up on the internet the official install instructions for `git` and `gh` (GitHub CLI) for Mac and Windows.

### username prompt

When `prio setup` is run, it should call a reusable ‘get work branch name’ function. This will read the user preferences of `prio` and if it is set it should just follow that. However if it is not set, it should use the GitHub API to detect their username. Look at the git config to determine the email address. Then it should look up https://api.github.com/search/users?q={email_address} and if `items.length` in the response JSON is not zero, take the first item and read the `login` prop for the default value `prio/{login}`. Then in the prompt that lets them configure their work branch, they can just press enter to accept the default value. Again there should be both a UI and CLI version of this. The UI should use the placeholder input property to show the default value. Either way pressing enter without typing anything accepts the default value.

There should be text there to explain the purpose of the work branch. This branch will dynamically have changes applied/unapplied as they desire using `prio` (It’s super effective), but we want it to be unique in case they accidentally git push it, we wouldn’t want to overwrite the work of anyone else using `prio` ;)

## work branch architecture

Basically any branch is going to have it’s baseline commits (which is the combination of one or more branches, defaulting to whatever is on the default branch) + user-provided commits.

There should be a data directory in the git repository `.git/prio` which will house tracking data for the repo.

### work branch name

Standardize the name of the main/primary/work clone to be just work clone

### if the work clone is on any other branch (not the work branch)

every prio command should say that `prio` is inactive. And that the user can either to `git checkout {work branch name}` or `prio setup` to get back to the work branch at any time

## `prio apply` / `prio unapply`

Basically the user can either run `prio apply branch-name` or `prio apply pr-{pr-num}` 

First prio will detect which commits are user-initiated - not part of the baseline.

Then in the prio-mc clone it will start with the ‘default branch’ (`main` usually) and then apply or unapply all the branches the user specified. Internally it will first hard reset the prio-mc clone to have just the default branch, then it will merge in every branch that was requested. However if there is a merge conflict it will pause and let the user know there are merge conflicts that need to be resolved in the {path to prio-mc directory} clone. Then `prio apply` can exit. In that repository there should be a postcommit hook to detect when the merge conflict is resolved, and then continue the apply work.

### merge conflict resolution history

Not only should the main clone have a `.git/prio` directory to track which commits are automatic baseline commits (not to be confused with user-provided commits), also the prio-mc repository should have a `.git/prio` directory. This will detect a history of known merge conflict resolutions. It should mention the name of both branches, and the commit sha that resolved the conflict. It should store each and every merge conflict resolution sorted by how deep down the tree it is. So the plan will need to spec out a function to determine the order of commit shas listed. This is in case someone else on the team (not the prio user) decides to do a force push on their branch. Maybe the most recent merge conflict resolution is no longer useful, but the one before that maybe still is

### merge conflict simplification

Any time a merge conflict is found, an attempt should be made to resolve it with merge conflicts on file.

Once that is done, each combination of PR should be checked to determine what are the simplest merge conflicts to resolve in what order.

For example, if the user does `prio apply branch-a branch-b branch-c` it’s possible that merging A+C followed by A/C + B will be simpler than simply doing A+B followed by A/B + C. So we want to measure the difficulty of each, and then present the merge conflict resolution attempt in that order.

## locking

Note that running multiple prio service functions simultaneously could cause file corruption.

- suggest to me various solutions to mitigate this
- additionally every file write should be atomic
- also suggest locking mechanism so each run can wait on the other one to finish

## header bar

The UI should generally have tabs across the top that are styled very similar to Chrome tabs. One tab for each repo. And it should be possible to drag/drop reorder them. There should be a tab at the end styled differently for ‘setup new repository’ (the UI for the `prio setup` action)

## second feature - `prio status`

Again there should be a UI and CLI version

This feature will show all the PRs and Branches that are currently applied to the work area, and it should show all the commits added on top of that. Each in a separate column. There should be a drag/drop interface to move commits between branches, or to reorder them.

## `prio mv`

This is the CLI command that will be used to move commits. The last parameter is the destination `branch-name` or `pr-{pr_num}`. Each positional parameter before the last parameter is the commit sha that should be moved. Only user-provided commits built after the base branch may be moved. The base which is the automatic merges that prio-mc performed as a result of the 

`-c` option is a positional parameter that can be added to indicate the branch should be created if it doesn’t exist. This is not allowed for `pr-{num}` format. the `-c` option can either be at the very end, or after the last positional parameter. This can be helpful in case the user gets confused, but if the `-c` option is at the specific location we expect then we know the user knows what they are doing (creating a branch)

Once this happens, the prio-mc branch will have to be reset to a compatible state, then it will have to pull the work branch to the prio-mc branch. Then it will have to cherry pick the commit(s) to the destination branch. The user may have to resolve merge conflicts there. When the merge conflicts are all done the prio-mc post commit hook will run and when the entire cherry pick of all the commits is done, then the **baseline** can be updated to. However this may introduce a new merge conflict merging branches together. So therefore the branch resolution path that prio apply uses will have to be invoked.

** Reuse code as much as possible. Keep code DRY **

Note this can also move commits in reverse. `prio mv commit-id-in-some-branch .` and the alias for work branch can either be the name of the work branch itself, or just `.`. Also note that prio mv can take commit ranges. Also store a mapping of every renamed commit because when a commit is moved its commit id changes.

## `prio pr branch-name`

This will push the branch and create the PR. If a [PR.md](http://PR.md) file exists, then it will use that as the PR description. Note that prio should never include the PR.md in the final commit when it gets pushed so the commit may need to be amended. It should initially keep the PR in draft state. And if the PR is stacked, it should mention the name of each PR number it is stacked after. If it’s stacked after a branch then it should mention the branch name. If that branch is not pushed then it should have parenthesis (not pushed yet)

## `prio push branch-name`

This pushes the branch but doesn’t make the PR yet.

Note: We use the gh cli. If not authenticated, then tell the user they’ll have to run the gh login command first.

## Chained branches with `prio stack`

Using either drag/drop UI, or using CLI, there should be the ability to stack branches after other branches. Syntax should be `prio stack dependency-branch stacked-branch` and these should either take branch-name or pr-pr_num syntax. Also it should support + syntax like `prio stack dependency-branch+dependency-branch stacked-branch`. There should also be a `prio unstack` command. And after every update to a branch, there should be 

## Suggestions for stacking/unstacking

After every `prio push`, `prio pr`, `prio stack`, `prio mv`, `prio sync` then it should detect which branch combinations might have a merge conflict (and keep a cache in the `prio-mc` `.git/prio` directory) and suggest branches that may be stacked after some other branch unnecessarily, or branches that could be stacked to prevent merge conflicts when the time comes to run `prio pr`.

## `prio syncs`

This will iterate over each of the known clones and run `prio sync` .

## `prio sync`

This will check each of the branches to see if they got merged to main yet. If so then they can be purged from the apply list because just having the work in `main` is enough.

## commit hooks on the work clone

there should be a post commit hook to note the last known good state in case `prio recover` is needed

## `prio recover`

If the user adds a merge commit on top of the prio-managed merge commits, or if the user removes one of the prio-managed merge commits (basically the state of the work branch doesn’t match what is defined in `.git/prio`) then this is an unrecoverable state. The user will be forced to run `prio recover` which will hard reset to the last known good state.

However if the work tree is not clean (there are any non-stashed changes) it will ask the user to run `git reset --hard` or `git stash` first before they run `prio recover`. Then `prio-recover` should create `prio/backup/{timestmap_ms}` of the current commit level before recovering to the last known good state. Then it should show a warning line of the backup that was taken.

However if the backup would contain now new commits beyond the baseline, then no backup needs to be taken and no warning needs to be printed.

## how commit hooks will work

Installation of commit hooks should be part of a separate reusable function. Separate hooks are configured for the work branch vs the prio-mc branch.

- the hook paths for prio should be `.git/prio/hooks`
- installation of hooks should detect if `.husky` is gitignored. If it is gitignored, then create the `.husky/_/post-commit` file if it doesn’t exist and append a command to also run `.git/prio/hooks`
- When configuring the `core.hooksPath`, if it’s already set to the husky directory leave it there. Otherwise set it to the `.git/prio/hooks` directory.
    - If it’s present and set to any other value, print a warning.

# Plan size

This plan needs to be very detailed as if a junior developer was implementing the feature.

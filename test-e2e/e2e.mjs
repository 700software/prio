/**
 * test-e2e/e2e.mjs — end-to-end scenario flow for prio.
 *
 * Runs inside a Docker container (invoked by run.mjs via docker run).
 * Each phase maps to one or more prio subcommands and asserts observable outcomes.
 *
 * Coverage:
 *   setup, status, mv -c, mv, mv ., mv -a, apply, unapply,
 *   stack, unstack, push, pr, prs, sync, syncs, recover, unsetup,
 *   merge-conflict banner + resolution,
 *   mv cross-branch (local-only source), mv cross-branch (pushed source: -f),
 *   cp (non-destructive copy),
 *   mv stale-prio-mc-ref regression (untouched branch ref preservation),
 *   prio stack merge-conflict propagation (WARNING not SUCCESS),
 *   prio status auto-recovery when prio-mc conflict state wiped,
 *   prio push dependency enforcement (-p flag)
 */

import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import {
  assertHasMergeConflict,
  assertIncludes,
  assertNoMergeConflict,
  assertNotIncludes,
  assertStatusHasBranch,
  assertStatusUnassignedEmpty,
  branchCommitsSinceMain,
  commitAll,
  ensureGithubRepo,
  exec,
  freshClone,
  gh,
  git,
  gitCapture,
  headSha,
  mcPath,
  PRIO_BINARY,
  prioCapture,
  prioOk,
  prioOut,
  prioOutCombined,
  prioStatusText,
  printSummary,
  readRepoState,
  step,
  writeFile,
} from "./utils.mjs";

// ── Configuration ─────────────────────────────────────────────────────────────

const GITHUB_REPO = process.env.GITHUB_REPO;
if (!GITHUB_REPO) {
  console.error("GITHUB_REPO env var is required (e.g. myuser/tmp-prio-test)");
  process.exit(1);
}

const WORKSPACE = process.env.PRIO_E2E_WORKSPACE || "/tmp/prio-e2e";
const WORK = `${WORKSPACE}/tmp-prio-test`;
const UPSTREAM = `${WORKSPACE}/tmp-prio-test-upstream`;
const REMOTE = `https://github.com/${GITHUB_REPO}.git`;

console.log("\n" + "═".repeat(60));
console.log("prio e2e test suite");
console.log(`  repo:      ${GITHUB_REPO}`);
console.log(`  workspace: ${WORKSPACE}`);
console.log(`  binary:    ${PRIO_BINARY}`);
console.log("═".repeat(60));

// ── Phase 1: Bootstrap ────────────────────────────────────────────────────────

await step("1. ensure GitHub repo exists", async () => {
  await ensureGithubRepo(GITHUB_REPO);
});

await step(
  "2. seed main via upstream clone + clean up leftover branches",
  async () => {
    freshClone(UPSTREAM, REMOTE);
    git(UPSTREAM, "checkout", "-B", "main");
    writeFile(`${UPSTREAM}/README.md`, "# tmp-prio-test\n");
    writeFile(`${UPSTREAM}/conflict.txt`, "base\n");
    commitAll(UPSTREAM, "seed main");
    git(UPSTREAM, "push", "--force", "-u", "origin", "main");

    // Delete any leftover remote branches from previous runs (except main).
    const lsRemote = git(UPSTREAM, "ls-remote", "--heads", "origin");
    const extraBranches = lsRemote
      .split("\n")
      .filter(Boolean)
      .map((line) => line.split("\t")[1]?.replace("refs/heads/", ""))
      .filter((b) => b && b !== "main");
    for (const branch of extraBranches) {
      try {
        git(UPSTREAM, "push", "origin", "--delete", branch);
      } catch {
        // Ignore failures for branches that no longer exist
      }
    }
  }
);

await step("3. clone work repo + prio setup", async () => {
  freshClone(WORK, REMOTE);
  prioOk(WORK, "setup", "--work-branch", "prio/test");
  assert.equal(
    git(WORK, "branch", "--show-current").trim(),
    "prio/test",
    "should be on work branch after setup"
  );
  assert.ok(
    existsSync(`${WORK}/.git/prio/state.json`),
    ".git/prio/state.json should exist after setup"
  );
  const mc = mcPath(WORK);
  assert.ok(existsSync(mc), `prio-mc clone should exist at ${mc}`);
  const status = prioStatusText(WORK);
  assertStatusUnassignedEmpty(status);
});

// ── Phase 2: Branch creation and mv ──────────────────────────────────────────

let alphaFeatureSha;

await step("4. mv -c: create branch alpha from main only", async () => {
  writeFile(`${WORK}/alpha.txt`, "alpha\n");
  alphaFeatureSha = commitAll(WORK, "alpha feature");

  // mv syntax: prio mv [-c] <sha>... <destination>
  prioOk(WORK, "mv", "-c", alphaFeatureSha, "alpha");

  // For local-only branches, prio cherry-picks in prio-mc; the mc clone is authoritative.
  const mc = mcPath(WORK);
  const mcLog = git(mc, "log", "--oneline", "main..alpha");
  const lines = mcLog.split("\n").filter(Boolean);
  assert.equal(
    lines.length,
    1,
    `alpha (in prio-mc) should have exactly 1 commit above main, got:\n${mcLog}`
  );
  assert.ok(
    lines[0].includes("alpha feature"),
    'commit message should be "alpha feature"'
  );

  // The work branch (prio/test) should include alpha merged in
  const workLog = git(WORK, "log", "--oneline", "main..HEAD");
  assertIncludes(workLog, "alpha feature", "work branch after mv -c alpha");

  const status = prioStatusText(WORK);
  assertStatusHasBranch(status, "alpha");
  assertNoMergeConflict(status);
});

await step("5. mv: add second commit to existing branch alpha", async () => {
  writeFile(`${WORK}/alpha.txt`, "alpha v2\n");
  const sha = commitAll(WORK, "alpha follow-up");

  prioOk(WORK, "mv", sha, "alpha");

  // For local-only branches, check prio-mc's alpha ref for commit count
  const mc = mcPath(WORK);
  const mcLog = git(mc, "log", "--oneline", "main..alpha");
  assert.ok(
    mcLog.includes("alpha follow-up"),
    "follow-up commit should appear on alpha in prio-mc"
  );

  const lines = mcLog.split("\n").filter(Boolean);
  assert.equal(
    lines.length,
    2,
    `alpha (in prio-mc) should have 2 commits above main, got:\n${mcLog}`
  );

  assertNoMergeConflict(prioStatusText(WORK));
});

// ── Phase 3: Unassign and mv -a ───────────────────────────────────────────────

let floatSha;

await step("6. mv to unassigned (.)", async () => {
  writeFile(`${WORK}/float.txt`, "floating\n");
  floatSha = commitAll(WORK, "floating commit");

  prioOk(WORK, "mv", floatSha, ".");

  const status = prioStatusText(WORK);
  assertIncludes(
    status,
    "floating commit",
    "prio status after mv to unassigned"
  );
  assertIncludes(status, "Unassigned Commits:", "prio status");
  assertNotIncludes(status, "(none)", "unassigned should not be empty");
});

await step(
  "7. mv -a -c: move floating commit to new branch beta and apply",
  async () => {
    prioOk(WORK, "mv", "-a", "-c", floatSha, "beta");

    const status = prioStatusText(WORK);
    assertStatusHasBranch(status, "beta");
    assertStatusUnassignedEmpty(status);
    assertNoMergeConflict(status);

    // For local-only branches, check prio-mc's beta ref
    const mc = mcPath(WORK);
    const log = git(mc, "log", "--oneline", "main..beta");
    assert.ok(
      log.includes("floating commit"),
      "floating commit should be on beta in prio-mc"
    );
  }
);

// ── Phase 4: Explicit apply / unapply ────────────────────────────────────────

await step("8. unapply beta", async () => {
  prioOk(WORK, "unapply", "beta");

  const status = prioStatusText(WORK);
  // beta should not appear as an applied branch with checkmark
  assertNotIncludes(status, "✓  beta", "beta should not be applied");
  assertNoMergeConflict(status);
});

await step("9. apply alpha and beta together", async () => {
  prioOk(WORK, "apply", "alpha", "beta");

  const status = prioStatusText(WORK);
  assertStatusHasBranch(status, "alpha");
  assertStatusHasBranch(status, "beta");
  assertNoMergeConflict(status);
});

// ── Phase 5: Stack ────────────────────────────────────────────────────────────

await step("10. create gamma branch + stack after alpha", async () => {
  writeFile(`${WORK}/gamma.txt`, "gamma\n");
  const sha = commitAll(WORK, "gamma feature");

  prioOk(WORK, "mv", "-c", sha, "gamma");
  prioOk(WORK, "stack", "gamma", "alpha");

  const state = readRepoState(WORK);
  const entry = state.stacks.find((s) => s.branch === "gamma");
  assert.ok(entry, "stacks entry for gamma should exist in state.json");
  assert.deepEqual(
    entry.dependencies,
    ["alpha"],
    "gamma should depend on alpha"
  );

  // Apply all three; gamma stacked after alpha means no unnecessary conflict
  prioOk(WORK, "apply", "alpha", "beta", "gamma");
  const statusText = prioStatusText(WORK);
  assertNoMergeConflict(statusText);

  // ── Stack-aware status display ────────────────────────────────────────────
  // gamma's line should show the "(stacked after: alpha)" label.
  assertIncludes(statusText, "(stacked after: alpha)", "gamma status label");

  // gamma's unique commit should appear in the status.
  assertIncludes(statusText, "gamma feature", "gamma commit in status");

  // alpha's commits should appear only once (under alpha), NOT duplicated under
  // gamma.  Without the stacked-filter, "alpha feature" and "alpha follow-up"
  // would show up under both alpha and gamma — i.e. twice each.
  const alphaFeatureCount = (statusText.match(/alpha feature/g) || []).length;
  assert.equal(
    alphaFeatureCount,
    1,
    "alpha feature commit should appear only once in status (under alpha, not gamma)"
  );
  const alphaFollowupCount = (statusText.match(/alpha follow-up/g) || [])
    .length;
  assert.equal(
    alphaFollowupCount,
    1,
    "alpha follow-up commit should appear only once in status (under alpha, not gamma)"
  );
});

// ── Phase 6: Push and PR ──────────────────────────────────────────────────────

await step("11. push alpha to GitHub", async () => {
  prioOk(WORK, "push", "alpha");

  // Confirm origin/alpha exists in the work clone after push
  const verify = gitCapture(WORK, "rev-parse", "--verify", "origin/alpha");
  assert.equal(verify.status, 0, "origin/alpha should exist after prio push");
});

await step("12. pr alpha: create draft PR", async () => {
  // prio pr prints "SUCCESS: Created draft PR: <url>" to stderr via print_cli_result
  const out = prioOutCombined(WORK, "pr", "alpha");
  assertIncludes(out, "Created draft PR", "pr command output");
});

await step("13. prs: list open PRs (alpha visible)", async () => {
  // prs goes to stdout directly; it uses exec style
  const out = prioOut(WORK, "prs");
  assertIncludes(out, "alpha", "prs output should list alpha PR");
});

// ── Phase 7: Sync ─────────────────────────────────────────────────────────────

await step("14. upstream pushes a new commit to main", async () => {
  git(UPSTREAM, "fetch", "origin");
  git(UPSTREAM, "reset", "--hard", "origin/main");
  writeFile(`${UPSTREAM}/sync-marker.txt`, "synced\n");
  commitAll(UPSTREAM, "upstream main update");
  git(UPSTREAM, "push", "origin", "main");
});

await step("15. prio sync advances baseline", async () => {
  const beforeState = readRepoState(WORK);
  const before = beforeState.baseline_commit;

  prioOk(WORK, "sync");

  const afterState = readRepoState(WORK);
  const after = afterState.baseline_commit;

  assert.ok(
    after.length === 40,
    `baseline_commit should be a 40-char SHA, got: ${after}`
  );
  assert.notEqual(
    after,
    before,
    "baseline_commit should advance after upstream push + sync"
  );
});

await step(
  "16. simulate PR merge + prio sync removes merged branch",
  async () => {
    // Push beta so it can be merged, then merge it into main via upstream
    prioOk(WORK, "push", "beta");

    // Merge beta into main via the upstream clone
    git(UPSTREAM, "fetch", "origin");
    git(UPSTREAM, "reset", "--hard", "origin/main");
    git(
      UPSTREAM,
      "merge",
      "--no-ff",
      "origin/beta",
      "-m",
      "Merge beta into main"
    );
    git(UPSTREAM, "push", "origin", "main");

    // prio sync should detect beta is merged and remove it from applied_branches
    prioOk(WORK, "sync");

    const state = readRepoState(WORK);
    assert.ok(
      !state.applied_branches.includes("beta"),
      "beta should be removed from applied_branches after merge + sync"
    );
  }
);

// ── Phase 8: Recovery and cleanup ────────────────────────────────────────────

await step(
  "17. recover: rebuild after manually corrupted HEAD (no commit)",
  async () => {
    // prio recover is for emergency rollback when the work branch was corrupted
    // (e.g. partial apply, manual reset --hard) WITHOUT a normal user commit.
    // We simulate this by moving HEAD back one commit without committing.

    const beforeHead = headSha(WORK);

    // Move HEAD backwards without committing (simulates failed mid-apply state)
    git(WORK, "reset", "--hard", "HEAD~1");
    const corruptedHead = headSha(WORK);
    assert.notEqual(
      corruptedHead,
      beforeHead,
      "HEAD should have moved after manual reset"
    );

    // prio recover should rebuild the work area from last-good state
    prioOk(WORK, "recover");

    // After recover, prio status should be healthy (no conflict)
    assertNoMergeConflict(prioStatusText(WORK));

    // The work branch should have been rebuilt (HEAD differs from corrupted state)
    const afterHead = headSha(WORK);
    // Either restored to last-good or rebuilt to a valid merge state
    assert.notEqual(
      afterHead,
      corruptedHead,
      `HEAD should not still be at the corrupted commit after recover\n` +
        `(corrupted: ${corruptedHead}, after: ${afterHead})`
    );
  }
);

await step(
  "18. unstack gamma (local-only: rebase + pushed-branch guards)",
  async () => {
    const mc = mcPath(WORK);

    // Before unstack: status should still show the stacked label.
    const statusBefore = prioStatusText(WORK);
    assertIncludes(
      statusBefore,
      "(stacked after: alpha)",
      "stacked label should be present before unstack"
    );

    // gamma is local-only (never pushed).  Unstack should rebase it off alpha in
    // prio-mc so the gamma branch sits directly on main.
    prioOk(WORK, "unstack", "gamma");

    const stateAfter = readRepoState(WORK);
    assert.ok(
      !stateAfter.stacks.some((s) => s.branch === "gamma"),
      "gamma stack entry should be removed from state.json after unstack"
    );

    // Status should no longer show the stacked label.
    const statusAfter = prioStatusText(WORK);
    assertNotIncludes(
      statusAfter,
      "stacked after",
      "stacked label should be gone after unstack"
    );

    // prio-mc's gamma branch should now have only gamma's unique commit (rebased
    // directly onto main, dependency commits stripped out).
    const mcGammaLog = git(mc, "log", "--oneline", "main..gamma", "--");
    const mcGammaLines = mcGammaLog.trim().split("\n").filter(Boolean);
    assert.equal(
      mcGammaLines.length,
      1,
      `prio-mc gamma should have exactly 1 commit above main after rebase, got:\n${mcGammaLog}`
    );
    assert.ok(
      mcGammaLines[0].includes("gamma feature"),
      `prio-mc gamma commit should be "gamma feature", got: ${mcGammaLines[0]}`
    );

    // ── Pushed-branch error guard ─────────────────────────────────────────────
    // Re-stack gamma after alpha so we can test the pushed-branch path.
    prioOk(WORK, "stack", "gamma", "alpha");
    // Push gamma directly (bypasses prio push to set up the pushed state).
    git(WORK, "push", "origin", "gamma");

    // prio unstack without -k or -f on a pushed branch should fail.
    const noFlagResult = prioCapture(WORK, "unstack", "gamma");
    assert.notEqual(
      noFlagResult.status,
      0,
      "prio unstack on pushed branch (no flag) should exit non-zero"
    );
    assertIncludes(
      noFlagResult.stdout + noFlagResult.stderr,
      "pushed to origin",
      "error should mention that branch was pushed to origin"
    );

    // -k flag: metadata-only unstack should succeed even for a pushed branch.
    prioOk(WORK, "unstack", "-k", "gamma");
    const stateAfterK = readRepoState(WORK);
    assert.ok(
      !stateAfterK.stacks.some((s) => s.branch === "gamma"),
      "gamma stack entry should be removed after -k unstack"
    );
  }
);

await step("19. syncs: run prio syncs for all registered repos", async () => {
  prioOk(WORK, "syncs");
  // passes if exit 0 — there is at least one registered repo (this one)
});

// ── Phase 9: Merge conflict ───────────────────────────────────────────────────

await step(
  "20. set up conflict branches (two branches writing conflict.txt)",
  async () => {
    // Clear work area first
    const state = readRepoState(WORK);
    if (state.applied_branches.length > 0) {
      prioOk(WORK, "unapply", ...state.applied_branches);
    }

    // conflict-a: created via WORK using prio mv -c.
    // After unapply, prio/test is at baseline (conflict.txt = "base").
    // shaA diff: "base" → "version-a" — cherry-picks cleanly onto conflict-a in prio-mc.
    writeFile(`${WORK}/conflict.txt`, "version-a\n");
    const shaA = commitAll(WORK, "conflict a");
    // All above-baseline commits are now assigned → auto-apply fires.
    // conflict-a merges cleanly. prio/test now has conflict.txt = "version-a".
    prioOk(WORK, "mv", "-c", shaA, "conflict-a");

    // conflict-b: created INDEPENDENTLY via the upstream clone, branching from origin/main.
    // This ensures its commit diff is "base" → "version-b" (no dependency on conflict-a).
    // When prio later merges conflict-a + conflict-b, the 3-way merge detects the conflict:
    //   base = origin/main ("base"), ours = after conflict-a ("version-a"), theirs = conflict-b ("version-b").
    git(UPSTREAM, "fetch", "origin");
    git(UPSTREAM, "checkout", "-B", "conflict-b", "origin/main");
    writeFile(`${UPSTREAM}/conflict.txt`, "version-b\n");
    commitAll(UPSTREAM, "conflict b");
    git(UPSTREAM, "push", "-u", "origin", "conflict-b");

    // Fetch conflict-b into WORK as a local branch so prio-mc can find it.
    // prio-mc's origin is WORK (not GitHub), so prio-mc can only access branches that
    // exist locally in WORK. After this fetch, WORK has a local conflict-b pointing at
    // the GitHub commit, and prio-mc can fetch it as origin/conflict-b.
    git(WORK, "fetch", "origin", "conflict-b:conflict-b");

    // Apply both branches together. prio-mc merges conflict-a (success) then conflict-b
    // (conflict: both modify conflict.txt from "base"). Returns PrioResult::Warning → exit 0.
    // prioCapture ignores the exit code.
    prioCapture(WORK, "apply", "conflict-a", "conflict-b");
  }
);

await step(
  "21. prio status shows merge conflict banner with correct details",
  async () => {
    const status = prioStatusText(WORK);
    assertHasMergeConflict(status);
    // Incoming branch name should be visible (not blank)
    assertIncludes(
      status,
      "conflict-b",
      "status should name the incoming branch"
    );
    assertIncludes(
      status,
      "Resolve conflicts in:",
      "status should show resolution instructions"
    );
    assertIncludes(status, mcPath(WORK), "status should show the mc path");
    assertIncludes(status, "git -C", "status should show git commit command");
  }
);

await step("22. resolve conflict in prio-mc and continue apply", async () => {
  const mc = mcPath(WORK);

  // Write the resolution and commit in prio-mc.
  // The mc post-commit hook calls `prio internal-mc-post-commit` automatically,
  // which continues the merge chain and sync_work_clone completes the apply.
  writeFile(`${mc}/conflict.txt`, "resolved\n");
  git(mc, "add", "conflict.txt");
  git(mc, "commit", "--no-edit");

  // Explicitly invoke the mc post-commit continuation in case the hook didn't fire
  // (e.g. PATH not inherited). This is idempotent: if the hook already ran it returns
  // "No merge in progress." (exit 0) without doing anything harmful.
  exec(PRIO_BINARY, ["internal-mc-post-commit", `--mc-path=${mc}`]);

  assertNoMergeConflict(prioStatusText(WORK));
});

// ── Phase 10: mv cross-branch and cp ─────────────────────────────────────────
//
// Remove the conflict branches (conflict-a and conflict-b) from the applied set
// before the cross-branch and cp tests.  Each execute_apply_merge call rebuilds
// the work area from scratch, and conflict-b always conflicts with conflict-a on
// the GitHub origin — keeping them applied would cause every mv/cp apply to
// re-encounter that conflict, which is orthogonal to what the tests exercise.

prioOk(WORK, "unapply", "conflict-b");
prioOk(WORK, "unapply", "conflict-a");

let localSrcASha;
let pushedSrcASha;

await step(
  "23. mv cross-branch (local-only source: commit moved off branch)",
  async () => {
    const mc = mcPath(WORK);

    // Create two commits on the work branch then assign them to a local-only branch.
    // Use separate files so that after splitting, the two branches don't conflict on merge.
    writeFile(`${WORK}/local-src-alpha.txt`, "local-alpha\n");
    const localASha = commitAll(WORK, "local-src alpha");
    prioOk(WORK, "mv", "-c", localASha, "local-src");
    localSrcASha = localASha;

    writeFile(`${WORK}/local-src-beta.txt`, "local-beta\n");
    const localBSha = commitAll(WORK, "local-src beta");
    prioOk(WORK, "mv", localBSha, "local-src");

    // local-src now has 2 cherry-picks in prio-mc (alpha and beta above main).
    const mcLogBefore = git(mc, "log", "--oneline", "main..local-src", "--");
    assert.equal(
      mcLogBefore.trim().split("\n").filter(Boolean).length,
      2,
      `local-src should have 2 commits before cross-branch mv:\n${mcLogBefore}`
    );

    // Move only the alpha commit to a new branch — detected via commit_map.
    prioOk(WORK, "mv", "-c", localASha, "local-split");

    // local-src should now have only the beta commit.
    const mcSrcLog = git(mc, "log", "--oneline", "main..local-src", "--");
    const mcSrcLines = mcSrcLog.trim().split("\n").filter(Boolean);
    assert.equal(
      mcSrcLines.length,
      1,
      `local-src should have 1 commit after removing alpha:\n${mcSrcLog}`
    );
    assert.ok(
      mcSrcLines[0].includes("local-src beta"),
      `local-src should contain 'local-src beta', got: ${mcSrcLines[0]}`
    );

    // local-split should have exactly the alpha commit.
    const mcSplitLog = git(mc, "log", "--oneline", "main..local-split", "--");
    const mcSplitLines = mcSplitLog.trim().split("\n").filter(Boolean);
    assert.equal(
      mcSplitLines.length,
      1,
      `local-split should have 1 commit:\n${mcSplitLog}`
    );
    assert.ok(
      mcSplitLines[0].includes("local-src alpha"),
      `local-split should contain 'local-src alpha', got: ${mcSplitLines[0]}`
    );

    // Both branches should appear in prio status.
    const status = prioStatusText(WORK);
    assertStatusHasBranch(status, "local-src");
    assertStatusHasBranch(status, "local-split");
    assertNoMergeConflict(status);
  }
);

await step(
  "24. mv cross-branch (pushed source: error without -f, success with -f)",
  async () => {
    const mc = mcPath(WORK);

    // Create a fresh branch in the upstream clone and push it to origin.
    git(UPSTREAM, "fetch", "origin");
    git(UPSTREAM, "checkout", "-B", "pushed-src", "origin/main");
    writeFile(`${UPSTREAM}/pushed-src.txt`, "pushed alpha\n");
    pushedSrcASha = commitAll(UPSTREAM, "pushed-src alpha");
    writeFile(`${UPSTREAM}/pushed-src.txt`, "pushed beta\n");
    commitAll(UPSTREAM, "pushed-src beta");
    git(UPSTREAM, "push", "-u", "origin", "pushed-src");

    // Fetch + apply pushed-src in the WORK clone so prio-mc can access it.
    git(WORK, "fetch", "origin", "pushed-src:pushed-src");
    prioOk(WORK, "apply", "pushed-src");

    // Without -f: moving a commit off a pushed branch should fail.
    const noFlagResult = prioCapture(
      WORK,
      "mv",
      pushedSrcASha,
      "-c",
      "pushed-dest"
    );
    assert.notEqual(
      noFlagResult.status,
      0,
      "prio mv on pushed source (no -f) should exit non-zero"
    );
    const noFlagOut = noFlagResult.stdout + noFlagResult.stderr;
    assertIncludes(noFlagOut, "-f", "error should mention -f flag");
    assertIncludes(noFlagOut, "prio cp", "error should mention prio cp");

    // With -f: rebases pushed-src, force-pushes it, cherry-picks alpha to pushed-dest,
    // then auto-applies. The apply merges local-src, local-split, pushed-src cleanly but
    // hits a conflict on pushed-dest (both pushed-src and pushed-dest modify pushed-src.txt
    // from the same base). Prio returns a WARNING (exit 0).
    const mvResult = prioCapture(
      WORK,
      "mv",
      "-f",
      pushedSrcASha,
      "-c",
      "pushed-dest"
    );
    const mvOut = mvResult.stdout + mvResult.stderr;
    assertIncludes(
      mvOut,
      "Merge conflict in prio-mc",
      "mv -f output should report conflict"
    );
    assertIncludes(
      mvOut,
      "pushed-dest",
      "mv -f output should name the conflicting branch"
    );

    // prio-mc's pushed-src should have only the beta commit now.
    const mcSrcLog = git(mc, "log", "--oneline", "main..pushed-src", "--");
    const mcSrcLines = mcSrcLog.trim().split("\n").filter(Boolean);
    assert.equal(
      mcSrcLines.length,
      1,
      `pushed-src should have 1 commit after -f mv:\n${mcSrcLog}`
    );
    assert.ok(
      mcSrcLines[0].includes("pushed-src beta"),
      `pushed-src should contain 'pushed-src beta', got: ${mcSrcLines[0]}`
    );

    // prio-mc's pushed-dest should have the alpha commit.
    const mcDestLog = git(mc, "log", "--oneline", "main..pushed-dest", "--");
    const mcDestLines = mcDestLog.trim().split("\n").filter(Boolean);
    assert.equal(
      mcDestLines.length,
      1,
      `pushed-dest should have 1 commit:\n${mcDestLog}`
    );
    assert.ok(
      mcDestLines[0].includes("pushed-src alpha"),
      `pushed-dest should contain 'pushed-src alpha', got: ${mcDestLines[0]}`
    );

    // origin/pushed-src should have been force-pushed (only beta content).
    git(WORK, "fetch", "origin", "pushed-src");
    const originLog = git(WORK, "log", "--oneline", "main..origin/pushed-src");
    assert.equal(
      originLog.trim().split("\n").filter(Boolean).length,
      1,
      `origin/pushed-src should have 1 commit after force-push:\n${originLog}`
    );
    assert.ok(
      originLog.includes("pushed-src beta"),
      `origin/pushed-src should contain 'pushed-src beta', got: ${originLog}`
    );

    // prio status should now show the merge conflict banner with pushed-dest as the incoming branch.
    const status = prioStatusText(WORK);
    assertHasMergeConflict(status);
    assertIncludes(
      status,
      "pushed-dest",
      "status should name the incoming branch"
    );
    assertIncludes(
      status,
      "Resolve conflicts in:",
      "status should show resolution instructions"
    );

    // Resolve the conflict in prio-mc, then let the post-commit hook continue the apply.
    writeFile(`${mc}/pushed-src.txt`, "resolved-pushed\n");
    git(mc, "add", "pushed-src.txt");
    git(mc, "commit", "--no-edit");

    // Fallback: explicitly invoke mc-post-commit in case the hook didn't fire
    // (e.g. PATH not inherited). Idempotent if hook already ran.
    exec(PRIO_BINARY, ["internal-mc-post-commit", `--mc-path=${mc}`]);

    assertNoMergeConflict(prioStatusText(WORK));
  }
);

// Unapply pushed-src and pushed-dest before the cp test.  Both branches modify
// pushed-src.txt from the same base, so every subsequent apply rebuild would
// re-encounter the same conflict — orthogonal to what step 25 exercises.
prioOk(WORK, "unapply", "pushed-dest");
prioOk(WORK, "unapply", "pushed-src");

await step("25. cp: copy commit to new branch (source unchanged)", async () => {
  const mc = mcPath(WORK);

  // Create a fresh source branch via the upstream clone.
  git(UPSTREAM, "fetch", "origin");
  git(UPSTREAM, "checkout", "-B", "copy-src", "origin/main");
  writeFile(`${UPSTREAM}/copy-src.txt`, "copy content\n");
  const copySrcSha = commitAll(UPSTREAM, "copy-src original");
  git(UPSTREAM, "push", "-u", "origin", "copy-src");

  // Fetch + apply copy-src in WORK.
  git(WORK, "fetch", "origin", "copy-src:copy-src");
  prioOk(WORK, "apply", "copy-src");

  const mcSrcBefore = git(
    mc,
    "log",
    "--oneline",
    "main..origin/copy-src",
    "--"
  );
  assert.equal(
    mcSrcBefore.trim().split("\n").filter(Boolean).length,
    1,
    `copy-src should have 1 commit before cp:\n${mcSrcBefore}`
  );

  // Copy the commit to a new branch — source must remain unchanged.
  prioOk(WORK, "cp", "-c", copySrcSha, "cp-dest");

  // cp-dest should have the copied commit.
  const mcDestLog = git(mc, "log", "--oneline", "main..cp-dest", "--");
  const mcDestLines = mcDestLog.trim().split("\n").filter(Boolean);
  assert.equal(
    mcDestLines.length,
    1,
    `cp-dest should have 1 commit:\n${mcDestLog}`
  );
  assert.ok(
    mcDestLines[0].includes("copy-src original"),
    `cp-dest should contain 'copy-src original', got: ${mcDestLines[0]}`
  );

  // Source branch must still have its original commit (cp is non-destructive).
  const mcSrcAfter = git(mc, "log", "--oneline", "main..origin/copy-src", "--");
  assert.equal(
    mcSrcAfter.trim().split("\n").filter(Boolean).length,
    1,
    `copy-src should still have 1 commit after cp:\n${mcSrcAfter}`
  );
  assert.ok(
    mcSrcAfter.includes("copy-src original"),
    `copy-src should still contain 'copy-src original' after cp:\n${mcSrcAfter}`
  );

  assertNoMergeConflict(prioStatusText(WORK));
});

// ── Phase 11: mv cross-branch unassign ────────────────────────────────────────
//
// Verifies that `prio mv <sha> .` on a commit that lives on an applied branch
// (cross-branch unassign) correctly:
//   1. Removes the commit from the source branch in prio-mc.
//   2. Cherry-picks it onto the work branch via prio-mc (NOT directly on the
//      work clone — the mc-first invariant).
//   3. Leaves the commit visible as an "Unassigned Commit" in prio status.

await step(
  "26. mv cross-branch unassign (commit moved to work area)",
  async () => {
    const mc = mcPath(WORK);

    // Unapply copy-src and cp-dest to keep the apply stack clean.
    prioOk(WORK, "unapply", "cp-dest");
    prioOk(WORK, "unapply", "copy-src");

    // Create a local-only branch with one commit so we have a cross-branch target.
    writeFile(`${WORK}/unassign-test.txt`, "unassign content\n");
    const unassignSha = commitAll(WORK, "unassign-test commit");
    prioOk(WORK, "mv", "-c", unassignSha, "unassign-src");

    // Verify the commit is on unassign-src in prio-mc (not in the work area).
    const mcLogBefore = git(mc, "log", "--oneline", "main..unassign-src", "--");
    assert.ok(
      mcLogBefore.includes("unassign-test commit"),
      `unassign-src should contain the commit before unassign:\n${mcLogBefore}`
    );
    const statusBefore = prioStatusText(WORK);
    assertStatusHasBranch(statusBefore, "unassign-src");
    assertStatusUnassignedEmpty(statusBefore);

    // Unassign: move the commit from unassign-src back to the work area.
    prioOk(WORK, "mv", unassignSha, ".");

    // The source branch should now be empty (no commits above main).
    const mcLogAfter = git(mc, "log", "--oneline", "main..unassign-src", "--");
    assert.equal(
      mcLogAfter.trim(),
      "",
      `unassign-src should be empty after unassign, got:\n${mcLogAfter}`
    );

    // The commit should appear as an unassigned commit in prio status.
    const statusAfter = prioStatusText(WORK);
    assertNotIncludes(
      statusAfter,
      "(none)",
      "unassigned commits should not be empty after cross-branch unassign"
    );
    assertIncludes(
      statusAfter,
      "unassign-test commit",
      "the unassigned commit should appear in prio status"
    );
    assertNoMergeConflict(statusAfter);

    // Verify the work clone was updated via mc-first: the work branch should be
    // ahead of the apply baseline by exactly 1 commit (the unassigned commit).
    const state = readRepoState(WORK);
    const aboveBaseline = gitCapture(
      WORK,
      "log",
      `${state.baseline_commit}..HEAD`,
      "--oneline"
    )
      .stdout.trim()
      .split("\n")
      .filter(Boolean);
    assert.equal(
      aboveBaseline.length,
      1,
      `Expected 1 commit above baseline after unassign, got:\n${aboveBaseline.join(
        "\n"
      )}`
    );
    assert.ok(
      aboveBaseline[0].includes("unassign-test commit"),
      `Commit above baseline should be 'unassign-test commit', got: ${aboveBaseline[0]}`
    );
  }
);

// ── Phase 12: stale prio-mc local ref regression ─────────────────────────────
//
// Regression for: `prio mv -c` wiping commits off an untouched applied branch.
//
// Root cause (pre-fix): reset_mc_to_default only cleaned prio-mc/* branches, so
// local per-feature refs from previous operations accumulated in prio-mc across
// runs.  merge_ref_for_branch preferred local over origin/*, so a stale local
// ref was used during the apply merge; then the post-apply sync force-fetched
// the stale ref back to the work clone, discarding real commits.
//
// Fix: reset_mc_to_default now deletes ALL local branch refs (not just
// prio-mc/*).  rebase_filter_shas uses a targeted abort+checkout instead of
// reset_mc_to_default so freshly-built refs survive to the apply merge.  The
// pre-execute_apply_merge reset in mv.rs is similarly replaced with a targeted
// checkout.
//
// This step exercises the exact scenario from the original bug report:
// - an applied branch (stale-victim) has several pushed commits
// - prio-mc's local stale-victim ref is manually regressed to simulate staleness
// - a separate `prio mv -c` moves a commit off another branch
// - the test asserts stale-victim is completely unchanged in the work clone

await step(
  "27. mv -c preserves untouched applied branch refs (stale prio-mc ref regression)",
  async () => {
    const mc = mcPath(WORK);

    // ── 1. Build a multi-commit applied branch (the victim) ──────────────────

    writeFile(`${WORK}/sv1.txt`, "sv1\n");
    const sv1Sha = commitAll(WORK, "stale-victim: commit 1");
    prioOk(WORK, "mv", "-c", sv1Sha, "stale-victim");

    writeFile(`${WORK}/sv2.txt`, "sv2\n");
    const sv2Sha = commitAll(WORK, "stale-victim: commit 2");
    prioOk(WORK, "mv", sv2Sha, "stale-victim");

    writeFile(`${WORK}/sv3.txt`, "sv3\n");
    const sv3Sha = commitAll(WORK, "stale-victim: commit 3");
    prioOk(WORK, "mv", sv3Sha, "stale-victim");

    // Push so it has origin/<branch> tracking (mirrors the original bug scenario).
    git(WORK, "push", "origin", "stale-victim");

    // Record the correct tip — this must be preserved after the upcoming mv.
    const victimTipBefore = git(WORK, "rev-parse", "stale-victim").trim();

    // Verify prio-mc has the up-to-date local ref (it was synced by the last mv).
    const mcVictimCurrent = git(mc, "rev-parse", "stale-victim").trim();
    assert.equal(
      mcVictimCurrent,
      victimTipBefore,
      "prio-mc local stale-victim should match work before injection"
    );

    // ── 2. Inject a stale local ref in prio-mc ───────────────────────────────
    //
    // Simulate the pre-fix condition: reset_mc_to_default left per-feature refs
    // from prior operations intact, so prio-mc's local stale-victim could point
    // to an old commit.  Move it back to main tip (worst case: zero commits on
    // the branch from prio-mc's perspective).
    const mainInMc = git(mc, "rev-parse", "main").trim();
    git(mc, "branch", "-f", "stale-victim", mainInMc);

    // Confirm the injection made it stale.
    assert.notEqual(
      git(mc, "rev-parse", "stale-victim").trim(),
      victimTipBefore,
      "prio-mc stale-victim should differ from work after injection"
    );

    // ── 3. Create a source branch with one commit and push it ────────────────

    writeFile(`${WORK}/solo-src.txt`, "solo source\n");
    const soloSha = commitAll(WORK, "solo-src: the commit to move");
    prioOk(WORK, "mv", "-c", soloSha, "solo-src");
    git(WORK, "push", "origin", "solo-src");

    // ── 4. Move solo-src's commit to a new branch ────────────────────────────
    //
    // This is the operation that triggered the original bug:
    //   - reset_mc_to_default runs at the start (should now clear the stale ref)
    //   - execute_apply_merge rebuilds the work branch
    //   - post-apply sync must NOT overwrite stale-victim with prio-mc's old ref
    prioOk(WORK, "mv", "-f", soloSha, "-c", "solo-dest");

    // ── 5. Assert stale-victim is completely unchanged ───────────────────────

    const victimTipAfter = git(WORK, "rev-parse", "stale-victim").trim();
    assert.equal(
      victimTipAfter,
      victimTipBefore,
      `stale-victim tip was corrupted by the mv.\n` +
        `  expected (work tip before mv): ${victimTipBefore}\n` +
        `  actual  (work tip after  mv): ${victimTipAfter}\n` +
        `  prio-mc stale ref was:        ${mainInMc}`
    );

    // All three commits must still be visible in prio status.
    const status = prioStatusText(WORK);
    assertIncludes(status, "stale-victim: commit 1", "prio status after mv");
    assertIncludes(status, "stale-victim: commit 2", "prio status after mv");
    assertIncludes(status, "stale-victim: commit 3", "prio status after mv");
    assertNoMergeConflict(status);
  }
);

// ── Phase 13: prio stack with merge conflict ──────────────────────────────────
//
// Regression for: `prio stack` silently eating merge conflicts and reporting
// SUCCESS instead of WARNING.
//
// Root cause (pre-fix): run_stack ignored the PrioResult from apply::run and
// always called suggestions::run_and_log unconditionally, which invoked
// reset_mc_to_default (aborting the in-flight merge) before returning a
// misleading SUCCESS result.  prio status then saw merge_in_progress=true in
// state.json but a clean prio-mc worktree.
//
// Fix:
//   1. run_stack propagates apply::run's Warning result instead of overriding it.
//   2. suggestions::run_and_log guards against state.merge_in_progress.
//   3. prio status probes MERGE_HEAD and auto-re-applies when prio-mc is clean
//      but state says conflict (belt-and-suspenders auto-recovery).

{
  const mc = mcPath(WORK);

  // Unapply leftover branches from previous phases.
  const stateBeforeStack = readRepoState(WORK);
  if (stateBeforeStack.applied_branches.length > 0) {
    prioOk(WORK, "unapply", ...stateBeforeStack.applied_branches);
  }

  // Create stk-base: modifies stk-shared.txt = "stk-base-version"
  writeFile(`${WORK}/stk-shared.txt`, "stk-base-version\n");
  const stkBaseSha = commitAll(WORK, "stk-base commit");
  prioOk(WORK, "mv", "-c", stkBaseSha, "stk-base");

  // Create stk-top independently from main (via UPSTREAM) so its diff is
  // base → "stk-top-version" (no dependency on stk-base).  When applied after
  // stk-base, the 3-way merge will conflict on stk-shared.txt.
  git(UPSTREAM, "fetch", "origin");
  git(UPSTREAM, "checkout", "-B", "stk-top", "origin/main");
  writeFile(`${UPSTREAM}/stk-shared.txt`, "stk-top-version\n");
  commitAll(UPSTREAM, "stk-top commit");
  git(UPSTREAM, "push", "-u", "origin", "stk-top");

  // Fetch stk-top into WORK (prio-mc's origin is WORK, so it needs a local ref).
  git(WORK, "fetch", "origin", "stk-top:stk-top");

  // Apply stk-top after stk-base → conflict.  Use prioCapture to not throw.
  prioCapture(WORK, "apply", "stk-top");

  // Resolve the initial conflict so both branches are cleanly applied.
  writeFile(`${mc}/stk-shared.txt`, "stk-resolved\n");
  git(mc, "add", "stk-shared.txt");
  git(mc, "commit", "--no-edit");
  exec(PRIO_BINARY, ["internal-mc-post-commit", `--mc-path=${mc}`]);

  assertNoMergeConflict(
    prioStatusText(WORK),
    "status should be clean after initial conflict resolution"
  );

  await step(
    "28. prio stack warns on apply conflict (does not silently eat it)",
    async () => {
      // prio stack stk-top stk-base re-applies in order (stk-base first, then
      // stk-top on top).  stk-top's commit changes the same file that stk-base
      // already modified, so the merge conflicts.
      const stackResult = prioCapture(WORK, "stack", "stk-top", "stk-base");
      const stackOut = stackResult.stdout + stackResult.stderr;

      // Must surface WARNING, not SUCCESS.
      assertIncludes(
        stackOut,
        "WARNING",
        "prio stack should emit WARNING on conflict"
      );
      assertNotIncludes(
        stackOut,
        "Stacked stk-top",
        "prio stack should NOT emit success message"
      );

      // prio-mc must have a live MERGE_HEAD (conflict is materialised).
      const mergeHead = gitCapture(mc, "rev-parse", "--verify", "MERGE_HEAD");
      assert.equal(
        mergeHead.status,
        0,
        "MERGE_HEAD must exist in prio-mc after stack conflict"
      );

      // prio status must show the conflict banner.
      const status = prioStatusText(WORK);
      assertHasMergeConflict(status);
      assertIncludes(
        status,
        "stk-top",
        "status should name the incoming branch"
      );
    }
  );

  await step(
    "29. resolve stack conflict in prio-mc; status becomes clean",
    async () => {
      // Resolve in prio-mc and invoke the mc post-commit hook.
      writeFile(`${mc}/stk-shared.txt`, "stk-restacked-resolved\n");
      git(mc, "add", "stk-shared.txt");
      git(mc, "commit", "--no-edit");
      exec(PRIO_BINARY, ["internal-mc-post-commit", `--mc-path=${mc}`]);

      assertNoMergeConflict(prioStatusText(WORK));
    }
  );

  await step(
    "30. prio status auto-recovers when prio-mc conflict state is wiped",
    async () => {
      // Re-trigger a conflict via prio stack so prio-mc has MERGE_HEAD.
      // stk-top was restacked cleanly in step 29, so we need to force a new
      // conflict.  Unstack stk-top (metadata-only -k) and re-apply in the
      // conflicting order (stk-top first, then stk-base on top of it).
      prioOk(WORK, "unstack", "stk-top", "-k");
      prioOk(WORK, "unapply", "stk-base", "stk-top");

      // Apply in order that will conflict (stk-top first, stk-base second).
      prioCapture(WORK, "apply", "stk-top", "stk-base");

      // At this point prio-mc should have MERGE_HEAD + state.merge_in_progress = true.
      const mergeHeadBefore = gitCapture(
        mc,
        "rev-parse",
        "--verify",
        "MERGE_HEAD"
      );
      assert.equal(
        mergeHeadBefore.status,
        0,
        "MERGE_HEAD should exist before abort"
      );

      // Simulate the pre-fix bug: something (e.g. old suggestions code) aborts the
      // merge and checks out the default branch in prio-mc, leaving state.json still
      // claiming merge_in_progress = true.
      git(mc, "merge", "--abort");
      const defaultBranch = readRepoState(WORK).default_branch;
      git(mc, "checkout", defaultBranch);

      // Verify the contradiction: state says conflict, git says clean.
      const mergeHeadAfter = gitCapture(
        mc,
        "rev-parse",
        "--verify",
        "MERGE_HEAD"
      );
      assert.notEqual(
        mergeHeadAfter.status,
        0,
        "MERGE_HEAD should be gone after abort"
      );
      const stateAfterAbort = readRepoState(WORK);
      assert.ok(
        stateAfterAbort.merge_in_progress,
        "state.json should still say merge_in_progress"
      );

      // prio status should detect the contradiction and auto-re-apply (restoring the conflict).
      const statusOut = prioOutCombined(WORK, "status");
      assertIncludes(
        statusOut,
        "Detected stale conflict state",
        "status should print auto-recovery warning"
      );

      // After auto-recovery, status should show the conflict banner again.
      const statusAfter = prioStatusText(WORK);
      assertHasMergeConflict(statusAfter);

      // Clean up: resolve conflict so subsequent steps start fresh.
      const stateAfterRecovery = readRepoState(WORK);
      if (stateAfterRecovery.merge_in_progress) {
        writeFile(`${mc}/stk-shared.txt`, "recovery-resolved\n");
        git(mc, "add", "stk-shared.txt");
        git(mc, "commit", "--no-edit");
        exec(PRIO_BINARY, ["internal-mc-post-commit", `--mc-path=${mc}`]);
      }
    }
  );

  await step(
    "31. prio status continues after resolved conflict when hook did not fire",
    async () => {
      // Re-trigger the same conflict, then commit the resolution with the prio
      // hook intentionally disabled. Status should continue from the committed
      // merge result, not wipe prio-mc and recreate the conflict.
      prioOk(WORK, "unapply", "stk-base", "stk-top");
      prioCapture(WORK, "apply", "stk-top", "stk-base");

      const mergeHeadBefore = gitCapture(
        mc,
        "rev-parse",
        "--verify",
        "MERGE_HEAD"
      );
      assert.equal(
        mergeHeadBefore.status,
        0,
        "MERGE_HEAD should exist before resolving without the hook"
      );

      writeFile(`${mc}/stk-shared.txt`, "hookless-resolution\n");
      git(mc, "add", "stk-shared.txt");
      exec("git", ["-C", mc, "commit", "--no-edit"], {
        env: { PRIO_AUTOMATED: "1" },
      });

      const statusOut = prioOutCombined(WORK, "status");
      assertIncludes(
        statusOut,
        "post-commit hook did not run",
        "status should continue the committed merge resolution"
      );
      assertNotIncludes(
        statusOut,
        "Detected stale conflict state",
        "status must not re-apply from scratch after a committed resolution"
      );
      assertNoMergeConflict(statusOut);
    }
  );
}

// ── Phase 14: push dependency enforcement ─────────────────────────────────────
//
// A stacked branch cannot be pushed to origin if its dependency branches are
// still local-only (unpushed), because reviewers would then be unable to see
// the base context.  `prio push -p` overrides this by pushing all dependency
// branches first.

await step(
  "31. prio push blocks stacked branch when dependency is unpushed",
  async () => {
    // Unapply anything left from phase 13.
    const stateBefore = readRepoState(WORK);
    if (stateBefore.applied_branches.length > 0) {
      prioOk(WORK, "unapply", ...stateBefore.applied_branches);
    }

    // Create push-dep: a local-only applied branch.
    writeFile(`${WORK}/push-dep.txt`, "push-dep content\n");
    const pushDepSha = commitAll(WORK, "push-dep commit");
    prioOk(WORK, "mv", "-c", pushDepSha, "push-dep");

    // Create push-child: a local-only applied branch that depends on push-dep.
    writeFile(`${WORK}/push-child.txt`, "push-child content\n");
    const pushChildSha = commitAll(WORK, "push-child commit");
    prioOk(WORK, "mv", "-c", pushChildSha, "push-child");
    prioOk(WORK, "stack", "push-child", "push-dep");

    // `prio push push-child` (without -p) must fail because push-dep is unpushed.
    const noFlagResult = prioCapture(WORK, "push", "push-child");
    assert.notEqual(
      noFlagResult.status,
      0,
      "prio push of stacked branch with unpushed dep should exit non-zero"
    );
    const noFlagOut = noFlagResult.stdout + noFlagResult.stderr;
    assertIncludes(
      noFlagOut,
      "push-dep",
      "error should name the unpushed dependency"
    );
    assertIncludes(noFlagOut, "-p", "error should mention -p flag");

    // Verify push-dep is still not pushed.
    const depPushed = gitCapture(
      WORK,
      "rev-parse",
      "--verify",
      "origin/push-dep"
    );
    assert.notEqual(
      depPushed.status,
      0,
      "origin/push-dep should not exist yet"
    );
  }
);

await step(
  "32. prio push -p pushes dependency branches and then the target",
  async () => {
    // `prio push -p push-child` must push push-dep first, then push-child.
    prioOk(WORK, "push", "-p", "push-child");

    // Both push-dep and push-child must now exist on origin.
    const depPushed = gitCapture(
      WORK,
      "rev-parse",
      "--verify",
      "origin/push-dep"
    );
    assert.equal(
      depPushed.status,
      0,
      "origin/push-dep should exist after prio push -p"
    );

    const childPushed = gitCapture(
      WORK,
      "rev-parse",
      "--verify",
      "origin/push-child"
    );
    assert.equal(
      childPushed.status,
      0,
      "origin/push-child should exist after prio push -p"
    );
  }
);

// ── Done ──────────────────────────────────────────────────────────────────────

printSummary();
console.log("\n✓ All steps passed\n");

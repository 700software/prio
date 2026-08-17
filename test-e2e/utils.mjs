/**
 * test-e2e/utils.mjs — composable helpers for the prio e2e suite.
 *
 * All helpers throw on failure so the calling step aborts immediately.
 * Tests inside Docker: PRIO_BINARY points to the built release binary.
 */

import { spawnSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { dirname } from "node:path";
import assert from "node:assert/strict";

// ── Binary path ───────────────────────────────────────────────────────────────

export const PRIO_BINARY =
  process.env.PRIO_BINARY || "/prio/src-tauri/target/release/prio";

// ── Low-level subprocess helpers ─────────────────────────────────────────────

/**
 * Run a command synchronously. Streams output to the terminal.
 * Throws if the process exits non-zero.
 */
export function exec(cmd, args = [], opts = {}) {
  const result = spawnSync(cmd, args, {
    stdio: "inherit",
    env: { ...process.env, ...opts.env },
    cwd: opts.cwd,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(
      `Command failed (exit ${result.status}): ${cmd} ${args.join(" ")}`
    );
  }
  return result;
}

/**
 * Run a command and return its stdout as a string.
 * Throws if the process exits non-zero.
 */
export function execOut(cmd, args = [], opts = {}) {
  const result = spawnSync(cmd, args, {
    stdio: ["ignore", "pipe", "pipe"],
    env: { ...process.env, ...opts.env },
    cwd: opts.cwd,
    encoding: "utf8",
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    const stderr = result.stderr?.trim() ?? "";
    throw new Error(
      `Command failed (exit ${result.status}): ${cmd} ${args.join(
        " "
      )}\n${stderr}`
    );
  }
  return result.stdout ?? "";
}

/**
 * Run a command and return its stdout + stderr combined as a string.
 * Throws if the process exits non-zero.
 */
export function execOutCombined(cmd, args = [], opts = {}) {
  const result = spawnSync(cmd, args, {
    stdio: ["ignore", "pipe", "pipe"],
    env: { ...process.env, ...opts.env },
    cwd: opts.cwd,
    encoding: "utf8",
  });
  if (result.error) throw result.error;
  const combined = (result.stdout ?? "") + (result.stderr ?? "");
  if (result.status !== 0) {
    throw new Error(
      `Command failed (exit ${result.status}): ${cmd} ${args.join(
        " "
      )}\n${combined.trim()}`
    );
  }
  return combined;
}

/**
 * Run a command, return { stdout, stderr, status }.
 * Never throws — callers check the return value.
 */
export function execCapture(cmd, args = [], opts = {}) {
  const result = spawnSync(cmd, args, {
    stdio: ["ignore", "pipe", "pipe"],
    env: { ...process.env, ...opts.env },
    cwd: opts.cwd,
    encoding: "utf8",
  });
  return {
    stdout: result.stdout ?? "",
    stderr: result.stderr ?? "",
    status: result.status ?? -1,
    error: result.error,
  };
}

// ── Filesystem helpers ────────────────────────────────────────────────────────

export function writeFile(path, content) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, content, "utf8");
}

export function readFile(path) {
  return readFileSync(path, "utf8");
}

// ── Git helpers ───────────────────────────────────────────────────────────────

export function git(cwd, ...args) {
  return execOut("git", args, { cwd });
}

export function gitCapture(cwd, ...args) {
  return execCapture("git", args, { cwd });
}

export function headSha(cwd) {
  return git(cwd, "rev-parse", "HEAD").trim();
}

export function gitLogOneline(cwd, range) {
  return git(cwd, "log", "--oneline", range);
}

/** All commits on `branch` that are not on `main`. */
export function branchCommitsSinceMain(cwd, branch) {
  return git(cwd, "log", "--oneline", `main..${branch}`);
}

export function commitAll(cwd, message) {
  git(cwd, "add", "-A");
  git(cwd, "commit", "--allow-empty", "-m", message);
  return headSha(cwd);
}

export function freshClone(dir, url) {
  if (existsSync(dir)) {
    rmSync(dir, { recursive: true, force: true });
  }
  mkdirSync(dirname(dir), { recursive: true });
  exec("git", ["clone", url, dir]);
}

// ── gh helpers ────────────────────────────────────────────────────────────────

export function gh(...args) {
  return execOut("gh", args, {
    env: { GH_TOKEN: process.env.GH_TOKEN },
  });
}

export function ghCapture(...args) {
  return execCapture("gh", args, {
    env: { GH_TOKEN: process.env.GH_TOKEN },
  });
}

/**
 * Ensure `slug` (e.g. "myuser/tmp-prio-test") exists on GitHub.
 * Creates a private repo if missing.
 */
export async function ensureGithubRepo(slug) {
  const check = ghCapture("repo", "view", slug, "--json", "name");
  if (check.status === 0) {
    console.log(`  GitHub repo ${slug} already exists.`);
    return;
  }
  console.log(`  Creating GitHub repo ${slug} ...`);
  const [owner, name] = slug.split("/");
  gh(
    "repo",
    "create",
    slug,
    "--private",
    "--description",
    "prio e2e test repo (auto-created)"
  );
  console.log(`  Created: https://github.com/${slug}`);
}

// ── prio helpers ──────────────────────────────────────────────────────────────

/**
 * Run prio and stream output. Throws on non-zero exit (assertion: should succeed).
 */
export function prioOk(repoPath, ...args) {
  return exec(PRIO_BINARY, [`--repo=${repoPath}`, ...args]);
}

/**
 * Run prio and capture stdout as a string. Throws on non-zero exit.
 * Use this for commands that write their content to stdout (e.g. `status`, `prs`).
 */
export function prioOut(repoPath, ...args) {
  return execOut(PRIO_BINARY, [`--repo=${repoPath}`, ...args]);
}

/**
 * Run prio and capture stdout+stderr combined. Throws on non-zero exit.
 * Use this for commands whose result message lands on stderr (e.g. `pr`, `push`).
 */
export function prioOutCombined(repoPath, ...args) {
  return execOutCombined(PRIO_BINARY, [`--repo=${repoPath}`, ...args]);
}

/**
 * Run prio and return { stdout, stderr, status }.
 * Never throws — for commands expected to fail or where we inspect output.
 */
export function prioCapture(repoPath, ...args) {
  return execCapture(PRIO_BINARY, [`--repo=${repoPath}`, ...args]);
}

/**
 * Return the stdout of `prio status`. Throws on non-zero exit.
 */
export function prioStatusText(repoPath) {
  return prioOut(repoPath, "status");
}

// ── State inspection ──────────────────────────────────────────────────────────

export function readRepoState(repoPath) {
  const path = `${repoPath}/.git/prio/state.json`;
  return JSON.parse(readFile(path));
}

/**
 * Derive the prio-mc clone path from the work clone path.
 * Matches the default: <parent>/<reponame>-prio-mc
 */
export function mcPath(repoPath) {
  const parts = repoPath.split("/");
  const name = parts[parts.length - 1];
  const parent = parts.slice(0, -1).join("/");
  return `${parent}/${name}-prio-mc`;
}

// ── Assertions ────────────────────────────────────────────────────────────────

export function assertIncludes(haystack, needle, label = "") {
  const msg = label
    ? `Expected ${label} to include: ${needle}`
    : `Expected output to include: ${needle}`;
  assert.ok(haystack.includes(needle), `${msg}\n\nActual:\n${haystack}`);
}

export function assertNotIncludes(haystack, needle, label = "") {
  const msg = label
    ? `Expected ${label} NOT to include: ${needle}`
    : `Expected output NOT to include: ${needle}`;
  assert.ok(!haystack.includes(needle), `${msg}\n\nActual:\n${haystack}`);
}

export function assertStatusHasBranch(status, branchName) {
  assertIncludes(status, branchName, "prio status");
}

export function assertStatusUnassignedEmpty(status) {
  // Status should show "(none)" under Unassigned Commits
  const unassignedIdx = status.indexOf("Unassigned Commits:");
  assert.ok(
    unassignedIdx >= 0,
    'Expected "Unassigned Commits:" in status output'
  );
  const afterUnassigned = status.slice(unassignedIdx);
  assertIncludes(afterUnassigned, "(none)", "unassigned commits section");
}

export function assertNoMergeConflict(status) {
  assertNotIncludes(status, "Merge conflict in prio-mc", "prio status");
}

export function assertHasMergeConflict(status) {
  assertIncludes(status, "Merge conflict in prio-mc", "prio status");
}

// ── Step runner ───────────────────────────────────────────────────────────────

const ANSI_RESET = "\x1b[0m";
const ANSI_BOLD = "\x1b[1m";
const ANSI_GREEN = "\x1b[32m";
const ANSI_RED = "\x1b[31m";
const ANSI_YELLOW = "\x1b[33m";
const ANSI_CYAN = "\x1b[36m";

let stepCount = 0;
let passCount = 0;
let failCount = 0;

export async function step(name, fn) {
  stepCount++;
  const label = `${ANSI_BOLD}${ANSI_CYAN}[step ${stepCount}]${ANSI_RESET} ${name}`;
  console.log(`\n${label}`);
  const start = Date.now();
  try {
    await fn();
    const elapsed = Date.now() - start;
    passCount++;
    console.log(`  ${ANSI_GREEN}✓ passed${ANSI_RESET} (${elapsed}ms)`);
  } catch (err) {
    failCount++;
    console.error(`  ${ANSI_RED}✗ FAILED${ANSI_RESET}: ${err.message}`);
    throw err;
  }
}

export function printSummary() {
  console.log(`\n${"─".repeat(60)}`);
  console.log(
    `${ANSI_BOLD}Results:${ANSI_RESET} ` +
      `${ANSI_GREEN}${passCount} passed${ANSI_RESET}  ` +
      (failCount > 0
        ? `${ANSI_RED}${failCount} failed${ANSI_RESET}`
        : `${ANSI_YELLOW}0 failed${ANSI_RESET}`) +
      `  (${stepCount} total)`
  );
}

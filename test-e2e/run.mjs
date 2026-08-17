/**
 * test-e2e/run.mjs — host-side entry point.
 *
 * Usage:  node test-e2e/run.mjs
 *   (or)  npm test
 *
 * What this does:
 *   1. Preflight: verify `gh auth status` on the host.
 *   2. Extract GH_TOKEN from the host's gh session.
 *   3. Determine the GitHub repo slug (env GITHUB_REPO or derived from gh user).
 *   4. Build the Docker image (prio-e2e) from test-e2e/docker/Dockerfile.
 *   5. Run docker with:
 *        - The repo source bind-mounted at /prio
 *        - GH_TOKEN forwarded as env
 *        - GITHUB_REPO forwarded as env
 *        - A build+test CMD:
 *            cargo build --release -q && node /prio/test-e2e/e2e.mjs
 */

import { spawnSync } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(__dirname, "..");

// ── Helpers ───────────────────────────────────────────────────────────────────

function run(cmd, args, opts = {}) {
  const result = spawnSync(cmd, args, {
    stdio: opts.stdio ?? "inherit",
    encoding: opts.encoding,
    env: { ...process.env, ...opts.env },
  });
  if (result.error) {
    console.error(`Failed to spawn ${cmd}: ${result.error.message}`);
    process.exit(1);
  }
  return result;
}

function runOut(cmd, args, opts = {}) {
  return run(cmd, args, {
    ...opts,
    stdio: ["ignore", "pipe", "pipe"],
    encoding: "utf8",
  });
}

function die(msg) {
  console.error(`\n✗ ${msg}`);
  process.exit(1);
}

// ── Step 1: gh auth preflight ─────────────────────────────────────────────────

console.log("\n[preflight] Checking gh auth status on host...");
const authStatus = run("gh", ["auth", "status"], {
  stdio: ["ignore", "pipe", "pipe"],
  encoding: "utf8",
});
if (authStatus.status !== 0) {
  die(
    "gh auth status failed. Please run `gh auth login` on the host first.\n" +
      (authStatus.stderr ?? "")
  );
}
console.log("[preflight] gh auth OK");

// ── Step 2: Extract GH_TOKEN ──────────────────────────────────────────────────

console.log("[preflight] Extracting GH_TOKEN from host gh session...");
const tokenResult = runOut("gh", ["auth", "token"]);
if (tokenResult.status !== 0) {
  die(
    "Could not retrieve GH_TOKEN from gh auth token:\n" +
      (tokenResult.stderr ?? "")
  );
}
const GH_TOKEN = tokenResult.stdout.trim();
if (!GH_TOKEN) {
  die("GH_TOKEN is empty — ensure gh auth login was completed on the host.");
}
console.log("[preflight] GH_TOKEN obtained.");

// ── Step 3: Determine GITHUB_REPO ─────────────────────────────────────────────

let GITHUB_REPO = process.env.GITHUB_REPO;
if (!GITHUB_REPO) {
  console.log("[preflight] GITHUB_REPO not set — deriving from gh whoami...");
  const whoami = runOut("gh", ["api", "user", "--jq", ".login"]);
  if (whoami.status !== 0) {
    die(
      "Could not determine GitHub username from gh api.\n" +
        (whoami.stderr ?? "")
    );
  }
  const login = whoami.stdout.trim();
  if (!login) die("Could not determine GitHub username (empty response).");
  GITHUB_REPO = `${login}/tmp-prio-test`;
  console.log(`[preflight] Using GITHUB_REPO=${GITHUB_REPO}`);
}

// ── Step 4: Docker build ──────────────────────────────────────────────────────

const IMAGE = "prio-e2e";
const DOCKERFILE = resolve(__dirname, "docker", "Dockerfile");

console.log(`\n[docker] Building image ${IMAGE} from ${DOCKERFILE} ...`);
const buildResult = run("docker", [
  "build",
  "-t",
  IMAGE,
  "-f",
  DOCKERFILE,
  REPO_ROOT,
]);
if (buildResult.status !== 0) {
  die(`docker build failed (exit ${buildResult.status})`);
}
console.log("[docker] Image built.");

// ── Step 5: Docker run ────────────────────────────────────────────────────────

const CMD = [
  "bash",
  "-c",
  [
    // Configure git to authenticate via the forwarded GH_TOKEN
    "gh auth setup-git",
    // Build prio from the bind-mounted source
    "cd /prio && cargo build --release --manifest-path src-tauri/Cargo.toml -q",
    // Add prio binary to PATH so git hooks (post-commit) can invoke it
    "export PATH=/prio/src-tauri/target/release:$PATH",
    // Run the e2e scenario
    "node /prio/test-e2e/e2e.mjs",
  ].join(" && "),
];

console.log("[docker] Starting container ...\n");
const runResult = run("docker", [
  "run",
  "--rm",
  // Bind-mount the prio source so we test HEAD code and avoid copying huge build artefacts
  "--mount",
  `type=bind,source=${REPO_ROOT},target=/prio`,
  // Forward authentication
  "-e",
  `GH_TOKEN=${GH_TOKEN}`,
  "-e",
  `GITHUB_REPO=${GITHUB_REPO}`,
  // Optional overrides forwarded from host
  ...(process.env.PRIO_E2E_WORKSPACE
    ? ["-e", `PRIO_E2E_WORKSPACE=${process.env.PRIO_E2E_WORKSPACE}`]
    : []),
  // Isolate prio config from developer's own installation
  "-e",
  "PRIO_CONFIG_DIR=/tmp/prio-e2e/config",
  IMAGE,
  ...CMD,
]);

if (runResult.status !== 0) {
  die(`e2e container exited with status ${runResult.status}`);
}

console.log("\n✓ npm test passed\n");

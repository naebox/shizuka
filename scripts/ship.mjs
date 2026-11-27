#!/usr/bin/env node
/**
 * Zero-Failure CI & Release Shipping Script (`npm run ship` / `npm run ship:release`).
 *
 * Workflow:
 * 1. Runs local fast gates (`extract-release-notes --check-manifests`, `tsc`, `vite build`,
 *    and local `cargo clippy --all-targets -- -D warnings`).
 * 2. Pushes `HEAD` to an ephemeral remote branch `verify/<short-sha>` instead of `main`.
 * 3. Waits for GitHub Actions `CI` (Frontend + Windows x64 + macOS HAL & Clippy & Tests)
 *    to complete on that exact commit SHA.
 * 4. Always deletes the remote `verify/<short-sha>` branch when done.
 * 5. ONLY if every CI job succeeded: fast-forwards `origin/main` to `HEAD` (and, when
 *    `--release` or `--tag vX.Y.Z` is passed, creates and pushes the release tag).
 */

import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

function parseArgs(argv) {
  const opts = {
    release: false,
    tag: null,
    skipLocalRust: false,
  };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--release") {
      opts.release = true;
    } else if (arg === "--tag") {
      opts.tag = argv[++i] ?? null;
    } else if (arg === "--skip-local-rust") {
      opts.skipLocalRust = true;
    } else {
      throw new Error(`Unknown argument: ${arg}`);
    }
  }
  return opts;
}

function run(cmd, args, { capture = false } = {}) {
  if (capture) {
    return execFileSync(cmd, args, {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    }).trim();
  }
  execFileSync(cmd, args, { stdio: "inherit" });
  return "";
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function main() {
  const opts = parseArgs(process.argv.slice(2));
  const rootDir = process.cwd();

  // 1. Ensure working tree has no uncommitted changes to tracked files
  const dirty = run("git", ["status", "--porcelain", "--untracked-files=no"], {
    capture: true,
  });
  if (dirty) {
    throw new Error(
      `Working tree has uncommitted changes. Commit your changes first before running ship:\n${dirty}`
    );
  }

  const pkg = JSON.parse(
    fs.readFileSync(path.join(rootDir, "package.json"), "utf8")
  );
  const targetTag = opts.tag || (opts.release ? `v${pkg.version}` : null);

  if (targetTag) {
    const remoteTag = run("git", ["ls-remote", "--tags", "origin", targetTag], {
      capture: true,
    });
    if (remoteTag) {
      throw new Error(
        `Tag ${targetTag} already exists on origin. Bump version across manifests before shipping a new release.`
      );
    }
  }

  // 2. Local verification gates
  console.log("==> [1/4] Running local manifest, frontend, and Rust checks...");
  const checkArgs = ["scripts/extract-release-notes.mjs", "--check-manifests"];
  if (targetTag) {
    checkArgs.push("--tag", targetTag);
  }
  run(process.execPath, checkArgs);
  run(process.execPath, [
    path.join("node_modules", "typescript", "bin", "tsc"),
  ]);
  run(process.execPath, [
    path.join("node_modules", "vite", "bin", "vite.js"),
    "build",
  ]);

  if (!opts.skipLocalRust) {
    run("cargo", [
      "clippy",
      "--manifest-path",
      "src-tauri/Cargo.toml",
      "--all-targets",
      "--",
      "-D",
      "warnings",
    ]);
  }

  // 3. Push HEAD to ephemeral `verify/<shortSha>` branch for cross-OS GitHub Actions CI
  const fullSha = run("git", ["rev-parse", "HEAD"], { capture: true });
  const shortSha = fullSha.slice(0, 8);
  const verifyBranch = `verify/${shortSha}`;

  console.log(
    `==> [2/4] Pushing ${shortSha} to origin/${verifyBranch} for Ubuntu + Windows + macOS CI...`
  );
  run("git", [
    "push",
    "--force",
    "origin",
    `${fullSha}:refs/heads/${verifyBranch}`,
  ]);

  let ciPassed = false;
  let runId = null;

  try {
    // Poll until GitHub registers the workflow run for this exact commit SHA
    for (let attempt = 0; attempt < 20; attempt += 1) {
      await sleep(3000);
      const raw = run(
        "gh",
        [
          "api",
          `repos/{owner}/{repo}/actions/workflows/ci.yml/runs?branch=${encodeURIComponent(
            verifyBranch
          )}&head_sha=${fullSha}&per_page=5`,
        ],
        { capture: true }
      );
      const parsed = JSON.parse(raw);
      const match = (parsed.workflow_runs ?? []).find(
        (r) => r.head_sha === fullSha
      );
      if (match) {
        runId = match.id;
        console.log(`==> [3/4] Watching CI run #${runId}: ${match.html_url}`);
        break;
      }
    }

    if (!runId) {
      throw new Error(
        `Timed out waiting for GitHub Actions CI run to start for ${verifyBranch} (${fullSha}).`
      );
    }

    // Poll run completion (timeout: 35 minutes)
    const deadline = Date.now() + 35 * 60 * 1000;
    while (Date.now() < deadline) {
      const rawRun = run(
        "gh",
        ["api", `repos/{owner}/{repo}/actions/runs/${runId}`],
        { capture: true }
      );
      const runInfo = JSON.parse(rawRun);
      if (runInfo.status === "completed") {
        if (runInfo.conclusion === "success") {
          console.log(`==> CI run #${runId} passed on all platforms!`);
          ciPassed = true;
          break;
        }
        console.error(
          `\n==> CI run #${runId} FAILED (conclusion: ${runInfo.conclusion}). Fetching failure logs:\n`
        );
        try {
          run("gh", ["run", "view", String(runId), "--log-failed"]);
        } catch {
          // Ignore secondary error if log fetch fails
        }
        throw new Error(
          `Cross-platform CI failed on ${verifyBranch}. main and release tags were NOT modified.`
        );
      }
      process.stdout.write(
        `    [CI #${runId}] status=${runInfo.status} — waiting 15s...\n`
      );
      await sleep(15000);
    }

    if (!ciPassed) {
      throw new Error(`Timed out waiting for CI run #${runId} to complete.`);
    }
  } finally {
    console.log(`==> Cleaning up remote temporary branch ${verifyBranch}...`);
    try {
      run("git", ["push", "origin", "--delete", verifyBranch]);
    } catch {
      console.warn(`Warning: failed to delete remote branch ${verifyBranch}`);
    }
  }

  // 4. Fast-forward `origin/main` (and push release tag if requested)
  console.log(`==> [4/4] Fast-forwarding origin/main to verified ${shortSha}...`);
  run("git", ["push", "origin", `${fullSha}:refs/heads/main`]);

  if (targetTag) {
    const existingLocalTag = run("git", ["tag", "-l", targetTag], {
      capture: true,
    });
    if (!existingLocalTag) {
      run("git", ["tag", targetTag, fullSha]);
    }
    console.log(`==> Pushing verified release tag ${targetTag} to origin...`);
    run("git", ["push", "origin", `refs/tags/${targetTag}`]);
  }

  console.log("==> Ship completed cleanly!");
}

main().catch((err) => {
  console.error(`\nShip aborted: ${err instanceof Error ? err.message : err}`);
  process.exit(1);
});

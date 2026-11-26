#!/usr/bin/env node
/**
 * Validates unified version across Shizuka manifests and extracts bilingual (JA / EN)
 * release notes for a given version from CHANGELOG.md.
 *
 * Used by:
 * - `.github/workflows/ci.yml` (`--check-manifests`) so forgetting CHANGELOG.md (or either language) fails CI immediately
 * - `.github/workflows/release.yml` (`--check-manifests --markdown-out ... --summary-out ...`)
 *   so GitHub Releases and `latest.json` always contain bilingual Japanese & English release notes.
 */

import fs from "node:fs";
import path from "node:path";
import process from "node:process";

function parseArgs(argv) {
  const opts = {
    version: null,
    tag: null,
    checkManifests: false,
    githubOutput: null,
    markdownOut: null,
    summaryOut: null,
  };

  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--check-manifests") {
      opts.checkManifests = true;
    } else if (arg === "--version") {
      opts.version = argv[++i] ?? null;
    } else if (arg === "--tag") {
      opts.tag = argv[++i] ?? null;
    } else if (arg === "--github-output") {
      opts.githubOutput = argv[++i] ?? null;
    } else if (arg === "--markdown-out") {
      opts.markdownOut = argv[++i] ?? null;
    } else if (arg === "--summary-out") {
      opts.summaryOut = argv[++i] ?? null;
    } else {
      throw new Error(`Unknown argument: ${arg}`);
    }
  }
  return opts;
}

function escapeRegExp(str) {
  return str.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function readManifestVersions(rootDir) {
  const pkg = JSON.parse(
    fs.readFileSync(path.join(rootDir, "package.json"), "utf8")
  );
  const tauriConf = JSON.parse(
    fs.readFileSync(path.join(rootDir, "src-tauri", "tauri.conf.json"), "utf8")
  );
  const cargoToml = fs.readFileSync(
    path.join(rootDir, "src-tauri", "Cargo.toml"),
    "utf8"
  );
  const cargoMatch = cargoToml.match(/^version\s*=\s*"([^"]+)"/m);
  if (!cargoMatch) {
    throw new Error("Could not parse version from src-tauri/Cargo.toml");
  }

  const infoPlist = fs.readFileSync(
    path.join(rootDir, "drivers", "macos-hal", "Info.plist"),
    "utf8"
  );
  const halMatch = infoPlist.match(
    /<key>CFBundleShortVersionString<\/key>\s*<string>([^<]+)<\/string>/
  );
  if (!halMatch) {
    throw new Error(
      "Could not parse CFBundleShortVersionString from drivers/macos-hal/Info.plist"
    );
  }

  return {
    pkgVersion: String(pkg.version ?? "").trim(),
    tauriVersion: String(tauriConf.version ?? "").trim(),
    cargoVersion: cargoMatch[1].trim(),
    halVersion: halMatch[1].trim(),
  };
}

function extractChangelogEntry(changelogPath, version) {
  if (!fs.existsSync(changelogPath)) {
    throw new Error(
      `CHANGELOG.md not found at ${changelogPath}. Every release must document its changes.`
    );
  }

  const lines = fs
    .readFileSync(changelogPath, "utf8")
    .replace(/\r\n/g, "\n")
    .split("\n");

  const headerRe = new RegExp(
    `^##\\s+\\[${escapeRegExp(version)}\\]` +
      `(?:\\s*\\([^)]*\\))?` +
      `(?:\\s*(?:—|--|-)\\s*(.+))?\\s*$`
  );

  let startIdx = -1;
  let subtitle = "";
  for (let i = 0; i < lines.length; i += 1) {
    const m = lines[i].match(headerRe);
    if (m) {
      startIdx = i + 1;
      subtitle = (m[1] ?? "").trim();
      break;
    }
  }

  if (startIdx === -1) {
    throw new Error(
      `CHANGELOG.md is missing a section for version ${version}. ` +
        `Add '## [${version}] (YYYY-MM-DD) — <変更概要 / Summary>' with '### 主な変更点' and '### What's Changed' before releasing.`
    );
  }

  const sectionLines = [];
  for (let i = startIdx; i < lines.length; i += 1) {
    if (/^##\s+\[/.test(lines[i])) {
      break;
    }
    sectionLines.push(lines[i]);
  }

  while (
    sectionLines.length > 0 &&
    (sectionLines[0].trim() === "" || sectionLines[0].trim() === "---")
  ) {
    sectionLines.shift();
  }
  while (
    sectionLines.length > 0 &&
    (sectionLines[sectionLines.length - 1].trim() === "" ||
      sectionLines[sectionLines.length - 1].trim() === "---")
  ) {
    sectionLines.pop();
  }

  const body = sectionLines.join("\n").trim();
  if (!body || !/^-\s+\S+/m.test(body)) {
    throw new Error(
      `CHANGELOG.md section for version ${version} is empty or has no bullet items ('- ...').`
    );
  }

  const hasJaHeading = /^###\s+主な変更点\s*$/m.test(body);
  const hasEnHeading = /^###\s+What's Changed\s*$/m.test(body);
  if (!hasJaHeading || !hasEnHeading) {
    throw new Error(
      `CHANGELOG.md section for version ${version} must include both '### 主な変更点' (Japanese) and '### What's Changed' (English) subsections.`
    );
  }

  return { subtitle, body };
}

function buildReleaseMarkdown(version, subtitle, body) {
  const heading = subtitle
    ? `## Shizuka v${version} — ${subtitle}`
    : `## Shizuka v${version}`;

  return [
    heading,
    "",
    body,
    "",
    "### ダウンロード / Downloads",
    `- **Windows（推奨インストーラ / Recommended Installer）**: \`Shizuka_${version}_x64-setup.exe\``,
    `- **Windows（ポータブル版 / Portable ZIP）**: \`Shizuka_${version}_windows_x64_portable.zip\``,
    `- **Windows（MSI パッケージ / MSI Package）**: \`Shizuka_${version}_x64_en-US.msi\``,
    `- **macOS（Apple Silicon / M1+）**: \`Shizuka_${version}_aarch64.dmg\``,
    `- **macOS（Intel）**: \`Shizuka_${version}_x64.dmg\``,
    "- **チェックサム / Checksums**: `SHA256SUMS.txt`",
    "",
    "> [!NOTE]",
    "> **初回起動時の OS 確認画面について / First-Launch OS Prompt (Unsigned Personal Build)**",
    "> - **Windows**: SmartScreen が表示された場合は「詳細情報」→「実行」（**More info** → **Run anyway**）を選択してください。",
    "> - **macOS**: 初回はアプリアイコンを右クリック →「開く」（Right-click → **Open**, or **System Settings → Privacy & Security → Open Anyway**）から起動してください。",
    "",
  ].join("\n");
}

function buildUpdaterSummary(version, subtitle, body) {
  const titleLine = subtitle
    ? `Shizuka v${version} — ${subtitle}`
    : `Shizuka v${version}`;
  return `${titleLine}\n\n${body}\n`;
}

function main() {
  const opts = parseArgs(process.argv.slice(2));
  const rootDir = process.cwd();
  const manifests = readManifestVersions(rootDir);

  if (opts.checkManifests) {
    const { pkgVersion, tauriVersion, cargoVersion, halVersion } = manifests;
    if (
      pkgVersion !== tauriVersion ||
      pkgVersion !== cargoVersion ||
      pkgVersion !== halVersion
    ) {
      throw new Error(
        `Version mismatch across manifests: package.json=${pkgVersion}, ` +
          `tauri.conf.json=${tauriVersion}, Cargo.toml=${cargoVersion}, ` +
          `drivers/macos-hal/Info.plist=${halVersion}`
      );
    }
  }

  let version = opts.version;
  if (!version) {
    if (opts.tag) {
      version = opts.tag.replace(/^v/, "");
    } else {
      version = manifests.pkgVersion;
    }
  }

  const tag = opts.tag || `v${version}`;
  if (opts.checkManifests) {
    if (version !== manifests.pkgVersion) {
      throw new Error(
        `Requested version (${version}) does not match package.json (${manifests.pkgVersion})`
      );
    }
    if (tag !== `v${manifests.pkgVersion}`) {
      throw new Error(
        `Release tag (${tag}) does not match package version (v${manifests.pkgVersion})`
      );
    }
  }

  const changelogPath = path.join(rootDir, "CHANGELOG.md");
  const { subtitle, body } = extractChangelogEntry(changelogPath, version);
  const releaseMarkdown = buildReleaseMarkdown(version, subtitle, body);
  const updaterSummary = buildUpdaterSummary(version, subtitle, body);

  if (opts.markdownOut) {
    fs.writeFileSync(opts.markdownOut, releaseMarkdown, "utf8");
  }
  if (opts.summaryOut) {
    fs.writeFileSync(opts.summaryOut, updaterSummary, "utf8");
  }
  if (opts.githubOutput) {
    fs.appendFileSync(
      opts.githubOutput,
      `version=${version}\ntag=${tag}\n`,
      "utf8"
    );
  }

  console.log(
    `Validated bilingual release notes for Shizuka v${version}${
      subtitle ? ` (${subtitle})` : ""
    }`
  );
}

try {
  main();
} catch (err) {
  const msg = err instanceof Error ? err.message : String(err);
  if (process.env.GITHUB_ACTIONS === "true") {
    console.error(`::error::${msg}`);
  } else {
    console.error(`Error: ${msg}`);
  }
  process.exit(1);
}

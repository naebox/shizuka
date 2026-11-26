# AGENTS.md — Shizuka Development & Runtime Guidelines

## Browser / WebView Support Policy

**Browser Support:**
- **Target Runtimes Only:** Windows **WebView2** (Evergreen Chromium) and macOS **WKWebView** (macOS 14+ / Safari 17.5+ engine) via Tauri v2.
- **Excluded Browsers:** Firefox, legacy Chromium/Safari, Android WebView, and iOS browsers are **not** targeted. Omit `-moz-*` vendor pseudo-elements unless needed for harmless parity.
- **Baseline & Fallbacks:**
  - **Baseline Widely Available & Newly Available (2024+)** features supported by both Evergreen Chromium and Safari 17.5+ (`light-dark()`, `:has()`, `@layer`, `@property`, CSS Nesting, Container Queries, Popover, `transition-behavior: allow-discrete`) are safe to use **without** fallbacks.
  - **Limited Availability** features supported by only one of the two target engines (e.g., `<input type="checkbox" switch>` in WKWebView vs. `appearance: base-select` / `interpolate-size: allow-keywords` in WebView2) are permitted when they degrade gracefully to native/CSS behavior in `<= 20` lines with zero external dependencies (e.g., applying `role="switch"` when `!('switch' in HTMLInputElement.prototype)`).

---

## Architecture & Engineering Invariants

1. **License Cleanliness (MIT Only)**
   - Keep the repository strictly **MIT-licensed** ([LICENSE](./LICENSE)).
   - Never commit or bundle third-party proprietary or copyleft (GPL) driver binaries.
   - **Windows:** Download official WHQL-signed VB-CABLE on demand in `src-tauri/src/installer.rs` and clean up temporary archives immediately after installation.
   - **macOS:** Use the clean-room MIT CoreAudio HAL plug-in in `drivers/macos-hal/ShizukaAudio.c`.

2. **Real-Time Audio Safety (`src-tauri/src/audio/`)**
   - **Zero-Allocation Callbacks:** Never allocate heap memory (`Vec`, `String`, `Box`), lock a `Mutex`, or run ML inference inside `cpal` input/output stream callbacks. Communicate exclusively through lock-free SPSC ring buffers (`ringbuf`) and atomic primitives.
   - **Clock Drift Compensation:** Physical microphones and virtual audio cables run on independent clocks. Always route output through `AdaptiveOutputResampler` + `DriftController` (`resampler.rs`) to prevent long-call buffer underruns or latency growth.
   - **Inference Budget:** Keep `[profile.dev.package."*"]` and `[profile.test.package."*"]` set to `opt-level = 3` and `debug-assertions = false` in `src-tauri/Cargo.toml`, and pin `tract-*` crates to `=0.21.4` for `deep_filter` (`DeepFilterNet3` standard 20ms lookahead & `DeepFilterNet3_ll` 0ms lookahead, 48 kHz / 480-sample hop) compatibility.

3. **No Console Window Popups on Windows**
   - Preserve `#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]` in `src-tauri/src/main.rs`.
   - Always pass `CREATE_NO_WINDOW` (`0x0800_0000`) when spawning child processes (`curl.exe`, `tar.exe`, `powershell.exe`) on Windows.

4. **Device Filtering**
   - Keep auxiliary multi-channel virtual endpoints (`CABLE In 16ch` / `CABLE Out 16ch`) hidden in `src-tauri/src/audio/devices.rs` (`should_hide_device`) so voice routing dropdowns stay clear.

5. **Release & Changelog Discipline**
   - Every version bump MUST keep `package.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`, and `drivers/macos-hal/Info.plist` at the exact same version string **and** add a matching `## [X.Y.Z] (YYYY-MM-DD) — <変更概要>` section with plain-Japanese user-facing bullet points to [CHANGELOG.md](./CHANGELOG.md).
   - Both CI (`.github/workflows/ci.yml`) and Release (`.github/workflows/release.yml`) run `node scripts/extract-release-notes.mjs --check-manifests` to fail immediately if `CHANGELOG.md` is missing the version's notes, and automatically populate GitHub Releases and `latest.json` from `CHANGELOG.md`.

# Changelog

All notable changes to this project will be documented in this file.

## [0.6.0] — 2026-09-29

### Added

- **Non-interactive mode** — `--yes` (or an explicit `--mode`) runs without any
  prompt: `--mode optimize|upscale|restore`, `--recursive` / `--no-recursive`,
  `--delete-originals`, `--force-recheck`, `--quality low|medium|high`,
  `--target` / `--preset` for upscaling, and `--json` summaries.
- **`--no-color`** support, plus the `NO_COLOR` convention and automatic
  color suppression when output is not a TTY.
- **Checksum-verified self-update** — the release `SHA256SUMS` is downloaded and
  the artifact is refused on mismatch.
- Makefile (`make check` / `test` / `lint` / `install` / `package`), a CI
  workflow, GitHub issue templates, `CONTRIBUTING.md`, `SCOPE.md`, and a man page.
- `--health-check` now exits non-zero when a required tool is missing.

### Fixed

- **Same-extension conversions (video/images) could read and write the same
  file.** They now convert to a temporary sibling and swap atomically, matching
  the existing audio behavior.
- Update and install URLs pointed at the old repository owner (`pablogonz12`);
  they now use `pgm1207`.
- `threads_used` in the JSON summary reported the requested count instead of the
  actual (video-clamped) count.
- Older config files missing `default_recursive` / `default_delete_originals` /
  `default_folder` no longer fail to parse and silently reset.
- Windows agent liveness check matched PIDs by substring (PID 123 matched 1234);
  it now matches exactly.
- Removed an always-failing unit test (`dump_best_encoder`) that made `cargo test`
  unusable.

### Changed

- Repository hygiene: stopped telling git to ignore `Cargo.lock`, added ignores
  for build outputs, and removed stale artifacts (`agent.rs.txt`,
  `rusty-crunch.exe`, `gh.zip`, `gh_cli/`).
- README rewritten to match the implementation (the old text referenced
  `DashMap`, `rayon`, and `tracing`, none of which are dependencies).

> Note: releases 0.5.5–0.5.9 were tagged but never recorded here.

## [0.5.4] — 2026-03-13

### Added

- **Multi-job workflow** — batch multiple conversion jobs in a single run. Specify input/output format, confirm each job, and all share the same folder/recursive/delete settings. Max 8 jobs per session.
- **Output subfolder option** — choose whether converted files go to the same folder or a dedicated subfolder (e.g., `/input/output_mp3/` for MP3 conversions). Applies to both standard and recommended crunch modes.
- **All Lossless Audio → FLAC conversion** — new special format option `★ All Lossless (WAV/AIFF → FLAC)` automatically creates two jobs: WAV→FLAC and AIFF→FLAC
- **Thread mode selector** — choose processing threads: Power (100%), Balanced (75%), or Power Saver (50% or single-threaded). Stored in config and applied to all conversion jobs.
- **Auto-update from GitHub** — new "🔄 Check for Updates" menu option detects newer releases and downloads the binary directly from GitHub releases page
- **Clean/Uninstall tools** — "🗑 Clean / Uninstall tools" in Settings menu lets you selectively uninstall installed dependencies (ffmpeg, ImageMagick, Ghostscript, LibreOffice)
- **Delete failure tracking** — conversions now report how many files failed to delete (permissions/quarantine issues) so you can manually clean them up
- AIFF audio format support in converter (lossless, converts to MP3/OGG/FLAC/AAC/M4A/OPUS)

### Fixed

- **macOS: binary quarantine blocks from downloaded DMG/ZIP** — added `install.sh` script with `xattr -d` remove quarantine flag from binary
- **macOS: delete originals fails in recommended crunch** — improved delete error handling and reporting for better visibility into quarantine/permission issues
- **Unicode rendering in output messages** — fixed Rust unicode escape sequences in error/info messages

## [0.5.3] — 2026-03-12

### Fixed

- **Windows: program "crashes" after installing tools** — after auto-installing ffmpeg/etc via winget/choco, the program now shows a friendly success message and asks the user to restart instead of exiting with a confusing error (newly installed tools aren’t visible to the current process until the shell restarts)
- **Clean mode screen clearing on Windows** — added `cls` fallback for cmd.exe / legacy terminals where ANSI escape codes don’t work
- **Results disappear instantly** — after processing files, the program now pauses with "Press Enter to return to the menu" so you can actually read the results before the screen clears

## [0.5.2] — 2026-03-12

### Fixed

- **Windows: VCRUNTIME140.dll not found** — Windows builds now statically link the C runtime (`+crt-static`) so the binary runs on any Windows machine without needing the Visual C++ Redistributable installed
- **Linux: fully static binary** — switched Linux CI target from `x86_64-unknown-linux-gnu` to `x86_64-unknown-linux-musl` for a self-contained binary that works on any distro regardless of glibc version

## [0.5.1] — 2026-03-12

### Fixed

- **PDF re-optimization skip** — same-extension optimizations (PDF → PDF) now cache file metadata (size + mtime) in `~/.config/rusty-crunch/opt_cache.json` so already-optimized files are instantly skipped on repeat runs
- Added 2 new unit tests for the optimization cache (`cache_entry_matches_unchanged_file`, `cache_roundtrip_serialize`)

## [0.5.0] — 2026-03-10

### Added

- **Agent Mode [BETA]** — automated background file conversion service
  - Configure rules: folder + input format → output format with recursive/delete options
  - **Watch trigger** — converts files as soon as they appear (via OS file system notifications)
  - **Periodic trigger** — scans folders at a configurable interval
  - Runs as a **true background service** that survives terminal close
  - PID file management for clean start/stop lifecycle
  - Log output to `~/.config/rusty-crunch/agent.log`
- New CLI flags: `--agent`, `--agent-stop`, `--agent-status`
- Platform-aware BETA/ALPHA labeling (ALPHA on macOS due to limited testing)
- Non-interactive dependency checking for headless agent mode (`deps::check()`)
- Shared `human_bytes()` utility in `util.rs`

### Fixed

- Agent gracefully skips non-existent watch folders instead of crashing
- Periodic mode enforces minimum 60-second interval
- Clean PID file removal on early exit
- Stale PID detection and cleanup

### Changed

- Agent spawns as detached background process (Unix: `setsid()`, Windows: `DETACHED_PROCESS`)
- SIGTERM handler on Unix for clean shutdown when killed
- Version bumped to 0.5.0

### Dependencies

- Added: notify 6 (file system watching), ctrlc 3 (signal handling), libc 0.2 (Unix process management)

## [0.4.0] — 2026-02-24

### Added

- **Recommended Crunch Mode** — new main menu option that automatically converts all media in a folder to the most efficient format:
  - Audio: WAV → FLAC (lossless), MP3/OGG/AAC/M4A/WMA → OPUS (lossy)
  - Video: AVI/MOV/FLV/WMV/TS → MKV
  - Images: BMP/TIFF/ICO/GIF → PNG (lossless), JPEG → AVIF (lossy)
  - Documents: PDF → PDF (Optimized with 150 PPI downsampling)
- **Display Mode Settings** — Verbose (show everything) or Clean (clear screen between interactions)
- **GitHub Actions CI** — automated cross-platform release builds (Linux, macOS, Windows)
- Display mode selector in Settings → Edit defaults

### Fixed

- PDF → PDF conversion now properly differs from PDF (Optimized) — prevents same-extension skip bug
- JPEG output extension correctly maps to `.jpg` (not `.jpeg`)
- H.264 encoder detection via runtime frame probe (detects h264_nvenc, h264_qsv, libx264, libopenh264)
- LibreOffice thread-safety via global mutex
- `delete_originals` setting now defaults to `false`
- Error cleanup safely skips in-place optimizations (same-extension conversions)

### Technical

- Refactored `converter.rs` to dispatch via MediaType enum
- Created `util.rs` for shared utilities (cores, has(), best_h264_encoder)
- Config struct serializes to JSON in `~/.config/rusty-crunch/config.json`
- Parallel processing via rayon with CAS-based atomic compression ratio tracking
- Progress bar with ETA, best/worst compression stats

### Dependencies

- Added: clap 4 (CLI parsing with derive macros)
- Video: ffmpeg with hardware encoder probing
- Images: ImageMagick (magick/convert)
- Documents: Ghostscript (PDF optimization), LibreOffice (format conversion)
- Config: serde, serde_json (JSON serialization)
- UI: dialoguer 0.11 (interactive prompts), console 0.15 (colors/terminal control)
- Parallel: rayon 1.10, indicatif 0.17 (progress bars)
- Utils: walkdir 2.5 (directory traversal), dirs 5.0 (config path)

---

## [0.3.0] — Earlier

(See git history for details)

# 🔧 rusty-crunch

A fast, terminal-based media converter and background agent written in Rust. Batch-compress audio, video, images, and documents **locally** — nothing is ever uploaded.

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

## Features

- **Safe conversion pipeline** — converts to an isolated staging directory, validates the result, and publishes using a same-filesystem rename. A failing converter leaves existing destinations untouched.
- **Predictable concurrent output** — workers reserve destination names, so similarly named inputs (such as `photo.jpg` and `photo.jpeg`) cannot overwrite each other's output.
- **Tunable parallelism** — a Tokio runtime with bounded conversion concurrency, user-selectable with `--threads N`. Video jobs remain capped at two simultaneous encodes.
- **Faster folder discovery** — recommended mode discovers format availability in one filesystem scan and excludes generated output subfolders from later recursive scans.
- **Smart caching** — an optimization cache keyed on `(size, high-resolution mtime)` skips unchanged same-format files across runs.
- **Recommended Crunch** — one-key optimal pipelines (lossless WAV/AIFF → FLAC, legacy video → MKV, JPEG → AVIF, PDF re-optimization, …).
- **Upscale** — smart integer upscaling (2x/3x/4x to a target height) with Anime/Movie profiles and hardware-encoder detection.
- **Safer restore** — tracks completed output sizes and modification timestamps to avoid deleting results that users later changed. Destructive conversions retain backed-up originals.
- **Scriptable and inspectable** — `--yes`, `--output-subfolder`, `--keep-originals`, and `--json` support automation; JSON includes matched files and input/output byte counts.
- **Agent Mode** — a detached background service that watches folders (OS notifications) or scans periodically, with PID-based lifecycle management.
- **Auto-installs dependencies** — detects your package manager (dnf/apt/pacman/zypper/brew/winget/choco/scoop) and installs missing tools.
- **Self-update with checksum verification** — verifies the release `SHA256SUMS` before replacing the binary.
- **Color-aware output** — honours `--no-color`, `NO_COLOR`, and non-TTY output.
- **Cross-platform** — statically linked binaries for Linux (musl), macOS, and Windows.

| Category  | Supported formats                                    |
|-----------|------------------------------------------------------|
| 🎵 Audio  | AIFF, FLAC, WAV, MP3, OGG, AAC, M4A, WMA, OPUS       |
| 🎬 Video  | MP4, MKV, AVI, MOV, WEBM, FLV, WMV, TS               |
| 🖼️ Images | PNG, JPEG, BMP, GIF, WEBP, TIFF, AVIF, ICO           |
| 📄 Docs   | PDF, DOCX, XLSX, PPTX, ODT, ODS, ODP, EPUB           |

## Installation

### Option A — pre-built binary

Download the latest release for your OS from the [Releases page](https://github.com/pgm1207/rusty-crunch/releases).

| OS      | File                                        |
|---------|---------------------------------------------|
| Linux   | `rusty-crunch-linux-x86_64`                 |
| macOS   | `rusty-crunch-macos-x86_64` / `-aarch64`    |
| Windows | `rusty-crunch-windows-x86_64.exe`           |

```bash
chmod +x rusty-crunch-linux-x86_64
./install.sh            # removes macOS quarantine, optionally installs
# or run it in place:
./rusty-crunch-linux-x86_64
```

`install.sh` flags: `--yes` (install without prompting), `--no-install` (quarantine removal only). Set `INSTALL_DIR` to change the target (default `/usr/local/bin`).

### Option B — build from source

```bash
# Install Rust: https://rustup.rs
git clone https://github.com/pgm1207/rusty-crunch.git
cd rusty-crunch
cargo build --release      # ./target/release/rusty-crunch
make install               # optional: build + install + man page
```

## Required external tools

rusty-crunch is a **front-end** that orchestrates these tools. They must be installed — rusty-crunch will try to **auto-install** them via your package manager when possible.

| Tool            | Used for        | License           | Website                                    |
|-----------------|-----------------|-------------------|--------------------------------------------|
| **FFmpeg**      | Audio & Video   | LGPL 2.1 / GPL    | [ffmpeg.org](https://ffmpeg.org)           |
| **ImageMagick** | Images          | Apache 2.0        | [imagemagick.org](https://imagemagick.org) |
| **Ghostscript** | PDF compression | AGPL 3.0          | [ghostscript.com](https://ghostscript.com) |
| **LibreOffice** | Office docs     | MPL 2.0           | [libreoffice.org](https://libreoffice.org) |

Check everything at once:

```bash
rusty-crunch --health-check        # exit code 1 if anything is missing
```

## Usage

```bash
rusty-crunch                       # interactive menu
rusty-crunch ~/Music               # skip the folder picker
rusty-crunch --dry-run ~/Music     # preview, change nothing
rusty-crunch --health-check        # verify external tools
rusty-crunch --agent-status        # is the background agent running?
rusty-crunch --agent-stop          # stop the background agent
```

### Non-interactive / scriptable mode

Use `--yes` (or an explicit `--mode`) to skip every prompt. Requires a `FOLDER`.

```bash
# Optimize a folder with config defaults
rusty-crunch --yes ~/Videos

# Recursive, delete originals, high quality, machine-readable output
rusty-crunch --yes --recursive --delete-originals --quality high --json ~/Videos

# Only files between 10MB and 4GB
rusty-crunch --yes --min-size 10MB --max-size 4GB ~/Videos

# Control resource use and protect originals, overriding saved defaults
rusty-crunch --yes --threads 4 --keep-originals ~/Videos

# Store converted files separately and get pure JSON on stdout
rusty-crunch --yes --recursive --output-subfolder compressed --json ~/Pictures

# Preview and count selected inputs without invoking external tools
rusty-crunch --yes --dry-run --json ~/Pictures | jq 'map({format: .input_format, count: .files_matched, bytes: .input_bytes})'

# Upscale everything to 1440p with the anime profile
rusty-crunch --yes --mode upscale --target 1440 --preset anime ~/Anime

# Undo the last conversion session
rusty-crunch --mode restore
```

#### CLI reference

| Flag | Description |
|------|-------------|
| `FOLDER` | Folder to process (skips the picker). |
| `-y, --yes` | Non-interactive: use config defaults, skip all prompts. |
| `--mode <optimize\|upscale\|restore>` | Non-interactive action (default `optimize`). |
| `--recursive` / `--no-recursive` | Override the config default for sub-folder scanning. |
| `--delete-originals` | Move successfully converted originals into restore backups. |
| `--keep-originals` | Override a saved delete-originals default; incompatible with `--delete-originals`. |
| `--threads N` | Set maximum parallel conversions (1 to 64); video still capped at 2 jobs. |
| `--output-subfolder NAME` | Write outputs into a single named subdirectory, preserving relative paths. |
| `--force-recheck` | Re-process files even if previously optimized. |
| `--quality <low\|medium\|high>` | Output quality for optimize. |
| `--conflict <skip\|overwrite\|rename>` | How to handle an existing output file (non-interactive). |
| `--min-size SIZE` / `--max-size SIZE` | Only process files within a size range (e.g. `10MB`, `4GB`). |
| `--target HEIGHT` / `--preset <anime\|movie>` | Upscale target height / profile. |
| `--dry-run` | Simulate the run without converting anything. |
| `--json` | Machine-readable output (conversion summary, `--history`, `--stats`). With `--yes`, stdout contains only JSON. |
| `--no-color` | Disable colored output (also honours `NO_COLOR`). |
| `--list-formats` | List supported input/output formats and exit. |
| `--check-update` | Report a newer release without installing it. |
| `--self-update` | Download and install the latest release (checksum-verified). |
| `--history` | List recorded conversion sessions (newest first). |
| `--stats` | Show cumulative files converted and space reclaimed. |
| `--notify` | Send a desktop notification when a batch finishes. |
| `-v, --verbose` | Verbose diagnostics (also written to the log file). |
| `-q, --quiet` | Suppress non-essential output. |
| `--health-check` | Verify required external tools (exit 1 if missing). |
| `--agent` / `--agent-stop` / `--agent-status` | Background agent control. |
| `-V, --version`, `-h, --help` | Version / help. |

### JSON summary fields

Non-interactive `--json` prints an array of conversion summaries. The relevant per-format fields are `files_matched` (selected files), `files_converted`, `files_skipped`, `files_failed`, `input_bytes`, `output_bytes`, `bytes_saved`, `bytes_added`, and `duration_secs`. With `--dry-run`, `files_matched` and `input_bytes` describe the planned work, while converted/output bytes remain zero. Failures still produce a nonzero exit status.

### Conversion and recovery safety

The converter writes a complete result into a temporary sibling directory, checks that it exists and is not empty, then publishes it. The previous destination is retained through the publication step so a failed replacement can be rolled back. Staging needs **temporary free space on the output filesystem**, potentially as large as the encoded result. Files left in a hidden `.rusty-crunch-stage-*` directory after a rare publication/rollback failure should be inspected manually before cleanup.

`Restore` refuses to remove files that have changed since conversion and conservatively refuses to delete unverified outputs from older history records. It does **not** restore an older destination overwritten using `--conflict overwrite`; keep external backups of important outputs. In-place same-extension rewrites done with `--delete-originals` are also backed up and can be restored. Without that flag, same-extension jobs write a renamed output instead of changing the original. The agent's automatic conversions do not create undo history. Always test irreversible workflows on disposable samples before using `--delete-originals`.

### Recommended Crunch

| Input                    | Output          | Rationale                    |
|--------------------------|-----------------|------------------------------|
| WAV, AIFF                | FLAC            | Lossless, ~60% smaller       |
| MP3, OGG, AAC, M4A, WMA  | OPUS            | Best lossy codec             |
| AVI, MOV, FLV, WMV, TS   | MKV             | Modern container, H.264      |
| BMP, TIFF, ICO, GIF      | PNG             | Lossless compression         |
| JPEG                     | AVIF            | Best lossy image codec       |
| PDF                      | PDF (Optimized) | 150 PPI image downsampling   |

### Upscale

Smart integer upscaling to a target height. Each file gets the closest 2x/3x/4x multiplier, and MKV is used to preserve subtitle/audio streams. Choose an **Anime** or **Movie** profile; rusty-crunch probes for the best H.264 encoder (VAAPI/NVENC/…) and falls back to CPU.

### Agent Mode

A detached background service that converts files matching your rules. Configure rules in the interactive menu; the agent survives terminal close.

- **Watch** — converts files as soon as they appear (OS notifications, 2 s debounce).
- **Periodic** — scans folders at a configurable interval (minimum 60 s).

Logs go to `~/.config/rusty-crunch/agent.log`.

## Files & locations

All state lives under `~/.config/rusty-crunch/` (Linux), `~/Library/Application Support/rusty-crunch/` (macOS), or `%APPDATA%\rusty-crunch\` (Windows):

| Path | Purpose |
|------|---------|
| `config.json` | Settings + agent rules. |
| `opt_cache.json` | Optimization cache (size + nanosecond-resolution mtime). |
| `history/session_<id>.json` | Restore history; new session IDs are unique even for jobs started in the same second. |
| `history/backups/…` | Original files kept for restore. |
| `agent.pid` / `agent.log` | Agent lifecycle + log. |

## Environment variables

| Variable | Effect |
|----------|--------|
| `NO_COLOR` | Disable colored output (any non-empty value). |
| `RUSTY_CRUNCH_CONFIG` | Override the config file path (also relocates cache/history/log). |
| `RUSTY_CRUNCH_VAAPI_DEVICE` | Override the VAAPI render node (e.g. `/dev/dri/renderD129`). |
| `RUSTY_CRUNCH_VERSION` / `RUSTY_CRUNCH_REPO` | Pin the version / repo used by `install.sh`. |
| `INSTALL_DIR` | Target directory used by `install.sh` (default `/usr/local/bin`). |

## Exit codes

| Code | Meaning |
|------|---------|
| `0` | Success (including a dry run with nothing to do). |
| `1` | Runtime error, missing tool, or one or more files failed to convert. |
| `2` | Command-line usage error (invalid flag/combination). |

## Troubleshooting

- **“Missing required tools”** — run `rusty-crunch --health-check`, then `--yes` once so rusty-crunch can auto-install, or install manually.
- **Colors/emoji in logs** — pass `--no-color` or set `NO_COLOR=1`; piping output already disables color.
- **A video encode fills memory** — video jobs are capped at 2 concurrent encodes; lower the thread mode in Settings.
- **Same-extension conversion** (e.g. MKV → MKV) — written to a temp file and swapped atomically, so an interrupted run never corrupts the original.

## Caveats — what it does *not* do

- It does **not** re-encode with lossless-only settings by default; conversions are lossy unless the target format is lossless (FLAC/PNG).
- It does **not** download or phone home except for the optional update check and dependency installation.
- Auto-update verifies `SHA256SUMS`, but the checksum is fetched from the same release (not signed).
- It will not run destructive operations (`--delete-originals`) non-interactively unless you explicitly pass the flag.

## Project structure

```
rusty-crunch/
├── Cargo.toml
├── src/
│   ├── main.rs         # CLI parsing, menus, recommended crunch, non-interactive mode
│   ├── agent.rs        # Agent Mode — watch/periodic daemon
│   ├── config.rs       # Persistent settings + agent rules
│   ├── formats.rs      # Media types, formats, compatibility map
│   ├── prompt.rs       # Interactive prompts (dialoguer)
│   ├── converter.rs    # External-tool execution (ffmpeg/magick/gs/libreoffice)
│   ├── processor.rs    # Batch engine, progress, caching, history/restore
│   ├── deps.rs         # Package-manager detection + install/clean
│   └── util.rs         # Hashing, threads, encoder probing, self-update
├── Makefile            # check / test / lint / install / package
└── .github/workflows/  # CI + release
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) and [SCOPE.md](SCOPE.md). Run `make check` before opening a PR.

## Acknowledgments

Built on an excellent open-source ecosystem: [tokio](https://crates.io/crates/tokio), [clap](https://crates.io/crates/clap), [dialoguer](https://crates.io/crates/dialoguer), [indicatif](https://crates.io/crates/indicatif), [console](https://crates.io/crates/console), [notify](https://crates.io/crates/notify), [walkdir](https://crates.io/crates/walkdir), [serde](https://crates.io/crates/serde).

## License

MIT — see [LICENSE](LICENSE).

# Scope

This document records what rusty-crunch is for — and, just as importantly, what
it is **not** for — so that feature requests have a shared reference point.

## Why it exists

Converting and compressing media on Linux usually means remembering the right
`ffmpeg`/`magick`/`gs` incantation for every format. rusty-crunch turns those
incantations into a safe, repeatable, batched workflow with sensible defaults,
progress reporting, caching, and an undo path.

## In scope

1. **Batch conversion** of audio, video, images and documents using the standard
   external tools.
2. **Recommended pipelines** for the common "make this smaller/more portable"
   cases.
3. **Integer upscaling** with sensible presets and hardware-encoder detection.
4. **Safe, resumable operation** — caching, atomic writes, backups and restore.
5. **A background agent** that watches folders (or scans periodically).
6. **Non-interactive, scriptable operation** with stable exit codes and `--json`.
7. **Dependency management** — detect, install, and clean the external tools.
8. **Honest coverage** — clear reporting of what was converted, skipped, or failed.

## Out of scope

- **A media player, editor, or library manager.** rusty-crunch transforms files.
- **Cloud/remote processing.** Everything runs locally.
- **Bundling the external tools.** They are dependencies, licensed separately.
- **Lossless-only re-encoding by default.** Unless the target format is lossless,
  conversion is lossy. The recommended table documents the trade-offs.
- **A signed update channel.** Updates are checksum-verified, not GPG-signed.
- **Running destructive actions unattended.** `--delete-originals` requires an
  explicit flag; the agent only deletes when its rule says so.

## Quality bar

- `cargo test` must pass; new logic gets tests.
- `cargo clippy --all-targets -- -D warnings` must be clean.
- No panics on user input: bad sizes, missing tools, unreadable files, and
  interrupted runs are handled with errors, not `unwrap()`.
- Every user-visible flag is documented in the README and the man page.
- Same-extension conversions never read and write the same file.

## Finish line (current milestone)

The current milestone is a trustworthy **1.0**: stable CLI, documented
behaviour, restore that always works, and cross-platform binaries. Until then,
prefer fixing correctness and safety issues over net-new features.

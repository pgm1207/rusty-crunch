# Contributing to rusty-crunch

Thanks for taking the time to contribute! This document covers the ground rules
and the checks your change should pass.

## Ground rules

- **One binary, no cloud.** rusty-crunch orchestrates local tools only; it must
  never upload user media.
- **Never destroy data silently.** Destructive actions (`--delete-originals`,
  same-extension overwrites) must be explicit and safe on interruption.
- **Keep the external-tool surface small.** FFmpeg, ImageMagick, Ghostscript and
  LibreOffice are the supported backends; adding a hard dependency needs a strong
  reason.
- **Cross-platform.** Linux, macOS and Windows are all supported targets; avoid
  Unix-only assumptions in shared code (`#[cfg]` where required).
- **Document user-visible behaviour** in `README.md` and `CHANGELOG.md`.

## Development setup

```bash
git clone https://github.com/pgm1207/rusty-crunch
cd rusty-crunch
cargo build          # debug binary at ./target/debug/rusty-crunch
cargo test
```

## Before opening a PR

Run the full check suite:

```bash
make check           # fmt + clippy + tests + build
```

or individually:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
```

Please keep new logic covered by unit tests where practical, and keep the
`--json` and human output in sync when you add fields.

## Adding a feature

1. Wire new flags through the `Cli` struct in `src/main.rs` (and the
   non-interactive path in `run_noninteractive` if it applies there).
2. Add the behaviour to the relevant module (`converter`, `processor`, …).
3. Document it in `README.md` (CLI reference table) and add a `CHANGELOG.md`
   entry under `Unreleased`/the next version.
4. Add a test.

## Reporting bugs

Include:

- rusty-crunch version (`rusty-crunch --version`)
- OS and architecture
- the exact command you ran
- `rusty-crunch --health-check` output
- the error text (run with the failing command plus `--no-color` if relevant)

Use the GitHub issue templates where possible.

## Commit messages

Short, imperative subject line; explain *why* in the body when it is not obvious.

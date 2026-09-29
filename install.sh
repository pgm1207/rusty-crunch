#!/usr/bin/env bash
#
# rusty-crunch installer
#
# Installs the rusty-crunch binary, preferring a local build if present,
# otherwise downloading a checksum-verified release.
#
# Usage:
#   ./install.sh                 # interactive (prompts before installing)
#   ./install.sh --yes           # install without prompting
#   ./install.sh --no-install    # prepare/verify only, don't install
#   ./install.sh --version 0.6.0 # install a specific release
#
# Environment:
#   INSTALL_DIR            target directory (default: /usr/local/bin)
#   RUSTY_CRUNCH_VERSION   release version to fetch (default: latest)
#   RUSTY_CRUNCH_REPO      owner/repo (default: pgm1207/rusty-crunch)

set -eu

REPO="${RUSTY_CRUNCH_REPO:-pgm1207/rusty-crunch}"
INSTALL_DIR="${INSTALL_DIR:-/usr/local/bin}"
VERSION="${RUSTY_CRUNCH_VERSION:-}"
AUTO_YES=false
NO_INSTALL=false
BINARY="rusty-crunch"

usage() {
    sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'
}

while [ $# -gt 0 ]; do
    case "$1" in
        --yes|-y)      AUTO_YES=true ;;
        --no-install)  NO_INSTALL=true ;;
        --version)     shift; VERSION="${1:-}" ;;
        --version=*)   VERSION="${1#--version=}" ;;
        -h|--help)     usage; exit 0 ;;
        *) echo "Unknown argument: $1" >&2; usage; exit 2 ;;
    esac
    shift
done

say()  { printf '%s\n' "$*"; }
die()  { printf 'error: %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

is_tty() { [ -t 0 ]; }

say "🔧 rusty-crunch installer"
say ""

# ── Detect platform ─────────────────────────────────────────────────────────
os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
    Linux)  plat="linux" ;;
    Darwin) plat="macos" ;;
    *) die "unsupported OS: $os (use the Windows/macOS release asset directly)" ;;
esac
case "$arch" in
    x86_64|amd64) march="x86_64" ;;
    aarch64|arm64) march="aarch64" ;;
    *) die "unsupported architecture: $arch" ;;
esac

if [ "$plat" = "linux" ] && [ "$march" != "x86_64" ]; then
    die "no prebuilt Linux binary for $march; build from source instead"
fi
if [ "$plat" = "macos" ]; then
    artifact="rusty-crunch-macos-$march"
else
    artifact="rusty-crunch-linux-x86_64"
fi
say "Platform: $plat/$march → $artifact"

# ── Locate a binary: local build first, else download ───────────────────────
src=""
for candidate in "./$BINARY" "./$artifact" "./target/release/$BINARY"; do
    if [ -f "$candidate" ]; then src="$candidate"; break; fi
done

tmpdir=""
cleanup() { [ -n "$tmpdir" ] && rm -rf "$tmpdir"; }
trap cleanup EXIT INT TERM

if [ -n "$src" ]; then
    say "Using local binary: $src"
else
    have curl || die "curl is required to download rusty-crunch"
    tmpdir="$(mktemp -d)"

    if [ -z "$VERSION" ]; then
        say "Resolving latest release…"
        api="https://api.github.com/repos/$REPO/releases/latest"
        VERSION="$(curl -sfL "$api" | sed -n 's/.*"tag_name": *"v\{0,1\}\([^"]*\)".*/\1/p' | head -n1)"
        [ -n "$VERSION" ] || die "could not determine the latest version (set RUSTY_CRUNCH_VERSION)"
    fi
    say "Version: $VERSION"

    base="https://github.com/$REPO/releases/download/v$VERSION"
    say "Downloading $artifact…"
    curl -fL --connect-timeout 30 --max-time 300 -o "$tmpdir/$artifact" "$base/$artifact" \
        || die "download failed: $base/$artifact"
    src="$tmpdir/$artifact"

    # ── Verify checksum ─────────────────────────────────────────────────────
    if curl -sfL --connect-timeout 10 --max-time 30 -o "$tmpdir/SHA256SUMS" "$base/SHA256SUMS"; then
        if have sha256sum; then
            want="$(grep -E "[[:space:]]\*?$artifact\$" "$tmpdir/SHA256SUMS" | awk '{print $1}' | head -n1)"
            got="$(sha256sum "$src" | awk '{print $1}')"
        elif have shasum; then
            want="$(grep -E "[[:space:]]\*?$artifact\$" "$tmpdir/SHA256SUMS" | awk '{print $1}' | head -n1)"
            got="$(shasum -a 256 "$src" | awk '{print $1}')"
        else
            want=""; got=""
            say "⚠  sha256sum/shasum not found — skipping checksum verification"
        fi
        if [ -n "$want" ]; then
            [ "$want" = "$got" ] || die "checksum mismatch for $artifact (expected $want, got $got)"
            say "✓ Checksum verified"
        fi
    else
        say "⚠  Release publishes no SHA256SUMS — skipping checksum verification"
    fi
fi

chmod +x "$src"
say "✓ Binary ready"

# ── macOS quarantine ────────────────────────────────────────────────────────
if [ "$plat" = "macos" ] && have xattr; then
    if xattr "$src" 2>/dev/null | grep -q "com.apple.quarantine"; then
        say "🔓 Removing macOS quarantine attribute…"
        xattr -d com.apple.quarantine "$src" 2>/dev/null || true
    fi
fi

if [ "$NO_INSTALL" = true ]; then
    say ""
    say "Prepared binary at: $src"
    say "Run it directly, or re-run without --no-install to install."
    exit 0
fi

# ── Confirm ─────────────────────────────────────────────────────────────────
if [ "$AUTO_YES" != true ] && is_tty; then
    printf 'Install to %s? [y/N] ' "$INSTALL_DIR"
    read -r reply
    case "$reply" in
        [Yy]*) : ;;
        *) say "Skipped installation."; exit 0 ;;
    esac
fi

# ── Install ─────────────────────────────────────────────────────────────────
if [ ! -d "$INSTALL_DIR" ]; then
    mkdir -p "$INSTALL_DIR" 2>/dev/null || sudo mkdir -p "$INSTALL_DIR"
fi

if [ -w "$INSTALL_DIR" ]; then
    install -m 0755 "$src" "$INSTALL_DIR/$BINARY"
else
    say "ℹ  $INSTALL_DIR is not writable; using sudo…"
    sudo install -m 0755 "$src" "$INSTALL_DIR/$BINARY"
fi
say "✓ Installed to $INSTALL_DIR/$BINARY"

# ── PATH configuration ──────────────────────────────────────────────────────
case ":$PATH:" in
    *":$INSTALL_DIR:"*)
        say "✓ $INSTALL_DIR is already on your PATH"
        ;;
    *)
        say ""
        say "⚠  $INSTALL_DIR is not on your PATH."
        line="export PATH=\"$INSTALL_DIR:\$PATH\""
        rc=""
        shell_name="$(basename "${SHELL:-sh}")"
        case "$shell_name" in
            fish) rc="$HOME/.config/fish/config.fish"; line="set -gx PATH $INSTALL_DIR \$PATH" ;;
            zsh)  rc="$HOME/.zshrc" ;;
            bash) rc="$HOME/.bashrc" ;;
            *)    rc="$HOME/.profile" ;;
        esac
        if [ -n "$rc" ] && ! grep -qsF "$INSTALL_DIR" "$rc"; then
            if [ "$AUTO_YES" = true ] || { is_tty && { printf 'Add it to %s? [y/N] ' "$rc"; read -r a; case "$a" in [Yy]*) : ;; *) false ;; esac; }; }; then
                mkdir -p "$(dirname "$rc")"
                printf '\n# Added by rusty-crunch installer\n%s\n' "$line" >> "$rc"
                say "✓ Appended to $rc (restart your shell or run: source $rc)"
            else
                say "Add this to $rc manually:"
                say "    $line"
            fi
        else
            say "Add this to your shell profile:"
            say "    $line"
        fi
        ;;
esac

say ""
say "Done. Run: $BINARY --help"

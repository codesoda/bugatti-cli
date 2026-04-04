#!/bin/sh
# install.sh — Bugatti CLI installer.
#
# When run via curl|sh, downloads the pre-built binary from GitHub Releases.
# When run from a repo checkout, builds from source (requires Rust/Cargo).
#
# Usage:
#   curl -sSf https://raw.githubusercontent.com/codesoda/bugatti-cli/main/install.sh | sh
#   ./install.sh [options]       # from a repo checkout (builds from source)
#
# Options:
#   --skip-symlink      Skip creating ~/.local/bin symlink
#   --help, -h          Show this help message
#
# Environment overrides:
#   BUGATTI_HOME       — Override ~/.bugatti install root
#   BUGATTI_LOCAL_BIN  — Override ~/.local/bin symlink directory

set -eu

# --- Configuration (overridable for forks) ---

REPO_OWNER="${BUGATTI_REPO_OWNER:-codesoda}"
REPO_NAME="${BUGATTI_REPO_NAME:-bugatti-cli}"
REPO_REF="${BUGATTI_REPO_REF:-main}"

# --- Color support ---

if [ -t 1 ] && command -v tput >/dev/null 2>&1 && [ "$(tput colors 2>/dev/null || echo 0)" -ge 8 ]; then
    USE_COLOR=1
else
    USE_COLOR=0
fi

if [ "$USE_COLOR" = 1 ]; then
    C_RESET='\033[0m'
    C_BOLD='\033[1m'
    C_DIM='\033[38;5;249m'
    C_OK='\033[38;5;114m'
    C_WARN='\033[38;5;216m'
    C_ERR='\033[38;5;210m'
    C_HEADER='\033[38;5;141m'
    C_CHECK='\033[38;5;151m'
else
    C_RESET=''
    C_BOLD=''
    C_DIM=''
    C_OK=''
    C_WARN=''
    C_ERR=''
    C_HEADER=''
    C_CHECK=''
fi

# --- Output helpers ---

header() {
    printf '\n%b%b%s%b\n' "$C_BOLD" "$C_HEADER" "$*" "$C_RESET"
    printf '%b%s%b\n' "$C_DIM" "$(echo "$*" | sed 's/./-/g')" "$C_RESET"
}

info() {
    printf '%b%s%b\n' "$C_OK" "$*" "$C_RESET"
}

dim() {
    printf '%b%s%b\n' "$C_DIM" "$*" "$C_RESET"
}

ok() {
    printf '%b✓ %s%b\n' "$C_CHECK" "$*" "$C_RESET"
}

ok_detail() {
    printf '%b✓ %s %b(%s)%b\n' "$C_CHECK" "$1" "$C_DIM" "$2" "$C_RESET"
}

warn() {
    printf '%b! %s%b\n' "$C_WARN" "$*" "$C_RESET" >&2
}

die() {
    printf '%b✗ %s%b\n' "$C_ERR" "$*" "$C_RESET" >&2
    exit 1
}

# --- Usage ---

usage() {
    cat <<'USAGE'
Bugatti CLI Installer

Usage:
  curl -sSf https://raw.githubusercontent.com/codesoda/bugatti-cli/main/install.sh | sh
  ./install.sh [options]

Options:
  --skip-symlink      Skip creating ~/.local/bin symlink
  --help, -h          Show this help message

Environment overrides:
  BUGATTI_HOME       — Override ~/.bugatti install root
  BUGATTI_LOCAL_BIN  — Override ~/.local/bin symlink directory
USAGE
}

# --- Argument parsing ---

SKIP_SYMLINK=0

parse_args() {
    while [ $# -gt 0 ]; do
        case "$1" in
            --skip-symlink)
                SKIP_SYMLINK=1
                ;;
            --help|-h)
                usage
                exit 0
                ;;
            *)
                die "Unknown option: $1 (use --help)"
                ;;
        esac
        shift
    done
}

# --- Cleanup trap ---

TMP_DIR=""

cleanup() {
    if [ -n "$TMP_DIR" ] && [ -d "$TMP_DIR" ]; then
        rm -rf "$TMP_DIR"
    fi
}

trap cleanup EXIT INT TERM

# --- Global result variables ---

INSTALLED_BINARY=""
SOURCE_ROOT=""

# --- Detect architecture ---

detect_target() {
    os="$(uname -s)"
    arch="$(uname -m)"

    case "$os" in
        Darwin) ;;
        *) die "Pre-built binaries are only available for macOS (got $os). Build from a clone instead." ;;
    esac

    case "$arch" in
        arm64|aarch64) echo "aarch64-apple-darwin" ;;
        *) die "Pre-built binaries are only available for arm64 (got $arch). Build from a clone instead." ;;
    esac
}

# --- Install from GitHub release ---

install_from_release() {
    if ! command -v curl >/dev/null 2>&1; then
        die "curl is required for remote install"
    fi

    target="$(detect_target)"

    header "Fetching latest release"

    # Get the latest release tag via redirect (avoids GitHub API rate limits)
    latest_url="https://github.com/$REPO_OWNER/$REPO_NAME/releases/latest"
    tag="$(curl -sSf -o /dev/null -w '%{redirect_url}' "$latest_url" | grep -oE '[^/]+$')"
    if [ -z "$tag" ]; then
        die "Could not determine latest release — check https://github.com/$REPO_OWNER/$REPO_NAME/releases"
    fi
    ok_detail "Release" "$tag"

    asset_name="bugatti-${tag}-${target}.tar.gz"
    asset_url="https://github.com/$REPO_OWNER/$REPO_NAME/releases/download/${tag}/${asset_name}"

    TMP_DIR="$(mktemp -d)"
    info "Downloading $asset_name..."
    if ! curl -sSfL "$asset_url" -o "$TMP_DIR/$asset_name"; then
        die "Failed to download $asset_url"
    fi
    ok "Downloaded"

    tar xzf "$TMP_DIR/$asset_name" -C "$TMP_DIR"
    downloaded_binary="$TMP_DIR/bugatti-${tag}-${target}/bugatti"
    if [ ! -f "$downloaded_binary" ]; then
        die "Archive does not contain expected binary"
    fi

    install_binary "$downloaded_binary"
}

# --- Build from local source ---

build_from_source() {
    ok_detail "Source tree" "$SOURCE_ROOT"

    header "Checking prerequisites"
    if ! command -v cargo >/dev/null 2>&1; then
        die "cargo is required (install Rust: https://rustup.rs)"
    fi
    ok "cargo found"

    header "Building bugatti"
    if ! (cd "$SOURCE_ROOT" && cargo build --release); then
        die "cargo build failed"
    fi

    built_binary="$SOURCE_ROOT/target/release/bugatti"
    if [ ! -f "$built_binary" ]; then
        die "Build succeeded but binary not found at $built_binary"
    fi

    ok_detail "Built" "$built_binary"
    install_binary "$built_binary"
}

# --- Install binary to BUGATTI_HOME ---

install_binary() {
    src_binary="$1"
    bugatti_home="${BUGATTI_HOME:-$HOME/.bugatti}"
    bin_dir="$bugatti_home/bin"
    mkdir -p "$bin_dir"

    target_path="$bin_dir/bugatti"

    # Remove existing before install
    if [ -e "$target_path" ] || [ -L "$target_path" ]; then
        rm "$target_path"
    fi

    cp "$src_binary" "$target_path"
    chmod +x "$target_path"

    # macOS: remove quarantine and apply ad-hoc code signature
    secure_binary "$target_path"

    ok_detail "Installed" "$target_path"

    INSTALLED_BINARY="$target_path"
}

# --- macOS binary security ---

secure_binary() {
    case "$(uname -s)" in
        Darwin)
            if command -v xattr >/dev/null 2>&1; then
                xattr -dr com.apple.quarantine "$1" 2>/dev/null || true
                xattr -dr com.apple.provenance "$1" 2>/dev/null || true
            fi
            if command -v codesign >/dev/null 2>&1; then
                codesign --force --sign - "$1" 2>/dev/null || true
            fi
            ;;
    esac
}

# --- Symlink to ~/.local/bin ---

ensure_local_bin_symlink() {
    local_bin="${BUGATTI_LOCAL_BIN:-$HOME/.local/bin}"
    symlink_path="$local_bin/bugatti"

    if [ -e "$local_bin" ] && [ ! -d "$local_bin" ]; then
        warn "$local_bin exists but is not a directory — skipping symlink"
        return 1
    fi

    mkdir -p "$local_bin"

    if [ -L "$symlink_path" ]; then
        rm "$symlink_path"
    elif [ -e "$symlink_path" ]; then
        warn "$symlink_path exists and is not a symlink — skipping (remove it manually to fix)"
        return 1
    fi

    ln -s "$INSTALLED_BINARY" "$symlink_path"
    ok_detail "Symlinked" "$symlink_path -> $INSTALLED_BINARY"

    case ":${PATH}:" in
        *":${local_bin}:"*)
            ;;
        *)
            warn "$local_bin is not on your PATH — add it to your shell profile:"
            dim "  export PATH=\"$local_bin:\$PATH\""
            ;;
    esac

    return 0
}

# --- Summary ---

print_summary() {
    header "Summary"

    ok_detail "Binary" "$INSTALLED_BINARY"

    printf '\n'
    dim "  Get started:"
    dim "    bugatti test path/to/test.test.toml   # run a single test"
    dim "    bugatti test                           # discover and run all tests"
    dim ""
    dim "  See examples/  for working projects to try."
    printf '\n'
    printf '%b%b  Done!%b\n\n' "$C_BOLD" "$C_OK" "$C_RESET"
}

# --- Main ---

main() {
    parse_args "$@"

    printf '\n%b%bBugatti Installer%b\n' "$C_BOLD" "$C_HEADER" "$C_RESET"
    dim "━━━━━━━━━━━━━━━━━"
    printf '\n'

    # If running from a repo checkout, build locally; otherwise grab the release binary
    script_dir="$(cd "$(dirname "$0")" && pwd)"
    if [ -f "$script_dir/Cargo.toml" ] && [ -d "$script_dir/src" ]; then
        SOURCE_ROOT="$script_dir"
        build_from_source
    else
        install_from_release
    fi

    if [ "$SKIP_SYMLINK" = 0 ]; then
        ensure_local_bin_symlink || true
    fi

    print_summary
}

main "$@"

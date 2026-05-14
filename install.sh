#!/usr/bin/env sh
# gitoui installer
#
# Install or update gitoui — a terminal UI git log viewer.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/NayJi7/gitoui/master/install.sh | sh
#
# Environment overrides:
#   INSTALL_DIR      — destination directory (default: $HOME/.local/bin)
#   GITOUI_VERSION   — specific tag to install (default: latest)
#   NO_COLOR         — set to anything to disable ANSI colors

set -eu

# ── Constants ───────────────────────────────────────────────────────────
REPO="NayJi7/gitoui"
BIN_NAME="gitoui"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"
GITOUI_VERSION="${GITOUI_VERSION:-latest}"

# ── Colors (truecolor; auto-disabled when not a TTY or NO_COLOR set) ───
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
    ORANGE=$(printf '\033[38;2;240;81;51m')   # brand primary #F05133
    SHADOW=$(printf '\033[38;2;139;42;18m')   # brand shadow  #8B2A12
    CREAM=$(printf '\033[38;2;241;236;236m')  # brand cream   #F1ECEC
    GRAY=$(printf '\033[38;2;130;125;125m')
    GREEN=$(printf '\033[32m')
    YELLOW=$(printf '\033[33m')
    RED=$(printf '\033[31m')
    BOLD=$(printf '\033[1m')
    DIM=$(printf '\033[2m')
    RESET=$(printf '\033[0m')
else
    ORANGE='' SHADOW='' CREAM='' GRAY=''
    GREEN='' YELLOW='' RED=''
    BOLD='' DIM='' RESET=''
fi

# ── Logging helpers ────────────────────────────────────────────────────
info()  { printf '  %s❯%s  %s\n' "${BOLD}${ORANGE}" "${RESET}" "$1"; }
ok()    { printf '  %s✓%s  %s\n' "${BOLD}${GREEN}" "${RESET}" "$1"; }
warn()  { printf '  %s!%s  %s\n' "${BOLD}${YELLOW}" "${RESET}" "$1"; }
fail()  { printf '  %s✗%s  %s\n' "${BOLD}${RED}" "${RESET}" "$1" >&2; exit 1; }

# ── Brand banner — G logo block-art + colored wordmark ────────────────
# Column layout for the G (12 cells wide, 6 rows):
#   row 0  ████████████   (top bar)
#   row 1  ██             (left bar)
#   row 2  ██    ██████   (left + tongue — tongue ends at col 11)
#   row 3  ██        ██   (left + right vert at cols 10-11)
#   row 4  ██        ██   (same)
#   row 5  ████████████   (bottom bar)
# Wordmark: "git" in orange (#F05133), "oui" in cream (#F1ECEC).
print_banner() {
    G="${ORANGE}${BOLD}"
    R="${RESET}"
    GIT="${ORANGE}${BOLD}"
    OUI="${CREAM}${BOLD}"
    printf '\n'
    printf '   %s████████████%s        %sgit%s%soui%s\n'                "$G" "$R" "$GIT" "$R" "$OUI" "$R"
    printf '   %s██%s                  %sa terminal UI for the git log.%s\n' "$G" "$R" "$DIM" "$R"
    printf '   %s██%s    %s██████%s\n'                                  "$G" "$R" "$G" "$R"
    printf '   %s██%s        %s██%s    %srepo:%s github.com/%s%s%s\n'   "$G" "$R" "$G" "$R" "$DIM" "$R" "$BOLD" "$REPO" "$R"
    printf '   %s██%s        %s██%s\n'                                  "$G" "$R" "$G" "$R"
    printf '   %s████████████%s\n'                                      "$G" "$R"
    printf '\n'
}

# ── Tooling check ──────────────────────────────────────────────────────
need() {
    command -v "$1" >/dev/null 2>&1 || fail "Required tool not found: $1"
}

# ── OS / arch detection → cargo target triple ──────────────────────────
detect_target() {
    OS="$(uname -s)"
    ARCH="$(uname -m)"
    case "$OS" in
        Linux)
            case "$ARCH" in
                x86_64|amd64)   TARGET="x86_64-unknown-linux-gnu" ;;
                aarch64|arm64)  TARGET="aarch64-unknown-linux-gnu" ;;
                *) fail "Unsupported architecture: ${OS}/${ARCH}" ;;
            esac
            ;;
        Darwin)
            case "$ARCH" in
                x86_64|amd64)   TARGET="x86_64-apple-darwin" ;;
                arm64|aarch64)  TARGET="aarch64-apple-darwin" ;;
                *) fail "Unsupported architecture: ${OS}/${ARCH}" ;;
            esac
            ;;
        *) fail "Unsupported OS: ${OS} — gitoui ships for Linux and macOS." ;;
    esac
    info "Platform: ${BOLD}${OS} (${ARCH}) → ${TARGET}${RESET}"
}

# ── Resolve the version to install ─────────────────────────────────────
resolve_version() {
    if [ "$GITOUI_VERSION" = "latest" ]; then
        info "Resolving latest release…"
        RESOLVED_VERSION=$(
            curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
            | grep '"tag_name"' \
            | head -n1 \
            | sed -E 's/.*"tag_name"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/'
        ) || fail "Could not query GitHub for the latest release."
        [ -n "$RESOLVED_VERSION" ] || fail "No published releases found yet for ${REPO}."
    else
        case "$GITOUI_VERSION" in
            v*) RESOLVED_VERSION="$GITOUI_VERSION" ;;
            *)  RESOLVED_VERSION="v${GITOUI_VERSION}" ;;
        esac
    fi
    # Archive names use the bare version number (no leading "v").
    VERSION_NUMBER="${RESOLVED_VERSION#v}"
    info "Target version: ${BOLD}${RESOLVED_VERSION}${RESET}"
}

# ── Existing-install check — skips work when already up-to-date ────────
check_existing() {
    UPDATE=0
    if command -v "$BIN_NAME" >/dev/null 2>&1; then
        CURRENT_RAW="$("$BIN_NAME" --version 2>/dev/null || true)"
        # `--version` prints `gitoui X.Y.Z` (clap default). Grab the last
        # whitespace-separated token; tolerate other formats by falling
        # back to "unknown" so we still go through the update path.
        CURRENT_VERSION="$(printf '%s\n' "$CURRENT_RAW" | awk '{print $NF}')"
        [ -n "$CURRENT_VERSION" ] || CURRENT_VERSION="unknown"
        if [ "v${CURRENT_VERSION}" = "$RESOLVED_VERSION" ] \
           || [ "$CURRENT_VERSION" = "$RESOLVED_VERSION" ]; then
            ok "gitoui ${BOLD}${CURRENT_VERSION}${RESET} is already up to date."
            print_welcome
            exit 0
        fi
        info "Found gitoui ${BOLD}${CURRENT_VERSION}${RESET} → updating to ${BOLD}${RESOLVED_VERSION}${RESET}."
        UPDATE=1
    fi
}

# ── Download + extract — uses curl --progress-bar for a clean UI ──────
download_archive() {
    ARCHIVE="${BIN_NAME}-${VERSION_NUMBER}-${TARGET}.tar.gz"
    URL="https://github.com/${REPO}/releases/download/${RESOLVED_VERSION}/${ARCHIVE}"
    CHECKSUM_URL="https://github.com/${REPO}/releases/download/${RESOLVED_VERSION}/checksum.txt"
    TMP="$(mktemp -d 2>/dev/null || mktemp -d -t gitoui)"
    # shellcheck disable=SC2064
    trap "rm -rf '$TMP'" EXIT INT TERM
    info "Downloading ${BOLD}${ARCHIVE}${RESET}"
    printf '    %s' "${DIM}"
    if ! curl --fail --location --progress-bar -o "${TMP}/${ARCHIVE}" "$URL"; then
        printf '%s' "${RESET}"
        fail "Download failed: $URL"
    fi
    printf '%s' "${RESET}"

    # Verify checksum when checksum.txt is published — non-fatal if
    # the file is missing (older releases may not include it).
    if curl --fail --silent --location -o "${TMP}/checksum.txt" "$CHECKSUM_URL" 2>/dev/null; then
        EXPECTED="$(grep " ${ARCHIVE}\$" "${TMP}/checksum.txt" \
                    | awk '{print $1}' | head -n1 || true)"
        if [ -n "$EXPECTED" ]; then
            if command -v sha256sum >/dev/null 2>&1; then
                ACTUAL="$(sha256sum "${TMP}/${ARCHIVE}" | awk '{print $1}')"
            elif command -v shasum >/dev/null 2>&1; then
                ACTUAL="$(shasum -a 256 "${TMP}/${ARCHIVE}" | awk '{print $1}')"
            else
                ACTUAL=""
            fi
            if [ -n "$ACTUAL" ]; then
                if [ "$ACTUAL" = "$EXPECTED" ]; then
                    ok "Checksum verified."
                else
                    fail "Checksum mismatch — refusing to install."
                fi
            fi
        fi
    fi

    info "Extracting…"
    tar -xzf "${TMP}/${ARCHIVE}" -C "$TMP" \
        || fail "Extraction failed (corrupted archive?)."
    [ -f "${TMP}/${BIN_NAME}" ] || fail "Binary missing from archive."
    chmod +x "${TMP}/${BIN_NAME}"
}

# ── Install to $INSTALL_DIR ────────────────────────────────────────────
install_binary() {
    mkdir -p "$INSTALL_DIR" 2>/dev/null \
        || fail "Cannot create install directory: ${INSTALL_DIR}"
    DEST="${INSTALL_DIR}/${BIN_NAME}"
    # Atomic-ish replace: write to a sibling temp path then rename, so a
    # mid-install interruption doesn't leave a half-written binary.
    TMP_DEST="${DEST}.new"
    cp "${TMP}/${BIN_NAME}" "$TMP_DEST" \
        || fail "Cannot write to ${INSTALL_DIR} (permission?)"
    chmod 755 "$TMP_DEST"
    mv -f "$TMP_DEST" "$DEST"
    if [ "$UPDATE" -eq 1 ]; then
        ok "Updated ${BOLD}${BIN_NAME}${RESET} at ${BOLD}${DEST}${RESET}"
    else
        ok "Installed ${BOLD}${BIN_NAME}${RESET} to ${BOLD}${DEST}${RESET}"
    fi
}

# ── Warn if $INSTALL_DIR isn't on the user's PATH ──────────────────────
check_path() {
    case ":$PATH:" in
        *":${INSTALL_DIR}:"*) return 0 ;;
    esac
    warn "${BOLD}${INSTALL_DIR}${RESET} is not in your PATH."
    HINT_FILE=""
    case "${SHELL:-}" in
        */zsh)   HINT_FILE="$HOME/.zshrc" ;;
        */bash)  HINT_FILE="$HOME/.bashrc" ;;
        */fish)  HINT_FILE="$HOME/.config/fish/config.fish" ;;
    esac
    if [ -n "$HINT_FILE" ]; then
        printf '      %sAdd this line to %s%s%s:%s\n' \
            "$DIM" "$BOLD" "$HINT_FILE" "${RESET}${DIM}" "$RESET"
    else
        printf '      %sAdd this line to your shell rc:%s\n' "$DIM" "$RESET"
    fi
    printf '        %sexport PATH="%s:$PATH"%s\n\n' \
        "${ORANGE}${BOLD}" "$INSTALL_DIR" "$RESET"
}

# ── Welcome message — keep it short on purpose ─────────────────────────
print_welcome() {
    printf '\n'
    printf '  %sQuick start%s — inside any git repo:\n' "${BOLD}" "${RESET}"
    printf '    %sgitoui%s        %sopen the commit-graph viewer%s\n' \
        "${ORANGE}${BOLD}" "$RESET" "$DIM" "$RESET"
    printf '    %sgitoui --help%s %sall flags and options%s\n' \
        "${ORANGE}${BOLD}" "$RESET" "$DIM" "$RESET"
    printf '\n'
    printf '  %sIn-app:%s press %s?%s anywhere for the keymap, %sq%s to quit.\n' \
        "$DIM" "$RESET" "${BOLD}" "$RESET" "${BOLD}" "$RESET"
    printf '\n'
}

# ── Main ──────────────────────────────────────────────────────────────
main() {
    print_banner
    need uname
    need tar
    need curl
    detect_target
    resolve_version
    check_existing
    download_archive
    install_binary
    check_path
    print_welcome
}

main

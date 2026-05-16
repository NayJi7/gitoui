#!/usr/bin/env sh
# gitoui installer
#
# Install or update gitoui — a terminal UI git log viewer.
#
# Usage:
#   curl -fsSL https://nayji7.github.io/gitoui/install.sh | sh
#
# Pass --ascii via `sh -s -- --ascii` (or run locally) to force the ASCII
# banner — useful in terminals that drop image-protocol bytes or when
# capturing the output to plain text.
#
# Environment overrides:
#   INSTALL_DIR                  destination directory
#                                (default: $HOME/.local/bin)
#   GITOUI_VERSION               specific tag to install (default: latest)
#   NO_COLOR                     disable ANSI colors
#   GITOUI_ASCII                 set to 1 to force ASCII banner
#                                (same as --ascii)
#   GITOUI_NO_TRACK              skip the anonymous install ping (a single
#                                GET to a Cloudflare Worker that only
#                                increments a counter — no IP, no UA, no
#                                payload)
#   GITOUI_INSTALL_QUIET         legacy combined toggle (v0.1.6): skips
#                                BOTH the banner AND the welcome trailer.
#   GITOUI_INSTALL_NO_BANNER     skip the brand banner only. Set by
#                                `gitoui --update` so the splash isn't
#                                drawn twice.
#   GITOUI_INSTALL_NO_WELCOME    skip the Quick-start welcome only. Set
#                                by the startup-prompt path (after a
#                                successful auto-update gitoui re-execs
#                                straight into the TUI, no need for the
#                                shell trailer).
#   GITOUI_INSTALL_FORCE_BINARY  install to $INSTALL_DIR even if a cargo-
#                                managed gitoui exists in ~/.cargo/bin/.
#                                Set by `gitoui --update` when the running
#                                binary lives outside ~/.cargo/bin/ to
#                                avoid silently updating the wrong copy.

set -eu

# ── CLI arg parsing ─────────────────────────────────────────────────────
ASCII_BANNER="${GITOUI_ASCII:-0}"
for arg in "$@"; do
    case "$arg" in
        --ascii) ASCII_BANNER=1 ;;
    esac
done

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

# ── Brand banner ──────────────────────────────────────────────────────
# Reproduces `gitoui --help`'s splash exactly — see
# src/lib.rs::print_no_repo_splash. Two render paths:
#   1. Terminal supports Kitty graphics or iTerm2 inline image protocol
#      → emit two separate inline images (logo + wordmark) into the
#        same 5-row band, just like the binary's splash.
#   2. Otherwise → ASCII fallback (small G silhouette + figlet-style
#      "gitoui" wordmark).
#
# PNG payloads were produced from the brand SVGs at the exact pixel
# canvas the binary uses (LOGO: cell_w*32 × cell_h*64, WM: same):
#   magick -density 300 -background none assets/brand/logo-nobg.svg \
#     -resize '256x320^' -gravity center -extent 256x320 \
#     -depth 8 -strip /tmp/logo-splash.png
#   magick -density 300 -background none assets/brand/wordmark-nobg.svg \
#     -resize 672x320 -depth 8 -strip /tmp/wm-splash.png
# Then `base64 -w0` each one.

# Splash layout — must match the literals in print_no_repo_splash.
LEFT_MARGIN=2
LOGO_COLS=8
LOGO_ROWS=5
GAP_COLS=2
WM_COLS=21
WM_ROWS=5
WM_OFFSET=$((LEFT_MARGIN + LOGO_COLS + GAP_COLS))   # = 12
TOTAL_ROWS=$WM_ROWS                                 # = 5

LOGO_PNG_SIZE=1324
LOGO_PNG_B64="iVBORw0KGgoAAAANSUhEUgAAAQAAAAFACAMAAABk9FI4AAABp1BMVEUAAABQGxHDQSnvUTPqTzLrTzK8Pyj/f0f/Wjj/kFv///+5PieNKxKMKhKHKRL/e0bAQSnvUTPwUTPwUTPwUTPwUTPwUTPtUDJ3KBrsUDLmTTFQGxHoTjHcSi9KGRDaSi7bSi7iTDDBQSnCQSnGQyquOyXTRy0AAAAAAAAAAAAAAADfSy8AAAAAAAAAAAAAAAAAAADiTDAAAAD2UzXrTzLqTzLwUTPwUTPwUTPwUTPvUTPbSi/wUTPLRCvvUTPpTzLHQyrjTTAAAADKRCvwUTPORCiFKBGLKhKIKRGXLxe6PCHIQibuUDLCPySiMxrsUDLDPySKKhKjMxrCPySJKRFNGhDoTjHJQiaYLxauOB3tUDJYHhPoTjHrTzFaHhNZHhPoTjHvUTPtUDLtUDLtUDLlTTEvEAq7PyjAQSm+QCi/QCirOiSDLByELRyFLRyGLRyJLh2GLR0AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAADwUTPxUTPxUjO7PSK7PCLyUjSKKhKLKhKKKRKJKRGIKRGZLxeYLxblTTDmTTDoTjDaoJEJAAAAfXRSTlMAAAAAAAAAAAAAAAAAAAAABYXFwsG/wFEKtHAPtnQRdXbSm5yaSKAnJiISmg8KCwkFlwYBAymkrKuuY0P5R/W1SZYDRvXLiIWFqPr9/vv7/fr++/r+Ebf6/vv9Erv+EhK9+/T1+a4OapeWl35hYGFiYUIHFyMkIBwbGhECCANhHKsAAAK3SURBVHja7dzVclRBFEDRCwR3JzC4WwjBNRDcPbi7Q3DJDBAIEPhoHnnPmaqTW7P2B3SfXlX92F1MmDhpcqQpU8vctOnFjObuRm5mMauSPUNqs4s5jQ0wFwAAANkzAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAMHoFrq5kUBqvMXLCxxixaHAZYsXVbilq8IX4GVq1a3lLc1rWGAtW1FuYsDDMo+QjbA4OwjAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAADAiAIU1DU2vKBhi2bn1qw7MBRmzYuGlzXlu2ZgOM3LY9tn+w5myAUTt2pgK0pwPsygWoAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAPgPMLqxAXZ3jBnb/8aN37O3Vu1/tWo2QHXf/gORDh46/CXQ1yNHkwG+Hev5HutHb6Cfx09kA5w8FQSI6Z0GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABAfQDOnO1JrDf+ePpcZ2iB2vkLvxL7ffFSFODyldgKV/v+JNb3NzZ9d6W4FgToDvz/UI/yAcodAAAAAGTPAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAANSnamdx/Ubs8fLNW7ndDnXnbnHv/oOHgR49fvI0s2fPQ714WXS9ev0m0tt370vch4/Fp66WUJ/bWstcxz/o3zPLCMqVbQAAAABJRU5ErkJggg=="
WM_PNG_SIZE=8149
WM_PNG_B64="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"

detect_image_protocol() {
    # User asked for the ASCII path explicitly (--ascii flag or
    # GITOUI_ASCII=1 env var) — skip detection.
    if [ "$ASCII_BANNER" = "1" ]; then
        echo none
        return
    fi
    # Strongest signals first. Kitty's own terminal exports
    # KITTY_WINDOW_ID; Ghostty exports GHOSTTY_RESOURCES_DIR. Either
    # one is an unambiguous "Kitty graphics protocol works" signal.
    if [ -n "${KITTY_WINDOW_ID:-}" ] || [ -n "${GHOSTTY_RESOURCES_DIR:-}" ]; then
        echo kitty
        return
    fi
    # $TERM substring — typical values: xterm-kitty, xterm-ghostty.
    # Misses cases where the shell rc rewrites TERM to xterm-256color
    # (the GHOSTTY_RESOURCES_DIR check above covers that path).
    case "${TERM:-}" in
        *kitty*|*ghostty*)
            echo kitty
            return
            ;;
    esac
    # TERM_PROGRAM is set by the terminal app itself. Ghostty uses
    # lowercase; iTerm.app + WezTerm + Warp implement iTerm2's inline
    # image protocol.
    case "${TERM_PROGRAM:-}" in
        ghostty)
            echo kitty
            return
            ;;
        iTerm.app|WezTerm|WarpTerminal)
            echo iterm2
            return
            ;;
    esac
    # XDG advertisement (less common, but Ghostty + some others set it).
    case "${XDG_TERMINAL_EMULATOR:-}" in
        ghostty|kitty)
            echo kitty
            return
            ;;
    esac
    echo none
}

# Emit one Kitty graphics command for `bytes_b64`. Chunked at 4096
# chars to mirror kitty_encode_inner in src/protocol.rs — large PNGs
# (the wordmark) exceed the chunk limit and must be split.
emit_kitty_image() {
    # $1 = base64 string, $2 = c (cell width), $3 = r (cell height),
    # $4 = image id, $5 = 1 to prepend the d=C clear-at-cursor escape.
    _b64="$1"; _cw="$2"; _ch="$3"; _id="$4"
    if [ "$5" = "1" ]; then
        printf '\033_Ga=d,d=C;\033\\'
    fi
    _len=${#_b64}
    _pos=1
    _first=1
    while [ "$_pos" -le "$_len" ]; do
        _chunk=$(printf '%s' "$_b64" | cut -c"$_pos"-$((_pos + 4095)))
        _pos=$((_pos + 4096))
        if [ "$_pos" -le "$_len" ]; then _m=1; else _m=0; fi
        if [ "$_first" = "1" ]; then
            printf '\033_Ga=T,f=100,q=2,i=%s,c=%s,r=%s,m=%s;%s\033\\' \
                "$_id" "$_cw" "$_ch" "$_m" "$_chunk"
            _first=0
        else
            printf '\033_Gm=%s;%s\033\\' "$_m" "$_chunk"
        fi
    done
}

# Emit one iTerm2 inline image command for `bytes_b64`.
emit_iterm2_image() {
    # $1 = base64, $2 = png byte size, $3 = cell width, $4 = cell height
    printf '\033]1337;File=size=%s;width=%s;height=%s;preserveAspectRatio=0;inline=1:%s\007' \
        "$2" "$3" "$4" "$1"
}

# Reproduce src/lib.rs::print_no_repo_splash verbatim: reserve a band
# of TOTAL_ROWS rows, then paint two side-by-side images (logo +
# wordmark) into it via save/restore-cursor. The cursor lands on the
# last row of the band, so two trailing newlines = one blank gap.
print_banner_image() {
    proto="$1"
    # Reserve TOTAL_ROWS lines so the terminal scrolls if cursor is
    # near the bottom of the viewport.
    i=0
    while [ $i -lt "$TOTAL_ROWS" ]; do
        printf '\n'
        i=$((i + 1))
    done
    # Cursor back up to the top of the reserved band.
    printf '\033[%sA' "$TOTAL_ROWS"
    # Save cursor (DEC private), advance LEFT_MARGIN cols, emit logo.
    printf '\0337'
    printf '\033[%sC' "$LEFT_MARGIN"
    case "$proto" in
        kitty)
            emit_kitty_image "$LOGO_PNG_B64" "$LOGO_COLS" "$LOGO_ROWS" 1 1
            ;;
        iterm2)
            emit_iterm2_image "$LOGO_PNG_B64" "$LOGO_PNG_SIZE" \
                "$LOGO_COLS" "$LOGO_ROWS"
            ;;
    esac
    # Restore cursor to top-of-band, advance to wordmark column.
    printf '\0338'
    printf '\033[%sC' "$WM_OFFSET"
    case "$proto" in
        kitty)
            # No d=C prefix here — Ghostty interprets that as "delete
            # row" and would erase the logo (see protocol.rs comment).
            emit_kitty_image "$WM_PNG_B64" "$WM_COLS" "$WM_ROWS" 2 0
            ;;
        iterm2)
            emit_iterm2_image "$WM_PNG_B64" "$WM_PNG_SIZE" \
                "$WM_COLS" "$WM_ROWS"
            ;;
    esac
}

print_banner_ascii() {
    # ASCII fallback — small G silhouette faithful to the brand book's
    # 4×5 cell grid + figlet-style "gitoui" wordmark in block letters.
    # G is 5 rows × 8 cols (matching the splash logo's 8×5 cell box);
    # wordmark is figlet "Standard" (6 rows × 26 cols). The wordmark's
    # `g` descender extends one row below the G's bottom bar — same as
    # the brand SVG, so the bottoms line up visually.
    #
    # G grid maps src/brand.rs CELLS exactly (4 cells wide × 5 tall,
    # each cell rendered as 2 cols × 1 row in ASCII):
    #
    #   R1   ████████   top bar (all 4 cells)
    #   R2   ██         left bar (C1 only)
    #   R3   ██  ████   left + tongue + right vert (no C2)
    #   R4   ██▓▓▓▓██   left + shadow + shadow + right vert
    #   R5   ████████   bottom bar (all 4 cells)
    O="${ORANGE}${BOLD}"
    S="${SHADOW}${BOLD}"
    Wg="${ORANGE}${BOLD}"   # wordmark — "git" portion
    Wo="${CREAM}${BOLD}"    # wordmark — "oui" portion
    R="${RESET}"

    # Wordmark rows split at col 14 (between "t" and "o"). Six-row
    # figlet "Standard" rendering, placed so r0 (the `_ _` letter-tops)
    # sits one row ABOVE the G's top bar — fits the brand wordmark's
    # natural ascender area without stretching the G upward.
    g0='       _ _   '   ; o0='           _ '
    g1='  __ _(_) |_ '   ; o1='___  _   _(_)'
    g2=' / _` | | __/'   ; o2=' _ \| | | | |'
    g3='| (_| | | || '   ; o3='(_) | |_| | |'
    g4=' \__, |_|\__\'   ; o4='___/ \__,_|_|'
    g5=' |___/        '  ; o5='             '

    # Row 0 — wordmark letter-tops only; G zone is blank (13 cols).
    printf '             %s%s%s%s%s%s\n' \
        "$Wg" "$g0" "$R" "$Wo" "$o0" "$R"
    # G_R1 — top bar (8 orange) + wordmark body row 1
    printf '  %s████████%s   %s%s%s%s%s%s\n' \
        "$O" "$R" "$Wg" "$g1" "$R" "$Wo" "$o1" "$R"
    # G_R2 — left bar (2 orange + 6 spaces) + wordmark r2
    printf '  %s██%s         %s%s%s%s%s%s\n' \
        "$O" "$R" "$Wg" "$g2" "$R" "$Wo" "$o2" "$R"
    # G_R3 — left + tongue (2 orange + 2 space + 4 orange) + wordmark r3
    printf '  %s██%s  %s████%s   %s%s%s%s%s%s\n' \
        "$O" "$R" "$O" "$R" "$Wg" "$g3" "$R" "$Wo" "$o3" "$R"
    # G_R4 — left + 2 shadow + right (2 orange + 4 shadow + 2 orange) + wordmark r4
    printf '  %s██%s%s████%s%s██%s   %s%s%s%s%s%s\n' \
        "$O" "$R" "$S" "$R" "$O" "$R" "$Wg" "$g4" "$R" "$Wo" "$o4" "$R"
    # G_R5 — bottom bar (8 orange) + wordmark r5 (g's descender)
    printf '  %s████████%s   %s%s%s%s%s%s\n' \
        "$O" "$R" "$Wg" "$g5" "$R" "$Wo" "$o5" "$R"
}

print_banner() {
    # `gitoui --update` already drew its own splash above the update
    # prompt — skip ours so the user doesn't see two banners stacked.
    # `GITOUI_INSTALL_QUIET` was the original combined toggle (v0.1.6);
    # `GITOUI_INSTALL_NO_BANNER` is the per-section successor used by
    # newer binaries to keep the welcome trailer while still skipping
    # the splash.
    [ -n "${GITOUI_INSTALL_QUIET:-}" ] && return
    [ -n "${GITOUI_INSTALL_NO_BANNER:-}" ] && return
    printf '\n'
    proto=$(detect_image_protocol)
    if [ "$proto" != "none" ]; then
        print_banner_image "$proto"
        # Two newlines = one blank separator below the image band, mirroring
        # the writeln!()s at the tail of print_no_repo_splash.
        printf '\n\n'
    else
        print_banner_ascii
        printf '\n'
    fi
    printf '  %sSay oui to the smoothest git terminal youser experience.%s\n' "$DIM" "$RESET"
    printf '  %srepo:%s github.com/%s%s%s\n' "$DIM" "$RESET" "$BOLD" "$REPO" "$RESET"
    printf '\n'
}
# ── Install counter ping ──────────────────────────────────────────────
# Best-effort GET to a Cloudflare Worker that just increments a counter
# so the README badge can show the curl-install total. The Worker only
# accepts pings with `curl/*` / `Wget/*` User-Agent, never logs IP or
# payload, and capped at 2s here so a slow / unreachable counter never
# delays the install. Skip with `GITOUI_NO_TRACK=1`. To point at a
# different Worker (forks): edit the URL below; the worker source is in
# `worker/src/index.js` of this repo.
track_install() {
    if [ -n "${GITOUI_NO_TRACK:-}" ]; then
        return 0
    fi
    # The earlier guard tried to detect an unreplaced `__INSTALL_COUNTER_URL__`
    # placeholder via a `case` — but the same sed pass that swapped the URL
    # also rewrote the case pattern, so the guard tautologically matched the
    # real URL and the ping never fired. Trust the URL now; if it's wrong,
    # curl --max-time 2 + `|| true` swallow the noise.
    curl -fsSL --max-time 2 \
        "https://gitoui-install-counter.nayji7.workers.dev/ping" \
        >/dev/null 2>&1 || true
}

# ── Existing-install detection ────────────────────────────────────────
# Look at concrete filesystem paths (NOT `command -v gitoui` / PATH lookup)
# so we always update the right copy regardless of what comes first on
# the user's PATH. Two roots considered:
#   - $HOME/.cargo/bin/gitoui     (cargo-managed install)
#   - $INSTALL_DIR/gitoui         (binary install we write to)
# Caller may set GITOUI_INSTALL_FORCE_BINARY=1 to skip the cargo check —
# `gitoui --update` does this when the running binary is NOT in
# ~/.cargo/bin, so a leftover cargo install doesn't capture the update.
detect_existing() {
    EXISTING_METHOD="none"
    EXISTING_PATH=""
    CARGO_BIN="${HOME}/.cargo/bin/${BIN_NAME}"
    LOCAL_BIN="${INSTALL_DIR}/${BIN_NAME}"
    if [ -z "${GITOUI_INSTALL_FORCE_BINARY:-}" ] && [ -x "$CARGO_BIN" ]; then
        EXISTING_METHOD="cargo"
        EXISTING_PATH="$CARGO_BIN"
        return
    fi
    if [ -x "$LOCAL_BIN" ]; then
        EXISTING_METHOD="binary"
        EXISTING_PATH="$LOCAL_BIN"
    fi
}

# Fetch the `max_stable_version` from crates.io. Used by the cargo-update
# path to avoid running `cargo install --force` (which always rebuilds)
# when we're already on the latest. Falls back to empty on any failure;
# caller proceeds with cargo install in that case.
fetch_crates_version() {
    curl -fsSL --max-time 5 \
        -H "User-Agent: gitoui-install.sh" \
        "https://crates.io/api/v1/crates/${BIN_NAME}" 2>/dev/null \
        | grep -o '"max_stable_version"[[:space:]]*:[[:space:]]*"[^"]*"' \
        | head -n1 \
        | sed -E 's/.*"max_stable_version"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/'
}

# Cargo-managed install update path. Mirrors the binary path's "already
# up to date → exit 0 silently" / "newer → install + log" semantics so
# the install counter ping (in `main`) only fires when something actually
# changed.
update_via_cargo() {
    info "Detected cargo install at ${BOLD}${EXISTING_PATH}${RESET}."
    need cargo
    LATEST_CRATES="$(fetch_crates_version)"
    CURRENT_RAW="$("$EXISTING_PATH" --version 2>/dev/null || true)"
    CURRENT_VERSION="$(printf '%s\n' "$CURRENT_RAW" | awk 'NF{last=$NF} END{print last}')"
    if [ -n "$LATEST_CRATES" ] && [ "$CURRENT_VERSION" = "$LATEST_CRATES" ]; then
        ok "gitoui ${BOLD}${CURRENT_VERSION}${RESET} is already up to date."
        # Mirror the binary `check_existing` early-exit: print the
        # welcome (suppressed under QUIET) and skip the ping.
        print_welcome
        exit 0
    fi
    info "Found gitoui ${BOLD}${CURRENT_VERSION:-unknown}${RESET} → updating to ${BOLD}${LATEST_CRATES:-latest}${RESET}."
    info "Running ${BOLD}cargo install ${BIN_NAME} --locked --force${RESET}…"
    if ! cargo install "${BIN_NAME}" --locked --force; then
        fail "cargo install failed — leaving the current binary in place."
    fi
    ok "gitoui updated via ${BOLD}cargo${RESET}."
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
# Uses the absolute $EXISTING_PATH set by detect_existing instead of a
# PATH lookup so we always interrogate the same binary we're about to
# overwrite (matters when the user has multiple gitoui copies on PATH).
check_existing() {
    UPDATE=0
    if [ "$EXISTING_METHOD" = "none" ] || [ -z "$EXISTING_PATH" ]; then
        return
    fi
    CURRENT_RAW="$("$EXISTING_PATH" --version 2>/dev/null || true)"
    # `--version` prints `gitoui X.Y.Z` (clap default). Grab the last
    # whitespace-separated token of the LAST line — older binaries used
    # to print the brand splash above the version, and the captured
    # escape sequences would otherwise re-render mid-line when echoed
    # back. Fallback "unknown" keeps the update path live for unknown
    # formats.
    CURRENT_VERSION="$(printf '%s\n' "$CURRENT_RAW" | awk 'NF{last=$NF} END{print last}')"
    [ -n "$CURRENT_VERSION" ] || CURRENT_VERSION="unknown"
    if [ "v${CURRENT_VERSION}" = "$RESOLVED_VERSION" ] \
       || [ "$CURRENT_VERSION" = "$RESOLVED_VERSION" ]; then
        ok "gitoui ${BOLD}${CURRENT_VERSION}${RESET} is already up to date."
        print_welcome
        exit 0
    fi
    info "Found gitoui ${BOLD}${CURRENT_VERSION}${RESET} → updating to ${BOLD}${RESOLVED_VERSION}${RESET}."
    UPDATE=1
}

# ── Download + extract — silent curl, clean error reporting ──────────
# We swap the progress bar for `--silent --show-error` so failures
# report on their own left-aligned line instead of trailing the bar's
# carriage-return cursor at the right edge of the terminal.
download_archive() {
    ARCHIVE="${BIN_NAME}-${VERSION_NUMBER}-${TARGET}.tar.gz"
    URL="https://github.com/${REPO}/releases/download/${RESOLVED_VERSION}/${ARCHIVE}"
    CHECKSUM_URL="https://github.com/${REPO}/releases/download/${RESOLVED_VERSION}/checksum.txt"
    TMP="$(mktemp -d 2>/dev/null || mktemp -d -t gitoui)"
    # shellcheck disable=SC2064
    trap "rm -rf '$TMP'" EXIT INT TERM
    info "Downloading ${BOLD}${ARCHIVE}${RESET}"
    if ! curl --fail --location --silent --show-error \
            -o "${TMP}/${ARCHIVE}" "$URL" 2>"${TMP}/curl.err"; then
        # Curl already wrote a one-line error to curl.err; surface it
        # under the install prefix so the message stays in the column.
        if [ -s "${TMP}/curl.err" ]; then
            printf '    %s%s%s\n' "$DIM" "$(cat "${TMP}/curl.err")" "$RESET"
        fi
        fail "Download failed: $URL"
    fi

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
    info "Installing at ${BOLD}${DEST}${RESET}"
    # Atomic-ish replace: write to a sibling temp path then rename, so a
    # mid-install interruption doesn't leave a half-written binary.
    TMP_DEST="${DEST}.new"
    cp "${TMP}/${BIN_NAME}" "$TMP_DEST" \
        || fail "Cannot write to ${INSTALL_DIR} (permission?)"
    chmod 755 "$TMP_DEST"
    mv -f "$TMP_DEST" "$DEST"
    if [ "$UPDATE" -eq 1 ]; then
        ok "Updated ${BOLD}${BIN_NAME}${RESET}"
    else
        ok "Installed ${BOLD}${BIN_NAME}${RESET}"
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
    # When called from `gitoui --update` in startup-prompt mode, the
    # running gitoui re-execs into the new binary right after we return
    # — the quick-start block would just be redundant noise. Explicit
    # `gitoui --update` keeps the welcome visible since it exits after
    # install rather than re-execing into the TUI.
    [ -n "${GITOUI_INSTALL_QUIET:-}" ] && return
    [ -n "${GITOUI_INSTALL_NO_WELCOME:-}" ] && return
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
    detect_existing
    case "$EXISTING_METHOD" in
        cargo)
            # cargo-managed install — defer to `cargo install`, which
            # owns version resolution (talks to crates.io). update_via_cargo
            # exits 0 silently if already on the latest stable.
            update_via_cargo
            track_install    # only reached when cargo actually installed
            print_welcome
            ;;
        *)
            # Binary path — GitHub Release archives.
            resolve_version
            check_existing      # exits 0 here if already up-to-date — no ping
            download_archive    # exits 1 on failure (`fail()`) — no ping
            install_binary      # exits 1 on failure (`fail()`) — no ping
            track_install       # reached only on a real install or update
            check_path
            print_welcome
            ;;
    esac
}

main

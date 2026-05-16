//! Auto-update — checks crates.io for a newer release at startup, prompts
//! the user, and runs the update via `cargo install` or the hosted
//! `install.sh` depending on how gitoui was installed.
//!
//! State (last check timestamp + "never ask again" preference) lives in
//! `$XDG_CACHE_HOME/gitoui/update.toml` so the main config stays a pure
//! user-edited file.

use std::env;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const CRATES_URL: &str = "https://crates.io/api/v1/crates/gitoui";
const INSTALL_SH_URL: &str = "https://nayji7.github.io/gitoui/install.sh";
const GH_RELEASES_BASE: &str = "https://github.com/NayJi7/gitoui/releases/download";
const CACHE_DIRNAME: &str = "gitoui";
const CACHE_FILENAME: &str = "update.toml";
const CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60; // 1 day
const HTTP_TIMEOUT_SECS: u64 = 4;

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Sidecar cache (purely transient): only the timestamp + last seen
/// version. The "never ask again" preference lives in the main config
/// instead, so the user can revert it by editing
/// `[core.option] check_updates = true`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct UpdateState {
    /// Unix timestamp of the last crates.io check, regardless of outcome.
    #[serde(default)]
    last_checked_at: u64,
    /// Latest version string seen on the last successful check.
    #[serde(default)]
    last_known_version: String,
}

fn cache_path() -> Option<PathBuf> {
    let base = env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join(CACHE_DIRNAME).join(CACHE_FILENAME))
}

fn read_state() -> UpdateState {
    let Some(path) = cache_path() else {
        return UpdateState::default();
    };
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_state(state: &UpdateState) {
    let Some(path) = cache_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(s) = toml::to_string(state) {
        let _ = fs::write(&path, s);
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Fetch the latest stable version published on crates.io. Returns `None`
/// on any failure (network, parsing, …) — callers must treat the absence
/// as "couldn't check this time" and not as "no update available".
fn fetch_latest_from_crates() -> Option<String> {
    #[derive(Deserialize)]
    struct Crate {
        max_stable_version: Option<String>,
        newest_version: Option<String>,
    }
    #[derive(Deserialize)]
    struct Resp {
        #[serde(rename = "crate")]
        crate_: Crate,
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
        // crates.io rejects requests without a User-Agent.
        .user_agent(concat!("gitoui/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()?;
    // The `json` feature on reqwest isn't enabled in our build (avoids
    // pulling in serde_json as a duplicate dep); decode the body manually.
    let body = client.get(CRATES_URL).send().ok()?.text().ok()?;
    let resp: Resp = serde_json::from_str(&body).ok()?;
    resp.crate_
        .max_stable_version
        .or(resp.crate_.newest_version)
}

fn is_newer(latest: &str, current: &str) -> bool {
    match (
        semver::Version::parse(latest),
        semver::Version::parse(current),
    ) {
        (Ok(l), Ok(c)) => l > c,
        // If parsing fails, fall back to string inequality — better than
        // ignoring a release because of a version-string oddity.
        _ => latest != current && !latest.is_empty(),
    }
}

/// Pick the most appropriate update mechanism based on where the running
/// binary sits on disk. Cargo-installed binaries get `cargo install`,
/// everything else (curl one-liner, distro package, manual download) gets
/// the install.sh script.
fn detect_install_method() -> InstallMethod {
    let Ok(exe) = env::current_exe() else {
        return InstallMethod::CurlScript;
    };
    let exe_str = exe.to_string_lossy();
    // Standard cargo install prefix on every OS we support.
    if exe_str.contains("/.cargo/bin/") || exe_str.contains("\\.cargo\\bin\\") {
        // Sanity check that `cargo` is actually on PATH — if not, the
        // user removed it after installing; fall back to the script.
        if which("cargo") {
            return InstallMethod::Cargo;
        }
    }
    InstallMethod::CurlScript
}

#[derive(Debug, Clone, Copy)]
enum InstallMethod {
    Cargo,
    CurlScript,
}

/// Short label describing where the current binary was installed
/// from. Used in the Config view header so the user can tell at a
/// glance which install path will apply on the next `gitoui --update`.
/// `dev` is reported when the binary lives inside a `target/`
/// directory (i.e. a `cargo build` artifact, not a real install).
pub fn install_label() -> &'static str {
    let Ok(exe) = env::current_exe() else {
        return "unknown";
    };
    let exe_str = exe.to_string_lossy();
    if exe_str.contains("/target/debug/") || exe_str.contains("/target/release/") {
        return "dev";
    }
    match detect_install_method() {
        InstallMethod::Cargo => "cargo",
        InstallMethod::CurlScript => "curl",
    }
}

fn which(cmd: &str) -> bool {
    Command::new(cmd)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Run the appropriate installer for the detected install method.
///
/// `keep_welcome` controls whether install.sh prints its `Quick start`
/// trailer (or, for the cargo path, whether `update.rs` prints an
/// equivalent itself after `cargo install` returns):
///
/// - `true`  → explicit `gitoui --update`. The user invoked update as
///   a one-shot command; show them how to launch gitoui afterwards
///   instead of re-execing into the TUI.
/// - `false` → auto-prompt at startup. The user was about to enter
///   the TUI; suppress the welcome so the re-exec lands them straight
///   in the running app.
fn run_update(keep_welcome: bool) -> io::Result<()> {
    let method = detect_install_method();
    println!();
    match method {
        InstallMethod::Cargo => {
            // `--quiet` mutes cargo's "Compiling …" stream but still
            // surfaces warnings / errors — gives the same visual
            // calm as install.sh's progress lines.
            let status = Command::new("cargo")
                .args(["install", "gitoui", "--locked", "--force", "--quiet"])
                .status()?;
            if !status.success() {
                return Err(io::Error::other(
                    "cargo install exited non-zero — leaving current binary in place",
                ));
            }
            // Cargo doesn't print a quick-start hint; we do it
            // ourselves so the user lands in the same spot as
            // install.sh's tail.
            if keep_welcome {
                print_post_update_welcome();
            }
        }
        InstallMethod::CurlScript => {
            // sh -c "curl -fsSL <url> | sh" so the pipe stays inside
            // the shell. install.sh honours these env knobs:
            //   GITOUI_INSTALL_NO_BANNER   skip its brand splash
            //   GITOUI_INSTALL_NO_WELCOME  skip its quick-start trailer
            //   GITOUI_INSTALL_FORCE_BINARY  pin to ~/.local/bin even
            //     if a stale ~/.cargo/bin/gitoui exists
            let mut cmd = Command::new("sh");
            cmd.env("GITOUI_INSTALL_NO_BANNER", "1")
                .env("GITOUI_INSTALL_FORCE_BINARY", "1");
            if !keep_welcome {
                cmd.env("GITOUI_INSTALL_NO_WELCOME", "1");
            }
            let status = cmd
                .arg("-c")
                .arg(format!("curl -fsSL '{INSTALL_SH_URL}' | sh"))
                .status()?;
            if !status.success() {
                return Err(io::Error::other(
                    "install.sh exited non-zero — leaving current binary in place",
                ));
            }
        }
    }
    Ok(())
}

/// Mirrors install.sh's `print_welcome` so the cargo path (which doesn't
/// invoke install.sh) ends on the same Quick-start trailer when the user
/// explicitly ran `gitoui --update`.
fn print_post_update_welcome() {
    use std::io::IsTerminal;
    let color = io::stdout().is_terminal() && env::var_os("NO_COLOR").is_none();
    let (orange, bold, dim, reset) = if color {
        ("\x1b[38;2;240;81;51m", "\x1b[1m", "\x1b[2m", "\x1b[0m")
    } else {
        ("", "", "", "")
    };
    println!();
    println!("  {bold}Quick start{reset} — inside any git repo:");
    println!("    {orange}{bold}gitoui{reset}        {dim}open the commit-graph viewer{reset}");
    println!("    {orange}{bold}gitoui --help{reset} {dim}all flags and options{reset}");
    println!();
    println!(
        "  {dim}In-app:{reset} press {bold}?{reset} anywhere for the keymap, {bold}q{reset} to quit."
    );
    println!();
}

/// Maps the Rust target triple of the running binary to the same triple
/// used in the release archive filenames. Returns `None` on a platform
/// we don't publish prebuilt binaries for.
fn current_target_triple() -> Option<&'static str> {
    use std::env::consts::{ARCH, OS};
    Some(match (OS, ARCH) {
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        _ => return None,
    })
}

/// Check whether the GitHub Release archive for `version` and the
/// running platform is published yet. Returns:
/// - `Some(true)`  → archive exists, safe to run install.sh
/// - `Some(false)` → archive 404s (release was just tagged, build CI
///   still running — we mustn't prompt the user yet)
/// - `None`        → network error or unsupported platform, caller
///   should err on the side of "ask anyway".
///
/// Avoids the race where `crates.io` already publishes a new version
/// (parallel `publish-crate` job finishes in seconds) but the multi-arch
/// `release` job hasn't uploaded the binaries yet — running install.sh
/// in that window resolves an older tag and silently exits "up to date".
fn github_release_ready(version: &str) -> Option<bool> {
    let target = current_target_triple()?;
    let url = format!("{GH_RELEASES_BASE}/v{version}/gitoui-{version}-{target}.tar.gz");
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
        .user_agent(concat!("gitoui/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()?;
    // HEAD follows redirects by default (the asset URL 302's to S3),
    // so a final 200 means the file is downloadable.
    let resp = client.head(&url).send().ok()?;
    Some(resp.status().is_success())
}

/// Resolve the path the running binary should be re-execed from. Three
/// fallback layers to survive the rough edges of `current_exe()` after
/// install.sh has just swapped the binary in place:
///
/// 1. `current_exe()` as-is if the file still exists at that path
///    (true when install.sh wrote elsewhere, or on macOS).
/// 2. Strip a literal " (deleted)" suffix — Linux readlink(/proc/self/exe)
///    appends that when the original inode was unlinked or replaced
///    (which is exactly what `mv -f` inside `install_binary` does).
/// 3. `PATH`-based lookup of `gitoui` as a last resort, for the case
///    where install.sh wrote to a different directory than the one the
///    user invoked us from.
fn resolved_self_path() -> io::Result<PathBuf> {
    let raw = env::current_exe()?;
    if raw.exists() {
        return Ok(raw);
    }
    let s = raw.to_string_lossy();
    if let Some(stripped) = s.strip_suffix(" (deleted)") {
        let candidate = PathBuf::from(stripped);
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    let path_var = env::var_os("PATH").unwrap_or_default();
    for dir in env::split_paths(&path_var) {
        let candidate = dir.join("gitoui");
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "gitoui binary not found after install — re-run `gitoui` manually",
    ))
}

/// Args to forward to the re-execed binary. Drops `--update` so the new
/// binary doesn't immediately loop into another update check — we just
/// finished updating, the user's intent is satisfied.
fn forwarded_args() -> Vec<String> {
    env::args().skip(1).filter(|a| a != "--update").collect()
}

/// Replace the current process with a freshly-execed copy of the same
/// binary path so the user lands inside the just-updated app without
/// re-typing the command.
#[cfg(unix)]
fn exec_self() -> io::Result<()> {
    use std::os::unix::process::CommandExt;
    let exe = resolved_self_path()?;
    // `.exec()` only returns on failure.
    Err(Command::new(exe).args(forwarded_args()).exec())
}

#[cfg(not(unix))]
fn exec_self() -> io::Result<()> {
    // Windows path: spawn the new process and exit cleanly.
    let exe = resolved_self_path()?;
    let status = Command::new(exe).args(forwarded_args()).status()?;
    std::process::exit(status.code().unwrap_or(0));
}

/// Outcome of a startup check — used by `lib::run()` to decide whether
/// to keep going into the UI or not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckOutcome {
    /// Either the check was skipped (cache fresh, never_ask, non-TTY,
    /// network down) or no newer version is available.
    Continue,
    /// User declined this time but didn't disable future checks.
    Skipped,
    /// User picked `N` — preference persisted, won't ask again.
    SilencedForever,
    /// Update succeeded; `lib::run()` should NOT enter the UI (we're
    /// about to `exec` the new binary).
    Updated,
}

/// Startup check. Reads the cache, hits crates.io if stale, and prompts
/// when a newer version is found. Returns the chosen outcome.
///
/// `splash` is invoked just before the prompt so the brand artwork
/// shows above it — same convention as `--help` / `--version` /
/// no-repo paths in the main binary.
pub fn maybe_check_at_startup(splash: impl FnOnce()) -> CheckOutcome {
    if !user_wants_checks() {
        return CheckOutcome::Continue;
    }
    let mut state = read_state();
    if !io::stdout().is_terminal() || !io::stdin().is_terminal() {
        return CheckOutcome::Continue;
    }

    let now = unix_now();
    let cache_fresh = now.saturating_sub(state.last_checked_at) < CHECK_INTERVAL_SECS
        && !state.last_known_version.is_empty();

    let latest = if cache_fresh {
        state.last_known_version.clone()
    } else {
        match fetch_latest_from_crates() {
            Some(v) => {
                state.last_checked_at = now;
                state.last_known_version = v.clone();
                write_state(&state);
                v
            }
            None => return CheckOutcome::Continue,
        }
    };

    if !is_newer(&latest, current_version()) {
        return CheckOutcome::Continue;
    }
    // For curl-installed binaries, only prompt once GitHub has the
    // matching archive — otherwise the user says yes and install.sh
    // resolves the OLD tag, prints a misleading "already up to date".
    // Silent skip here: a tomorrow-startup check will surface the
    // version when the binaries land.
    if matches!(detect_install_method(), InstallMethod::CurlScript)
        && github_release_ready(&latest) == Some(false)
    {
        return CheckOutcome::Continue;
    }
    splash();
    // Startup-prompt path → after the update we re-exec into the TUI
    // so the user lands directly in the running app (their original
    // intent was to launch gitoui, the update was a side trip).
    prompt_and_act(&latest, /* relaunch_after = */ true)
}

/// Force an update check + prompt regardless of cache or the
/// `check_updates = false` config opt-out. Wired to `gitoui --update`
/// so the user always has an explicit escape hatch.
pub fn force_check(splash: impl FnOnce()) -> CheckOutcome {
    splash();
    println!("Checking for updates…");
    let latest = match fetch_latest_from_crates() {
        Some(v) => v,
        None => {
            eprintln!("Couldn't reach crates.io — no update check this time.");
            return CheckOutcome::Continue;
        }
    };
    let mut state = read_state();
    state.last_checked_at = unix_now();
    state.last_known_version = latest.clone();
    write_state(&state);

    if !is_newer(&latest, current_version()) {
        println!("Already on the latest version (v{}).", current_version());
        return CheckOutcome::Continue;
    }
    // Curl-installed: confirm GitHub has the matching archive before
    // we offer the prompt. Explicit user-driven `--update` deserves a
    // visible message instead of the silent skip the startup check
    // does, so the user understands the wait.
    if matches!(detect_install_method(), InstallMethod::CurlScript)
        && github_release_ready(&latest) == Some(false)
    {
        println!();
        println!("  gitoui v{latest} is published on crates.io but the prebuilt");
        println!("  binaries aren't on GitHub yet (release CI still building).");
        println!("  Try again in a few minutes, or build from source via");
        println!("  `cargo install gitoui --locked --force`.");
        return CheckOutcome::Continue;
    }
    // Explicit `--update` is a one-shot intent: print the install.sh-
    // style welcome at the end and exit — don't drop the user into the
    // TUI like the startup-prompt path does.
    prompt_and_act(&latest, /* relaunch_after = */ false)
}

fn prompt_and_act(latest: &str, relaunch_after: bool) -> CheckOutcome {
    println!();
    println!(
        "  gitoui v{latest} is available — you have v{}.",
        current_version()
    );
    print!("  Update now? [y = yes, n = skip, N = never ask again]: ");
    let _ = io::stdout().flush();

    let mut buf = String::new();
    if io::stdin().read_line(&mut buf).is_err() {
        return CheckOutcome::Continue;
    }
    let trimmed = buf.trim();
    let choice = if trimmed == "y" || trimmed == "yes" {
        Choice::Yes
    } else if trimmed == "N" {
        Choice::Never
    } else {
        Choice::No
    };

    match choice {
        Choice::Yes => match run_update(/* keep_welcome = */ !relaunch_after) {
            Ok(()) => {
                if relaunch_after {
                    // Startup path — hop into the freshly-installed
                    // binary so the user lands in the TUI they wanted
                    // to launch in the first place.
                    match exec_self() {
                        Ok(()) => CheckOutcome::Updated,
                        Err(e) => {
                            eprintln!("Couldn't re-exec the updated binary: {e}");
                            eprintln!("Re-run `gitoui` manually to use the new version.");
                            std::process::exit(0);
                        }
                    }
                } else {
                    // Explicit `--update` — install.sh (or our cargo
                    // helper above) already printed the Quick-start
                    // welcome. Exit cleanly so the user can re-run
                    // `gitoui` themselves when they're ready.
                    std::process::exit(0);
                }
            }
            Err(e) => {
                eprintln!("Update failed: {e}");
                eprintln!("Continuing with the current version.");
                CheckOutcome::Skipped
            }
        },
        Choice::No => CheckOutcome::Skipped,
        Choice::Never => {
            disable_in_user_config();
            println!("OK — gitoui won't ask about updates again.");
            if let Some(p) = crate::config::resolve_config_file_path() {
                println!(
                    "(Re-enable by removing `check_updates = false` from {} \
                     or by running `gitoui --update`.)",
                    p.display()
                );
            } else {
                println!("(Re-enable by removing `check_updates = false` from your config.)");
            }
            CheckOutcome::SilencedForever
        }
    }
}

enum Choice {
    Yes,
    No,
    Never,
}

/// Probe `[core.option] check_updates` directly from the user config
/// file. Done with a minimal parse rather than `crate::config::load()`
/// so the check runs before the full config layer (syntect themes, …)
/// pays its cost — and so a broken config doesn't block the update
/// check itself.
fn user_wants_checks() -> bool {
    let Some(path) = crate::config::resolve_config_file_path() else {
        return true;
    };
    let Ok(text) = fs::read_to_string(&path) else {
        return true;
    };
    let Ok(doc) = text.parse::<toml::Table>() else {
        return true;
    };
    doc.get("core")
        .and_then(|c| c.as_table())
        .and_then(|c| c.get("option"))
        .and_then(|o| o.as_table())
        .and_then(|o| o.get("check_updates"))
        .and_then(|v| v.as_bool())
        .unwrap_or(true)
}

/// Persist `[core.option] check_updates = false` to the user config.
/// Preserves any other entries in the file by editing the parsed
/// toml::Table in place.
fn disable_in_user_config() {
    let Some(path) = crate::config::resolve_config_file_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut doc = fs::read_to_string(&path)
        .ok()
        .and_then(|s| s.parse::<toml::Table>().ok())
        .unwrap_or_default();
    let core = doc
        .entry("core".to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let core = match core.as_table_mut() {
        Some(t) => t,
        None => return,
    };
    let option = core
        .entry("option".to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let option = match option.as_table_mut() {
        Some(t) => t,
        None => return,
    };
    option.insert("check_updates".to_string(), toml::Value::Boolean(false));
    let _ = fs::write(&path, doc.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_versions_compare_correctly() {
        assert!(is_newer("0.1.4", "0.1.3"));
        assert!(is_newer("0.2.0", "0.1.99"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.1.3", "0.1.3"));
        assert!(!is_newer("0.1.2", "0.1.3"));
    }

    #[test]
    fn fallback_string_compare_when_unparseable() {
        // Empty latest is treated as "no update".
        assert!(!is_newer("", "0.1.3"));
        // Garbage version that fails semver still returns true if
        // strings differ — better than missing a release.
        assert!(is_newer("nightly-1234", "0.1.3"));
    }
}

pub mod avatar;
pub mod color;
pub mod config;
pub mod git;
pub mod github;
pub mod github_auth;
pub mod graph;
pub mod highlight;
pub mod protocol;
pub mod themes;

mod app;
mod brand;
mod check;
mod dir_input;
mod event;
mod external;
mod keybind;
mod recents;
mod update;
mod view;
mod watcher;
mod widget;

use std::{path::Path, rc::Rc};

use app::{App, Ret};
use clap::{Parser, ValueEnum};
use graph::GraphImageManager;
use serde::Deserialize;

/// Gitoui - Say oui to the smoothest git terminal youser experience
#[derive(Parser)]
#[command(version)]
struct Args {
    /// Maximum number of commits to render
    #[arg(short = 'n', long, value_name = "NUMBER")]
    max_count: Option<usize>,

    /// Image protocol to render graph [default: auto]
    #[arg(short, long, value_name = "TYPE")]
    protocol: Option<ImageProtocolType>,

    /// Commit ordering algorithm [default: chrono]
    #[arg(short, long, value_name = "TYPE")]
    order: Option<CommitOrderType>,

    /// Commit graph image cell width [default: auto]
    #[arg(short, long, value_name = "TYPE")]
    graph_width: Option<GraphWidthType>,

    /// Commit graph image edge style [default: rounded]
    #[arg(short = 's', long, value_name = "TYPE")]
    graph_style: Option<GraphStyle>,

    /// Initial selection of commit [default: latest]
    #[arg(short, long, value_name = "TYPE")]
    initial_selection: Option<InitialSelection>,

    /// Force an update check + prompt now, then exit
    #[arg(long, conflicts_with_all = &["no_update_check"])]
    update: bool,

    /// Skip the once-a-day automatic update check at startup
    #[arg(long)]
    no_update_check: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImageProtocolType {
    Auto,
    Iterm,
    Kitty,
    KittyUnicode,
    Sixel,
}

impl From<Option<ImageProtocolType>> for protocol::ImageProtocol {
    fn from(protocol: Option<ImageProtocolType>) -> Self {
        match protocol {
            Some(ImageProtocolType::Auto) => protocol::auto_detect(),
            Some(ImageProtocolType::Iterm) => protocol::ImageProtocol::Iterm2,
            Some(ImageProtocolType::Kitty) => protocol::ImageProtocol::Kitty,
            Some(ImageProtocolType::KittyUnicode) => protocol::ImageProtocol::KittyUnicode {
                tmux: protocol::detect_tmux(),
            },
            Some(ImageProtocolType::Sixel) => protocol::ImageProtocol::Sixel,
            None => protocol::auto_detect(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommitOrderType {
    Chrono,
    Topo,
}

impl From<Option<CommitOrderType>> for git::SortCommit {
    fn from(order: Option<CommitOrderType>) -> Self {
        match order {
            Some(CommitOrderType::Chrono) => git::SortCommit::Chronological,
            Some(CommitOrderType::Topo) => git::SortCommit::Topological,
            None => git::SortCommit::Chronological,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GraphWidthType {
    Auto,
    Double,
    Single,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GraphStyle {
    Rounded,
    Angular,
    Smooth,
}

impl From<Option<GraphStyle>> for graph::GraphStyle {
    fn from(style: Option<GraphStyle>) -> Self {
        match style {
            Some(GraphStyle::Rounded) => graph::GraphStyle::Rounded,
            Some(GraphStyle::Angular) => graph::GraphStyle::Angular,
            Some(GraphStyle::Smooth) => graph::GraphStyle::Smooth,
            None => graph::GraphStyle::Rounded,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InitialSelection {
    Latest,
    Head,
}

impl From<Option<InitialSelection>> for app::InitialSelection {
    fn from(selection: Option<InitialSelection>) -> Self {
        match selection {
            Some(InitialSelection::Latest) => app::InitialSelection::Latest,
            Some(InitialSelection::Head) => app::InitialSelection::Head,
            None => app::InitialSelection::Latest,
        }
    }
}

/// Top-level error type returned by `gitoui::run()` and the entry points it
/// composes. Variants are deliberately coarse — finer-grained domain errors
/// (e.g. `git::actions` returning `Result<_, String>`) are surfaced as `String`
/// inside their respective variants so the user sees the underlying message
/// verbatim.
///
/// Conversions:
/// - `std::io::Error` → `Error::Io` (auto via `?`)
/// - `String` / `&str` → `Error::Other` (lets us keep `Err(msg.into())` shorthand
///   in legacy call sites without forcing every parser to pick a variant)
/// - `toml::de::Error`, `garde::Report` → `Error::Config` (for `?` chaining
///   inside `config::load`)
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("git: {0}")]
    Git(String),

    #[error("config: {0}")]
    Config(String),

    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}

impl From<String> for Error {
    fn from(s: String) -> Self {
        Error::Other(s)
    }
}

impl From<&str> for Error {
    fn from(s: &str) -> Self {
        Error::Other(s.to_string())
    }
}

impl From<toml::de::Error> for Error {
    fn from(e: toml::de::Error) -> Self {
        Error::Config(e.to_string())
    }
}

impl From<garde::Report> for Error {
    fn from(e: garde::Report) -> Self {
        Error::Config(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub fn run() -> Result<()> {
    // Print the gitoui splash above clap's help / error output so the brand
    // shows on every entry path, not just the "no-repo" prompt. clap's
    // `parse()` would auto-exit before we get a chance to draw, so we use
    // `try_parse()` and re-emit the formatted message ourselves.
    let args = match Args::try_parse() {
        Ok(args) => args,
        Err(err) => {
            let proto = protocol::auto_detect();
            print_no_repo_splash(proto);
            // clap separates "expected" exits (--help / --version) from real
            // failures via `ErrorKind`. We mirror clap's behaviour: success
            // exit on help/version, non-zero on parse errors. Using `print`
            // (stdout) for help/version and `eprint` (stderr) for errors
            // matches what `parse()` would have done.
            use clap::error::ErrorKind;
            match err.kind() {
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
                    print!("{}", err);
                    std::process::exit(0);
                }
                _ => {
                    eprint!("{}", err);
                    std::process::exit(2);
                }
            }
        }
    };

    // `--update` is the explicit escape hatch — always prompts, ignores
    // the cache + the "never_ask" preference, and exits afterwards.
    // Splash is shown above the check just like the --help / --version
    // paths so the brand surfaces on every entry point.
    if args.update {
        let proto = protocol::auto_detect();
        update::force_check(|| print_no_repo_splash(proto));
        return Ok(());
    }
    // Otherwise, do the silent once-a-day check + prompt. The function
    // bails out fast (no network) when the cache is fresh, the user has
    // chosen "never", or we're not attached to a TTY. On `Updated` it
    // already exec'd the new binary and never returns. Splash only
    // prints if we're actually about to prompt — silent paths stay
    // silent.
    if !args.no_update_check {
        let proto = protocol::auto_detect();
        let _ = update::maybe_check_at_startup(|| print_no_repo_splash(proto));
    }

    // Validate the user config FIRST — before we pay the cost of
    // initialising syntect (which loads ~100 syntax definitions and a
    // dozen theme files) or spawning the event thread. A misformatted
    // TOML or unknown theme name otherwise added ~half a second of
    // latency before the user saw the diagnostic.
    let mut preflight_config = match config::load_or_diagnose() {
        Ok(c) => Some(c),
        Err(diag) => {
            let proto = protocol::auto_detect();
            print_no_repo_splash(proto);
            diag.write_to_stderr();
            std::process::exit(1);
        }
    };

    highlight::init();
    let ec = event::EventController::init();
    let mut refresh_view_context = None;
    let mut terminal = None;
    // Counter incremented each time the user triggers `LoadMore` from the
    // commit list. Multiplies `core.option.load_more_count` to compute the
    // current commit cap when the CLI did not pass an explicit `-n`.
    let mut load_more_count: usize = 0;
    // Filesystem watcher on .git/ — keeps the UI in sync with external git
    // operations (commits from another shell, push/pull/fetch, branch
    // switches, …). Held alive for the whole `run()` lifetime; dropping it
    // stops the watcher thread. Created at most once on the first iteration
    // where the .git directory becomes available.
    let mut _git_watcher: Option<_> = None;

    let ret = loop {
        // First iteration uses the diagnostic-bearing loader so any
        // failure renders as a styled --help-style error block and
        // exits cleanly. Subsequent reloads (config reopened from
        // inside the app) go through the legacy `config::load()` and
        // log the error to stderr — the running session shouldn't
        // crash because the user typo'd a hex code mid-edit.
        let (mut core_config, ui_config, mut graph_config, mut color_theme, keybind_patch) =
            if let Some(c) = preflight_config.take() {
                // First iteration consumes the pre-flight result that we
                // computed *before* paying for syntect / event thread.
                c
            } else if terminal.is_none() {
                // Pre-flight got consumed by an earlier iteration that
                // bailed (`Ret::Refresh` re-enters this loop, etc.) —
                // re-load with the diagnostic-bearing path.
                match config::load_or_diagnose() {
                    Ok(config) => config,
                    Err(diag) => {
                        let proto = protocol::auto_detect();
                        print_no_repo_splash(proto);
                        diag.write_to_stderr();
                        std::process::exit(1);
                    }
                }
            } else {
                match config::load() {
                    Ok(config) => config,
                    Err(e) => {
                        eprintln!("Failed to reload config: {e}");
                        continue;
                    }
                }
            };
        // Resolve the configured theme: built-in name OR a user file at
        // `~/.config/gitoui/themes/<name>.toml`. The loader path already
        // validated this returns Ok at boot, so any error here would be from
        // a config-reload mid-session — fall back silently in that case.
        if !core_config.option.theme.is_empty() {
            if let Ok(def) = crate::themes::resolve_or_load(&core_config.option.theme) {
                color_theme = def.color_theme;
                core_config.option.syntax_theme = def.syntax_theme;
                // Theme-tinted graph palette wins over the generic config default
                // — but only if the theme actually shipped one. An empty Vec means
                // "the theme didn't customize the graph", so we leave the user's
                // `[graph.color.branches]` setting alone.
                if !color_theme.graph_branches.is_empty() {
                    graph_config.color.branches = color_theme.graph_branches.clone();
                }
            }
        }
        let keybind = keybind::KeyBinds::new(keybind_patch);

        // Lazy-load policy: an explicit CLI `-n` always wins. Otherwise default
        // to `initial_load_count` and grow by `load_more_count` each time the
        // user requests "load more" from the commit list.
        let max_count = match args.max_count {
            Some(n) => Some(n),
            None => Some(
                core_config.option.initial_load_count
                    + load_more_count * core_config.option.load_more_count,
            ),
        };
        let image_protocol = args.protocol.or(core_config.option.protocol).into();
        let order = args.order.or(core_config.option.order).into();
        let graph_width = args.graph_width.or(core_config.option.graph_width);
        let graph_style = args.graph_style.or(core_config.option.graph_style).into();
        let graph_image_width_mode = graph_config.row_image_width;
        let initial_selection = args
            .initial_selection
            .or(core_config.option.initial_selection)
            .into();

        // Centralised graph-palette construction — see `build_graph_color_set`
        // for the theme-vs-config precedence and the transparent-background
        // patching logic.
        let graph_color_set = color::build_graph_color_set(&color_theme, &graph_config.color);

        let mouse_enabled = ui_config.common.mouse_enabled;
        let git_user_name = std::process::Command::new("git")
            .args(["config", "user.name"])
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| "Not set".into());
        let git_user_email = std::process::Command::new("git")
            .args(["config", "user.email"])
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| "Not set".into());
        let git_default_branch = std::process::Command::new("git")
            .args(["config", "init.defaultBranch"])
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| "main".into());
        let github_auth_state = github_auth::load_state();
        let avatar_manager = avatar::AvatarManager::new(
            image_protocol,
            github_repos_from_remotes(),
            github_auth_state.token.clone(),
            Some(ec.sender()),
        );
        let mut avatar_manager = avatar_manager;
        avatar_manager.set_github_avatars(core_config.github_avatars());
        let default_branch = core_config
            .option
            .default_branch
            .clone()
            .unwrap_or_else(|| git_default_branch.clone());
        let mut ctx = Rc::new(app::AppContext {
            keybind,
            core_config,
            ui_config,
            color_theme,
            image_protocol,
            avatar_manager: std::sync::Mutex::new(avatar_manager),
            git_user_name,
            git_user_email,
            git_default_branch: git_default_branch.clone(),
            github_auth_state,
            branch_color_map: rustc_hash::FxHashMap::default(),
            graph_color_set: graph_color_set.clone(),
            graph_config: graph_config.clone(),
            // Filled after the repository is loaded — see below.
            current_branch_remote_state: None,
            repo_path: std::path::PathBuf::new(),
        });

        // If we were launched from a sub-directory of a repo, jump up to
        // the work-tree root so the header pwd, all `git` invocations,
        // and the FS watcher anchor at the same place — the repo, not
        // wherever the shell happened to be when the user typed
        // `gitoui`. Failures here are harmless: when the cwd isn't in a
        // repo at all the next `Repository::load(".", ...)` returns the
        // no-repo error and the splash takes over.
        if let Some(root) = git::find_repo_root(Path::new(".")) {
            let _ = std::env::set_current_dir(&root);
        }
        let repository = match git::Repository::load(Path::new("."), order, max_count) {
            Ok(repo) => repo,
            Err(e) if terminal.is_none() => {
                let err_str = e.to_string().to_lowercase();
                let is_no_repo = err_str.contains("not a git repository")
                    || err_str.contains("repository not found")
                    || err_str.contains("could not find")
                    || err_str.contains("not found");
                if is_no_repo {
                    let cwd = std::env::current_dir()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|_| ".".to_string());
                    use std::io::Write;
                    print_no_repo_splash(image_protocol);
                    print!(
                        "No git repository found in '{}'.\nInitialize a new repository here? (y/n) ",
                        cwd
                    );
                    let _ = std::io::stdout().flush();
                    let mut input = String::new();
                    if std::io::stdin().read_line(&mut input).is_ok()
                        && input.trim().eq_ignore_ascii_case("y")
                    {
                        let status = std::process::Command::new("git")
                            .args(["init", "-b", &default_branch])
                            .status();
                        match status {
                            Ok(s) if s.success() => {
                                println!(
                                    "Initialized repository with branch '{}'.",
                                    default_branch
                                );
                            }
                            _ => {
                                let _ = std::process::Command::new("git").arg("init").status();
                                println!("Initialized repository.");
                            }
                        }
                        continue;
                    } else {
                        break Err(e);
                    }
                } else {
                    break Err(e);
                }
            }
            Err(e) => {
                eprintln!("Failed to load repository: {e}");
                continue;
            }
        };

        // Start the filesystem watcher once per run() — the repo path is
        // stable across config-reload iterations.
        if _git_watcher.is_none() {
            _git_watcher = watcher::start(repository.path(), ec.sender());
        }

        // Compute (ahead, behind) of the current branch vs its upstream so
        // the status bar can show whether the user needs to push or pull.
        // Done here (not in AppContext::new) because we need the loaded
        // repository to know the current branch. The Rc is unique at this
        // point (no clones yet), so get_mut succeeds.
        let remote_state = match repository.head() {
            git::Head::Branch { name } => {
                let upstream = git::actions::branch_upstream(repository.path(), name);
                if upstream.as_ref().map_or(true, |u| u.is_empty()) {
                    None
                } else {
                    let ahead = git::actions::branch_ahead_count(repository.path(), name)
                        .ok()
                        .and_then(|s| s.parse::<usize>().ok())
                        .unwrap_or(0);
                    let behind = git::actions::branch_behind_count(repository.path(), name)
                        .ok()
                        .and_then(|s| s.parse::<usize>().ok())
                        .unwrap_or(0);
                    Some((ahead, behind))
                }
            }
            _ => None,
        };
        if let Some(ctx_mut) = Rc::get_mut(&mut ctx) {
            ctx_mut.current_branch_remote_state = remote_state;
            ctx_mut.repo_path = repository.path().to_path_buf();
        }

        let graph = graph::calc_graph(&repository);

        let cell_width_type = check::decide_cell_width_type(&graph, graph_width)?;

        let graph_image_manager = GraphImageManager::new(
            &graph,
            &graph_color_set,
            cell_width_type,
            graph_style,
            graph_image_width_mode,
            image_protocol,
        );

        if terminal.is_none() {
            terminal = Some(ratatui::init());
            // Push the kitty keyboard protocol disambiguation flag
            // RIGHT after raw mode + alt-screen are entered. Without
            // this, terminals fall back to xterm's legacy meta encoding
            // (Alt+letter → ESC + letter), which is unreliable when the
            // two bytes drift apart in the read window — `Alt+c` is
            // then delivered as a stray `Esc` that closes the active
            // view, with the `c` arriving too late to be combined.
            // Terminals that don't speak the protocol silently ignore
            // the push; the `state` strip in `event::EventController`
            // normalizes the lock-key bits this protocol attaches.
            let _ = ratatui::crossterm::execute!(
                std::io::stdout(),
                ratatui::crossterm::event::PushKeyboardEnhancementFlags(
                    ratatui::crossterm::event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES,
                )
            );
            if mouse_enabled {
                ratatui::crossterm::execute!(
                    std::io::stdout(),
                    ratatui::crossterm::event::EnableMouseCapture
                )
                .unwrap();
            }
        }

        let mut app = App::new(
            &repository,
            graph_image_manager,
            &graph,
            &graph_color_set,
            cell_width_type,
            graph_style,
            initial_selection,
            ctx.clone(),
            &ec,
            refresh_view_context,
        );

        match app.run(terminal.as_mut().unwrap()) {
            Ok(Ret::Quit) => {
                break Ok(());
            }
            Ok(Ret::Refresh(request)) => {
                refresh_view_context = Some(request.context);
                continue;
            }
            Ok(Ret::LoadMore(request)) => {
                // CLI `-n` is hard cap; only grow when no explicit limit was given.
                if args.max_count.is_none() {
                    load_more_count = load_more_count.saturating_add(1);
                }
                refresh_view_context = Some(request.context);
                continue;
            }
            Err(e) => {
                // app::run returns std::io::Error; the From impl on Error wraps it.
                break Err(e.into());
            }
        }
    };

    // Mirror the disambiguate push from startup with a Pop so we don't
    // leave the user's terminal stuck in enhanced-keys mode after
    // gitoui exits. Best-effort; terminals that ignored the push will
    // also ignore the pop.
    let _ = ratatui::crossterm::execute!(
        std::io::stdout(),
        ratatui::crossterm::event::PopKeyboardEnhancementFlags,
    );
    ratatui::crossterm::execute!(
        std::io::stdout(),
        ratatui::crossterm::event::DisableMouseCapture
    )
    .unwrap();
    ratatui::restore();
    ret
}

/// Print the gitoui logo + wordmark inline (opencode-style) before the no-repo
/// prompt. The G logomark is rendered SMALLER than the wordmark and vertically
/// centered next to it, so the wordmark text dominates the splash. Layout in
/// terminal cells:
///
/// ```text
///   .. [G..] .[wordmark...........]   ← rows R0..R0+4 (5-row band)
///   ↑   ↑    ↑     ↑
///   2   logo gap   wordmark
/// ```
///
/// Both Kitty (default `C=0`) and iTerm2 advance the cursor by `(cell_w, cell_h-1)`
/// after rendering — i.e. the cursor lands on the LAST row of the image. Two
/// trailing newlines therefore produce exactly one blank separator row before
/// the prompt. Falls back to a plain text banner when the protocol can't
/// render inline (e.g. KittyUnicode placeholder mode).
fn print_no_repo_splash(image_protocol: protocol::ImageProtocol) {
    use std::io::Write;

    let left_margin: u16 = 2;
    let logo_w: usize = 8;
    let logo_h: usize = 5;
    let gap: u16 = 2;
    let wm_w: usize = 21;
    let wm_h: usize = 5;
    let total_h: u16 = wm_h as u16;
    // Logo and wordmark are now both 5 rows tall — same baseline, no slack.
    let logo_y_offset: u16 = 0;

    println!();

    let render_fallback = || println!("        gitoui\n");
    let Some(logo_png) = brand::render_logo_sized(logo_w as u32, logo_h as u32) else {
        render_fallback();
        return;
    };
    let Some(wm_png) = brand::render_wordmark_sized(wm_w as u32, wm_h as u32) else {
        render_fallback();
        return;
    };
    // Logo first: emit with the d=C clear prefix (no prior placements to worry
    // about). Wordmark second: skip the clear prefix so we don't accidentally
    // erase the logo on terminals that interpret d=C row-wide rather than
    // cell-wide (e.g. Ghostty when both images share rows).
    let Some(logo_esc) = image_protocol.encode_inline(&logo_png, logo_w, logo_h, 1, true) else {
        render_fallback();
        return;
    };
    let Some(wm_esc) = image_protocol.encode_inline(&wm_png, wm_w, wm_h, 2, false) else {
        render_fallback();
        return;
    };

    let mut stdout = std::io::stdout().lock();
    // Reserve total_h rows so the terminal scrolls if the cursor is near the bottom.
    for _ in 0..total_h {
        let _ = writeln!(stdout);
    }
    // Move cursor back up to the top-left of the reserved band.
    let _ = write!(stdout, "\x1b[{}A", total_h);
    // Save cursor at (top, 0) so we can come back here for the wordmark
    // without depending on how each protocol advances the cursor after an image.
    let _ = write!(stdout, "\x1b7");

    // --- Emit the small G logomark ---
    if logo_y_offset > 0 {
        let _ = write!(stdout, "\x1b[{}B", logo_y_offset);
    }
    let _ = write!(stdout, "\x1b[{}C", left_margin);
    let _ = write!(stdout, "{}", logo_esc);
    // Flush so the terminal commits the logo placement before any subsequent
    // cursor manipulation can be (mis)interpreted as overlapping with it.
    let _ = stdout.flush();

    // --- Emit the wordmark ---
    // Restore to (top, 0), then walk right to the wordmark's start column.
    let _ = write!(stdout, "\x1b8");
    let _ = write!(stdout, "\x1b[{}C", left_margin + logo_w as u16 + gap);
    let _ = write!(stdout, "{}", wm_esc);

    // Two newlines = one blank separator row before the prompt.
    let _ = writeln!(stdout);
    let _ = writeln!(stdout);
    let _ = stdout.flush();
}

fn github_repos_from_remotes() -> Vec<String> {
    let output = std::process::Command::new("git")
        .args(["remote", "-v"])
        .output()
        .ok();
    let Some(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let mut repos = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Some(url) = line.split_whitespace().nth(1) else {
            continue;
        };
        if let Some(repo) = parse_github_repo(url) {
            if !repos.contains(&repo) {
                repos.push(repo);
            }
        }
    }
    repos
}

fn parse_github_repo(url: &str) -> Option<String> {
    let path = if let Some(rest) = url.strip_prefix("git@github.com:") {
        rest
    } else if let Some(rest) = url.strip_prefix("https://github.com/") {
        rest
    } else {
        url.strip_prefix("ssh://git@github.com/")?
    };
    let path = path.trim_end_matches(".git");
    let (owner, repo) = path.split_once('/')?;
    if owner.is_empty() || repo.is_empty() {
        None
    } else {
        Some(format!("{owner}/{repo}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_github_repo_from_common_remote_urls() {
        assert_eq!(
            parse_github_repo("git@github.com:owner/repo.git"),
            Some("owner/repo".to_string())
        );
        assert_eq!(
            parse_github_repo("https://github.com/org/repo"),
            Some("org/repo".to_string())
        );
        assert_eq!(
            parse_github_repo("ssh://git@github.com/user/repo.git"),
            Some("user/repo".to_string())
        );
        assert_eq!(parse_github_repo("https://example.com/user/repo"), None);
    }
}

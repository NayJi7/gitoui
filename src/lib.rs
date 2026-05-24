pub mod avatar;
pub mod color;
pub mod config;
pub mod git;
pub mod github;
pub mod github_auth;
pub mod graph;
pub mod highlight;
pub mod log;
pub mod protocol;
pub mod themes;

mod app;
mod brand;
mod check;
mod dir_input;
mod event;
mod external;
mod keybind;
mod panic_guard;
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

    /// Delete the on-disk commit cache for the current repository,
    /// then exit. Next launch re-walks `git log` from scratch and
    /// rebuilds the cache. Useful when the cache appears stale or
    /// corrupt, or to reclaim disk space.
    #[arg(short = 'C', long)]
    clear_cache: bool,
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
/// composes. Variants are deliberately coarse, finer-grained domain errors
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
    // Init the file logger BEFORE anything else so even early-bailout
    // paths (clap parse errors, missing repo splash) can leave a trail.
    #[cfg(debug_assertions)]
    log::init(log::Level::Debug);
    #[cfg(not(debug_assertions))]
    log::init(log::Level::Info);

    // Print the gitoui splash above clap's help / error output so the brand
    // shows on every entry path, not just the "no-repo" prompt. clap's
    // `parse()` would auto-exit before we get a chance to draw, so we use
    // `try_parse()` and re-emit the formatted message ourselves.
    let args = match Args::try_parse() {
        Ok(args) => args,
        Err(err) => {
            // clap separates "expected" exits (--help / --version) from real
            // failures via `ErrorKind`. We mirror clap's behaviour: success
            // exit on help/version, non-zero on parse errors. Using `print`
            // (stdout) for help/version and `eprint` (stderr) for errors
            // matches what `parse()` would have done.
            use clap::error::ErrorKind;
            // Splash on every entry path EXCEPT --version: that one is
            // meant to be parsed by scripts (install.sh, package
            // managers, …) and a multi-line image-protocol splash
            // bleeds into the captured output.
            if err.kind() != ErrorKind::DisplayVersion {
                let proto = protocol::auto_detect();
                print_no_repo_splash(proto);
            }
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

    // `--update` is the explicit escape hatch, always prompts, ignores
    // the cache + the "never_ask" preference, and exits afterwards.
    // Splash is shown above the check just like the --help / --version
    // paths so the brand surfaces on every entry point.
    if args.update {
        let proto = protocol::auto_detect();
        update::force_check(|| print_no_repo_splash(proto));
        return Ok(());
    }

    // `--clear-cache` / `-C`: one-shot cache wipe for the current
    // repository, then exit. Resolves the repo root first so the
    // command works from any subdirectory (matches `git`'s own
    // behaviour). On a non-repo dir the cache key wouldn't have
    // existed anyway, so reporting "not in a repo" and exiting is
    // the cleanest UX.
    if args.clear_cache {
        let repo_root = git::find_repo_root(Path::new("."));
        match repo_root {
            Some(root) => {
                git::cache::invalidate(&root);
                println!("Cleared gitoui commit cache for {}", root.display());
            }
            None => {
                eprintln!("Not inside a git repository - nothing to clear.");
                std::process::exit(1);
            }
        }
        return Ok(());
    }
    // Otherwise, do the silent once-a-day check + prompt. The function
    // bails out fast (no network) when the cache is fresh, the user has
    // chosen "never", or we're not attached to a TTY. On `Updated` it
    // already exec'd the new binary and never returns. Splash only
    // prints if we're actually about to prompt, silent paths stay
    // silent.
    if !args.no_update_check {
        let proto = protocol::auto_detect();
        let _ = update::maybe_check_at_startup(|| print_no_repo_splash(proto));
    }

    // Validate the user config FIRST, before we pay the cost of
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

    // Warm up syntect (syntax definitions + themes) on a background
    // thread. The Lazy statics init on first access; triggering them
    // here means the ~200-500 ms cost is paid in parallel with the
    // main thread's Repository::load + calc_graph, so by the time the
    // user opens Config or a diff view, syntect is already loaded.
    highlight::warm_up_in_background();
    let ec = event::EventController::init();
    let mut refresh_view_context = None;
    let mut terminal = None;
    // Latch the huge-repo auto-disable to FIRST launch only. Without
    // this, every subsequent Refresh iteration (close Config view,
    // etc.) would re-force `ui_config.list.graph_enabled = false`
    // and silently undo a user who explicitly toggled the graph
    // back on via the Config view. Once we've auto-disabled, we
    // step out of the way and let the saved config drive.
    // Track which repo paths we've already auto-disabled the graph
    // for in this session. Indexed by canonical path so cd-ing into a
    // different huge repo triggers a fresh auto-disable (the user's
    // expectation: "every huge repo I visit gets graph turned off
    // automatically the first time, but I can toggle it back on per
    // repo if I want"). Without this per-path tracking, a single
    // process-wide latch meant: visit huge repo A -> disabled, cd to
    // small repo B -> no change, cd to huge repo C -> NOT disabled
    // (latch already set from A). And worse, a small repo opened
    // after a huge one would skip the disable check entirely instead
    // of restoring the user's saved graph-enabled choice.
    let mut huge_repo_autodisabled_paths: rustc_hash::FxHashSet<std::path::PathBuf> =
        rustc_hash::FxHashSet::default();
    // Filesystem watcher on .git/, keeps the UI in sync with external git
    // operations (commits from another shell, push/pull/fetch, branch
    // switches, …). Held alive for the whole `run()` lifetime; dropping it
    // stops the watcher thread. We track the watched path so a `cd` into
    // a different repo can rebuild the watcher onto the new `.git/`
    // otherwise the old watcher keeps firing for the *previous* repo and
    // gitoui takes those phantom events as a reason to full-refresh.
    let mut _git_watcher: Option<_> = None;
    let mut watched_git_dir: Option<std::path::PathBuf> = None;
    // Identity of the bg stream we last spawned. Compared against
    // the current iteration's identity, equal -> already streamed,
    // skip; different (or None) -> spawn a fresh bg walker AND reset
    // `bg_load_finished` so the gate / spinner re-arm correctly.
    //
    // We key on (path, sort, head_hash) so EVERY trigger that
    // changes what should be on screen forces a re-stream:
    //
    //   - path differs -> cd into another repo.
    //   - sort differs -> Config view changed chrono <-> topo.
    //   - head_hash differs -> external git activity bumped HEAD
    //     (commit / amend / fetch / pull / push / branch checkout
    //     picked up by the watcher OR by a manual `r` reload).
    //
    // Previously this was a single `PathBuf` so a same-repo Refresh
    // (sort change, manual reload, watcher refresh after external
    // commit) would skip the bg spawn and the list would be stuck on
    // the fg-loaded 500-commit slice with the wrong order or missing
    // the externally-added commits.
    #[derive(Clone, PartialEq, Eq)]
    struct BgStreamKey {
        path: std::path::PathBuf,
        sort: git::SortCommit,
        head_hash: Option<String>,
    }
    let bg_load_started_for: std::sync::Arc<std::sync::Mutex<Option<BgStreamKey>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    // Shared latch flipped to true by the bg streaming thread's
    // RAII guard when it exits (success OR panic). Used to seed
    // `bg_full_load_in_progress` on every iteration: the flag must
    // NOT spuriously re-arm to "loading" after the bg thread
    // already finished, otherwise the header logo loops forever on
    // every config refresh. Reset to false whenever we spawn a new
    // bg thread (cd to a different repo).
    let bg_load_finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // Monotonically increasing generation counter for the bg streaming
    // thread. Incremented on every bg spawn so the main thread can
    // discard stale AppendCommits / ReplaceGraph / BackgroundCacheReady
    // events from a previous generation that are still in the channel
    // when a new bg run starts (e.g. after a config refresh or cd).
    let mut bg_generation: u64 = 0;

    let ret = loop {
        // Detect any change between iterations that means the bg
        // stream we last produced no longer matches what should be
        // on screen: path differs (cd into another repo), the
        // sort order can't be checked yet (config not loaded), but
        // the live HEAD hash is cheap to query - if it moved, we
        // know the watcher / manual reload was right to refresh.
        //
        // Sort + final identity check happen lower down once
        // `order` is resolved; this top-of-loop pre-reset only
        // exists to seed `bg_full_load_in_progress = true` BEFORE
        // AppContext::new so the header spinner re-arms instantly
        // on a same-repo HEAD-moved refresh. A second reset at the
        // spawn site handles sort changes (which the user can only
        // request via Config view that closed and triggered this
        // very Refresh, so they always lead to a full key change).
        if let Some(probe) = git::find_repo_root(Path::new(".")) {
            let probe_canonical = probe.canonicalize().unwrap_or(probe);
            let live_head = git::cache::current_head_hash(&probe_canonical);
            let needs_reset = {
                let guard = bg_load_started_for.lock().unwrap();
                match guard.as_ref() {
                    None => true,
                    Some(key) => key.path != probe_canonical || key.head_hash != live_head,
                }
            };
            if needs_reset {
                bg_load_finished.store(false, std::sync::atomic::Ordering::Release);
            }
        }

        // First iteration uses the diagnostic-bearing loader so any
        // failure renders as a styled --help-style error block and
        // exits cleanly. Subsequent reloads (config reopened from
        // inside the app) go through the legacy `config::load()` and
        // log the error to stderr, the running session shouldn't
        // crash because the user typo'd a hex code mid-edit.
        let (mut core_config, ui_config, mut graph_config, mut color_theme, keybind_patch) =
            if let Some(c) = preflight_config.take() {
                // First iteration consumes the pre-flight result that we
                // computed *before* paying for syntect / event thread.
                c
            } else if terminal.is_none() {
                // Pre-flight got consumed by an earlier iteration that
                // bailed (`Ret::Refresh` re-enters this loop, etc.)
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
        // a config-reload mid-session, fall back silently in that case.
        if !core_config.option.theme.is_empty() {
            if let Ok(def) = crate::themes::resolve_or_load(&core_config.option.theme) {
                color_theme = def.color_theme;
                core_config.option.syntax_theme = def.syntax_theme;
                // Theme-tinted graph palette wins over the generic config default
                //, but only if the theme actually shipped one. An empty Vec means
                // "the theme didn't customize the graph", so we leave the user's
                // `[graph.color.branches]` setting alone.
                if !color_theme.graph_branches.is_empty() {
                    graph_config.color.branches = color_theme.graph_branches.clone();
                }
            }
        }
        let keybind = keybind::KeyBinds::new(keybind_patch);

        // Load policy: an explicit CLI `-n` always wins. Otherwise
        // cap the foreground walk to a small fixed number so the
        // launch stays under ~100 ms even on 332k-commit repos.
        // The COMPLETE history is loaded in the background thread
        // spawned a few lines below and silently swapped in as soon
        // as it's ready - the user starts scrolling immediately on
        // the recent slice and gets the full set without ever
        // hitting a blocking `git log` walk.
        const FG_INITIAL_LOAD: usize = 500;
        let max_count = args.max_count.or(Some(FG_INITIAL_LOAD));
        let image_protocol = args.protocol.or(core_config.option.protocol).into();
        let order = args.order.or(core_config.option.order).into();

        // Second pre-reset gate: the top-of-loop check couldn't see
        // the sort order (config wasn't loaded yet). If the new sort
        // differs from the last bg-stream, reset `bg_load_finished`
        // NOW so the App's `bg_full_load_in_progress` (seeded a few
        // lines below from this latch) starts true and the header
        // spinner animates while bg re-streams in the new order.
        // Without this, a Config-view sort flip would silently
        // re-spawn bg but the spinner would stay off because the
        // latch was still `true` from the previous (now-stale)
        // finish.
        {
            let guard = bg_load_started_for.lock().unwrap();
            let sort_changed = guard.as_ref().is_some_and(|k| k.sort != order);
            drop(guard);
            if sort_changed {
                bg_load_finished.store(false, std::sync::atomic::Ordering::Release);
            }
        }
        let graph_width = args.graph_width.or(core_config.option.graph_width);
        let graph_style = args.graph_style.or(core_config.option.graph_style).into();
        let graph_image_width_mode = graph_config.row_image_width;
        let initial_selection = args
            .initial_selection
            .or(core_config.option.initial_selection)
            .into();

        // Centralised graph-palette construction, see `build_graph_color_set`
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
            // Filled after the repository is loaded, see below.
            current_branch_remote_state: None,
            repo_path: std::path::PathBuf::new(),
            graph_huge_repo_warning: false,
            // True from launch until the bg streaming thread's
            // RAII guard fires. Drives the header logo animation
            // and the 50 ms wakeup tick. On every iteration we
            // seed it from the persistent `bg_load_finished`
            // latch: if bg already finished in a prior iteration,
            // the flag starts false (no animation); otherwise
            // true (still loading).
            bg_full_load_in_progress: std::sync::atomic::AtomicBool::new(
                !bg_load_finished.load(std::sync::atomic::Ordering::Acquire),
            ),
        });

        // If we were launched from a sub-directory of a repo, jump up to
        // the work-tree root so the header pwd, all `git` invocations,
        // and the FS watcher anchor at the same place, the repo, not
        // wherever the shell happened to be when the user typed
        // `gitoui`. Failures here are harmless: when the cwd isn't in a
        // repo at all the next `Repository::load(".", ...)` returns the
        // no-repo error and the splash takes over.
        if let Some(root) = git::find_repo_root(Path::new(".")) {
            let _ = std::env::set_current_dir(&root);
        }
        // Foreground load: `Repository::load(., max_count)` walks
        // `git log --max-count=N` only. Stays UI-snappy on huge
        // repos because the walk is bounded. The bg cache writer
        // below runs in a separate OS thread and never touches the
        // main thread - it just persists the full history to disk
        // for future use.
        // Bg streaming via AppendCommits now feeds the live list
        // directly; no Repository swap to consume.
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

        // Background full-load thread. Walks the FULL `git log`
        // (or reads the disk cache) on its own OS thread, then
        // ships the hydrated Repository back via the channel for
        // a silent swap. Spawned at most once per session: the
        // atomic latch flips true on first call and every Refresh
        // iteration after that sees it true and skips.
        // Background streaming loader. Walks the FULL `git log` (or
        // reads the disk cache) on its own OS thread, builds
        // CommitInfo entries for every commit BEYOND the fg-loaded
        // initial slice, and ships them back in small batches via
        // `AppendCommits` events. The main thread appends each
        // batch into the live `CommitListState` without ANY rebuild
        // - cursor / scroll position / search state survive
        // untouched. Spawned at most once per session.
        // Check if bg streaming already ran for THIS specific
        // (repo, sort, HEAD) combination. The key extends the
        // previous path-only check so EVERY change that should
        // re-stream actually re-streams:
        //
        //   - Path differs -> cd into another repo.
        //   - Sort differs -> Config view changed chrono <-> topo;
        //     the on-disk cache + the previous bg stream are in the
        //     OLD order, so we have to re-walk + re-emit.
        //   - HEAD hash differs -> external git activity bumped HEAD
        //     (commit / amend / fetch / pull / push / branch
        //     checkout picked up by the watcher OR `r` reload). The
        //     prev stream missed any commits past the captured
        //     fg slice's tip.
        //
        // When the key matches, we trust the previous stream is
        // still valid (theme change, config setting that doesn't
        // affect the commit set, etc.) and skip the spawn - the
        // user won't see any flicker on those refreshes.
        //
        // Compared against the canonicalised form so symlinked
        // aliases of the same repo don't double-trigger.
        // Always re-spawn bg on every iteration. The previous
        // "skip if (path, sort, HEAD) unchanged" gate was correct
        // for the bg-streaming work itself but missed a subtler
        // problem: every `Ret::Refresh` (Config close, theme
        // change, search apply, etc.) BUILDS A FRESH App with a
        // new empty CommitListState. The previous App's
        // bg-streamed commits live in the OLD CommitListState which
        // is now dropped. If we skipped the bg spawn here, the new
        // App would render only its fg-loaded ~500 commits forever
        // (user reported: "j'active le graph et je peux pas scroll
        // plus bas que le commit 500" after a Config close).
        //
        // Cache hit makes re-spawn cheap: ratatui (~2.5k) re-streams
        // in <100 ms, gitoui (~530) in <50 ms. Even rust (~332k)
        // is ~5 s of bg work that runs while the user is back on
        // the list - they see the "..." indicator and the count
        // tick up again. Acceptable for the rare case (config
        // change, etc.).
        //
        // BgStreamKey is kept for the spinner re-arm logic (the
        // top-of-loop pre-reset of `bg_load_finished` still keys
        // on path/HEAD diff). The "skip spawn when key unchanged"
        // arm of the gate is removed; we always spawn.
        let current_repo_path = repository
            .path()
            .canonicalize()
            .unwrap_or_else(|_| repository.path().to_path_buf());
        let current_head_hash = git::cache::current_head_hash(&current_repo_path);
        let new_key = BgStreamKey {
            path: current_repo_path.clone(),
            sort: order,
            head_hash: current_head_hash,
        };
        {
            let mut guard = bg_load_started_for.lock().unwrap();
            *guard = Some(new_key.clone());
            // Reset finished latch so the spinner re-arms on every
            // bg respawn.
            bg_load_finished.store(false, std::sync::atomic::Ordering::Release);
        }
        bg_generation += 1;
        let bg_gen = bg_generation;
        {
            let repo_path_for_bg = repository.path().to_path_buf();
            let head_for_bg = repository.head().clone();
            let stashes_for_bg = repository.stashes();
            let sender = ec.sender();
            // Snapshot of what main's fg load already has - skip those
            // in the stream so we don't double-display.
            let fg_loaded: rustc_hash::FxHashSet<git::CommitHash> = repository
                .all_commits()
                .iter()
                .map(|c| c.commit_hash.clone())
                .collect();
            let graph_color_set_for_bg = graph_color_set.clone();
            // Honour the user's graph-on/off preference at stream
            // start. When the graph column is disabled, the
            // `ReplaceGraph` payload at the end is pure waste (it
            // allocates the full vertex / branch topology nobody
            // ever reads) and on big repos causes a visible
            // re-render flicker. Pre-snapshot the flag so the bg
            // thread doesn't have to reach back into AppContext.
            let graph_enabled_for_bg = ctx.ui_config.list.graph_enabled;
            let bg_load_finished_for_bg = bg_load_finished.clone();
            std::thread::Builder::new()
                .name("gitoui-bg-stream".into())
                .spawn(move || {
                    // RAII guard: ALWAYS sends `BackgroundCacheReady`
                    // when the thread exits, regardless of which
                    // path drops us out of this closure (normal
                    // completion, early return from a stream open
                    // failure, or a panic). Without this, any
                    // failure in the bg pipeline silently left the
                    // header logo spinning forever - the user saw
                    // "le G ne s'arrete jamais de load" because the
                    // single flag-clearing event never fired.
                    struct BgDoneGuard {
                        sender: crate::event::Sender,
                        total: std::cell::Cell<usize>,
                        finished: std::sync::Arc<std::sync::atomic::AtomicBool>,
                        gen: u64,
                    }
                    impl Drop for BgDoneGuard {
                        fn drop(&mut self) {
                            self.finished
                                .store(true, std::sync::atomic::Ordering::Release);
                            self.sender.send(event::AppEvent::BackgroundCacheReady {
                                gen: self.gen,
                                total_commits: self.total.get(),
                            });
                        }
                    }
                    let _bg_done = BgDoneGuard {
                        sender: sender.clone(),
                        total: std::cell::Cell::new(0),
                        finished: bg_load_finished_for_bg,
                        gen: bg_gen,
                    };

                    // STEP 1: load refs + stashes for color assignment.
                    // We bypass the full Repository hydrate here - it
                    // would allocate parents_map + children_map +
                    // commit_map on every single commit (the OOM
                    // cause on rust-lang/rust at ~332k commits).
                    let (mut ref_map, _head) = git::load_refs(&repo_path_for_bg);
                    let stash_ref_map = git::load_stashes_as_refs(&repo_path_for_bg);
                    git::merge_ref_maps(&mut ref_map, stash_ref_map);

                    // Build branch_color_for from the same stable
                    // ordering used by `calc_colors_only` so the
                    // colours streamed here match what the final
                    // calc_graph would assign at the fg<->bg
                    // boundary (HEAD's branch first -> slot 0, then
                    // every other ref alphabetically).
                    let head_branch_name = match &head_for_bg {
                        git::Head::Branch { name } => Some(name.clone()),
                        _ => None,
                    };
                    let mut branch_tips: Vec<(String, git::CommitHash)> = Vec::new();
                    for refs in ref_map.values() {
                        for r in refs {
                            match r {
                                git::Ref::Branch { name, target }
                                | git::Ref::RemoteBranch { name, target } => {
                                    branch_tips.push((name.clone(), target.clone()));
                                }
                                _ => {}
                            }
                        }
                    }
                    branch_tips.sort_by(|a, b| {
                        let a_is_head = head_branch_name.as_ref() == Some(&a.0);
                        let b_is_head = head_branch_name.as_ref() == Some(&b.0);
                        b_is_head.cmp(&a_is_head).then_with(|| a.0.cmp(&b.0))
                    });
                    let mut branch_color_for: rustc_hash::FxHashMap<git::CommitHash, usize> =
                        rustc_hash::FxHashMap::default();
                    for (idx, (_, tip)) in branch_tips.iter().enumerate() {
                        // Don't overwrite - first branch claim wins
                        // the colour for a tip-shared commit.
                        branch_color_for.entry(tip.clone()).or_insert(idx);
                    }
                    let next_fresh_color = std::cell::Cell::new(branch_tips.len());

                    // Pre-index stashes by their first-parent so we
                    // can splice them into the commit stream the
                    // same way the fg Repository::load path does
                    // via `merge_stashes_to_summaries`. Without this,
                    // the bg stream would silently drop stash entries
                    // and the user would see them only on the
                    // fg-loaded initial slice.
                    let mut stash_by_parent: rustc_hash::FxHashMap<
                        git::CommitHash,
                        Vec<git::CommitSummary>,
                    > = rustc_hash::FxHashMap::default();
                    for stash in &stashes_for_bg {
                        if let Some(parent) = stash.parent_commit_hashes.first() {
                            stash_by_parent
                                .entry(parent.clone())
                                .or_default()
                                .push(stash.clone());
                        }
                    }

                    // STEP 2: open the cache stream. If we have a
                    // useful surgical-extend (HEAD moved forward),
                    // rewrite the cache first so the StreamReader
                    // below picks up the prepended commits in one
                    // go. The extend rewrites in streaming fashion
                    // (header + new commits + existing commits via
                    // StreamReader), no big Vec ever held.
                    let _ = git::cache::try_extend(&repo_path_for_bg, order);

                    let stream_reader_opt = git::cache::open_stream_for(&repo_path_for_bg, order);

                    // Shared bg-stream state. `inherited_color` is
                    // the bulk allocation here - ~332k entries x
                    // ~50 bytes = ~16 MB on rust-lang/rust, which
                    // is the new ceiling for bg memory peak.
                    let mut inherited_color: rustc_hash::FxHashMap<git::CommitHash, usize> =
                        rustc_hash::FxHashMap::default();
                    // Pending = commits whose colour we couldn't
                    // resolve yet because their child hasn't been
                    // seen. With `--date-order` on heavily-rebased
                    // repos (commit dates rewritten by rebases /
                    // cherry-picks), a parent can show up in the
                    // stream BEFORE its child - so pending isn't
                    // tiny on those repos (~40k on rust-lang/rust).
                    //
                    // Two structures: `pending_map` for O(1) lookup
                    // when a child resolves and wants to unblock its
                    // parent, and `pending_order` preserving insertion
                    // FIFO so the STEP 4 cleanup walks any true
                    // orphans in a stable order (deterministic colour
                    // assignment when we fall back to fresh slots).
                    // A pending hash gets removed from `pending_map`
                    // the moment it's emitted via the cascade; STEP 4
                    // skips any `pending_order` entry already gone
                    // from the map.
                    let mut pending_map: rustc_hash::FxHashMap<
                        git::CommitHash,
                        git::CommitSummary,
                    > = rustc_hash::FxHashMap::default();
                    let mut pending_order: std::collections::VecDeque<git::CommitHash> =
                        std::collections::VecDeque::new();

                    const BATCH_SIZE: usize = 500;
                    let mut batch: Vec<crate::widget::commit_list::CommitInfo> =
                        Vec::with_capacity(BATCH_SIZE);
                    let mut total_streamed: usize = 0;

                    // Color resolution helper. Order of precedence:
                    //   1. branch_color_for (commit is a branch tip)
                    //   2. inherited_color (a child has already
                    //      propagated colour to this commit)
                    //   3. None (caller pushes to pending)
                    fn resolve_color(
                        hash: &git::CommitHash,
                        branch_color_for: &rustc_hash::FxHashMap<git::CommitHash, usize>,
                        inherited_color: &rustc_hash::FxHashMap<git::CommitHash, usize>,
                    ) -> Option<usize> {
                        if let Some(&c) = branch_color_for.get(hash) {
                            return Some(c);
                        }
                        inherited_color.get(hash).copied()
                    }

                    let emit_commit =
                        |commit: git::CommitSummary,
                         color_index: usize,
                         ref_map: &git::RefMap,
                         batch: &mut Vec<crate::widget::commit_list::CommitInfo>,
                         total_streamed: &mut usize,
                         sender: &crate::event::Sender,
                         fg_loaded: &rustc_hash::FxHashSet<git::CommitHash>| {
                            if fg_loaded.contains(&commit.commit_hash) {
                                return;
                            }
                            let refs: Vec<git::Ref> = ref_map
                                .get(&commit.commit_hash)
                                .cloned()
                                .unwrap_or_default();
                            let color = graph_color_set_for_bg
                                .get(color_index)
                                .to_ratatui_color();
                            batch.push(crate::widget::commit_list::CommitInfo::new(
                                std::sync::Arc::new(commit),
                                refs,
                                color,
                            ));
                            *total_streamed += 1;
                            if batch.len() >= BATCH_SIZE {
                                sender.send(event::AppEvent::AppendCommits {
                                    gen: bg_gen,
                                    batch: std::mem::take(batch),
                                });
                                // No artificial sleep: the foreground
                                // dispatch loop already throttles via
                                // APPEND_YIELD_THRESHOLD (yields to
                                // render every ~1500 commits applied),
                                // which provides the smooth-streaming
                                // pacing. Sleeping here would add
                                // ~3-5s of pure-wait to a fresh-walk
                                // of rust-lang/rust (664 batches × 5ms)
                                // on top of the actual streaming work.
                            }
                        };

                    // Helper that takes a freshly-decoded commit,
                    // resolves its colour (or defers to pending),
                    // and on resolve also propagates colour to
                    // parents + cascades any pending parents that
                    // can now resolve. Pulls out into a closure so
                    // both the cache and the fresh-walk paths can
                    // share it.
                    //
                    // Cascade model: when we propagate a colour to
                    // a parent hash, we ALSO check whether that hash
                    // is sitting in `pending_map`. If so, emit it
                    // immediately and recurse (its parents may unblock
                    // further commits). Done via a work-stack to keep
                    // the recursion explicit + bounded by stack depth
                    // of the actual dependency chain (not the call
                    // stack, which would overflow on long pending
                    // chains on huge repos).
                    let mut process_commit = |commit: git::CommitSummary,
                                              branch_color_for: &rustc_hash::FxHashMap<
                        git::CommitHash,
                        usize,
                    >,
                                              inherited_color: &mut rustc_hash::FxHashMap<
                        git::CommitHash,
                        usize,
                    >,
                                              pending_map: &mut rustc_hash::FxHashMap<
                        git::CommitHash,
                        git::CommitSummary,
                    >,
                                              _pending_order: &mut std::collections::VecDeque<
                        git::CommitHash,
                    >,
                                              ref_map: &git::RefMap,
                                              batch: &mut Vec<
                        crate::widget::commit_list::CommitInfo,
                    >,
                                              total_streamed: &mut usize,
                                              sender: &crate::event::Sender,
                                              fg_loaded: &rustc_hash::FxHashSet<
                        git::CommitHash,
                    >| {
                        let hash = commit.commit_hash.clone();
                        // STREAM-ORDER GUARANTEE: never defer. With
                        // `--date-order` on rebase-heavy repos like
                        // rust-lang/rust, parents can appear before
                        // their children in git log output. Deferring
                        // such commits to a `pending` queue meant they
                        // emitted out of stream order (cascade or
                        // STEP 4 timing), which left the init commit
                        // somewhere mid-list instead of at the
                        // expected bottom of the list. Strict
                        // stream-order requires emitting every commit
                        // at its arrival turn; if its colour can't be
                        // inherited yet, allocate a fresh slot from
                        // the same pool used for legitimate new
                        // branches. The colour is "approximate"
                        // (doesn't match the topology branch the
                        // commit truly belongs to) but the LIST ORDER
                        // matches git log exactly - Shift+G then
                        // lands on the real oldest commit.
                        let color_index =
                            match resolve_color(&hash, branch_color_for, inherited_color) {
                                Some(c) => c,
                                None => {
                                    let n = next_fresh_color.get();
                                    next_fresh_color.set(n + 1);
                                    n
                                }
                            };
                        // pending_map / pending_order intentionally
                        // unused on the happy path - kept in the
                        // closure signature for the cascade hook
                        // (cascade still removes any incidental
                        // pending entries, see below).
                        // Emit + cascade. The work-stack lists
                        // (commit, color) pairs ready to be emitted.
                        // Each pop emits the commit, propagates its
                        // colour to parents (allocating fresh slots
                        // for extra merge parents), and pushes any
                        // parent that was sitting in `pending_map`
                        // back onto the stack so it emits before we
                        // return.
                        let mut to_emit: Vec<(git::CommitSummary, usize)> =
                            vec![(commit, color_index)];
                        while let Some((c, color)) = to_emit.pop() {
                            let chash = c.commit_hash.clone();
                            let parents = c.parent_commit_hashes.clone();
                            // Propagate colour to parents BEFORE
                            // emitting so any pending parent cascade
                            // picks up the right value.
                            if let Some(first_parent) = parents.first() {
                                inherited_color.entry(first_parent.clone()).or_insert(color);
                            }
                            for extra_parent in parents.iter().skip(1) {
                                inherited_color
                                    .entry(extra_parent.clone())
                                    .or_insert_with(|| {
                                        let n = next_fresh_color.get();
                                        next_fresh_color.set(n + 1);
                                        n
                                    });
                            }
                            // Emit the commit.
                            emit_commit(
                                c,
                                color,
                                ref_map,
                                batch,
                                total_streamed,
                                sender,
                                fg_loaded,
                            );
                            // Splice in stashes for this commit.
                            if let Some(stashes) = stash_by_parent.remove(&chash) {
                                for stash in stashes {
                                    emit_commit(
                                        stash,
                                        color,
                                        ref_map,
                                        batch,
                                        total_streamed,
                                        sender,
                                        fg_loaded,
                                    );
                                }
                            }
                            // Cascade: if any parent was pending,
                            // it's now resolvable - pop it from the
                            // map and queue it for emission. Push in
                            // reverse so first-parent emits first
                            // (stack pops LIFO).
                            //
                            // Re-resolve each parent's colour from
                            // inherited_color rather than reusing
                            // `color` directly - for extra parents
                            // we just inserted a fresh slot, and
                            // `branch_color_for` may also override.
                            for parent_hash in parents.iter().rev() {
                                if let Some(pcommit) = pending_map.remove(parent_hash) {
                                    let pcolor = resolve_color(
                                        parent_hash,
                                        branch_color_for,
                                        inherited_color,
                                    )
                                    .unwrap_or_else(|| {
                                        // Shouldn't happen: we just
                                        // populated `inherited_color`
                                        // for every parent above. But
                                        // belt-and-braces: allocate a
                                        // fresh slot rather than
                                        // panic.
                                        let n = next_fresh_color.get();
                                        next_fresh_color.set(n + 1);
                                        n
                                    });
                                    to_emit.push((pcommit, pcolor));
                                }
                            }
                        }
                    };

                    // STEP 3: stream the commits. Cache path first,
                    // then the fresh-walk fallback (which doubles
                    // as the cache writer).
                    if let Some(mut reader) = stream_reader_opt {
                        loop {
                            match reader.next_commit() {
                                Some(Ok(c)) => {
                                    process_commit(
                                        c,
                                        &branch_color_for,
                                        &mut inherited_color,
                                        &mut pending_map,
                                        &mut pending_order,
                                        &ref_map,
                                        &mut batch,
                                        &mut total_streamed,
                                        &sender,
                                        &fg_loaded,
                                    );
                                }
                                Some(Err(e)) => {
                                    glog_warn!("bg-stream cache read error (stopping): {}", e);
                                    break;
                                }
                                None => break,
                            }
                        }
                    } else {
                        // Fresh walk: stream commits one at a time
                        // from `git log`, feeding each to the colour
                        // pipeline AND to a dedicated cache-writer
                        // OS thread via a bounded mpsc channel. The
                        // disk write is fully decoupled from the
                        // hot path so a slow filesystem can no
                        // longer add latency to the user-visible
                        // streaming. Memory peak stays bounded - no
                        // Vec<Commit> ever materialises, and the
                        // bounded channel (5000 slots, ~2.5 MB
                        // worst-case) caps RAM growth: if the disk
                        // can't keep up, the bg streamer back-
                        // pressures naturally into the git log
                        // subprocess instead of unbounded buffering.
                        let head_hash = git::cache::current_head_hash(&repo_path_for_bg);
                        let reachable =
                            git::cache::reachable_commit_count(&repo_path_for_bg).unwrap_or(0);
                        let cache_path = git::cache::cache_path_for(&repo_path_for_bg);

                        // Bounded sender to the cache-writer thread.
                        // None when the writer couldn't be created
                        // (no HEAD, no cache path, IO failure on
                        // create). Cleared on any send error so a
                        // dead consumer doesn't keep paying clone+
                        // send cost per commit.
                        let mut cache_tx: Option<std::sync::mpsc::SyncSender<git::CommitSummary>> =
                            None;
                        // Join handle so we can wait for the writer
                        // to drain + finalise (atomic rename) before
                        // the post-walk steps re-open the cache.
                        let mut cache_writer_handle: Option<std::thread::JoinHandle<()>> = None;

                        if let (Some(head), Some(path)) = (head_hash, cache_path) {
                            let header = git::cache::CacheHeader {
                                magic: *b"GTOI",
                                version: 4,
                                head_hash: head,
                                reachable_count: reachable,
                                sort_order: match order {
                                    git::SortCommit::Chronological => 0,
                                    git::SortCommit::Topological => 1,
                                },
                            };
                            match git::cache::StreamWriter::create(&path, header) {
                                Ok(mut w) => {
                                    let (tx, rx) =
                                        std::sync::mpsc::sync_channel::<git::CommitSummary>(5000);
                                    let handle = panic_guard::spawn_protected(
                                        "bg-cache-writer",
                                        move || {
                                            // Drain commits until the
                                            // sender drops. Any
                                            // mid-stream write error
                                            // aborts the loop - we
                                            // skip finish() so the
                                            // .tmp file (created by
                                            // StreamWriter::create)
                                            // is left orphaned and
                                            // never confused for a
                                            // valid cache. Atomicity
                                            // comes for free from the
                                            // existing tmp+rename in
                                            // StreamWriter.
                                            let mut had_error = false;
                                            while let Ok(commit) = rx.recv() {
                                                if let Err(e) = w.write_commit(&commit) {
                                                    glog_warn!(
                                                        "cache write failed mid-stream: {}",
                                                        e
                                                    );
                                                    had_error = true;
                                                    break;
                                                }
                                            }
                                            // Drop the receiver
                                            // explicitly before
                                            // calling finish() so the
                                            // bg streamer sees a
                                            // closed channel (in case
                                            // of write error) and
                                            // stops cloning commits.
                                            drop(rx);
                                            if had_error {
                                                glog_warn!(
                                                    "bg-stream: cache write aborted; \
                                                     .tmp left orphaned (will be \
                                                     ignored on next launch)"
                                                );
                                                return;
                                            }
                                            match w.finish() {
                                                Ok(_) => glog_info!(
                                                    "bg-stream: cache rebuilt \
                                                     during fresh walk"
                                                ),
                                                Err(e) => glog_warn!(
                                                    "cache writer finish \
                                                     failed: {}",
                                                    e
                                                ),
                                            }
                                        },
                                    );
                                    cache_tx = Some(tx);
                                    cache_writer_handle = Some(handle);
                                }
                                Err(e) => {
                                    glog_warn!("cache writer create failed: {}", e);
                                }
                            }
                        }

                        git::stream_commits_one_by_one(
                            &repo_path_for_bg,
                            order,
                            &head_for_bg,
                            &stashes_for_bg,
                            |commit| {
                                // Ship to the cache-writer thread
                                // FIRST so the clone + send is
                                // overlapped with the UI-side
                                // processing on multi-core. Send
                                // is bounded (sync_channel(5000));
                                // a slow disk back-pressures here
                                // and naturally throttles the git
                                // log subprocess via the closure's
                                // blocking semantics.
                                let drop_tx = if let Some(tx) = cache_tx.as_ref() {
                                    match tx.send(commit.clone()) {
                                        Ok(()) => false,
                                        Err(_) => {
                                            glog_warn!(
                                                "bg-stream: cache-writer thread \
                                                 dropped channel; skipping \
                                                 further cache writes"
                                            );
                                            true
                                        }
                                    }
                                } else {
                                    false
                                };
                                if drop_tx {
                                    cache_tx = None;
                                }
                                process_commit(
                                    commit,
                                    &branch_color_for,
                                    &mut inherited_color,
                                    &mut pending_map,
                                    &mut pending_order,
                                    &ref_map,
                                    &mut batch,
                                    &mut total_streamed,
                                    &sender,
                                    &fg_loaded,
                                );
                                true
                            },
                        );
                        // Drop the sender so the cache writer's
                        // recv loop returns Err, drains, and runs
                        // finish() (which performs the atomic
                        // tmp -> final rename).
                        drop(cache_tx);
                        // Wait for the writer to land the file
                        // before we fall through to the "rehydrate
                        // for ReplaceGraph" step, which re-opens
                        // the cache via StreamReader::open. Without
                        // this join, we'd race the rename.
                        if let Some(handle) = cache_writer_handle {
                            let _ = handle.join();
                        }
                    }

                    // STEP 4: drain any pending commits left over
                    // (true orphans whose child was never seen, e.g.
                    // unreachable / dangling ancestors slipping in
                    // via stash refs). Walk in original insertion
                    // order. Skip any hash already drained via the
                    // cascade (i.e. removed from `pending_map`
                    // while still listed in `pending_order`). For
                    // each surviving entry, prefer the resolved
                    // colour from `branch_color_for` /
                    // `inherited_color` (a previous STEP 4 emit may
                    // have just propagated one); fall back to a
                    // fresh colour slot only when truly orphaned.
                    // After each emit, cascade any newly-resolvable
                    // parents from `pending_map` the same way
                    // process_commit does.
                    while let Some(h) = pending_order.pop_front() {
                        let Some(commit) = pending_map.remove(&h) else {
                            continue;
                        };
                        let color_index = resolve_color(&h, &branch_color_for, &inherited_color)
                            .unwrap_or_else(|| {
                                let n = next_fresh_color.get();
                                next_fresh_color.set(n + 1);
                                n
                            });
                        let mut to_emit: Vec<(git::CommitSummary, usize)> =
                            vec![(commit, color_index)];
                        while let Some((c, color)) = to_emit.pop() {
                            let chash = c.commit_hash.clone();
                            let parents = c.parent_commit_hashes.clone();
                            if let Some(first_parent) = parents.first() {
                                inherited_color.entry(first_parent.clone()).or_insert(color);
                            }
                            for extra_parent in parents.iter().skip(1) {
                                inherited_color
                                    .entry(extra_parent.clone())
                                    .or_insert_with(|| {
                                        let n = next_fresh_color.get();
                                        next_fresh_color.set(n + 1);
                                        n
                                    });
                            }
                            emit_commit(
                                c,
                                color,
                                &ref_map,
                                &mut batch,
                                &mut total_streamed,
                                &sender,
                                &fg_loaded,
                            );
                            if let Some(stashes) = stash_by_parent.remove(&chash) {
                                for stash in stashes {
                                    emit_commit(
                                        stash,
                                        color,
                                        &ref_map,
                                        &mut batch,
                                        &mut total_streamed,
                                        &sender,
                                        &fg_loaded,
                                    );
                                }
                            }
                            for parent_hash in parents.iter().rev() {
                                if let Some(pcommit) = pending_map.remove(parent_hash) {
                                    let pcolor = resolve_color(
                                        parent_hash,
                                        &branch_color_for,
                                        &inherited_color,
                                    )
                                    .unwrap_or_else(|| {
                                        let n = next_fresh_color.get();
                                        next_fresh_color.set(n + 1);
                                        n
                                    });
                                    to_emit.push((pcommit, pcolor));
                                }
                            }
                        }
                    }

                    // Flush the trailing partial batch.
                    if !batch.is_empty() {
                        sender.send(event::AppEvent::AppendCommits { gen: bg_gen, batch });
                    }

                    _bg_done.total.set(total_streamed);

                    // STEP 5: optionally ship a full Graph. The
                    // user clarified this causes a 1-frame flicker
                    // and is pure waste when the graph column is
                    // disabled. Skip the alloc entirely when
                    // graph_enabled is false.
                    //
                    // When enabled, we have to re-hydrate a
                    // bg-local Repository from the streamed
                    // commits. Re-reading the just-built cache is
                    // the cheapest path - it's already on disk and
                    // hot in the page cache, so the bincode decode
                    // is dominated by allocations.
                    //
                    // Skip the post-stream Graph rebake on huge repos
                    // (>100k commits). Materialising the full topology
                    // would allocate ~200-400 MB on top of what bg + fg
                    // already hold, triggering the OS OOM killer. The
                    // fg's initial-slice Graph (~500 commits) stays in
                    // use; bg-streamed rows past that render without
                    // graph images (colour bars only). This is the
                    // documented degraded mode for huge repos.
                    const HUGE_REPO_GRAPH_SKIP: usize = 100_000;

                    if graph_enabled_for_bg && total_streamed < HUGE_REPO_GRAPH_SKIP {
                        if let Some(mut reader) =
                            git::cache::open_stream_for(&repo_path_for_bg, order)
                        {
                            let mut all: Vec<git::CommitSummary> = Vec::new();
                            while let Some(Ok(c)) = reader.next_commit() {
                                all.push(c);
                            }
                            if !all.is_empty() {
                                if let Ok(bg_repo) =
                                    git::Repository::from_cached_summaries(&repo_path_for_bg, all)
                                {
                                    let g = crate::graph::calc_graph(&bg_repo);
                                    sender.send(event::AppEvent::ReplaceGraph {
                                        gen: bg_gen,
                                        graph: Box::new(g),
                                    });
                                }
                            }
                        }
                    } else if graph_enabled_for_bg {
                        crate::glog_info!(
                            "bg-stream: skipping post-stream Graph rebake \
                             (streamed {} > {} threshold)",
                            total_streamed,
                            HUGE_REPO_GRAPH_SKIP
                        );
                    }
                    // BgDoneGuard's Drop fires BackgroundCacheReady
                    // on function exit (normal OR panic).
                })
                .ok();
        }

        // (Re)start the filesystem watcher whenever the repo path changes
        // (first launch, or after `cd`-into-another-repo). Config-reload
        // iterations keep the same path and reuse the existing watcher.
        // Dropping the old debouncer first stops its thread cleanly so we
        // don't leak handles or get cross-talk events from the old path.
        let current_git_dir = repository.path().to_path_buf();
        let needs_rebind = watched_git_dir
            .as_ref()
            .map(|p| p != &current_git_dir)
            .unwrap_or(true);
        if needs_rebind {
            _git_watcher = None;
            // `Repository.path()` is the WORK TREE root, not the
            // `.git/` directory. The watcher needs `.git/` so it
            // monitors HEAD / refs / packed-refs / index, not every
            // file edit in the working tree. Resolve via
            // `git rev-parse --git-dir`; if that fails, fall back
            // to the conventional `<root>/.git`.
            let dotgit = std::process::Command::new("git")
                .arg("rev-parse")
                .arg("--git-dir")
                .current_dir(&current_git_dir)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| {
                    let rel = s.trim().to_string();
                    if std::path::Path::new(&rel).is_absolute() {
                        std::path::PathBuf::from(rel)
                    } else {
                        current_git_dir.join(rel)
                    }
                })
                .and_then(|p| std::fs::canonicalize(&p).ok())
                .unwrap_or_else(|| current_git_dir.join(".git"));
            glog_info!(
                "watcher rebind: work_tree={:?} -> .git={:?}",
                current_git_dir,
                dotgit
            );
            _git_watcher = watcher::start(&dotgit, ec.sender());
            if _git_watcher.is_none() {
                // notify-debouncer init failed (inotify limit exhausted,
                // permission denied, malformed .git symlink, ...). The
                // app still works, but external git ops (commit from
                // another shell, fetch, branch checkout) will NOT
                // auto-refresh - user has to press `R` manually.
                // Log it so the silent degradation is at least
                // debuggable from the rotating session log.
                glog_warn!(
                    "filesystem watcher init failed for {:?}; \
                     external git changes won't auto-refresh until \
                     you press `R`",
                    current_git_dir
                );
            }
            watched_git_dir = Some(current_git_dir);
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
        // Force-disable the graph column on repos too large for inline
        // PNG rendering to keep up. The threshold is user-overridable
        // via `[core.option] huge_repo_threshold` in config.toml
        // (default 50000); set to 0 to disable the auto-disable.
        // The Config view still allows toggling back on - it just
        // displays a red warning in the Details panel so the user
        // knows what they're signing up for.
        //
        // `repository.commit_count()` is NOT usable here - the
        // foreground load is clipped to `max_count` so it only sees
        // ~400 commits at this point and would never trip a
        // mid-thousands threshold. Shell out to
        // `git rev-list --count HEAD` for the true total (one git
        // plumbing call, ~10 ms even on 300k repos).
        // `core_config` has already moved into AppContext by this
        // point, so grab the threshold off the (Rc-wrapped) ctx instead.
        let huge_repo_threshold = ctx.core_config.option.huge_repo_threshold;
        let total_commits = std::process::Command::new("git")
            .args(["rev-list", "--count", "HEAD"])
            .current_dir(repository.path())
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    String::from_utf8(o.stdout)
                        .ok()
                        .and_then(|s| s.trim().parse::<usize>().ok())
                } else {
                    None
                }
            })
            .unwrap_or(0);
        let huge_repo = huge_repo_threshold > 0 && total_commits > huge_repo_threshold;
        if huge_repo {
            glog_info!(
                "huge repo detected ({} commits > {} threshold) - forcing graph_enabled=false",
                total_commits,
                huge_repo_threshold
            );
        }
        if let Some(ctx_mut) = Rc::get_mut(&mut ctx) {
            ctx_mut.current_branch_remote_state = remote_state;
            ctx_mut.repo_path = repository.path().to_path_buf();
            if huge_repo {
                // Warning lives for the whole session - the user
                // should always see it on the Config view's Graph
                // Enabled row, even after they explicitly opted in.
                ctx_mut.graph_huge_repo_warning = true;
                // Force-disable ONCE: at first launch the cached
                // saved-config may have `graph_enabled = true`
                // (default for fresh installs), so we override it
                // to false. On every later iteration we trust the
                // user's saved choice - if they toggled it back
                // on via Config view, that value is reloaded from
                // disk on Refresh and the override here would
                // silently undo it.
                let canonical = repository
                    .path()
                    .canonicalize()
                    .unwrap_or_else(|_| repository.path().to_path_buf());
                if !huge_repo_autodisabled_paths.contains(&canonical) {
                    ctx_mut.ui_config.list.graph_enabled = false;
                    huge_repo_autodisabled_paths.insert(canonical);
                }
            }
        }

        // Always run the real `calc_graph`. The cheap stub
        // `calc_colors_only` used branch-membership first-parent
        // walks to assign colours, which produced a DIFFERENT
        // colour-per-commit mapping than the lane-based scheme
        // calc_graph uses. The user saw inconsistent tints
        // between graph-enabled and graph-disabled modes for
        // the same commit; running the real walk here keeps the
        // colours stable across both. The cost on the
        // fg-loaded 500-commit slice is a few ms.
        let graph = graph::calc_graph(&repository);

        let cell_width_type = check::decide_cell_width_type(&graph, graph_width)?;

        // The image manager now OWNS the Graph, the lifetime-free design
        // lets the bg streaming thread hot-swap the topology later via
        // `replace_graph` once it finishes walking the full history.
        let graph_image_manager = GraphImageManager::new(
            graph,
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
            // two bytes drift apart in the read window, `Alt+c` is
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
            &graph_color_set,
            cell_width_type,
            graph_style,
            initial_selection,
            ctx.clone(),
            &ec,
            refresh_view_context,
            bg_generation,
        );

        match app.run(terminal.as_mut().unwrap()) {
            Ok(Ret::Quit) => {
                break Ok(());
            }
            Ok(Ret::Refresh(request)) => {
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
/// after rendering, i.e. the cursor lands on the LAST row of the image. Two
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
    // Logo and wordmark are now both 5 rows tall, same baseline, no slack.
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

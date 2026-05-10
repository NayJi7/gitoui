pub mod avatar;
pub mod color;
pub mod config;
pub mod git;
pub mod github_auth;
pub mod graph;
pub mod highlight;
pub mod protocol;
pub mod themes;

mod app;
mod brand;
mod check;
mod event;
mod external;
mod keybind;
mod view;
mod watcher;
mod widget;

use std::{path::Path, rc::Rc};

use app::{App, Ret};
use clap::{Parser, ValueEnum};
use graph::GraphImageManager;
use serde::Deserialize;

/// Gitoui - Interactive Git client for the terminal
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

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub fn run() -> Result<()> {
    let args = Args::parse();

    highlight::init();
    let ec = event::EventController::init();
    let mut refresh_view_context = None;
    let mut terminal = None;
    // Filesystem watcher on .git/ — keeps the UI in sync with external git
    // operations (commits from another shell, push/pull/fetch, branch
    // switches, …). Held alive for the whole `run()` lifetime; dropping it
    // stops the watcher thread. Created at most once on the first iteration
    // where the .git directory becomes available.
    let mut _git_watcher: Option<_> = None;

    let ret = loop {
        let (mut core_config, ui_config, graph_config, mut color_theme, keybind_patch) =
            match config::load() {
                Ok(config) => config,
                Err(e) if terminal.is_none() => break Err(e),
                Err(e) => {
                    eprintln!("Failed to reload config: {}", e);
                    continue;
                }
            };
        if let Some(def) = crate::themes::get_theme(&core_config.option.theme) {
            color_theme = def.color_theme;
            core_config.option.syntax_theme = def.syntax_theme.to_owned();
        }
        let keybind = keybind::KeyBind::new(keybind_patch);

        let max_count = args.max_count;
        let image_protocol = args.protocol.or(core_config.option.protocol).into();
        let order = args.order.or(core_config.option.order).into();
        let graph_width = args.graph_width.or(core_config.option.graph_width);
        let graph_style = args.graph_style.or(core_config.option.graph_style).into();
        let graph_image_width_mode = graph_config.row_image_width;
        let initial_selection = args
            .initial_selection
            .or(core_config.option.initial_selection)
            .into();

        // If the config graph background is transparent (default), override it with the
        // theme's bg color so Kitty composites against the correct color instead of the
        // terminal's native background. Kitty images composite against the terminal-native bg
        // for their transparent pixels, ignoring ANSI cell bg codes.
        let graph_color_set = {
            if graph_config.color.background == "#00000000" {
                if let ratatui::style::Color::Rgb(r, g, b) = color_theme.bg {
                    let mut patched = graph_config.color.clone();
                    patched.background = format!("#{:02x}{:02x}{:02x}ff", r, g, b);
                    color::GraphColorSet::new(&patched)
                } else {
                    color::GraphColorSet::new(&graph_config.color)
                }
            } else {
                color::GraphColorSet::new(&graph_config.color)
            }
        };

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
            // Filled after the repository is loaded — see below.
            current_branch_remote_state: None,
        });

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
                                println!("Initialized repository with branch '{}'.", default_branch);
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
            Err(e) => {
                break Err(Box::new(e));
            }
        }
    };

    ratatui::crossterm::execute!(
        std::io::stdout(),
        ratatui::crossterm::event::DisableMouseCapture
    )
    .unwrap();
    ratatui::restore();
    ret.map_err(Into::into)
}

/// Print the gitoui logo + wordmark inline (opencode-style) before the no-repo
/// prompt. Reserves `cell_h` rows by emitting newlines first, moves the cursor
/// back up, prints the image escape, then moves the cursor below — works for
/// both Kitty (which doesn't auto-advance the cursor) and iTerm2/Sixel.
/// Falls back to a plain text banner when the protocol can't render inline.
fn print_no_repo_splash(image_protocol: protocol::ImageProtocol) {
    use std::io::Write;
    let cell_w: usize = 28;
    let cell_h: usize = 5;

    println!();
    let Some(png) = brand::render_mixed_png(cell_w as u32, cell_h as u32) else {
        println!("        gitoui\n");
        return;
    };
    let Some(escape) = image_protocol.encode_inline(&png, cell_w, cell_h, 1) else {
        println!("        gitoui\n");
        return;
    };

    let mut stdout = std::io::stdout().lock();
    // Reserve cell_h rows.
    for _ in 0..cell_h {
        let _ = writeln!(stdout);
    }
    // Move cursor back up over the reserved rows.
    let _ = write!(stdout, "\x1b[{}A", cell_h);
    // Render the image at the (now-reserved) cursor position.
    let _ = write!(stdout, "{}", escape);
    // Move cursor back down past the image, then add a blank line.
    let _ = write!(stdout, "\x1b[{}B\n", cell_h);
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
    } else if let Some(rest) = url.strip_prefix("ssh://git@github.com/") {
        rest
    } else {
        return None;
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

//! `gitoui --uninstall`: remove the binary the way it was installed, then
//! (only if the user agrees) the settings, GitHub login and caches.
//!
//! Nothing is deleted without a `y`. Homebrew and cargo own their copy of the
//! binary, so their own uninstall command is run instead of deleting the file.

use std::env;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::update::{self, InstallMethod};

const APP_DIR_NAME: &str = "gitoui";

/// Directories gitoui may have written to, without duplicates. The modules
/// resolve them in two ways (`$XDG_*` / `$HOME/...` and the `dirs` crate), so
/// both are listed; only the ones that exist are shown and removed.
fn data_dir_candidates(
    xdg_config: Option<PathBuf>,
    xdg_cache: Option<PathBuf>,
    home: Option<PathBuf>,
    config_dir: Option<PathBuf>,
    cache_dir: Option<PathBuf>,
) -> Vec<PathBuf> {
    let bases = [
        xdg_config,
        home.clone().map(|h| h.join(".config")),
        config_dir,
        xdg_cache,
        home.map(|h| h.join(".cache")),
        cache_dir,
    ];
    let mut out: Vec<PathBuf> = Vec::new();
    for base in bases.into_iter().flatten() {
        let dir = base.join(APP_DIR_NAME);
        if !out.contains(&dir) {
            out.push(dir);
        }
    }
    out
}

fn existing_data_dirs() -> Vec<PathBuf> {
    let var = |name: &str| {
        env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    data_dir_candidates(
        var("XDG_CONFIG_HOME"),
        var("XDG_CACHE_HOME"),
        var("HOME"),
        dirs::config_dir(),
        dirs::cache_dir(),
    )
    .into_iter()
    .filter(|d| d.is_dir())
    .collect()
}

/// Ask a yes/no question, `no` by default. Only `y` / `yes` agree.
fn confirm(question: &str, input: &mut impl BufRead) -> bool {
    print!("  {question} [y/N]: ");
    let _ = io::stdout().flush();
    let mut line = String::new();
    if input.read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// A binary built in `target/` is a dev build, not an install.
fn is_dev_build(exe: &Path) -> bool {
    let s = exe.to_string_lossy();
    s.contains("/target/debug/")
        || s.contains("/target/release/")
        || s.contains("\\target\\debug\\")
        || s.contains("\\target\\release\\")
}

fn run_tool(program: &str, args: &[&str]) -> io::Result<()> {
    let status = Command::new(program).args(args).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "`{program} {}` failed",
            args.join(" ")
        )))
    }
}

pub fn run() -> io::Result<()> {
    if !io::stdin().is_terminal() {
        return Err(io::Error::other(
            "--uninstall asks for confirmation, run it in a terminal",
        ));
    }
    let exe = update::resolved_self_path()?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    let method = update::detect_install_method();
    let data_dirs = existing_data_dirs();
    let mut input = io::stdin().lock();

    println!();
    if is_dev_build(&exe) {
        println!(
            "  {} is a development build, not an install: the binary is left alone.",
            exe.display()
        );
    } else {
        match method {
            InstallMethod::Homebrew => println!(
                "  gitoui was installed with Homebrew: `brew uninstall gitoui` will be run."
            ),
            InstallMethod::Cargo => {
                println!("  gitoui was installed with cargo: `cargo uninstall gitoui` will be run.")
            }
            InstallMethod::CurlScript => {
                println!("  The binary {} will be deleted.", exe.display())
            }
        }
        if confirm("Remove the gitoui binary?", &mut input) {
            match method {
                InstallMethod::Homebrew => run_tool("brew", &["uninstall", "gitoui"])?,
                InstallMethod::Cargo => run_tool("cargo", &["uninstall", "gitoui"])?,
                InstallMethod::CurlScript => std::fs::remove_file(&exe)?,
            }
            println!("  Binary removed.");
        } else {
            println!("  Binary kept.");
        }
    }

    if data_dirs.is_empty() {
        println!("  No settings or cache found.");
        return Ok(());
    }
    println!();
    println!("  Your settings, GitHub login, recent directories, logs and caches:");
    for d in &data_dirs {
        println!("    {}", d.display());
    }
    if confirm("Delete them too?", &mut input) {
        for d in &data_dirs {
            match std::fs::remove_dir_all(d) {
                Ok(()) => println!("  Deleted {}", d.display()),
                Err(e) => eprintln!("  Could not delete {}: {e}", d.display()),
            }
        }
    } else {
        println!("  Data kept.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Option<PathBuf> {
        Some(PathBuf::from(s))
    }

    #[test]
    fn candidates_are_deduplicated() {
        let c = data_dir_candidates(None, None, p("/h"), p("/h/.config"), p("/h/.cache"));
        assert_eq!(
            c,
            vec![
                PathBuf::from("/h/.config/gitoui"),
                PathBuf::from("/h/.cache/gitoui")
            ]
        );
    }

    #[test]
    fn xdg_and_platform_dirs_are_both_listed() {
        let c = data_dir_candidates(p("/x/c"), p("/x/k"), p("/h"), p("/p/c"), p("/p/k"));
        assert!(c.contains(&PathBuf::from("/x/c/gitoui")));
        assert!(c.contains(&PathBuf::from("/x/k/gitoui")));
        assert!(c.contains(&PathBuf::from("/p/c/gitoui")));
        assert!(c.contains(&PathBuf::from("/h/.cache/gitoui")));
    }

    #[test]
    fn only_yes_agrees() {
        for (answer, expected) in [
            ("y\n", true),
            ("YES\n", true),
            ("n\n", false),
            ("\n", false),
            ("oui\n", false),
        ] {
            assert_eq!(confirm("?", &mut answer.as_bytes()), expected, "{answer:?}");
        }
    }

    #[test]
    fn dev_builds_are_not_installs() {
        assert!(is_dev_build(Path::new("/r/gitoui/target/debug/gitoui")));
        assert!(!is_dev_build(Path::new("/home/u/.local/bin/gitoui")));
    }
}

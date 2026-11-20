//! Asks the terminal, once at startup, how big a cell is in pixels and
//! whether it speaks Sixel.
//!
//! Sixel has no scaling: the pixels we send are the pixels drawn, so the
//! encoder needs the real cell size to fit an image into N cells. Kitty
//! and iTerm2 take a cell count and scale on the terminal side.
//!
//! On Unix the pixel size comes from the `TIOCGWINSZ` ioctl. Windows has
//! no equivalent that works under ConPTY, so Windows Terminal is asked
//! with `CSI 16 t` (cell size) and `CSI c` (device attributes, where
//! attribute 4 means Sixel). The attributes reply doubles as the end
//! marker: every terminal answers it, and it comes after the `16 t` reply.

/// How long to wait for the device attributes reply before giving up.
#[cfg(windows)]
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(300);

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TerminalReport {
    /// Cell size in pixels, as (width, height).
    pub cell_px: Option<(u16, u16)>,
    /// The device attributes list includes 4 (Sixel graphics).
    pub sixel: bool,
    /// The device attributes reply arrived: the terminal answered at all.
    pub answered: bool,
}

/// Reads every CSI reply in `reply` and keeps what we asked for. Only the
/// Windows probe reads replies from the key events (Unix gets the cell size
/// from the terminal's ioctl), so elsewhere it exists for the tests alone.
#[cfg(any(windows, test))]
pub fn parse_reply(reply: &str) -> TerminalReport {
    let mut report = TerminalReport::default();
    let mut rest = reply;
    // The ESC is optional: the Windows console input does not report it.
    while let Some(start) = rest.find('[') {
        let body = &rest[start + 1..];
        let Some(end) = body.find(|c: char| ('\x40'..='\x7e').contains(&c)) else {
            break;
        };
        let params = &body[..end];
        match body.as_bytes()[end] {
            // `CSI 6 ; height ; width t`: cell size in pixels.
            b't' => {
                let mut fields = params.split(';');
                if fields.next() == Some("6") {
                    let height = fields.next().and_then(|v| v.parse::<u16>().ok());
                    let width = fields.next().and_then(|v| v.parse::<u16>().ok());
                    if let (Some(w), Some(h)) = (width, height) {
                        if w > 0 && h > 0 {
                            report.cell_px = Some((w, h));
                        }
                    }
                }
            }
            // `CSI ? 61 ; 4 ; 6 c`: primary device attributes.
            b'c' => {
                if let Some(attrs) = params.strip_prefix('?') {
                    report.answered = true;
                    report.sixel = attrs.split(';').any(|a| a == "4");
                }
            }
            _ => {}
        }
        rest = &body[end + 1..];
    }
    report
}

/// Can gitoui read the keyboard here? On Windows keys come from the console
/// API, so stdin has to be a Windows console: Windows Terminal, PowerShell,
/// cmd, VS Code, or any program run through `winpty`. Git Bash (mintty) and
/// pipes give a pipe instead, where gitoui would draw its screen and then
/// never hear a key. Everywhere else a terminal is a plain file descriptor.
#[cfg(windows)]
pub fn can_read_keys() -> bool {
    use std::os::windows::io::AsRawHandle;
    extern "system" {
        fn GetConsoleMode(handle: *mut std::ffi::c_void, mode: *mut u32) -> i32;
    }
    let mut mode = 0u32;
    // SAFETY: `GetConsoleMode` only reads the handle and writes `mode`; a
    // handle that is not a console just makes it return 0.
    unsafe { GetConsoleMode(std::io::stdin().as_raw_handle(), &mut mode) != 0 }
}

#[cfg(not(windows))]
pub fn can_read_keys() -> bool {
    true
}

/// What to tell someone who started gitoui where it cannot read keys.
pub const NO_CONSOLE_MESSAGE: &str = "gitoui needs a Windows console to read the keyboard, and \
this terminal does not give it one (Git Bash / mintty, or a pipe).\n\
Run it in Windows Terminal, PowerShell or cmd, or in Git Bash through winpty:\n\n    winpty gitoui\n";

/// Queries the terminal. Returns an empty report when stdin or stdout is
/// not a terminal, or when nothing could be learned.
pub fn probe() -> TerminalReport {
    use std::io::IsTerminal;
    if !(std::io::stdin().is_terminal() && std::io::stdout().is_terminal()) {
        return TerminalReport::default();
    }
    probe_terminal()
}

#[cfg(not(windows))]
fn probe_terminal() -> TerminalReport {
    let mut report = TerminalReport::default();
    if let Ok(size) = ratatui::crossterm::terminal::window_size() {
        if size.columns > 0 && size.rows > 0 && size.width > 0 && size.height > 0 {
            report.cell_px = Some((size.width / size.columns, size.height / size.rows));
        }
    }
    report
}

#[cfg(windows)]
fn probe_terminal() -> TerminalReport {
    use ratatui::crossterm::{
        event::{self, Event, KeyCode, KeyEventKind},
        terminal,
    };
    use std::io::Write;
    use std::time::Instant;

    if terminal::enable_raw_mode().is_err() {
        return TerminalReport::default();
    }
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(b"\x1b[16t\x1b[c");
    let _ = stdout.flush();

    // The replies come back as key events, one per character. The ESC that
    // opens each reply is not reported, `parse_reply` copes without it.
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let mut reply = String::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || !event::poll(left).unwrap_or(false) {
            break;
        }
        let Ok(Event::Key(key)) = event::read() else {
            continue;
        };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        match key.code {
            KeyCode::Char(c) => reply.push(c),
            KeyCode::Esc => reply.push('\x1b'),
            _ => {}
        }
        if parse_reply(&reply).answered {
            break;
        }
    }
    let _ = terminal::disable_raw_mode();
    parse_reply(&reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_terminal_reply_gives_cell_size_and_sixel() {
        let report = parse_reply("\x1b[6;20;10t\x1b[?61;4;6;7;14;21;22;23;24;28;32;42;52c");
        assert_eq!(
            report,
            TerminalReport {
                cell_px: Some((10, 20)),
                sixel: true,
                answered: true,
            }
        );
    }

    #[test]
    fn reply_without_the_escape_characters_still_parses() {
        let report = parse_reply("[6;20;10t[?61;4;6c");
        assert_eq!(report.cell_px, Some((10, 20)));
        assert!(report.sixel);
        assert!(report.answered);
    }

    #[test]
    fn terminal_without_sixel_attribute() {
        let report = parse_reply("\x1b[?62;22c");
        assert_eq!(report.cell_px, None);
        assert!(!report.sixel);
        assert!(report.answered);
    }

    #[test]
    fn attribute_40_is_not_sixel() {
        assert!(!parse_reply("\x1b[?1;40c").sixel);
    }

    #[test]
    fn zero_cell_size_is_ignored() {
        assert_eq!(parse_reply("\x1b[6;0;0t").cell_px, None);
    }

    #[test]
    fn truncated_or_empty_reply_learns_nothing() {
        assert_eq!(parse_reply(""), TerminalReport::default());
        assert_eq!(parse_reply("\x1b[6;20"), TerminalReport::default());
    }
}

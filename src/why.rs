//! `gitoui --why`: the closing card of the promo film, replayed in the
//! terminal. "Say oui" with its orange underline drawn left to right,
//! then the three reasons ticked one by one.
//!
//! The animation only runs on a terminal; piped output gets the same
//! text at once. Colors follow the film's palette and honour NO_COLOR.

use std::io::{self, IsTerminal, Write};
use std::thread::sleep;
use std::time::Duration;

const REASONS: [&str; 3] = ["fast.", "free.", "all in one."];

struct Palette {
    title: &'static str,
    brand: &'static str,
    text: &'static str,
    check: &'static str,
    reset: &'static str,
}

const COLORS: Palette = Palette {
    title: "\x1b[1;38;2;241;236;236m",
    brand: "\x1b[1;38;2;240;81;51m",
    text: "\x1b[38;2;220;215;214m",
    check: "\x1b[38;2;152;176;160m",
    reset: "\x1b[0m",
};

const PLAIN: Palette = Palette {
    title: "",
    brand: "",
    text: "",
    check: "",
    reset: "",
};

/// Pauses between the steps, in milliseconds. All zero when the output
/// isn't a terminal.
struct Timing {
    beat: u64,
    stroke: u64,
    tick: u64,
}

const ANIMATED: Timing = Timing {
    beat: 280,
    stroke: 70,
    tick: 320,
};

const INSTANT: Timing = Timing {
    beat: 0,
    stroke: 0,
    tick: 0,
};

pub fn run() {
    let stdout = io::stdout();
    let animate = stdout.is_terminal();
    let color = animate && std::env::var_os("NO_COLOR").is_none();
    let palette = if color { &COLORS } else { &PLAIN };
    let timing = if animate { &ANIMATED } else { &INSTANT };
    let mut out = stdout.lock();
    let _ = write_why(&mut out, palette, timing);
}

fn pause(w: &mut impl Write, ms: u64) -> io::Result<()> {
    w.flush()?;
    if ms > 0 {
        sleep(Duration::from_millis(ms));
    }
    Ok(())
}

fn write_why(w: &mut impl Write, p: &Palette, t: &Timing) -> io::Result<()> {
    writeln!(w)?;
    writeln!(w, "  {}Say{} {}oui{}", p.title, p.reset, p.brand, p.reset)?;

    // Underline under "oui" (columns 6..9), drawn one stroke at a time.
    write!(w, "      ")?;
    for _ in 0..3 {
        pause(w, t.stroke)?;
        write!(w, "{}\u{2594}{}", p.brand, p.reset)?;
    }
    writeln!(w)?;
    pause(w, t.beat)?;

    for reason in REASONS {
        pause(w, t.tick)?;
        writeln!(
            w,
            "  {}\u{2713}{} {}{}{}",
            p.check, p.reset, p.text, reason, p.reset
        )?;
    }
    writeln!(w)?;
    w.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_output_matches_the_film_card() {
        let mut buf = Vec::new();
        write_why(&mut buf, &PLAIN, &INSTANT).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert_eq!(
            text,
            "\n  Say oui\n      \u{2594}\u{2594}\u{2594}\n  \u{2713} fast.\n  \u{2713} free.\n  \u{2713} all in one.\n\n"
        );
    }

    #[test]
    fn colored_output_resets_after_every_span() {
        let mut buf = Vec::new();
        write_why(&mut buf, &COLORS, &INSTANT).unwrap();
        let text = String::from_utf8(buf).unwrap();
        // "Say", "oui", 3 underline strokes, then check + text per reason.
        assert_eq!(text.matches("\x1b[0m").count(), 2 + 3 + 2 * 3);
        assert!(text.contains("\x1b[1;38;2;240;81;51moui\x1b[0m"));
    }
}

//! Keyboard and mouse events translated into the bytes a terminal sends.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use vt100::{MouseProtocolEncoding, MouseProtocolMode};

/// `1 + (Shift 1 | Alt 2 | Ctrl 4)`, the xterm modifier parameter.
fn modifier_param(mods: KeyModifiers) -> u8 {
    let mut m = 0;
    if mods.contains(KeyModifiers::SHIFT) {
        m |= 1;
    }
    if mods.contains(KeyModifiers::ALT) {
        m |= 2;
    }
    if mods.contains(KeyModifiers::CONTROL) {
        m |= 4;
    }
    m + 1
}

fn csi_key(final_byte: char, mods: KeyModifiers) -> Vec<u8> {
    let any = mods.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT | KeyModifiers::CONTROL);
    if any {
        format!("\x1b[1;{}{final_byte}", modifier_param(mods)).into_bytes()
    } else {
        format!("\x1b[{final_byte}").into_bytes()
    }
}

fn tilde_key(code: u8, mods: KeyModifiers) -> Vec<u8> {
    let any = mods.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT | KeyModifiers::CONTROL);
    if any {
        format!("\x1b[{code};{}~", modifier_param(mods)).into_bytes()
    } else {
        format!("\x1b[{code}~").into_bytes()
    }
}

/// `None` when the key has no terminal encoding.
pub(crate) fn encode_key(key: KeyEvent, application_cursor: bool) -> Option<Vec<u8>> {
    let mods = key.modifiers;
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    let alt = mods.contains(KeyModifiers::ALT);
    let move_key = |plain: char| -> Vec<u8> {
        let any = mods.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT | KeyModifiers::CONTROL);
        if !any && application_cursor {
            format!("\x1bO{plain}").into_bytes()
        } else {
            csi_key(plain, mods)
        }
    };
    let bytes = match key.code {
        KeyCode::Char(c) => {
            if ctrl {
                let lower = c.to_ascii_lowercase();
                match lower {
                    'a'..='z' => vec![lower as u8 & 0x1f],
                    ' ' | '@' => vec![0x00],
                    '[' => vec![0x1b],
                    '\\' => vec![0x1c],
                    ']' => vec![0x1d],
                    '^' => vec![0x1e],
                    '_' => vec![0x1f],
                    _ => c.to_string().into_bytes(),
                }
            } else {
                c.to_string().into_bytes()
            }
        }
        KeyCode::Enter => b"\r".to_vec(),
        KeyCode::Tab => b"\t".to_vec(),
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => move_key('A'),
        KeyCode::Down => move_key('B'),
        KeyCode::Right => move_key('C'),
        KeyCode::Left => move_key('D'),
        KeyCode::Home => move_key('H'),
        KeyCode::End => move_key('F'),
        KeyCode::Insert => tilde_key(2, mods),
        KeyCode::Delete => tilde_key(3, mods),
        KeyCode::PageUp => tilde_key(5, mods),
        KeyCode::PageDown => tilde_key(6, mods),
        KeyCode::F(n @ 1..=4) => {
            format!("\x1bO{}", ['P', 'Q', 'R', 'S'][usize::from(n) - 1]).into_bytes()
        }
        KeyCode::F(n @ 5..=12) => {
            const CODES: [u8; 8] = [15, 17, 18, 19, 20, 21, 23, 24];
            tilde_key(CODES[usize::from(n) - 5], KeyModifiers::NONE)
        }
        _ => return None,
    };
    let arrow_like = matches!(
        key.code,
        KeyCode::Up
            | KeyCode::Down
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::Insert
            | KeyCode::Delete
            | KeyCode::PageUp
            | KeyCode::PageDown
    );
    if alt && !arrow_like && !matches!(key.code, KeyCode::BackTab) {
        let mut out = vec![0x1b];
        out.extend(bytes);
        Some(out)
    } else {
        Some(bytes)
    }
}

/// `col` and `row` count from 0 at the corner of the output area.
pub(crate) fn encode_mouse(
    kind: MouseEventKind,
    modifiers: KeyModifiers,
    col: u16,
    row: u16,
    mode: MouseProtocolMode,
    encoding: MouseProtocolEncoding,
) -> Option<Vec<u8>> {
    let button_code = |b: MouseButton| match b {
        MouseButton::Left => 0u16,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    };
    // (code, is_release)
    let (mut code, release) = match kind {
        MouseEventKind::Down(b) => (button_code(b), false),
        MouseEventKind::Up(b) => (button_code(b), true),
        MouseEventKind::Drag(b) => (button_code(b) + 32, false),
        MouseEventKind::Moved => (3 + 32, false),
        MouseEventKind::ScrollUp => (64, false),
        MouseEventKind::ScrollDown => (65, false),
        _ => return None,
    };
    let wanted = match mode {
        MouseProtocolMode::None => false,
        MouseProtocolMode::Press => {
            matches!(
                kind,
                MouseEventKind::Down(_) | MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
            )
        }
        MouseProtocolMode::PressRelease => {
            !matches!(kind, MouseEventKind::Drag(_) | MouseEventKind::Moved)
        }
        MouseProtocolMode::ButtonMotion => !matches!(kind, MouseEventKind::Moved),
        MouseProtocolMode::AnyMotion => true,
    };
    if !wanted {
        return None;
    }
    if modifiers.contains(KeyModifiers::SHIFT) {
        code += 4;
    }
    if modifiers.contains(KeyModifiers::ALT) {
        code += 8;
    }
    if modifiers.contains(KeyModifiers::CONTROL) {
        code += 16;
    }
    let (x, y) = (u32::from(col) + 1, u32::from(row) + 1);
    match encoding {
        MouseProtocolEncoding::Sgr => {
            let end = if release { 'm' } else { 'M' };
            Some(format!("\x1b[<{code};{x};{y}{end}").into_bytes())
        }
        MouseProtocolEncoding::Default | MouseProtocolEncoding::Utf8 => {
            // Releases carry button 3 in the legacy encodings, keeping modifiers.
            let code = if release { (code & !0b11) | 3 } else { code };
            let b = u32::from(code) + 32;
            let mut out = b"\x1b[M".to_vec();
            if encoding == MouseProtocolEncoding::Default {
                if x + 32 > 255 || y + 32 > 255 || b > 255 {
                    return None;
                }
                out.extend([b as u8, (x + 32) as u8, (y + 32) as u8]);
            } else {
                for v in [b, x + 32, y + 32] {
                    let mut buf = [0u8; 4];
                    out.extend(char::from_u32(v)?.encode_utf8(&mut buf).as_bytes());
                }
            }
            Some(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
    use vt100::{MouseProtocolEncoding as Enc, MouseProtocolMode as Mode};

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    fn enc(code: KeyCode, mods: KeyModifiers) -> Option<Vec<u8>> {
        encode_key(key(code, mods), false)
    }

    #[test]
    fn encodes_text_and_control_keys() {
        let none = KeyModifiers::NONE;
        assert_eq!(enc(KeyCode::Char('a'), none), Some(b"a".to_vec()));
        assert_eq!(enc(KeyCode::Char('é'), none), Some("é".as_bytes().to_vec()));
        assert_eq!(
            enc(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Some(b"\x03".to_vec())
        );
        assert_eq!(
            enc(KeyCode::Char('z'), KeyModifiers::CONTROL),
            Some(b"\x1a".to_vec())
        );
        assert_eq!(
            enc(KeyCode::Char(' '), KeyModifiers::CONTROL),
            Some(b"\x00".to_vec())
        );
        assert_eq!(
            enc(KeyCode::Char('b'), KeyModifiers::ALT),
            Some(b"\x1bb".to_vec())
        );
        assert_eq!(enc(KeyCode::Enter, none), Some(b"\r".to_vec()));
        assert_eq!(enc(KeyCode::Backspace, none), Some(b"\x7f".to_vec()));
        assert_eq!(enc(KeyCode::Esc, none), Some(b"\x1b".to_vec()));
        assert_eq!(enc(KeyCode::Tab, none), Some(b"\t".to_vec()));
        assert_eq!(
            enc(KeyCode::BackTab, KeyModifiers::SHIFT),
            Some(b"\x1b[Z".to_vec())
        );
    }

    #[test]
    fn arrows_follow_application_cursor_mode() {
        let none = KeyModifiers::NONE;
        assert_eq!(
            encode_key(key(KeyCode::Up, none), false),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            encode_key(key(KeyCode::Up, none), true),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(
            encode_key(key(KeyCode::Home, none), true),
            Some(b"\x1bOH".to_vec())
        );
        assert_eq!(
            encode_key(key(KeyCode::Right, KeyModifiers::CONTROL), false),
            Some(b"\x1b[1;5C".to_vec())
        );
    }

    #[test]
    fn function_and_navigation_keys() {
        let none = KeyModifiers::NONE;
        assert_eq!(enc(KeyCode::F(1), none), Some(b"\x1bOP".to_vec()));
        assert_eq!(enc(KeyCode::F(5), none), Some(b"\x1b[15~".to_vec()));
        assert_eq!(enc(KeyCode::F(12), none), Some(b"\x1b[24~".to_vec()));
        assert_eq!(enc(KeyCode::PageDown, none), Some(b"\x1b[6~".to_vec()));
        assert_eq!(enc(KeyCode::Delete, none), Some(b"\x1b[3~".to_vec()));
        assert_eq!(enc(KeyCode::Null, none), None);
    }

    #[test]
    fn mouse_is_sent_only_when_asked() {
        let down = MouseEventKind::Down(MouseButton::Left);
        assert_eq!(
            encode_mouse(down, KeyModifiers::NONE, 0, 0, Mode::None, Enc::Sgr),
            None
        );
        // A plain press mode ignores releases and motion.
        let up = MouseEventKind::Up(MouseButton::Left);
        assert_eq!(
            encode_mouse(up, KeyModifiers::NONE, 0, 0, Mode::Press, Enc::Sgr),
            None
        );
        assert_eq!(
            encode_mouse(
                MouseEventKind::Moved,
                KeyModifiers::NONE,
                0,
                0,
                Mode::PressRelease,
                Enc::Sgr
            ),
            None
        );
    }

    #[test]
    fn sgr_click_release_and_wheel() {
        let none = KeyModifiers::NONE;
        let down = MouseEventKind::Down(MouseButton::Left);
        let up = MouseEventKind::Up(MouseButton::Left);
        assert_eq!(
            encode_mouse(down, none, 0, 0, Mode::PressRelease, Enc::Sgr),
            Some(b"\x1b[<0;1;1M".to_vec())
        );
        assert_eq!(
            encode_mouse(up, none, 0, 0, Mode::PressRelease, Enc::Sgr),
            Some(b"\x1b[<0;1;1m".to_vec())
        );
        assert_eq!(
            encode_mouse(MouseEventKind::ScrollUp, none, 4, 2, Mode::Press, Enc::Sgr),
            Some(b"\x1b[<64;5;3M".to_vec())
        );
        assert_eq!(
            encode_mouse(
                MouseEventKind::ScrollDown,
                KeyModifiers::CONTROL,
                0,
                0,
                Mode::Press,
                Enc::Sgr
            ),
            Some(b"\x1b[<81;1;1M".to_vec())
        );
    }

    #[test]
    fn default_encoding_offsets() {
        let down = MouseEventKind::Down(MouseButton::Left);
        assert_eq!(
            encode_mouse(down, KeyModifiers::NONE, 0, 0, Mode::Press, Enc::Default),
            Some(vec![0x1b, b'[', b'M', 32, 33, 33])
        );
        assert_eq!(
            encode_mouse(down, KeyModifiers::NONE, 300, 0, Mode::Press, Enc::Default),
            None
        );
        assert_eq!(
            encode_mouse(
                MouseEventKind::Up(MouseButton::Left),
                KeyModifiers::NONE,
                1,
                1,
                Mode::PressRelease,
                Enc::Default
            ),
            Some(vec![0x1b, b'[', b'M', 35, 34, 34])
        );
    }

    #[test]
    fn drag_and_motion_modes() {
        let drag = MouseEventKind::Drag(MouseButton::Left);
        assert_eq!(
            encode_mouse(drag, KeyModifiers::NONE, 0, 0, Mode::PressRelease, Enc::Sgr),
            None
        );
        assert_eq!(
            encode_mouse(drag, KeyModifiers::NONE, 0, 0, Mode::ButtonMotion, Enc::Sgr),
            Some(b"\x1b[<32;1;1M".to_vec())
        );
        assert_eq!(
            encode_mouse(
                MouseEventKind::Moved,
                KeyModifiers::NONE,
                0,
                0,
                Mode::AnyMotion,
                Enc::Sgr
            ),
            Some(b"\x1b[<35;1;1M".to_vec())
        );
    }
}

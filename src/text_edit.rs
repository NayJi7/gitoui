//! Word-wise editing keys shared by the text fields that do not have their own
//! editor: the search of the commit list, of the diff and of the Actions page.
//!
//! Terminals disagree on what Ctrl+Backspace sends: the key itself with the
//! Ctrl modifier, `Ctrl+H` (the legacy ASCII backspace) or the control
//! character. They all mean "delete the word on the left".

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Ctrl+Backspace, Alt+Backspace, Ctrl+H, Ctrl+W: delete the word on the left.
pub fn is_delete_word_left(key: &KeyEvent) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key
        .modifiers
        .intersects(KeyModifiers::ALT | KeyModifiers::META);
    match key.code {
        KeyCode::Backspace => ctrl || alt,
        KeyCode::Char('h')
        | KeyCode::Char('w')
        | KeyCode::Char('\u{08}')
        | KeyCode::Char('\u{7f}') => ctrl,
        _ => false,
    }
}

/// Ctrl+Delete, Alt+Delete, Alt+D: delete the word on the right.
pub fn is_delete_word_right(key: &KeyEvent) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key
        .modifiers
        .intersects(KeyModifiers::ALT | KeyModifiers::META);
    match key.code {
        KeyCode::Delete => ctrl || alt,
        KeyCode::Char('d') => alt,
        _ => false,
    }
}

/// Maps the editing shortcuts of readline and of the terminals onto the keys
/// every text field of the app already understands, so they work the same
/// everywhere there is typing:
/// Ctrl+A/E (start/end), Ctrl+B/F (left/right), Alt+B/F and Alt+arrows (word
/// left/right), Alt+D (delete word right), and every spelling of "delete the
/// word on the left" (Ctrl+W, Ctrl+H, Alt+Backspace…) become Ctrl+Backspace.
pub fn normalize_edit_key(key: KeyEvent) -> KeyEvent {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key
        .modifiers
        .intersects(KeyModifiers::ALT | KeyModifiers::META);
    let to = |code: KeyCode, modifiers: KeyModifiers| KeyEvent {
        code,
        modifiers,
        kind: key.kind,
        state: key.state,
    };
    let none = KeyModifiers::NONE;
    let word = KeyModifiers::CONTROL;
    match key.code {
        KeyCode::Char('a') if ctrl && !alt => to(KeyCode::Home, none),
        KeyCode::Char('e') if ctrl && !alt => to(KeyCode::End, none),
        KeyCode::Char('b') if ctrl && !alt => to(KeyCode::Left, none),
        KeyCode::Char('f') if ctrl && !alt => to(KeyCode::Right, none),
        KeyCode::Char('b') if alt && !ctrl => to(KeyCode::Left, word),
        KeyCode::Char('f') if alt && !ctrl => to(KeyCode::Right, word),
        KeyCode::Left if alt && !ctrl => to(KeyCode::Left, word),
        KeyCode::Right if alt && !ctrl => to(KeyCode::Right, word),
        _ if is_delete_word_left(&key) => to(KeyCode::Backspace, word),
        _ if is_delete_word_right(&key) => to(KeyCode::Delete, word),
        _ => key,
    }
}

/// Applies a key to a `tui_input::Input`, the field of the commit list search
/// and of the Actions search: the library reads Ctrl+H as a plain backspace
/// and ignores Ctrl+Backspace, so the word deletions are done here.
pub fn edit_input(input: &mut tui_input::Input, key: &KeyEvent) {
    use tui_input::{backend::crossterm::EventHandler, InputRequest};
    if is_delete_word_left(key) {
        input.handle(InputRequest::DeletePrevWord);
    } else if is_delete_word_right(key) {
        input.handle(InputRequest::DeleteNextWord);
    } else {
        input.handle_event(&ratatui::crossterm::event::Event::Key(*key));
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Byte index where the word before `cursor` starts: skips what is not part of
/// a word, then the word itself.
pub fn word_left(s: &str, cursor: usize) -> usize {
    let mut i = cursor.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    let before: Vec<(usize, char)> = s[..i].char_indices().collect();
    let mut k = before.len();
    while k > 0 && !is_word_char(before[k - 1].1) {
        k -= 1;
    }
    while k > 0 && is_word_char(before[k - 1].1) {
        k -= 1;
    }
    before
        .get(k)
        .map_or(if k == 0 { 0 } else { i }, |(pos, _)| *pos)
}

/// Byte index where the word after `cursor` ends.
pub fn word_right(s: &str, cursor: usize) -> usize {
    let mut i = cursor.min(s.len());
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    let mut chars = s[i..].char_indices().peekable();
    let mut end = i;
    while let Some(&(pos, c)) = chars.peek() {
        if is_word_char(c) {
            break;
        }
        end = i + pos + c.len_utf8();
        chars.next();
    }
    while let Some(&(pos, c)) = chars.peek() {
        if !is_word_char(c) {
            break;
        }
        end = i + pos + c.len_utf8();
        chars.next();
    }
    end
}

/// Deletes the word before `cursor` in `s` and returns the new cursor.
pub fn delete_word_left(s: &mut String, cursor: usize) -> usize {
    let start = word_left(s, cursor);
    let end = cursor.min(s.len());
    if start < end {
        s.drain(start..end);
    }
    start.min(s.len())
}

/// Deletes the word after `cursor` in `s`; the cursor does not move.
pub fn delete_word_right(s: &mut String, cursor: usize) {
    let end = word_right(s, cursor);
    let start = cursor.min(s.len());
    if start < end {
        s.drain(start..end);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn every_way_a_terminal_sends_ctrl_backspace_deletes_a_word() {
        let ctrl = KeyModifiers::CONTROL;
        assert!(is_delete_word_left(&key(KeyCode::Backspace, ctrl)));
        assert!(is_delete_word_left(&key(
            KeyCode::Backspace,
            KeyModifiers::ALT
        )));
        assert!(is_delete_word_left(&key(KeyCode::Char('h'), ctrl)));
        assert!(is_delete_word_left(&key(KeyCode::Char('w'), ctrl)));
        assert!(is_delete_word_left(&key(KeyCode::Char('\u{08}'), ctrl)));
        assert!(is_delete_word_left(&key(KeyCode::Char('\u{7f}'), ctrl)));
        // Plain keys do not.
        assert!(!is_delete_word_left(&key(
            KeyCode::Backspace,
            KeyModifiers::NONE
        )));
        assert!(!is_delete_word_left(&key(
            KeyCode::Char('h'),
            KeyModifiers::NONE
        )));
        assert!(is_delete_word_right(&key(KeyCode::Delete, ctrl)));
        assert!(!is_delete_word_right(&key(
            KeyCode::Delete,
            KeyModifiers::NONE
        )));
    }

    #[test]
    fn readline_shortcuts_become_the_keys_every_field_knows() {
        let ctrl = KeyModifiers::CONTROL;
        let alt = KeyModifiers::ALT;
        let n = |code, m| normalize_edit_key(key(code, m));
        assert_eq!(
            n(KeyCode::Char('a'), ctrl),
            key(KeyCode::Home, KeyModifiers::NONE)
        );
        assert_eq!(
            n(KeyCode::Char('e'), ctrl),
            key(KeyCode::End, KeyModifiers::NONE)
        );
        assert_eq!(
            n(KeyCode::Char('b'), ctrl),
            key(KeyCode::Left, KeyModifiers::NONE)
        );
        assert_eq!(
            n(KeyCode::Char('f'), ctrl),
            key(KeyCode::Right, KeyModifiers::NONE)
        );
        assert_eq!(n(KeyCode::Char('b'), alt), key(KeyCode::Left, ctrl));
        assert_eq!(n(KeyCode::Char('f'), alt), key(KeyCode::Right, ctrl));
        assert_eq!(n(KeyCode::Left, alt), key(KeyCode::Left, ctrl));
        assert_eq!(n(KeyCode::Right, alt), key(KeyCode::Right, ctrl));
        assert_eq!(n(KeyCode::Char('d'), alt), key(KeyCode::Delete, ctrl));
        for k in [
            key(KeyCode::Char('w'), ctrl),
            key(KeyCode::Char('h'), ctrl),
            key(KeyCode::Backspace, alt),
            key(KeyCode::Char('\u{08}'), ctrl),
        ] {
            assert_eq!(
                normalize_edit_key(k),
                key(KeyCode::Backspace, ctrl),
                "{k:?}"
            );
        }
        // Everything else is left alone: typing, plain arrows, ctrl+arrows.
        for k in [
            key(KeyCode::Char('x'), KeyModifiers::NONE),
            key(KeyCode::Left, KeyModifiers::NONE),
            key(KeyCode::Left, ctrl),
            key(KeyCode::Char('a'), KeyModifiers::NONE),
            key(KeyCode::Enter, KeyModifiers::NONE),
        ] {
            assert_eq!(normalize_edit_key(k), k);
        }
    }

    #[test]
    fn an_input_field_edits_by_word() {
        let mut input = tui_input::Input::new("fix the build".into());
        edit_input(&mut input, &key(KeyCode::Backspace, KeyModifiers::CONTROL));
        assert_eq!(input.value(), "fix the ");
        edit_input(&mut input, &key(KeyCode::Left, KeyModifiers::CONTROL));
        assert_eq!(input.visual_cursor(), 4, "ctrl+left jumps a word");
        edit_input(&mut input, &key(KeyCode::Char('X'), KeyModifiers::SHIFT));
        assert_eq!(input.value(), "fix Xthe ");
    }

    #[test]
    fn the_word_before_the_cursor_goes_with_the_spaces_after_it() {
        let mut s = "fix the build".to_string();
        let end = s.len();
        let c = delete_word_left(&mut s, end);
        assert_eq!((s.as_str(), c), ("fix the ", 8));
        let c = delete_word_left(&mut s, c);
        assert_eq!((s.as_str(), c), ("fix ", 4));
        let c = delete_word_left(&mut s, c);
        assert_eq!((s.as_str(), c), ("", 0));
        let c = delete_word_left(&mut s, c);
        assert_eq!((s.as_str(), c), ("", 0), "nothing left to delete");
    }

    #[test]
    fn punctuation_is_skipped_then_the_word_goes() {
        let mut s = "a/b-c...".to_string();
        let end = s.len();
        let c = delete_word_left(&mut s, end);
        assert_eq!((s.as_str(), c), ("a/b-", 4));
    }

    #[test]
    fn it_works_in_the_middle_and_with_accents() {
        let mut s = "déjà vu ici".to_string();
        let cursor = "déjà vu".len();
        let c = delete_word_left(&mut s, cursor);
        assert_eq!(s, "déjà  ici");
        assert_eq!(c, "déjà ".len());
        let mut s = "un deux trois".to_string();
        delete_word_right(&mut s, 2);
        assert_eq!(s, "un trois");
    }
}

use std::ops::{Deref, DerefMut};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use rustc_hash::FxHashMap;
use serde::{de::Deserializer, Deserialize};

use crate::event::UserEvent;

const DEFAULT_KEY_BIND: &str = include_str!("../assets/default-keybind.toml");

/// Per-scope key map. Stores `KeyEvent → action_name` where the action
/// name is whatever string the view defined for that scope (e.g.
/// "approve", "merge", "edit_own"). Views resolve back to a typed enum
/// via their own `from_action_name` helper — that keeps action sets
/// per-view (so two scopes can both define `reload` without collision)
/// AND lets a typo in the TOML fail at config-load time instead of
/// silently no-op'ing at runtime.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ScopeBindings(FxHashMap<KeyEvent, String>);

impl Deref for ScopeBindings {
    type Target = FxHashMap<KeyEvent, String>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for ScopeBindings {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Full keybind set: the existing global map plus per-scope overrides.
/// Scope paths are dotted strings like "pr", "pr.list", "rebase.resume"
/// — the resolver walks parent scopes when a leaf doesn't bind the
/// key. Deref'ing yields the global `KeyBind` so existing call sites
/// (`ctx.keybind.get(&key)`, `ctx.keybind.keys_for_event(...)`, etc.)
/// keep working unchanged after AppContext switches the field type.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct KeyBinds {
    /// Existing global key → UserEvent map. Unchanged.
    pub global: KeyBind,
    /// Per-scope overrides. Keys are dotted scope paths
    /// ("pr.list", "rebase.resume", …). A binding in a child scope
    /// wins over the same key in a parent scope.
    pub scopes: FxHashMap<String, ScopeBindings>,
}

impl Deref for KeyBinds {
    type Target = KeyBind;
    fn deref(&self) -> &Self::Target {
        &self.global
    }
}

impl DerefMut for KeyBinds {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.global
    }
}

impl KeyBinds {
    /// Build the default bindings from the embedded TOML, then layer
    /// any user-provided overrides on top.
    ///
    /// Merge semantics are REPLACING per event/action — the same rule
    /// for both the global section and every scope. When the user
    /// specifies any keys for `event` (global) or `action` (scoped),
    /// every default key that pointed to that event/action is dropped
    /// FIRST, then the user's keys are inserted. So:
    ///
    /// * `navigate_up = ["x"]` rebinds NavigateUp to `x` ONLY — the
    ///   default `k` and `Up` are gone. To keep them too, write
    ///   `navigate_up = ["k", "up", "x"]` explicitly.
    /// * `approve = ["ctrl-a"]` rebinds approve in `[scope.pr]` to
    ///   `Ctrl+A` ONLY.
    /// * Untouched events/actions keep their full default key list.
    pub fn new(custom: Option<KeyBinds>) -> Self {
        let mut bundle: KeyBinds = toml::from_str(DEFAULT_KEY_BIND)
            .expect("default key bind should be correct");
        if let Some(custom) = custom {
            let overridden_events: rustc_hash::FxHashSet<UserEvent> =
                custom.global.0.values().copied().collect();
            bundle
                .global
                .0
                .retain(|_, ue| !overridden_events.contains(ue));
            for (key_event, user_event) in custom.global.0 {
                bundle.global.insert(key_event, user_event);
            }
            for (scope_path, sb) in custom.scopes {
                let target = bundle.scopes.entry(scope_path).or_default();
                let overridden: rustc_hash::FxHashSet<String> =
                    sb.0.values().cloned().collect();
                target.0.retain(|_, action| !overridden.contains(action));
                for (key_event, action) in sb.0 {
                    target.insert(key_event, action);
                }
            }
        }
        bundle
    }

    /// Look up a key in `scope` and its ancestor scopes, returning the
    /// deepest match. Walk order for `["pr", "list"]` is `pr.list` →
    /// `pr` → no-match. The global UserEvent map is NOT consulted here
    /// — views that hit `None` fall through to their existing global
    /// `UserEvent` dispatch.
    pub fn resolve_scoped(&self, scope: &[&str], key: KeyEvent) -> Option<&str> {
        for depth in (0..=scope.len()).rev() {
            if depth == 0 {
                break;
            }
            let path = scope[..depth].join(".");
            if let Some(sb) = self.scopes.get(&path) {
                if let Some(action) = sb.get(&key) {
                    return Some(action.as_str());
                }
            }
        }
        None
    }

    /// Footer/help-friendly single-key string for a scoped action.
    /// Picks the shortest non-empty binding so footers stay compact;
    /// ties go to the lexicographically-first name (`a` before `Ctrl+A`).
    /// Returns an empty string when the action is unbound — callers
    /// can either drop the hint or render the bare action name.
    pub fn primary_scoped_key(&self, scope: &[&str], action: &str) -> String {
        let mut keys = self.keys_for_scoped_action(scope, action);
        keys.sort_by(|a, b| a.chars().count().cmp(&b.chars().count()).then(a.cmp(b)));
        keys.into_iter().next().unwrap_or_default()
    }

    /// Same as `primary_scoped_key` but for the global UserEvent map.
    /// Used by footer hints to surface the user's actual bound key for
    /// generic actions like Confirm/Cancel rather than hardcoded
    /// `Enter`/`Esc` strings.
    pub fn primary_global_key(&self, event: UserEvent) -> String {
        let mut keys = self.global.keys_for_event(event);
        keys.sort_by(|a, b| a.chars().count().cmp(&b.chars().count()).then(a.cmp(b)));
        keys.into_iter().next().unwrap_or_default()
    }

    /// Reverse lookup for the help page: all keys bound to `action`
    /// in the leaf scope only. Help renderers can format these via
    /// `key_event_to_string`. Returns sorted strings for stable
    /// display ordering.
    pub fn keys_for_scoped_action(&self, scope: &[&str], action: &str) -> Vec<String> {
        let path = scope.join(".");
        let Some(sb) = self.scopes.get(&path) else {
            return Vec::new();
        };
        let mut keys: Vec<KeyEvent> = sb
            .iter()
            .filter(|(_, a)| a.as_str() == action)
            .map(|(k, _)| *k)
            .collect();
        keys.sort_by(|a, b| a.partial_cmp(b).unwrap());
        keys.into_iter().map(key_event_to_string).collect()
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct KeyBind(FxHashMap<KeyEvent, UserEvent>);

impl Deref for KeyBind {
    type Target = FxHashMap<KeyEvent, UserEvent>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for KeyBind {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl KeyBind {
    pub fn new(custom_keybind_patch: Option<KeyBind>) -> Self {
        let mut keybind: KeyBind =
            toml::from_str(DEFAULT_KEY_BIND).expect("default key bind should be correct");

        if let Some(mut custom_keybind_patch) = custom_keybind_patch {
            for (key_event, user_event) in custom_keybind_patch.drain() {
                keybind.insert(key_event, user_event);
            }
        }

        keybind
    }

    pub fn keys_for_event(&self, user_event: UserEvent) -> Vec<String> {
        let mut key_events: Vec<KeyEvent> = self
            .iter()
            .filter(|(_, ue)| **ue == user_event)
            .map(|(ke, _)| *ke)
            .collect();
        key_events.sort_by(|a, b| a.partial_cmp(b).unwrap()); // At least when used for key bindings, it doesn't seem to be a problem...
        key_events.into_iter().map(key_event_to_string).collect()
    }

    pub fn user_command_event_numbers(&self) -> Vec<usize> {
        let mut numbers: Vec<usize> = self
            .values()
            .filter_map(|ue| {
                if let UserEvent::UserCommand(n) = ue {
                    Some(*n)
                } else {
                    None
                }
            })
            .collect();
        numbers.sort_unstable();
        numbers
    }
}

impl<'de> Deserialize<'de> for KeyBinds {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Walk the input table:
        //   • top-level `event_name = ["k", "shift-x"]`  → global
        //   • top-level `event_name = { ... nested ... }` → scope sub-tree
        // Scope sub-trees can themselves nest one level deeper (e.g.
        // `[pr.conversation]` inside `[pr]`), which TOML flattens into
        // a separate `pr.conversation` table at this level — so a
        // single walk over the keys is enough.
        let table = toml::value::Table::deserialize(deserializer)?;
        let mut bundle = KeyBinds::default();

        // Walk top-level:
        //   • `event_name = ["k", ...]`            → global UserEvent map.
        //   • `[scope.<path>]` sub-tables          → per-view scope maps.
        //
        // The `[scope]` wrapper is intentional: TOML forbids a key from
        // being both a value (`issues = ["shift-i"]`) and a sub-table
        // (`[issues]`) at the same level. Putting all scopes under
        // `[scope]` shields scope names from ever colliding with a
        // global event name — current or future.
        let mut scopes_input: FxHashMap<String, toml::value::Table> = FxHashMap::default();
        let mut globals_input: FxHashMap<String, Vec<String>> = FxHashMap::default();
        for (k, v) in table {
            if k == "scope" {
                let toml::Value::Table(scope_tree) = v else {
                    return Err(serde::de::Error::custom(
                        "keybind.scope must be a table of scope sub-tables",
                    ));
                };
                for (scope_name, sub) in scope_tree {
                    let toml::Value::Table(sub_table) = sub else {
                        return Err(serde::de::Error::custom(format!(
                            "keybind.scope.{scope_name}: expected a table"
                        )));
                    };
                    flatten_scope_tables(&scope_name, sub_table, &mut scopes_input);
                }
                continue;
            }
            match v {
                toml::Value::Array(_) => {
                    let keys: Vec<String> = v.try_into().map_err(serde::de::Error::custom)?;
                    globals_input.insert(k, keys);
                }
                _ => {
                    return Err(serde::de::Error::custom(format!(
                        "keybind: `{k}` must be a key list (use [scope.<name>] for view bindings)"
                    )));
                }
            }
        }

        // Build the global section by serializing back to TOML and
        // reusing the existing KeyBind deserializer (covers UserEvent
        // name parsing, key string parsing, and conflict detection).
        let globals_toml = toml::Value::try_from(globals_input).map_err(serde::de::Error::custom)?;
        bundle.global = globals_toml.try_into().map_err(serde::de::Error::custom)?;

        // Build each scope. Per-scope conflict detection (two events
        // bound to the same key WITHIN one scope → error). No cross-
        // scope conflict — that's the whole point of the design.
        for (path, scope_table) in scopes_input {
            let mut sb = ScopeBindings::default();
            for (action_name, keys_value) in scope_table {
                let keys: Vec<String> = keys_value.try_into().map_err(|e| {
                    serde::de::Error::custom(format!(
                        "keybind.{path}.{action_name}: {e}"
                    ))
                })?;
                for raw in keys {
                    let key_event = parse_key_event(&raw).map_err(|s| {
                        serde::de::Error::custom(format!(
                            "keybind.{path}: invalid key {raw:?}: {s}"
                        ))
                    })?;
                    if let Some(prev) = sb.insert(key_event, action_name.clone()) {
                        return Err(serde::de::Error::custom(format!(
                            "keybind.{path}: key {} is bound to both `{}` and `{}` — pick one",
                            key_event_to_string(key_event),
                            action_name,
                            prev,
                        )));
                    }
                }
            }
            bundle.scopes.insert(path, sb);
        }

        Ok(bundle)
    }
}

/// Flatten one level of nested scope tables. A `[pr]` section may itself
/// contain `[pr.list]` and `[pr.conversation]` sub-tables in the source
/// TOML; the standard TOML parse already lifts those to siblings at the
/// `pr` level when accessed as `Table`, so we walk one ply and re-key
/// them as `"pr.list"`, etc.
fn flatten_scope_tables(
    parent_path: &str,
    table: toml::value::Table,
    out: &mut FxHashMap<String, toml::value::Table>,
) {
    let mut own = toml::value::Table::new();
    for (k, v) in table {
        match v {
            toml::Value::Table(sub) => {
                let child_path = format!("{parent_path}.{k}");
                flatten_scope_tables(&child_path, sub, out);
            }
            other => {
                own.insert(k, other);
            }
        }
    }
    if !own.is_empty() {
        out.insert(parent_path.to_string(), own);
    }
}

impl<'de> Deserialize<'de> for KeyBind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let parsed_map = FxHashMap::<UserEvent, Vec<String>>::deserialize(deserializer)?;
        let mut key_map = FxHashMap::<KeyEvent, UserEvent>::default();
        for (user_event, key_events) in parsed_map {
            for key_event_str in key_events {
                let key_event = match parse_key_event(&key_event_str) {
                    Ok(e) => e,
                    Err(s) => {
                        let msg = format!("invalid key {key_event_str:?}: {s}");
                        return Err(serde::de::Error::custom(msg));
                    }
                };
                if let Some(conflict_user_event) = key_map.insert(key_event, user_event) {
                    // Render both events + the conflicting key as
                    // user-facing strings instead of `KeyEvent { ... }`
                    // Debug noise — the diagnostic prints this
                    // message verbatim under the brand splash.
                    let msg = format!(
                        "key {:?} is bound to both `{:?}` and `{:?}` — only one event per key",
                        key_event_to_string(key_event),
                        user_event,
                        conflict_user_event,
                    );
                    return Err(serde::de::Error::custom(msg));
                }
            }
        }

        Ok(KeyBind(key_map))
    }
}

fn parse_key_event(raw: &str) -> Result<KeyEvent, String> {
    let raw_lower = raw.to_ascii_lowercase().replace(' ', "");
    let (remaining, modifiers) = extract_modifiers(&raw_lower);
    // Cap at one user-supplied modifier — the app deliberately speaks
    // 2-key combos only (ctrl-a, alt-x, shift-r). Chains like
    // `ctrl-shift-a` aren't allowed in user configs, both to keep the
    // display strings short (footers show `Ctrl+a`, not `Ctrl+Shift+A`)
    // and because most terminals don't propagate three-key combos
    // reliably anyway.
    if modifiers.bits().count_ones() > 1 {
        // The caller (`Deserialize`) already prefixes the raw string,
        // so we only emit the explanation here — avoids the awkward
        // `"ctrl-shift-a": "ctrl-shift-a": only one modifier…` echo.
        return Err(
            "only one modifier prefix is allowed — use ctrl-X, alt-X or shift-X, \
             not chained combos"
                .to_string(),
        );
    }
    parse_key_code_with_modifiers(remaining, modifiers)
}

fn extract_modifiers(raw: &str) -> (&str, KeyModifiers) {
    let mut modifiers = KeyModifiers::empty();
    let mut current = raw;

    loop {
        match current {
            rest if rest.starts_with("ctrl-") => {
                modifiers.insert(KeyModifiers::CONTROL);
                current = &rest[5..];
            }
            rest if rest.starts_with("alt-") => {
                modifiers.insert(KeyModifiers::ALT);
                current = &rest[4..];
            }
            rest if rest.starts_with("shift-") => {
                modifiers.insert(KeyModifiers::SHIFT);
                current = &rest[6..];
            }
            _ => break, // break out of the loop if no known prefix is detected
        };
    }

    (current, modifiers)
}

fn parse_key_code_with_modifiers(
    raw: &str,
    mut modifiers: KeyModifiers,
) -> Result<KeyEvent, String> {
    let c = match raw {
        "esc" => KeyCode::Esc,
        "enter" => KeyCode::Enter,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        "backtab" => {
            modifiers.insert(KeyModifiers::SHIFT);
            KeyCode::BackTab
        }
        "backspace" => KeyCode::Backspace,
        "delete" => KeyCode::Delete,
        "insert" => KeyCode::Insert,
        "f1" => KeyCode::F(1),
        "f2" => KeyCode::F(2),
        "f3" => KeyCode::F(3),
        "f4" => KeyCode::F(4),
        "f5" => KeyCode::F(5),
        "f6" => KeyCode::F(6),
        "f7" => KeyCode::F(7),
        "f8" => KeyCode::F(8),
        "f9" => KeyCode::F(9),
        "f10" => KeyCode::F(10),
        "f11" => KeyCode::F(11),
        "f12" => KeyCode::F(12),
        "space" => KeyCode::Char(' '),
        "hyphen" => KeyCode::Char('-'),
        "minus" => KeyCode::Char('-'),
        "tab" => KeyCode::Tab,
        c if c.len() == 1 => {
            let mut c = c.chars().next().unwrap();
            if modifiers.contains(KeyModifiers::SHIFT) {
                c = c.to_ascii_uppercase();
            }
            KeyCode::Char(c)
        }
        _ => return Err(format!("Unable to parse {raw}")),
    };
    Ok(KeyEvent::new(c, modifiers))
}

fn key_event_to_string(key_event: KeyEvent) -> String {
    if let KeyCode::Char(c) = key_event.code {
        if key_event.modifiers == KeyModifiers::SHIFT {
            return c.to_ascii_uppercase().into();
        }
    }

    let char;
    let key_code = match key_event.code {
        KeyCode::Backspace => "Backspace",
        KeyCode::Enter => "Enter",
        KeyCode::Left => "Left",
        KeyCode::Right => "Right",
        KeyCode::Up => "Up",
        KeyCode::Down => "Down",
        KeyCode::Home => "Home",
        KeyCode::End => "End",
        KeyCode::PageUp => "PageUp",
        KeyCode::PageDown => "PageDown",
        KeyCode::Tab => "Tab",
        KeyCode::BackTab => "BackTab",
        KeyCode::Delete => "Delete",
        KeyCode::Insert => "Insert",
        KeyCode::F(n) => {
            char = format!("F{n}");
            &char
        }
        KeyCode::Char(' ') => "Space",
        KeyCode::Char(c) => {
            char = c.to_string();
            &char
        }
        KeyCode::Esc => "Esc",
        KeyCode::Null => "",
        KeyCode::CapsLock => "",
        KeyCode::Menu => "",
        KeyCode::ScrollLock => "",
        KeyCode::Media(_) => "",
        KeyCode::NumLock => "",
        KeyCode::PrintScreen => "",
        KeyCode::Pause => "",
        KeyCode::KeypadBegin => "",
        KeyCode::Modifier(_) => "",
    };

    let mut modifiers = Vec::with_capacity(3);

    if key_event.modifiers.intersects(KeyModifiers::CONTROL) {
        modifiers.push("Ctrl");
    }

    if key_event.modifiers.intersects(KeyModifiers::SHIFT) {
        modifiers.push("Shift");
    }

    if key_event.modifiers.intersects(KeyModifiers::ALT) {
        modifiers.push("Alt");
    }

    let mut key = modifiers.join("+");

    if !key.is_empty() {
        key.push('+');
    }
    key.push_str(key_code);

    key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[rustfmt::skip]
    #[test]
    fn test_deserialize_keybind() {
        let toml = r#"
            navigate_up = ["k"]
            navigate_down = ["j", "down"]
            navigate_left = ["ctrl-h", "shift-h", "alt-h"]
            navigate_right = ["ctrl-l", "alt-l"]
            quit = ["esc", "f12"]
            user_command_1 = ["d"]
            user_command_view_toggle_10 = ["e"]
        "#;

        let expected = KeyBind(
            [
                (
                    KeyEvent::new(KeyCode::Char('k'), KeyModifiers::empty()),
                    UserEvent::NavigateUp,
                ),
                (
                    KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty()),
                    UserEvent::NavigateDown,
                ),
                (
                    KeyEvent::new(KeyCode::Down, KeyModifiers::empty()),
                    UserEvent::NavigateDown,
                ),
                (
                    KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL),
                    UserEvent::NavigateLeft,
                ),
                (
                    KeyEvent::new(KeyCode::Char('h'), KeyModifiers::SHIFT),
                    UserEvent::NavigateLeft,
                ),
                (
                    KeyEvent::new(KeyCode::Char('h'), KeyModifiers::ALT),
                    UserEvent::NavigateLeft,
                ),
                (
                    KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL),
                    UserEvent::NavigateRight,
                ),
                (
                    KeyEvent::new(KeyCode::Char('l'), KeyModifiers::ALT),
                    UserEvent::NavigateRight,
                ),
                (
                    KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()),
                    UserEvent::Quit,
                ),
                (
                    KeyEvent::new(KeyCode::F(12), KeyModifiers::empty()),
                    UserEvent::Quit,
                ),
                (
                    KeyEvent::new(KeyCode::Char('d'), KeyModifiers::empty()),
                    UserEvent::UserCommand(1),
                ),
                (
                    KeyEvent::new(KeyCode::Char('e'), KeyModifiers::empty()),
                    UserEvent::UserCommand(10),
                ),
            ]
            .into_iter()
            .collect(),
        );

        let actual: KeyBind = toml::from_str(toml).unwrap();

        assert_eq!(actual, expected);
    }

    #[rustfmt::skip]
    #[test]
    fn test_key_event_to_string() {
        let key_event = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::empty());
        assert_eq!(key_event_to_string(key_event), "k");

        let key_event = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty());
        assert_eq!(key_event_to_string(key_event), "j");

        let key_event = KeyEvent::new(KeyCode::Down, KeyModifiers::empty());
        assert_eq!(key_event_to_string(key_event), "Down");

        let key_event = KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL);
        assert_eq!(key_event_to_string(key_event), "Ctrl+h");

        let key_event = KeyEvent::new(KeyCode::Char('h'), KeyModifiers::SHIFT);
        assert_eq!(key_event_to_string(key_event), "H");

        let key_event = KeyEvent::new(KeyCode::Char('H'), KeyModifiers::SHIFT);
        assert_eq!(key_event_to_string(key_event), "H");

        let key_event = KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT);
        assert_eq!(key_event_to_string(key_event), "Shift+Left");

        let key_event = KeyEvent::new(KeyCode::Char('h'), KeyModifiers::ALT);
        assert_eq!(key_event_to_string(key_event), "Alt+h");

        // Chained modifiers can still be CONSTRUCTED via the
        // crossterm-side runtime (BackTab adds SHIFT internally on top
        // of a Ctrl modifier, for instance) — the formatter must
        // therefore still render them sanely, even though the TOML
        // parser refuses to accept them as user input.
        let key_event = KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL | KeyModifiers::SHIFT);
        assert_eq!(key_event_to_string(key_event), "Ctrl+Shift+l");

        let key_event = KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL | KeyModifiers::SHIFT | KeyModifiers::ALT);
        assert_eq!(key_event_to_string(key_event), "Ctrl+Shift+Alt+l");

        let key_event = KeyEvent::new(KeyCode::Esc, KeyModifiers::empty());
        assert_eq!(key_event_to_string(key_event), "Esc");

        let key_event = KeyEvent::new(KeyCode::F(12), KeyModifiers::empty());
        assert_eq!(key_event_to_string(key_event), "F12");
    }

    #[test]
    fn parser_rejects_chained_modifiers() {
        // 2+ modifier prefixes → config-load error. The cap keeps the
        // app firmly in "max 2 keystrokes" territory and avoids the
        // unreliable Ctrl+Shift+letter terminal handling.
        let toml = r#"navigate_up = ["ctrl-shift-a"]"#;
        let err = toml::from_str::<KeyBind>(toml).unwrap_err();
        assert!(
            err.to_string().contains("only one modifier"),
            "want modifier-cap message, got: {err}"
        );

        let toml = r#"navigate_up = ["alt-shift-ctrl-l"]"#;
        let err = toml::from_str::<KeyBind>(toml).unwrap_err();
        assert!(err.to_string().contains("only one modifier"));
    }

    #[test]
    fn keybinds_parse_global_only() {
        // Old format with no scope sections must still produce a
        // KeyBinds with populated `global` and empty `scopes`.
        let toml = r#"
            navigate_up = ["k"]
            quit = ["esc", "q"]
        "#;
        let kb: KeyBinds = toml::from_str(toml).unwrap();
        assert_eq!(kb.scopes.len(), 0);
        assert_eq!(kb.global.len(), 3);
        assert_eq!(
            kb.global
                .get(&KeyEvent::new(KeyCode::Esc, KeyModifiers::empty())),
            Some(&UserEvent::Quit)
        );
    }

    #[test]
    fn keybinds_parse_scoped_sections() {
        let toml = r#"
            quit = ["q"]

            [scope.pr]
            approve = ["a"]
            merge = ["m"]

            [scope.pr.conversation]
            new_comment = ["c"]
            quote_reply = ["shift-r"]

            [scope.rebase]
            pick = ["p"]
            drop = ["d"]
        "#;
        let kb: KeyBinds = toml::from_str(toml).unwrap();
        assert!(kb.scopes.contains_key("pr"));
        assert!(kb.scopes.contains_key("pr.conversation"));
        assert!(kb.scopes.contains_key("rebase"));
        assert_eq!(
            kb.scopes["pr"]
                .get(&KeyEvent::new(KeyCode::Char('a'), KeyModifiers::empty())),
            Some(&"approve".to_string())
        );
        assert_eq!(
            kb.scopes["pr.conversation"]
                .get(&KeyEvent::new(KeyCode::Char('R'), KeyModifiers::SHIFT)),
            Some(&"quote_reply".to_string())
        );
    }

    #[test]
    fn resolve_scoped_walks_parent_chain() {
        let toml = r#"
            [scope.pr]
            approve = ["a"]

            [scope.pr.conversation]
            new_comment = ["c"]
        "#;
        let kb: KeyBinds = toml::from_str(toml).unwrap();
        let a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::empty());
        let c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::empty());
        let z = KeyEvent::new(KeyCode::Char('z'), KeyModifiers::empty());

        // `a` exists in `pr` only — looking up via `pr.conversation`
        // must walk up to `pr` and find it.
        assert_eq!(kb.resolve_scoped(&["pr", "conversation"], a), Some("approve"));
        assert_eq!(kb.resolve_scoped(&["pr"], a), Some("approve"));
        // `c` is only in the leaf scope, parent doesn't see it.
        assert_eq!(
            kb.resolve_scoped(&["pr", "conversation"], c),
            Some("new_comment")
        );
        assert_eq!(kb.resolve_scoped(&["pr"], c), None);
        // unknown key returns None at any depth.
        assert_eq!(kb.resolve_scoped(&["pr", "conversation"], z), None);
    }

    #[test]
    fn resolve_scoped_leaf_wins_over_parent() {
        // Same key bound to different actions in parent + child:
        // the child wins because the resolver walks deepest-first.
        let toml = r#"
            [scope.pr]
            approve = ["a"]

            [scope.pr.conversation]
            assignees_picker = ["a"]
        "#;
        let kb: KeyBinds = toml::from_str(toml).unwrap();
        let a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::empty());
        assert_eq!(
            kb.resolve_scoped(&["pr", "conversation"], a),
            Some("assignees_picker")
        );
        assert_eq!(kb.resolve_scoped(&["pr"], a), Some("approve"));
    }

    #[test]
    fn keybinds_per_scope_conflict_rejected() {
        // Two actions binding the same key WITHIN one scope → error.
        let toml = r#"
            [scope.pr]
            approve = ["a"]
            merge   = ["a"]
        "#;
        let err = toml::from_str::<KeyBinds>(toml).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("pr"), "want scope name in error, got: {msg}");
        assert!(msg.contains("bound to both"), "got: {msg}");
    }

    #[test]
    fn keybinds_cross_scope_same_key_is_ok() {
        // `a` bound globally AND scoped should NOT error — that's
        // the whole point of scoping.
        let toml = r#"
            stage = ["a"]

            [scope.pr]
            approve = ["a"]
        "#;
        let kb: KeyBinds = toml::from_str(toml).unwrap();
        let a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::empty());
        assert_eq!(kb.global.get(&a), Some(&UserEvent::Stage));
        assert_eq!(kb.resolve_scoped(&["pr"], a), Some("approve"));
    }

    #[test]
    fn user_override_replaces_action_keys_in_scope() {
        // User remaps `approve` from `a` to `ctrl-a`. Because scope
        // overrides are REPLACING (not additive), the default `a →
        // approve` is dropped — only `Ctrl+A` triggers approve now.
        let user_toml = r#"
            [scope.pr]
            approve = ["ctrl-a"]
        "#;
        let user: KeyBinds = toml::from_str(user_toml).unwrap();
        let kb = KeyBinds::new(Some(user));

        let ctrl_a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL);
        let a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::empty());

        assert_eq!(kb.resolve_scoped(&["pr"], ctrl_a), Some("approve"));
        // Default `a` no longer triggers approve.
        assert_eq!(kb.resolve_scoped(&["pr"], a), None);

        // Same-scope actions the user DIDN'T touch keep their defaults.
        let m = KeyEvent::new(KeyCode::Char('m'), KeyModifiers::empty());
        assert_eq!(kb.resolve_scoped(&["pr"], m), Some("merge"));

        // Unrelated scopes untouched.
        let ctrl_s = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert_eq!(kb.resolve_scoped(&["compose"], ctrl_s), Some("submit"));
    }

    #[test]
    fn user_override_keeps_multiple_user_keys() {
        // User explicitly lists 2 keys — both work, the default is gone.
        let user_toml = r#"
            [scope.pr]
            approve = ["ctrl-a", "shift-a"]
        "#;
        let user: KeyBinds = toml::from_str(user_toml).unwrap();
        let kb = KeyBinds::new(Some(user));

        let ctrl_a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL);
        let shift_a = KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT);
        let a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::empty());

        assert_eq!(kb.resolve_scoped(&["pr"], ctrl_a), Some("approve"));
        assert_eq!(kb.resolve_scoped(&["pr"], shift_a), Some("approve"));
        assert_eq!(kb.resolve_scoped(&["pr"], a), None);
    }

    #[test]
    fn embedded_defaults_have_all_expected_scopes() {
        // KeyBinds::new(None) bootstraps from `assets/default-keybind.toml`.
        // Catch silently-renamed or removed scopes in CI rather than at
        // runtime when a user hits a bound key and nothing happens.
        let kb = KeyBinds::new(None);
        for scope in [
            "pr",
            "pr.list",
            "pr.conversation",
            "issues",
            "issues.list",
            "issues.detail",
            "rebase",
            "rebase.resume",
            "rebase.reword_editor",
            "conflict",
            "compose",
        ] {
            assert!(
                kb.scopes.contains_key(scope),
                "default-keybind.toml lost scope {scope}"
            );
        }
        // Spot-check one binding per scope to confirm action names
        // round-trip the way the views expect them.
        let a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::empty());
        assert_eq!(kb.resolve_scoped(&["pr"], a), Some("approve"));
        assert_eq!(kb.resolve_scoped(&["issues"], a), Some("assignees_picker"));
        let ctrl_s = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert_eq!(kb.resolve_scoped(&["compose"], ctrl_s), Some("submit"));
    }

    #[test]
    fn keys_for_scoped_action_returns_sorted_strings() {
        let toml = r#"
            [scope.pr]
            approve = ["a", "ctrl-a"]
        "#;
        let kb: KeyBinds = toml::from_str(toml).unwrap();
        let keys = kb.keys_for_scoped_action(&["pr"], "approve");
        assert_eq!(keys, vec!["a", "Ctrl+a"]);
    }
}

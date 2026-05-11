use ratatui::style::Color as RatatuiColor;
use serde::Deserialize;
use smart_default::SmartDefault;
use umbra::optional;

use crate::config::GraphColorConfig;

#[optional(derives = [Deserialize], visibility = pub)]
#[derive(Debug, Clone, PartialEq, Eq, SmartDefault)]
pub struct ColorTheme {
    #[default(RatatuiColor::Rgb(0xc0, 0xca, 0xf5))]
    pub fg: RatatuiColor,
    #[default(RatatuiColor::Rgb(0x1a, 0x1b, 0x26))]
    pub bg: RatatuiColor,

    #[default(RatatuiColor::White)]
    pub list_selected_fg: RatatuiColor,
    #[default(RatatuiColor::DarkGray)]
    pub list_selected_bg: RatatuiColor,
    /// Foreground/background for a commit row marked as the first endpoint of
    /// the 2-commit comparison flow (Space / Ctrl+click). Each theme defines
    /// a more saturated, accent-colored variant of its selection palette so
    /// the marked row stands out clearly while still feeling theme-coherent.
    #[default(RatatuiColor::White)]
    pub list_compare_marked_fg: RatatuiColor,
    #[default(RatatuiColor::Rgb(0x6e, 0x55, 0xab))]
    pub list_compare_marked_bg: RatatuiColor,
    #[default(RatatuiColor::Yellow)]
    pub list_ref_paren_fg: RatatuiColor,
    #[default(RatatuiColor::Green)]
    pub list_ref_branch_fg: RatatuiColor,
    #[default(RatatuiColor::Red)]
    pub list_ref_remote_branch_fg: RatatuiColor,
    #[default(RatatuiColor::Yellow)]
    pub list_ref_tag_fg: RatatuiColor,
    #[default(RatatuiColor::Magenta)]
    pub list_ref_stash_fg: RatatuiColor,
    #[default(RatatuiColor::Cyan)]
    pub list_head_fg: RatatuiColor,
    #[default(RatatuiColor::Rgb(0xb4, 0xbd, 0xe5))]
    pub list_commit_message_fg: RatatuiColor,
    #[default(RatatuiColor::Cyan)]
    pub list_name_fg: RatatuiColor,
    #[default(RatatuiColor::Yellow)]
    pub list_hash_fg: RatatuiColor,
    #[default(RatatuiColor::Magenta)]
    pub list_date_fg: RatatuiColor,
    #[default(RatatuiColor::Black)]
    pub list_match_fg: RatatuiColor,
    #[default(RatatuiColor::Yellow)]
    pub list_match_bg: RatatuiColor,

    #[default(RatatuiColor::Reset)]
    pub detail_label_fg: RatatuiColor,
    #[default(RatatuiColor::Cyan)]
    pub detail_name_fg: RatatuiColor,
    #[default(RatatuiColor::Magenta)]
    pub detail_date_fg: RatatuiColor,
    #[default(RatatuiColor::Blue)]
    pub detail_email_fg: RatatuiColor,
    #[default(RatatuiColor::Yellow)]
    pub detail_hash_fg: RatatuiColor,
    #[default(RatatuiColor::Green)]
    pub detail_ref_branch_fg: RatatuiColor,
    #[default(RatatuiColor::Red)]
    pub detail_ref_remote_branch_fg: RatatuiColor,
    #[default(RatatuiColor::Yellow)]
    pub detail_ref_tag_fg: RatatuiColor,
    #[default(RatatuiColor::Rgb(0x9e, 0xce, 0x6a))]
    pub detail_file_change_add_fg: RatatuiColor,
    #[default(RatatuiColor::Rgb(0xe0, 0xaf, 0x68))]
    pub detail_file_change_modify_fg: RatatuiColor,
    #[default(RatatuiColor::Rgb(0xf7, 0x76, 0x8e))]
    pub detail_file_change_delete_fg: RatatuiColor,
    #[default(RatatuiColor::Rgb(0x7d, 0xcf, 0xff))]
    pub detail_file_change_move_fg: RatatuiColor,

    #[default(RatatuiColor::White)]
    pub ref_selected_fg: RatatuiColor,
    #[default(RatatuiColor::DarkGray)]
    pub ref_selected_bg: RatatuiColor,

    #[default(RatatuiColor::Green)]
    pub help_block_title_fg: RatatuiColor,
    #[default(RatatuiColor::Yellow)]
    pub help_key_fg: RatatuiColor,

    #[default(RatatuiColor::Reset)]
    pub virtual_cursor_fg: RatatuiColor,
    #[default(RatatuiColor::Reset)]
    pub status_input_fg: RatatuiColor,
    #[default(RatatuiColor::DarkGray)]
    pub status_input_transient_fg: RatatuiColor,
    #[default(RatatuiColor::Cyan)]
    pub status_info_fg: RatatuiColor,
    #[default(RatatuiColor::Green)]
    pub status_success_fg: RatatuiColor,
    #[default(RatatuiColor::Yellow)]
    pub status_warn_fg: RatatuiColor,
    #[default(RatatuiColor::Red)]
    pub status_error_fg: RatatuiColor,

    #[default(RatatuiColor::DarkGray)]
    pub divider_fg: RatatuiColor,

    /// Per-theme graph-branch palette. Empty means "no override" — the loader
    /// then falls back to `core_config.graph.color.branches` from `[graph.color]`
    /// in the user's TOML. Each shipped theme fills this with its own
    /// 12-colour palette so the commit-graph visually matches its world
    /// (Dracula greens/pinks, Gruvbox earth tones, etc.). Imported themes
    /// can specify their own palette by listing hex strings here.
    #[default(Vec::<String>::new())]
    pub graph_branches: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphColor {
    r: u8,
    g: u8,
    b: u8,
    a: u8,
}

impl GraphColor {
    pub fn from_rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        Self::from_rgba(r, g, b, 255)
    }

    pub fn to_image_color(self) -> image::Rgba<u8> {
        image::Rgba([self.r, self.g, self.b, self.a])
    }

    pub fn to_ratatui_color(self) -> RatatuiColor {
        RatatuiColor::Rgb(self.r, self.g, self.b)
    }

    fn transparent() -> Self {
        Self::from_rgba(0, 0, 0, 0)
    }
}

#[derive(Debug, Clone)]
pub struct GraphColorSet {
    pub colors: Vec<GraphColor>,
    pub edge_color: GraphColor,
    pub background_color: GraphColor,
}

impl GraphColorSet {
    pub fn new(config: &GraphColorConfig) -> Self {
        let colors = config
            .branches
            .iter()
            .filter_map(|s| parse_rgba_color(s))
            .collect();
        let edge_color = parse_rgba_color(&config.edge).unwrap_or(GraphColor::transparent());
        let background_color =
            parse_rgba_color(&config.background).unwrap_or(GraphColor::transparent());

        Self {
            colors,
            edge_color,
            background_color,
        }
    }

    pub fn get(&self, index: usize) -> GraphColor {
        self.colors[index % self.colors.len()]
    }
}

/// Build a `GraphColorSet` from the current theme + graph config in a single
/// place — used both on initial load and every time a theme cycles. Centralises
/// two pieces of logic that used to be inlined: (1) theme-provided
/// `graph_branches` win over `[graph.color.branches]` from the user TOML, and
/// (2) a transparent `[graph.color.background]` is filled with the theme's
/// `bg` so kitty composites against the right colour instead of the terminal's
/// native bg.
pub fn build_graph_color_set(
    theme: &ColorTheme,
    config: &crate::config::GraphColorConfig,
) -> GraphColorSet {
    let mut effective = config.clone();
    if !theme.graph_branches.is_empty() {
        effective.branches = theme.graph_branches.clone();
    }
    if effective.background == "#00000000" {
        if let RatatuiColor::Rgb(r, g, b) = theme.bg {
            effective.background = format!("#{:02x}{:02x}{:02x}ff", r, g, b);
        }
    }
    GraphColorSet::new(&effective)
}

fn parse_rgba_color(s: &str) -> Option<GraphColor> {
    if !s.starts_with('#') {
        return None;
    }

    let s = &s[1..];
    let l = s.len();
    if l != 6 && l != 8 {
        return None;
    }

    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    if l == 6 {
        Some(GraphColor::from_rgb(r, g, b))
    } else {
        let a = u8::from_str_radix(&s[6..8], 16).ok()?;
        Some(GraphColor::from_rgba(r, g, b, a))
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case("#ff0000", Some(GraphColor { r: 255, g: 0, b: 0, a: 255}))]
    #[case("#AABBCCDD", Some(GraphColor { r: 170, g: 187, b: 204, a: 221}))]
    #[case("#ff000", None)]
    #[case("#fff", None)]
    #[case("000000", None)]
    #[case("##123456", None)]
    fn test_parse_rgba_color(#[case] input: &str, #[case] expected: Option<GraphColor>) {
        assert_eq!(parse_rgba_color(input), expected);
    }

    /// Baseline for `build_graph_color_set` — pinned ahead of the upcoming
    /// "cache the parsed palette" optimisation. These tests freeze the
    /// theme-vs-config precedence and the transparent-bg patching logic so
    /// the cache layer can't silently flip any case.
    mod build_graph_color_set_baseline {
        use super::*;
        use crate::config::GraphColorConfig;

        fn cfg_with_branches(branches: Vec<&str>) -> GraphColorConfig {
            let mut c = GraphColorConfig::default();
            c.branches = branches.into_iter().map(|s| s.to_string()).collect();
            c.background = "#222222".into();
            c
        }

        #[test]
        fn theme_branches_override_config_when_non_empty() {
            let mut theme = ColorTheme::default();
            theme.graph_branches = vec!["#aabbcc".into(), "#ddeeff".into()];
            let cfg = cfg_with_branches(vec!["#111111", "#222222", "#333333"]);

            let set = build_graph_color_set(&theme, &cfg);

            // Theme palette won — 2 entries, not the 3 from the config.
            assert_eq!(set.colors.len(), 2);
            assert_eq!(set.get(0), GraphColor::from_rgb(0xaa, 0xbb, 0xcc));
            assert_eq!(set.get(1), GraphColor::from_rgb(0xdd, 0xee, 0xff));
        }

        #[test]
        fn empty_theme_branches_falls_back_to_config() {
            let theme = ColorTheme::default(); // graph_branches = Vec::new()
            let cfg = cfg_with_branches(vec!["#111111", "#222222", "#333333"]);

            let set = build_graph_color_set(&theme, &cfg);

            assert_eq!(set.colors.len(), 3);
            assert_eq!(set.get(0), GraphColor::from_rgb(0x11, 0x11, 0x11));
            assert_eq!(set.get(2), GraphColor::from_rgb(0x33, 0x33, 0x33));
        }

        #[test]
        fn transparent_background_is_patched_with_theme_bg() {
            let mut theme = ColorTheme::default();
            theme.bg = RatatuiColor::Rgb(0x1a, 0x1b, 0x26);
            let mut cfg = cfg_with_branches(vec!["#ffffff"]);
            cfg.background = "#00000000".into(); // explicit transparent sentinel

            let set = build_graph_color_set(&theme, &cfg);

            // Patched: theme bg with full alpha.
            assert_eq!(
                set.background_color,
                GraphColor::from_rgba(0x1a, 0x1b, 0x26, 0xff)
            );
        }

        #[test]
        fn explicit_background_is_kept_unchanged() {
            let mut theme = ColorTheme::default();
            theme.bg = RatatuiColor::Rgb(0x1a, 0x1b, 0x26);
            let mut cfg = cfg_with_branches(vec!["#ffffff"]);
            cfg.background = "#abcdef".into();

            let set = build_graph_color_set(&theme, &cfg);

            assert_eq!(
                set.background_color,
                GraphColor::from_rgb(0xab, 0xcd, 0xef)
            );
        }

        #[test]
        fn non_rgb_theme_bg_skips_patching() {
            let mut theme = ColorTheme::default();
            theme.bg = RatatuiColor::Reset; // not an RGB triple
            let mut cfg = cfg_with_branches(vec!["#ffffff"]);
            cfg.background = "#00000000".into();

            let set = build_graph_color_set(&theme, &cfg);

            // Stays transparent — patching only fires on RGB bg.
            assert_eq!(set.background_color, GraphColor::transparent());
        }
    }
}

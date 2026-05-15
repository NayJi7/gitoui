use crate::color::ColorTheme;
use ratatui::style::Color;

#[derive(Debug)]
pub struct ThemeDefinition {
    pub color_theme: ColorTheme,
    /// Name of the syntect syntax theme to use for diff highlighting.
    pub syntax_theme: String,
}

pub fn list_themes() -> &'static [&'static str] {
    &[
        "Tokyo Night",
        "Dracula",
        "Catppuccin Mocha",
        "Catppuccin Latte",
        "Gruvbox Dark",
        "Nord",
        "Solarized Dark",
        "Solarized Light",
        "One Dark",
        "Monokai Pro",
    ]
}

/// Names of every custom theme file found under `<config_dir>/themes/`.
/// Returns the file stems (no `.toml`), sorted case-insensitively.
/// Silently returns an empty vec if the dir doesn't exist, isn't readable,
/// or there's no config dir at all — the config view treats this as
/// "no custom themes available", same as the empty built-ins case.
pub fn discover_custom_themes() -> Vec<String> {
    let Some(config_path) = crate::config::resolve_config_file_path() else {
        return Vec::new();
    };
    let Some(dir) = config_path.parent().map(|p| p.join(THEMES_DIR_NAME)) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let builtins: std::collections::HashSet<&str> = list_themes().iter().copied().collect();
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("toml"))
        .filter_map(|e| {
            e.path()
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_owned)
        })
        // Built-in names take precedence — a custom file shadowing a
        // built-in name would be confusing in the cycle.
        .filter(|n| !builtins.contains(n.as_str()))
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names
}

/// Built-ins + custom themes, in a stable order suitable for the
/// config view's ←/→ cycle: built-ins first (as listed), then custom
/// themes sorted alphabetically. Empty vec is never returned — even
/// without custom files we get the 10 built-ins.
pub fn list_all_themes() -> Vec<String> {
    let mut all: Vec<String> = list_themes().iter().map(|&s| s.to_string()).collect();
    all.extend(discover_custom_themes());
    all
}

pub fn get_theme(name: &str) -> Option<ThemeDefinition> {
    match name {
        "Tokyo Night" => Some(tokyo_night()),
        "Dracula" => Some(dracula()),
        "Catppuccin Mocha" => Some(catppuccin_mocha()),
        "Catppuccin Latte" => Some(catppuccin_latte()),
        "Gruvbox Dark" => Some(gruvbox_dark()),
        "Nord" => Some(nord()),
        "Solarized Dark" => Some(solarized_dark()),
        "Solarized Light" => Some(solarized_light()),
        "One Dark" => Some(one_dark()),
        "Monokai Pro" => Some(monokai_pro()),
        _ => None,
    }
}

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

/// Build the per-theme graph palette from a list of static hex strings.
/// Each theme calls this with 12 colours tuned to its world so the commit
/// graph reads as part of the theme instead of the generic Tab20 default.
fn graph_palette(hex: &[&str]) -> Vec<String> {
    hex.iter().map(|s| (*s).to_owned()).collect()
}

fn tokyo_night() -> ThemeDefinition {
    ThemeDefinition {
        syntax_theme: "base16-ocean.dark".to_string(),
        color_theme: ColorTheme {
            bg: rgb(0x1a, 0x1b, 0x26),
            fg: rgb(0xa9, 0xb1, 0xd6),
            list_selected_fg: rgb(0xc0, 0xca, 0xf5),
            list_selected_bg: rgb(0x51, 0x5c, 0x7e),
            list_compare_marked_fg: rgb(0xff, 0xff, 0xff),
            list_compare_marked_bg: rgb(0x6e, 0x55, 0xab),
            list_ref_paren_fg: rgb(0x51, 0x59, 0x7d),
            list_ref_branch_fg: rgb(0x9e, 0xce, 0x6a),
            list_ref_remote_branch_fg: rgb(0xf7, 0x76, 0x8e),
            list_ref_tag_fg: rgb(0xe0, 0xaf, 0x68),
            list_ref_stash_fg: rgb(0xbb, 0x9a, 0xf7),
            list_head_fg: rgb(0x7d, 0xcf, 0xff),
            list_commit_message_fg: rgb(0xa9, 0xb1, 0xd6),
            list_name_fg: rgb(0x7d, 0xcf, 0xff),
            list_hash_fg: rgb(0xe0, 0xaf, 0x68),
            list_date_fg: rgb(0xbb, 0x9a, 0xf7),
            list_match_fg: rgb(0x1a, 0x1b, 0x26),
            list_match_bg: rgb(0xe0, 0xaf, 0x68),
            detail_label_fg: rgb(0x51, 0x59, 0x7d),
            detail_name_fg: rgb(0x7d, 0xcf, 0xff),
            detail_date_fg: rgb(0xbb, 0x9a, 0xf7),
            detail_email_fg: rgb(0x7a, 0xa2, 0xf7),
            detail_hash_fg: rgb(0xe0, 0xaf, 0x68),
            detail_ref_branch_fg: rgb(0x9e, 0xce, 0x6a),
            detail_ref_remote_branch_fg: rgb(0xf7, 0x76, 0x8e),
            detail_ref_tag_fg: rgb(0xe0, 0xaf, 0x68),
            detail_file_change_add_fg: rgb(0x9e, 0xce, 0x6a),
            detail_file_change_modify_fg: rgb(0xff, 0x9e, 0x64),
            detail_file_change_delete_fg: rgb(0xf7, 0x76, 0x8e),
            detail_file_change_move_fg: rgb(0x7d, 0xcf, 0xff),
            ref_selected_fg: rgb(0xc0, 0xca, 0xf5),
            ref_selected_bg: rgb(0x51, 0x5c, 0x7e),
            help_block_title_fg: rgb(0x9e, 0xce, 0x6a),
            help_key_fg: rgb(0xe0, 0xaf, 0x68),
            virtual_cursor_fg: Color::Reset,
            status_input_fg: Color::Reset,
            status_input_transient_fg: rgb(0x51, 0x59, 0x7d),
            status_info_fg: rgb(0x7d, 0xcf, 0xff),
            status_success_fg: rgb(0x9e, 0xce, 0x6a),
            status_warn_fg: rgb(0xe0, 0xaf, 0x68),
            status_error_fg: rgb(0xf7, 0x76, 0x8e),
            divider_fg: rgb(0x3b, 0x42, 0x61),
            graph_branches: graph_palette(&[
                "#f7768e", "#7aa2f7", "#9ece6a", "#e0af68", "#bb9af7", "#7dcfff", "#ff9e64",
                "#73daca", "#ad8ee6", "#2ac3de", "#ff007c", "#41a6b5",
            ]),
        },
    }
}

fn dracula() -> ThemeDefinition {
    ThemeDefinition {
        syntax_theme: "Dracula".to_string(),
        color_theme: ColorTheme {
            bg: rgb(0x28, 0x2a, 0x36),
            fg: rgb(0xf8, 0xf8, 0xf2),
            list_selected_fg: rgb(0xf8, 0xf8, 0xf2),
            list_selected_bg: rgb(0x44, 0x47, 0x5a),
            list_compare_marked_fg: rgb(0xff, 0xff, 0xff),
            list_compare_marked_bg: rgb(0x80, 0x55, 0xc8),
            list_ref_paren_fg: rgb(0x62, 0x72, 0xa4),
            list_ref_branch_fg: rgb(0x50, 0xfa, 0x7b),
            list_ref_remote_branch_fg: rgb(0xff, 0x55, 0x55),
            list_ref_tag_fg: rgb(0xf1, 0xfa, 0x8c),
            list_ref_stash_fg: rgb(0xbd, 0x93, 0xf9),
            list_head_fg: rgb(0x8b, 0xe9, 0xfd),
            list_commit_message_fg: rgb(0xf8, 0xf8, 0xf2),
            list_name_fg: rgb(0x50, 0xfa, 0x7b),
            list_hash_fg: rgb(0xff, 0xb8, 0x6c),
            list_date_fg: rgb(0xbd, 0x93, 0xf9),
            list_match_fg: rgb(0x28, 0x2a, 0x36),
            list_match_bg: rgb(0xf1, 0xfa, 0x8c),
            detail_label_fg: rgb(0x62, 0x72, 0xa4),
            detail_name_fg: rgb(0x50, 0xfa, 0x7b),
            detail_date_fg: rgb(0xbd, 0x93, 0xf9),
            detail_email_fg: rgb(0xff, 0x79, 0xc6),
            detail_hash_fg: rgb(0xff, 0xb8, 0x6c),
            detail_ref_branch_fg: rgb(0x50, 0xfa, 0x7b),
            detail_ref_remote_branch_fg: rgb(0xff, 0x55, 0x55),
            detail_ref_tag_fg: rgb(0xf1, 0xfa, 0x8c),
            detail_file_change_add_fg: rgb(0x50, 0xfa, 0x7b),
            detail_file_change_modify_fg: rgb(0xff, 0xb8, 0x6c),
            detail_file_change_delete_fg: rgb(0xff, 0x55, 0x55),
            detail_file_change_move_fg: rgb(0x8b, 0xe9, 0xfd),
            ref_selected_fg: rgb(0xf8, 0xf8, 0xf2),
            ref_selected_bg: rgb(0x44, 0x47, 0x5a),
            help_block_title_fg: rgb(0x50, 0xfa, 0x7b),
            help_key_fg: rgb(0xf1, 0xfa, 0x8c),
            virtual_cursor_fg: Color::Reset,
            status_input_fg: Color::Reset,
            status_input_transient_fg: rgb(0x62, 0x72, 0xa4),
            status_info_fg: rgb(0x8b, 0xe9, 0xfd),
            status_success_fg: rgb(0x50, 0xfa, 0x7b),
            status_warn_fg: rgb(0xf1, 0xfa, 0x8c),
            status_error_fg: rgb(0xff, 0x55, 0x55),
            divider_fg: rgb(0x44, 0x47, 0x5a),
            graph_branches: graph_palette(&[
                "#ff5555", "#8be9fd", "#50fa7b", "#ffb86c", "#bd93f9", "#ff79c6", "#f1fa8c",
                "#6272a4", "#ff6e6e", "#69ff94", "#caa9fa", "#a4ffff",
            ]),
        },
    }
}

fn catppuccin_mocha() -> ThemeDefinition {
    ThemeDefinition {
        syntax_theme: "base16-ocean.dark".to_string(),
        color_theme: ColorTheme {
            bg: rgb(0x1e, 0x1e, 0x2e),
            fg: rgb(0xcd, 0xd6, 0xf4),
            list_selected_fg: rgb(0xcd, 0xd6, 0xf4),
            list_selected_bg: rgb(0x45, 0x47, 0x5a),
            list_compare_marked_fg: rgb(0xff, 0xff, 0xff),
            list_compare_marked_bg: rgb(0x8a, 0x6a, 0xc4),
            list_ref_paren_fg: rgb(0x7f, 0x84, 0x9c),
            list_ref_branch_fg: rgb(0xa6, 0xe3, 0xa1),
            list_ref_remote_branch_fg: rgb(0xf3, 0x8b, 0xa8),
            list_ref_tag_fg: rgb(0xf9, 0xe2, 0xaf),
            list_ref_stash_fg: rgb(0xcb, 0xa6, 0xf7),
            list_head_fg: rgb(0x89, 0xb4, 0xfa),
            list_commit_message_fg: rgb(0xcd, 0xd6, 0xf4),
            list_name_fg: rgb(0x89, 0xdc, 0xeb),
            list_hash_fg: rgb(0xfa, 0xb3, 0x87),
            list_date_fg: rgb(0xcb, 0xa6, 0xf7),
            list_match_fg: rgb(0x1e, 0x1e, 0x2e),
            list_match_bg: rgb(0xf9, 0xe2, 0xaf),
            detail_label_fg: rgb(0x7f, 0x84, 0x9c),
            detail_name_fg: rgb(0x89, 0xdc, 0xeb),
            detail_date_fg: rgb(0xcb, 0xa6, 0xf7),
            detail_email_fg: rgb(0x89, 0xb4, 0xfa),
            detail_hash_fg: rgb(0xfa, 0xb3, 0x87),
            detail_ref_branch_fg: rgb(0xa6, 0xe3, 0xa1),
            detail_ref_remote_branch_fg: rgb(0xf3, 0x8b, 0xa8),
            detail_ref_tag_fg: rgb(0xf9, 0xe2, 0xaf),
            detail_file_change_add_fg: rgb(0xa6, 0xe3, 0xa1),
            detail_file_change_modify_fg: rgb(0xfa, 0xb3, 0x87),
            detail_file_change_delete_fg: rgb(0xf3, 0x8b, 0xa8),
            detail_file_change_move_fg: rgb(0x94, 0xe2, 0xd5),
            ref_selected_fg: rgb(0xcd, 0xd6, 0xf4),
            ref_selected_bg: rgb(0x45, 0x47, 0x5a),
            help_block_title_fg: rgb(0xa6, 0xe3, 0xa1),
            help_key_fg: rgb(0xf9, 0xe2, 0xaf),
            virtual_cursor_fg: Color::Reset,
            status_input_fg: Color::Reset,
            status_input_transient_fg: rgb(0x7f, 0x84, 0x9c),
            status_info_fg: rgb(0x89, 0xdc, 0xeb),
            status_success_fg: rgb(0xa6, 0xe3, 0xa1),
            status_warn_fg: rgb(0xf9, 0xe2, 0xaf),
            status_error_fg: rgb(0xf3, 0x8b, 0xa8),
            divider_fg: rgb(0x31, 0x32, 0x44),
            graph_branches: graph_palette(&[
                "#f38ba8", "#89b4fa", "#a6e3a1", "#fab387", "#cba6f7", "#89dceb", "#f9e2af",
                "#94e2d5", "#f5c2e7", "#b4befe", "#eba0ac", "#74c7ec",
            ]),
        },
    }
}

fn catppuccin_latte() -> ThemeDefinition {
    ThemeDefinition {
        syntax_theme: "Solarized (light)".to_string(),
        color_theme: ColorTheme {
            bg: rgb(0xef, 0xf1, 0xf5),
            fg: rgb(0x4c, 0x4f, 0x69),
            list_selected_fg: rgb(0x4c, 0x4f, 0x69),
            list_selected_bg: rgb(0xbc, 0xc0, 0xcc),
            list_compare_marked_fg: rgb(0xff, 0xff, 0xff),
            list_compare_marked_bg: rgb(0x9c, 0x6f, 0xed),
            list_ref_paren_fg: rgb(0x8c, 0x8f, 0xa1),
            list_ref_branch_fg: rgb(0x40, 0xa0, 0x2b),
            list_ref_remote_branch_fg: rgb(0xd2, 0x0f, 0x39),
            list_ref_tag_fg: rgb(0xdf, 0x8e, 0x1d),
            list_ref_stash_fg: rgb(0x88, 0x39, 0xef),
            list_head_fg: rgb(0x1e, 0x66, 0xf5),
            list_commit_message_fg: rgb(0x4c, 0x4f, 0x69),
            list_name_fg: rgb(0x04, 0xa5, 0xe5),
            list_hash_fg: rgb(0xfe, 0x64, 0x0b),
            list_date_fg: rgb(0x88, 0x39, 0xef),
            list_match_fg: rgb(0xef, 0xf1, 0xf5),
            list_match_bg: rgb(0xdf, 0x8e, 0x1d),
            detail_label_fg: rgb(0x8c, 0x8f, 0xa1),
            detail_name_fg: rgb(0x04, 0xa5, 0xe5),
            detail_date_fg: rgb(0x88, 0x39, 0xef),
            detail_email_fg: rgb(0x1e, 0x66, 0xf5),
            detail_hash_fg: rgb(0xdf, 0x8e, 0x1d),
            detail_ref_branch_fg: rgb(0x40, 0xa0, 0x2b),
            detail_ref_remote_branch_fg: rgb(0xd2, 0x0f, 0x39),
            detail_ref_tag_fg: rgb(0xdf, 0x8e, 0x1d),
            detail_file_change_add_fg: rgb(0x40, 0xa0, 0x2b),
            detail_file_change_modify_fg: rgb(0xfe, 0x64, 0x0b),
            detail_file_change_delete_fg: rgb(0xd2, 0x0f, 0x39),
            detail_file_change_move_fg: rgb(0x17, 0x92, 0x99),
            ref_selected_fg: rgb(0x4c, 0x4f, 0x69),
            ref_selected_bg: rgb(0xbc, 0xc0, 0xcc),
            help_block_title_fg: rgb(0x40, 0xa0, 0x2b),
            help_key_fg: rgb(0xdf, 0x8e, 0x1d),
            virtual_cursor_fg: Color::Reset,
            status_input_fg: Color::Reset,
            status_input_transient_fg: rgb(0x8c, 0x8f, 0xa1),
            status_info_fg: rgb(0x04, 0xa5, 0xe5),
            status_success_fg: rgb(0x40, 0xa0, 0x2b),
            status_warn_fg: rgb(0xdf, 0x8e, 0x1d),
            status_error_fg: rgb(0xd2, 0x0f, 0x39),
            divider_fg: rgb(0xcc, 0xd0, 0xda),
            graph_branches: graph_palette(&[
                "#d20f39", "#1e66f5", "#40a02b", "#fe640b", "#8839ef", "#04a5e5", "#df8e1d",
                "#179299", "#ea76cb", "#7287fd", "#e64553", "#209fb5",
            ]),
        },
    }
}

fn gruvbox_dark() -> ThemeDefinition {
    ThemeDefinition {
        syntax_theme: "base16-mocha.dark".to_string(),
        color_theme: ColorTheme {
            bg: rgb(0x28, 0x28, 0x28),
            fg: rgb(0xeb, 0xdb, 0xb2),
            list_selected_fg: rgb(0xfb, 0xf1, 0xc7),
            list_selected_bg: rgb(0x50, 0x49, 0x45),
            list_compare_marked_fg: rgb(0xff, 0xff, 0xff),
            list_compare_marked_bg: rgb(0x9d, 0x4f, 0x6f),
            list_ref_paren_fg: rgb(0x92, 0x83, 0x74),
            list_ref_branch_fg: rgb(0xb8, 0xbb, 0x26),
            list_ref_remote_branch_fg: rgb(0xfb, 0x49, 0x34),
            list_ref_tag_fg: rgb(0xfa, 0xbd, 0x2f),
            list_ref_stash_fg: rgb(0xd3, 0x86, 0x9b),
            list_head_fg: rgb(0x8e, 0xc0, 0x7c),
            list_commit_message_fg: rgb(0xeb, 0xdb, 0xb2),
            list_name_fg: rgb(0x8e, 0xc0, 0x7c),
            list_hash_fg: rgb(0xfa, 0xbd, 0x2f),
            list_date_fg: rgb(0xd3, 0x86, 0x9b),
            list_match_fg: rgb(0x28, 0x28, 0x28),
            list_match_bg: rgb(0xfa, 0xbd, 0x2f),
            detail_label_fg: rgb(0x92, 0x83, 0x74),
            detail_name_fg: rgb(0x8e, 0xc0, 0x7c),
            detail_date_fg: rgb(0xd3, 0x86, 0x9b),
            detail_email_fg: rgb(0x83, 0xa5, 0x98),
            detail_hash_fg: rgb(0xfa, 0xbd, 0x2f),
            detail_ref_branch_fg: rgb(0xb8, 0xbb, 0x26),
            detail_ref_remote_branch_fg: rgb(0xfb, 0x49, 0x34),
            detail_ref_tag_fg: rgb(0xfa, 0xbd, 0x2f),
            detail_file_change_add_fg: rgb(0xb8, 0xbb, 0x26),
            detail_file_change_modify_fg: rgb(0xfe, 0x80, 0x19),
            detail_file_change_delete_fg: rgb(0xfb, 0x49, 0x34),
            detail_file_change_move_fg: rgb(0x83, 0xa5, 0x98),
            ref_selected_fg: rgb(0xfb, 0xf1, 0xc7),
            ref_selected_bg: rgb(0x50, 0x49, 0x45),
            help_block_title_fg: rgb(0xb8, 0xbb, 0x26),
            help_key_fg: rgb(0xfa, 0xbd, 0x2f),
            virtual_cursor_fg: Color::Reset,
            status_input_fg: Color::Reset,
            status_input_transient_fg: rgb(0x92, 0x83, 0x74),
            status_info_fg: rgb(0x8e, 0xc0, 0x7c),
            status_success_fg: rgb(0xb8, 0xbb, 0x26),
            status_warn_fg: rgb(0xfa, 0xbd, 0x2f),
            status_error_fg: rgb(0xfb, 0x49, 0x34),
            divider_fg: rgb(0x3c, 0x38, 0x36),
            graph_branches: graph_palette(&[
                "#fb4934", "#83a598", "#b8bb26", "#fe8019", "#d3869b", "#8ec07c", "#fabd2f",
                "#458588", "#cc241d", "#689d6a", "#d65d0e", "#b16286",
            ]),
        },
    }
}

fn nord() -> ThemeDefinition {
    ThemeDefinition {
        syntax_theme: "base16-ocean.dark".to_string(),
        color_theme: ColorTheme {
            bg: rgb(0x2e, 0x34, 0x40),
            fg: rgb(0xe5, 0xe9, 0xf0),
            list_selected_fg: rgb(0xec, 0xef, 0xf4),
            list_selected_bg: rgb(0x43, 0x4c, 0x5e),
            list_compare_marked_fg: rgb(0xff, 0xff, 0xff),
            list_compare_marked_bg: rgb(0x80, 0x6c, 0xa3),
            list_ref_paren_fg: rgb(0x4c, 0x56, 0x6a),
            list_ref_branch_fg: rgb(0xa3, 0xbe, 0x8c),
            list_ref_remote_branch_fg: rgb(0xbf, 0x61, 0x6a),
            list_ref_tag_fg: rgb(0xeb, 0xcb, 0x8b),
            list_ref_stash_fg: rgb(0xb4, 0x8e, 0xad),
            list_head_fg: rgb(0x88, 0xc0, 0xd0),
            list_commit_message_fg: rgb(0xd8, 0xde, 0xe9),
            list_name_fg: rgb(0x81, 0xa1, 0xc1),
            list_hash_fg: rgb(0xeb, 0xcb, 0x8b),
            list_date_fg: rgb(0xb4, 0x8e, 0xad),
            list_match_fg: rgb(0x2e, 0x34, 0x40),
            list_match_bg: rgb(0xeb, 0xcb, 0x8b),
            detail_label_fg: rgb(0x4c, 0x56, 0x6a),
            detail_name_fg: rgb(0x81, 0xa1, 0xc1),
            detail_date_fg: rgb(0xb4, 0x8e, 0xad),
            detail_email_fg: rgb(0x88, 0xc0, 0xd0),
            detail_hash_fg: rgb(0xeb, 0xcb, 0x8b),
            detail_ref_branch_fg: rgb(0xa3, 0xbe, 0x8c),
            detail_ref_remote_branch_fg: rgb(0xbf, 0x61, 0x6a),
            detail_ref_tag_fg: rgb(0xeb, 0xcb, 0x8b),
            detail_file_change_add_fg: rgb(0xa3, 0xbe, 0x8c),
            detail_file_change_modify_fg: rgb(0xd0, 0x87, 0x70),
            detail_file_change_delete_fg: rgb(0xbf, 0x61, 0x6a),
            detail_file_change_move_fg: rgb(0x8f, 0xbc, 0xbb),
            ref_selected_fg: rgb(0xec, 0xef, 0xf4),
            ref_selected_bg: rgb(0x43, 0x4c, 0x5e),
            help_block_title_fg: rgb(0xa3, 0xbe, 0x8c),
            help_key_fg: rgb(0xeb, 0xcb, 0x8b),
            virtual_cursor_fg: Color::Reset,
            status_input_fg: Color::Reset,
            status_input_transient_fg: rgb(0x4c, 0x56, 0x6a),
            status_info_fg: rgb(0x88, 0xc0, 0xd0),
            status_success_fg: rgb(0xa3, 0xbe, 0x8c),
            status_warn_fg: rgb(0xeb, 0xcb, 0x8b),
            status_error_fg: rgb(0xbf, 0x61, 0x6a),
            divider_fg: rgb(0x3b, 0x42, 0x52),
            graph_branches: graph_palette(&[
                "#bf616a", "#81a1c1", "#a3be8c", "#d08770", "#b48ead", "#88c0d0", "#ebcb8b",
                "#8fbcbb", "#5e81ac", "#e9c5a3", "#cf978c", "#7daea3",
            ]),
        },
    }
}

fn solarized_dark() -> ThemeDefinition {
    ThemeDefinition {
        syntax_theme: "Solarized (dark)".to_string(),
        color_theme: ColorTheme {
            bg: rgb(0x00, 0x2b, 0x36),
            fg: rgb(0x83, 0x94, 0x96),
            list_selected_fg: rgb(0x93, 0xa1, 0xa1),
            list_selected_bg: rgb(0x07, 0x36, 0x42),
            list_compare_marked_fg: rgb(0xff, 0xff, 0xff),
            list_compare_marked_bg: rgb(0x6c, 0x71, 0xc4),
            list_ref_paren_fg: rgb(0x58, 0x6e, 0x75),
            list_ref_branch_fg: rgb(0x85, 0x99, 0x00),
            list_ref_remote_branch_fg: rgb(0xdc, 0x32, 0x2f),
            list_ref_tag_fg: rgb(0xb5, 0x89, 0x00),
            list_ref_stash_fg: rgb(0x6c, 0x71, 0xc4),
            list_head_fg: rgb(0x26, 0x8b, 0xd2),
            list_commit_message_fg: rgb(0x83, 0x94, 0x96),
            list_name_fg: rgb(0x2a, 0xa1, 0x98),
            list_hash_fg: rgb(0xb5, 0x89, 0x00),
            list_date_fg: rgb(0x6c, 0x71, 0xc4),
            list_match_fg: rgb(0x00, 0x2b, 0x36),
            list_match_bg: rgb(0xb5, 0x89, 0x00),
            detail_label_fg: rgb(0x58, 0x6e, 0x75),
            detail_name_fg: rgb(0x2a, 0xa1, 0x98),
            detail_date_fg: rgb(0x6c, 0x71, 0xc4),
            detail_email_fg: rgb(0x26, 0x8b, 0xd2),
            detail_hash_fg: rgb(0xb5, 0x89, 0x00),
            detail_ref_branch_fg: rgb(0x85, 0x99, 0x00),
            detail_ref_remote_branch_fg: rgb(0xdc, 0x32, 0x2f),
            detail_ref_tag_fg: rgb(0xb5, 0x89, 0x00),
            detail_file_change_add_fg: rgb(0x85, 0x99, 0x00),
            detail_file_change_modify_fg: rgb(0xcb, 0x4b, 0x16),
            detail_file_change_delete_fg: rgb(0xdc, 0x32, 0x2f),
            detail_file_change_move_fg: rgb(0x2a, 0xa1, 0x98),
            ref_selected_fg: rgb(0x93, 0xa1, 0xa1),
            ref_selected_bg: rgb(0x07, 0x36, 0x42),
            help_block_title_fg: rgb(0x85, 0x99, 0x00),
            help_key_fg: rgb(0xb5, 0x89, 0x00),
            virtual_cursor_fg: Color::Reset,
            status_input_fg: Color::Reset,
            status_input_transient_fg: rgb(0x58, 0x6e, 0x75),
            status_info_fg: rgb(0x2a, 0xa1, 0x98),
            status_success_fg: rgb(0x85, 0x99, 0x00),
            status_warn_fg: rgb(0xb5, 0x89, 0x00),
            status_error_fg: rgb(0xdc, 0x32, 0x2f),
            divider_fg: rgb(0x07, 0x36, 0x42),
            graph_branches: graph_palette(&[
                "#dc322f", "#268bd2", "#859900", "#cb4b16", "#6c71c4", "#2aa198", "#b58900",
                "#d33682", "#e07f4e", "#7facd5", "#a4c200", "#9b9bbd",
            ]),
        },
    }
}

fn solarized_light() -> ThemeDefinition {
    ThemeDefinition {
        syntax_theme: "Solarized (light)".to_string(),
        color_theme: ColorTheme {
            bg: rgb(0xfd, 0xf6, 0xe3),
            fg: rgb(0x65, 0x7b, 0x83),
            list_selected_fg: rgb(0x58, 0x6e, 0x75),
            list_selected_bg: rgb(0xee, 0xe8, 0xd5),
            list_compare_marked_fg: rgb(0xff, 0xff, 0xff),
            list_compare_marked_bg: rgb(0x6c, 0x71, 0xc4),
            list_ref_paren_fg: rgb(0x93, 0xa1, 0xa1),
            list_ref_branch_fg: rgb(0x85, 0x99, 0x00),
            list_ref_remote_branch_fg: rgb(0xdc, 0x32, 0x2f),
            list_ref_tag_fg: rgb(0xb5, 0x89, 0x00),
            list_ref_stash_fg: rgb(0x6c, 0x71, 0xc4),
            list_head_fg: rgb(0x26, 0x8b, 0xd2),
            list_commit_message_fg: rgb(0x65, 0x7b, 0x83),
            list_name_fg: rgb(0x2a, 0xa1, 0x98),
            list_hash_fg: rgb(0xb5, 0x89, 0x00),
            list_date_fg: rgb(0x6c, 0x71, 0xc4),
            list_match_fg: rgb(0xfd, 0xf6, 0xe3),
            list_match_bg: rgb(0xb5, 0x89, 0x00),
            detail_label_fg: rgb(0x93, 0xa1, 0xa1),
            detail_name_fg: rgb(0x2a, 0xa1, 0x98),
            detail_date_fg: rgb(0x6c, 0x71, 0xc4),
            detail_email_fg: rgb(0x26, 0x8b, 0xd2),
            detail_hash_fg: rgb(0xb5, 0x89, 0x00),
            detail_ref_branch_fg: rgb(0x85, 0x99, 0x00),
            detail_ref_remote_branch_fg: rgb(0xdc, 0x32, 0x2f),
            detail_ref_tag_fg: rgb(0xb5, 0x89, 0x00),
            detail_file_change_add_fg: rgb(0x85, 0x99, 0x00),
            detail_file_change_modify_fg: rgb(0xcb, 0x4b, 0x16),
            detail_file_change_delete_fg: rgb(0xdc, 0x32, 0x2f),
            detail_file_change_move_fg: rgb(0x2a, 0xa1, 0x98),
            ref_selected_fg: rgb(0x58, 0x6e, 0x75),
            ref_selected_bg: rgb(0xee, 0xe8, 0xd5),
            help_block_title_fg: rgb(0x85, 0x99, 0x00),
            help_key_fg: rgb(0xb5, 0x89, 0x00),
            virtual_cursor_fg: Color::Reset,
            status_input_fg: Color::Reset,
            status_input_transient_fg: rgb(0x93, 0xa1, 0xa1),
            status_info_fg: rgb(0x2a, 0xa1, 0x98),
            status_success_fg: rgb(0x85, 0x99, 0x00),
            status_warn_fg: rgb(0xb5, 0x89, 0x00),
            status_error_fg: rgb(0xdc, 0x32, 0x2f),
            divider_fg: rgb(0xee, 0xe8, 0xd5),
            graph_branches: graph_palette(&[
                "#dc322f", "#268bd2", "#859900", "#cb4b16", "#6c71c4", "#2aa198", "#b58900",
                "#d33682", "#e07f4e", "#7facd5", "#a4c200", "#9b9bbd",
            ]),
        },
    }
}

fn one_dark() -> ThemeDefinition {
    ThemeDefinition {
        syntax_theme: "base16-eighties.dark".to_string(),
        color_theme: ColorTheme {
            bg: rgb(0x28, 0x2c, 0x34),
            fg: rgb(0xab, 0xb2, 0xbf),
            list_selected_fg: rgb(0xdc, 0xdf, 0xe4),
            list_selected_bg: rgb(0x3e, 0x44, 0x51),
            list_compare_marked_fg: rgb(0xff, 0xff, 0xff),
            list_compare_marked_bg: rgb(0x9c, 0x53, 0xb0),
            list_ref_paren_fg: rgb(0x7f, 0x84, 0x8e),
            list_ref_branch_fg: rgb(0x98, 0xc3, 0x79),
            list_ref_remote_branch_fg: rgb(0xe0, 0x6c, 0x75),
            list_ref_tag_fg: rgb(0xe5, 0xc0, 0x7b),
            list_ref_stash_fg: rgb(0xc6, 0x78, 0xdd),
            list_head_fg: rgb(0x61, 0xaf, 0xef),
            list_commit_message_fg: rgb(0xab, 0xb2, 0xbf),
            list_name_fg: rgb(0x56, 0xb6, 0xc2),
            list_hash_fg: rgb(0xe5, 0xc0, 0x7b),
            list_date_fg: rgb(0xc6, 0x78, 0xdd),
            list_match_fg: rgb(0x28, 0x2c, 0x34),
            list_match_bg: rgb(0xe5, 0xc0, 0x7b),
            detail_label_fg: rgb(0x7f, 0x84, 0x8e),
            detail_name_fg: rgb(0x56, 0xb6, 0xc2),
            detail_date_fg: rgb(0xc6, 0x78, 0xdd),
            detail_email_fg: rgb(0x61, 0xaf, 0xef),
            detail_hash_fg: rgb(0xe5, 0xc0, 0x7b),
            detail_ref_branch_fg: rgb(0x98, 0xc3, 0x79),
            detail_ref_remote_branch_fg: rgb(0xe0, 0x6c, 0x75),
            detail_ref_tag_fg: rgb(0xe5, 0xc0, 0x7b),
            detail_file_change_add_fg: rgb(0x98, 0xc3, 0x79),
            detail_file_change_modify_fg: rgb(0xd1, 0x9a, 0x66),
            detail_file_change_delete_fg: rgb(0xe0, 0x6c, 0x75),
            detail_file_change_move_fg: rgb(0x56, 0xb6, 0xc2),
            ref_selected_fg: rgb(0xdc, 0xdf, 0xe4),
            ref_selected_bg: rgb(0x3e, 0x44, 0x51),
            help_block_title_fg: rgb(0x98, 0xc3, 0x79),
            help_key_fg: rgb(0xe5, 0xc0, 0x7b),
            virtual_cursor_fg: Color::Reset,
            status_input_fg: Color::Reset,
            status_input_transient_fg: rgb(0x7f, 0x84, 0x8e),
            status_info_fg: rgb(0x56, 0xb6, 0xc2),
            status_success_fg: rgb(0x98, 0xc3, 0x79),
            status_warn_fg: rgb(0xe5, 0xc0, 0x7b),
            status_error_fg: rgb(0xe0, 0x6c, 0x75),
            divider_fg: rgb(0x3e, 0x44, 0x51),
            graph_branches: graph_palette(&[
                "#e06c75", "#61afef", "#98c379", "#d19a66", "#c678dd", "#56b6c2", "#e5c07b",
                "#be5046", "#7fa6f0", "#a8d189", "#ca8af0", "#76d4e1",
            ]),
        },
    }
}

fn monokai_pro() -> ThemeDefinition {
    ThemeDefinition {
        syntax_theme: "Monokai".to_string(),
        color_theme: ColorTheme {
            bg: rgb(0x2d, 0x2a, 0x2e),
            fg: rgb(0xfc, 0xfc, 0xfa),
            list_selected_fg: rgb(0xfc, 0xfc, 0xfa),
            list_selected_bg: rgb(0x40, 0x3e, 0x41),
            list_compare_marked_fg: rgb(0xff, 0xff, 0xff),
            list_compare_marked_bg: rgb(0x88, 0x6c, 0xb8),
            list_ref_paren_fg: rgb(0x72, 0x70, 0x72),
            list_ref_branch_fg: rgb(0xa9, 0xdc, 0x76),
            list_ref_remote_branch_fg: rgb(0xff, 0x61, 0x88),
            list_ref_tag_fg: rgb(0xff, 0xd8, 0x66),
            list_ref_stash_fg: rgb(0xab, 0x9d, 0xf2),
            list_head_fg: rgb(0x78, 0xdc, 0xe8),
            list_commit_message_fg: rgb(0xfc, 0xfc, 0xfa),
            list_name_fg: rgb(0x78, 0xdc, 0xe8),
            list_hash_fg: rgb(0xff, 0xd8, 0x66),
            list_date_fg: rgb(0xab, 0x9d, 0xf2),
            list_match_fg: rgb(0x2d, 0x2a, 0x2e),
            list_match_bg: rgb(0xff, 0xd8, 0x66),
            detail_label_fg: rgb(0x72, 0x70, 0x72),
            detail_name_fg: rgb(0x78, 0xdc, 0xe8),
            detail_date_fg: rgb(0xab, 0x9d, 0xf2),
            detail_email_fg: rgb(0x78, 0xdc, 0xe8),
            detail_hash_fg: rgb(0xff, 0xd8, 0x66),
            detail_ref_branch_fg: rgb(0xa9, 0xdc, 0x76),
            detail_ref_remote_branch_fg: rgb(0xff, 0x61, 0x88),
            detail_ref_tag_fg: rgb(0xff, 0xd8, 0x66),
            detail_file_change_add_fg: rgb(0xa9, 0xdc, 0x76),
            detail_file_change_modify_fg: rgb(0xfc, 0x98, 0x67),
            detail_file_change_delete_fg: rgb(0xff, 0x61, 0x88),
            detail_file_change_move_fg: rgb(0x78, 0xdc, 0xe8),
            ref_selected_fg: rgb(0xfc, 0xfc, 0xfa),
            ref_selected_bg: rgb(0x40, 0x3e, 0x41),
            help_block_title_fg: rgb(0xa9, 0xdc, 0x76),
            help_key_fg: rgb(0xff, 0xd8, 0x66),
            virtual_cursor_fg: Color::Reset,
            status_input_fg: Color::Reset,
            status_input_transient_fg: rgb(0x72, 0x70, 0x72),
            status_info_fg: rgb(0x78, 0xdc, 0xe8),
            status_success_fg: rgb(0xa9, 0xdc, 0x76),
            status_warn_fg: rgb(0xff, 0xd8, 0x66),
            status_error_fg: rgb(0xff, 0x61, 0x88),
            divider_fg: rgb(0x40, 0x3e, 0x41),
            graph_branches: graph_palette(&[
                "#ff6188", "#78dce8", "#a9dc76", "#fc9867", "#ab9df2", "#ffd866", "#e07c91",
                "#56cfd9", "#84b65f", "#c9785a", "#8979d0", "#dfb74a",
            ]),
        },
    }
}

// ─────────────────────────────────────────────────────────────────────
// Custom theme files on disk
//
// Users can drop a `<name>.toml` in `~/.config/gitoui/themes/` and
// reference it via `core.option.theme = "<name>"`. The file is a flat
// TOML with the same color tokens as the `[color]` section of the
// main config + two optional metadata fields:
//
//     base = "Dracula"          # optional, inherit unset tokens
//     syntax_theme = "Dracula"  # optional syntect palette name
//     fg = "#abc"               # any subset of ColorTheme tokens
//     bg = "#123"
//     # …
//
// Missing tokens fall back to the `base` theme's values; if no `base`
// is set, they use ColorTheme::default(). This lets the user write a
// 4-line file to recolor just the accent + keep everything else from
// Dracula intact.
// ─────────────────────────────────────────────────────────────────────

use std::path::{Path, PathBuf};

/// Things that can go wrong loading a custom theme file. Surfaced
/// to the user via `ConfigDiagnostic::from_theme_load_error`.
#[derive(Debug)]
pub enum ThemeLoadError {
    /// Name isn't a built-in AND no file at `<config>/themes/<name>.toml`.
    /// `searched` is the expected disk path so the message can point
    /// the user at where to drop the file.
    NotFound { name: String, searched: PathBuf },
    /// `<config>/themes/<name>.toml` exists but TOML parsing failed.
    /// Catches both syntax errors and bad hex values (ratatui's Color
    /// deserializer rejects malformed `#rrggbb` strings).
    BadFile {
        path: PathBuf,
        source: toml::de::Error,
    },
    /// User specified `base = "X"` in their theme file but X isn't a
    /// known built-in theme name.
    UnknownBase {
        path: PathBuf,
        name: String,
        base: String,
    },
    /// I/O error reading the file (permissions, etc.).
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// Default sub-directory under the config dir where custom themes
/// live. Kept public so the config view's "where to put a theme"
/// hint stays in sync.
pub const THEMES_DIR_NAME: &str = "themes";

/// Resolve the expected on-disk path for a custom theme named `name`.
/// Returns `None` if we can't even determine a config dir (no $HOME
/// + no $XDG_CONFIG_HOME on a system without a home directory).
pub fn custom_theme_path(name: &str) -> Option<PathBuf> {
    let base = crate::config::resolve_config_file_path()?;
    base.parent()
        .map(|p| p.join(THEMES_DIR_NAME).join(format!("{name}.toml")))
}

/// Built-in lookup first, disk fall-back second. The fall-back path
/// is `<config_dir>/themes/<name>.toml`. Empty `name` is treated as
/// "no theme picked" — same convention as `core.option.theme = ""`
/// in the main config — and returns `Ok(default)`.
pub fn resolve_or_load(name: &str) -> Result<ThemeDefinition, ThemeLoadError> {
    if name.is_empty() {
        return Ok(default_theme_definition());
    }
    if let Some(def) = get_theme(name) {
        return Ok(def);
    }
    // Built-in lookup missed — try disk.
    let Some(path) = custom_theme_path(name) else {
        return Err(ThemeLoadError::NotFound {
            name: name.to_string(),
            searched: PathBuf::from(format!("<config>/{THEMES_DIR_NAME}/{name}.toml")),
        });
    };
    if !path.exists() {
        return Err(ThemeLoadError::NotFound {
            name: name.to_string(),
            searched: path,
        });
    }
    load_theme_from_path(&path)
}

fn default_theme_definition() -> ThemeDefinition {
    ThemeDefinition {
        color_theme: ColorTheme::default(),
        syntax_theme: "base16-ocean.dark".to_string(),
    }
}

/// Per-file schema. `#[serde(flatten)]` lifts every OptionalColorTheme
/// field to the top level so users write `fg = "#abc"` not
/// `[colors] fg = "#abc"`.
#[derive(Default, serde::Deserialize)]
struct CustomThemeFile {
    base: Option<String>,
    syntax_theme: Option<String>,
    #[serde(flatten)]
    colors: crate::color::OptionalColorTheme,
}

fn load_theme_from_path(path: &Path) -> Result<ThemeDefinition, ThemeLoadError> {
    let content = std::fs::read_to_string(path).map_err(|source| ThemeLoadError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let file: CustomThemeFile =
        toml::from_str(&content).map_err(|source| ThemeLoadError::BadFile {
            path: path.to_path_buf(),
            source,
        })?;

    // Start from the explicit `base = "..."` if any, else from the
    // built-in defaults. Theme inheritance is single-level (no chain)
    // to keep load behaviour predictable.
    let base_def = match file.base.as_deref() {
        Some(base_name) => get_theme(base_name).ok_or_else(|| ThemeLoadError::UnknownBase {
            path: path.to_path_buf(),
            name: file_stem(path),
            base: base_name.to_string(),
        })?,
        None => default_theme_definition(),
    };

    let user_set_syntax = file.syntax_theme.is_some();
    let mut color_theme = base_def.color_theme;
    file.colors.merge_into(&mut color_theme);

    // Pick a syntax theme:
    //  1. explicit `syntax_theme = "..."` in the file wins.
    //  2. else, auto-detect from the resolved bg's luminance — overriding
    //     the inherited base. The user can override `bg` to a light color
    //     while keeping `base = "Tokyo Night"`; we don't want the dark
    //     base's syntax then. If they DO want the base's syntax verbatim
    //     they can set it explicitly.
    let syntax_theme = if user_set_syntax {
        file.syntax_theme.unwrap()
    } else {
        auto_pick_syntax_theme(&color_theme)
    };

    Ok(ThemeDefinition {
        color_theme,
        syntax_theme,
    })
}

/// Choose a sensible default `syntax_theme` based on a `ColorTheme`'s bg
/// luminance. Returns the name of a syntax theme that always ships with
/// gitoui (either bundled via `embed_theme!` or part of syntect's
/// `load_defaults()`), so the lookup at render time never misses.
fn auto_pick_syntax_theme(theme: &ColorTheme) -> String {
    if is_dark_color(theme.bg) {
        "base16-ocean.dark".to_string()
    } else {
        "InspiredGitHub".to_string()
    }
}

/// Quick light/dark check on a ratatui `Color`. Uses the perceived
/// luminance formula (rec. 601 coefficients) for RGB; named colors
/// fall back to a hard-coded best-guess. Unknown / `Reset` is treated
/// as dark since terminals overwhelmingly default to a dark background.
fn is_dark_color(c: Color) -> bool {
    match c {
        Color::Rgb(r, g, b) => {
            let lum = 0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b);
            lum < 128.0
        }
        Color::White | Color::Gray | Color::LightYellow => false,
        _ => true,
    }
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("?")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_themes_are_listed() {
        assert_eq!(list_themes().len(), 10);
    }

    #[test]
    fn get_theme_returns_some_for_every_listed_name() {
        for name in list_themes() {
            assert!(get_theme(name).is_some(), "missing theme: {name}");
        }
    }

    #[test]
    fn get_theme_returns_none_for_unknown() {
        assert!(get_theme("not-a-theme").is_none());
    }

    fn write_tmp_theme(name: &str, body: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("gitoui-test-themes");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{name}.toml"));
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn load_theme_inherits_base_and_overrides() {
        let path = write_tmp_theme(
            "load_inherits",
            r##"
base = "Tokyo Night"
fg = { Rgb = [255, 0, 100] }
bg = { Rgb = [10, 10, 10] }
graph_branches = ["#f05133", "#8b2a12"]
"##,
        );
        let def = load_theme_from_path(&path).unwrap();
        let base = tokyo_night().color_theme;
        // overridden tokens take user values
        assert_eq!(def.color_theme.fg, Color::Rgb(255, 0, 100));
        assert_eq!(def.color_theme.bg, Color::Rgb(10, 10, 10));
        // non-overridden tokens stay on the inherited base
        assert_eq!(def.color_theme.list_selected_fg, base.list_selected_fg);
        assert_eq!(def.color_theme.list_hash_fg, base.list_hash_fg);
        // graph palette is fully replaced when user supplies one
        assert_eq!(def.color_theme.graph_branches.len(), 2);
        assert_eq!(def.color_theme.graph_branches[0], "#f05133");
    }

    #[test]
    fn load_theme_rejects_unknown_base() {
        let path = write_tmp_theme(
            "bad_base",
            r#"
base = "NotARealTheme"
fg = { Rgb = [1, 2, 3] }
"#,
        );
        let err = load_theme_from_path(&path).unwrap_err();
        match err {
            ThemeLoadError::UnknownBase { base, .. } => assert_eq!(base, "NotARealTheme"),
            other => panic!("expected UnknownBase, got {other:?}"),
        }
    }

    #[test]
    fn load_theme_rejects_bad_toml() {
        let path = write_tmp_theme("bad_toml", "this is = not valid = toml ===");
        let err = load_theme_from_path(&path).unwrap_err();
        assert!(matches!(err, ThemeLoadError::BadFile { .. }));
    }

    #[test]
    fn resolve_or_load_built_in_name() {
        let def = resolve_or_load("Tokyo Night").unwrap();
        // sanity: matches the built-in definition
        assert_eq!(def.color_theme.bg, tokyo_night().color_theme.bg);
    }

    #[test]
    fn resolve_or_load_empty_returns_default() {
        // Empty name = "no theme picked", same as `core.option.theme = ""`.
        let def = resolve_or_load("").unwrap();
        assert_eq!(def.color_theme, ColorTheme::default());
    }

    #[test]
    fn auto_pick_syntax_theme_light_bg() {
        let path = write_tmp_theme(
            "auto_light",
            r##"
base = "Tokyo Night"
bg = "#ffffff"
"##,
        );
        let def = load_theme_from_path(&path).unwrap();
        // Light bg + no explicit syntax_theme → light syntect theme.
        assert_eq!(def.syntax_theme, "InspiredGitHub");
    }

    #[test]
    fn auto_pick_syntax_theme_dark_bg() {
        let path = write_tmp_theme(
            "auto_dark",
            r##"
base = "Tokyo Night"
bg = "#0a0a0a"
"##,
        );
        let def = load_theme_from_path(&path).unwrap();
        assert_eq!(def.syntax_theme, "base16-ocean.dark");
    }

    #[test]
    fn explicit_syntax_theme_wins_over_auto() {
        let path = write_tmp_theme(
            "explicit_syntax",
            r##"
base = "Tokyo Night"
bg = "#ffffff"
syntax_theme = "Dracula"
"##,
        );
        let def = load_theme_from_path(&path).unwrap();
        // Light bg would auto-pick InspiredGitHub, but explicit wins.
        assert_eq!(def.syntax_theme, "Dracula");
    }
}

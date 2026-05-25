use std::{
    env,
    path::{Path, PathBuf},
    str::FromStr,
};

use chrono::{DateTime, FixedOffset, Local};
use garde::Validate;
use rustc_hash::FxHashMap;
use serde::Deserialize;
use smart_default::SmartDefault;
use umbra::optional;

use crate::{
    color::{ColorTheme, OptionalColorTheme},
    graph::GraphImageWidthMode,
    keybind::KeyBinds,
    CommitOrderType, GraphRenderer, GraphStyle, GraphWidthType, InitialSelection, Result,
};

/// Predefined date/time formats for the application.
#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, SmartDefault)]
pub enum DateTimeFormat {
    /// DD/MM/YYYY - HH:MM (default)
    #[default]
    DDMMYYYY_HHMM,
    /// DD/MM/YYYY
    DDMMYYYY,
    /// MM/DD/YYYY HH:MM
    MMDDYYYY_HHMM,
    /// YYYY/MM/DD HH:MM
    YYYYMMDD_HHMM,
    /// ISO 8601: YYYY-MM-DD HH:MM:SS ±HHMM
    ISO,
    /// YYYY-MM-DD HH:MM
    YYYYMMDD_HHMM_DASH,
}

impl DateTimeFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            DateTimeFormat::DDMMYYYY_HHMM => "%d/%m/%Y - %H:%M",
            DateTimeFormat::DDMMYYYY => "%d/%m/%Y",
            DateTimeFormat::MMDDYYYY_HHMM => "%m/%d/%Y %H:%M",
            DateTimeFormat::YYYYMMDD_HHMM => "%Y/%m/%d %H:%M",
            DateTimeFormat::ISO => "%Y-%m-%d %H:%M:%S %z",
            DateTimeFormat::YYYYMMDD_HHMM_DASH => "%Y-%m-%d %H:%M",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            DateTimeFormat::DDMMYYYY_HHMM => "DD/MM/YYYY - HH:MM",
            DateTimeFormat::DDMMYYYY => "DD/MM/YYYY",
            DateTimeFormat::MMDDYYYY_HHMM => "MM/DD/YYYY HH:MM",
            DateTimeFormat::YYYYMMDD_HHMM => "YYYY/MM/DD HH:MM",
            DateTimeFormat::ISO => "ISO 8601",
            DateTimeFormat::YYYYMMDD_HHMM_DASH => "YYYY-MM-DD HH:MM",
        }
    }

    pub fn format(&self, dt: &DateTime<FixedOffset>, local: bool) -> String {
        if local {
            dt.with_timezone(&Local).format(self.as_str()).to_string()
        } else {
            dt.format(self.as_str()).to_string()
        }
    }

    pub fn cycle_next(&self) -> Self {
        match self {
            DateTimeFormat::DDMMYYYY_HHMM => DateTimeFormat::DDMMYYYY,
            DateTimeFormat::DDMMYYYY => DateTimeFormat::MMDDYYYY_HHMM,
            DateTimeFormat::MMDDYYYY_HHMM => DateTimeFormat::YYYYMMDD_HHMM,
            DateTimeFormat::YYYYMMDD_HHMM => DateTimeFormat::ISO,
            DateTimeFormat::ISO => DateTimeFormat::YYYYMMDD_HHMM_DASH,
            DateTimeFormat::YYYYMMDD_HHMM_DASH => DateTimeFormat::DDMMYYYY_HHMM,
        }
    }

    pub fn cycle_prev(&self) -> Self {
        match self {
            DateTimeFormat::DDMMYYYY_HHMM => DateTimeFormat::YYYYMMDD_HHMM_DASH,
            DateTimeFormat::DDMMYYYY => DateTimeFormat::DDMMYYYY_HHMM,
            DateTimeFormat::MMDDYYYY_HHMM => DateTimeFormat::DDMMYYYY,
            DateTimeFormat::YYYYMMDD_HHMM => DateTimeFormat::MMDDYYYY_HHMM,
            DateTimeFormat::ISO => DateTimeFormat::YYYYMMDD_HHMM,
            DateTimeFormat::YYYYMMDD_HHMM_DASH => DateTimeFormat::ISO,
        }
    }
}

impl FromStr for DateTimeFormat {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "ddmmyyyy_hhmm" => Ok(DateTimeFormat::DDMMYYYY_HHMM),
            "ddmmyyyy" => Ok(DateTimeFormat::DDMMYYYY),
            "mmddyyyy_hhmm" => Ok(DateTimeFormat::MMDDYYYY_HHMM),
            "yyyymmdd_hhmm" => Ok(DateTimeFormat::YYYYMMDD_HHMM),
            "iso" => Ok(DateTimeFormat::ISO),
            "yyyymmdd_hhmm_dash" => Ok(DateTimeFormat::YYYYMMDD_HHMM_DASH),
            _ => Err(format!("Unknown date_time_format: {}", s)),
        }
    }
}

impl<'de> Deserialize<'de> for DateTimeFormat {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        DateTimeFormat::from_str(&s).map_err(serde::de::Error::custom)
    }
}

const XDG_CONFIG_HOME_ENV_NAME: &str = "XDG_CONFIG_HOME";
const DEFAULT_CONFIG_DIR: &str = ".config";
const APP_DIR_NAME: &str = "gitoui";
const CONFIG_FILE_NAME: &str = "config.toml";
const CONFIG_FILE_ENV_NAME: &str = "GITOUI_CONFIG_FILE";

pub fn load() -> Result<(
    CoreConfig,
    UiConfig,
    GraphConfig,
    ColorTheme,
    Option<KeyBinds>,
)> {
    let config = match config_file_path_from_env() {
        Some(user_path) => {
            if !user_path.exists() {
                return Err(crate::Error::Config(format!(
                    "Config file specified by ${CONFIG_FILE_ENV_NAME} environment variable not found: {}",
                    user_path.display()
                )));
            }
            read_config_from_path(&user_path)
        }
        None => {
            if let Some(default_path) = config_file_path() {
                if default_path.exists() {
                    read_config_from_path(&default_path)
                } else {
                    Ok(Config::default())
                }
            } else {
                Ok(Config::default())
            }
        }
    }?;

    config.validate()?;

    Ok((
        config.core,
        config.ui,
        config.graph,
        config.color,
        config.keybind,
    ))
}

/// Resolve the config file path that `load()` reads from.
/// `$GITOUI_CONFIG_FILE` first, then `$XDG_CONFIG_HOME/gitoui/config.toml`
/// (defaulting to `~/.config/gitoui/config.toml`). Returns the path even
/// if the file doesn't exist yet, callers (e.g. the in-app `o:open file`
/// shortcut) use it to seed a new config on first edit.
pub fn resolve_config_file_path() -> Option<PathBuf> {
    config_file_path_from_env().or_else(config_file_path)
}

fn config_file_path_from_env() -> Option<PathBuf> {
    env::var(CONFIG_FILE_ENV_NAME).ok().map(PathBuf::from)
}

fn config_file_path() -> Option<PathBuf> {
    env::var(XDG_CONFIG_HOME_ENV_NAME)
        .ok()
        .map(PathBuf::from)
        .or_else(|| env::home_dir().map(|home| home.join(DEFAULT_CONFIG_DIR)))
        .map(|config_dir| config_dir.join(APP_DIR_NAME).join(CONFIG_FILE_NAME))
}

fn read_config_from_path(path: &Path) -> Result<Config> {
    let content = std::fs::read_to_string(path)?;
    let config: OptionalConfig = toml::from_str(&content)?;
    let mut config: Config = config.into();
    migrate_legacy_protocol(&mut config);
    Ok(config)
}

fn migrate_legacy_protocol(config: &mut Config) {
    let opt = &mut config.core.option;
    if opt.graph_renderer.is_some() || opt.protocol.is_none() {
        return;
    }
    let mapped = match opt.protocol.as_deref() {
        Some("auto") => Some(GraphRenderer::Auto),
        Some("kitty") => Some(GraphRenderer::Kitty),
        Some("kitty-unicode") => Some(GraphRenderer::KittyUnicode),
        Some("iterm") | Some("iterm2") => Some(GraphRenderer::Iterm),
        Some("sixel") => Some(GraphRenderer::Sixel),
        Some("unicode") => Some(GraphRenderer::Ascii),
        _ => None,
    };
    if let Some(r) = mapped {
        opt.graph_renderer = Some(r);
    }
    opt.protocol = None;
}

#[optional(derives = [Deserialize])]
#[derive(Debug, Default, Clone, PartialEq, Eq, Validate)]
struct Config {
    #[garde(dive)]
    #[nested]
    core: CoreConfig,
    #[garde(dive)]
    #[nested]
    ui: UiConfig,
    #[garde(dive)]
    #[nested]
    graph: GraphConfig,
    #[garde(skip)]
    #[nested]
    color: ColorTheme,
    // The user customed keybinds, please ref `assets/default-keybind.toml`
    #[garde(skip)]
    keybind: Option<KeyBinds>,
}

#[optional(derives = [Deserialize])]
#[derive(Debug, Default, Clone, PartialEq, Eq, Validate)]
pub struct CoreConfig {
    #[garde(skip)]
    #[nested]
    pub option: CoreOptionConfig,
    #[garde(skip)]
    #[nested]
    pub search: CoreSearchConfig,
    #[garde(dive)]
    #[nested]
    pub user_command: CoreUserCommandConfig,
    #[garde(dive)]
    #[nested]
    pub external: CoreExternalConfig,
}

#[optional(derives = [Deserialize])]
#[derive(Debug, Clone, PartialEq, Eq, SmartDefault)]
pub struct CoreOptionConfig {
    pub graph_renderer: Option<GraphRenderer>,
    pub protocol: Option<String>,
    pub order: Option<CommitOrderType>,
    pub graph_width: Option<GraphWidthType>,
    pub graph_style: Option<GraphStyle>,
    pub initial_selection: Option<InitialSelection>,
    #[default = true]
    pub auto_refresh: bool,
    #[default = 500]
    pub auto_refresh_debounce_ms: u64,
    /// Commit-count cap above which gitoui force-disables the graph
    /// column at startup. Repos exceeding this many commits would
    /// otherwise hit lag / freeze / glitch in the inline-image
    /// pipeline. The user can still flip the toggle back on from
    /// the Config view; the Details panel shows a warning when the
    /// threshold was tripped. Set to 0 to disable the auto-disable
    /// entirely.
    #[default = 50_000]
    pub huge_repo_threshold: usize,
    #[default = "Tokyo Night"]
    pub theme: String,
    #[default = "base16-ocean.dark"]
    pub syntax_theme: String,
    #[default(DateTimeFormat::DDMMYYYY_HHMM)]
    pub date_time_format: DateTimeFormat,
    #[default = true]
    pub date_time_local: bool,
    pub user_name: Option<String>,
    pub user_email: Option<String>,
    pub default_branch: Option<String>,
    #[default = true]
    pub github_avatars: bool,
    /// Whether the binary checks crates.io for a newer version at
    /// startup. Set to `false` (manually or via the prompt's `N`
    /// answer) to silence the once-a-day check.
    #[default = true]
    pub check_updates: bool,
}

#[optional(derives = [Deserialize])]
#[derive(Debug, Clone, PartialEq, Eq, SmartDefault)]
pub struct CoreSearchConfig {
    #[default = false]
    pub ignore_case: bool,
    #[default = false]
    pub fuzzy: bool,
    #[default = false]
    pub regex: bool,
}

#[optional]
#[derive(Debug, Clone, PartialEq, Eq, SmartDefault, Validate)]
pub struct CoreUserCommandConfig {
    #[garde(dive)]
    #[default(FxHashMap::from_iter([("1".into(), UserCommand {
        name: "git diff".into(),
        r#type: UserCommandType::Inline,
        commands: vec![
            "git".into(),
            "--no-pager".into(),
            "diff".into(),
            "--color=always".into(),
            "{{first_parent_hash}}".into(),
            "{{target_hash}}".into(),
        ],
        refresh: false,
    })]))]
    pub commands: FxHashMap<String, UserCommand>,
    #[garde(range(min = 0))]
    #[default = 4]
    pub tab_width: u16,
}

impl<'de> Deserialize<'de> for OptionalCoreUserCommandConfig {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{Error, MapAccess, Visitor};
        use std::fmt;

        struct OptionalCoreUserCommandConfigVisitor;

        impl<'de> Visitor<'de> for OptionalCoreUserCommandConfigVisitor {
            type Value = OptionalCoreUserCommandConfig;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a user command configuration")
            }

            fn visit_map<V>(self, mut map: V) -> std::result::Result<Self::Value, V::Error>
            where
                V: MapAccess<'de>,
            {
                let mut commands = FxHashMap::default();
                let mut tab_width = None;

                while let Some(key) = map.next_key::<String>()? {
                    if let Some(suffix) = key.strip_prefix("commands_") {
                        let command_key = suffix.to_string();
                        if command_key.is_empty() {
                            return Err(V::Error::custom(
                                "command key cannot be empty, like `commands_`",
                            ));
                        }
                        let command_value: UserCommand = map.next_value()?;
                        commands.insert(command_key, command_value);
                    } else if key == "tab_width" {
                        tab_width = Some(map.next_value()?);
                    } else if key == "commands" {
                        return Err(V::Error::custom(
                            "invalid key `commands`, use `commands_n` format instead",
                        ));
                    } else {
                        let _: serde::de::IgnoredAny = map.next_value()?;
                    }
                }

                let commands = if commands.is_empty() {
                    None
                } else {
                    Some(commands)
                };

                Ok(OptionalCoreUserCommandConfig {
                    commands,
                    tab_width,
                })
            }
        }

        deserializer.deserialize_map(OptionalCoreUserCommandConfigVisitor)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Validate)]
pub struct UserCommand {
    #[garde(length(min = 1))]
    pub name: String,
    #[serde(default)]
    #[garde(skip)]
    pub r#type: UserCommandType,
    #[garde(length(min = 1), inner(length(min = 1)))]
    pub commands: Vec<String>,
    #[serde(default)]
    #[garde(custom(validate_user_command_refresh(&self.r#type)))]
    pub refresh: bool,
}

fn validate_user_command_refresh(
    command_type: &UserCommandType,
) -> impl FnOnce(&bool, &()) -> garde::Result + '_ {
    move |refresh, _| {
        if matches!(command_type, UserCommandType::Inline) && *refresh {
            return Err(garde::Error::new(
                "refresh cannot be true for inline command",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UserCommandType {
    #[default]
    Inline,
    Silent,
    Suspend,
}

#[optional(derives = [Deserialize])]
#[derive(Debug, Default, Clone, PartialEq, Eq, Validate)]
pub struct UiConfig {
    #[garde(skip)]
    #[nested]
    pub common: UiCommonConfig,
    #[garde(dive)]
    #[nested]
    pub list: UiListConfig,
    #[garde(dive)]
    #[nested]
    pub detail: UiDetailConfig,
    #[garde(dive)]
    #[nested]
    pub user_command: UiUserCommandConfig,
    #[garde(dive)]
    #[nested]
    pub refs: UiRefsConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum DiffMode {
    #[default]
    Enhanced,
    Raw,
    /// Two columns: old version on the left, new on the right, separated by a
    /// vertical bar. Removed lines render only on the left, added only on the
    /// right; consecutive Del+Add pairs are zipped row-by-row so modifications
    /// align side by side. Long lines are truncated with `…` rather than
    /// wrapped (wrapping each half independently misaligns the pair).
    SideBySide,
    /// Side-by-side layout with the Enhanced styling: per-side line-number
    /// gutter, full-row background color on additions / deletions, and syntax
    /// highlighting on the content. Does not (yet) support the clickable
    /// "show more" expand buttons of the single-column Enhanced view, for
    /// gap navigation, fall back to the Enhanced mode.
    SideBySideEnhanced,
}

/// Layout for the merge-conflict editor opened with `a` on an unmerged file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ConflictViewMode {
    /// Three vertical panes side by side: Ours | Base | Theirs.
    /// Falls back to TwoPane automatically when the terminal is too narrow.
    ThreePane,
    /// Two vertical panes: Ours | Theirs (base is hidden).
    #[default]
    TwoPane,
    /// Stacked, full-width: one hunk shown at a time with Ours then Theirs
    /// rendered top-to-bottom. Best for narrow terminals.
    Inline,
}

/// Layout for the interactive-rebase editor opened from the Rebase dialog
/// when "Interactive (-i)" is checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum RebaseViewMode {
    /// Single dense list, closest to the CLI experience.
    Compact,
    /// GitKraken-style: each row followed by an italic explanation of what
    /// the picked action will do, with inline reword editor.
    #[default]
    Inline,
    /// Compact list on top, live "Result preview" pane below, mirrors
    /// the conflict editor layout.
    Split,
}

#[optional(derives = [Deserialize])]
#[derive(Debug, Clone, PartialEq, Eq, SmartDefault)]
pub struct UiCommonConfig {
    #[default(CursorType::Native)]
    pub cursor_type: CursorType,
    #[default = true]
    pub mouse_enabled: bool,
    #[default(DiffMode::Enhanced)]
    pub diff_mode: DiffMode,
    #[default(ConflictViewMode::TwoPane)]
    pub conflict_view: ConflictViewMode,
    #[default(RebaseViewMode::Inline)]
    pub rebase_view: RebaseViewMode,
    /// Whether to render Nerd Font glyphs in chrome (e.g. the GitHub
    /// logomark in the PR / Issues view headers). On by default, users
    /// without a Nerd Font installed can opt out via the config file
    /// (this field is intentionally not exposed in the in-app config
    /// page; it's a TOML-only knob):
    ///
    /// ```toml
    /// [ui.common]
    /// nerd-font = false
    /// ```
    #[default = true]
    pub nerd_font: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub enum CursorType {
    Native,
    Virtual(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default, Validate)]
pub enum ClipboardConfig {
    #[default]
    Auto,
    Custom {
        #[garde(length(min = 1), inner(length(min = 1)))]
        commands: Vec<String>,
    },
}

#[optional(derives = [Deserialize])]
#[derive(Debug, Clone, PartialEq, Eq, SmartDefault, Validate)]
pub struct CoreExternalConfig {
    #[garde(dive)]
    #[default(ClipboardConfig::Auto)]
    pub clipboard: ClipboardConfig,
}

#[optional(derives = [Deserialize])]
#[derive(Debug, Clone, PartialEq, Eq, SmartDefault, Validate)]
pub struct UiListConfig {
    #[garde(length(min = 1))]
    #[default(vec![
        UserListColumnType::Graph,
        UserListColumnType::Marker,
        UserListColumnType::CommitMessage,
        UserListColumnType::Name,
        UserListColumnType::Hash,
        UserListColumnType::Date,
    ])]
    pub columns: Vec<UserListColumnType>,
    #[garde(range(min = 1))]
    #[default = 20]
    pub commit_message_min_width: u16,
    #[garde(length(min = 1))]
    #[default = "%d/%m/%Y - %H:%M"]
    pub date_format: String,
    #[garde(range(min = 0))]
    #[default = 20]
    pub date_width: u16,
    #[garde(skip)]
    #[default = true]
    pub date_local: bool,
    #[garde(range(min = 0))]
    #[default = 20]
    pub name_width: u16,
    /// Whether to render the inline commit graph image (Kitty/iTerm2/Sixel)
    /// at all. Disabling it removes the lane image pipeline entirely —
    /// no SVG/PNG generation, no terminal-image protocol uploads — which
    /// makes scrolling near-instant on huge repos (rust-lang/rust, linux,
    /// …) at the cost of losing the visual branch view. The `Marker`
    /// column still draws the per-commit `│` accent so the active branch
    /// color is still legible.
    #[garde(skip)]
    #[default = true]
    pub graph_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UserListColumnType {
    Graph,
    Marker,
    #[serde(rename = "commit_message")]
    CommitMessage,
    Name,
    Hash,
    Date,
}

#[optional(derives = [Deserialize])]
#[derive(Debug, Clone, PartialEq, Eq, SmartDefault, Validate)]
pub struct UiDetailConfig {
    #[garde(range(min = 1))]
    #[default = 20]
    pub height: u16,
    #[garde(length(min = 1))]
    #[default = "%Y-%m-%d %H:%M:%S %z"]
    pub date_format: String,
    #[garde(skip)]
    #[default = true]
    pub date_local: bool,
}

#[optional(derives = [Deserialize])]
#[derive(Debug, Clone, PartialEq, Eq, SmartDefault, Validate)]
pub struct UiUserCommandConfig {
    #[garde(range(min = 1))]
    #[default = 20]
    pub height: u16,
}

#[optional(derives = [Deserialize])]
#[derive(Debug, Clone, PartialEq, Eq, SmartDefault, Validate)]
pub struct UiRefsConfig {
    #[garde(range(min = 1))]
    #[default = 36]
    pub width: u16,
}

#[optional(derives = [Deserialize])]
#[derive(Debug, Clone, PartialEq, Eq, SmartDefault, Validate)]
pub struct GraphConfig {
    #[garde(skip)]
    #[default(GraphImageWidthMode::Compact)]
    pub row_image_width: GraphImageWidthMode,
    #[garde(dive)]
    #[nested]
    pub color: GraphColorConfig,
}

#[optional(derives = [Deserialize])]
#[derive(Debug, Clone, PartialEq, Eq, SmartDefault, Validate)]
pub struct GraphColorConfig {
    #[garde(length(min = 1), inner(pattern(r"^#([0-9a-fA-F]{6}|[0-9a-fA-F]{8})$")))]
    // Ordered for maximum *consecutive* contrast: adjacent palette indices
    // are far apart in hue/saturation so neighbouring branches in the graph
    // never read as "the same red" or "the same blue". 16 entries instead
    // of 8, with the previous palette, any repo with >8 active lanes
    // wrapped around and showed three reds (this was the user complaint).
    #[default(vec![
        "#1f77b4".into(), // steel blue
        "#d62728".into(), // brick red
        "#2ca02c".into(), // forest green
        "#ff7f0e".into(), // orange
        "#9467bd".into(), // purple
        "#17becf".into(), // teal
        "#e377c2".into(), // pink
        "#bcbd22".into(), // olive
        "#fcbf49".into(), // amber
        "#4dabf7".into(), // sky blue
        "#20c997".into(), // emerald
        "#8c564b".into(), // brown
        "#fa8072".into(), // salmon
        "#6f42c1".into(), // indigo
        "#ffd43b".into(), // yellow
        "#c92a2a".into(), // dark red
    ])]
    pub branches: Vec<String>,
    #[garde(pattern(r"^#([0-9a-fA-F]{6}|[0-9a-fA-F]{8})$"))]
    #[default = "#00000000"]
    pub edge: String,
    #[garde(pattern(r"^#([0-9a-fA-F]{6}|[0-9a-fA-F]{8})$"))]
    #[default = "#00000000"]
    pub background: String,
}

impl CoreConfig {
    pub fn graph_style(&self) -> crate::GraphStyle {
        self.option
            .graph_style
            .unwrap_or(crate::GraphStyle::Rounded)
    }
    pub fn set_graph_style(&mut self, style: crate::GraphStyle) {
        self.option.graph_style = Some(style);
    }
    pub fn graph_width(&self) -> crate::GraphWidthType {
        self.option
            .graph_width
            .unwrap_or(crate::GraphWidthType::Auto)
    }
    pub fn set_graph_width(&mut self, width: crate::GraphWidthType) {
        self.option.graph_width = Some(width);
    }
    pub fn initial_selection(&self) -> crate::InitialSelection {
        self.option
            .initial_selection
            .unwrap_or(crate::InitialSelection::Latest)
    }
    pub fn set_initial_selection(&mut self, sel: crate::InitialSelection) {
        self.option.initial_selection = Some(sel);
    }
    pub fn graph_renderer(&self) -> crate::GraphRenderer {
        self.option.graph_renderer.unwrap_or_default()
    }
    pub fn set_graph_renderer(&mut self, renderer: crate::GraphRenderer) {
        self.option.graph_renderer = Some(renderer);
    }
    pub fn order(&self) -> crate::CommitOrderType {
        self.option.order.unwrap_or(crate::CommitOrderType::Chrono)
    }
    pub fn set_order(&mut self, order: crate::CommitOrderType) {
        self.option.order = Some(order);
    }
    pub fn date_time_format(&self) -> DateTimeFormat {
        self.option.date_time_format
    }
    pub fn set_date_time_format(&mut self, format: DateTimeFormat) {
        self.option.date_time_format = format;
    }
    pub fn date_time_local(&self) -> bool {
        self.option.date_time_local
    }
    pub fn set_date_time_local(&mut self, local: bool) {
        self.option.date_time_local = local;
    }
    pub fn user_name(&self) -> Option<&str> {
        self.option.user_name.as_deref()
    }
    pub fn set_user_name(&mut self, name: Option<String>) {
        self.option.user_name = name;
    }
    pub fn user_email(&self) -> Option<&str> {
        self.option.user_email.as_deref()
    }
    pub fn set_user_email(&mut self, email: Option<String>) {
        self.option.user_email = email;
    }
    pub fn default_branch(&self) -> Option<&str> {
        self.option.default_branch.as_deref()
    }
    pub fn set_default_branch(&mut self, name: Option<String>) {
        self.option.default_branch = name.filter(|s| !s.is_empty());
    }
    pub fn github_avatars(&self) -> bool {
        self.option.github_avatars
    }
    pub fn set_github_avatars(&mut self, enabled: bool) {
        self.option.github_avatars = enabled;
    }
}

impl UiCommonConfig {
    pub fn diff_mode(&self) -> DiffMode {
        self.diff_mode
    }
    pub fn set_diff_mode(&mut self, mode: DiffMode) {
        self.diff_mode = mode;
    }
    pub fn mouse_enabled(&self) -> bool {
        self.mouse_enabled
    }
    pub fn set_mouse_enabled(&mut self, enabled: bool) {
        self.mouse_enabled = enabled;
    }
    pub fn conflict_view(&self) -> ConflictViewMode {
        self.conflict_view
    }
    pub fn set_conflict_view(&mut self, mode: ConflictViewMode) {
        self.conflict_view = mode;
    }
    pub fn rebase_view(&self) -> RebaseViewMode {
        self.rebase_view
    }
    pub fn set_rebase_view(&mut self, mode: RebaseViewMode) {
        self.rebase_view = mode;
    }
}

pub fn save(core: &CoreConfig, ui: &UiConfig) -> std::result::Result<(), String> {
    let path = config_file_path().ok_or("Could not determine config path")?;

    let mut doc = if path.exists() {
        let content =
            std::fs::read_to_string(&path).map_err(|e| format!("Failed to read config: {}", e))?;
        content
            .parse::<toml::Table>()
            .map_err(|e| format!("Failed to parse config: {}", e))?
    } else {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create config directory: {}", e))?;
        }
        toml::Table::new()
    };

    if let Some(style) = core.option.graph_style {
        set_nested_string(
            &mut doc,
            &["core", "option", "graph_style"],
            match style {
                crate::GraphStyle::Rounded => "rounded",
                crate::GraphStyle::Angular => "angular",
                crate::GraphStyle::Smooth => "smooth",
            },
        );
    }

    if let Some(width) = core.option.graph_width {
        set_nested_string(
            &mut doc,
            &["core", "option", "graph_width"],
            match width {
                crate::GraphWidthType::Auto => "auto",
                crate::GraphWidthType::Double => "double",
                crate::GraphWidthType::Single => "single",
            },
        );
    }

    if let Some(renderer) = core.option.graph_renderer {
        set_nested_string(
            &mut doc,
            &["core", "option", "graph_renderer"],
            match renderer {
                crate::GraphRenderer::Auto => "auto",
                crate::GraphRenderer::Ascii => "ascii",
                crate::GraphRenderer::Kitty => "kitty",
                crate::GraphRenderer::Iterm => "iterm",
                crate::GraphRenderer::KittyUnicode => "kitty-unicode",
                crate::GraphRenderer::Sixel => "sixel",
            },
        );
    }

    if let Some(sel) = core.option.initial_selection {
        set_nested_string(
            &mut doc,
            &["core", "option", "initial_selection"],
            match sel {
                crate::InitialSelection::Latest => "latest",
                crate::InitialSelection::Head => "head",
            },
        );
    }

    if let Some(order) = core.option.order {
        set_nested_string(
            &mut doc,
            &["core", "option", "order"],
            match order {
                crate::CommitOrderType::Chrono => "chrono",
                crate::CommitOrderType::Topo => "topo",
            },
        );
    }

    // Persist the commit-load limits. Both fields were missing from
    // earlier `save()` calls, the in-memory value updated cleanly
    // when the user edited them in the Config view but reverted to the
    // file's previous value on the next launch.
    set_nested_string(
        &mut doc,
        &["ui", "common", "diff_mode"],
        match ui.common.diff_mode {
            DiffMode::Enhanced => "enhanced",
            DiffMode::Raw => "raw",
            DiffMode::SideBySide => "side-by-side",
            DiffMode::SideBySideEnhanced => "side-by-side-enhanced",
        },
    );

    set_nested_string(
        &mut doc,
        &["ui", "common", "conflict_view"],
        match ui.common.conflict_view {
            ConflictViewMode::ThreePane => "three-pane",
            ConflictViewMode::TwoPane => "two-pane",
            ConflictViewMode::Inline => "inline",
        },
    );

    set_nested_string(
        &mut doc,
        &["ui", "common", "rebase_view"],
        match ui.common.rebase_view {
            RebaseViewMode::Compact => "compact",
            RebaseViewMode::Inline => "inline",
            RebaseViewMode::Split => "split",
        },
    );

    set_nested_bool(
        &mut doc,
        &["ui", "common", "mouse_enabled"],
        ui.common.mouse_enabled,
    );

    set_nested_bool(
        &mut doc,
        &["core", "search", "ignore_case"],
        core.search.ignore_case,
    );
    set_nested_bool(&mut doc, &["core", "search", "fuzzy"], core.search.fuzzy);
    set_nested_bool(&mut doc, &["core", "search", "regex"], core.search.regex);

    set_nested_string(&mut doc, &["core", "option", "theme"], &core.option.theme);

    set_nested_string(
        &mut doc,
        &["core", "option", "syntax_theme"],
        &core.option.syntax_theme,
    );

    set_nested_string(
        &mut doc,
        &["core", "option", "date_time_format"],
        match core.option.date_time_format {
            DateTimeFormat::DDMMYYYY_HHMM => "ddmmyyyy_hhmm",
            DateTimeFormat::DDMMYYYY => "ddmmyyyy",
            DateTimeFormat::MMDDYYYY_HHMM => "mmddyyyy_hhmm",
            DateTimeFormat::YYYYMMDD_HHMM => "yyyymmdd_hhmm",
            DateTimeFormat::ISO => "iso",
            DateTimeFormat::YYYYMMDD_HHMM_DASH => "yyyymmdd_hhmm_dash",
        },
    );

    set_nested_bool(
        &mut doc,
        &["core", "option", "date_time_local"],
        core.option.date_time_local,
    );

    set_nested_option_string(
        &mut doc,
        &["core", "option", "user_name"],
        &core.option.user_name,
    );
    set_nested_option_string(
        &mut doc,
        &["core", "option", "user_email"],
        &core.option.user_email,
    );

    set_nested_option_string(
        &mut doc,
        &["core", "option", "default_branch"],
        &core.option.default_branch,
    );

    set_nested_bool(
        &mut doc,
        &["core", "option", "github_avatars"],
        core.option.github_avatars,
    );

    set_nested_bool(
        &mut doc,
        &["ui", "list", "graph_enabled"],
        ui.list.graph_enabled,
    );

    let toml_string =
        toml::to_string_pretty(&doc).map_err(|e| format!("Failed to serialize config: {}", e))?;
    std::fs::write(&path, toml_string).map_err(|e| format!("Failed to write config: {}", e))?;
    Ok(())
}

fn set_nested_string(doc: &mut toml::Table, keys: &[&str], value: &str) {
    let mut table = doc;
    for key in &keys[..keys.len() - 1] {
        table = table
            .entry(key.to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .unwrap();
    }
    table.insert(
        keys.last().unwrap().to_string(),
        toml::Value::String(value.to_string()),
    );
}

fn set_nested_bool(doc: &mut toml::Table, keys: &[&str], value: bool) {
    let mut table = doc;
    for key in &keys[..keys.len() - 1] {
        table = table
            .entry(key.to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .unwrap();
    }
    table.insert(
        keys.last().unwrap().to_string(),
        toml::Value::Boolean(value),
    );
}

fn set_nested_option_string(doc: &mut toml::Table, keys: &[&str], value: &Option<String>) {
    let mut table = doc;
    for key in &keys[..keys.len() - 1] {
        table = table
            .entry(key.to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .unwrap();
    }
    let last_key = keys.last().unwrap().to_string();
    match value {
        Some(v) => {
            table.insert(last_key, toml::Value::String(v.clone()));
        }
        None => {
            table.remove(&last_key);
        }
    }
}

/// Load the config from disk and surface ANY problem (parse, schema
/// validation, unknown theme, …) as a structured `ConfigDiagnostic`
/// instead of the legacy `Error::Config(String)` blob. Used at boot
/// time so the entry point can render the failure as a styled `--help`
/// -like error block and exit cleanly instead of dumping a Debug
/// stack.
///
/// Returns the same tuple shape as `load()` so the boot loop can use
/// it as a drop-in replacement for the first iteration.
#[allow(clippy::type_complexity)]
pub fn load_or_diagnose() -> std::result::Result<
    (
        CoreConfig,
        UiConfig,
        GraphConfig,
        ColorTheme,
        Option<KeyBinds>,
    ),
    ConfigDiagnostic,
> {
    // Resolve which file to read, env var beats default path. If
    // GITOUI_CONFIG_FILE is set but missing → hard fail; everything
    // else (no env var, default path missing) gracefully uses the
    // built-in defaults.
    let (chosen_path, must_exist) = match config_file_path_from_env() {
        Some(p) => (Some(p), true),
        None => (config_file_path(), false),
    };

    let raw_config = if let Some(path) = chosen_path.as_ref() {
        if !path.exists() {
            if must_exist {
                return Err(ConfigDiagnostic::from_message(
                    format!(
                        "Config file specified by ${CONFIG_FILE_ENV_NAME} \
                         environment variable not found"
                    ),
                    Some(path.clone()),
                ));
            }
            Config::default()
        } else {
            let content = std::fs::read_to_string(path).map_err(|e| {
                ConfigDiagnostic::from_message(format!("read: {e}"), Some(path.clone()))
            })?;
            let parsed: OptionalConfig = toml::from_str(&content)
                .map_err(|e| ConfigDiagnostic::from_toml(e, path.clone()))?;
            parsed.into()
        }
    } else {
        Config::default()
    };

    raw_config
        .validate()
        .map_err(|e| ConfigDiagnostic::from_validation(&e, chosen_path.clone()))?;

    // Theme name check, empty string means "use the built-in
    // default palette", which is allowed. A non-empty string MUST
    // resolve to a known built-in OR a `~/.config/gitoui/themes/<name>.toml`
    // file; otherwise we'd silently fall back to defaults and the user
    // would never know their typo.
    let theme_name = &raw_config.core.option.theme;
    if !theme_name.is_empty() {
        crate::themes::resolve_or_load(theme_name)
            .map_err(ConfigDiagnostic::from_theme_load_error)?;
    }

    Ok((
        raw_config.core,
        raw_config.ui,
        raw_config.graph,
        raw_config.color,
        raw_config.keybind,
    ))
}

// ─────────────────────────────────────────────────────────────────────
// Config diagnostics, pretty error path for invalid user configs.
//
// When `gitoui::run()` boots, the first `config::load()` call may fail
// for a handful of reasons: a TOML syntax error, an unknown field, a
// keybind conflict, an invalid hex color, an unknown named theme, …
// We render those as a single styled error block (brand splash on
// top, red title, dim path, the raw cause, then a docs hint) and exit
// instead of letting Rust's default `Error: ...` dump leak into the
// terminal. Mirrors the `--help` entry point so users see a coherent
// "we couldn't start" page instead of a stack trace.
// ─────────────────────────────────────────────────────────────────────

/// Structured description of a config-load failure. Built from
/// concrete error types as we catch them, then rendered to stderr by
/// `write_to_stderr` at the very top of the boot flow.
#[derive(Debug, Clone)]
pub struct ConfigDiagnostic {
    /// Short headline shown in red. Always something like
    /// "configuration error" or "theme not found".
    pub title: &'static str,
    /// One-line summary of what went wrong.
    pub summary: String,
    /// Optional multi-line body, verbatim TOML parse output, a list
    /// of valid options, the offending key, etc.
    pub details: Vec<String>,
    /// Offending file. None for synthetic errors (e.g. CLI args).
    pub path: Option<PathBuf>,
    /// Tail line in dim, typically a docs URL or a quick remedy.
    pub hint: Option<String>,
}

impl ConfigDiagnostic {
    /// Wrap a `toml::de::Error` (parse failure), TOML's own message
    /// already includes "expected" details + a line/col context, so
    /// we drop it in verbatim under the title.
    pub fn from_toml(err: toml::de::Error, path: PathBuf) -> Self {
        Self {
            title: "configuration error",
            summary: "Could not parse config TOML".to_string(),
            details: err.to_string().lines().map(str::to_string).collect(),
            path: Some(path),
            hint: Some(
                "Docs: https://nayji7.github.io/gitoui/configurations/config-file-format.html"
                    .to_string(),
            ),
        }
    }

    /// `garde::Report` carries one or more field-level validation
    /// failures (range, regex, custom predicates). We flatten the
    /// report into one line per failed field.
    pub fn from_validation(err: &garde::Report, path: Option<PathBuf>) -> Self {
        let details = err
            .iter()
            .map(|(field, e)| format!("[{field}] {e}"))
            .collect();
        Self {
            title: "configuration error",
            summary: "Some config values are out of range".to_string(),
            details,
            path,
            hint: Some(
                "Docs: https://nayji7.github.io/gitoui/configurations/config-file-format.html"
                    .to_string(),
            ),
        }
    }

    /// Surface a `ThemeLoadError` (raised when resolving
    /// `core.option.theme` against built-ins + the user's themes/
    /// directory) as a structured diagnostic.
    pub fn from_theme_load_error(err: crate::themes::ThemeLoadError) -> Self {
        use crate::themes::ThemeLoadError;
        match err {
            ThemeLoadError::NotFound { name, searched } => Self {
                title: "theme not found",
                summary: format!("Unknown theme: {name:?}"),
                details: {
                    let mut d = vec!["Built-in themes:".to_string()];
                    for chunk in crate::themes::list_themes().chunks(4) {
                        d.push(format!("  {}", chunk.join(", ")));
                    }
                    d.push(String::new());
                    d.push(format!(
                        "Custom themes would be loaded from: {}",
                        searched.display()
                    ));
                    d
                },
                path: None,
                hint: Some(
                    "Docs: https://nayji7.github.io/gitoui/configurations/themes.html".to_string(),
                ),
            },
            ThemeLoadError::BadFile { path, source } => Self {
                title: "theme file invalid",
                summary: "Could not parse the custom theme TOML".to_string(),
                details: source.to_string().lines().map(str::to_string).collect(),
                path: Some(path),
                hint: Some(
                    "Docs: https://nayji7.github.io/gitoui/configurations/themes.html".to_string(),
                ),
            },
            ThemeLoadError::UnknownBase { path, name, base } => Self {
                title: "theme inherits unknown base",
                summary: format!("`{name}` sets base = {base:?} (not a built-in)"),
                details: {
                    let mut d = vec!["Valid base names:".to_string()];
                    for chunk in crate::themes::list_themes().chunks(4) {
                        d.push(format!("  {}", chunk.join(", ")));
                    }
                    d
                },
                path: Some(path),
                hint: Some(
                    "Docs: https://nayji7.github.io/gitoui/configurations/themes.html".to_string(),
                ),
            },
            ThemeLoadError::Io { path, source } => Self {
                title: "theme file unreadable",
                summary: source.to_string(),
                details: Vec::new(),
                path: Some(path),
                hint: None,
            },
        }
    }

    /// Raised when `core.option.theme = "X"` and X is neither a
    /// built-in name nor a file at `~/.config/gitoui/themes/X.toml`.
    /// (Kept around for callers that pre-date the ThemeLoadError plumbing.)
    pub fn unknown_theme(name: &str, builtins: &[&str]) -> Self {
        let mut details = vec!["Built-in themes:".to_string()];
        for chunk in builtins.chunks(4) {
            details.push(format!("  {}", chunk.join(", ")));
        }
        details.push(String::new());
        details.push("Set `core.option.theme` to one of the above, or drop a".to_string());
        details.push("TOML file at ~/.config/gitoui/themes/<name>.toml.".to_string());
        Self {
            title: "theme not found",
            summary: format!("Unknown theme: {name:?}"),
            details,
            path: None,
            hint: Some(
                "Docs: https://nayji7.github.io/gitoui/configurations/themes.html".to_string(),
            ),
        }
    }

    /// Wrap any other config error from the legacy `Error::Config`
    /// path so it goes through the same pretty printer.
    pub fn from_message(msg: impl Into<String>, path: Option<PathBuf>) -> Self {
        Self {
            title: "configuration error",
            summary: msg.into(),
            details: Vec::new(),
            path,
            hint: Some(
                "Docs: https://nayji7.github.io/gitoui/configurations/config-file-format.html"
                    .to_string(),
            ),
        }
    }

    /// Write the diagnostic to stderr with ANSI styling when the
    /// stream is a TTY; raw text otherwise. The caller is expected
    /// to have rendered the brand splash beforehand (so the error
    /// reads as a continuation of the `gitoui --help` look).
    pub fn write_to_stderr(&self) {
        use std::io::IsTerminal;
        let tty = std::io::stderr().is_terminal();
        // Brand orange #F05133 for the title, matches the splash.
        let red = if tty { "\x1b[1;38;2;240;81;51m" } else { "" };
        let dim = if tty { "\x1b[2m" } else { "" };
        let reset = if tty { "\x1b[0m" } else { "" };

        eprintln!();
        eprintln!("  {red}gitoui: {}{reset}, {}", self.title, self.summary);
        if let Some(path) = &self.path {
            eprintln!("  {dim}{}{reset}", path.display());
        }
        eprintln!();
        for line in &self.details {
            eprintln!("    {line}");
        }
        if let Some(hint) = &self.hint {
            eprintln!();
            eprintln!("  {dim}{hint}{reset}");
        }
        eprintln!();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        // Spot-check the defaults users notice when first running gitoui, anything
        // that, if silently changed, would surprise the user or affect first-run
        // behavior. The full struct shape is exercised via `Config::default()` as
        // the base in the partial / complete TOML tests below.
        let cfg = Config::default();

        // Auto-refresh on by default with anti-flicker debounce.
        assert!(cfg.core.option.auto_refresh);
        assert_eq!(cfg.core.option.auto_refresh_debounce_ms, 500);

        // Theming defaults that ship with the binary.
        assert_eq!(cfg.core.option.theme, "Tokyo Night");
        assert_eq!(cfg.core.option.syntax_theme, "base16-ocean.dark");

        // UX defaults.
        assert!(cfg.ui.common.mouse_enabled);
        assert_eq!(cfg.ui.common.cursor_type, CursorType::Native);
        assert_eq!(cfg.ui.common.diff_mode, DiffMode::Enhanced);
        assert!(cfg.core.option.github_avatars);
        assert!(cfg.core.option.date_time_local);
        assert_eq!(
            cfg.core.option.date_time_format,
            DateTimeFormat::DDMMYYYY_HHMM
        );

        // Default user command should be the inline `git diff` preview.
        let cmd = cfg
            .core
            .user_command
            .commands
            .get("1")
            .expect("default user command 1 should exist");
        assert_eq!(cmd.name, "git diff");
        assert_eq!(cmd.r#type, UserCommandType::Inline);

        // Default columns include the graph + commit message at minimum.
        assert!(cfg.ui.list.columns.contains(&UserListColumnType::Graph));
        assert!(cfg
            .ui
            .list
            .columns
            .contains(&UserListColumnType::CommitMessage));

        // Graph branch palette must be non-empty (otherwise the graph wouldn't render).
        assert!(!cfg.graph.color.branches.is_empty());
    }

    #[test]
    fn test_config_complete_toml() {
        let toml = r##"
            [core.option]
            graph_renderer = "kitty-unicode"
            order = "topo"
            graph_width = "single"
            graph_style = "angular"
            initial_selection = "head"
            [core.search]
            ignore_case = true
            fuzzy = true
            [core.user_command]
            commands_1 = { name = "git diff no color", commands = ["git", "diff", "{{first_parent_hash}}", "{{target_hash}}"] }
            commands_2 = { name = "echo hello", type = "silent", commands = ["echo", "hello"], refresh = true }
            commands_10 = { name = "echo world", type = "inline", commands = ["echo", "world"], refresh = false }
            commands_3 = { name = "open vim", type = "suspend", commands = ["vim"] }
            tab_width = 2
            [ui.common]
            cursor_type = { Virtual = "|" }
            [ui.list]
            columns = ["date", "commit_message", "hash", "graph"]
            commit_message_min_width = 40
            date_format = "%Y/%m/%d"
            date_width = 20
            date_local = false
            name_width = 30
            [ui.detail]
            height = 30
            date_format = "%Y/%m/%d %H:%M:%S"
            date_local = false
            [ui.user_command]
            height = 30
            [ui.refs]
            width = 40
            [graph]
            row_image_width = "fixed"
            [graph.color]
            branches = ["#ff0000", "#00ff00", "#0000ff"]
            edge = "#000000"
            background = "#ffffff"
        "##;
        let actual: Config = toml::from_str::<OptionalConfig>(toml).unwrap().into();

        // Build expected by overriding only the fields the TOML changes, defaults
        // come from `Config::default()`. Adding a new default no longer requires
        // touching this test.
        let mut expected = Config::default();
        expected.core.option.graph_renderer = Some(GraphRenderer::KittyUnicode);
        expected.core.option.order = Some(CommitOrderType::Topo);
        expected.core.option.graph_width = Some(GraphWidthType::Single);
        expected.core.option.graph_style = Some(GraphStyle::Angular);
        expected.core.option.initial_selection = Some(InitialSelection::Head);
        expected.core.search.ignore_case = true;
        expected.core.search.fuzzy = true;
        expected.core.user_command.commands = FxHashMap::from_iter([
            (
                "1".into(),
                UserCommand {
                    name: "git diff no color".into(),
                    r#type: UserCommandType::Inline,
                    commands: vec![
                        "git".into(),
                        "diff".into(),
                        "{{first_parent_hash}}".into(),
                        "{{target_hash}}".into(),
                    ],
                    refresh: false,
                },
            ),
            (
                "2".into(),
                UserCommand {
                    name: "echo hello".into(),
                    r#type: UserCommandType::Silent,
                    commands: vec!["echo".into(), "hello".into()],
                    refresh: true,
                },
            ),
            (
                "10".into(),
                UserCommand {
                    name: "echo world".into(),
                    r#type: UserCommandType::Inline,
                    commands: vec!["echo".into(), "world".into()],
                    refresh: false,
                },
            ),
            (
                "3".into(),
                UserCommand {
                    name: "open vim".into(),
                    r#type: UserCommandType::Suspend,
                    commands: vec!["vim".into()],
                    refresh: false,
                },
            ),
        ]);
        expected.core.user_command.tab_width = 2;
        expected.ui.common.cursor_type = CursorType::Virtual("|".into());
        expected.ui.list.columns = vec![
            UserListColumnType::Date,
            UserListColumnType::CommitMessage,
            UserListColumnType::Hash,
            UserListColumnType::Graph,
        ];
        expected.ui.list.commit_message_min_width = 40;
        expected.ui.list.date_format = "%Y/%m/%d".into();
        expected.ui.list.date_local = false;
        expected.ui.list.name_width = 30;
        expected.ui.detail.height = 30;
        expected.ui.detail.date_format = "%Y/%m/%d %H:%M:%S".into();
        expected.ui.detail.date_local = false;
        expected.ui.user_command.height = 30;
        expected.ui.refs.width = 40;
        expected.graph.row_image_width = GraphImageWidthMode::Fixed;
        expected.graph.color.branches = vec!["#ff0000".into(), "#00ff00".into(), "#0000ff".into()];
        expected.graph.color.edge = "#000000".into();
        expected.graph.color.background = "#ffffff".into();

        assert_eq!(actual, expected);
    }

    #[test]
    fn test_config_partial_toml() {
        let toml = r#"
            [ui.list]
            date_format = "%Y/%m/%d"
        "#;
        let actual: Config = toml::from_str::<OptionalConfig>(toml).unwrap().into();

        // Partial config: only the explicit field changes; everything else inherits
        // from `Config::default()`.
        let mut expected = Config::default();
        expected.ui.list.date_format = "%Y/%m/%d".into();

        assert_eq!(actual, expected);
    }

    #[test]
    fn test_config_clipboard_auto() {
        let toml = r#"
            [core.external]
            clipboard = "Auto"
        "#;
        let config: Config = toml::from_str::<OptionalConfig>(toml).unwrap().into();
        assert_eq!(config.core.external.clipboard, ClipboardConfig::Auto);
    }

    #[test]
    fn test_config_clipboard_custom_single_command() {
        let toml = r#"
            [core.external]
            clipboard = { Custom = { commands = ["wl-copy"] } }
        "#;
        let config: Config = toml::from_str::<OptionalConfig>(toml).unwrap().into();
        assert_eq!(
            config.core.external.clipboard,
            ClipboardConfig::Custom {
                commands: vec!["wl-copy".into()]
            }
        );
    }

    #[test]
    fn test_config_clipboard_custom_command_with_args() {
        let toml = r#"
            [core.external]
            clipboard = { Custom = { commands = ["xclip", "-selection", "clipboard"] } }
        "#;
        let config: Config = toml::from_str::<OptionalConfig>(toml).unwrap().into();
        assert_eq!(
            config.core.external.clipboard,
            ClipboardConfig::Custom {
                commands: vec!["xclip".into(), "-selection".into(), "clipboard".into()]
            }
        );
    }

    #[test]
    fn test_config_syntax_theme_roundtrip() {
        let mut core = CoreConfig::default();
        let ui = UiConfig::default();
        core.option.syntax_theme = "Monokai Extended".into();

        let temp_dir = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", temp_dir.path());
        let app_dir = temp_dir.path().join("gitoui");
        std::fs::create_dir_all(&app_dir).unwrap();

        // Save config
        save(&core, &ui).unwrap();

        // Load config
        let (loaded_core, _, _, _, _) = load().unwrap();
        assert_eq!(loaded_core.option.syntax_theme, "Monokai Extended");
    }
}

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
    keybind::KeyBind,
    CommitOrderType, GraphStyle, GraphWidthType, ImageProtocolType, InitialSelection, Result,
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
const APP_DIR_NAME: &str = "gitui";
const CONFIG_FILE_NAME: &str = "config.toml";
const CONFIG_FILE_ENV_NAME: &str = "GITUI_CONFIG_FILE";

pub fn load() -> Result<(
    CoreConfig,
    UiConfig,
    GraphConfig,
    ColorTheme,
    Option<KeyBind>,
)> {
    let config = match config_file_path_from_env() {
        Some(user_path) => {
            if !user_path.exists() {
                let msg = format!(
                    "Config file specified by ${CONFIG_FILE_ENV_NAME} environment variable not found: {}",
                    user_path.display()
                );
                return Err(msg.into());
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
    Ok(config.into())
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
    keybind: Option<KeyBind>,
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
    pub protocol: Option<ImageProtocolType>,
    pub order: Option<CommitOrderType>,
    pub graph_width: Option<GraphWidthType>,
    pub graph_style: Option<GraphStyle>,
    pub initial_selection: Option<InitialSelection>,
    #[default = true]
    pub auto_refresh: bool,
    #[default = 500]
    pub auto_refresh_debounce_ms: u64,
    #[default = 500]
    pub initial_load_count: usize,
    #[default = 200]
    pub load_more_count: usize,
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
    #[default("main".to_string())]
    pub default_branch: String,
    #[default = true]
    pub github_avatars: bool,
}

#[optional(derives = [Deserialize])]
#[derive(Debug, Clone, PartialEq, Eq, SmartDefault)]
pub struct CoreSearchConfig {
    #[default = false]
    pub ignore_case: bool,
    #[default = false]
    pub fuzzy: bool,
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
#[serde(rename_all = "lowercase")]
pub enum DiffMode {
    #[default]
    Enhanced,
    Raw,
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
    #[default = 26]
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
    #[default(vec![
        "#1f77b4".into(), // steel blue
        "#d62728".into(), // brick red
        "#2ca02c".into(), // forest green
        "#ff7f0e".into(), // orange
        "#9467bd".into(), // purple
        "#8c564b".into(), // brown
        "#e377c2".into(), // pink/rose
        "#17becf".into(), // teal
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
    pub fn protocol(&self) -> Option<crate::ImageProtocolType> {
        self.option.protocol
    }
    pub fn set_protocol(&mut self, protocol: crate::ImageProtocolType) {
        self.option.protocol = Some(protocol);
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
    pub fn default_branch(&self) -> &str {
        &self.option.default_branch
    }
    pub fn set_default_branch(&mut self, name: Option<String>) {
        self.option.default_branch = name.unwrap_or_else(|| "main".to_string());
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

    if let Some(protocol) = core.option.protocol {
        set_nested_string(
            &mut doc,
            &["core", "option", "protocol"],
            match protocol {
                crate::ImageProtocolType::Auto => "auto",
                crate::ImageProtocolType::Iterm => "iterm",
                crate::ImageProtocolType::Kitty => "kitty",
                crate::ImageProtocolType::KittyUnicode => "kitty-unicode",
                crate::ImageProtocolType::Sixel => "sixel",
            },
        );
    }

    set_nested_string(
        &mut doc,
        &["ui", "common", "diff_mode"],
        match ui.common.diff_mode {
            DiffMode::Enhanced => "enhanced",
            DiffMode::Raw => "raw",
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

    set_nested_string(
        &mut doc,
        &["core", "option", "theme"],
        &core.option.theme,
    );

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

    set_nested_string(
        &mut doc,
        &["core", "option", "default_branch"],
        &core.option.default_branch,
    );

    set_nested_bool(
        &mut doc,
        &["core", "option", "github_avatars"],
        core.option.github_avatars,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let actual = Config::default();
        let expected = Config {
            core: CoreConfig {
                option: CoreOptionConfig {
                    protocol: None,
                    order: None,
                    graph_width: None,
                    graph_style: None,
                    initial_selection: None,
                    auto_refresh: true,
                    auto_refresh_debounce_ms: 500,
                    initial_load_count: 500,
                    load_more_count: 200,
                    theme: "Tokyo Night".into(),
                    syntax_theme: "base16-ocean.dark".into(),
                    date_time_format: DateTimeFormat::DDMMYYYY_HHMM,
                    date_time_local: true,
                    user_name: None,
                    user_email: None,
                    default_branch: "main".into(),
                    github_avatars: true,
                },
                search: CoreSearchConfig {
                    ignore_case: false,
                    fuzzy: false,
                },
                user_command: CoreUserCommandConfig {
                    commands: FxHashMap::from_iter([(
                        "1".into(),
                        UserCommand {
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
                        },
                    )]),
                    tab_width: 4,
                },
                external: CoreExternalConfig {
                    clipboard: ClipboardConfig::Auto,
                },
            },
            ui: UiConfig {
                common: UiCommonConfig {
                    cursor_type: CursorType::Native,
                    mouse_enabled: true,
                    diff_mode: DiffMode::Enhanced,
                },
                list: UiListConfig {
                    columns: vec![
                        UserListColumnType::Graph,
                        UserListColumnType::Marker,
                        UserListColumnType::CommitMessage,
                        UserListColumnType::Name,
                        UserListColumnType::Hash,
                        UserListColumnType::Date,
                    ],
                    commit_message_min_width: 20,
                    date_format: "%d/%m/%Y - %H:%M".into(),
                    date_width: 20,
                    date_local: true,
                    name_width: 20,
                },
                detail: UiDetailConfig {
                    height: 20,
                    date_format: "%Y-%m-%d %H:%M:%S %z".into(),
                    date_local: true,
                },
                user_command: UiUserCommandConfig { height: 20 },
                refs: UiRefsConfig { width: 26 },
            },
            graph: GraphConfig {
                row_image_width: GraphImageWidthMode::Compact,
                color: GraphColorConfig {
                    branches: vec![
                        "#1f77b4".into(), // steel blue
                        "#d62728".into(), // brick red
                        "#2ca02c".into(), // forest green
                        "#ff7f0e".into(), // orange
                        "#9467bd".into(), // purple
                        "#8c564b".into(), // brown
                        "#e377c2".into(), // pink/rose
                        "#17becf".into(), // teal
                    ],
                    edge: "#00000000".into(),
                    background: "#00000000".into(),
                },
            },
            color: ColorTheme::default(),
            keybind: None,
        };
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_config_complete_toml() {
        let toml = r##"
            [core.option]
            protocol = "kitty-unicode"
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
        let expected = Config {
            core: CoreConfig {
                option: CoreOptionConfig {
                    protocol: Some(ImageProtocolType::KittyUnicode),
                    order: Some(CommitOrderType::Topo),
                    graph_width: Some(GraphWidthType::Single),
                    graph_style: Some(GraphStyle::Angular),
                    initial_selection: Some(InitialSelection::Head),
                    auto_refresh: true,
                    auto_refresh_debounce_ms: 500,
                    initial_load_count: 500,
                    load_more_count: 200,
                    theme: "Tokyo Night".into(),
                    syntax_theme: "base16-ocean.dark".into(),
                    date_time_format: DateTimeFormat::DDMMYYYY_HHMM,
                    date_time_local: true,
                    user_name: None,
                    user_email: None,
                    default_branch: "main".into(),
                    github_avatars: true,
                },
                search: CoreSearchConfig {
                    ignore_case: true,
                    fuzzy: true,
                },
                user_command: CoreUserCommandConfig {
                    commands: FxHashMap::from_iter([
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
                    ]),
                    tab_width: 2,
                },
                external: CoreExternalConfig {
                    clipboard: ClipboardConfig::Auto,
                },
            },
            ui: UiConfig {
                common: UiCommonConfig {
                    cursor_type: CursorType::Virtual("|".into()),
                    mouse_enabled: true,
                    diff_mode: DiffMode::Enhanced,
                },
                list: UiListConfig {
                    columns: vec![
                        UserListColumnType::Date,
                        UserListColumnType::CommitMessage,
                        UserListColumnType::Hash,
                        UserListColumnType::Graph,
                    ],
                    commit_message_min_width: 40,
                    date_format: "%Y/%m/%d".into(),
                    date_width: 20,
                    date_local: false,
                    name_width: 30,
                },
                detail: UiDetailConfig {
                    height: 30,
                    date_format: "%Y/%m/%d %H:%M:%S".into(),
                    date_local: false,
                },
                user_command: UiUserCommandConfig { height: 30 },
                refs: UiRefsConfig { width: 40 },
            },
            graph: GraphConfig {
                row_image_width: GraphImageWidthMode::Fixed,
                color: GraphColorConfig {
                    branches: vec!["#ff0000".into(), "#00ff00".into(), "#0000ff".into()],
                    edge: "#000000".into(),
                    background: "#ffffff".into(),
                },
            },
            color: ColorTheme::default(),
            keybind: None,
        };
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_config_partial_toml() {
        let toml = r#"
            [ui.list]
            date_format = "%Y/%m/%d"
        "#;
        let actual: Config = toml::from_str::<OptionalConfig>(toml).unwrap().into();
        let expected = Config {
            core: CoreConfig {
                option: CoreOptionConfig {
                    protocol: None,
                    order: None,
                    graph_width: None,
                    graph_style: None,
                    initial_selection: None,
                    auto_refresh: true,
                    auto_refresh_debounce_ms: 500,
                    initial_load_count: 500,
                    load_more_count: 200,
                    theme: "Tokyo Night".into(),
                    syntax_theme: "base16-ocean.dark".into(),
                    date_time_format: DateTimeFormat::DDMMYYYY_HHMM,
                    date_time_local: true,
                    user_name: None,
                    user_email: None,
                    default_branch: "main".into(),
                    github_avatars: true,
                },
                search: CoreSearchConfig {
                    ignore_case: false,
                    fuzzy: false,
                },
                user_command: CoreUserCommandConfig {
                    commands: FxHashMap::from_iter([(
                        "1".into(),
                        UserCommand {
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
                        },
                    )]),
                    tab_width: 4,
                },
                external: CoreExternalConfig {
                    clipboard: ClipboardConfig::Auto,
                },
            },
            ui: UiConfig {
                common: UiCommonConfig {
                    cursor_type: CursorType::Native,
                    mouse_enabled: true,
                    diff_mode: DiffMode::Enhanced,
                },
                list: UiListConfig {
                    columns: vec![
                        UserListColumnType::Graph,
                        UserListColumnType::Marker,
                        UserListColumnType::CommitMessage,
                        UserListColumnType::Name,
                        UserListColumnType::Hash,
                        UserListColumnType::Date,
                    ],
                    commit_message_min_width: 20,
                    date_format: "%Y/%m/%d".into(),
                    date_width: 20,
                    date_local: true,
                    name_width: 20,
                },
                detail: UiDetailConfig {
                    height: 20,
                    date_format: "%Y-%m-%d %H:%M:%S %z".into(),
                    date_local: true,
                },
                user_command: UiUserCommandConfig { height: 20 },
                refs: UiRefsConfig { width: 26 },
            },
            graph: GraphConfig {
                row_image_width: GraphImageWidthMode::Compact,
                color: GraphColorConfig {
                    branches: vec![
                        "#1f77b4".into(), // steel blue
                        "#d62728".into(), // brick red
                        "#2ca02c".into(), // forest green
                        "#ff7f0e".into(), // orange
                        "#9467bd".into(), // purple
                        "#8c564b".into(), // brown
                        "#e377c2".into(), // pink/rose
                        "#17becf".into(), // teal
                    ],
                    edge: "#00000000".into(),
                    background: "#00000000".into(),
                },
            },
            color: ColorTheme::default(),
            keybind: None,
        };
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
        let app_dir = temp_dir.path().join("gitui");
        std::fs::create_dir_all(&app_dir).unwrap();

        // Save config
        save(&core, &ui).unwrap();

        // Load config
        let (loaded_core, _, _, _, _) = load().unwrap();
        assert_eq!(loaded_core.option.syntax_theme, "Monokai Extended");
    }
}

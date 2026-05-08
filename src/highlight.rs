use once_cell::sync::Lazy;
use ratatui::style::{Color, Style};
use syntect::{
    easy::HighlightLines,
    highlighting::ThemeSet,
    parsing::SyntaxSet,
};

static SYNTAX_SET: Lazy<SyntaxSet> = Lazy::new(|| {
    let mut builder = SyntaxSet::load_defaults_newlines().into_builder();
    // Load additional syntaxes from assets/syntaxes if the directory exists
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let syntaxes_dir = exe_dir.join("assets").join("syntaxes");
            if syntaxes_dir.exists() {
                let _ = builder.add_from_folder(&syntaxes_dir, true);
            }
        }
    }
    // Also try from the source directory (for development)
    let dev_syntaxes = std::path::PathBuf::from("assets/syntaxes");
    if dev_syntaxes.exists() {
        let _ = builder.add_from_folder(&dev_syntaxes, true);
    }
    builder.build()
});

// Embed custom themes directly into the binary so they're always available
static THEME_SET: Lazy<ThemeSet> = Lazy::new(|| {
    let mut ts = ThemeSet::load_defaults();

    macro_rules! embed_theme {
        ($name:expr, $path:expr) => {{
            static THEME_XML: &str = include_str!($path);
            let mut cursor = std::io::Cursor::new(THEME_XML.as_bytes());
            match ThemeSet::load_from_reader(&mut cursor) {
                Ok(theme) => {
                    let theme_name = theme.name.clone().unwrap_or_else(|| $name.to_string());
                    ts.themes.insert(theme_name, theme);
                }
                Err(e) => {
                    eprintln!(
                        "[THEME_SET] Failed to parse embedded {} theme: {:?}",
                        $name, e
                    );
                }
            }
        }};
    }

    // Load all embedded custom themes
    embed_theme!("Dracula", "../assets/themes/Dracula.tmTheme");
    embed_theme!("Monokai", "../assets/themes/Monokai.tmTheme");
    embed_theme!("3024 Day", "../assets/themes/3024Day.tmTheme");
    embed_theme!("Agola Dark", "../assets/themes/AgolaDark.tmTheme");
    embed_theme!("Blackboard", "../assets/themes/Blackboard.tmTheme");
    embed_theme!("Cobalt", "../assets/themes/Cobalt.tmTheme");
    ts
});

fn syntect_color_to_ratatui(c: syntect::highlighting::Color) -> Color {
    Color::Rgb(c.r, c.g, c.b)
}

pub struct SyntaxHighlighter {
    highlighter: HighlightLines<'static>,
}

impl SyntaxHighlighter {
    pub fn new(file_path: &str) -> Option<Self> {
        Self::new_with_theme(file_path, "base16-ocean.dark")
    }

    pub fn new_with_theme(file_path: &str, theme_name: &str) -> Option<Self> {
        let syntax = Self::find_syntax(file_path)?;
        let theme = THEME_SET
            .themes
            .get(theme_name)
            .or_else(|| THEME_SET.themes.get("base16-ocean.dark"))
            .unwrap_or_else(|| &THEME_SET.themes["InspiredGitHub"]);
        let highlighter = HighlightLines::new(syntax, theme);
        Some(Self { highlighter })
    }

    fn find_syntax(file_path: &str) -> Option<&'static syntect::parsing::SyntaxReference> {
        // Try standard lookup by file path
        if let Ok(Some(syntax)) = SYNTAX_SET.find_syntax_for_file(file_path) {
            return Some(syntax);
        }
        // Try by extension
        let ext = std::path::Path::new(file_path)
            .extension()
            .and_then(|e| e.to_str())?;
        if let Some(syntax) = SYNTAX_SET.find_syntax_by_extension(ext) {
            return Some(syntax);
        }
        // Fallback mappings for common extensions not in default set
        match ext {
            "ts" => SYNTAX_SET.find_syntax_by_extension("js"),
            "tsx" => SYNTAX_SET.find_syntax_by_extension("js"),
            "vue" => SYNTAX_SET.find_syntax_by_extension("html"),
            "svelte" => SYNTAX_SET.find_syntax_by_extension("html"),
            _ => None,
        }
    }

    /// Highlight a single line, returning spans with syntax colors.
    pub fn highlight_line(
        &mut self,
        content: &str,
        base_style: Style,
        _preserve_fg: Option<Color>,
    ) -> Vec<ratatui::text::Span<'static>> {
        let ranges = self
            .highlighter
            .highlight_line(content, &SYNTAX_SET)
            .unwrap_or_default();

        ranges
            .into_iter()
            .map(|(style, text)| {
                let fg = syntect_color_to_ratatui(style.foreground);
                let mut span_style = base_style;
                span_style.fg = Some(fg);
                ratatui::text::Span::styled(text.to_string(), span_style)
            })
            .collect()
    }
}

pub fn init() {
    Lazy::force(&SYNTAX_SET);
    Lazy::force(&THEME_SET);
}

pub fn list_syntax_themes() -> Vec<&'static str> {
    vec![
        "base16-ocean.dark",
        "base16-ocean.light",
        "base16-mocha.dark",
        "base16-eighties.dark",
        "InspiredGitHub",
        "Solarized (dark)",
        "Solarized (light)",
        "Dracula",
        "Monokai",
        "3024 Day",
        "Agola Dark",
        "Blackboard",
        "Cobalt",
    ]
}

use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Extra fields to show in the Terraform detail Status panel,
    /// pulled from the outputs secret.
    #[serde(default)]
    pub detail_fields: Vec<DetailField>,

    /// Custom tabs that filter Terraform resources by annotation
    /// and display configurable columns.
    #[serde(default)]
    pub custom_tabs: Vec<CustomTab>,

    /// Custom keyboard shortcuts that open a URL in the browser.
    /// Each shortcut binds a single key and defines a URL template.
    ///
    /// Available template variables:
    ///   {context}      - kubeconfig context name
    ///   {namespace}    - resource namespace
    ///   {name}         - resource name
    ///   {output.KEY}   - value from the Terraform outputs secret
    ///
    /// Example:
    ///   [[shortcuts]]
    ///   key = "b"
    ///   label = "Grafana"
    ///   url = "https://grafana.example.com/explore?cluster={context}&pod={name}"
    #[serde(default)]
    pub shortcuts: Vec<Shortcut>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shortcut {
    /// Single character key binding (e.g. "b", "B", "L")
    pub key: char,
    /// Label shown in the status bar (e.g. "Grafana Logs")
    pub label: String,
    /// URL template with {context}, {namespace}, {name}, {output.KEY} placeholders
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DetailField {
    /// Label shown in the status panel (e.g. "LKE ID")
    pub label: String,
    /// Key in the Terraform outputs secret (e.g. "cluster_id")
    pub source: String,
    /// RGB color triple, e.g. [240, 200, 60]
    #[serde(default = "default_detail_color")]
    pub color: [u8; 3],
    /// Whether to render bold
    #[serde(default)]
    pub bold: bool,
}

fn default_detail_color() -> [u8; 3] {
    [200, 210, 230]
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomTab {
    /// Tab label shown in the header (e.g. "Upgrades")
    pub name: String,
    /// Annotation key used to filter Terraform resources.
    /// Only resources carrying this annotation are shown.
    pub annotation: String,
    /// Whether the annotation value is a JSON map that should be
    /// expanded into one row per key-value pair.
    #[serde(default)]
    pub expand_json_map: bool,
    /// Column definitions for this tab.
    #[serde(default)]
    pub columns: Vec<CustomColumn>,
    /// Sort by this column name (must match a column label, case-insensitive).
    /// Falls back to first column if not specified.
    #[serde(default)]
    pub sort_by: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomColumn {
    /// Column header label
    pub label: String,
    /// Where to get the value
    pub source: CustomColumnSource,
    /// RGB color triple
    #[serde(default)]
    pub color: Option<[u8; 3]>,
    /// Whether to render bold
    #[serde(default)]
    pub bold: bool,
    /// Column width as percentage (0-100)
    #[serde(default = "default_column_width")]
    pub width: u16,
    /// Treat value as a date and color by proximity
    /// (red=past, yellow=within 7 days, green=future)
    #[serde(default)]
    pub date_highlight: bool,
}

fn default_column_width() -> u16 {
    15
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustomColumnSource {
    /// The resource namespace
    Namespace,
    /// The resource name
    Name,
    /// The Ready condition status
    Ready,
    /// The resource age
    Age,
    /// When expand_json_map is true: the JSON key from the annotation map
    AnnotationKey,
    /// When expand_json_map is true: the JSON value from the annotation map.
    /// When expand_json_map is false: the raw annotation value.
    AnnotationValue,
}

impl Config {
    pub fn load() -> Self {
        if let Some(path) = config_path() {
            if path.exists() {
                match std::fs::read_to_string(&path) {
                    Ok(contents) => match toml::from_str(&contents) {
                        Ok(config) => return config,
                        Err(e) => {
                            eprintln!("Warning: failed to parse {}: {}", path.display(), e);
                        }
                    },
                    Err(e) => {
                        eprintln!("Warning: failed to read {}: {}", path.display(), e);
                    }
                }
            }
        }
        Config::default()
    }
}

fn config_path() -> Option<PathBuf> {
    // Check TERRARIUM_CONFIG env var first
    if let Ok(path) = std::env::var("TERRARIUM_CONFIG") {
        return Some(PathBuf::from(path));
    }
    // Then ~/.config/terrarium/config.toml
    dirs_or_home().map(|d| d.join("terrarium").join("config.toml"))
}

fn dirs_or_home() -> Option<PathBuf> {
    std::env::var("XDG_CONFIG_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".config"))
        })
}

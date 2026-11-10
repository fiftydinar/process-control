//! Settings kept between runs.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::At;

/// How often the process list is read, as the settings file stores it.
///
/// A task manager is looked at, not stared at, and reading `/proc` costs
/// more than drawing the window; one second is what the rest of them do.
pub const REFRESH_CHOICES: [(u32, &str); 5] = [
    (500, "0.5 seconds"),
    (1000, "1 second"),
    (2000, "2 seconds"),
    (5000, "5 seconds"),
    (10000, "10 seconds"),
];

/// The default refresh interval, in milliseconds.
pub const DEFAULT_REFRESH: u32 = 1000;

/// How tall the process table's rows are drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RowSize {
    /// The compact rows the app has always drawn.
    #[default]
    Small,
    /// Roomier rows, with a larger icon and text.
    Larger,
    /// The roomiest rows, for a touch screen or a distant display.
    Large,
}

impl RowSize {
    /// Every size, small to large.
    pub const ALL: [RowSize; 3] = [RowSize::Small, RowSize::Larger, RowSize::Large];

    /// The name the View menu shows, which is also what the settings file
    /// stores.
    pub fn name(self) -> &'static str {
        match self {
            RowSize::Small => "Small",
            RowSize::Larger => "Larger",
            RowSize::Large => "Large",
        }
    }

    /// The size with this name, as the settings file stores it.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|size| size.name().eq_ignore_ascii_case(name))
    }
}

/// Settings written to the user's config directory.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    /// Name of the colour theme, as [`crate::theme::Theme::name`] gives it.
    pub theme: Option<String>,
    /// Milliseconds between two reads of the process list.
    pub refresh: Option<u32>,
    /// Whether processes whose name starts with this in a tree are shown
    /// with their parent, which is how a kernel thread is held under kthreadd.
    pub show_kernel_threads: Option<bool>,
    /// Whether the table draws each application's icon.
    pub show_icons: Option<bool>,
    /// Whether processes are grouped into applications by their window
    /// class, rather than listed one by one.
    pub group_apps: Option<bool>,
    /// Name of the row size, as [`RowSize::name`] gives it.
    pub row_size: Option<String>,
    /// Name of the sort column, if the list is sorted by one.
    pub sort: Option<String>,
    /// Whether the sort runs the other way round.
    pub descending: Option<bool>,
}

impl Config {
    /// Reads the settings, or gives defaults when there are none.
    pub fn load() -> Self {
        default_path()
            .as_deref()
            .map(Self::load_from)
            .unwrap_or_default()
    }

    /// Reads settings from `path`.
    pub fn load_from(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Writes the settings to the user's config directory.
    pub fn save(&self) -> Result<(), String> {
        match default_path() {
            Some(path) => self.save_to(&path),
            None => Err("no config directory for this user".to_string()),
        }
    }

    /// Writes the settings to `path`, creating its directory.
    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).at(parent)?;
        }
        let text = toml::to_string_pretty(self)
            .map_err(|error| format!("cannot encode the settings: {error}"))?;
        std::fs::write(path, text).at(path)
    }

    /// The refresh interval, from 0.5 s to 10 s.
    pub fn refresh(&self) -> u32 {
        self.refresh
            .filter(|value| (200..=60_000).contains(value))
            .unwrap_or(DEFAULT_REFRESH)
    }

    /// How often the list is read, as a span.
    pub fn refresh_span(&self) -> std::time::Duration {
        std::time::Duration::from_millis(u64::from(self.refresh()))
    }

    /// How tall the table's rows are drawn.
    pub fn row_size(&self) -> RowSize {
        self.row_size
            .as_deref()
            .and_then(RowSize::from_name)
            .unwrap_or_default()
    }
}

/// `$XDG_CONFIG_HOME/process-control/config.toml`, or the platform's own
/// directory.
fn default_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("process-control").join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("process-control-{name}-{}", std::process::id()))
    }

    #[test]
    fn settings_round_trip_through_the_file() {
        let directory = temporary("round-trip");
        let path = directory.join("config.toml");
        let config = Config {
            theme: Some("Nord".to_string()),
            refresh: Some(2000),
            show_kernel_threads: Some(true),
            show_icons: Some(false),
            group_apps: Some(false),
            row_size: Some("Large".to_string()),
            sort: Some("CPU".to_string()),
            descending: Some(true),
        };

        config.save_to(&path).expect("the settings are written");
        let loaded = Config::load_from(&path);
        std::fs::remove_dir_all(&directory).expect("the temporary directory is removed");

        assert_eq!(loaded.theme.as_deref(), Some("Nord"));
        assert_eq!(loaded.refresh(), 2000);
        assert_eq!(loaded.show_kernel_threads, Some(true));
        assert_eq!(loaded.show_icons, Some(false));
        assert_eq!(loaded.group_apps, Some(false));
        assert_eq!(loaded.row_size(), RowSize::Large);
        assert_eq!(loaded.sort.as_deref(), Some("CPU"));
        assert_eq!(loaded.descending, Some(true));
    }

    #[test]
    fn a_missing_file_gives_defaults() {
        let missing = temporary("no-such-config").join("config.toml");
        let config = Config::load_from(&missing);
        assert_eq!(config.refresh(), DEFAULT_REFRESH);
        assert_eq!(config.row_size(), RowSize::Small);
        assert!(config.theme.is_none());
    }

    #[test]
    fn a_row_size_name_round_trips_and_an_unknown_one_is_small() {
        for size in RowSize::ALL {
            assert_eq!(RowSize::from_name(size.name()), Some(size));
        }
        assert_eq!(RowSize::from_name("large"), Some(RowSize::Large));
        assert_eq!(RowSize::from_name("huge"), None);
        let config = Config {
            row_size: Some("huge".to_string()),
            ..Config::default()
        };
        assert_eq!(config.row_size(), RowSize::Small);
    }

    #[test]
    fn an_out_of_range_refresh_falls_back_to_the_default() {
        let mut config = Config {
            refresh: Some(0),
            ..Config::default()
        };
        assert_eq!(config.refresh(), DEFAULT_REFRESH);
        config.refresh = Some(120_000);
        assert_eq!(config.refresh(), DEFAULT_REFRESH);
        config.refresh = Some(500);
        assert_eq!(config.refresh(), 500);
    }
}

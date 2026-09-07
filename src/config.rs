use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    /// CalDAV account. Absent when the user only has ICS subscriptions.
    pub server: Option<ServerConfig>,
    #[serde(default)]
    pub display: DisplayConfig,
    #[serde(default)]
    pub calendars: CalendarsConfig,
    #[serde(default)]
    pub notifications: NotificationsConfig,
    #[serde(default)]
    pub subscriptions: Vec<IcsSubscription>,
    /// Per-calendar color overrides, matched on calendar name. When present
    /// these win over both a server-provided color and the random palette
    /// default.
    #[serde(default)]
    pub calendar_colors: Vec<CalendarColorEntry>,
}

/// A read-only online ICS calendar that rcal fetches and caches locally.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct IcsSubscription {
    /// Display name shown in `rcal calendars`.
    pub name: String,
    /// URL of the .ics feed.
    pub url: String,
}

/// User-assigned color for a calendar, looked up by name.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CalendarColorEntry {
    /// Display name of the calendar (as shown by `rcal calendars`).
    pub name: String,
    /// `#RRGGBB` color for the calendar's event summaries and month markers.
    pub color: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ServerConfig {
    pub url: String,
    pub username: String,
    /// Command to execute that prints password to stdout
    #[serde(default)]
    pub password_command: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DisplayConfig {
    #[serde(default = "default_view")]
    pub default_view: String,
    #[serde(default = "default_time_format")]
    pub time_format: String,
    #[serde(default = "default_color_scheme")]
    pub color_scheme: String,
    /// `#RRGGBB` color used to accent event summaries in the terminal views.
    #[serde(default)]
    pub accent_color: Option<String>,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            default_view: default_view(),
            time_format: default_time_format(),
            color_scheme: default_color_scheme(),
            accent_color: None,
        }
    }
}

fn default_view() -> String {
    "today".to_string()
}

fn default_time_format() -> String {
    "24h".to_string()
}

fn default_color_scheme() -> String {
    "auto".to_string()
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct CalendarsConfig {
    #[serde(default)]
    pub show: Vec<String>,
    #[serde(default)]
    pub hide: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NotificationsConfig {
    #[serde(default = "default_notifications_enabled")]
    pub enabled: bool,
    #[serde(default = "default_reminder_minutes")]
    pub reminder_minutes: Vec<u32>,
}

impl Default for NotificationsConfig {
    fn default() -> Self {
        Self {
            enabled: default_notifications_enabled(),
            reminder_minutes: default_reminder_minutes(),
        }
    }
}

fn default_notifications_enabled() -> bool {
    true
}

fn default_reminder_minutes() -> Vec<u32> {
    vec![15, 5]
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: None,
            display: DisplayConfig::default(),
            calendars: CalendarsConfig::default(),
            notifications: NotificationsConfig::default(),
            subscriptions: Vec::new(),
            calendar_colors: Vec::new(),
        }
    }
}

impl Config {
    /// Build a config from CalDAV server credentials
    pub fn new(
        url: impl Into<String>,
        username: impl Into<String>,
        password_command: Option<String>,
    ) -> Self {
        Self {
            server: Some(ServerConfig {
                url: url.into(),
                username: username.into(),
                password_command,
            }),
            display: DisplayConfig::default(),
            calendars: CalendarsConfig::default(),
            notifications: NotificationsConfig::default(),
            subscriptions: Vec::new(),
            calendar_colors: Vec::new(),
        }
    }

    /// Serialize to TOML and write to the given path, creating parent
    /// directories as needed
    pub fn write_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Failed to create config directory: {}",
                    parent.display()
                )
            })?;
        }
        let content =
            toml::to_string(self).context("Failed to serialize config file")?;
        fs::write(path, content)
            .with_context(|| format!("Failed to write config file: {}", path.display()))?;
        Self::restrict_permissions(path);
        Ok(())
    }

    /// Restrict the config file to owner-only access on Unix. This is applied
    /// on write only; if the user later changes the permissions, rcal leaves
    /// them alone.
    #[cfg(unix)]
    fn restrict_permissions(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }

    #[cfg(not(unix))]
    fn restrict_permissions(_path: &Path) {}

    /// Load configuration from default location
    pub fn load() -> Result<Self> {
        let config_path = Self::config_path()?;

        if !config_path.exists() {
            anyhow::bail!(
                "Config file not found at {}. Run 'rcal add-account' to connect a CalDAV server,\n\
                 or 'rcal subscribe <URL>' to subscribe to an online ICS feed.",
                config_path.display()
            );
        }

        Self::load_from(&config_path)
    }

    /// Load configuration from a specific path
    pub fn load_from(path: &PathBuf) -> Result<Self> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;

        let config: Config =
            toml::from_str(&content).with_context(|| "Failed to parse config file")?;

        Ok(config)
    }

    /// Get the default config file path
    pub fn config_path() -> Result<PathBuf> {
        let config_dir = dirs::config_dir().context("Could not determine config directory")?;

        Ok(config_dir.join("rcal").join("config.toml"))
    }

    /// Get the default data directory
    pub fn data_dir() -> Result<PathBuf> {
        let data_dir = dirs::data_dir().context("Could not determine data directory")?;

        Ok(data_dir.join("rcal"))
    }

    /// Get the database path
    pub fn db_path() -> Result<PathBuf> {
        Ok(Self::data_dir()?.join("rcal.db"))
    }

    /// Add an ICS subscription. Returns `false` (and leaves the config
    /// untouched) if the URL is already subscribed.
    pub fn add_subscription(&mut self, name: String, url: String) -> bool {
        if self.subscriptions.iter().any(|s| s.url == url) {
            return false;
        }
        self.subscriptions.push(IcsSubscription { name, url });
        true
    }

    /// The user-assigned `#RRGGBB` color for the calendar called `name`, from
    /// the `[[calendar_colors]]` sections. `None` when the user has not
    /// configured a color for that calendar name.
    pub fn calendar_color_for(&self, name: &str) -> Option<&str> {
        self.calendar_colors
            .iter()
            .find(|e| e.name == name)
            .map(|e| e.color.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_config_path() -> PathBuf {
        std::env::temp_dir().join(format!("rcal-config-test-{}.toml", uuid::Uuid::new_v4()))
    }

    #[test]
    fn test_write_and_load_roundtrip() {
        let path = temp_config_path();
        let config = Config::new(
            "https://dav.example.com/",
            "alice",
            Some("pass show caldav".to_string()),
        );
        config.write_to(&path).unwrap();

        let loaded = Config::load_from(&path).unwrap();
        let server = loaded.server.as_ref().unwrap();
        assert_eq!(server.url, "https://dav.example.com/");
        assert_eq!(server.username, "alice");
        assert_eq!(server.password_command.as_deref(), Some("pass show caldav"));
        assert_eq!(loaded.display.default_view, "today");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_password_command_is_optional() {
        let path = temp_config_path();
        Config::new("https://dav.example.com/", "alice", None)
            .write_to(&path)
            .unwrap();

        let loaded = Config::load_from(&path).unwrap();
        assert_eq!(loaded.server.as_ref().unwrap().password_command, None);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_custom_default_view_roundtrips() {
        let path = temp_config_path();
        let mut config = Config::new("https://dav.example.com/", "alice", None);
        config.display.default_view = "month".to_string();
        config.write_to(&path).unwrap();

        let loaded = Config::load_from(&path).unwrap();
        assert_eq!(loaded.display.default_view, "month");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_subscriptions_roundtrip() {
        let path = temp_config_path();
        let mut config = Config::new("https://dav.example.com/", "alice", None);
        assert!(config.add_subscription(
            "Holidays".to_string(),
            "https://example.com/holidays.ics".to_string()
        ));
        // Duplicate URL is rejected.
        assert!(!config.add_subscription(
            "Holidays Again".to_string(),
            "https://example.com/holidays.ics".to_string()
        ));
        config.write_to(&path).unwrap();

        let loaded = Config::load_from(&path).unwrap();
        assert_eq!(loaded.subscriptions.len(), 1);
        assert_eq!(loaded.subscriptions[0].name, "Holidays");
        assert_eq!(loaded.subscriptions[0].url, "https://example.com/holidays.ics");
        assert!(loaded
            .subscriptions
            .iter()
            .any(|s| s.url == "https://example.com/holidays.ics"));
        assert!(!loaded
            .subscriptions
            .iter()
            .any(|s| s.url == "https://example.com/other.ics"));

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_subscriptions_default_to_empty() {
        let path = temp_config_path();
        Config::new("https://dav.example.com/", "alice", None)
            .write_to(&path)
            .unwrap();
        let loaded = Config::load_from(&path).unwrap();
        assert!(loaded.subscriptions.is_empty());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_default_config_has_no_server() {
        let config = Config::default();
        assert!(config.server.is_none());

        let content = toml::to_string(&config).unwrap();
        assert!(!content.contains("[server]"), "server section must be omitted");

        let loaded: Config = toml::from_str(&content).unwrap();
        assert!(loaded.server.is_none());
    }

    #[test]
    fn test_serverless_config_roundtrips() {
        let path = temp_config_path();
        let mut config = Config::default();
        config
            .subscriptions
            .push(IcsSubscription {
                name: "Feed".to_string(),
                url: "https://example.com/feed.ics".to_string(),
            });
        config.write_to(&path).unwrap();

        let loaded = Config::load_from(&path).unwrap();
        assert!(loaded.server.is_none());
        assert_eq!(loaded.subscriptions.len(), 1);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_accent_color_roundtrip() {
        let path = temp_config_path();
        let mut config = Config::new("https://dav.example.com/", "alice", None);
        // Default: no accent.
        assert_eq!(config.display.accent_color, None);
        config.display.accent_color = Some("#3b82f6".to_string());
        config.write_to(&path).unwrap();

        let loaded = Config::load_from(&path).unwrap();
        assert_eq!(loaded.display.accent_color.as_deref(), Some("#3b82f6"));

        // A config written before accent_color existed (no `accent_color` key)
        // still parses with None.
        let content = r#"
[server]
url = "https://dav.example.com/"
username = "alice"

[display]
default_view = "today"
time_format = "24h"
color_scheme = "auto"
"#;
        let legacy: Config = toml::from_str(content).unwrap();
        assert_eq!(legacy.display.accent_color, None);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_calendar_colors_roundtrip() {
        let path = temp_config_path();
        let mut config = Config::new("https://dav.example.com/", "alice", None);
        config
            .calendar_colors
            .push(CalendarColorEntry {
                name: "Work".to_string(),
                color: "#ff0000".to_string(),
            });
        config
            .calendar_colors
            .push(CalendarColorEntry {
                name: "Holidays".to_string(),
                color: "#00ff00".to_string(),
            });
        config.write_to(&path).unwrap();

        let loaded = Config::load_from(&path).unwrap();
        assert_eq!(loaded.calendar_colors.len(), 2);
        assert_eq!(loaded.calendar_color_for("Work"), Some("#ff0000"));
        assert_eq!(loaded.calendar_color_for("Holidays"), Some("#00ff00"));
        // Exact name match only.
        assert_eq!(loaded.calendar_color_for("work"), None);
        assert_eq!(loaded.calendar_color_for("Other"), None);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_calendar_colors_default_to_empty() {
        let path = temp_config_path();
        Config::new("https://dav.example.com/", "alice", None)
            .write_to(&path)
            .unwrap();
        let loaded = Config::load_from(&path).unwrap();
        assert!(loaded.calendar_colors.is_empty());
        assert_eq!(loaded.calendar_color_for("Work"), None);

        // A config written before calendar_colors existed (a plain `[server]`
        // block) still parses with an empty list.
        let content = r#"
[server]
url = "https://dav.example.com/"
username = "alice"
password_command = "echo secret"
"#;
        let legacy: Config = toml::from_str(content).unwrap();
        assert!(legacy.calendar_colors.is_empty());
        std::fs::remove_file(&path).ok();
    }

    #[cfg(unix)]
    #[test]
    fn test_config_written_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let path = temp_config_path();
        Config::new("https://dav.example.com/", "alice", None)
            .write_to(&path)
            .unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);

        std::fs::remove_file(&path).ok();
    }
}

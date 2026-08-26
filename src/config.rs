use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    #[serde(default)]
    pub display: DisplayConfig,
    #[serde(default)]
    pub calendars: CalendarsConfig,
    #[serde(default)]
    pub notifications: NotificationsConfig,
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
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            default_view: default_view(),
            time_format: default_time_format(),
            color_scheme: default_color_scheme(),
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

impl Config {
    /// Load configuration from default location
    pub fn load() -> Result<Self> {
        let config_path = Self::config_path()?;

        if !config_path.exists() {
            anyhow::bail!(
                "Config file not found at {}. Run 'rcal init' to create one.",
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
}

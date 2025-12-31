use std::error::Error;

use serde::{Deserialize, Serialize};

#[cfg(feature = "tracing")]
use crate::config::LoggingConfig;
use crate::config::{ConnectionConfig, TransportConfig};

/// JSON configuration file structure
#[derive(Deserialize, Serialize, Debug, Default)]
#[serde(rename_all = "snake_case")]
pub struct JsonConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub listen: Option<std::net::SocketAddr>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport: Option<TransportConfig>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection: Option<ConnectionConfig>,

    #[cfg(feature = "tracing")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logging: Option<LoggingConfig>,
}

impl JsonConfig {
    /// Load configuration from a JSON string
    ///
    /// # Arguments
    ///
    /// * `json_str` - A JSON string containing the configuration
    ///
    /// # Returns
    ///
    /// A `JsonConfig` instance, or an error if parsing fails
    pub fn from_str(json_str: &str) -> Result<Self, Box<dyn Error>> {
        let config: JsonConfig = serde_json::from_str(json_str)?;
        Ok(config)
    }

    /// Load configuration from a JSON file
    ///
    /// # Arguments
    ///
    /// * `config_path` - Path to the JSON configuration file
    ///
    /// # Returns
    ///
    /// A `JsonConfig` instance, or an error if the file doesn't exist or parsing fails
    pub fn from_file(config_path: &std::path::Path) -> Result<Self, Box<dyn Error>> {
        if !config_path.exists() {
            return Err(format!("Configuration file not found: {}", config_path.display()).into());
        }

        let content = std::fs::read_to_string(config_path)?;
        Self::from_str(&content)
    }
}

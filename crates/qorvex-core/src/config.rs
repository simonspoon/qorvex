//! Persistent configuration for qorvex.
//!
//! Stores user settings in `~/.qorvex/config.json`. The primary use case is
//! recording the path to the Swift agent source directory so that the agent can
//! be automatically built and launched when a session starts.
//!
//! # Example
//!
//! ```no_run
//! use qorvex_core::config::QorvexConfig;
//!
//! // Load (returns defaults if file doesn't exist)
//! let config = QorvexConfig::load();
//!
//! // Check agent source dir
//! if let Some(dir) = &config.agent_source_dir {
//!     println!("Agent source: {}", dir.display());
//! }
//! ```

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::ipc::qorvex_dir;

const CONFIG_FILENAME: &str = "config.json";

/// Persistent qorvex configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QorvexConfig {
    /// Path to the Swift agent project directory (containing `project.yml`).
    /// Recorded by `install.sh` so that sessions can auto-build the agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_source_dir: Option<PathBuf>,
}

impl QorvexConfig {
    /// Load config from `~/.qorvex/config.json`.
    ///
    /// Returns [`Default`] if the file does not exist or cannot be parsed.
    pub fn load() -> Self {
        let path = qorvex_dir().join(CONFIG_FILENAME);
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Save config to `~/.qorvex/config.json`.
    pub fn save(&self) -> std::io::Result<()> {
        let path = qorvex_dir().join(CONFIG_FILENAME);
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(path, json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_no_agent_dir() {
        let config = QorvexConfig::default();
        assert!(config.agent_source_dir.is_none());
    }

    #[test]
    fn roundtrip_serialization() {
        let config = QorvexConfig {
            agent_source_dir: Some(PathBuf::from("/Users/test/qorvex-agent")),
        };
        let json = serde_json::to_string(&config).unwrap();
        let loaded: QorvexConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.agent_source_dir, config.agent_source_dir);
    }

    #[test]
    fn deserialize_empty_json() {
        let loaded: QorvexConfig = serde_json::from_str("{}").unwrap();
        assert!(loaded.agent_source_dir.is_none());
    }

    #[test]
    fn load_returns_default_for_missing_file() {
        // QorvexConfig::load() should not panic even if file doesn't exist
        let config = QorvexConfig::load();
        // We can't assert much since the real config file might exist,
        // but at minimum it should not panic.
        let _ = config;
    }
}

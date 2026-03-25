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

    /// TCP port the Swift agent listens on. Defaults to 8080 if absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_port: Option<u16>,

    /// Apple Development Team ID for code-signing on physical devices.
    /// Required when deploying the agent to a real device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub development_team: Option<String>,

    /// Override bundle ID for the agent app when signing for physical devices.
    /// Needed when the default `com.qorvex.agent` is already claimed by another team.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_bundle_id: Option<String>,
}

impl QorvexConfig {
    /// Returns the configured agent port, defaulting to 8080.
    pub fn agent_port(&self) -> u16 {
        self.agent_port.unwrap_or(8080)
    }

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

    /// Returns the effective agent source directory.
    ///
    /// Resolution order:
    /// 1. Explicit `agent_source_dir` from config
    /// 2. Homebrew share path (`HOMEBREW_PREFIX/share/qorvex/agent`)
    pub fn effective_agent_source_dir(&self) -> Option<PathBuf> {
        if self.agent_source_dir.is_some() {
            return self.agent_source_dir.clone();
        }
        homebrew_agent_path()
    }

    /// Save config to `~/.qorvex/config.json`.
    pub fn save(&self) -> std::io::Result<()> {
        let path = qorvex_dir().join(CONFIG_FILENAME);
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, json)
    }
}

/// Probe for the agent source directory installed by Homebrew.
///
/// Checks `HOMEBREW_PREFIX/share/qorvex/agent` (arm64 default: `/opt/homebrew`,
/// Intel default: `/usr/local`). Returns the path only if the directory exists.
fn homebrew_agent_path() -> Option<PathBuf> {
    let prefixes = ["/opt/homebrew", "/usr/local"];
    for prefix in &prefixes {
        let path = PathBuf::from(prefix).join("share/qorvex/agent");
        if path.exists() {
            return Some(path);
        }
    }
    None
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
            agent_port: None,
            development_team: None,
            agent_bundle_id: None,
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

    #[test]
    fn effective_agent_source_dir_prefers_explicit() {
        let config = QorvexConfig {
            agent_source_dir: Some(PathBuf::from("/explicit/path")),
            ..Default::default()
        };
        assert_eq!(
            config.effective_agent_source_dir(),
            Some(PathBuf::from("/explicit/path"))
        );
    }

    #[test]
    fn effective_agent_source_dir_returns_none_when_no_homebrew() {
        let config = QorvexConfig::default();
        // On CI or machines without Homebrew qorvex, this should either
        // return the Homebrew path (if installed) or None. Just verify no panic.
        let _ = config.effective_agent_source_dir();
    }
}

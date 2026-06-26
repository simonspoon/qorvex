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

    /// Path to the Android Kotlin agent project directory (containing `gradlew`).
    /// Required to build/install/launch the Android agent (`start-agent`
    /// targeting Android). The Android analog of `agent_source_dir`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub android_agent_source_dir: Option<PathBuf>,

    /// Path to the Android SDK root (`ANDROID_HOME` / `ANDROID_SDK_ROOT`).
    /// Used to locate `adb`/`emulator` when not on `PATH`. Optional: if absent
    /// and the tools are on `PATH`, Android commands still work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub android_sdk_root: Option<PathBuf>,

    /// TCP port the Android (Kotlin) agent serves on inside the device.
    /// Defaults to 8080 if absent (matches `DEFAULT_ANDROID_AGENT_PORT`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub android_device_port: Option<u16>,
}

/// Errors returned when validating Android-related configuration.
///
/// These surface at the point an Android operation is requested so a missing or
/// invalid Android config yields a clear, actionable message rather than a
/// downstream crash (story #89 / spec F3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AndroidConfigError {
    /// No Android agent project directory is configured.
    MissingAgentSourceDir,
    /// The configured Android agent project directory does not exist.
    AgentSourceDirNotFound(PathBuf),
    /// The configured Android agent project directory has no `gradlew` wrapper.
    GradlewNotFound(PathBuf),
    /// The configured Android SDK root does not exist.
    SdkRootNotFound(PathBuf),
}

impl std::fmt::Display for AndroidConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AndroidConfigError::MissingAgentSourceDir => write!(
                f,
                "Android agent project directory is not configured. Set \
                 `android_agent_source_dir` in ~/.qorvex/config.json (the path to \
                 the Kotlin agent project containing `gradlew`)."
            ),
            AndroidConfigError::AgentSourceDirNotFound(p) => write!(
                f,
                "Configured Android agent project directory does not exist: {}",
                p.display()
            ),
            AndroidConfigError::GradlewNotFound(p) => write!(
                f,
                "Android agent project directory is missing its Gradle wrapper \
                 (expected `gradlew` at {})",
                p.display()
            ),
            AndroidConfigError::SdkRootNotFound(p) => write!(
                f,
                "Configured Android SDK root does not exist: {}",
                p.display()
            ),
        }
    }
}

impl std::error::Error for AndroidConfigError {}

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

    /// Returns the configured Android agent device port, defaulting to 8080.
    pub fn android_device_port(&self) -> u16 {
        self.android_device_port.unwrap_or(8080)
    }

    /// Returns the effective Android agent project directory (no fallback —
    /// Android has no Homebrew-installed default, so this is the configured
    /// value verbatim).
    pub fn effective_android_agent_source_dir(&self) -> Option<PathBuf> {
        self.android_agent_source_dir.clone()
    }

    /// Validate the Android configuration required to build/launch the agent.
    ///
    /// Returns `Ok(project_dir)` with the resolved agent project directory when
    /// the config is complete and the paths exist; otherwise an
    /// [`AndroidConfigError`] describing exactly what is missing or invalid.
    ///
    /// This is the fail-fast guard for `start-agent` against an Android target
    /// (spec F3): a missing/invalid Android config yields a clear validation
    /// error here rather than a downstream Gradle/adb crash. The SDK root is
    /// validated only if configured (the tools may be on `PATH`).
    pub fn validate_android(&self) -> Result<PathBuf, AndroidConfigError> {
        if let Some(ref sdk) = self.android_sdk_root {
            if !sdk.exists() {
                return Err(AndroidConfigError::SdkRootNotFound(sdk.clone()));
            }
        }

        let project_dir = self
            .effective_android_agent_source_dir()
            .ok_or(AndroidConfigError::MissingAgentSourceDir)?;
        if !project_dir.exists() {
            return Err(AndroidConfigError::AgentSourceDirNotFound(project_dir));
        }
        if !project_dir.join("gradlew").exists() {
            return Err(AndroidConfigError::GradlewNotFound(project_dir));
        }
        Ok(project_dir)
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
            ..Default::default()
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

    // --- Android config (additive) ---

    #[test]
    fn legacy_config_json_still_loads_with_android_fields_defaulting_to_none() {
        // An old config.json (no Android keys) must still deserialize, with the
        // new Android fields defaulting to None — additive, iOS path unchanged.
        let legacy = r#"{"agent_source_dir":"/Users/test/qorvex-agent","agent_port":8080}"#;
        let loaded: QorvexConfig = serde_json::from_str(legacy).unwrap();
        assert_eq!(
            loaded.agent_source_dir,
            Some(PathBuf::from("/Users/test/qorvex-agent"))
        );
        assert!(loaded.android_agent_source_dir.is_none());
        assert!(loaded.android_sdk_root.is_none());
        assert!(loaded.android_device_port.is_none());
        // Default device port falls back to 8080.
        assert_eq!(loaded.android_device_port(), 8080);
    }

    #[test]
    fn android_device_port_defaults_to_8080() {
        let config = QorvexConfig::default();
        assert_eq!(config.android_device_port(), 8080);
        let config = QorvexConfig {
            android_device_port: Some(9999),
            ..Default::default()
        };
        assert_eq!(config.android_device_port(), 9999);
    }

    #[test]
    fn validate_android_missing_source_dir_is_clear_error() {
        let config = QorvexConfig::default();
        let err = config.validate_android().unwrap_err();
        assert_eq!(err, AndroidConfigError::MissingAgentSourceDir);
        // The message is actionable (names the config key to set).
        assert!(err.to_string().contains("android_agent_source_dir"));
    }

    #[test]
    fn validate_android_nonexistent_source_dir_is_clear_error() {
        let config = QorvexConfig {
            android_agent_source_dir: Some(PathBuf::from("/no/such/qorvex-agent-android")),
            ..Default::default()
        };
        let err = config.validate_android().unwrap_err();
        assert!(matches!(err, AndroidConfigError::AgentSourceDirNotFound(_)));
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn validate_android_missing_gradlew_is_clear_error() {
        // A real directory that has no `gradlew` — use the OS temp dir.
        let tmp = std::env::temp_dir();
        let config = QorvexConfig {
            android_agent_source_dir: Some(tmp.clone()),
            ..Default::default()
        };
        // Only assert the gradlew-missing path if the temp dir genuinely lacks
        // a gradlew (it always does); guard against the absurd case.
        if !tmp.join("gradlew").exists() {
            let err = config.validate_android().unwrap_err();
            assert!(matches!(err, AndroidConfigError::GradlewNotFound(_)));
            assert!(err.to_string().contains("gradlew"));
        }
    }

    #[test]
    fn validate_android_nonexistent_sdk_root_is_clear_error() {
        let config = QorvexConfig {
            android_sdk_root: Some(PathBuf::from("/no/such/android-sdk")),
            android_agent_source_dir: Some(PathBuf::from("/no/such/agent")),
            ..Default::default()
        };
        // SDK root is checked first, so this surfaces SdkRootNotFound.
        let err = config.validate_android().unwrap_err();
        assert!(matches!(err, AndroidConfigError::SdkRootNotFound(_)));
    }

    #[test]
    fn validate_android_ok_when_project_and_gradlew_exist() {
        // Build a temp project dir with a gradlew stub.
        let base =
            std::env::temp_dir().join(format!("qorvex-android-cfgtest-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&base);
        std::fs::write(base.join("gradlew"), b"#!/bin/sh\n").unwrap();
        let config = QorvexConfig {
            android_agent_source_dir: Some(base.clone()),
            ..Default::default()
        };
        let resolved = config.validate_android().unwrap();
        assert_eq!(resolved, base);
        let _ = std::fs::remove_dir_all(&base);
    }
}

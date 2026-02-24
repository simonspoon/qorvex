//! Screen change detection for iOS Simulator automation.
//!
//! This module provides automatic screen change detection by polling the
//! accessibility tree and comparing hashes to detect UI changes.
//!
//! # Example
//!
//! ```no_run
//! use std::sync::Arc;
//! use qorvex_core::agent_driver::AgentDriver;
//! use qorvex_core::driver::AutomationDriver;
//! use qorvex_core::session::Session;
//! use qorvex_core::watcher::{ScreenWatcher, WatcherConfig};
//!
//! #[tokio::main]
//! async fn main() {
//!     let session = Session::new(Some("SIMULATOR-UDID".to_string()), "default");
//!     let driver: Arc<dyn AutomationDriver> = Arc::new(AgentDriver::direct("localhost", 8080));
//!     let config = WatcherConfig::default();
//!
//!     let handle = ScreenWatcher::spawn(
//!         session,
//!         driver,
//!         config,
//!     );
//!
//!     // Later, stop the watcher
//!     handle.stop().await;
//! }
//! ```

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Instant;

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use tracing::{debug, debug_span, Instrument};

use crate::element::UIElement;
use crate::driver::AutomationDriver;
use crate::session::Session;

/// Configuration for the screen watcher.
#[derive(Debug, Clone)]
pub struct WatcherConfig {
    /// Polling interval in milliseconds (default: 1000).
    pub interval_ms: u64,
}

impl Default for WatcherConfig {
    fn default() -> Self {
        Self {
            interval_ms: 1000,
        }
    }
}

/// Handle for controlling a running screen watcher.
///
/// The watcher will continue running until `stop()` is called or the
/// handle is dropped.
pub struct WatcherHandle {
    cancel_token: CancellationToken,
    join_handle: JoinHandle<()>,
}

impl WatcherHandle {
    /// Stops the watcher and waits for it to finish.
    pub async fn stop(self) {
        self.cancel_token.cancel();
        let _ = self.join_handle.await;
    }

    /// Cancels the watcher without waiting.
    pub fn cancel(&self) {
        self.cancel_token.cancel();
    }

    /// Returns whether the watcher is still running.
    pub fn is_running(&self) -> bool {
        !self.join_handle.is_finished()
    }
}

/// Screen watcher that detects UI changes by polling the accessibility tree.
pub struct ScreenWatcher;

impl ScreenWatcher {
    /// Spawns a new screen watcher task.
    ///
    /// The watcher will poll the accessibility tree at the configured interval
    /// and broadcast `ScreenInfoUpdated` events when changes are detected.
    ///
    /// # Arguments
    ///
    /// * `session` - The session to broadcast events to
    /// * `driver` - The automation driver to use for polling
    /// * `config` - Watcher configuration
    ///
    /// # Returns
    ///
    /// A `WatcherHandle` that can be used to stop the watcher.
    pub fn spawn(
        session: Arc<Session>,
        driver: Arc<dyn AutomationDriver>,
        config: WatcherConfig,
    ) -> WatcherHandle {
        let cancel_token = CancellationToken::new();
        let token_clone = cancel_token.clone();

        let join_handle = tokio::spawn(async move {
            Self::run_loop(session, driver, config, token_clone).await;
        });

        WatcherHandle {
            cancel_token,
            join_handle,
        }
    }

    async fn run_loop(
        session: Arc<Session>,
        driver: Arc<dyn AutomationDriver>,
        config: WatcherConfig,
        cancel_token: CancellationToken,
    ) {
        let base_interval = tokio::time::Duration::from_millis(config.interval_ms);
        let mut consecutive_errors: u32 = 0;
        let mut adaptive_interval = base_interval;
        const MAX_ADAPTIVE: tokio::time::Duration = tokio::time::Duration::from_secs(30);

        loop {
            let sleep_duration = Self::backoff_interval(adaptive_interval, consecutive_errors);

            tokio::select! {
                _ = cancel_token.cancelled() => {
                    break;
                }
                _ = tokio::time::sleep(sleep_duration) => {
                    let poll_start = Instant::now();
                    let ok = {
                        let span = debug_span!("watcher_poll");
                        Self::check_for_changes(&session, &driver).instrument(span).await
                    };
                    let elapsed = poll_start.elapsed();

                    // Adapt interval to avoid hammering a slow agent on large pages.
                    if elapsed > base_interval {
                        let new_interval = std::cmp::min(elapsed * 2, MAX_ADAPTIVE);
                        if new_interval != adaptive_interval {
                            debug!(?elapsed, ?new_interval, "watcher adapting interval for slow poll");
                        }
                        adaptive_interval = new_interval;
                    } else {
                        adaptive_interval = base_interval;
                    }

                    let prev_errors = consecutive_errors;
                    if ok {
                        consecutive_errors = 0;
                    } else {
                        consecutive_errors = consecutive_errors.saturating_add(1);
                    }
                    if prev_errors == 0 && consecutive_errors > 0 {
                        debug!(consecutive_errors, "watcher entering backoff");
                    } else if prev_errors > 0 && consecutive_errors == 0 {
                        debug!("watcher recovered from backoff");
                    }
                }
            }
        }
    }

    /// Returns `true` if the poll cycle completed successfully, `false` on error.
    async fn check_for_changes(
        session: &Arc<Session>,
        driver: &Arc<dyn AutomationDriver>,
    ) -> bool {
        let hierarchy = match driver.dump_tree().await {
            Ok(h) => h,
            Err(_) => return false,
        };

        let elements = crate::driver::flatten_elements(&hierarchy);

        // Compute hash of elements
        let element_hash = Self::hash_elements(&elements);

        // Update session (this handles hash comparison internally)
        session
            .update_screen_info(elements, element_hash)
            .await;

        true
    }

    /// Computes the backoff interval given the base interval and consecutive error count.
    /// Doubles with each error, capped at 30 seconds.
    fn backoff_interval(base: std::time::Duration, consecutive_errors: u32) -> std::time::Duration {
        const MAX_BACKOFF: std::time::Duration = std::time::Duration::from_secs(30);
        let multiplier = 1u64.checked_shl(consecutive_errors).unwrap_or(u64::MAX);
        let backoff = base.saturating_mul(multiplier as u32);
        std::cmp::min(backoff, MAX_BACKOFF)
    }

    /// Computes a hash of the element list for change detection.
    fn hash_elements(elements: &[UIElement]) -> u64 {
        let mut hasher = DefaultHasher::new();

        for element in elements {
            element.identifier.hash(&mut hasher);
            element.label.hash(&mut hasher);
            element.value.hash(&mut hasher);
            element.element_type.hash(&mut hasher);

            // Hash frame if present
            if let Some(ref frame) = element.frame {
                (frame.x as i64).hash(&mut hasher);
                (frame.y as i64).hash(&mut hasher);
                (frame.width as i64).hash(&mut hasher);
                (frame.height as i64).hash(&mut hasher);
            }
        }

        hasher.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_watcher_config_default() {
        let config = WatcherConfig::default();
        assert_eq!(config.interval_ms, 1000);
    }

    #[test]
    fn test_hash_elements_empty() {
        let elements: Vec<UIElement> = vec![];
        let hash = ScreenWatcher::hash_elements(&elements);
        // Empty vec should produce a consistent hash
        assert_eq!(hash, ScreenWatcher::hash_elements(&[]));
    }

    #[test]
    fn test_hash_elements_deterministic() {
        let element = UIElement {
            identifier: Some("test-id".to_string()),
            label: Some("Test Label".to_string()),
            value: Some("Test Value".to_string()),
            element_type: Some("Button".to_string()),
            frame: None,
            children: vec![],
            role: None,
            hittable: None,
        };

        let hash1 = ScreenWatcher::hash_elements(&[element.clone()]);
        let hash2 = ScreenWatcher::hash_elements(&[element]);

        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_hash_elements_different() {
        let element1 = UIElement {
            identifier: Some("id1".to_string()),
            label: None,
            value: None,
            element_type: None,
            frame: None,
            children: vec![],
            role: None,
            hittable: None,
        };

        let element2 = UIElement {
            identifier: Some("id2".to_string()),
            label: None,
            value: None,
            element_type: None,
            frame: None,
            children: vec![],
            role: None,
            hittable: None,
        };

        let hash1 = ScreenWatcher::hash_elements(&[element1]);
        let hash2 = ScreenWatcher::hash_elements(&[element2]);

        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_backoff_interval_no_errors() {
        let base = std::time::Duration::from_millis(500);
        let result = ScreenWatcher::backoff_interval(base, 0);
        assert_eq!(result, base);
    }

    #[test]
    fn test_backoff_interval_doubles() {
        let base = std::time::Duration::from_millis(500);
        assert_eq!(ScreenWatcher::backoff_interval(base, 1), std::time::Duration::from_millis(1000));
        assert_eq!(ScreenWatcher::backoff_interval(base, 2), std::time::Duration::from_millis(2000));
        assert_eq!(ScreenWatcher::backoff_interval(base, 3), std::time::Duration::from_millis(4000));
    }

    #[test]
    fn test_backoff_interval_caps_at_30s() {
        let base = std::time::Duration::from_millis(500);
        let result = ScreenWatcher::backoff_interval(base, 10);
        assert_eq!(result, std::time::Duration::from_secs(30));
    }

    #[test]
    fn test_backoff_interval_overflow_safe() {
        let base = std::time::Duration::from_millis(500);
        // Very large error count should not panic, should cap at 30s
        let result = ScreenWatcher::backoff_interval(base, u32::MAX);
        assert_eq!(result, std::time::Duration::from_secs(30));
    }
}

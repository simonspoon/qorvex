//! Session state management for iOS Simulator automation.
//!
//! This module provides the [`Session`] type, which tracks the state of an
//! automation session including action history, screenshots, and event
//! broadcasting for connected watchers.
//!
//! # Architecture
//!
//! A session acts as the central state manager for a REPL instance:
//!
//! - Actions performed in the REPL are logged to the session
//! - Screenshots are stored and broadcasted when updated
//! - Watchers subscribe to session events via broadcast channels
//! - The action log is maintained as a ring buffer to limit memory usage
//! - Actions are persisted to JSON Lines files in `~/.qorvex/logs/`
//!
//! # Example
//!
//! ```no_run
//! use qorvex_core::session::Session;
//! use qorvex_core::action::{ActionType, ActionResult};
//!
//! #[tokio::main]
//! async fn main() {
//!     // Create a new session
//!     let session = Session::new(Some("SIMULATOR-UDID".to_string()), "default");
//!
//!     // Subscribe to events (for a watcher)
//!     let mut rx = session.subscribe();
//!
//!     // Log an action
//!     session.log_action(
//!         ActionType::Tap {
//!             selector: "button".to_string(),
//!             by_label: false,
//!             element_type: None,
//!         },
//!         ActionResult::Success,
//!         None
//!     ).await;
//! }
//! ```

use std::collections::VecDeque;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex, RwLock};
use uuid::Uuid;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::action::{ActionLog, ActionType, ActionResult};
use crate::axe::UIElement;
use crate::ipc::qorvex_dir;

/// Maximum number of action log entries to retain in the ring buffer.
const MAX_ACTION_LOG_SIZE: usize = 1000;

/// Returns the logs directory path (`~/.qorvex/logs/`).
///
/// Creates the directory if it doesn't exist.
fn logs_dir() -> PathBuf {
    let dir = qorvex_dir().join("logs");
    std::fs::create_dir_all(&dir).ok();
    dir
}

/// Events broadcast to watchers when session state changes.
///
/// These events are sent through the session's broadcast channel to notify
/// connected watchers (such as the TUI) of state changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionEvent {
    /// A new action was logged to the session.
    ActionLogged(ActionLog),

    /// The screenshot was updated.
    ///
    /// Contains the base64-encoded PNG image data wrapped in an `Arc`
    /// for efficient cloning during broadcast.
    ScreenshotUpdated(Arc<String>),

    /// The screen info was updated (from screen watcher).
    ///
    /// Contains the current UI elements and optionally a screenshot.
    ScreenInfoUpdated {
        /// The current UI elements on screen.
        elements: Arc<Vec<UIElement>>,
        /// Optional base64-encoded PNG screenshot.
        screenshot: Option<Arc<String>>,
    },

    /// The session has started.
    Started {
        /// The unique identifier for this session.
        session_id: Uuid,
    },

    /// The session has ended.
    Ended,
}

/// Shared session state for an automation session.
///
/// The session maintains:
/// - A unique identifier and creation timestamp
/// - The target simulator's UDID (if connected)
/// - A ring buffer of recent actions (up to 1000 entries)
/// - The current screenshot (if any)
/// - A broadcast channel for notifying watchers of state changes
/// - A persistent log file in `~/.qorvex/logs/`
/// - Screen hash for change detection (used by watcher)
///
/// Sessions are created via [`Session::new`], which returns an `Arc<Session>`
/// for safe sharing across async tasks.
pub struct Session {
    /// The unique identifier for this session.
    pub id: Uuid,

    /// When this session was created.
    pub created_at: DateTime<Utc>,

    /// The UDID of the connected simulator, if any.
    pub simulator_udid: Option<String>,

    /// Ring buffer of action log entries (private, access via methods).
    action_log: RwLock<VecDeque<ActionLog>>,

    /// The current screenshot as base64-encoded PNG (private, access via methods).
    current_screenshot: RwLock<Option<Arc<String>>>,

    /// Broadcast channel for session events.
    event_tx: broadcast::Sender<SessionEvent>,

    /// Buffered writer for persistent JSON Lines log file.
    log_writer: Mutex<Option<BufWriter<std::fs::File>>>,

    /// Hash of the current screen elements for change detection.
    screen_hash: RwLock<u64>,

    /// Current UI elements on screen (cached from last screen info update).
    current_elements: RwLock<Option<Arc<Vec<UIElement>>>>,

    /// Perceptual hash of the current screenshot for visual change detection.
    screenshot_hash: RwLock<u64>,
}

impl Session {
    /// Creates a new session.
    ///
    /// # Arguments
    ///
    /// * `simulator_udid` - Optional UDID of the simulator to associate with this session
    /// * `session_name` - Name used for the persistent log file
    ///
    /// # Returns
    ///
    /// An `Arc<Session>` for safe sharing across async tasks. The session is
    /// initialized with a new UUID, the current timestamp, an empty action log,
    /// no screenshot, and a persistent log file at `~/.qorvex/logs/{session_name}_{timestamp}.jsonl`.
    pub fn new(simulator_udid: Option<String>, session_name: &str) -> Arc<Self> {
        let (event_tx, _) = broadcast::channel(100);
        let created_at = Utc::now();

        // Create persistent log file
        let log_writer = {
            let timestamp = created_at.format("%Y%m%d_%H%M%S");
            let log_path = logs_dir().join(format!("{}_{}.jsonl", session_name, timestamp));
            std::fs::File::create(&log_path)
                .ok()
                .map(BufWriter::new)
        };

        Arc::new(Self {
            id: Uuid::new_v4(),
            created_at,
            simulator_udid,
            action_log: RwLock::new(VecDeque::with_capacity(MAX_ACTION_LOG_SIZE)),
            current_screenshot: RwLock::new(None),
            event_tx,
            log_writer: Mutex::new(log_writer),
            screen_hash: RwLock::new(0),
            current_elements: RwLock::new(None),
            screenshot_hash: RwLock::new(0),
        })
    }

    /// Subscribes to session events.
    ///
    /// Returns a broadcast receiver that will receive [`SessionEvent`]s as they
    /// occur. This is typically used by watchers (like the TUI) to stay updated
    /// on session state changes.
    ///
    /// # Returns
    ///
    /// A `broadcast::Receiver<SessionEvent>` that can be used to receive events.
    /// Note that broadcast receivers may miss events if they lag too far behind.
    pub fn subscribe(&self) -> broadcast::Receiver<SessionEvent> {
        self.event_tx.subscribe()
    }

    /// Logs an action and broadcasts it to watchers.
    ///
    /// Creates an [`ActionLog`] entry with the given action, result, and optional
    /// screenshot, then adds it to the session's action log, writes it to the
    /// persistent log file, and broadcasts it to all subscribers.
    ///
    /// If a screenshot is provided, the current screenshot is also updated and
    /// a [`SessionEvent::ScreenshotUpdated`] event is broadcast.
    ///
    /// # Arguments
    ///
    /// * `action` - The type of action that was performed
    /// * `result` - The result of the action (success or failure)
    /// * `screenshot` - Optional base64-encoded PNG screenshot taken after the action
    ///
    /// # Returns
    ///
    /// The created [`ActionLog`] entry.
    ///
    /// # Note
    ///
    /// The action log is maintained as a ring buffer. When the maximum size
    /// is reached, the oldest entry is removed. Actions are also persisted to
    /// the JSON Lines log file at `~/.qorvex/logs/`.
    pub async fn log_action(&self, action: ActionType, result: ActionResult, screenshot: Option<String>) -> ActionLog {
        // Wrap screenshot in Arc for cheap clones in hot path
        let screenshot_arc = screenshot.map(Arc::new);
        let log = ActionLog::new(action, result, screenshot_arc.clone());

        // Update action log with ring buffer behavior
        {
            let mut action_log = self.action_log.write().await;
            if action_log.len() >= MAX_ACTION_LOG_SIZE {
                action_log.pop_front(); // Remove oldest entry
            }
            action_log.push_back(log.clone());
        }

        // Write to persistent log file (without screenshot to keep file size manageable)
        {
            let mut writer_guard = self.log_writer.lock().await;
            if let Some(ref mut writer) = *writer_guard {
                // Create a copy without screenshot for file logging
                let file_log = ActionLog {
                    id: log.id,
                    timestamp: log.timestamp,
                    action: log.action.clone(),
                    result: log.result.clone(),
                    screenshot: None,
                };
                if let Ok(json) = serde_json::to_string(&file_log) {
                    let _ = writeln!(writer, "{}", json);
                    let _ = writer.flush();
                }
            }
        }

        // Update screenshot if provided
        if let Some(ref ss) = screenshot_arc {
            *self.current_screenshot.write().await = Some(ss.clone());
            // Ignore send errors - no subscribers is expected
            let _ = self.event_tx.send(SessionEvent::ScreenshotUpdated(ss.clone()));
        }

        // Broadcast action (ignore if no subscribers)
        let _ = self.event_tx.send(SessionEvent::ActionLogged(log.clone()));

        log
    }

    /// Returns all action log entries.
    ///
    /// # Returns
    ///
    /// A `Vec<ActionLog>` containing all logged actions in chronological order.
    /// This is a copy of the internal log, so modifications do not affect the session.
    pub async fn get_action_log(&self) -> Vec<ActionLog> {
        self.action_log.read().await.iter().cloned().collect()
    }

    /// Returns the current screenshot, if any.
    ///
    /// # Returns
    ///
    /// `Some(Arc<String>)` containing the base64-encoded PNG screenshot,
    /// or `None` if no screenshot has been captured yet.
    pub async fn get_screenshot(&self) -> Option<Arc<String>> {
        self.current_screenshot.read().await.clone()
    }

}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("id", &self.id)
            .field("created_at", &self.created_at)
            .field("simulator_udid", &self.simulator_udid)
            .field("action_log", &"<RwLock<VecDeque<ActionLog>>>")
            .field("current_screenshot", &"<RwLock<Option<Arc<String>>>>")
            .field("event_tx", &"<broadcast::Sender>")
            .field("log_writer", &"<Mutex<Option<BufWriter<File>>>>")
            .finish()
    }
}

impl Session {
    /// Updates the current screenshot without logging an action.
    ///
    /// This is useful for updating the screenshot independently of action
    /// logging, such as during periodic refresh or on demand.
    ///
    /// # Arguments
    ///
    /// * `screenshot` - Base64-encoded PNG screenshot data
    ///
    /// # Events
    ///
    /// Broadcasts a [`SessionEvent::ScreenshotUpdated`] event to all subscribers.
    pub async fn update_screenshot(&self, screenshot: String) {
        let screenshot_arc = Arc::new(screenshot);
        *self.current_screenshot.write().await = Some(screenshot_arc.clone());
        // Ignore send errors - no subscribers is expected
        let _ = self.event_tx.send(SessionEvent::ScreenshotUpdated(screenshot_arc));
    }

    /// Updates the screen info if either the element hash or visual hash has changed.
    ///
    /// This method is called by the screen watcher to update the cached
    /// screen elements and broadcast changes to subscribers. It detects changes
    /// via two mechanisms:
    /// 1. Element hash - detects accessibility tree changes
    /// 2. Screenshot hash - detects visual changes (animations, scroll position)
    ///
    /// # Arguments
    ///
    /// * `elements` - The current UI elements on screen
    /// * `element_hash` - The computed hash of the elements
    /// * `screenshot` - Optional base64-encoded PNG screenshot
    /// * `screenshot_hash` - Optional perceptual hash (dHash) of the screenshot
    /// * `visual_threshold` - Hamming distance threshold for visual change detection (0-64)
    ///
    /// # Returns
    ///
    /// `true` if the screen info changed (either hash was different), `false` otherwise.
    ///
    /// # Events
    ///
    /// If either hash changed significantly, broadcasts a [`SessionEvent::ScreenInfoUpdated`] event.
    /// If a screenshot is provided and changed, also broadcasts [`SessionEvent::ScreenshotUpdated`].
    pub async fn update_screen_info(
        &self,
        elements: Vec<UIElement>,
        element_hash: u64,
        screenshot: Option<String>,
        screenshot_hash: Option<u64>,
        visual_threshold: u32,
    ) -> bool {
        // Check if element hash changed
        let mut screen_hash = self.screen_hash.write().await;
        let element_changed = *screen_hash != element_hash;

        // Check if visual hash changed significantly
        let mut stored_screenshot_hash = self.screenshot_hash.write().await;
        let visual_changed = if let Some(new_hash) = screenshot_hash {
            let old_hash = *stored_screenshot_hash;
            // Hamming distance: number of differing bits
            let distance = (old_hash ^ new_hash).count_ones();
            distance > visual_threshold
        } else {
            false
        };

        // If neither changed, nothing to do
        if !element_changed && !visual_changed {
            return false;
        }

        // Update hashes
        *screen_hash = element_hash;
        if let Some(new_hash) = screenshot_hash {
            *stored_screenshot_hash = new_hash;
        }
        drop(screen_hash);
        drop(stored_screenshot_hash);

        // Wrap in Arc for efficient sharing
        let elements_arc = Arc::new(elements);
        let screenshot_arc = screenshot.map(Arc::new);

        // Update cached elements
        *self.current_elements.write().await = Some(elements_arc.clone());

        // Update screenshot if provided
        if let Some(ref ss) = screenshot_arc {
            *self.current_screenshot.write().await = Some(ss.clone());
            // Also send ScreenshotUpdated for watchers that only listen to that
            let _ = self.event_tx.send(SessionEvent::ScreenshotUpdated(ss.clone()));
        }

        // Broadcast screen info updated event
        let _ = self.event_tx.send(SessionEvent::ScreenInfoUpdated {
            elements: elements_arc,
            screenshot: screenshot_arc,
        });

        true
    }

    /// Returns the current UI elements, if available.
    ///
    /// # Returns
    ///
    /// `Some(Arc<Vec<UIElement>>)` containing the cached elements,
    /// or `None` if no screen info has been captured yet.
    pub async fn get_current_elements(&self) -> Option<Arc<Vec<UIElement>>> {
        self.current_elements.read().await.clone()
    }
}

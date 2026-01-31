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
//!     let session = Session::new(Some("SIMULATOR-UDID".to_string()));
//!
//!     // Subscribe to events (for a watcher)
//!     let mut rx = session.subscribe();
//!
//!     // Log an action
//!     session.log_action(
//!         ActionType::TapElement { id: "button".to_string() },
//!         ActionResult::Success,
//!         None
//!     ).await;
//! }
//! ```

use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use uuid::Uuid;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::action::{ActionLog, ActionType, ActionResult};

/// Maximum number of action log entries to retain in the ring buffer.
const MAX_ACTION_LOG_SIZE: usize = 1000;

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
///
/// Sessions are created via [`Session::new`], which returns an `Arc<Session>`
/// for safe sharing across async tasks.
#[derive(Debug)]
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
}

impl Session {
    /// Creates a new session.
    ///
    /// # Arguments
    ///
    /// * `simulator_udid` - Optional UDID of the simulator to associate with this session
    ///
    /// # Returns
    ///
    /// An `Arc<Session>` for safe sharing across async tasks. The session is
    /// initialized with a new UUID, the current timestamp, an empty action log,
    /// and no screenshot.
    pub fn new(simulator_udid: Option<String>) -> Arc<Self> {
        let (event_tx, _) = broadcast::channel(100);
        Arc::new(Self {
            id: Uuid::new_v4(),
            created_at: Utc::now(),
            simulator_udid,
            action_log: RwLock::new(VecDeque::with_capacity(MAX_ACTION_LOG_SIZE)),
            current_screenshot: RwLock::new(None),
            event_tx,
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
    /// screenshot, then adds it to the session's action log and broadcasts it to
    /// all subscribers.
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
    /// is reached, the oldest entry is removed.
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

        // Update screenshot if provided
        if let Some(ref ss) = screenshot_arc {
            *self.current_screenshot.write().await = Some(ss.clone());
            if let Err(e) = self.event_tx.send(SessionEvent::ScreenshotUpdated(ss.clone())) {
                eprintln!("[session] Failed to broadcast screenshot update: {}", e);
            }
        }

        // Broadcast action
        if let Err(e) = self.event_tx.send(SessionEvent::ActionLogged(log.clone())) {
            eprintln!("[session] Failed to broadcast action logged event: {}", e);
        }

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
        if let Err(e) = self.event_tx.send(SessionEvent::ScreenshotUpdated(screenshot_arc)) {
            eprintln!("[session] Failed to broadcast screenshot update: {}", e);
        }
    }
}

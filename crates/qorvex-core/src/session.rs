// Session management for iOS Simulator automation

use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use uuid::Uuid;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::action::{ActionLog, ActionType, ActionResult};

/// Event broadcast to watchers when session state changes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionEvent {
    /// New action was logged
    ActionLogged(ActionLog),
    /// Screenshot was updated
    ScreenshotUpdated(String), // base64 PNG
    /// Session started
    Started { session_id: Uuid },
    /// Session ended
    Ended,
}

/// Shared session state
#[derive(Debug)]
pub struct Session {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub simulator_udid: Option<String>,
    action_log: RwLock<Vec<ActionLog>>,
    current_screenshot: RwLock<Option<String>>,
    event_tx: broadcast::Sender<SessionEvent>,
}

impl Session {
    pub fn new(simulator_udid: Option<String>) -> Arc<Self> {
        let (event_tx, _) = broadcast::channel(100);
        Arc::new(Self {
            id: Uuid::new_v4(),
            created_at: Utc::now(),
            simulator_udid,
            action_log: RwLock::new(Vec::new()),
            current_screenshot: RwLock::new(None),
            event_tx,
        })
    }

    /// Subscribe to session events (for watchers)
    pub fn subscribe(&self) -> broadcast::Receiver<SessionEvent> {
        self.event_tx.subscribe()
    }

    /// Log an action and broadcast to watchers
    pub async fn log_action(&self, action: ActionType, result: ActionResult, screenshot: Option<String>) -> ActionLog {
        let log = ActionLog::new(action, result, screenshot.clone());

        // Update action log
        self.action_log.write().await.push(log.clone());

        // Update screenshot if provided
        if let Some(ref ss) = screenshot {
            *self.current_screenshot.write().await = Some(ss.clone());
            let _ = self.event_tx.send(SessionEvent::ScreenshotUpdated(ss.clone()));
        }

        // Broadcast action
        let _ = self.event_tx.send(SessionEvent::ActionLogged(log.clone()));

        log
    }

    /// Get all action logs
    pub async fn get_action_log(&self) -> Vec<ActionLog> {
        self.action_log.read().await.clone()
    }

    /// Get current screenshot
    pub async fn get_screenshot(&self) -> Option<String> {
        self.current_screenshot.read().await.clone()
    }

    /// Update screenshot without logging action
    pub async fn update_screenshot(&self, screenshot: String) {
        *self.current_screenshot.write().await = Some(screenshot.clone());
        let _ = self.event_tx.send(SessionEvent::ScreenshotUpdated(screenshot));
    }
}

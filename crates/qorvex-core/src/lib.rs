//! # qorvex-core
//!
//! Core library for iOS Simulator automation on macOS.
//!
//! This crate provides the foundational components for interacting with iOS simulators,
//! including device management, accessibility-based UI automation, session tracking,
//! and inter-process communication.
//!
//! ## Modules
//!
//! - [`simctl`] - Wrapper around Apple's `xcrun simctl` CLI for simulator control
//! - [`axe`] - Wrapper around the `axe` accessibility tool for UI inspection and interaction
//! - [`session`] - Session state management with event broadcasting
//! - [`ipc`] - Unix socket-based IPC for REPL and watcher communication
//! - [`action`] - Action types and logging for automation operations
//! - [`executor`] - Action execution engine with result handling
//!
//! ## External Dependencies
//!
//! This crate requires the following external tools to be installed:
//!
//! - **Xcode** (for `xcrun simctl`) - Provides simulator control functionality
//! - **axe** - Third-party accessibility tool (`brew install cameroncooke/axe/axe`)
//!
//! ## Example
//!
//! ```no_run
//! use qorvex_core::simctl::Simctl;
//! use qorvex_core::axe::Axe;
//!
//! // Get the booted simulator
//! let udid = Simctl::get_booted_udid().expect("No booted simulator");
//!
//! // Dump the UI hierarchy
//! let hierarchy = Axe::dump_hierarchy(&udid).expect("Failed to dump UI");
//!
//! // Find and tap a button
//! if let Some(button) = Axe::find_element(&hierarchy, "login-button") {
//!     Axe::tap_element(&udid, "login-button").expect("Failed to tap");
//! }
//! ```

pub mod action;
pub mod axe;
pub mod executor;
pub mod ipc;
pub mod session;
pub mod simctl;
pub mod watcher;

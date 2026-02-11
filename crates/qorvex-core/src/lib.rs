//! # qorvex-core
//!
//! Core library for iOS Simulator and device automation on macOS.
//!
//! This crate provides the foundational components for interacting with iOS simulators
//! and physical devices, including a backend-agnostic driver abstraction, binary protocol
//! codec, session tracking, and inter-process communication.
//!
//! ## Modules
//!
//! ### Driver abstraction
//! - [`driver`] - `AutomationDriver` trait, `DriverConfig`, glob matching for element selectors
//! - [`element`] - Shared `UIElement` and `ElementFrame` types
//! - [`protocol`] - Binary wire protocol codec for Rust ↔ Swift agent communication
//! - [`executor`] - Backend-agnostic action execution engine
//!
//! ### Backends
//! - [`agent_client`] - Low-level async TCP client for the Swift agent
//! - [`agent_driver`] - `AgentDriver` backend (simulators via TCP, devices via USB tunnel)
//! - [`agent_lifecycle`] - Swift agent install/launch/health-check via `xcrun simctl`
//! - [`usb_tunnel`] - Physical device discovery and port forwarding via usbmuxd
//!
//! ### Infrastructure
//! - [`simctl`] - Wrapper around Apple's `xcrun simctl` CLI for simulator control
//! - [`session`] - Session state management with event broadcasting
//! - [`ipc`] - Unix socket-based IPC for REPL and watcher communication
//! - [`action`] - Action types and logging for automation operations
//! - [`watcher`] - Screen change detection via accessibility tree polling
//!
//! ## External Dependencies
//!
//! - **Xcode** (for `xcrun simctl`) - Provides simulator control functionality
//!
//! ## Example
//!
//! ```no_run
//! use qorvex_core::simctl::Simctl;
//!
//! // Get the booted simulator
//! let udid = Simctl::get_booted_udid().expect("No booted simulator");
//! ```

pub mod action;
pub mod agent_client;
pub mod agent_lifecycle;
pub mod agent_driver;
pub mod element;
pub mod driver;
pub mod executor;
pub mod ipc;
pub mod protocol;
pub mod session;
pub mod simctl;
pub mod usb_tunnel;
pub mod watcher;

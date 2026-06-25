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
//! - [`android_driver`] - `AndroidDriver` backend (Kotlin agent via `adb forward` TCP tunnel)
//! - [`agent_lifecycle`] - Swift agent install/launch/health-check via `xcrun simctl`
//! - [`android_lifecycle`] - Kotlin agent Gradle-build/install/`am instrument`/health-poll via `adb`
//! - [`usb_tunnel`] - Physical device discovery and port forwarding via usbmuxd
//!
//! ### Infrastructure
//! - [`simctl`] - Wrapper around Apple's `xcrun simctl` CLI for simulator control
//! - [`adb_device`] - Wrapper around Android's `adb` CLI for device/emulator control
//! - [`adb_forward`] - Single `adb forward` TCP tunnel to the on-device Android agent
//! - [`session`] - Session state management with event broadcasting
//! - [`ipc`] - Unix socket-based IPC for REPL and watcher communication
//! - [`action`] - Action types and logging for automation operations
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
pub mod adb_device;
pub mod adb_forward;
pub mod agent_client;
pub mod agent_driver;
pub mod agent_lifecycle;
pub mod agent_session;
pub mod android_driver;
pub mod android_lifecycle;
pub mod config;
pub mod core_device_tunnel;
pub mod coredevice;
pub mod driver;
pub mod element;
pub mod executor;
pub mod ipc;
pub mod protocol;
pub mod session;
pub mod simctl;
pub mod usb_tunnel;

//! Shared memory-footprint types for `memory-info`.
//!
//! [`MemoryInfo`] is the one output shape the command reports on every
//! platform, so the iOS side ([`crate::simctl::Simctl::memory_info`]) and the
//! Android side ([`crate::adb_device::Adb::memory_info`]) both build it and
//! the server serializes it without a per-platform branch. It lives in its own
//! module rather than in either backend because both of them, and the server,
//! need it.

use serde::{Deserialize, Serialize};

/// How hard the device is currently pressed for memory.
///
/// Deliberately coarse: the two platforms report pressure in units that do not
/// compare (a macOS `kern.memorystatus_vm_pressure_level` level vs. a Linux PSI
/// stall percentage), but both map cleanly onto these three buckets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryPressure {
    /// Memory is comfortable.
    Normal,
    /// The system is reclaiming and apps may start getting warnings.
    Warn,
    /// The system is thrashing or killing processes.
    Critical,
}

/// The target app's own memory footprint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppMemory {
    /// Process id of the running app.
    pub pid: u32,
    /// Resident footprint in bytes (host RSS on the simulator, TOTAL PSS on
    /// Android).
    pub footprint_bytes: u64,
    /// The command the figure came from, so a surprising number can be traced.
    pub source: String,
}

/// The device's overall memory state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceMemory {
    /// Total physical memory in bytes.
    pub total_bytes: u64,
    /// Memory available for a new allocation, in bytes.
    pub free_bytes: u64,
    /// Current memory pressure.
    pub pressure: MemoryPressure,
    /// The command(s) the figures came from.
    pub source: String,
}

/// The `memory-info` report: the target app's footprint and its device's
/// memory state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryInfo {
    /// The target app's footprint.
    pub app: AppMemory,
    /// The device the app is running on.
    pub device: DeviceMemory,
}

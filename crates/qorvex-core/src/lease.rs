//! Throwaway simulator leases and disk reclamation.
//!
//! A lease is a simulator cloned from a shut-down golden device
//! (`qorvex-golden-<model>`) and named `qorvex-lease-<short-id>`. Leases are
//! recorded in `leases.db` under the qorvex state directory together with a
//! heartbeat and a TTL, so a lease whose owner died can be found and deleted.
//!
//! The selection logic ([`plan_reap`], [`plan_reclaim`]) is pure: it works on
//! lease rows and [`DeviceInfo`] snapshots, so it is tested without a simulator.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use rusqlite::{params, Connection};
use serde::Serialize;

use crate::ipc::qorvex_dir;
use crate::simctl::{Simctl, SimctlError};

/// Name prefix of every leased (and therefore deletable) simulator.
pub const LEASE_PREFIX: &str = "qorvex-lease-";

/// Name prefix of the shut-down devices leases are cloned from.
pub const GOLDEN_PREFIX: &str = "qorvex-golden-";

/// Default lease TTL: 4 hours.
pub const DEFAULT_TTL_SECS: u64 = 4 * 3600;

/// Default age after which an idle shut-down simulator is reclaimable: 14 days.
pub const DEFAULT_OLDER_THAN_SECS: u64 = 14 * 86400;

/// A lease-named device without a row is left alone this long after its
/// directory was last touched, so a clone that is still being recorded by
/// `lease` is never reaped from under it.
const ORPHAN_GRACE_SECS: i64 = 600;

/// Errors from lease operations.
#[derive(Debug, thiserror::Error)]
pub enum LeaseError {
    #[error("lease database error: {0}")]
    Db(#[from] rusqlite::Error),

    #[error("{0}")]
    Simctl(#[from] SimctlError),

    #[error("{0}")]
    Invalid(String),
}

/// A row of the lease table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Lease {
    pub udid: String,
    pub name: String,
    pub model: String,
    pub owner: String,
    pub created_at: i64,
    pub heartbeat_at: i64,
    pub ttl_secs: i64,
}

impl Lease {
    /// Whether the last heartbeat is older than the TTL at `now` (unix seconds).
    pub fn is_expired(&self, now: i64) -> bool {
        now - self.heartbeat_at > self.ttl_secs
    }
}

/// Current time as unix seconds.
pub fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Parses `<n>s|m|h|d` into seconds.
pub fn parse_duration(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let bad = || format!("invalid duration '{}': expected <n>s|m|h|d", s);
    let (num, mult) = [("s", 1u64), ("m", 60), ("h", 3600), ("d", 86400)]
        .into_iter()
        .find_map(|(u, m)| s.strip_suffix(u).map(|n| (n, m)))
        .ok_or_else(bad)?;
    let n: u64 = num.parse().map_err(|_| bad())?;
    n.checked_mul(mult)
        .filter(|secs| *secs <= i64::MAX as u64)
        .ok_or_else(|| format!("duration '{}' is too large", s))
}

/// SQLite-backed store of leases.
pub struct LeaseDb {
    conn: Connection,
}

impl LeaseDb {
    /// Opens (creating if needed) `leases.db` in the qorvex state directory.
    pub fn open_default() -> Result<Self, LeaseError> {
        Self::open(&qorvex_dir().join("leases.db"))
    }

    /// Opens (creating if needed) a lease database at `path`.
    pub fn open(path: &Path) -> Result<Self, LeaseError> {
        Self::init(Connection::open(path)?)
    }

    /// Opens a private in-memory database.
    pub fn open_in_memory() -> Result<Self, LeaseError> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self, LeaseError> {
        // Agents lease and heartbeat concurrently; wait out a competing writer.
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS leases (
                udid TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                model TEXT NOT NULL,
                owner TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                heartbeat_at INTEGER NOT NULL,
                ttl_secs INTEGER NOT NULL
            )",
        )?;
        Ok(Self { conn })
    }

    pub fn insert(&self, l: &Lease) -> Result<(), LeaseError> {
        self.conn.execute(
            "INSERT INTO leases (udid, name, model, owner, created_at, heartbeat_at, ttl_secs)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                l.udid,
                l.name,
                l.model,
                l.owner,
                l.created_at,
                l.heartbeat_at,
                l.ttl_secs
            ],
        )?;
        Ok(())
    }

    /// Sets the heartbeat of `udid` to `now`; false if there is no such lease.
    pub fn heartbeat(&self, udid: &str, now: i64) -> Result<bool, LeaseError> {
        let n = self.conn.execute(
            "UPDATE leases SET heartbeat_at = ?2 WHERE udid = ?1",
            params![udid, now],
        )?;
        Ok(n > 0)
    }

    /// Removes the row for `udid`; false if there was none.
    pub fn remove(&self, udid: &str) -> Result<bool, LeaseError> {
        let n = self
            .conn
            .execute("DELETE FROM leases WHERE udid = ?1", params![udid])?;
        Ok(n > 0)
    }

    /// The lease for `udid`, if any.
    pub fn get(&self, udid: &str) -> Result<Option<Lease>, LeaseError> {
        Ok(self.list()?.into_iter().find(|l| l.udid == udid))
    }

    /// All leases, oldest first.
    pub fn list(&self) -> Result<Vec<Lease>, LeaseError> {
        let mut stmt = self.conn.prepare(
            "SELECT udid, name, model, owner, created_at, heartbeat_at, ttl_secs
             FROM leases ORDER BY created_at, udid",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Lease {
                udid: r.get(0)?,
                name: r.get(1)?,
                model: r.get(2)?,
                owner: r.get(3)?,
                created_at: r.get(4)?,
                heartbeat_at: r.get(5)?,
                ttl_secs: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

/// A simulator as seen on this machine, for selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceInfo {
    pub udid: String,
    pub name: String,
    pub booted: bool,
    pub available: bool,
    /// Newest mtime (unix seconds) of the device directory and its `data`
    /// directory; `None` if the directory is missing.
    pub mtime: Option<i64>,
}

/// `~/Library/Developer/CoreSimulator/Devices`.
pub fn devices_dir() -> PathBuf {
    core_simulator_dir().join("Devices")
}

/// `~/Library/Developer/CoreSimulator`.
pub fn core_simulator_dir() -> PathBuf {
    dirs::home_dir()
        .expect("Could not determine home directory")
        .join("Library/Developer/CoreSimulator")
}

fn mtime_secs(path: &Path) -> Option<i64> {
    let m = std::fs::metadata(path).ok()?.modified().ok()?;
    let d = m.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(d.as_secs() as i64)
}

/// Snapshot of every simulator with its directory mtime.
pub fn gather_devices() -> Result<Vec<DeviceInfo>, LeaseError> {
    let root = devices_dir();
    Ok(Simctl::list_device_details()?
        .into_iter()
        .map(|d| {
            let dir = root.join(&d.udid);
            let mtime = mtime_secs(&dir)
                .into_iter()
                .chain(mtime_secs(&dir.join("data")))
                .max();
            DeviceInfo {
                booted: d.state == "Booted",
                udid: d.udid,
                name: d.name,
                available: d.is_available,
                mtime,
            }
        })
        .collect())
}

/// On-disk size in bytes of a directory (`du -sk`); 0 if it is missing.
pub fn dir_size_bytes(path: &Path) -> u64 {
    Command::new("du")
        .arg("-sk")
        .arg(path)
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .split_whitespace()
                .next()
                .and_then(|k| k.parse::<u64>().ok())
        })
        .map_or(0, |k| k * 1024)
}

/// What [`plan_reap`] decided.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReapPlan {
    /// Leases past their TTL. The device may already be gone.
    pub expired: Vec<Lease>,
    /// Lease-named devices with no row that are safe to delete.
    pub orphans: Vec<DeviceInfo>,
    /// Lease-named devices with no row that were left alone (booted, or touched
    /// too recently to tell).
    pub skipped: Vec<DeviceInfo>,
}

/// Picks the leases and orphan devices `reap` should delete.
///
/// An orphan is a `qorvex-lease-*` device with no row. It is deleted when it is
/// not booted, or when it is booted but its directory has been idle longer than
/// the default TTL. Anything touched within the last ten minutes is left alone.
pub fn plan_reap(leases: &[Lease], devices: &[DeviceInfo], now: i64) -> ReapPlan {
    let known: HashSet<&str> = leases.iter().map(|l| l.udid.as_str()).collect();
    let mut plan = ReapPlan {
        expired: leases
            .iter()
            .filter(|l| l.is_expired(now))
            .cloned()
            .collect(),
        ..Default::default()
    };
    for d in devices {
        if !d.name.starts_with(LEASE_PREFIX) || known.contains(d.udid.as_str()) {
            continue;
        }
        let idle = d.mtime.map(|m| now - m);
        let fresh = idle.is_some_and(|i| i <= ORPHAN_GRACE_SECS);
        let stale_booted = idle.is_some_and(|i| i > DEFAULT_TTL_SECS as i64);
        if !fresh && (!d.booted || stale_booted) {
            plan.orphans.push(d.clone());
        } else {
            plan.skipped.push(d.clone());
        }
    }
    plan
}

/// One device `sims reclaim` would delete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Candidate {
    pub udid: String,
    pub name: String,
    pub reason: &'static str,
}

/// What [`plan_reclaim`] decided.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReclaimPlan {
    pub candidates: Vec<Candidate>,
    /// Devices that would qualify but are booted; reclaim never touches them
    /// (`qorvex reap` handles expired leases).
    pub skipped_booted: Vec<DeviceInfo>,
}

/// Picks the simulators `sims reclaim` should delete: unavailable devices,
/// expired and orphan leases, and shut-down devices idle for `older_than_secs`
/// (never goldens, never devices with a live lease, never booted devices).
pub fn plan_reclaim(
    leases: &[Lease],
    devices: &[DeviceInfo],
    now: i64,
    older_than_secs: u64,
) -> ReclaimPlan {
    let reap = plan_reap(leases, devices, now);
    let live: HashSet<&str> = leases
        .iter()
        .filter(|l| !l.is_expired(now))
        .map(|l| l.udid.as_str())
        .collect();
    let expired: HashSet<&str> = reap.expired.iter().map(|l| l.udid.as_str()).collect();
    let orphans: HashSet<&str> = reap.orphans.iter().map(|d| d.udid.as_str()).collect();

    let mut plan = ReclaimPlan::default();
    for d in devices {
        if d.name.starts_with(GOLDEN_PREFIX) {
            continue;
        }
        let reason = if expired.contains(d.udid.as_str()) {
            "expired lease"
        } else if orphans.contains(d.udid.as_str()) {
            "orphan lease"
        } else if !d.available {
            "unavailable"
        } else if live.contains(d.udid.as_str())
            || !d.mtime.is_some_and(|m| now - m > older_than_secs as i64)
        {
            continue;
        } else {
            "idle"
        };
        if d.booted {
            // Only a lease's device is worth reporting; an idle or unavailable
            // device that is booted is someone's live simulator.
            if reason.ends_with("lease") {
                plan.skipped_booted.push(d.clone());
            }
        } else {
            plan.candidates.push(Candidate {
                udid: d.udid.clone(),
                name: d.name.clone(),
                reason,
            });
        }
    }
    plan
}

/// Shuts down (if booted) and deletes a simulator, then drops its lease row.
/// Does not check the name; callers that need the lease-prefix guard use
/// [`release`].
pub fn delete_device(db: &LeaseDb, udid: &str) -> Result<(), LeaseError> {
    // Shutdown fails harmlessly on a device that is already shut down.
    let _ = Simctl::shutdown(udid);
    Simctl::delete(udid)?;
    db.remove(udid)?;
    Ok(())
}

/// Deletes a simulator without shutting it down first, then drops its lease
/// row. `simctl delete` fails on a device that is booted, which is what
/// `sims reclaim` wants if someone booted it after the plan was made.
pub fn delete_only(db: &LeaseDb, udid: &str) -> Result<(), LeaseError> {
    Simctl::delete(udid)?;
    db.remove(udid)?;
    Ok(())
}

/// What [`release`] did.
#[derive(Debug, PartialEq, Eq)]
pub enum ReleaseOutcome {
    Deleted,
    /// The device was already gone; only the row was removed.
    RowOnly,
}

/// Refuses any name that is not a lease clone.
pub fn check_releasable(name: &str) -> Result<(), LeaseError> {
    if name.starts_with(LEASE_PREFIX) {
        Ok(())
    } else {
        Err(LeaseError::Invalid(format!(
            "refusing to delete '{}': name does not start with {}",
            name, LEASE_PREFIX
        )))
    }
}

/// Deletes the leased simulator `udid` and its row. Idempotent: a device that
/// no longer exists just loses its row.
pub fn release(db: &LeaseDb, udid: &str) -> Result<ReleaseOutcome, LeaseError> {
    let details = Simctl::list_device_details()?;
    match details.iter().find(|d| d.udid == udid) {
        None => {
            db.remove(udid)?;
            Ok(ReleaseOutcome::RowOnly)
        }
        Some(d) => {
            check_releasable(&d.name)?;
            delete_device(db, udid)?;
            Ok(ReleaseOutcome::Deleted)
        }
    }
}

/// Leases a fresh simulator: ensures the golden, clones it, records the lease,
/// boots the clone and waits for the boot. The clone is deleted on any failure
/// after it was made.
pub fn acquire(
    db: &LeaseDb,
    model: &str,
    owner: &str,
    runtime: Option<&str>,
    ttl_secs: u64,
) -> Result<Lease, LeaseError> {
    let golden_name = format!("{}{}", GOLDEN_PREFIX, model);
    // Serialise golden lookup/creation and the clone across concurrent leases,
    // so two callers cannot both create the golden. Released before the boot.
    let clone = {
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(qorvex_dir().join("leases.lock"))
            .map_err(|e| LeaseError::Invalid(format!("lease lock: {}", e)))?;
        lock.lock()
            .map_err(|e| LeaseError::Invalid(format!("lease lock: {}", e)))?;

        let golden = Simctl::list_device_details()?
            .into_iter()
            .find(|d| d.name == golden_name && d.is_available);
        let golden_udid = match golden {
            Some(d) => {
                if let Some(r) = runtime.filter(|r| *r != d.runtime) {
                    return Err(LeaseError::Invalid(format!(
                        "golden '{}' already exists with runtime {}, not {}",
                        golden_name, d.runtime, r
                    )));
                }
                d.udid
            }
            None => {
                let runtime = match runtime {
                    Some(r) => r.to_string(),
                    None => Simctl::newest_ios_runtime()?,
                };
                Simctl::create_device(&golden_name, model, &runtime)?
            }
        };

        let id = uuid::Uuid::new_v4().simple().to_string();
        let name = format!("{}{}", LEASE_PREFIX, &id[..8]);
        let udid = Simctl::clone_device(&golden_udid, &name)?;
        (udid, name)
    };
    let (udid, name) = clone;

    let now = now_secs();
    let lease = Lease {
        udid: udid.clone(),
        name,
        model: model.to_string(),
        owner: owner.to_string(),
        created_at: now,
        heartbeat_at: now,
        ttl_secs: ttl_secs as i64,
    };
    let result = db
        .insert(&lease)
        .and_then(|_| Ok(Simctl::boot(&udid)?))
        .and_then(|_| Ok(Simctl::wait_for_boot(&udid)?));
    if let Err(e) = result {
        let _ = delete_device(db, &udid);
        return Err(e);
    }
    Ok(lease)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_000_000;

    fn lease(udid: &str, heartbeat_at: i64, ttl: i64) -> Lease {
        Lease {
            udid: udid.into(),
            name: format!("{}{}", LEASE_PREFIX, udid),
            model: "iPhone Air".into(),
            owner: "o".into(),
            created_at: heartbeat_at,
            heartbeat_at,
            ttl_secs: ttl,
        }
    }

    fn dev(udid: &str, name: &str, booted: bool, age: Option<i64>) -> DeviceInfo {
        DeviceInfo {
            udid: udid.into(),
            name: name.into(),
            booted,
            available: true,
            mtime: age.map(|a| NOW - a),
        }
    }

    #[test]
    fn parse_duration_units() {
        assert_eq!(parse_duration("30s"), Ok(30));
        assert_eq!(parse_duration("5m"), Ok(300));
        assert_eq!(parse_duration("4h"), Ok(14400));
        assert_eq!(parse_duration("14d"), Ok(14 * 86400));
        // Fits u64 seconds but not i64, which the lease table stores.
        assert!(parse_duration("213503982334602d").is_err());
    }

    #[test]
    fn parse_duration_rejects_garbage() {
        for bad in [
            "",
            "h",
            "4",
            "4x",
            "-1h",
            "1.5h",
            "5é",
            "é",
            "99999999999999999999d",
        ] {
            assert!(parse_duration(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn expiry_is_strictly_after_ttl() {
        let l = lease("a", NOW - 100, 100);
        assert!(!l.is_expired(NOW));
        assert!(l.is_expired(NOW + 1));
    }

    #[test]
    fn db_roundtrip() {
        let db = LeaseDb::open_in_memory().unwrap();
        db.insert(&lease("a", 10, 100)).unwrap();
        db.insert(&lease("b", 20, 100)).unwrap();
        assert_eq!(db.list().unwrap().len(), 2);
        assert!(db.heartbeat("a", 99).unwrap());
        assert!(!db.heartbeat("zzz", 99).unwrap());
        assert_eq!(db.list().unwrap()[0].heartbeat_at, 99);
        assert!(db.remove("a").unwrap());
        assert!(!db.remove("a").unwrap());
        assert_eq!(db.list().unwrap().len(), 1);
    }

    #[test]
    fn db_persists_in_file() {
        let path =
            std::env::temp_dir().join(format!("qorvex-lease-test-{}.db", uuid::Uuid::new_v4()));
        LeaseDb::open(&path)
            .unwrap()
            .insert(&lease("a", 1, 2))
            .unwrap();
        assert_eq!(LeaseDb::open(&path).unwrap().list().unwrap().len(), 1);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn release_guard_refuses_foreign_names() {
        assert!(check_releasable("qorvex-lease-abc").is_ok());
        assert!(check_releasable("qorvex-golden-iPhone Air").is_err());
        assert!(check_releasable("iPhone 18 Pro").is_err());
    }

    #[test]
    fn reap_picks_expired_leases_only() {
        let leases = [lease("old", NOW - 500, 100), lease("new", NOW - 10, 100)];
        let plan = plan_reap(&leases, &[], NOW);
        assert_eq!(plan.expired.len(), 1);
        assert_eq!(plan.expired[0].udid, "old");
    }

    #[test]
    fn reap_orphans_are_conservative() {
        let devices = [
            dev("shut", "qorvex-lease-1", false, Some(3600)),
            dev("fresh", "qorvex-lease-2", false, Some(60)),
            dev("booted", "qorvex-lease-3", true, Some(3600)),
            dev("booted-old", "qorvex-lease-4", true, Some(5 * 3600)),
            dev("other", "iPhone 18 Pro", false, Some(99 * 86400)),
            dev("row", "qorvex-lease-5", false, Some(3600)),
        ];
        let leases = [lease("row", NOW - 10, 100)];
        let plan = plan_reap(&leases, &devices, NOW);
        let orphans: Vec<_> = plan.orphans.iter().map(|d| d.udid.as_str()).collect();
        let skipped: Vec<_> = plan.skipped.iter().map(|d| d.udid.as_str()).collect();
        assert_eq!(orphans, ["shut", "booted-old"]);
        assert_eq!(skipped, ["fresh", "booted"]);
    }

    #[test]
    fn reclaim_selection() {
        let mut unavailable = dev("unavail", "iPhone 15", false, Some(60));
        unavailable.available = false;
        let mut unavailable_golden =
            dev("golden-un", "qorvex-golden-iPad", false, Some(99 * 86400));
        unavailable_golden.available = false;
        let devices = [
            unavailable,
            dev("idle", "iPhone 18 Pro", false, Some(20 * 86400)),
            dev("recent", "iPad", false, Some(86400)),
            dev(
                "golden",
                "qorvex-golden-iPhone Air",
                false,
                Some(99 * 86400),
            ),
            dev("booted-idle", "iPhone 17", true, Some(99 * 86400)),
            dev("exp", "qorvex-lease-e", false, Some(3600)),
            dev("exp-booted", "qorvex-lease-eb", true, Some(3600)),
            dev("live", "qorvex-lease-l", false, Some(99 * 86400)),
            dev("orphan", "qorvex-lease-o", false, Some(3600)),
        ];
        let leases = [
            lease("exp", NOW - 500, 100),
            lease("exp-booted", NOW - 500, 100),
            lease("live", NOW - 10, 100),
        ];
        let plan = plan_reclaim(&leases, &devices, NOW, 14 * 86400);
        let got: Vec<_> = plan
            .candidates
            .iter()
            .map(|c| (c.udid.as_str(), c.reason))
            .collect();
        assert_eq!(
            got,
            [
                ("unavail", "unavailable"),
                ("idle", "idle"),
                ("exp", "expired lease"),
                ("orphan", "orphan lease"),
            ]
        );
        let skipped: Vec<_> = plan
            .skipped_booted
            .iter()
            .map(|d| d.udid.as_str())
            .collect();
        assert_eq!(skipped, ["exp-booted"]);
    }

    #[test]
    fn newest_runtime_is_numeric_and_ios_only() {
        let json = br#"{"runtimes":[
            {"identifier":"com.apple.CoreSimulator.SimRuntime.iOS-9-3","version":"9.3","isAvailable":true},
            {"identifier":"com.apple.CoreSimulator.SimRuntime.iOS-26-5","version":"26.5","isAvailable":true},
            {"identifier":"com.apple.CoreSimulator.SimRuntime.iOS-27-0","version":"27.0","isAvailable":false},
            {"identifier":"com.apple.CoreSimulator.SimRuntime.watchOS-30-0","version":"30.0","isAvailable":true}]}"#;
        assert_eq!(
            Simctl::newest_ios_runtime_from(json).unwrap().as_deref(),
            Some("com.apple.CoreSimulator.SimRuntime.iOS-26-5")
        );
    }
}

//! `lease`, `release`, `reap` and `sims reclaim`: throwaway simulators and disk
//! reclamation. Selection logic lives in `qorvex_core::lease`; this is the
//! glue that runs it and prints the result.

use qorvex_core::lease::{
    acquire, core_simulator_dir, delete_device, delete_only, devices_dir, dir_size_bytes,
    gather_devices, now_secs, parse_duration, plan_reap, plan_reclaim, release, Lease, LeaseDb,
    ReleaseOutcome, DEFAULT_OLDER_THAN_SECS,
};

use crate::CliError;

fn fail<E: std::fmt::Display>(what: &str) -> impl FnOnce(E) -> CliError + '_ {
    move |e| CliError::ActionFailed(format!("{}: {}", what, e))
}

fn open_db() -> Result<LeaseDb, CliError> {
    LeaseDb::open_default().map_err(fail("lease database"))
}

fn human_bytes(b: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} B", b)
    } else {
        format!("{:.1} {}", v, UNITS[i])
    }
}

fn human_secs(s: i64) -> String {
    let s = s.max(0);
    match s {
        0..=59 => format!("{}s", s),
        60..=3599 => format!("{}m", s / 60),
        3600..=86399 => format!("{}h{}m", s / 3600, s % 3600 / 60),
        _ => format!("{}d{}h", s / 86400, s % 86400 / 3600),
    }
}

fn duration_arg(s: &str) -> Result<u64, CliError> {
    parse_duration(s).map_err(CliError::ActionFailed)
}

pub fn lease(
    model: Option<&str>,
    owner: Option<&str>,
    runtime: Option<&str>,
    ttl: &str,
    json: bool,
) -> Result<(), CliError> {
    let (Some(model), Some(owner)) = (model, owner) else {
        return Err(CliError::ActionFailed(
            "lease requires --model and --owner".into(),
        ));
    };
    let ttl = duration_arg(ttl)?;
    let db = open_db()?;
    let started = std::time::Instant::now();
    let l = acquire(&db, model, owner, runtime, ttl).map_err(fail("lease failed"))?;
    if json {
        println!(
            "{}",
            serde_json::to_string(&l).map_err(|e| CliError::Protocol(e.to_string()))?
        );
    } else {
        println!("{}", l.udid);
        println!("QORVEX_DEVICE={}", l.udid);
    }
    eprintln!(
        "Leased {} ({}) in {:.1}s",
        l.udid,
        l.name,
        started.elapsed().as_secs_f32()
    );
    Ok(())
}

pub fn heartbeat(udid: &str) -> Result<(), CliError> {
    if open_db()?
        .heartbeat(udid, now_secs())
        .map_err(fail("heartbeat"))?
    {
        Ok(())
    } else {
        Err(CliError::ActionFailed(format!("no lease for {}", udid)))
    }
}

pub fn list(json: bool) -> Result<(), CliError> {
    let leases = open_db()?.list().map_err(fail("lease list"))?;
    let now = now_secs();
    if json {
        let rows: Vec<_> = leases
            .iter()
            .map(|l| {
                let mut v = serde_json::to_value(l).unwrap_or_default();
                v["expired"] = l.is_expired(now).into();
                v
            })
            .collect();
        println!("{}", serde_json::json!({ "leases": rows }));
    } else if leases.is_empty() {
        eprintln!("No leases");
    } else {
        println!(
            "{:<36}  {:<19}  {:<16}  {:>7}  {:>9}  EXPIRED",
            "UDID", "NAME", "OWNER", "AGE", "HEARTBEAT"
        );
        for l in &leases {
            println!(
                "{:<36}  {:<19}  {:<16}  {:>7}  {:>9}  {}",
                l.udid,
                l.name,
                l.owner,
                human_secs(now - l.created_at),
                human_secs(now - l.heartbeat_at),
                if l.is_expired(now) { "yes" } else { "no" }
            );
        }
    }
    Ok(())
}

pub fn release_cmd(udid: Option<&str>, owner: Option<&str>) -> Result<(), CliError> {
    let db = open_db()?;
    let targets: Vec<String> = match (udid, owner) {
        (Some(u), _) => vec![u.to_string()],
        (None, Some(o)) => db
            .list()
            .map_err(fail("lease list"))?
            .into_iter()
            .filter(|l| l.owner == o)
            .map(|l| l.udid)
            .collect(),
        (None, None) => return Err(CliError::ActionFailed("pass a UDID or --owner".into())),
    };
    let mut failed = false;
    for u in &targets {
        match release(&db, u) {
            Ok(ReleaseOutcome::Deleted) => eprintln!("Released {}", u),
            Ok(ReleaseOutcome::RowOnly) => eprintln!("Released {} (device already gone)", u),
            Err(e) => {
                eprintln!("Error: {}: {}", u, e);
                failed = true;
            }
        }
    }
    if failed {
        Err(CliError::ActionFailed("release failed".into()))
    } else {
        Ok(())
    }
}

fn lease_detail(l: &Lease, now: i64) -> String {
    format!(
        "owner {}, heartbeat {} ago",
        l.owner,
        human_secs(now - l.heartbeat_at)
    )
}

pub fn reap(dry_run: bool) -> Result<(), CliError> {
    let db = open_db()?;
    let leases = db.list().map_err(fail("lease list"))?;
    let devices = gather_devices().map_err(fail("list devices"))?;
    let now = now_secs();
    let plan = plan_reap(&leases, &devices, now);
    let verb = if dry_run { "Would reap" } else { "Reaped" };
    let mut failed = false;
    for l in &plan.expired {
        // A heartbeat may have landed since the plan was made.
        let renewed = match db.get(&l.udid) {
            Ok(row) => row.is_none_or(|r| !r.is_expired(now_secs())),
            Err(_) => true,
        };
        if renewed {
            println!(
                "Skipped {} {} (released or heartbeat renewed)",
                l.udid, l.name
            );
            continue;
        }
        let res = if dry_run {
            Ok(())
        } else {
            release(&db, &l.udid).map(|_| ())
        };
        match res {
            Ok(()) => println!(
                "{} {} {} (expired lease: {})",
                verb,
                l.udid,
                l.name,
                lease_detail(l, now)
            ),
            Err(e) => {
                eprintln!("Error: {}: {}", l.udid, e);
                failed = true;
            }
        }
    }
    for d in &plan.orphans {
        let res = if dry_run {
            Ok(())
        } else {
            delete_device(&db, &d.udid)
        };
        match res {
            Ok(()) => println!("{} {} {} (orphan, no lease row)", verb, d.udid, d.name),
            Err(e) => {
                eprintln!("Error: {}: {}", d.udid, e);
                failed = true;
            }
        }
    }
    for d in &plan.skipped {
        println!(
            "Skipped {} {} (no lease row, but booted or recently used)",
            d.udid, d.name
        );
    }
    if plan.expired.is_empty() && plan.orphans.is_empty() {
        eprintln!("Nothing to reap");
    }
    if failed {
        Err(CliError::ActionFailed("reap failed".into()))
    } else {
        Ok(())
    }
}

pub fn reclaim(older_than: Option<&str>, yes: bool, json: bool) -> Result<(), CliError> {
    let older_than = match older_than {
        Some(s) => duration_arg(s)?,
        None => DEFAULT_OLDER_THAN_SECS,
    };
    let db = open_db()?;
    let leases = db.list().map_err(fail("lease list"))?;
    let devices = gather_devices().map_err(fail("list devices"))?;
    let plan = plan_reclaim(&leases, &devices, now_secs(), older_than);

    let root = devices_dir();
    let sized: Vec<_> = plan
        .candidates
        .iter()
        .map(|c| (c, dir_size_bytes(&root.join(&c.udid))))
        .collect();
    let total: u64 = sized.iter().map(|(_, b)| b).sum();

    let mut freed = 0u64;
    let mut deleted = Vec::new();
    let mut failed = false;
    if yes {
        for (c, bytes) in &sized {
            match delete_only(&db, &c.udid) {
                Ok(()) => {
                    freed += bytes;
                    deleted.push(c.udid.as_str());
                }
                Err(e) => {
                    eprintln!("Error: {}: {}", c.udid, e);
                    failed = true;
                }
            }
        }
    }
    let caches = dir_size_bytes(&core_simulator_dir().join("Caches"));

    if json {
        let rows: Vec<_> = sized
            .iter()
            .map(|(c, b)| serde_json::json!({ "udid": c.udid, "name": c.name, "reason": c.reason, "bytes": b }))
            .collect();
        let skipped: Vec<_> = plan.skipped_booted.iter().map(|d| &d.udid).collect();
        println!(
            "{}",
            serde_json::json!({
                "candidates": rows,
                "total_bytes": total,
                "deleted": deleted,
                "freed_bytes": freed,
                "skipped_booted": skipped,
                "caches_bytes": caches,
            })
        );
    } else {
        for (c, b) in &sized {
            println!(
                "{:>10}  {}  {}  ({})",
                human_bytes(*b),
                c.udid,
                c.name,
                c.reason
            );
        }
        if yes {
            println!(
                "Freed {} ({} devices deleted)",
                human_bytes(freed),
                deleted.len()
            );
        } else {
            println!(
                "Total {} across {} devices (dry run; pass --yes to delete)",
                human_bytes(total),
                sized.len()
            );
        }
        for d in &plan.skipped_booted {
            println!("Skipped {} {} (booted; use `qorvex reap`)", d.udid, d.name);
        }
        println!(
            "CoreSimulator Caches: {} (not deleted)",
            human_bytes(caches)
        );
        println!("Unused runtimes can also be large: see `xcrun simctl runtime list` and `xcrun simctl runtime delete`");
    }
    if failed {
        Err(CliError::ActionFailed(
            "reclaim failed for some devices".into(),
        ))
    } else {
        Ok(())
    }
}

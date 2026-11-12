//! Server-owned screen recording for `qorvex record start|stop`.
//!
//! The recorder is a child process of the server (the CLI exits between
//! `start` and `stop`): `xcrun simctl io <udid> recordVideo` on an iOS
//! simulator, `adb shell screenrecord` on an Android device. Dropping a
//! [`Recording`] interrupts the recorder, so stopping the server never leaves
//! one running.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tracing::{info, warn};

/// How long a recorder is given to finish its file after the interrupt.
const STOP_TIMEOUT: Duration = Duration::from_secs(15);
/// How long `start` watches the recorder for an immediate failure.
const START_GRACE: Duration = Duration::from_secs(1);

/// The device a recording runs on.
#[derive(Debug, Clone)]
pub enum RecordTarget {
    /// An iOS simulator, by UDID.
    Simulator { udid: String },
    /// An Android device: its adb serial and the on-device temp file.
    Android { serial: String, device_path: String },
}

/// A running recorder child and the host file it will produce.
pub struct Recording {
    child: Child,
    file: PathBuf,
    target: RecordTarget,
    /// `stop` has already dealt with the Android device file (removed it after
    /// a pull, or kept it because the pull failed); `Drop` leaves it alone.
    device_file_handled: bool,
}

/// The on-device temp file for an Android recording made by this server.
pub fn android_device_path() -> String {
    format!("/sdcard/qorvex-record-{}.mp4", std::process::id())
}

/// Refuses a `file` whose parent directory does not exist, before anything
/// is spawned.
pub fn check_parent(file: &Path) -> Result<(), String> {
    match file.parent() {
        Some(parent) if parent.is_dir() => Ok(()),
        Some(parent) => Err(format!(
            "record: parent directory does not exist: {}",
            parent.display()
        )),
        None => Err(format!("record: not a file path: {}", file.display())),
    }
}

impl Recording {
    /// Spawn the platform recorder for `target`, writing to `file`.
    pub async fn start(target: RecordTarget, file: PathBuf) -> Result<Self, String> {
        let mut cmd = match &target {
            RecordTarget::Simulator { udid } => {
                let mut c = Command::new("xcrun");
                c.args([
                    "simctl",
                    "io",
                    udid,
                    "recordVideo",
                    "--codec",
                    "h264",
                    "--force",
                ])
                .arg(&file);
                c
            }
            RecordTarget::Android {
                serial,
                device_path,
            } => {
                let mut c = Command::new("adb");
                c.args([
                    "-s",
                    serial,
                    "shell",
                    "screenrecord",
                    "--time-limit",
                    "0",
                    device_path,
                ]);
                c
            }
        };
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let child = cmd
            .spawn()
            .map_err(|e| format!("Failed to start recorder: {}", e))?;
        Self::watch_start(child, file, target).await
    }

    /// Give a freshly spawned recorder [`START_GRACE`] to fail; one that has
    /// already exited is reported with its stderr.
    async fn watch_start(
        child: Child,
        file: PathBuf,
        target: RecordTarget,
    ) -> Result<Self, String> {
        let mut rec = Recording {
            child,
            file,
            target,
            device_file_handled: false,
        };
        tokio::time::sleep(START_GRACE).await;
        match rec.child.try_wait() {
            Ok(None) => {
                // Only a start failure reads stderr; drain it from here on so
                // a chatty recorder never blocks on a full pipe.
                if let Some(mut stderr) = rec.child.stderr.take() {
                    tokio::spawn(async move {
                        let _ = tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await;
                    });
                }
                info!(file = %rec.file.display(), target = ?rec.target, "Recording started");
                Ok(rec)
            }
            Ok(Some(status)) => {
                let stderr = rec.read_stderr().await;
                Err(format!("Recorder exited at start ({}): {}", status, stderr))
            }
            Err(e) => Err(format!("Failed to start recorder: {}", e)),
        }
    }

    /// The host file this recording writes.
    pub fn file(&self) -> &Path {
        &self.file
    }

    /// End the recording cleanly and return the size of the host file in bytes.
    pub async fn stop(mut self) -> Result<u64, String> {
        self.interrupt();
        let status = match tokio::time::timeout(STOP_TIMEOUT, self.child.wait()).await {
            Ok(Ok(status)) => status,
            Ok(Err(e)) => return Err(format!("Failed to wait for recorder: {}", e)),
            Err(_) => {
                return Err(format!(
                    "Recorder did not exit within {} s of the interrupt",
                    STOP_TIMEOUT.as_secs()
                ))
            }
        };
        info!(%status, "Recorder exited");
        if let RecordTarget::Android {
            serial,
            device_path,
        } = &self.target
        {
            let pulled = Command::new("adb")
                .args(["-s", serial, "pull", device_path])
                .arg(&self.file)
                .stdin(Stdio::null())
                .output()
                .await;
            // On a failed pull the device copy is the only one: keep it.
            self.device_file_handled = true;
            let failure = match pulled {
                Ok(out) if out.status.success() => None,
                Ok(out) => Some(String::from_utf8_lossy(&out.stderr).trim().to_string()),
                Err(e) => Some(e.to_string()),
            };
            if let Some(reason) = failure {
                return Err(format!(
                    "Failed to pull recording (left on the device at {}): {}",
                    device_path, reason
                ));
            }
            self.remove_device_file();
        }
        match std::fs::metadata(&self.file) {
            Ok(meta) => Ok(meta.len()),
            Err(e) => Err(format!("Recording was not written ({}): {}", status, e)),
        }
    }

    /// Ask the recorder to finish: SIGINT to `simctl` (it finalises the mp4
    /// on SIGINT), or `pkill -INT screenrecord` on the device (the local
    /// `adb shell` exits when it does).
    fn interrupt(&self) {
        match &self.target {
            RecordTarget::Simulator { .. } => {
                if let Some(pid) = self.child.id() {
                    // SAFETY: plain kill(2) on our own child's pid.
                    unsafe {
                        libc::kill(pid as libc::pid_t, libc::SIGINT);
                    }
                }
            }
            RecordTarget::Android { serial, .. } => {
                let _ = std::process::Command::new("adb")
                    .args(["-s", serial, "shell", "pkill", "-INT", "screenrecord"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        }
    }

    /// Delete the on-device temp file of an Android recording.
    fn remove_device_file(&self) {
        if let RecordTarget::Android {
            serial,
            device_path,
        } = &self.target
        {
            let _ = std::process::Command::new("adb")
                .args(["-s", serial, "shell", "rm", "-f", device_path])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }

    async fn read_stderr(&mut self) -> String {
        let mut buf = String::new();
        if let Some(mut stderr) = self.child.stderr.take() {
            let _ = stderr.read_to_string(&mut buf).await;
        }
        buf.trim().to_string()
    }
}

impl Drop for Recording {
    /// A recording still running when it is dropped (session stopped, server
    /// exiting) is interrupted, given a few seconds to finish, then killed;
    /// an Android temp file is removed. No recorder outlives the server.
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            warn!(file = %self.file.display(), "Recording dropped while active; stopping recorder");
            self.interrupt();
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while matches!(self.child.try_wait(), Ok(None)) && std::time::Instant::now() < deadline
            {
                std::thread::sleep(Duration::from_millis(100));
            }
            if matches!(self.child.try_wait(), Ok(None)) {
                let _ = self.child.start_kill();
            }
        }
        if !self.device_file_handled {
            self.remove_device_file();
        }
    }
}

#[cfg(test)]
impl Recording {
    /// A recording backed by an arbitrary child (e.g. `sleep`), for tests
    /// that need an active recording without a device.
    pub fn from_child(child: Child, file: PathBuf, target: RecordTarget) -> Self {
        Recording {
            child,
            file,
            target,
            device_file_handled: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_parent_accepts_existing_dir() {
        let file = std::env::temp_dir().join("qorvex-record-test.mp4");
        assert!(check_parent(&file).is_ok());
    }

    #[test]
    fn check_parent_refuses_missing_dir() {
        let err = check_parent(Path::new("/nonexistent-qorvex-dir/x.mp4")).unwrap_err();
        assert_eq!(
            err,
            "record: parent directory does not exist: /nonexistent-qorvex-dir"
        );
    }

    #[test]
    fn android_device_path_is_per_server() {
        assert_eq!(
            android_device_path(),
            format!("/sdcard/qorvex-record-{}.mp4", std::process::id())
        );
    }

    /// A recorder that exits at once is reported as a start failure with its
    /// stderr, not as a running recording.
    #[tokio::test]
    async fn start_reports_recorder_that_exits_at_once() {
        let child = Command::new("sh")
            .args(["-c", "echo boom >&2; exit 3"])
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let target = RecordTarget::Simulator { udid: "t".into() };
        let err = Recording::watch_start(child, PathBuf::from("/tmp/x.mp4"), target)
            .await
            .err()
            .unwrap();
        assert!(err.starts_with("Recorder exited at start"), "{err}");
        assert!(err.ends_with("boom"), "{err}");
    }

    /// `stop` interrupts the recorder, waits for it and reports the file size.
    #[tokio::test]
    async fn stop_interrupts_and_reports_size() {
        let file =
            std::env::temp_dir().join(format!("qorvex-record-stop-{}.mp4", std::process::id()));
        std::fs::write(&file, b"12345").unwrap();
        let child = Command::new("sleep").arg("30").spawn().unwrap();
        let rec = Recording::from_child(
            child,
            file.clone(),
            RecordTarget::Simulator { udid: "t".into() },
        );
        let bytes = rec.stop().await.unwrap();
        assert_eq!(bytes, 5);
        let _ = std::fs::remove_file(&file);
    }

    /// Dropping an active recording leaves no recorder process behind.
    #[tokio::test]
    async fn drop_ends_active_recorder() {
        let child = Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id().unwrap() as libc::pid_t;
        let rec = Recording::from_child(
            child,
            PathBuf::from("/tmp/unused.mp4"),
            RecordTarget::Simulator { udid: "t".into() },
        );
        drop(rec);
        // The child has been reaped or is a zombie awaiting tokio's reaper;
        // either way kill(pid, 0) on a live `sleep` would succeed, so poll.
        let mut gone = false;
        for _ in 0..50 {
            let alive = unsafe { libc::kill(pid, 0) } == 0;
            let zombie = std::process::Command::new("ps")
                .args(["-o", "stat=", "-p", &pid.to_string()])
                .output()
                .map(|o| {
                    String::from_utf8_lossy(&o.stdout).trim().starts_with('Z')
                        || o.stdout.is_empty()
                })
                .unwrap_or(true);
            if !alive || zombie {
                gone = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(gone, "recorder pid {pid} still running after drop");
    }
}

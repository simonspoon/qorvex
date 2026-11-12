//! `qorvex doctor`: a read-only Android setup preflight.
//!
//! The check functions are pure: every environment value, path-existence
//! result and `adb` output is an argument, so the unit tests do not depend on
//! the machine they run on. [`run`] gathers the inputs and calls them.

use qorvex_core::adb_device::Adb;
use qorvex_core::android_lifecycle::{
    client_java_home_override, java_major_version, resolve_build_java_home,
};
use qorvex_core::config::QorvexConfig;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Result of one check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    Warn,
    Fail,
    Skip,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: &'static str,
    pub status: Status,
    pub detail: String,
    pub fix: Option<String>,
}

impl Check {
    fn ok(name: &'static str, detail: String) -> Self {
        Self {
            name,
            status: Status::Ok,
            detail,
            fix: None,
        }
    }
    fn warn(name: &'static str, detail: String, fix: Option<String>) -> Self {
        Self {
            name,
            status: Status::Warn,
            detail,
            fix,
        }
    }
    fn fail(name: &'static str, detail: String, fix: String) -> Self {
        Self {
            name,
            status: Status::Fail,
            detail,
            fix: Some(fix),
        }
    }
    fn skip(name: &'static str) -> Self {
        Self {
            name,
            status: Status::Skip,
            detail: "adb not available".to_string(),
            fix: None,
        }
    }

    /// The one-line text form.
    pub fn line(&self) -> String {
        match (self.status, &self.fix) {
            (Status::Ok, _) => format!("[ok] {}: {}", self.name, self.detail),
            (Status::Fail, fix) => format!(
                "[FAIL] {}: {}. Fix: {}",
                self.name,
                self.detail,
                fix.as_deref().unwrap_or("-")
            ),
            (Status::Warn, Some(fix)) => {
                format!("[warn] {}: {}. Fix: {}", self.name, self.detail, fix)
            }
            (Status::Warn, None) => format!("[warn] {}: {}", self.name, self.detail),
            (Status::Skip, _) => format!("[skip] {}: {}", self.name, self.detail),
        }
    }
}

/// True when no check failed.
pub fn all_ok(checks: &[Check]) -> bool {
    !checks.iter().any(|c| c.status == Status::Fail)
}

/// `{"ok":bool,"checks":[...]}`.
pub fn to_json(checks: &[Check]) -> serde_json::Value {
    let checks: Vec<_> = checks
        .iter()
        .map(|c| {
            let status = match c.status {
                Status::Ok => "ok",
                Status::Warn => "warn",
                Status::Fail => "fail",
                Status::Skip => "skip",
            };
            serde_json::json!({
                "name": c.name,
                "status": status,
                "detail": c.detail,
                "fix": c.fix,
            })
        })
        .collect();
    serde_json::json!({ "ok": !checks.iter().any(|c| c["status"] == "fail"), "checks": checks })
}

/// Check 1. `on_path` is the adb found on PATH; `version_line` its
/// `adb version` first line; `fallbacks` are existing adb binaries found in
/// the well-known SDK locations, in priority order.
pub fn check_adb(
    on_path: Option<&Path>,
    version_line: Option<&str>,
    fallbacks: &[PathBuf],
) -> Check {
    if let Some(path) = on_path {
        let version = version_line.unwrap_or("version unknown");
        return Check::ok("adb", format!("{} ({})", path.display(), version));
    }
    match fallbacks.first().and_then(|p| p.parent()) {
        Some(dir) => Check::fail(
            "adb",
            format!("not on PATH (found at {})", fallbacks[0].display()),
            format!("export PATH=\"{}:$PATH\"", dir.display()),
        ),
        None => Check::fail(
            "adb",
            "not on PATH and no Android SDK platform-tools found".to_string(),
            "install Android platform-tools (brew install --cask android-platform-tools) \
             and add its directory to PATH"
                .to_string(),
        ),
    }
}

/// Check 2. Sources are tried in order: `ANDROID_HOME`,
/// `ANDROID_SDK_ROOT`, config `android_sdk_root`, `local.properties`
/// `sdk.dir` (a valid `ANDROID_HOME` lets Gradle build even when `sdk.dir`
/// is stale). The first one set wins and must exist. `default_sdk` is the
/// default SDK dir when it exists on disk.
pub fn check_android_home(
    android_home: Option<&str>,
    android_sdk_root: Option<&str>,
    config_sdk_root: Option<&Path>,
    local_properties_sdk: Option<&str>,
    default_sdk: Option<&Path>,
    exists: &dyn Fn(&Path) -> bool,
) -> Check {
    let sources: [(&str, Option<String>); 4] = [
        ("ANDROID_HOME", android_home.map(str::to_string)),
        ("ANDROID_SDK_ROOT", android_sdk_root.map(str::to_string)),
        (
            "android_sdk_root in ~/.qorvex/config.json",
            config_sdk_root.map(|p| p.display().to_string()),
        ),
        (
            "sdk.dir in local.properties",
            local_properties_sdk.map(str::to_string),
        ),
    ];
    for (source, value) in sources {
        let Some(value) = value.filter(|v| !v.is_empty()) else {
            continue;
        };
        if exists(Path::new(&value)) {
            return Check::ok("ANDROID_HOME", format!("{source} = {value}"));
        }
        return Check::fail(
            "ANDROID_HOME",
            format!("{source} = {value}, but that path does not exist"),
            match default_sdk {
                Some(d) => format!(
                    "point {source} at an existing SDK, or export ANDROID_HOME=\"{}\"",
                    d.display()
                ),
                None => format!("point {source} at an existing Android SDK directory"),
            },
        );
    }
    match default_sdk {
        Some(d) => Check::fail(
            "ANDROID_HOME",
            "not set (Gradle fails with \"SDK location not found\")".to_string(),
            format!("export ANDROID_HOME=\"{}\"", d.display()),
        ),
        None => Check::warn(
            "ANDROID_HOME",
            "not set and no Android SDK found in the default location".to_string(),
            Some("install the Android SDK and export ANDROID_HOME=<sdk dir>".to_string()),
        ),
    }
}

/// Check 3. `resolved` is what start-agent would pin for Gradle (home and
/// major version). `ambient` is the user-chosen JDK (`QORVEX_ANDROID_JAVA_HOME`
/// / `JAVA_HOME`) with its major version when readable, used to name the
/// version when nothing in range was found.
pub fn check_jdk(resolved: Option<(&Path, u32)>, ambient: Option<(&Path, Option<u32>)>) -> Check {
    if let Some((home, major)) = resolved {
        return Check::ok("JDK", format!("Java {major} at {}", home.display()));
    }
    let detail = match ambient {
        Some((home, Some(major))) => format!(
            "Java {major} at {} is outside the supported 17-23 range for Gradle 8.10.2",
            home.display()
        ),
        Some((home, None)) => format!(
            "no usable java at {} (need Java 17-23 for Gradle 8.10.2)",
            home.display()
        ),
        None => "no JDK 17-23 found (Gradle 8.10.2 needs Java 17-23)".to_string(),
    };
    Check::fail(
        "JDK",
        detail,
        "install JDK 17 (brew install openjdk@17) and export \
         QORVEX_ANDROID_JAVA_HOME=<jdk home>"
            .to_string(),
    )
}

/// Check 4. `dir` is `effective_android_agent_source_dir()`.
pub fn check_agent_project(
    dir: Option<&Path>,
    has_gradlew: bool,
    has_app_apk: bool,
    has_test_apk: bool,
) -> Check {
    const FIX: &str = "set \"android_agent_source_dir\" in ~/.qorvex/config.json to the \
                       android-agent project directory (the one containing gradlew)";
    let Some(dir) = dir else {
        return Check::fail(
            "agent project",
            "no Android agent project configured".to_string(),
            FIX.to_string(),
        );
    };
    if !has_gradlew {
        return Check::fail(
            "agent project",
            format!("{} has no gradlew", dir.display()),
            FIX.to_string(),
        );
    }
    let yn = |b: bool| if b { "present" } else { "not built" };
    Check::ok(
        "agent project",
        format!(
            "{} (app apk: {}, androidTest apk: {})",
            dir.display(),
            yn(has_app_apk),
            yn(has_test_apk)
        ),
    )
}

/// Check 5. `output` is `adb devices -l` stdout, or the first line of adb's
/// stderr when the command failed.
pub fn check_devices(output: Result<&str, &str>) -> Check {
    let text = match output {
        Ok(t) => t,
        Err(e) => {
            return Check::fail(
                "devices",
                format!("`adb devices -l` failed: {e}"),
                "run `adb kill-server && adb start-server` and retry".to_string(),
            )
        }
    };
    let devices = Adb::parse_devices(text);
    if devices.is_empty() {
        return Check::warn(
            "devices",
            "no device attached".to_string(),
            Some("connect a device with USB debugging enabled or boot an emulator".to_string()),
        );
    }
    let list = devices
        .iter()
        .map(|d| {
            format!(
                "{} {} {}",
                d.serial,
                d.state,
                d.model.as_deref().unwrap_or("-")
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    let bad: Vec<_> = devices.iter().filter(|d| !d.is_ready()).collect();
    if bad.is_empty() {
        return Check::ok("devices", list);
    }
    let fixes = bad
        .iter()
        .map(|d| match d.state.as_str() {
            "unauthorized" => format!(
                "{}: accept the USB debugging prompt on the device",
                d.serial
            ),
            "offline" => format!(
                "{}: reconnect the device or run `adb kill-server`",
                d.serial
            ),
            other => format!("{}: state is `{other}`; reconnect the device", d.serial),
        })
        .collect::<Vec<_>>()
        .join("; ");
    Check::fail("devices", list, fixes)
}

/// Check 6.
pub fn check_server(on_path: Option<&Path>) -> Check {
    match on_path {
        Some(p) => Check::ok("server", p.display().to_string()),
        None => Check::fail(
            "server",
            "qorvex-server not found on PATH".to_string(),
            "install qorvex-server from the same build as qorvex".to_string(),
        ),
    }
}

/// First executable named `name` in the `path` list (a PATH-style string).
pub fn find_on_path(name: &str, path: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let path = path?;
    std::env::split_paths(path)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

/// `sdk.dir=...` from `local.properties` text.
pub fn parse_sdk_dir(local_properties: &str) -> Option<String> {
    local_properties.lines().find_map(|l| {
        let (k, v) = l.trim().split_once('=')?;
        (k.trim() == "sdk.dir").then(|| v.trim().to_string())
    })
}

fn first_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string()
}

/// Gather inputs from the machine and run every check.
pub fn run() -> Vec<Check> {
    let path_env = std::env::var_os("PATH");
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let env_str = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let config = QorvexConfig::load();
    let default_sdk = home
        .as_ref()
        .map(|h| h.join("Library/Android/sdk"))
        .filter(|p| p.exists());

    // 1. adb
    let adb_path = find_on_path("adb", path_env.as_deref());
    let version = adb_path.as_ref().and_then(|p| {
        let out = Command::new(p).arg("version").output().ok()?;
        Some(first_line(&out.stdout)).filter(|l| !l.is_empty())
    });
    let mut sdk_candidates: Vec<PathBuf> = Vec::new();
    sdk_candidates.extend(env_str("ANDROID_HOME").map(PathBuf::from));
    sdk_candidates.extend(env_str("ANDROID_SDK_ROOT").map(PathBuf::from));
    sdk_candidates.extend(config.android_sdk_root.clone());
    sdk_candidates.extend(default_sdk.clone());
    let fallbacks: Vec<PathBuf> = sdk_candidates
        .into_iter()
        .map(|s| s.join("platform-tools/adb"))
        .filter(|p| p.is_file())
        .collect();
    let adb = check_adb(adb_path.as_deref(), version.as_deref(), &fallbacks);
    let adb_ok = adb.status == Status::Ok;

    let agent_dir = config.effective_android_agent_source_dir();

    // 2. ANDROID_HOME
    let local_sdk = agent_dir
        .as_ref()
        .and_then(|d| std::fs::read_to_string(d.join("local.properties")).ok())
        .and_then(|t| parse_sdk_dir(&t));
    let android_home = check_android_home(
        env_str("ANDROID_HOME").as_deref(),
        env_str("ANDROID_SDK_ROOT").as_deref(),
        config.android_sdk_root.as_deref(),
        local_sdk.as_deref(),
        default_sdk.as_deref(),
        &|p| p.exists(),
    );

    // 3. JDK (same resolution as start-agent)
    let override_home = client_java_home_override().map(PathBuf::from);
    let resolved = resolve_build_java_home(override_home.as_deref())
        .and_then(|h| java_major_version(&h).map(|m| (h, m)));
    let ambient = override_home
        .as_ref()
        .map(|h| (h.as_path(), java_major_version(h)));
    let jdk = check_jdk(resolved.as_ref().map(|(h, m)| (h.as_path(), *m)), ambient);

    // 4. agent project
    let project = check_agent_project(
        agent_dir.as_deref(),
        agent_dir
            .as_ref()
            .is_some_and(|d| d.join("gradlew").is_file()),
        agent_dir.as_ref().is_some_and(|d| {
            d.join("build/outputs/apk/debug/qorvex-agent-android-debug.apk")
                .is_file()
        }),
        agent_dir.as_ref().is_some_and(|d| {
            d.join("build/outputs/apk/androidTest/debug/qorvex-agent-android-debug-androidTest.apk")
                .is_file()
        }),
    );

    // 5. devices
    let devices = match (adb_ok, adb_path.as_ref()) {
        (true, Some(p)) => match Command::new(p).args(["devices", "-l"]).output() {
            Ok(out) if out.status.success() => {
                check_devices(Ok(&String::from_utf8_lossy(&out.stdout)))
            }
            Ok(out) => check_devices(Err(&first_line(&out.stderr))),
            Err(e) => check_devices(Err(&e.to_string())),
        },
        _ => Check::skip("devices"),
    };

    // 6. server
    let server = check_server(find_on_path("qorvex-server", path_env.as_deref()).as_deref());

    vec![adb, android_home, jdk, project, devices, server]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> &Path {
        Path::new(s)
    }

    #[test]
    fn adb_ok_on_path() {
        let c = check_adb(Some(p("/x/adb")), Some("Android Debug Bridge 1.0.41"), &[]);
        assert_eq!(c.status, Status::Ok);
        assert!(c.line().starts_with("[ok] adb: /x/adb"));
        assert!(c.line().contains("1.0.41"));
    }

    #[test]
    fn adb_fail_names_platform_tools_dir() {
        let c = check_adb(None, None, &[PathBuf::from("/sdk/platform-tools/adb")]);
        assert_eq!(c.status, Status::Fail);
        assert!(c.line().starts_with("[FAIL] adb:"));
        assert!(c.fix.unwrap().contains("/sdk/platform-tools:"));
    }

    #[test]
    fn adb_fail_without_sdk() {
        let c = check_adb(None, None, &[]);
        assert_eq!(c.status, Status::Fail);
        assert!(c.fix.unwrap().contains("platform-tools"));
    }

    #[test]
    fn android_home_ok_from_env() {
        let c = check_android_home(Some("/sdk"), None, None, None, None, &|_| true);
        assert_eq!(c.status, Status::Ok);
    }

    #[test]
    fn android_home_ok_from_each_fallback() {
        assert_eq!(
            check_android_home(None, Some("/s"), None, None, None, &|_| true).status,
            Status::Ok
        );
        assert_eq!(
            check_android_home(None, None, Some(p("/s")), None, None, &|_| true).status,
            Status::Ok
        );
        assert_eq!(
            check_android_home(None, None, None, Some("/s"), None, &|_| true).status,
            Status::Ok
        );
    }

    #[test]
    fn android_home_nonexistent_dir_fails() {
        let c = check_android_home(Some("/nope"), None, None, None, None, &|_| false);
        assert_eq!(c.status, Status::Fail);
        assert!(c.detail.contains("does not exist"));
        assert!(c.detail.contains("/nope"));
        assert!(c.fix.unwrap().contains("point ANDROID_HOME at"));
    }

    #[test]
    fn stale_local_properties_fix_names_sdk_dir() {
        let c = check_android_home(None, None, None, Some("/gone"), Some(p("/d/sdk")), &|q| {
            q == Path::new("/d/sdk")
        });
        assert_eq!(c.status, Status::Fail);
        let fix = c.fix.unwrap();
        assert!(fix.contains("sdk.dir in local.properties"));
        assert!(fix.contains("export ANDROID_HOME=\"/d/sdk\""));
    }

    #[test]
    fn android_home_unset_with_default_sdk_fails_with_export() {
        let c = check_android_home(None, None, None, None, Some(p("/d/sdk")), &|_| true);
        assert_eq!(c.status, Status::Fail);
        assert!(c.fix.unwrap().contains("export ANDROID_HOME=\"/d/sdk\""));
    }

    #[test]
    fn android_home_unset_without_sdk_warns() {
        let c = check_android_home(None, None, None, None, None, &|_| false);
        assert_eq!(c.status, Status::Warn);
        assert!(c.line().starts_with("[warn] ANDROID_HOME:"));
    }

    #[test]
    fn android_home_empty_env_is_unset() {
        let c = check_android_home(Some(""), None, None, None, None, &|_| false);
        assert_eq!(c.status, Status::Warn);
    }

    #[test]
    fn jdk_ok() {
        let c = check_jdk(Some((p("/jdk"), 17)), None);
        assert_eq!(c.status, Status::Ok);
        assert!(c.detail.contains("Java 17"));
    }

    #[test]
    fn jdk_too_new_prints_version() {
        let c = check_jdk(None, Some((p("/jdk24"), Some(24))));
        assert_eq!(c.status, Status::Fail);
        assert!(c.detail.contains("Java 24"));
        assert!(c.fix.unwrap().contains("QORVEX_ANDROID_JAVA_HOME"));
    }

    #[test]
    fn jdk_too_old_prints_version() {
        let c = check_jdk(None, Some((p("/jdk16"), Some(16))));
        assert_eq!(c.status, Status::Fail);
        assert!(c.detail.contains("Java 16"));
    }

    #[test]
    fn jdk_none_found_and_unreadable() {
        assert_eq!(check_jdk(None, None).status, Status::Fail);
        let c = check_jdk(None, Some((p("/bad"), None)));
        assert_eq!(c.status, Status::Fail);
        assert!(c.detail.contains("/bad"));
    }

    #[test]
    fn agent_project_ok_reports_apks() {
        let c = check_agent_project(Some(p("/a")), true, true, false);
        assert_eq!(c.status, Status::Ok);
        assert!(c.detail.contains("app apk: present"));
        assert!(c.detail.contains("androidTest apk: not built"));
    }

    #[test]
    fn agent_project_missing_or_no_gradlew_fails() {
        for c in [
            check_agent_project(None, false, false, false),
            check_agent_project(Some(p("/a")), false, false, false),
        ] {
            assert_eq!(c.status, Status::Fail);
            assert!(c.fix.unwrap().contains("android_agent_source_dir"));
        }
    }

    const DEV_OK: &str = "List of devices attached\n1WMHH820XM0366\tdevice usb:1-1 product:hollywood model:Quest_2 device:hollywood transport_id:1\n";

    #[test]
    fn devices_ok_lists_each() {
        let c = check_devices(Ok(DEV_OK));
        assert_eq!(c.status, Status::Ok);
        assert!(c.detail.contains("1WMHH820XM0366 device Quest_2"));
    }

    #[test]
    fn devices_multiple_all_listed() {
        let out = "List of devices attached\nA\tdevice model:M1\nemulator-5554\tdevice model:M2\n";
        let c = check_devices(Ok(out));
        assert_eq!(c.status, Status::Ok);
        assert!(c.detail.contains("A device M1") && c.detail.contains("emulator-5554 device M2"));
    }

    #[test]
    fn devices_unauthorized_fails() {
        let c = check_devices(Ok("List of devices attached\nA\tunauthorized usb:1\n"));
        assert_eq!(c.status, Status::Fail);
        assert!(c.fix.unwrap().contains("USB debugging prompt"));
    }

    #[test]
    fn devices_offline_fails() {
        let c = check_devices(Ok("List of devices attached\nA\toffline\n"));
        assert_eq!(c.status, Status::Fail);
        assert!(c.fix.unwrap().contains("adb kill-server"));
    }

    #[test]
    fn devices_none_warns() {
        let c = check_devices(Ok("List of devices attached\n\n"));
        assert_eq!(c.status, Status::Warn);
        assert!(c.detail.contains("no device attached"));
    }

    #[test]
    fn devices_adb_error_fails_with_stderr() {
        let c = check_devices(Err("cannot connect to daemon"));
        assert_eq!(c.status, Status::Fail);
        assert!(c.detail.contains("cannot connect to daemon"));
    }

    #[test]
    fn devices_skip_line() {
        let c = Check::skip("devices");
        assert_eq!(c.status, Status::Skip);
        assert_eq!(c.line(), "[skip] devices: adb not available");
    }

    #[test]
    fn server_ok_and_fail() {
        assert_eq!(
            check_server(Some(p("/bin/qorvex-server"))).status,
            Status::Ok
        );
        let c = check_server(None);
        assert_eq!(c.status, Status::Fail);
        assert!(c.line().starts_with("[FAIL] server:"));
    }

    #[test]
    fn exit_and_json_shape() {
        let checks = vec![check_server(Some(p("/s"))), check_server(None)];
        assert!(!all_ok(&checks));
        let j = to_json(&checks);
        assert_eq!(j["ok"], false);
        assert_eq!(j["checks"][0]["status"], "ok");
        assert_eq!(j["checks"][1]["status"], "fail");
        assert!(j["checks"][1]["fix"].is_string());
        assert!(j["checks"][0]["fix"].is_null());
        // warn and skip do not fail
        assert!(all_ok(&[Check::skip("devices"), check_devices(Ok(""))]));
    }

    #[test]
    fn sdk_dir_parsing() {
        assert_eq!(
            parse_sdk_dir("# c\nsdk.dir = /x/y\n").as_deref(),
            Some("/x/y")
        );
        assert_eq!(parse_sdk_dir("foo=bar"), None);
    }
}

use std::fs;
use std::path::{Path, PathBuf};

use bifrost_cli::cli::CaCommands;
use bifrost_cli::commands::handle_ca_command;

use crate::TestCase;

pub fn get_all_tests() -> Vec<TestCase> {
    vec![
        TestCase::standalone(
            "cli_ca_install_mobile_single_device_fake_adb",
            "Validate CLI mobile CA install auto-selects one ready Android device",
            "mobile_device",
            || async move {
                let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
                let data_dir = temp.path().join("data");
                let log_path = temp.path().join("fake-adb.log");
                let adb_path = write_fake_adb(
                    temp.path(),
                    &log_path,
                    "List of devices attached\nandroid-1 device product:test model:Pixel_Test device:test transport_id:1\n",
                )?;

                let _data_guard = EnvGuard::set("BIFROST_DATA_DIR", data_dir.display().to_string());
                let _adb_guard = EnvGuard::set("BIFROST_ADB_PATH", adb_path.display().to_string());

                handle_ca_command(CaCommands::Install {
                    mobile: true,
                    ios: false,
                    configurator: false,
                    device: None,
                    yes: true,
                })
                .map_err(|error| error.to_string())?;

                let log = fs::read_to_string(&log_path)
                    .map_err(|error| format!("read fake adb log: {error}"))?;
                if !log.contains("devices -l") {
                    return Err(format!("fake adb log missing devices call: {log}"));
                }
                if !log.contains("-s android-1 push") {
                    return Err(format!("fake adb log missing push call: {log}"));
                }
                if !log.contains("android.intent.action.VIEW") {
                    return Err(format!("fake adb log missing installer VIEW intent: {log}"));
                }
                Ok(())
            },
        ),
        TestCase::standalone(
            "cli_ca_install_mobile_multiple_devices_requires_device_fake_adb",
            "Validate CLI mobile CA install requires --device with multiple ready Android devices in --yes mode",
            "mobile_device",
            || async move {
                let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
                let data_dir = temp.path().join("data");
                let log_path = temp.path().join("fake-adb.log");
                let adb_path = write_fake_adb(
                    temp.path(),
                    &log_path,
                    "List of devices attached\nandroid-1 device product:test model:Pixel_One device:test transport_id:1\nandroid-2 device product:test model:Pixel_Two device:test transport_id:2\n",
                )?;

                let _data_guard = EnvGuard::set("BIFROST_DATA_DIR", data_dir.display().to_string());
                let _adb_guard = EnvGuard::set("BIFROST_ADB_PATH", adb_path.display().to_string());

                let result = handle_ca_command(CaCommands::Install {
                    mobile: true,
                    ios: false,
                    configurator: false,
                    device: None,
                    yes: true,
                });
                let err = result.expect_err("multiple devices should require --device");
                let message = err.to_string();
                if !message.contains("Multiple ready Android devices detected")
                    || !message.contains("--device <serial>")
                {
                    return Err(format!("unexpected error message: {message}"));
                }
                Ok(())
            },
        ),
    ]
}

fn write_fake_adb(dir: &Path, log_path: &Path, devices_output: &str) -> Result<PathBuf, String> {
    let adb_path = dir.join(fake_adb_name());
    let script = fake_adb_script(log_path, devices_output);
    fs::write(&adb_path, script).map_err(|error| format!("write fake adb: {error}"))?;
    make_executable(&adb_path)?;
    Ok(adb_path)
}

fn fake_adb_name() -> &'static str {
    if cfg!(windows) {
        "fake-adb.cmd"
    } else {
        "fake-adb"
    }
}

fn fake_adb_script(log_path: &Path, devices_output: &str) -> String {
    if cfg!(windows) {
        format!(
            "@echo off\r\necho %*>>\"{}\"\r\nif \"%1\"==\"devices\" (\r\n{}\r\n)\r\nexit /b 0\r\n",
            log_path.display(),
            windows_echo_lines(devices_output)
        )
    } else {
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nif [ \"$1\" = \"devices\" ]; then\ncat <<'ADB_DEVICES'\n{}ADB_DEVICES\nfi\nexit 0\n",
            escape_single_quotes(&log_path.display().to_string()),
            devices_output
        )
    }
}

fn windows_echo_lines(output: &str) -> String {
    output
        .lines()
        .map(|line| format!("echo {line}"))
        .collect::<Vec<_>>()
        .join("\r\n")
}

fn escape_single_quotes(value: &str) -> String {
    value.replace('\'', "'\\''")
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)
        .map_err(|error| format!("stat fake adb: {error}"))?
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).map_err(|error| format!("chmod fake adb: {error}"))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

struct EnvGuard {
    key: &'static str,
    old_value: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, value: String) -> Self {
        let old_value = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self { key, old_value }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.old_value {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
}

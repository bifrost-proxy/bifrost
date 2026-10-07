//! Bounded process adapter and strict parsers for networksetup.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]
use std::io::{Read, Seek, SeekFrom};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use super::macos_owned::{Backend, Field, Operation, Protocol, Service, Value};
use crate::{BifrostError, Result};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(any(not(bifrost_proxy_test_io), test))]
const AUTH_TIMEOUT: Duration = Duration::from_secs(120);
const OUTPUT_LIMIT: u64 = 64 * 1024;

#[derive(Debug, Clone, Copy)]
pub(super) enum Privilege {
    Direct,
    Gui,
    Sudo,
}

type CommandRunner = fn(&str, &[&str], Duration) -> Result<Output>;

// The runner is the OS boundary. Tests inject recorded process responses while
// exercising the same query, command construction and validation paths.
pub(super) struct NetworkSetup<R = CommandRunner> {
    pub privilege: Privilege,
    runner: R,
}

impl NetworkSetup {
    pub(super) fn new(privilege: Privilege) -> Self {
        Self {
            privilege,
            runner: run_bounded,
        }
    }
}

pub(super) fn run_bounded(program: &str, args: &[&str], timeout: Duration) -> Result<Output> {
    #[cfg(test)]
    if matches!(
        program,
        "/usr/sbin/networksetup" | "/usr/sbin/scutil" | "/usr/bin/osascript" | "/usr/bin/sudo"
    ) {
        return Err(BifrostError::Config(
            "Native macOS proxy processes are forbidden in unit tests; inject a backend".into(),
        ));
    }
    #[cfg(all(bifrost_proxy_test_io, not(test)))]
    let fixture_command = super::macos_test_io::Fixture::from_env()?.command(program)?;
    #[cfg(all(bifrost_proxy_test_io, not(test)))]
    let program = fixture_command.to_str().ok_or_else(|| {
        BifrostError::Config(
            "TestProxyIoUnavailable: fixture command is not UTF-8; refusing native proxy I/O"
                .into(),
        )
    })?;
    // Files avoid a pipe-buffer deadlock without unjoinable reader threads.
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    let mut command = Command::new(program);
    command
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(stdout.try_clone()?)
        .stderr(stderr.try_clone()?);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|error| BifrostError::Config(format!("Failed to execute {program}: {error}")))?;
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() >= timeout {
            #[cfg(unix)]
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
            return Err(BifrostError::Config(format!(
                "networksetup command timed out after {}ms: {program}",
                timeout.as_millis()
            )));
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    stdout.seek(SeekFrom::Start(0))?;
    stderr.seek(SeekFrom::Start(0))?;
    let mut out = Vec::new();
    let mut err = Vec::new();
    stdout.take(OUTPUT_LIMIT).read_to_end(&mut out)?;
    stderr.take(OUTPUT_LIMIT).read_to_end(&mut err)?;
    let output = Output {
        status,
        stdout: out,
        stderr: err,
    };
    validate_output(&output)?;
    Ok(output)
}

fn validate_output(output: &Output) -> Result<()> {
    let message = format!(
        "{} {}",
        String::from_utf8_lossy(&output.stdout).trim(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let lower = message.to_ascii_lowercase();
    if lower.contains("user canceled")
        || lower.contains("user cancelled")
        || lower.contains("(-128)")
    {
        return Err(BifrostError::Config(
            "UserCancelled: User cancelled authorization".into(),
        ));
    }
    let permission = lower.contains("administrator")
        || lower.contains("not authorized")
        || lower.contains("permission denied")
        || lower.contains("must be root")
        || lower.contains("password is required");
    // networksetup is known to print ** Error while exiting successfully.
    if permission
        || !output.status.success()
        || lower.contains("** error")
        || lower.contains("error:")
    {
        return Err(BifrostError::Config(format!(
            "{}networksetup failed (code {:?}): {}",
            if permission { "RequiresAdmin: " } else { "" },
            output.status.code(),
            message.trim()
        )));
    }
    Ok(())
}

#[cfg(any(not(bifrost_proxy_test_io), test))]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

impl<R: Fn(&str, &[&str], Duration) -> Result<Output>> NetworkSetup<R> {
    fn query(&self, args: &[&str]) -> Result<String> {
        let output = (self.runner)("/usr/sbin/networksetup", args, COMMAND_TIMEOUT)?;
        String::from_utf8(output.stdout).map_err(|error| {
            BifrostError::Config(format!("networksetup output is not UTF-8: {error}"))
        })
    }

    fn mutate(&self, args: &[&str]) -> Result<()> {
        #[cfg(all(bifrost_proxy_test_io, not(test)))]
        {
            // A test binary never invokes native GUI/sudo executables. All
            // privilege modes use the same explicitly configured fake writer.
            (self.runner)("/usr/sbin/networksetup", args, COMMAND_TIMEOUT)?;
            Ok(())
        }
        #[cfg(any(not(bifrost_proxy_test_io), test))]
        {
            match self.privilege {
                Privilege::Direct => {
                    (self.runner)("/usr/sbin/networksetup", args, COMMAND_TIMEOUT)?;
                }
                Privilege::Sudo => {
                    let mut command = vec!["-n", "/usr/sbin/networksetup"];
                    command.extend_from_slice(args);
                    (self.runner)("/usr/bin/sudo", &command, COMMAND_TIMEOUT)?;
                }
                Privilege::Gui => {
                    let command = format!(
                        "/usr/sbin/networksetup {}",
                        args.iter()
                            .map(|value| shell_quote(value))
                            .collect::<Vec<_>>()
                            .join(" ")
                    );
                    let escaped = command.replace('\\', "\\\\").replace('"', "\\\"");
                    let script =
                        format!("do shell script \"{escaped}\" with administrator privileges");
                    (self.runner)("/usr/bin/osascript", &["-e", &script], AUTH_TIMEOUT)?;
                }
            }
            Ok(())
        }
    }
}

impl<R: Fn(&str, &[&str], Duration) -> Result<Output>> Backend for NetworkSetup<R> {
    fn requires_authorization(&self) -> bool {
        !matches!(self.privilege, Privilege::Direct)
    }
    fn services(&mut self) -> Result<Vec<Service>> {
        parse_services(&self.query(&["-listallnetworkservices"])?)
    }
    fn read(&mut self, service: &str, field: Field) -> Result<Value> {
        match field {
            Field::Http => {
                parse_protocol(&self.query(&["-getwebproxy", service])?).map(Value::Protocol)
            }
            Field::Https => {
                parse_protocol(&self.query(&["-getsecurewebproxy", service])?).map(Value::Protocol)
            }
            Field::Bypass => {
                parse_bypass(&self.query(&["-getproxybypassdomains", service])?).map(Value::Bypass)
            }
        }
    }
    fn write(&mut self, service: &str, operation: &Operation) -> Result<()> {
        match operation {
            Operation::Endpoint { field, host, port } => {
                let command = match field {
                    Field::Http => "-setwebproxy",
                    Field::Https => "-setsecurewebproxy",
                    Field::Bypass => {
                        return Err(BifrostError::Config("invalid proxy endpoint field".into()))
                    }
                };
                self.mutate(&[command, service, host, &port.to_string(), "off"])
            }
            Operation::Enabled { field, enabled } => {
                let command = match field {
                    Field::Http => "-setwebproxystate",
                    Field::Https => "-setsecurewebproxystate",
                    Field::Bypass => {
                        return Err(BifrostError::Config("invalid proxy enable field".into()))
                    }
                };
                self.mutate(&[command, service, if *enabled { "on" } else { "off" }])
            }
            Operation::Bypass(domains) => {
                let mut args = vec!["-setproxybypassdomains", service];
                if domains.is_empty() {
                    args.push("Empty");
                } else {
                    args.extend(domains.iter().map(String::as_str));
                }
                self.mutate(&args)
            }
        }
    }
}

pub(super) fn parse_services(text: &str) -> Result<Vec<Service>> {
    let services: Vec<_> = text
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with("An asterisk (") {
                return None;
            }
            let enabled = !line.starts_with('*');
            Some(Service {
                name: line.strip_prefix('*').unwrap_or(line).into(),
                enabled,
            })
        })
        .collect();
    if services.is_empty() {
        return Err(BifrostError::Config(
            "No enabled macOS network services were returned by networksetup".into(),
        ));
    }
    Ok(services)
}

pub(super) fn parse_protocol(text: &str) -> Result<Protocol> {
    let mut enabled = None;
    let mut host = None;
    let mut port = None;
    let mut authenticated = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "Enabled" => {
                enabled = match value.to_ascii_lowercase().as_str() {
                    "yes" => Some(true),
                    "no" => Some(false),
                    _ => None,
                }
            }
            "Server" => host = Some(value.to_owned()),
            "Port" => port = value.parse::<u16>().ok(),
            "Authenticated Proxy Enabled" => {
                authenticated = match value {
                    "1" | "Yes" => Some(true),
                    "0" | "No" => Some(false),
                    _ => None,
                }
            }
            _ => {}
        }
    }
    match (enabled, host, port, authenticated) {
        (Some(enabled), Some(host), Some(port), Some(authenticated)) => Ok(Protocol {
            enabled,
            host,
            port,
            authenticated,
        }),
        _ => Err(BifrostError::Config(
            "networksetup returned incomplete proxy settings; refusing to assume disabled".into(),
        )),
    }
}

pub(super) fn parse_bypass(text: &str) -> Result<Vec<String>> {
    let text = text.trim();
    if text.starts_with("There aren't any bypass domains set on ") {
        return Ok(Vec::new());
    }
    if text.is_empty() {
        return Err(BifrostError::Config(
            "networksetup returned empty bypass output".into(),
        ));
    }
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect())
}

/// Parse only ExceptionsList array entries; numeric entries in other scutil
/// arrays are not bypass domains. Prefer an enabled protocol's endpoint.
pub(super) fn parse_scutil_proxy(text: &str) -> Option<super::ProxyBackup> {
    if !text.contains("<dictionary>") {
        return None;
    }
    let mut protocols = [
        (false, String::new(), 0u16),
        (false, String::new(), 0u16),
        (false, String::new(), 0u16),
    ];
    let mut bypass = Vec::new();
    let mut exceptions = false;
    for line in text.lines().map(str::trim) {
        if line == "}" {
            exceptions = false;
            continue;
        }
        let Some((key, value)) = line.split_once(" : ") else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if key == "ExceptionsList" {
            exceptions = value.starts_with("<array>");
            continue;
        }
        if exceptions && !key.is_empty() && key.chars().all(|c| c.is_ascii_digit()) {
            bypass.push(value.to_owned());
            continue;
        }
        for (index, prefix) in ["HTTP", "HTTPS", "SOCKS"].into_iter().enumerate() {
            if key == format!("{prefix}Enable") {
                protocols[index].0 = value == "1";
            }
            if key == format!("{prefix}Proxy") {
                protocols[index].1 = value.into();
            }
            if key == format!("{prefix}Port") {
                protocols[index].2 = value.parse().ok()?;
            }
        }
    }
    let proxy = protocols
        .iter()
        .find(|proxy| proxy.0)
        .or_else(|| {
            protocols
                .iter()
                .find(|proxy| !proxy.1.is_empty() && proxy.2 != 0)
        })
        .unwrap_or(&protocols[0]);
    Some(super::ProxyBackup {
        enable: proxy.0,
        host: proxy.1.clone(),
        port: proxy.2,
        bypass: bypass.join(","),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disabled_services_remain_visible_for_cleanup() {
        assert_eq!(
            parse_services(
                "An asterisk (*) denotes that a network service is disabled.\nWi-Fi\n*USB LAN\n"
            )
            .unwrap(),
            vec![
                Service {
                    name: "Wi-Fi".into(),
                    enabled: true
                },
                Service {
                    name: "USB LAN".into(),
                    enabled: false
                }
            ]
        );
        assert!(parse_services("").is_err());
    }
    #[test]
    fn strict_proxy_and_bypass_parsers_preserve_heterogeneous_values() {
        let proxy = parse_protocol(
            "Enabled: No\nServer: ::1\nPort: 1234\nAuthenticated Proxy Enabled: 0\n",
        )
        .unwrap();
        assert!(!proxy.enabled);
        assert_eq!(proxy.host, "::1");
        assert!(parse_protocol("Enabled: No\n").is_err());
        assert_eq!(
            parse_bypass("*.corp\nlocalhost\n10.0.0.0/8\n").unwrap(),
            vec!["*.corp", "localhost", "10.0.0.0/8"]
        );
        assert!(
            parse_bypass("There aren't any bypass domains set on Wi-Fi.")
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn scutil_bypass_parser_reads_exception_array_and_enabled_protocol() {
        let proxy = parse_scutil_proxy("<dictionary> {\n HTTPEnable : 0\n HTTPProxy : dormant\n HTTPPort : 8080\n HTTPSEnable : 1\n HTTPSProxy : secure-corp\n HTTPSPort : 8443\n ExceptionsList : <array> {\n 0 : localhost\n 1 : *.corp\n }\n SupplementalMatchDomains : <array> {\n 0 : not-a-bypass\n }\n}").unwrap();
        assert!(proxy.enable);
        assert_eq!(proxy.host, "secure-corp");
        assert_eq!(proxy.bypass, "localhost,*.corp");
        assert!(parse_scutil_proxy("unreadable settings").is_none());
    }
    #[test]
    fn shell_arguments_cannot_inject_commands() {
        assert_eq!(
            shell_quote("Wi-Fi'; touch /tmp/no"),
            "'Wi-Fi'\\''; touch /tmp/no'"
        );
    }
    #[cfg(unix)]
    #[test]
    fn command_deadline_and_zero_exit_errors_are_enforced() {
        let started = Instant::now();
        assert!(
            run_bounded("/bin/sh", &["-c", "sleep 10"], Duration::from_millis(30))
                .unwrap_err()
                .to_string()
                .contains("timed out")
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(run_bounded(
            "/bin/sh",
            &["-c", "printf '** Error: configuration failed\\n'"],
            COMMAND_TIMEOUT
        )
        .is_err());
        assert!(run_bounded(
            "/bin/sh",
            &["-c", "printf 'You must be an administrator\\n'"],
            COMMAND_TIMEOUT
        )
        .unwrap_err()
        .to_string()
        .contains("RequiresAdmin"));
    }
}

#[cfg(test)]
#[cfg(unix)]
mod adapter_tests;

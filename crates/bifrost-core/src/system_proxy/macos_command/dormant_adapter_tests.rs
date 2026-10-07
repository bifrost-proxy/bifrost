use super::*;
use crate::system_proxy::macos_preferences::{Request, HELPER_ARGUMENT};
use std::cell::RefCell;
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
#[cfg(windows)]
use std::os::windows::process::ExitStatusExt;

fn dormant() -> Operation {
    Operation::DormantEndpoint {
        field: Field::Http,
        expected: Protocol {
            enabled: false,
            host: "127.0.0.1".into(),
            port: 18880,
            authenticated: false,
        },
        desired: Protocol {
            enabled: false,
            host: String::new(),
            port: 0,
            authenticated: false,
        },
    }
}

#[test]
fn only_direct_uses_bounded_existing_executable_and_elevation_fails_closed() {
    for privilege in [Privilege::Direct, Privilege::Sudo, Privilege::Gui] {
        let calls = RefCell::new(Vec::new());
        let runner = |program: &str, args: &[&str], timeout| {
            calls.borrow_mut().push((
                program.to_owned(),
                args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                timeout,
            ));
            Ok(Output {
                status: std::process::ExitStatus::from_raw(0),
                stdout: b"\"applied\"\n".to_vec(),
                stderr: Vec::new(),
            })
        };
        let mut os = NetworkSetup { privilege, runner };
        if !matches!(privilege, Privilege::Direct) {
            assert!(!os.supports_dormant_restore());
            assert!(os
                .write("Wi-Fi", &dormant())
                .unwrap_err()
                .to_string()
                .contains("Unsupported"));
            assert!(calls.borrow().is_empty());
            continue;
        }
        os.write("O'Reilly \"USB\"\\LAN", &dormant()).unwrap();
        let calls = calls.borrow();
        assert_eq!(calls.len(), 1);
        let (program, args, timeout) = &calls[0];
        match privilege {
            Privilege::Direct | Privilege::Sudo => {
                let index = usize::from(matches!(privilege, Privilege::Sudo)) * 2;
                assert_eq!(args[index], HELPER_ARGUMENT);
                let request: Request = serde_json::from_str(&args[index + 1]).unwrap();
                assert_eq!(request.service, "O'Reilly \"USB\"\\LAN");
                assert!(
                    request.deadline_millis
                        > super::super::macos_preferences::monotonic_millis().unwrap()
                );
                assert_eq!(*timeout, COMMAND_TIMEOUT);
                if matches!(privilege, Privilege::Sudo) {
                    assert_eq!(program, "/usr/bin/sudo");
                    assert_eq!(args[0], "-n");
                } else {
                    assert_eq!(program, std::env::current_exe().unwrap().to_str().unwrap());
                }
            }
            Privilege::Gui => {
                assert_eq!(program, "/usr/bin/osascript");
                assert_eq!(*timeout, AUTH_TIMEOUT);
                assert!(args[1].contains(HELPER_ARGUMENT));
                assert!(args[1].contains("with administrator privileges"));
                assert!(args[1].contains("O'\\\\''Reilly"));
            }
        }
    }
}

#[test]
fn helper_conflicts_malformed_output_and_native_test_execution_fail_closed() {
    for (output, expected) in [
        ("\"ownership_changed\"", "ProxyOwnershipChanged:"),
        ("success", "Invalid proxy helper result"),
    ] {
        let runner = |_: &str, _: &[&str], _: Duration| {
            Ok(Output {
                status: std::process::ExitStatus::from_raw(0),
                stdout: output.as_bytes().to_vec(),
                stderr: Vec::new(),
            })
        };
        let mut os = NetworkSetup {
            privilege: Privilege::Direct,
            runner,
        };
        assert!(os
            .write("Wi-Fi", &dormant())
            .unwrap_err()
            .to_string()
            .contains(expected));
    }
    // The fixed-path denylist is insufficient for self-exec; exact helper mode
    // is denied at the process runner as well as the helper's cfg-gated entry.
    assert!(run_bounded(
        "never-executed-self-binary",
        &[HELPER_ARGUMENT, "{}"],
        COMMAND_TIMEOUT
    )
    .unwrap_err()
    .to_string()
    .contains("forbidden"));
}

#[cfg(unix)]
#[test]
fn bounded_child_drop_and_unwind_reap_before_final_lock_reference_releases() {
    use fs2::FileExt;
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    for unwind in [false, true] {
        let file = tempfile::NamedTempFile::new().unwrap();
        let owner = std::fs::File::options()
            .read(true)
            .write(true)
            .open(file.path())
            .unwrap();
        owner.lock_exclusive().unwrap();
        let contender = std::fs::File::options()
            .read(true)
            .write(true)
            .open(file.path())
            .unwrap();
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "printf ready; exec sleep 10"])
            .stdin(Stdio::from(owner.try_clone().unwrap()))
            .stdout(Stdio::piped())
            .process_group(0);
        let mut child = BoundedChild::new(command.spawn().unwrap());
        drop(command);
        let mut ready = [0; 5];
        child
            .child
            .stdout
            .take()
            .unwrap()
            .read_exact(&mut ready)
            .unwrap();
        drop(owner);
        assert!(contender.try_lock_exclusive().is_err());
        let started = Instant::now();
        if unwind {
            let caught = std::panic::catch_unwind(move || {
                let _child = child;
                panic!("simulated supervisor unwind");
            });
            assert!(caught.is_err());
        } else {
            drop(child);
        }
        assert!(started.elapsed() < Duration::from_secs(2));
        let released_by = Instant::now() + Duration::from_secs(2);
        while contender.try_lock_exclusive().is_err() {
            assert!(Instant::now() < released_by);
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

#[cfg(unix)]
#[test]
fn poll_error_and_timeout_reap_the_child_and_release_inherited_lock() {
    use fs2::FileExt;
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    for fail_poll in [false, true] {
        let file = tempfile::NamedTempFile::new().unwrap();
        let owner = std::fs::File::options()
            .read(true)
            .write(true)
            .open(file.path())
            .unwrap();
        owner.lock_exclusive().unwrap();
        let contender = std::fs::File::options()
            .read(true)
            .write(true)
            .open(file.path())
            .unwrap();
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "printf ready; exec sleep 10"])
            .stdin(Stdio::from(owner.try_clone().unwrap()))
            .stdout(Stdio::piped())
            .process_group(0);
        let mut child = BoundedChild::new(command.spawn().unwrap());
        drop(command);
        let mut ready = [0; 5];
        child
            .child
            .stdout
            .take()
            .unwrap()
            .read_exact(&mut ready)
            .unwrap();
        drop(owner);
        assert!(contender.try_lock_exclusive().is_err());
        let started = Instant::now();
        let result = wait_child(child, "test child", Duration::from_millis(30), |child| {
            if fail_poll {
                Err(std::io::Error::other("injected try_wait failure"))
            } else {
                child.try_wait()
            }
        });
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
        let released_by = Instant::now() + Duration::from_secs(2);
        while contender.try_lock_exclusive().is_err() {
            assert!(Instant::now() < released_by);
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

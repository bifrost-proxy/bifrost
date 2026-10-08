//! Test the real adapter with recorded process responses, never networksetup.
use super::*;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::os::unix::process::ExitStatusExt;
use std::rc::Rc;

#[derive(Debug, PartialEq, Eq)]
struct Invocation {
    program: String,
    args: Vec<String>,
    timeout: Duration,
}
type Calls = Rc<RefCell<Vec<Invocation>>>;
type MockRunner = Box<dyn Fn(&str, &[&str], Duration) -> Result<Output>>;

enum Reply {
    Success(Vec<u8>),
    Failure(i32, &'static str),
    Timeout,
}

fn adapter(privilege: Privilege, replies: Vec<Reply>) -> (NetworkSetup<MockRunner>, Calls) {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let recorded = calls.clone();
    let replies = RefCell::new(VecDeque::from(replies));
    let runner: MockRunner = Box::new(move |program, args, timeout| {
        recorded.borrow_mut().push(Invocation {
            program: program.into(),
            args: args.iter().map(|arg| (*arg).into()).collect(),
            timeout,
        });
        let output = match replies
            .borrow_mut()
            .pop_front()
            .expect("unexpected command")
        {
            Reply::Success(stdout) => Output {
                status: std::process::ExitStatus::from_raw(0),
                stdout,
                stderr: Vec::new(),
            },
            Reply::Failure(code, message) => Output {
                status: std::process::ExitStatus::from_raw(code << 8),
                stdout: Vec::new(),
                stderr: message.as_bytes().to_vec(),
            },
            Reply::Timeout => {
                return Err(BifrostError::Config(
                    "networksetup command timed out after 10000ms".into(),
                ))
            }
        };
        // The fake process runner uses the production error classifier too.
        validate_output(&output)?;
        Ok(output)
    });
    (NetworkSetup { privilege, runner }, calls)
}

fn success(text: &str) -> Reply {
    Reply::Success(text.as_bytes().to_vec())
}
fn protocol_text(enabled: &str) -> String {
    format!("Enabled: {enabled}\nServer: corp-proxy\nPort: 8443\nAuthenticated Proxy Enabled: 0\n")
}

#[test]
fn queries_use_direct_read_commands_even_for_an_elevated_writer() {
    let (mut os, calls) = adapter(
        Privilege::Gui,
        vec![
            success("An asterisk (*) denotes a disabled service.\nWi-Fi\n*Ethernet\n"),
            success(&protocol_text("Yes")),
            success(&protocol_text("No")),
            success("*.corp\nlocalhost\n"),
        ],
    );
    assert!(os.requires_authorization());
    assert_eq!(os.services().unwrap().len(), 2);
    assert!(matches!(
        os.read("Wi-Fi", Field::Http).unwrap(),
        Value::Protocol(Protocol { enabled: true, .. })
    ));
    assert!(matches!(
        os.read("Wi-Fi", Field::Https).unwrap(),
        Value::Protocol(Protocol { enabled: false, .. })
    ));
    assert_eq!(
        os.read("Wi-Fi", Field::Bypass).unwrap(),
        Value::Bypass(vec!["*.corp".into(), "localhost".into()])
    );
    let commands = calls.borrow();
    assert_eq!(
        commands
            .iter()
            .map(|call| call.args[0].as_str())
            .collect::<Vec<_>>(),
        vec![
            "-listallnetworkservices",
            "-getwebproxy",
            "-getsecurewebproxy",
            "-getproxybypassdomains"
        ]
    );
    assert!(commands
        .iter()
        .all(|call| call.program == "/usr/sbin/networksetup" && call.timeout == COMMAND_TIMEOUT));
}

#[test]
fn direct_mutations_map_protocol_state_and_bypass_without_extra_commands() {
    let (mut os, calls) = adapter(Privilege::Direct, (0..6).map(|_| success("")).collect());
    assert!(!os.requires_authorization());
    for field in [Field::Http, Field::Https] {
        os.write(
            "USB LAN",
            &Operation::Endpoint {
                field,
                host: "127.0.0.1".into(),
                port: 18885,
            },
        )
        .unwrap();
        os.write(
            "USB LAN",
            &Operation::Enabled {
                field,
                enabled: field == Field::Http,
            },
        )
        .unwrap();
    }
    os.write("USB LAN", &Operation::Bypass(Vec::new())).unwrap();
    os.write(
        "USB LAN",
        &Operation::Bypass(vec!["*.corp".into(), "localhost".into()]),
    )
    .unwrap();
    let commands = calls.borrow();
    let args = commands
        .iter()
        .map(|call| call.args.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        args,
        vec![
            vec!["-setwebproxy", "USB LAN", "127.0.0.1", "18885", "off"],
            vec!["-setwebproxystate", "USB LAN", "on"],
            vec!["-setsecurewebproxy", "USB LAN", "127.0.0.1", "18885", "off"],
            vec!["-setsecurewebproxystate", "USB LAN", "off"],
            vec!["-setproxybypassdomains", "USB LAN", "Empty"],
            vec!["-setproxybypassdomains", "USB LAN", "*.corp", "localhost"],
        ]
    );
    assert!(commands
        .iter()
        .all(|call| call.program == "/usr/sbin/networksetup" && call.timeout == COMMAND_TIMEOUT));
}

#[test]
fn privileged_mutations_quote_gui_arguments_and_never_allow_sudo_prompts() {
    let (mut sudo, sudo_calls) = adapter(Privilege::Sudo, vec![success("")]);
    assert!(sudo.requires_authorization());
    sudo.write(
        "Wi-Fi",
        &Operation::Enabled {
            field: Field::Https,
            enabled: true,
        },
    )
    .unwrap();
    assert_eq!(
        sudo_calls.borrow()[0],
        Invocation {
            program: "/usr/bin/sudo".into(),
            args: vec![
                "-n",
                "/usr/sbin/networksetup",
                "-setsecurewebproxystate",
                "Wi-Fi",
                "on"
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            timeout: COMMAND_TIMEOUT
        }
    );
    let (mut gui, gui_calls) = adapter(Privilege::Gui, vec![success("")]);
    gui.write(
        "O'Reilly \"USB\"\\LAN",
        &Operation::Bypass(vec!["*.corp".into()]),
    )
    .unwrap();
    let commands = gui_calls.borrow();
    assert_eq!(commands[0].program, "/usr/bin/osascript");
    assert_eq!(commands[0].timeout, AUTH_TIMEOUT);
    assert_eq!(commands[0].args[0], "-e");
    assert_eq!(
        commands[0].args[1],
        r#"do shell script "/usr/sbin/networksetup '-setproxybypassdomains' 'O'\\''Reilly \"USB\"\\LAN' '*.corp'" with administrator privileges"#
    );
    assert!(!NetworkSetup::new(Privilege::Direct).requires_authorization());
}

#[test]
fn malformed_fields_and_output_fail_closed_before_additional_writes() {
    let (mut os, calls) = adapter(Privilege::Direct, vec![Reply::Success(vec![0xff])]);
    assert!(os.services().unwrap_err().to_string().contains("not UTF-8"));
    assert!(os
        .write(
            "Wi-Fi",
            &Operation::Endpoint {
                field: Field::Bypass,
                host: "local".into(),
                port: 18885
            }
        )
        .is_err());
    assert!(os
        .write(
            "Wi-Fi",
            &Operation::Enabled {
                field: Field::Bypass,
                enabled: true
            }
        )
        .is_err());
    assert_eq!(calls.borrow().len(), 1);
    for invalid in [
        "",
        "Enabled: Maybe\nServer: local\nPort: 1\nAuthenticated Proxy Enabled: 0",
        "Enabled: No\nServer: local\nPort: 1\nAuthenticated Proxy Enabled: Unknown",
    ] {
        assert!(parse_protocol(invalid).is_err());
    }
    assert!(parse_protocol(&format!(
        "ignored line\nUnknown Key: preserve\n{}",
        protocol_text("No")
    ))
    .is_ok());
    assert!(parse_bypass(" \n ").is_err());
}

#[test]
fn process_cancellation_timeout_and_zero_exit_errors_are_not_success() {
    for reply in [
        Reply::Failure(1, "User canceled. (-128)"),
        Reply::Failure(1, "User cancelled authorization"),
        Reply::Timeout,
        success("Error: invalid service"),
        Reply::Failure(2, "unexpected failure"),
    ] {
        let (mut os, calls) = adapter(Privilege::Gui, vec![reply]);
        assert!(os
            .write(
                "Wi-Fi",
                &Operation::Enabled {
                    field: Field::Http,
                    enabled: false
                }
            )
            .is_err());
        assert_eq!(calls.borrow().len(), 1);
    }
}

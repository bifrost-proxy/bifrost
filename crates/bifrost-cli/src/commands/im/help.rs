use super::{print_im_help, print_im_send_help};

pub(super) fn is_im_context_help_request(args: &[String]) -> bool {
    if args.first().is_some_and(|group| group == "send") {
        // The send parser consumes option values before checking the next token,
        // so content such as `--text -h` is not mistaken for a help request.
        return false;
    }

    if args.get(1).is_some_and(|value| value == "help") {
        return true;
    }

    let mut index = 1;
    while let Some(value) = args.get(index) {
        if matches!(value.as_str(), "--help" | "-h") {
            return true;
        }
        index += if im_option_takes_value(value) { 2 } else { 1 };
    }
    false
}

fn im_option_takes_value(value: &str) -> bool {
    !value.contains('=')
        && matches!(
            value,
            "--type"
                | "--app-id"
                | "--secret"
                | "--display-name"
                | "--enabled"
                | "--owner-open-id"
                | "--enable-long-connection"
                | "--runner"
                | "--agent-runner"
                | "--agent-runner-id"
                | "--brand"
                | "--provider"
                | "--receive-id-type"
                | "--receive-id"
                | "--msg-type"
                | "--target"
                | "--event"
                | "--chat-id"
                | "--user-id"
                | "--keyword"
                | "--regex"
                | "--script-file"
                | "--script"
                | "--reply"
                | "--timeout-ms"
                | "--target-mode"
                | "--idempotency-key"
                | "--concurrency-policy"
                | "--retry-max"
                | "--retry-delay-ms"
                | "--cron"
                | "--every"
                | "--timezone"
                | "--agent-prompt"
                | "--agent-prompt-file"
                | "--agent-session-key"
                | "--agent-work-dir"
                | "--agent-system-prompt"
                | "--agent-model"
                | "--agent-profile"
                | "--agent-profile-v2"
                | "--agent-sandbox"
                | "--agent-reasoning-effort"
                | "--agent-reasoning-summary"
                | "--agent-approval-policy"
                | "--agent-timeout-secs"
                | "--agent-local-provider"
                | "--agent-output-schema"
                | "--agent-color"
                | "--agent-add-dir"
                | "--agent-config"
                | "--agent-enable"
                | "--agent-disable"
                | "--cwd"
                | "--direction"
                | "--source"
                | "--format"
        )
}

pub(super) fn print_im_context_help(args: &[String]) {
    let group = args.first().map(String::as_str).unwrap_or("");
    let action = args.get(1).map(String::as_str);

    match (group, action) {
        ("provider", Some("add")) => println!(
            "Usage: bifrost im provider add <ID> --type <feishu|weixin|wechat> [--app-id <ID>] [--secret <SECRET>] [--display-name <NAME>] [--enabled <BOOL>] [--owner-open-id <ID>] [--enable-long-connection <BOOL>] [--runner <RUNNER>] [--brand <BRAND>]"
        ),
        ("provider", Some("update")) => println!(
            "Usage: bifrost im provider update <ID> [--display-name <NAME>] [--enabled <BOOL>] [--enable-long-connection <BOOL>]"
        ),
        ("provider", Some("delete" | "status")) => println!(
            "Usage: bifrost im provider {} <ID>",
            action.unwrap_or_default()
        ),
        ("provider", Some("capabilities")) => println!(
            "Usage: bifrost im provider capabilities <ID> [--format human|json|json-pretty]"
        ),
        ("provider", _) => println!(
            "Usage: bifrost im provider <list|add|update|delete|status|capabilities>"
        ),
        ("send", _) => print_im_send_help(),
        ("target", Some("add")) => println!(
            "Usage: bifrost im target add <ID> [--provider <ID>] --receive-id-type <TYPE> --receive-id <ID> [--display-name <NAME>] [--msg-type <TYPE>]"
        ),
        ("target", Some("update")) => println!(
            "Usage: bifrost im target update <ID> [--receive-id <ID>] [--display-name <NAME>] [--enabled <BOOL>]"
        ),
        ("target", Some("delete")) => println!("Usage: bifrost im target delete <ID>"),
        ("target", _) => println!("Usage: bifrost im target <list|add|update|delete>"),
        ("route", Some("add")) => println!(
            "Usage: bifrost im route add <ID> [--provider <ID>] [--event <TYPE>] [--chat-id <ID>] [--user-id <ID>] [--keyword <TEXT>] [--regex <REGEX>] [--script-file <PATH>|--script <TEXT>] [--reply <MODE>] [--timeout-ms <MS>]"
        ),
        ("route", Some("pause" | "resume" | "delete")) => println!(
            "Usage: bifrost im route {} <ID>",
            action.unwrap_or_default()
        ),
        ("route", _) => println!("Usage: bifrost im route <list|add|pause|resume|delete>"),
        ("schedule", Some("pause" | "resume" | "run" | "logs" | "delete")) => {
            println!(
                "Usage: bifrost im schedule {} <ID>",
                action.unwrap_or_default()
            )
        }
        ("schedule", Some("preview" | "add" | "update")) => println!(
            "Usage: bifrost im schedule {} <ID> [OPTIONS]",
            action.unwrap_or_default()
        ),
        ("schedule", _) => println!(
            "Usage: bifrost im schedule <list|preview|add|update|pause|resume|run|logs|delete>"
        ),
        ("history", _) => println!("Usage: bifrost im history <events|runs>"),
        ("messages", Some("list")) => println!(
            "Usage: bifrost im messages list [--provider <ID>] [--direction inbound|outbound] [--source user|bot]"
        ),
        ("messages", Some("clear")) => {
            println!("Usage: bifrost im messages clear <provider-id>")
        }
        ("messages", _) => println!("Usage: bifrost im messages <list|clear>"),
        _ => print_im_help(),
    }
}

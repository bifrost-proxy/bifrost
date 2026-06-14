use std::path::PathBuf;

use clap::{ArgAction, ArgGroup, Args, Subcommand, ValueHint};

#[derive(Subcommand, Clone, Debug)]
pub enum RemoteCommands {
    #[command(
        about = "Manage the connection to a remote Bifrost instance",
        long_about = "Manage the connection (up / down / status) to a remote Bifrost instance.\n\n            `up` performs first-time pairing or re-authorization; `down` revokes authorization on the relay; `status` verifies the saved authorization still works."
    )]
    Conn {
        #[command(subcommand)]
        action: RemoteConnCommands,
    },
    #[command(
        about = "Remote file operations (policy-gated; read/write/edit/patch/mkdir/move/delete/list/find/stat/glob/hash)"
    )]
    File {
        #[command(subcommand)]
        action: Box<RemoteFileCommands>,
    },
    #[command(about = "Inspect remote traffic records (list / get / search)")]
    Traffic {
        #[command(subcommand)]
        action: RemoteTrafficCommands,
    },
    #[command(
        about = "Execute a shell command on the remote host (HIGHEST PRIVILEGE — arbitrary code execution)",
        long_about = "Execute a shell command on the remote host.\n\n            ⚠  This is the highest-privilege remote command: it can run arbitrary code on the target machine, subject only to the Shell Access policy bound to the active grant. For policy-gated structured writes, prefer `bifrost remote file write/edit/patch`."
    )]
    Exec(Box<RemoteCommandExecArgs>),
    #[command(about = "Manage remote macOS keep-awake (requires remote_power_mgmt grant)")]
    KeepAwake {
        #[command(subcommand)]
        action: crate::commands::keepawake::KeepAwakeAction,
    },
}

#[derive(Subcommand, Clone, Debug)]
pub enum RemoteConnCommands {
    #[command(
        about = "Establish or re-authorize the connection (first-time pairing or SSH key)",
        long_about = "Establish or re-authorize the connection to a remote Bifrost instance.\n\n            Use this only for the initial pairing flow, or when authorization has expired / been revoked.\n            The pair code is one-time and should not be reused after a successful connect.\n            You can also connect with an exported SSH key file via `--ssh-key <path>` or with the fixed `BIFROST_REMOTE_SSH_KEY` env via `--ssh-key`.\n            After `up` succeeds, prefer `bifrost remote conn status` (or other read-only remote commands) instead of running `up` again."
    )]
    Up {
        #[arg(
            help = "One-time pair code displayed on the remote device",
            required_unless_present = "ssh_key"
        )]
        pair_code: Option<String>,
        #[arg(
            long,
            value_name = "PATH",
            value_hint = ValueHint::FilePath,
            conflicts_with = "pair_code",
            num_args = 0..=1,
            default_missing_value = "env:BIFROST_REMOTE_SSH_KEY",
            help = "Use SSH key auth. With PATH, read a key file; without PATH, read BIFROST_REMOTE_SSH_KEY"
        )]
        ssh_key: Option<String>,
        #[arg(
            long,
            requires = "ssh_key",
            help = "Override the device code derived from --ssh-key (debug only)"
        )]
        device_code: Option<String>,
        #[arg(
            long,
            help = "Custom label for this connection (shown on the remote management UI instead of hostname)"
        )]
        label: Option<String>,
    },
    #[command(
        about = "Revoke authorization for a remote Bifrost instance",
        long_about = "Revoke authorization for a remote Bifrost instance.\n\n            Without flags, revokes all grants for the target client (resolved via --client-id).\n            Use --all to revoke grants for every connected client at once.\n            Use --grant-id to revoke a single specific grant."
    )]
    Down {
        #[arg(long, help = "Revoke grants for ALL clients (no --client-id needed)")]
        all: bool,
        #[arg(long, help = "Revoke a single specific grant by ID")]
        grant_id: Option<String>,
    },
    #[command(
        about = "Check whether the saved authorization still works on the relay",
        long_about = "Check whether the saved authorization still works on the relay.\n\n            Use this after a successful `bifrost remote conn up` to verify that the saved authorization still works.\n            If a reusable authorization already exists, you do not need to run `conn up` again."
    )]
    Status,
}

#[derive(Subcommand, Clone, Debug)]
pub enum SettingCommands {
    #[command(about = "Manage local Shell Access policies and profiles")]
    Shell {
        #[command(subcommand)]
        action: Box<RemoteShellCommands>,
    },
    #[command(about = "Manage local remote-invoke SSH key for reusable remote access")]
    SshKey {
        #[command(subcommand)]
        action: Box<RemoteSshKeyCommands>,
    },
    #[command(about = "Manage local remote-invoke grants")]
    Grant {
        #[command(subcommand)]
        action: Box<RemoteGrantCommands>,
    },
}

#[derive(Subcommand, Clone, Debug)]
pub enum RemoteSshKeyCommands {
    #[command(
        about = "Create or replace the local remote-invoke SSH key and print/export the key file"
    )]
    Create {
        #[arg(
            long,
            default_value = "bifrost-device",
            help = "Label shown in remote management UI"
        )]
        label: String,
        #[arg(
            long,
            default_value = "permanent",
            value_parser = ["once", "30m", "1h", "1d", "permanent"],
            help = "Default grant lifetime for callers using this key"
        )]
        grant_mode: String,
        #[arg(
            short,
            long,
            value_hint = ValueHint::FilePath,
            help = "Write the generated key file to this path instead of stdout"
        )]
        output: Option<String>,
        #[arg(long, help = "Overwrite --output if it already exists")]
        force: bool,
        #[arg(long, help = "Print JSON metadata instead of human output")]
        json: bool,
        #[arg(
            long,
            help = "Skip the running Admin API and write the local key store directly"
        )]
        offline: bool,
    },
    #[command(about = "Export the active local remote-invoke SSH key file")]
    Export {
        #[arg(
            short,
            long,
            value_hint = ValueHint::FilePath,
            help = "Write the key file to this path instead of stdout"
        )]
        output: Option<String>,
        #[arg(long, help = "Overwrite --output if it already exists")]
        force: bool,
        #[arg(long, help = "Print JSON metadata instead of human output")]
        json: bool,
        #[arg(
            long,
            help = "Skip the running Admin API and read the local key store directly"
        )]
        offline: bool,
    },
    #[command(about = "Show the active local remote-invoke SSH key metadata")]
    Status {
        #[arg(long, help = "Print JSON")]
        json: bool,
        #[arg(
            long,
            help = "Skip the running Admin API and read the local key store directly"
        )]
        offline: bool,
    },
    #[command(about = "Revoke the active local remote-invoke SSH key")]
    Revoke {
        #[arg(long, help = "Print JSON")]
        json: bool,
        #[arg(
            long,
            help = "Skip the running Admin API and write the local key store directly"
        )]
        offline: bool,
    },
}

#[derive(Subcommand, Clone, Debug)]
pub enum RemoteFileCommands {
    #[command(about = "Read a file from the remote host (read-only)")]
    Read {
        #[arg(help = "Path (absolute, or relative to --cwd)")]
        path: String,
        #[arg(long, help = "Maximum bytes to return (capped by policy)")]
        max_bytes: Option<u64>,
        #[arg(long, help = "Allow binary content in the response")]
        allow_binary: bool,
        #[arg(
            long,
            help = "Start line (1-based). Only return lines from this offset"
        )]
        offset: Option<u32>,
        #[arg(long, help = "Maximum number of lines to return (used with --offset)")]
        limit: Option<u32>,
        #[arg(long, help = "Working directory override")]
        cwd: Option<String>,
        #[arg(long, default_value = "human", help = "Output format: human | json")]
        output: String,
    },
    #[command(about = "Read multiple files in one round-trip. Repeat --path for each file.")]
    ReadMany {
        #[arg(
            long = "path",
            required = true,
            help = "A file to read (repeatable, at least one required)"
        )]
        paths: Vec<String>,
        #[arg(long, help = "Maximum bytes to return per file (capped by policy)")]
        max_bytes: Option<u64>,
        #[arg(long, help = "Allow binary content in the response")]
        allow_binary: bool,
        #[arg(long, help = "Working directory override")]
        cwd: Option<String>,
        #[arg(long, default_value = "human", help = "Output format: human | json")]
        output: String,
    },
    #[command(about = "List files under a directory")]
    List {
        #[arg(help = "Directory (default '.' = policy root / cwd)")]
        path: Option<String>,
        #[arg(long, default_value_t = 1, help = "Max recursion depth")]
        depth: u32,
        #[arg(long, help = "Max entries to return per page")]
        max_matches: Option<usize>,
        #[arg(
            long,
            help = "Resume token from a previous page's next_cursor (pagination)"
        )]
        cursor: Option<u64>,
        #[arg(long = "no-ignore", help = "Do not respect .gitignore rules")]
        no_ignore: bool,
        #[arg(
            long = "exclude",
            help = "Additional directory names to exclude (repeatable)"
        )]
        exclude_patterns: Vec<String>,
        #[arg(long, help = "Working directory override")]
        cwd: Option<String>,
        #[arg(long, default_value = "human")]
        output: String,
    },
    #[command(about = "Show metadata (size, mtime, mode, sha256) for a path")]
    Stat {
        #[arg(help = "Path to stat")]
        path: String,
        #[arg(long, help = "Working directory override")]
        cwd: Option<String>,
        #[arg(long, default_value = "human")]
        output: String,
    },
    #[command(about = "Match files by glob pattern")]
    Glob {
        #[arg(help = "Glob pattern (e.g. 'src/**/*.rs')")]
        pattern: String,
        #[arg(long, help = "Max matches to return")]
        max_matches: Option<usize>,
        #[arg(
            long,
            help = "Resume token from a previous page's next_cursor (pagination)"
        )]
        cursor: Option<u64>,
        #[arg(long = "no-ignore", help = "Do not respect .gitignore rules")]
        no_ignore: bool,
        #[arg(
            long = "exclude",
            help = "Additional directory names to exclude (repeatable)"
        )]
        exclude_patterns: Vec<String>,
        #[arg(long, help = "Working directory override")]
        cwd: Option<String>,
        #[arg(long, default_value = "human")]
        output: String,
    },
    #[command(
        visible_alias = "search",
        about = "Regex-search file contents under the policy root"
    )]
    Find {
        #[arg(help = "Regex pattern (positional). Combine with --regex for multiple OR patterns")]
        pattern: Option<String>,
        #[arg(
            long = "regex",
            short = 'e',
            help = "Additional regex pattern, OR-combined with others (repeatable)"
        )]
        regex: Vec<String>,
        #[arg(long, help = "Subpath to restrict the search (default: policy root)")]
        path: Option<String>,
        #[arg(long, help = "Max matches to return")]
        max_matches: Option<usize>,
        #[arg(long, help = "Per-file scan byte cap")]
        max_scan: Option<u64>,
        #[arg(
            long,
            help = "Resume token from a previous page's next_cursor (pagination)"
        )]
        cursor: Option<u64>,
        #[arg(long, short = 'B', help = "Number of context lines before each match")]
        context_before: Option<u32>,
        #[arg(long, short = 'A', help = "Number of context lines after each match")]
        context_after: Option<u32>,
        #[arg(
            long = "around",
            short = 'C',
            help = "Lines of context before AND after each match, joined into a `snippet` field (takes precedence over -A/-B)"
        )]
        around: Option<u32>,
        #[arg(
            long = "case-insensitive",
            short = 'i',
            help = "Case-insensitive regex search"
        )]
        case_insensitive: bool,
        #[arg(
            long = "fixed-strings",
            short = 'F',
            help = "Treat patterns as literal strings instead of regex"
        )]
        fixed_strings: bool,
        #[arg(
            long = "word",
            short = 'w',
            help = "Match whole words only (wraps each pattern with word boundaries)"
        )]
        word: bool,
        #[arg(
            long = "glob",
            help = "Only search files matching this glob (e.g. '*.rs')"
        )]
        glob: Option<String>,
        #[arg(long = "no-ignore", help = "Do not respect .gitignore rules")]
        no_ignore: bool,
        #[arg(
            long = "exclude",
            help = "Additional directory names to exclude (repeatable)"
        )]
        exclude_patterns: Vec<String>,
        #[arg(long, help = "Working directory override")]
        cwd: Option<String>,
        #[arg(long, default_value = "human")]
        output: String,
    },
    #[command(about = "Hash a file (sha256 only)")]
    Hash {
        #[arg(help = "Path to hash")]
        path: String,
        #[arg(long, default_value = "sha256", help = "Digest algorithm")]
        algo: String,
        #[arg(long, help = "Working directory override")]
        cwd: Option<String>,
        #[arg(long, default_value = "human")]
        output: String,
    },
    #[command(
        about = "Write a file atomically. Content comes from --content, --content-file, --content-b64, or stdin."
    )]
    Write {
        #[arg(help = "Target path (absolute, or relative to --cwd)")]
        path: String,
        #[arg(
            long,
            help = "Inline UTF-8 content to write (simplest option for short text)"
        )]
        content: Option<String>,
        #[arg(long, value_hint = ValueHint::FilePath, help = "Read content from a local file (or '-' for stdin)")]
        content_file: Option<String>,
        #[arg(
            long = "content-b64",
            help = "Provide content as a base64-encoded string (overrides --content/--content-file)"
        )]
        content_b64: Option<String>,
        #[arg(long, help = "Expected current sha256 for optimistic locking")]
        base_sha256: Option<String>,
        #[arg(long, help = "Allow overwrite even if policy default forbids")]
        allow_overwrite: Option<bool>,
        #[arg(long = "create-parents", help = "Create missing parent directories")]
        create_parents: bool,
        #[arg(long, help = "Working directory override")]
        cwd: Option<String>,
        #[arg(long, default_value = "human")]
        output: String,
    },
    #[command(
        about = "Apply line-range edits atomically. --edits is a JSON array of {start_line,end_line,replacement}."
    )]
    Edit {
        #[arg(help = "Target path")]
        path: String,
        #[arg(long, help = "JSON array of edit ranges")]
        edits: String,
        #[arg(long, help = "Expected current sha256 for optimistic locking")]
        base_sha256: Option<String>,
        #[arg(long, help = "Working directory override")]
        cwd: Option<String>,
        #[arg(long, default_value = "human")]
        output: String,
    },
    #[command(about = "Create a directory on the remote host")]
    Mkdir {
        #[arg(help = "Directory to create")]
        path: String,
        #[arg(long, help = "Create intermediate parent directories as needed")]
        parents: bool,
        #[arg(long, help = "Working directory override")]
        cwd: Option<String>,
        #[arg(long, default_value = "human")]
        output: String,
    },
    #[command(
        visible_alias = "mv",
        about = "Move / rename a path on the remote host"
    )]
    Move {
        #[arg(help = "Source path")]
        from: String,
        #[arg(help = "Destination path")]
        to: String,
        #[arg(long, help = "Working directory override")]
        cwd: Option<String>,
        #[arg(long, default_value = "human")]
        output: String,
    },
    #[command(
        visible_alias = "rm",
        about = "Delete a file or directory on the remote host"
    )]
    Delete {
        #[arg(help = "Path to delete")]
        path: String,
        #[arg(long, help = "Recursively delete a non-empty directory")]
        recursive: bool,
        #[arg(long, help = "Working directory override")]
        cwd: Option<String>,
        #[arg(long, default_value = "human")]
        output: String,
    },
    #[command(
        visible_alias = "apply-patch",
        about = "Apply a unified diff across multiple files atomically"
    )]
    Patch {
        #[arg(long, value_hint = ValueHint::FilePath, help = "Path to a unified diff file (or '-' for stdin)")]
        patch_file: Option<String>,
        #[arg(
            long = "patch-b64",
            help = "Provide the unified diff as a base64 string (overrides --patch-file)"
        )]
        patch_b64: Option<String>,
        #[arg(
            long = "base-sha",
            value_name = "PATH=SHA256",
            help = "Per-file optimistic lock: expected current sha256 for a patch target \
                    (repeatable). Use an empty sha (PATH=) to assert the file does not exist yet. \
                    PATH must match the patch's target path."
        )]
        base_sha: Vec<String>,
        #[arg(long, help = "Working directory override")]
        cwd: Option<String>,
        #[arg(long, default_value = "human")]
        output: String,
    },
}

#[derive(Subcommand, Clone, Debug)]
pub enum RemoteShellCommands {
    #[command(about = "List configured shell policies and profiles")]
    List,
    #[command(about = "Show shell config summary or raw JSON")]
    Show {
        #[arg(long, help = "Print the full JSON payload")]
        json: bool,
    },
    #[command(about = "Apply a full shell config JSON file")]
    Apply {
        #[arg(value_hint = ValueHint::FilePath, help = "Path to remote_shell.json payload")]
        file: PathBuf,
    },
    #[command(about = "Manage shell profiles")]
    Profile {
        #[command(subcommand)]
        action: RemoteShellProfileCommands,
    },
    #[command(about = "Manage shell policies")]
    Policy {
        #[command(subcommand)]
        action: Box<RemoteShellPolicyCommands>,
    },
}

#[derive(Subcommand, Clone, Debug)]
pub enum RemoteGrantCommands {
    #[command(about = "List local active grants")]
    List {
        #[arg(long, help = "Print the full JSON payload")]
        json: bool,
    },
    #[command(about = "Update a grant's access scope and policy binding")]
    Update {
        #[arg(help = "Grant ID to update")]
        grant_id: String,
        #[arg(long, value_parser = ["query", "selected", "all"], help = "Shell/query access mode")]
        access: Option<String>,
        #[arg(
            long,
            value_parser = ["remote_query", "remote_shell_exec", "remote_shell_interactive"],
            help = "Explicit grant scope for shell/query access"
        )]
        scope: Option<String>,
        #[arg(long = "policy", action = ArgAction::Append, help = "Bind to this shell policy id (repeatable for selected access)")]
        policy: Vec<String>,
        #[arg(long, help = "Allow stdin: true or false")]
        stdin: Option<bool>,
        #[arg(long, help = "Allow interactive shell: true or false")]
        interactive: Option<bool>,
        #[arg(long, value_parser = ["none", "read", "read_write"], help = "File access level: none, read, or read_write")]
        file_access: Option<String>,
    },
    #[command(about = "Revoke a grant")]
    Revoke {
        #[arg(help = "Grant ID to revoke")]
        grant_id: String,
    },
}

#[derive(Subcommand, Clone, Debug)]
pub enum RemoteShellProfileCommands {
    #[command(about = "Add a shell profile")]
    Add {
        #[arg(long)]
        id: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        description: Option<String>,
        #[arg(long = "cwd", action = ArgAction::Append, help = "Allow cwd under this path")]
        cwd: Vec<String>,
        #[arg(long = "env", action = ArgAction::Append, help = "Allow this environment key")]
        env: Vec<String>,
        #[arg(long)]
        default_cwd: Option<String>,
        #[arg(long = "timeout-ms")]
        timeout_ms: Option<u64>,
        #[arg(long)]
        stdin: bool,
        #[arg(long)]
        interactive: bool,
        #[arg(long)]
        inherit_env: bool,
        #[arg(long, help = "Create the profile disabled")]
        disabled: bool,
    },
    #[command(about = "Delete a shell profile")]
    Delete { id: String },
    #[command(about = "Enable a shell profile")]
    Enable { id: String },
    #[command(about = "Disable a shell profile")]
    Disable { id: String },
}

#[derive(Subcommand, Clone, Debug)]
pub enum RemoteShellPolicyCommands {
    #[command(about = "Add a shell policy")]
    Add(Box<RemoteShellPolicyAddArgs>),
    #[command(
        about = "Update an existing shell policy's metadata in-place (preserves grant validity)"
    )]
    Update(Box<RemoteShellPolicyUpdateArgs>),
    #[command(about = "Delete a shell policy")]
    Delete { id: String },
    #[command(about = "Enable a shell policy")]
    Enable { id: String },
    #[command(about = "Disable a shell policy")]
    Disable { id: String },
}

#[derive(Args, Clone, Debug)]
pub struct RemoteShellPolicyAddArgs {
    #[arg(long)]
    pub id: String,
    #[arg(long)]
    pub name: String,
    #[arg(long)]
    pub description: Option<String>,
    #[arg(long, default_value = "argv_exec", help = "argv_exec or shell_text")]
    pub mode: String,
    #[arg(long, help = "Bind to an existing shell profile")]
    pub profile: Option<String>,
    #[arg(
        long = "program",
        action = ArgAction::Append,
        help = "Allow this executable path"
    )]
    pub program: Vec<String>,
    #[arg(
        long = "pattern",
        action = ArgAction::Append,
        help = "Allow this shell regex"
    )]
    pub pattern: Vec<String>,
    #[arg(long = "cwd", action = ArgAction::Append, help = "Allow cwd under this path")]
    pub cwd: Vec<String>,
    #[arg(
        long = "env",
        action = ArgAction::Append,
        help = "Allow this environment key"
    )]
    pub env: Vec<String>,
    #[arg(long)]
    pub default_cwd: Option<String>,
    #[arg(long = "timeout-ms")]
    pub timeout_ms: Option<u64>,
    #[arg(long, help = "Shell binary for shell_text mode")]
    pub shell: Option<String>,
    #[arg(long)]
    pub stdin: bool,
    #[arg(long)]
    pub interactive: bool,
    #[arg(long)]
    pub inherit_env: bool,
    #[arg(long, help = "Create the policy disabled")]
    pub disabled: bool,
}

#[derive(Args, Clone, Debug)]
pub struct RemoteShellPolicyUpdateArgs {
    #[arg(help = "Policy ID to update")]
    pub id: String,
    #[arg(long, help = "New display name")]
    pub name: Option<String>,
    #[arg(long, help = "New description")]
    pub description: Option<String>,
    #[arg(long, help = "New exec mode: argv_exec or shell_text")]
    pub mode: Option<String>,
    #[arg(long, help = "Bind to an existing shell profile")]
    pub profile: Option<String>,
    #[arg(long = "program", action = ArgAction::Append, help = "Replace allowed executables")]
    pub program: Vec<String>,
    #[arg(long = "pattern", action = ArgAction::Append, help = "Replace allowed shell patterns")]
    pub pattern: Vec<String>,
    #[arg(long = "cwd", action = ArgAction::Append, help = "Replace allowed cwd paths")]
    pub cwd: Vec<String>,
    #[arg(long = "env", action = ArgAction::Append, help = "Replace allowed env keys")]
    pub env: Vec<String>,
    #[arg(long, help = "New default working directory")]
    pub default_cwd: Option<String>,
    #[arg(long = "timeout-ms", help = "New timeout in milliseconds")]
    pub timeout_ms: Option<u64>,
    #[arg(long, help = "Shell binary for shell_text mode")]
    pub shell: Option<String>,
    #[arg(long, help = "Allow stdin")]
    pub stdin: Option<bool>,
    #[arg(long, help = "Allow interactive shell")]
    pub interactive: Option<bool>,
    #[arg(long, help = "Inherit environment from parent")]
    pub inherit_env: Option<bool>,
}

#[derive(Args, Clone, Debug)]
#[command(group(
    ArgGroup::new("command_input")
        .required(true)
        .args(["shell_text", "argv"])
))]
pub struct RemoteCommandExecArgs {
    #[arg(long, help = "Working directory on the remote host")]
    pub cwd: Option<String>,
    #[arg(
        long = "env",
        value_name = "KEY=VALUE",
        value_parser = parse_env_assignment,
        action = ArgAction::Append,
        help = "Environment variable assignment (repeatable)"
    )]
    pub env: Vec<(String, String)>,
    #[arg(long = "timeout-ms", help = "Remote command timeout in milliseconds")]
    pub timeout_ms: Option<u64>,
    #[arg(
        long = "shell-text",
        value_name = "TEXT",
        conflicts_with = "argv",
        help = "Execute the given shell text on the remote host"
    )]
    pub shell_text: Option<String>,
    #[arg(
        value_name = "PROGRAM [ARGS...]",
        num_args = 1..,
        last = true,
        allow_hyphen_values = true,
        help = "Program and arguments to execute after `--`"
    )]
    pub argv: Vec<String>,
    // PR #5b: large-output / long-running transport controls. Parsed here so
    // the caller side can opt into streaming semantics and resume. Wiring into
    // the actual exec pipeline lands in PR #5c.
    #[arg(
        long = "stream",
        help = "Stream stdout as bytes arrive instead of buffering the whole response"
    )]
    pub stream: bool,
    #[arg(long = "stdin", help = "Forward local stdin to the remote command")]
    pub stdin: bool,
    #[arg(long = "pty", help = "Request remote PTY/interactive execution")]
    pub pty: bool,
    #[arg(
        long = "interactive",
        help = "Shortcut for --pty --stdin --stream with local raw terminal mode"
    )]
    pub interactive: bool,
    #[arg(
        long = "output-file",
        value_name = "PATH",
        help = "Write streamed stdout to a local file instead of the terminal (implies --stream)"
    )]
    pub output_file: Option<String>,
    #[arg(
        long = "resume-call-id",
        value_name = "UUID",
        help = "Resume an existing remote call by its call_id (requires server-side session retention)"
    )]
    pub resume_call_id: Option<String>,
    #[arg(
        long = "resume-relay-token",
        value_name = "TOKEN",
        requires = "resume_call_id",
        help = "Relay token for the existing call (required with --resume-call-id)"
    )]
    pub resume_relay_token: Option<String>,
    #[arg(
        long = "no-verify-digest",
        help = "Skip the client-side streaming SHA-256 verification against the Done frame digest"
    )]
    pub no_verify_digest: bool,
}

fn parse_env_assignment(value: &str) -> Result<(String, String), String> {
    let Some((key, val)) = value.split_once('=') else {
        return Err("expected KEY=VALUE".to_string());
    };
    if key.is_empty() {
        return Err("environment variable name cannot be empty".to_string());
    }
    Ok((key.to_string(), val.to_string()))
}

#[derive(Args, Clone, Debug)]
pub struct RemoteTrafficListArgs {
    #[arg(short, long, default_value = "50", help = "Maximum records")]
    pub limit: usize,
    #[arg(long, help = "Cursor for pagination")]
    pub cursor: Option<u64>,
    #[arg(
        long,
        default_value = "backward",
        value_parser = ["backward", "forward"],
        help = "Pagination direction: backward or forward"
    )]
    pub direction: String,
    #[arg(long, help = "Filter by HTTP method")]
    pub method: Option<String>,
    #[arg(long, help = "Filter by status code")]
    pub status: Option<u16>,
    #[arg(long, help = "Filter by status >= value")]
    pub status_min: Option<u16>,
    #[arg(long, help = "Filter by status <= value")]
    pub status_max: Option<u16>,
    #[arg(
        long,
        value_parser = ["http", "https", "ws", "wss", "h3"],
        help = "Filter by protocol (http/https/ws/wss/h3)"
    )]
    pub protocol: Option<String>,
    #[arg(long, help = "Filter host contains")]
    pub host: Option<String>,
    #[arg(long, help = "Filter URL contains")]
    pub url: Option<String>,
    #[arg(long, help = "Filter path contains")]
    pub path: Option<String>,
    #[arg(long, help = "Filter by content type")]
    pub content_type: Option<String>,
    #[arg(long, help = "Filter by client IP")]
    pub client_ip: Option<String>,
    #[arg(long, help = "Filter by client app")]
    pub client_app: Option<String>,
    #[arg(
        long = "listener-port",
        visible_alias = "proxy-port",
        help = "Filter by proxy listener port"
    )]
    pub listener_port: Option<u16>,
    #[arg(long, help = "Filter by rule hit (true/false)")]
    pub has_rule_hit: Option<bool>,
    #[arg(long, help = "Filter websocket only (true/false)")]
    pub is_websocket: Option<bool>,
    #[arg(long, help = "Filter SSE only (true/false)")]
    pub is_sse: Option<bool>,
    #[arg(long, help = "Filter tunnel only (true/false)")]
    pub is_tunnel: Option<bool>,
    #[arg(
        short,
        long,
        default_value = "table",
        value_parser = ["table", "compact", "json", "json-pretty"],
        help = "Output format: table, compact, json, json-pretty"
    )]
    pub format: String,
    #[arg(long, help = "Disable colored output")]
    pub no_color: bool,
}

#[derive(Args, Clone, Debug)]
pub struct RemoteSearchArgs {
    #[arg(help = "Search keyword")]
    pub keyword: Option<String>,
    #[arg(short, long, default_value = "50", help = "Maximum results to return")]
    pub limit: usize,
    #[arg(
        short,
        long,
        default_value = "table",
        value_parser = ["table", "compact", "json", "json-pretty"],
        help = "Output format: table, compact, json, json-pretty"
    )]
    pub format: String,
    #[arg(long, help = "Disable colored output")]
    pub no_color: bool,
    #[arg(long, help = "Search only in URL/path")]
    pub url: bool,
    #[arg(long, help = "Search only in headers")]
    pub headers: bool,
    #[arg(long, help = "Search only in body")]
    pub body: bool,
    #[arg(long = "req-header", help = "Search only in request headers")]
    pub req_header: bool,
    #[arg(long = "res-header", help = "Search only in response headers")]
    pub res_header: bool,
    #[arg(long = "req-body", help = "Search only in request body")]
    pub req_body: bool,
    #[arg(long = "res-body", help = "Search only in response body")]
    pub res_body: bool,
    #[arg(
        long,
        value_parser = ["2xx", "3xx", "4xx", "5xx", "error"],
        help = "Filter by status: 2xx, 3xx, 4xx, 5xx, error"
    )]
    pub status: Option<String>,
    #[arg(
        long,
        value_parser = [
            "GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS", "CONNECT", "TRACE"
        ],
        help = "Filter by HTTP method: GET, POST, PUT, DELETE, etc."
    )]
    pub method: Option<String>,
    #[arg(long, help = "Filter host contains")]
    pub host: Option<String>,
    #[arg(long, help = "Filter path contains")]
    pub path: Option<String>,
    #[arg(
        long = "listener-port",
        visible_alias = "proxy-port",
        help = "Filter by proxy listener port"
    )]
    pub listener_port: Option<u16>,
    #[arg(
        long,
        value_parser = ["HTTP", "HTTPS", "WS", "WSS"],
        help = "Filter by protocol: HTTP, HTTPS, WS, WSS"
    )]
    pub protocol: Option<String>,
    #[arg(
        long,
        value_parser = [
            "json",
            "xml",
            "html",
            "form",
            "text",
            "javascript",
            "css",
            "image",
            "font",
            "binary"
        ],
        help = "Filter by content type: json, xml, html, form, etc."
    )]
    pub content_type: Option<String>,
    #[arg(long, help = "Filter by domain pattern")]
    pub domain: Option<String>,
    #[arg(
        long = "max-scan",
        default_value = "10000",
        help = "Maximum records to scan (default: 10000, use larger value for broader search)"
    )]
    pub max_scan: Option<usize>,
    #[arg(
        long = "max-results",
        default_value = "100",
        help = "Maximum matching results to return (default: 100)"
    )]
    pub max_results: Option<usize>,
}

#[derive(Subcommand, Clone, Debug)]
pub enum RemoteTrafficCommands {
    #[command(about = "List remote traffic records")]
    List(Box<RemoteTrafficListArgs>),
    #[command(about = "Get remote traffic record details")]
    Get {
        #[arg(help = "Traffic record ID")]
        id: String,
        #[arg(long, help = "Include request body")]
        request_body: bool,
        #[arg(long, help = "Include response body")]
        response_body: bool,
        #[arg(
            short,
            long,
            default_value = "json-pretty",
            value_parser = ["table", "compact", "json", "json-pretty"],
            help = "Output format: table, compact, json, json-pretty"
        )]
        format: String,
        #[arg(long, help = "Disable colored output")]
        no_color: bool,
    },
    #[command(about = "Search remote traffic records")]
    Search(Box<RemoteSearchArgs>),
}

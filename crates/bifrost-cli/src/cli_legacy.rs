//! Rewrites legacy `bifrost remote` argv prefixes into the current 4-tier
//! CLI tree, printing a one-shot deprecation notice to stderr.
//!
//! The 4-tier rename (commit `bb4d933a` onward) broke argv compatibility for
//! any script that invoked the old verbs directly. To give downstream
//! callers one release of breathing room we intercept argv BEFORE `clap`
//! sees it and rewrite:
//!
//! | Legacy                       | Current                          |
//! |------------------------------|----------------------------------|
//! | `remote connect ...`         | `remote conn up ...`             |
//! | `remote disconnect ...`      | `remote conn down ...`           |
//! | `remote status ...`          | `remote conn status ...`         |
//! | `remote search ...`          | `remote traffic search ...`      |
//! | `remote command exec ...`    | `remote exec ...`                |
//! | `remote file search ...`     | `remote file find ...`           |
//! | `remote file mv ...`         | `remote file move ...`           |
//! | `remote file rm ...`         | `remote file delete ...`         |
//! | `remote file apply-patch ...`| `remote file patch ...`          |
//!
//! This shim is intentionally conservative:
//! - It only rewrites when the legacy verb appears in the exact expected
//!   position (directly after `remote`, or after `remote file`). It does NOT
//!   rewrite occurrences inside a `--flag=value` or after `--`.
//! - It emits a single `warn!`-style line to stderr per invocation so scripts
//!   see the breakage coming without being spammed.
//!
//! **Removal plan:** delete this shim one minor release after it ships; a CI
//! guard in `scripts/ci/check-remote-cli-legacy.sh` already prevents in-repo
//! backsliding. Loud user-facing deprecation lives here.

use std::ffi::OsString;

/// Intercept the first N positional tokens that follow `remote` and rewrite
/// them into the current 4-tier form, printing a deprecation notice if any
/// rewrite happened.
pub fn rewrite_legacy_remote_argv(args: Vec<OsString>) -> Vec<OsString> {
    // Find the position of `remote` among positional tokens. We stop at the
    // first `--` (explicit positional terminator) and skip known value-taking
    // global flags so a `--log-level remote` pathological case doesn't
    // confuse us.
    let remote_pos = find_remote_pos(&args);
    let Some(pos) = remote_pos else {
        return args;
    };

    let mut out = args;
    let mut notices: Vec<&'static str> = Vec::new();

    // Tier-1 verbs immediately after `remote`:
    if let Some(verb) = out.get(pos + 1).and_then(|s| s.to_str()) {
        match verb {
            "connect" => {
                out.splice(pos + 1..=pos + 1, ["conn", "up"].iter().map(OsString::from));
                notices
                    .push("`bifrost remote connect` is deprecated; use `bifrost remote conn up`");
            }
            "disconnect" => {
                out.splice(
                    pos + 1..=pos + 1,
                    ["conn", "down"].iter().map(OsString::from),
                );
                notices.push(
                    "`bifrost remote disconnect` is deprecated; use `bifrost remote conn down`",
                );
            }
            "status" => {
                out.splice(
                    pos + 1..=pos + 1,
                    ["conn", "status"].iter().map(OsString::from),
                );
                notices.push(
                    "`bifrost remote status` is deprecated; use `bifrost remote conn status`",
                );
            }
            "search" => {
                out.splice(
                    pos + 1..=pos + 1,
                    ["traffic", "search"].iter().map(OsString::from),
                );
                notices.push(
                    "`bifrost remote search` is deprecated; use `bifrost remote traffic search`",
                );
            }
            "command" => {
                // `remote command exec <args>` -> `remote exec <args>`
                // Any other `remote command <sub>` was never shipped; leave as-is so clap errors loudly.
                if out.get(pos + 2).and_then(|s| s.to_str()) == Some("exec") {
                    // Replace the two-token span `command exec` with a single `exec`.
                    out.splice(pos + 1..=pos + 2, ["exec"].iter().map(OsString::from));
                    notices.push(
                        "`bifrost remote command exec` is deprecated; use `bifrost remote exec`",
                    );
                }
            }
            "file" => {
                // Tier-2 verbs after `remote file`. Copy the borrow to an
                // owned `String` before we mutate `out` to avoid E0502.
                let sub_owned: Option<String> = out
                    .get(pos + 2)
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_string());
                if let Some(sub) = sub_owned.as_deref() {
                    let replacement_and_notice: Option<(&'static str, &'static str)> = match sub {
                        "search" => Some((
                            "find",
                            "`bifrost remote file search` is deprecated; use `bifrost remote file find`",
                        )),
                        "mv" => Some((
                            "move",
                            "`bifrost remote file mv` is deprecated; use `bifrost remote file move`",
                        )),
                        "rm" => Some((
                            "delete",
                            "`bifrost remote file rm` is deprecated; use `bifrost remote file delete`",
                        )),
                        "apply-patch" => Some((
                            "patch",
                            "`bifrost remote file apply-patch` is deprecated; use `bifrost remote file patch`",
                        )),
                        _ => None,
                    };
                    if let Some((new_verb, msg)) = replacement_and_notice {
                        out[pos + 2] = OsString::from(new_verb);
                        notices.push(msg);
                    }
                }
            }
            _ => {}
        }
    }

    for msg in notices {
        eprintln!("warning: {msg} (legacy alias will be removed in the next minor release)");
    }

    out
}

fn find_remote_pos(args: &[OsString]) -> Option<usize> {
    // Skip argv[0] (program name). Stop at the first bare `--`.
    let mut i = 1;
    while i < args.len() {
        let s = args[i].to_str().unwrap_or("");
        if s == "--" {
            return None;
        }
        if s == "remote" {
            return Some(i);
        }
        // Skip value for global flags that take an argument. Conservative list;
        // if we miss one the worst case is we fail to rewrite (user sees the
        // raw clap error).
        if matches!(
            s,
            "--log-level"
                | "--log-dir"
                | "--log-output"
                | "--log-retention-days"
                | "--port"
                | "-p"
                | "--host"
                | "-H"
                | "--socks5-port"
        ) {
            i += 2;
            continue;
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rw(argv: &[&str]) -> Vec<String> {
        rewrite_legacy_remote_argv(argv.iter().map(OsString::from).collect())
            .into_iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn rewrites_connect() {
        assert_eq!(
            rw(&["bifrost", "remote", "connect", "ABC"]),
            vec!["bifrost", "remote", "conn", "up", "ABC"]
        );
    }

    #[test]
    fn rewrites_disconnect_with_flag() {
        assert_eq!(
            rw(&["bifrost", "remote", "disconnect", "--all"]),
            vec!["bifrost", "remote", "conn", "down", "--all"]
        );
    }

    #[test]
    fn rewrites_status() {
        assert_eq!(
            rw(&["bifrost", "remote", "status"]),
            vec!["bifrost", "remote", "conn", "status"]
        );
    }

    #[test]
    fn rewrites_search_to_traffic_search() {
        assert_eq!(
            rw(&["bifrost", "remote", "search", "foo"]),
            vec!["bifrost", "remote", "traffic", "search", "foo"]
        );
    }

    #[test]
    fn rewrites_command_exec() {
        assert_eq!(
            rw(&["bifrost", "remote", "command", "exec", "--", "ls"]),
            vec!["bifrost", "remote", "exec", "--", "ls"]
        );
    }

    #[test]
    fn rewrites_file_search() {
        assert_eq!(
            rw(&["bifrost", "remote", "file", "search", "pat"]),
            vec!["bifrost", "remote", "file", "find", "pat"]
        );
    }

    #[test]
    fn rewrites_file_mv() {
        assert_eq!(
            rw(&["bifrost", "remote", "file", "mv", "a", "b"]),
            vec!["bifrost", "remote", "file", "move", "a", "b"]
        );
    }

    #[test]
    fn rewrites_file_rm() {
        assert_eq!(
            rw(&["bifrost", "remote", "file", "rm", "x"]),
            vec!["bifrost", "remote", "file", "delete", "x"]
        );
    }

    #[test]
    fn rewrites_file_apply_patch() {
        assert_eq!(
            rw(&["bifrost", "remote", "file", "apply-patch"]),
            vec!["bifrost", "remote", "file", "patch"]
        );
    }

    #[test]
    fn leaves_current_verbs_untouched() {
        let argv = vec!["bifrost", "remote", "conn", "up"];
        assert_eq!(rw(&argv), argv);
    }

    #[test]
    fn respects_bare_dash_dash() {
        // Tokens after `--` must not be touched.
        let argv = vec!["bifrost", "--", "remote", "connect"];
        assert_eq!(rw(&argv), argv);
    }

    #[test]
    fn skips_value_of_global_flag() {
        // `--log-level remote` should not be misread as the `remote` subcommand.
        let argv = vec!["bifrost", "--log-level", "remote", "status"];
        // No `remote` subcommand present -> no rewrite.
        assert_eq!(rw(&argv), argv);
    }

    #[test]
    fn leaves_unknown_remote_verb_untouched() {
        // Unknown verb: let clap produce its own error.
        let argv = vec!["bifrost", "remote", "bogus"];
        assert_eq!(rw(&argv), argv);
    }

    #[test]
    fn rewrites_command_exec_only_exec() {
        // `remote command` (no `exec`) is not a legacy alias we support; leave as-is.
        let argv = vec!["bifrost", "remote", "command"];
        assert_eq!(rw(&argv), argv);
    }
}

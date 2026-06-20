//! In-process system tray helper, merged into the `bifrost` binary.
//!
//! Historically the tray ran as a separate `bifrost-tray` binary. To avoid
//! shipping a second distribution artifact, the tray now runs as a separate
//! *process* re-exec'd from the same `bifrost` binary (busybox-style
//! multi-call) via a hidden `__tray` subcommand.
//!
//! On macOS the AppKit/NSApplication event loop must run on the process main
//! thread; because the tray runs in its own re-exec'd child process, its
//! `main()` runs the event loop on that child's main thread, satisfying the
//! constraint naturally.

mod cli;

#[cfg(not(target_os = "linux"))]
mod config;
#[cfg(not(target_os = "linux"))]
mod lock;
#[cfg(not(target_os = "linux"))]
mod menu;
#[cfg(not(target_os = "linux"))]
mod runtime;
#[cfg(not(target_os = "linux"))]
mod system_stats;
#[cfg(not(target_os = "linux"))]
#[allow(clippy::module_inception)]
mod tray;

pub use cli::TRAY_SUBCOMMAND;

/// If the process was invoked as the hidden tray subcommand
/// (`bifrost __tray ...`), run the tray helper and never return.
///
/// This must be called at the very start of `main()`, BEFORE clap parsing and
/// logging initialization: the tray installs its own tracing subscriber and
/// must not collide with the CLI's logging setup, and the `__tray` token is not
/// a real clap subcommand.
///
/// Returns normally only when the process is NOT a tray process, so the caller
/// can continue with regular CLI handling.
pub fn run_if_tray_process() {
    let mut args = std::env::args_os();
    let _program = args.next();
    let Some(first) = args.next() else {
        return;
    };
    if first != TRAY_SUBCOMMAND {
        return;
    }

    // Remaining args after the `__tray` token are the tray's own flags.
    let tray_args = cli::TrayArgs::parse_from_args(args);

    #[cfg(target_os = "linux")]
    {
        let _ = tray_args;
        eprintln!("error: tray is not supported on Linux yet");
        std::process::exit(1);
    }

    #[cfg(not(target_os = "linux"))]
    {
        let code = match tray::run(tray_args) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("bifrost __tray: {e}");
                1
            }
        };
        std::process::exit(code);
    }
}

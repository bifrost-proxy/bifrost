mod cli;
mod config;
mod lock;
mod runtime;

#[cfg(not(target_os = "linux"))]
mod menu;
#[cfg(not(target_os = "linux"))]
mod tray;

fn main() {
    #[cfg(target_os = "linux")]
    {
        // Parse args for validation/help output even though tray is unsupported.
        let _args = cli::parse_args();
        eprintln!("error: tray is not supported on Linux yet");
        std::process::exit(1);
    }

    #[cfg(not(target_os = "linux"))]
    {
        let args = cli::parse_args();
        if let Err(e) = tray::run(args) {
            eprintln!("bifrost-tray: {e}");
            std::process::exit(1);
        }
    }
}

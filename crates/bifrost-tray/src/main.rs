mod cli;
mod config;
mod lock;
mod runtime;

#[cfg(not(target_os = "linux"))]
mod menu;
#[cfg(not(target_os = "linux"))]
mod tray;

fn main() {
    let args = cli::parse_args();

    #[cfg(target_os = "linux")]
    {
        eprintln!("error: tray is not supported on Linux yet");
        std::process::exit(1);
    }

    #[cfg(not(target_os = "linux"))]
    {
        if let Err(e) = tray::run(args) {
            eprintln!("bifrost-tray: {e}");
            std::process::exit(1);
        }
    }
}

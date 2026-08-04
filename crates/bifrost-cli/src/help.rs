const BIFROST_BANNER: &str = r#"
    ____  _ ____                __
   / __ )(_) __/_________  ____/ /_
  / __  / / /_/ ___/ __ \/ ___/ __/
 / /_/ / / __/ /  / /_/ (__  ) /_
/_____/_/_/ /_/   \____/____/\__/

"#;

pub fn print_banner() {
    if supports_color() {
        print_rainbow_banner();
    } else {
        print!("{}", BIFROST_BANNER);
    }
}

fn print_rainbow_banner() {
    const ESC: &str = "\x1b[";
    const RESET: &str = "\x1b[0m";

    println!();
    println!(
        "{}38;5;196m    ____  _ ____                __{}",
        ESC, RESET
    );
    println!(
        "{}38;5;208m   / __ )(_) __/_________  ____/ /_{}",
        ESC, RESET
    );
    println!(
        "{}38;5;226m  / __  / / /_/ ___/ __ \\/ ___/ __/{}",
        ESC, RESET
    );
    println!("{}38;5;46m / /_/ / / __/ /  / /_/ (__  ) /_{}", ESC, RESET);
    println!(
        "{}38;5;21m/_____/_/_/ /_/   \\____/____/\\__/{}",
        ESC, RESET
    );
    println!();
}

fn supports_color() -> bool {
    if std::env::var("NO_COLOR").is_ok() {
        return false;
    }
    if std::env::var("FORCE_COLOR").is_ok() {
        return true;
    }
    #[cfg(unix)]
    {
        if let Ok(term) = std::env::var("TERM") {
            if term == "dumb" {
                return false;
            }
        }
        unsafe { libc::isatty(libc::STDOUT_FILENO) != 0 }
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        let handle = std::io::stdout().as_raw_handle();
        let mut mode: u32 = 0;
        unsafe {
            let result = windows_sys::Win32::System::Console::GetConsoleMode(
                handle as windows_sys::Win32::Foundation::HANDLE,
                &mut mode,
            );
            result != 0
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

pub fn print_startup_help(port: u16) {
    print_banner();
    println!(
        r#"╭────────────────────────────────────────────────────────────────────────╮
│                               BIFROST                                   │
│                        Proxy is up and running                           │
╰────────────────────────────────────────────────────────────────────────╯

ADMIN UI
  http://127.0.0.1:{port}/

QUICK START (common workflows)
  1) Proxy all traffic (local machine)
     HTTP(S) proxy:  127.0.0.1:{port}
     SOCKS5 proxy:   127.0.0.1:{port}  (or use --socks5-port)

  2) Enable CLI proxy env vars (so terminal tools use it automatically)
     bifrost -p {port} start --cli-proxy

  3) Enable system proxy (so browsers/apps use it automatically)
     bifrost system-proxy enable --host 127.0.0.1 --port {port}
     bifrost system-proxy status

  4) Stop / inspect
     bifrost status
     bifrost stop

HELP & DISCOVERY
  bifrost --help                     Full CLI help (global options + subcommands)
  bifrost start --help               Start options (proxy behavior, TLS, access)
  bifrost rule --help                Rule commands
  bifrost config --help              Config management
  bifrost traffic --help             Traffic capture & inspection
  bifrost search --help              Search captured traffic (advanced)
  bifrost upgrade --help             Self-upgrade (alias: update)
Tip: Use 'bifrost <command> --help' to see all flags and examples."#,
        port = port
    );
    println!();
}

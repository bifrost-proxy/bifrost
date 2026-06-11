use std::path::PathBuf;

use clap::Parser;

/// Hidden subcommand token used to re-exec the current binary as the tray
/// helper process (busybox-style multi-call). Must not collide with any real
/// CLI subcommand and is intentionally excluded from clap/help output.
pub const TRAY_SUBCOMMAND: &str = "__tray";

#[derive(Parser, Debug, Clone)]
#[command(
    name = "bifrost __tray",
    about = "Internal system tray helper for Bifrost"
)]
pub struct TrayArgs {
    #[arg(long, help = "Bifrost data directory")]
    pub data_dir: PathBuf,

    #[arg(long, help = "Path to runtime.json")]
    pub runtime_file: PathBuf,

    #[arg(long, help = "Parent Bifrost process PID")]
    pub parent_pid: u32,

    #[arg(long, help = "Admin URL (optional, reduces initial probe)")]
    pub admin_url: Option<String>,

    #[arg(long, help = "Service listen port")]
    pub port: Option<u16>,

    #[arg(
        long,
        help = "Trusted path to the bifrost binary, used for start/stop/restart actions"
    )]
    pub bifrost_bin: Option<PathBuf>,

    #[arg(
        long,
        allow_hyphen_values = true,
        help = "Extra args to pass when restarting bifrost"
    )]
    pub start_args: Vec<String>,
}

impl TrayArgs {
    /// Parse tray args from an explicit argv slice (already stripped of the
    /// leading program name and the `__tray` token).
    pub fn parse_from_args<I, T>(args: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString> + Clone,
    {
        // Provide a synthetic program name so clap error/help messages read well.
        let mut full: Vec<std::ffi::OsString> = vec!["bifrost __tray".into()];
        full.extend(args.into_iter().map(Into::into));
        TrayArgs::parse_from(full)
    }
}

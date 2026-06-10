use std::path::PathBuf;

use clap::Parser;

#[derive(Parser, Debug, Clone)]
#[command(name = "bifrost-tray", about = "System tray helper for Bifrost")]
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

pub fn parse_args() -> TrayArgs {
    TrayArgs::parse()
}

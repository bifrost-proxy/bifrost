use bifrost_e2e::{Reporter, TestRunner, TestStatus};
use clap::Parser;
use std::process::ExitCode;
use std::time::Duration;

#[cfg(any(target_os = "windows", test))]
const WINDOWS_MAIN_THREAD_STACK_SIZE: usize = 8 * 1024 * 1024;

#[derive(Parser, Debug)]
#[command(name = "bifrost-e2e")]
#[command(about = "Bifrost E2E Test Runner", long_about = None)]
struct Args {
    #[arg(
        short,
        long,
        help = "Filter tests by category (routing, request, response, template, public)"
    )]
    category: Option<String>,

    #[arg(short, long, help = "Filter tests by name pattern")]
    test: Option<String>,

    #[arg(short, long, help = "List all available tests without running")]
    list: bool,

    #[arg(short, long, help = "Output JSON report to file")]
    output: Option<String>,

    #[arg(
        short,
        long,
        default_value = "18080",
        help = "Base port for proxy instances"
    )]
    port: u16,

    #[arg(
        short,
        long,
        help = "Number of concurrent test workers (default: 1, set >1 for parallel execution)"
    )]
    jobs: Option<usize>,

    #[arg(short, long, help = "Verbose output")]
    verbose: bool,

    #[arg(
        long,
        default_value = "0",
        env = "BIFROST_E2E_TIMEOUT",
        help = "Global timeout in seconds (0 = no timeout)"
    )]
    timeout: u64,

    #[arg(
        long,
        default_value = "120",
        env = "BIFROST_E2E_TEST_TIMEOUT",
        help = "Per-test timeout in seconds"
    )]
    test_timeout: u64,
}

async fn run(args: Args) -> ExitCode {
    let log_level = if args.verbose { "debug" } else { "info" };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(log_level));

    tracing_subscriber::fmt().with_env_filter(filter).init();

    let concurrency = args
        .jobs
        .or_else(|| {
            std::env::var("BIFROST_E2E_RUNNER_JOBS")
                .ok()
                .and_then(|v| v.parse().ok())
        })
        .unwrap_or(1);

    let reporter = Reporter::new(args.verbose);
    let global_timeout = if args.timeout > 0 {
        Some(Duration::from_secs(args.timeout))
    } else {
        None
    };
    let mut runner = TestRunner::new(args.port, reporter)
        .with_concurrency(concurrency)
        .with_global_timeout(global_timeout)
        .with_test_timeout(Duration::from_secs(args.test_timeout));

    runner.load_all_tests();

    if let Some(ref category) = args.category {
        runner.filter_by_category(category);
    }

    if let Some(ref pattern) = args.test {
        runner.filter_by_name(pattern);
    }

    if args.list {
        println!("\nAvailable tests:\n");
        for (category, tests) in runner.list_tests() {
            println!("  [{}]", category);
            for test in tests {
                println!("    - {}", test);
            }
            println!();
        }
        return ExitCode::SUCCESS;
    }

    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║           Bifrost E2E Test Runner                            ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    if concurrency > 1 {
        println!("  Concurrency: {} workers\n", concurrency);
    }

    let results = runner.run_all().await;

    let passed = results
        .iter()
        .filter(|r| r.status == TestStatus::Passed)
        .count();
    let failed = results
        .iter()
        .filter(|r| r.status == TestStatus::Failed)
        .count();
    let skipped = results
        .iter()
        .filter(|r| r.status == TestStatus::Skipped)
        .count();
    let total = results.len();

    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║                      Test Summary                            ║");
    println!("╠══════════════════════════════════════════════════════════════╣");
    println!(
        "║  Total:   {:4}                                               ║",
        total
    );
    println!(
        "║  Passed:  {:4}  ✓                                            ║",
        passed
    );
    println!(
        "║  Failed:  {:4}  ✗                                            ║",
        failed
    );
    println!(
        "║  Skipped: {:4}  ○                                            ║",
        skipped
    );
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    if let Some(output_path) = args.output {
        match runner.reporter().export_json(&results, &output_path) {
            Ok(_) => println!("Report saved to: {}", output_path),
            Err(e) => eprintln!("Failed to save report: {}", e),
        }
    }

    if failed > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(not(target_os = "windows"))]
#[tokio::main]
async fn main() -> ExitCode {
    run(Args::parse()).await
}

#[cfg(target_os = "windows")]
fn main() -> ExitCode {
    let args = Args::parse();

    match std::thread::Builder::new()
        .name("bifrost-e2e-main".to_string())
        .stack_size(WINDOWS_MAIN_THREAD_STACK_SIZE)
        .spawn(move || {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map(|runtime| runtime.block_on(run(args)))
        }) {
        Ok(handle) => match handle.join() {
            Ok(Ok(code)) => code,
            Ok(Err(error)) => {
                eprintln!("Failed to initialize tokio runtime: {error}");
                ExitCode::FAILURE
            }
            Err(_) => {
                eprintln!("bifrost-e2e main thread panicked");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("Failed to spawn bifrost-e2e main thread: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::WINDOWS_MAIN_THREAD_STACK_SIZE;

    #[test]
    fn windows_main_thread_stack_is_increased() {
        let stack_size = std::hint::black_box(WINDOWS_MAIN_THREAD_STACK_SIZE);
        assert!(stack_size >= 8 * 1024 * 1024);
    }
}

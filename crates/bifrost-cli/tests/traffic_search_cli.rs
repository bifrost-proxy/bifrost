#![cfg(unix)]

use std::path::Path;
use std::process::Command;

#[test]
fn traffic_search_cli_matrix_runs_against_real_proxy() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root");
    let output = Command::new("python3")
        .arg(root.join("e2e-tests/tests/test_traffic_search_matrix.py"))
        .current_dir(root)
        .env("SKIP_BUILD", "true")
        .env("BIFROST_BIN", env!("CARGO_BIN_EXE_bifrost"))
        .env_remove("RESULT_FILE")
        .output()
        .expect("run traffic/search CLI matrix");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "traffic/search CLI matrix failed: {}\n{stdout}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("SUMMARY passed="), "{stdout}");
    assert!(stdout.contains("failed=0"), "{stdout}");
}

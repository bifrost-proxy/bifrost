use std::path::PathBuf;
use std::process::Command;

#[test]
fn client_admin_cli_runs_against_a_real_lan_listener() {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("bifrost-cli must be inside the workspace crates directory")
        .to_path_buf();
    let script = repo_root.join("e2e-tests/tests/test_client_admin_cli.sh");

    let output = Command::new("bash")
        .arg(script)
        .current_dir(&repo_root)
        .env("SKIP_BUILD", "true")
        .env("BIFROST_BIN", env!("CARGO_BIN_EXE_bifrost"))
        .output()
        .expect("run Client Admin CLI E2E");

    assert!(
        output.status.success(),
        "Client Admin CLI E2E failed (status={}):\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("Client Admin CLI E2E passed"),
        "Client Admin CLI E2E did not complete every assertion: {}",
        String::from_utf8_lossy(&output.stdout),
    );
}

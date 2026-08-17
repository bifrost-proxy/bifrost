#[cfg(unix)]
#[test]
fn isolated_external_runner_stop_interrupts_native_transport() {
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("resolve repository root");
    let script = repo_root.join("e2e-tests/tests/test_external_runner_worker_stop.sh");

    let output = std::process::Command::new("bash")
        .arg(&script)
        .current_dir(&repo_root)
        .env("BIFROST_BIN", env!("CARGO_BIN_EXE_bifrost"))
        .env("SKIP_BUILD", "true")
        .output()
        .expect("run isolated external runner stop E2E");

    assert!(
        output.status.success(),
        "isolated external runner stop E2E failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("[external-runner-worker-stop] PASS"),
        "E2E did not emit its completion marker\nstdout:\n{}",
        String::from_utf8_lossy(&output.stdout),
    );
}

use std::process::Command;

use bifrost_core::rule_share::{append_rule_share_query, new_rule_share_payload};

#[test]
fn verify_cli_reports_json_errors_without_initializing_storage_or_logs() {
    let dir = tempfile::tempdir().unwrap();
    let payload = new_rule_share_payload("verify", "example.test status://200").unwrap();
    let url = append_rule_share_query("https://example.test/app?site=1", &payload).unwrap();
    let invoke = |url: &str, json: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_bifrost"));
        command
            .env("BIFROST_DATA_DIR", dir.path())
            .env("BIFROST_INTERNAL_CLIENT_BASE_URL", "http://127.0.0.1:1")
            .env("BIFROST_FORCE_UPDATE_CHECK", "1")
            .args(["rule", "verify", url]);
        if json {
            command.arg("--json");
        }
        command.output().unwrap()
    };
    let success = invoke(&url, true);
    assert!(success.status.success(), "{:?}", success);
    let report: serde_json::Value = serde_json::from_slice(&success.stdout).unwrap();
    assert_eq!(report["valid"], true);
    assert_eq!(report["target_url"], "https://example.test/app?site=1");
    let text = invoke(&url, false);
    assert!(text.status.success());
    assert!(String::from_utf8_lossy(&text.stdout).contains("Valid rule share link"));
    for invalid in [
        "https://example.test/",
        "https://example.test/?__bifrost_rule=!",
    ] {
        let failure = invoke(invalid, true);
        assert!(!failure.status.success());
        let report: serde_json::Value = serde_json::from_slice(&failure.stdout).unwrap();
        assert_eq!(report["valid"], false);
        assert!(!report["error"].as_str().unwrap().is_empty());
        assert!(report["next_action"]
            .as_str()
            .unwrap()
            .contains("regenerate"));
        assert!(String::from_utf8_lossy(&failure.stderr).contains("Error:"));
    }
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

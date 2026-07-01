use crate::assertions::assert_status;
use crate::{ProxyInstance, TestCase};
use std::net::TcpListener;

pub fn get_all_tests() -> Vec<TestCase> {
    vec![
        TestCase::standalone(
            "brute_force_lockout_after_max_failures",
            "Validate repeated failed logins are throttled without disabling remote access or clearing the password",
            "admin",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, admin_state) =
                    ProxyInstance::start_with_admin(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy with admin: {}", e))?;

                bifrost_admin::set_remote_access_enabled(&admin_state, true)
                    .map_err(|e| format!("Failed to enable remote access: {}", e))?;
                bifrost_admin::set_admin_password_hash(&admin_state, "goodpass1")
                    .map_err(|e| format!("Failed to set password: {}", e))?;

                let client = reqwest::Client::builder()
                    .danger_accept_invalid_certs(true)
                    .no_proxy()
                    .build()
                    .map_err(|e| format!("Failed to create client: {}", e))?;

                for i in 1..=4 {
                    let resp = client
                        .post(format!(
                            "http://127.0.0.1:{}/_bifrost/api/auth/login",
                            port
                        ))
                        .json(&serde_json::json!({"username": "admin", "password": "wrong"}))
                        .send()
                        .await
                        .map_err(|e| format!("Login attempt {} failed: {}", i, e))?;
                    assert_status(&resp, 401)?;
                    let body: serde_json::Value = resp
                        .json()
                        .await
                        .map_err(|e| format!("Failed to parse response: {}", e))?;
                    if body.get("error").is_none() {
                        return Err(format!("Missing error field in attempt {}", i));
                    }
                }

                let resp = client
                    .post(format!(
                        "http://127.0.0.1:{}/_bifrost/api/auth/login",
                        port
                    ))
                    .json(&serde_json::json!({"username": "admin", "password": "wrong"}))
                    .send()
                    .await
                    .map_err(|e| format!("5th login attempt failed: {}", e))?;
                assert_status(&resp, 403)?;
                let body: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse lockout response: {}", e))?;
                let locked = body
                    .get("locked_out")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if !locked {
                    return Err("Expected locked_out=true in response".to_string());
                }

                let status_resp = client
                    .get(format!(
                        "http://127.0.0.1:{}/_bifrost/api/auth/status",
                        port
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("Status request failed: {}", e))?;
                assert_status(&status_resp, 200)?;
                let status: serde_json::Value = status_resp
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse status: {}", e))?;
                let remote_enabled = status
                    .get("remote_access_enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                if !remote_enabled {
                    return Err(
                        "Expected remote_access_enabled=true after non-destructive throttle"
                            .to_string(),
                    );
                }
                let has_password = status
                    .get("has_password")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if !has_password {
                    return Err("Expected has_password=true after non-destructive throttle".to_string());
                }

                let recovery_resp = client
                    .post(format!(
                        "http://127.0.0.1:{}/_bifrost/api/auth/login",
                        port
                    ))
                    .json(&serde_json::json!({"username": "admin", "password": "goodpass1"}))
                    .send()
                    .await
                    .map_err(|e| format!("Recovery login failed: {}", e))?;
                assert_status(&recovery_resp, 200)?;

                Ok(())
            },
        ),
        TestCase::standalone(
            "brute_force_password_strength_validation",
            "Validate password strength requirements: min 6 chars, letters + digits",
            "admin",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, _admin_state) =
                    ProxyInstance::start_with_admin(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy with admin: {}", e))?;

                let client = reqwest::Client::builder()
                    .danger_accept_invalid_certs(true)
                    .no_proxy()
                    .build()
                    .map_err(|e| format!("Failed to create client: {}", e))?;

                let resp = client
                    .post(format!(
                        "http://127.0.0.1:{}/_bifrost/api/auth/passwd",
                        port
                    ))
                    .json(&serde_json::json!({"password": "ab1"}))
                    .send()
                    .await
                    .map_err(|e| format!("Short password request failed: {}", e))?;
                assert_status(&resp, 400)?;

                let resp = client
                    .post(format!(
                        "http://127.0.0.1:{}/_bifrost/api/auth/passwd",
                        port
                    ))
                    .json(&serde_json::json!({"password": "123456"}))
                    .send()
                    .await
                    .map_err(|e| format!("Digits-only password request failed: {}", e))?;
                assert_status(&resp, 400)?;

                let resp = client
                    .post(format!(
                        "http://127.0.0.1:{}/_bifrost/api/auth/passwd",
                        port
                    ))
                    .json(&serde_json::json!({"password": "abcdef"}))
                    .send()
                    .await
                    .map_err(|e| format!("Letters-only password request failed: {}", e))?;
                assert_status(&resp, 400)?;

                let resp = client
                    .post(format!(
                        "http://127.0.0.1:{}/_bifrost/api/auth/passwd",
                        port
                    ))
                    .json(&serde_json::json!({"password": "good1pass"}))
                    .send()
                    .await
                    .map_err(|e| format!("Valid password request failed: {}", e))?;
                assert_status(&resp, 200)?;

                Ok(())
            },
        ),
        TestCase::standalone(
            "brute_force_successful_login_resets_count",
            "Validate successful login resets the failed attempt counter",
            "admin",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, admin_state) =
                    ProxyInstance::start_with_admin(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy with admin: {}", e))?;

                bifrost_admin::set_remote_access_enabled(&admin_state, true)
                    .map_err(|e| format!("Failed to enable remote access: {}", e))?;
                bifrost_admin::set_admin_password_hash(&admin_state, "correct1pass")
                    .map_err(|e| format!("Failed to set password: {}", e))?;

                let client = reqwest::Client::builder()
                    .danger_accept_invalid_certs(true)
                    .no_proxy()
                    .build()
                    .map_err(|e| format!("Failed to create client: {}", e))?;

                for _ in 0..3 {
                    let resp = client
                        .post(format!(
                            "http://127.0.0.1:{}/_bifrost/api/auth/login",
                            port
                        ))
                        .json(&serde_json::json!({"username": "admin", "password": "wrong"}))
                        .send()
                        .await
                        .map_err(|e| format!("Failed login attempt: {}", e))?;
                    assert_status(&resp, 401)?;
                }

                let status = client
                    .get(format!(
                        "http://127.0.0.1:{}/_bifrost/api/auth/status",
                        port
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("Status request failed: {}", e))?;
                let status_json: serde_json::Value = status
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse status: {}", e))?;
                let failed = status_json
                    .get("failed_attempts")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                if failed != 3 {
                    return Err(format!("Expected 3 failed attempts, got {}", failed));
                }

                let resp = client
                    .post(format!(
                        "http://127.0.0.1:{}/_bifrost/api/auth/login",
                        port
                    ))
                    .json(&serde_json::json!({"username": "admin", "password": "correct1pass"}))
                    .send()
                    .await
                    .map_err(|e| format!("Correct login failed: {}", e))?;
                assert_status(&resp, 200)?;

                let status2 = client
                    .get(format!(
                        "http://127.0.0.1:{}/_bifrost/api/auth/status",
                        port
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("Status request 2 failed: {}", e))?;
                let status2_json: serde_json::Value = status2
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse status 2: {}", e))?;
                let failed2 = status2_json
                    .get("failed_attempts")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(99);
                if failed2 != 0 {
                    return Err(format!(
                        "Expected 0 failed attempts after success, got {}",
                        failed2
                    ));
                }

                Ok(())
            },
        ),
        TestCase::standalone(
            "brute_force_auth_status_shows_lockout_fields",
            "Validate auth status response includes lockout-related fields",
            "admin",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, _admin_state) =
                    ProxyInstance::start_with_admin(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy with admin: {}", e))?;

                let client = reqwest::Client::builder()
                    .danger_accept_invalid_certs(true)
                    .no_proxy()
                    .build()
                    .map_err(|e| format!("Failed to create client: {}", e))?;

                let resp = client
                    .get(format!(
                        "http://127.0.0.1:{}/_bifrost/api/auth/status",
                        port
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("Status request failed: {}", e))?;
                assert_status(&resp, 200)?;
                let json: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse status: {}", e))?;

                if json.get("locked_out").is_none() {
                    return Err("Missing 'locked_out' field in auth status".to_string());
                }
                if json.get("failed_attempts").is_none() {
                    return Err("Missing 'failed_attempts' field in auth status".to_string());
                }
                if json.get("max_attempts").is_none() {
                    return Err("Missing 'max_attempts' field in auth status".to_string());
                }
                if json.get("min_password_length").is_none() {
                    return Err("Missing 'min_password_length' field in auth status".to_string());
                }

                let max = json
                    .get("max_attempts")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                if max != 5 {
                    return Err(format!("Expected max_attempts=5, got {}", max));
                }
                let min_pwd = json
                    .get("min_password_length")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                if min_pwd != 6 {
                    return Err(format!("Expected min_password_length=6, got {}", min_pwd));
                }

                Ok(())
            },
        ),
    ]
}

fn pick_unused_port() -> Result<u16, String> {
    TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("Failed to bind ephemeral port: {}", e))?
        .local_addr()
        .map(|addr| addr.port())
        .map_err(|e| format!("Failed to read ephemeral port: {}", e))
}

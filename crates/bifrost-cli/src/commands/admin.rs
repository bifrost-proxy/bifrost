use bifrost_admin::admin_auth_db::AuthDb;
use bifrost_core::{text::truncate_chars_with_suffix, BifrostError, Result};
use dialoguer::{theme::ColorfulTheme, Password};

use crate::cli::{AdminCommands, AdminRemoteCommands};

fn open_auth_db() -> Result<AuthDb> {
    AuthDb::open_default()
}

fn ensure_username(db: &AuthDb, username: &str) -> Result<()> {
    let u = username.trim();
    if u.is_empty() {
        return Err(BifrostError::Config("Username cannot be empty".to_string()));
    }
    db.set_username(u)?;
    Ok(())
}

fn prompt_new_password() -> std::result::Result<String, dialoguer::Error> {
    Password::with_theme(&ColorfulTheme::default())
        .with_prompt("New admin password")
        .with_confirmation("Confirm password", "Passwords do not match")
        .allow_empty_password(false)
        .interact()
}

fn read_password_from_stdin() -> Result<String> {
    use std::io::Read;

    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| BifrostError::Config(format!("Failed to read stdin: {e}")))?;
    let pwd = buf.trim_end_matches(['\n', '\r']).to_string();
    if pwd.is_empty() {
        return Err(BifrostError::Config("Password cannot be empty".to_string()));
    }
    Ok(pwd)
}

fn set_password(db: &AuthDb, password: &str) -> Result<()> {
    bifrost_admin::validate_password_strength(password)?;
    let hashed = bcrypt::hash(password, bcrypt::DEFAULT_COST)
        .map_err(|e| BifrostError::Storage(format!("Failed to hash password: {e}")))?;
    db.set_password_hash(&hashed)?;
    db.reset_failed_count()?;
    Ok(())
}

pub fn handle_admin_command(action: AdminCommands) -> Result<()> {
    match action {
        AdminCommands::Remote { action } => {
            let db = open_auth_db()?;
            match action {
                AdminRemoteCommands::Enable => {
                    if !db.has_password() {
                        println!(
                            "Admin password not set yet. Please set a password to enable remote access."
                        );
                        let pwd = prompt_new_password().map_err(|e| {
                            BifrostError::Config(format!("Failed to read password: {e}"))
                        })?;
                        ensure_username(&db, "admin")?;
                        set_password(&db, &pwd)?;
                    }
                    db.set_remote_access_enabled(true)?;
                    println!("Remote admin access: enabled");
                }
                AdminRemoteCommands::Disable => {
                    db.set_remote_access_enabled(false)?;
                    println!("Remote admin access: disabled");
                }
                AdminRemoteCommands::Status => {
                    let enabled = db.is_remote_access_enabled();
                    let username = db.get_username().unwrap_or_else(|| "admin".to_string());
                    let failed_count = db.get_failed_count();
                    println!(
                        "Remote admin access: {}",
                        if enabled { "enabled" } else { "disabled" }
                    );
                    println!("Admin username: {}", username.trim());
                    println!(
                        "Admin password: {}",
                        if db.has_password() { "set" } else { "not set" }
                    );
                    println!(
                        "Failed login attempts: {}/{}",
                        failed_count,
                        bifrost_admin::MAX_LOGIN_ATTEMPTS
                    );
                    if failed_count >= bifrost_admin::MAX_LOGIN_ATTEMPTS {
                        println!("⚠️  Account locked out due to brute-force protection. Re-set password to unlock.");
                    }
                    println!(
                        "Audit DB: {}",
                        bifrost_admin::admin_audit::audit_db_path()?.display()
                    );
                }
            }
        }
        AdminCommands::Passwd {
            username,
            password_stdin,
        } => {
            let db = open_auth_db()?;
            ensure_username(&db, &username)?;

            let pwd = if password_stdin {
                read_password_from_stdin()?
            } else {
                prompt_new_password()
                    .map_err(|e| BifrostError::Config(format!("Failed to read password: {e}")))?
            };
            set_password(&db, &pwd)?;
            println!("Admin password updated.");
        }
        AdminCommands::RevokeAll => {
            let db = open_auth_db()?;
            let ts = chrono::Utc::now().timestamp();
            db.set_revoke_before(ts)?;
            println!("All admin sessions revoked (revoke_before={}).", ts);
        }
        AdminCommands::Audit {
            limit,
            offset,
            json,
        } => {
            let limit = limit.clamp(1, 500);
            let total = bifrost_admin::admin_audit::count_logins()?;
            let items = bifrost_admin::admin_audit::list_logins(limit, offset)?;
            if json {
                let out = serde_json::json!({
                    "total": total,
                    "limit": limit,
                    "offset": offset,
                    "items": items,
                });
                println!(
                    "{}",
                    serde_json::to_string_pretty(&out).unwrap_or_else(|_| out.to_string())
                );
                return Ok(());
            }

            if items.is_empty() {
                println!("No audit records.");
                println!(
                    "Audit DB: {}",
                    bifrost_admin::admin_audit::audit_db_path()?.display()
                );
                return Ok(());
            }

            println!(
                "Admin login audit (total: {}, showing: {}):",
                total,
                items.len()
            );
            println!("====================================================");
            for item in items {
                let ts = chrono::DateTime::<chrono::Utc>::from_timestamp(item.ts, 0)
                    .map(|t| t.to_rfc3339())
                    .unwrap_or_else(|| item.ts.to_string());
                let ua_preview = truncate_chars_with_suffix(&item.ua, 120, "...");
                println!(
                    "- id={} ts={} user={} ip={} ua={}",
                    item.id, ts, item.username, item.ip, ua_preview
                );
            }
            println!(
                "Audit DB: {}",
                bifrost_admin::admin_audit::audit_db_path()?.display()
            );
        }
    }

    Ok(())
}

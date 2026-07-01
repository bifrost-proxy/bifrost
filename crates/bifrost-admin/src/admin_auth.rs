use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bcrypt::{hash, verify, DEFAULT_COST};
use bifrost_core::{BifrostError, Result};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use rand::{distributions::Alphanumeric, Rng};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::admin_auth_db::AuthDb;
use crate::state::AdminState;

pub const MAX_LOGIN_ATTEMPTS: u32 = 5;
pub const MIN_PASSWORD_LENGTH: usize = 6;

const JWT_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminJwtClaims {
    pub sub: String,
    pub iat: i64,
    pub exp: i64,
    pub jti: String,
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs() as i64
}

fn require_auth_db(state: &AdminState) -> Result<&AuthDb> {
    state
        .auth_db
        .as_ref()
        .map(|db| db.as_ref())
        .ok_or_else(|| BifrostError::Config("Auth database not configured".to_string()))
}

pub fn is_remote_access_enabled(state: &AdminState) -> bool {
    state
        .auth_db
        .as_ref()
        .map(|db| db.is_remote_access_enabled())
        .unwrap_or(false)
}

pub fn get_admin_username(state: &AdminState) -> Option<String> {
    let db = state.auth_db.as_ref()?;
    db.get_username().or_else(|| Some("admin".to_string()))
}

pub fn ensure_admin_auth_material(state: &AdminState) -> Result<()> {
    let db = require_auth_db(state)?;

    if db.get_username().is_none() {
        db.set_username("admin")?;
    }

    if db.get_jwt_secret().is_none() {
        let secret: String = rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(64)
            .map(char::from)
            .collect();
        db.set_jwt_secret(&secret)?;
    }

    Ok(())
}

pub fn validate_password_strength(password: &str) -> Result<()> {
    if password.is_empty() {
        return Err(BifrostError::Config("Password cannot be empty".to_string()));
    }
    if password.len() < MIN_PASSWORD_LENGTH {
        return Err(BifrostError::Config(format!(
            "Password must be at least {} characters",
            MIN_PASSWORD_LENGTH
        )));
    }
    let has_letter = password.chars().any(|c| c.is_alphabetic());
    let has_digit = password.chars().any(|c| c.is_ascii_digit());
    if !has_letter || !has_digit {
        return Err(BifrostError::Config(
            "Password must contain both letters and digits".to_string(),
        ));
    }
    Ok(())
}

pub fn set_admin_password_hash(state: &AdminState, password: &str) -> Result<()> {
    let db = require_auth_db(state)?;
    validate_password_strength(password)?;
    let hashed = hash(password, DEFAULT_COST)
        .map_err(|e| BifrostError::Storage(format!("Failed to hash password: {e}")))?;
    db.set_password_hash(&hashed)?;
    Ok(())
}

pub fn set_remote_access_enabled(state: &AdminState, enabled: bool) -> Result<()> {
    let db = require_auth_db(state)?;
    db.set_remote_access_enabled(enabled)?;
    Ok(())
}

pub fn set_admin_username(state: &AdminState, username: &str) -> Result<()> {
    let db = require_auth_db(state)?;
    let trimmed = username.trim();
    if trimmed.is_empty() {
        return Err(BifrostError::Config("Username cannot be empty".to_string()));
    }
    db.set_username(trimmed)?;
    Ok(())
}

pub fn has_admin_password(state: &AdminState) -> bool {
    state
        .auth_db
        .as_ref()
        .map(|db| db.has_password())
        .unwrap_or(false)
}

pub fn get_failed_login_count(state: &AdminState) -> u32 {
    state
        .auth_db
        .as_ref()
        .map(|db| db.get_failed_count())
        .unwrap_or(0)
}

pub fn record_failed_login(state: &AdminState) -> Result<u32> {
    let db = require_auth_db(state)?;
    let new_count = db.increment_failed_count()?;

    if new_count >= MAX_LOGIN_ATTEMPTS {
        warn!(
            failed_count = new_count,
            "Login attempts exhausted; applying non-destructive throttling"
        );
    }
    Ok(new_count)
}

pub fn reset_failed_login_count(state: &AdminState) -> Result<()> {
    let db = require_auth_db(state)?;
    db.reset_failed_count()?;
    Ok(())
}

pub fn verify_admin_credentials(
    state: &AdminState,
    username: &str,
    password: &str,
) -> Result<bool> {
    let db = require_auth_db(state)?;
    let expected_username = db.get_username().unwrap_or_else(|| "admin".to_string());
    if username.trim() != expected_username.trim() {
        return Ok(false);
    }
    let Some(hash_str) = db.get_password_hash() else {
        return Ok(false);
    };
    let ok = verify(password, &hash_str)
        .map_err(|e| BifrostError::Storage(format!("Failed to verify password: {e}")))?;
    Ok(ok)
}

fn jwt_secret(state: &AdminState) -> Result<String> {
    let db = require_auth_db(state)?;
    let secret = db
        .get_jwt_secret()
        .ok_or_else(|| BifrostError::Config("JWT secret not initialized".to_string()))?;
    if secret.is_empty() {
        return Err(BifrostError::Config("JWT secret is empty".to_string()));
    }
    Ok(secret)
}

pub fn issue_admin_jwt(state: &AdminState, username: &str) -> Result<(String, AdminJwtClaims)> {
    ensure_admin_auth_material(state)?;
    let now = now_unix();
    let exp = now + JWT_TTL.as_secs() as i64;
    let claims = AdminJwtClaims {
        sub: username.to_string(),
        iat: now,
        exp,
        jti: uuid::Uuid::new_v4().to_string(),
    };

    let secret = jwt_secret(state)?;
    let mut header = Header::new(Algorithm::HS256);
    header.typ = Some("JWT".to_string());
    let token = jsonwebtoken::encode(
        &header,
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| BifrostError::Storage(format!("Failed to encode JWT: {e}")))?;
    Ok((token, claims))
}

pub fn revoke_all_admin_sessions(state: &AdminState) -> Result<()> {
    let db = require_auth_db(state)?;
    let ts = now_unix();
    db.set_revoke_before(ts)?;
    Ok(())
}

pub fn validate_admin_jwt(state: &AdminState, token: &str) -> Result<AdminJwtClaims> {
    ensure_admin_auth_material(state)?;
    let secret = jwt_secret(state)?;

    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = true;
    validation.leeway = 5;

    let data = jsonwebtoken::decode::<AdminJwtClaims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .map_err(|e| BifrostError::Proxy(format!("Invalid token: {e}")))?;

    let claims = data.claims;

    if claims.exp <= claims.iat {
        return Err(BifrostError::Proxy("Invalid token timestamps".to_string()));
    }
    let ttl = claims.exp - claims.iat;
    if ttl > JWT_TTL.as_secs() as i64 {
        return Err(BifrostError::Proxy(
            "Token lifetime exceeds 7 days".to_string(),
        ));
    }

    if let Some(db) = state.auth_db.as_ref() {
        if let Some(revoke_before) = db.get_revoke_before() {
            if claims.iat <= revoke_before {
                return Err(BifrostError::Proxy("Token revoked".to_string()));
            }
        }
    }

    Ok(claims)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admin_auth_db::AuthDb;

    fn new_state() -> (AdminState, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let auth_db_path = tmp.path().join("auth.db");
        let auth_db = AuthDb::open(&auth_db_path).expect("auth db");

        (AdminState::new(19999).with_auth_db(auth_db), tmp)
    }

    fn encode_with_secret(secret: &str, claims: &AdminJwtClaims) -> String {
        let mut header = Header::new(Algorithm::HS256);
        header.typ = Some("JWT".to_string());
        jsonwebtoken::encode(
            &header,
            claims,
            &EncodingKey::from_secret(secret.as_bytes()),
        )
        .expect("encode jwt")
    }

    #[test]
    fn test_issue_admin_jwt_sets_7d_ttl_and_round_trip_validate() {
        let (state, _tmp) = new_state();
        let (token, claims) = issue_admin_jwt(&state, "admin").expect("issue token");
        assert!(claims.exp > claims.iat);
        assert_eq!(claims.exp - claims.iat, JWT_TTL.as_secs() as i64);

        let parsed = validate_admin_jwt(&state, &token).expect("validate token");
        assert_eq!(parsed.sub, "admin");
        assert_eq!(parsed.iat, claims.iat);
        assert_eq!(parsed.exp, claims.exp);
        assert_eq!(parsed.jti, claims.jti);
    }

    #[test]
    fn test_validate_admin_jwt_rejects_invalid_timestamps_and_too_long_ttl() {
        let (state, _tmp) = new_state();
        ensure_admin_auth_material(&state).expect("init auth material");
        let secret = state
            .auth_db
            .as_ref()
            .unwrap()
            .get_jwt_secret()
            .expect("jwt secret");

        let now = now_unix();

        let claims = AdminJwtClaims {
            sub: "admin".to_string(),
            iat: now + 120,
            exp: now + 60,
            jti: uuid::Uuid::new_v4().to_string(),
        };
        let token = encode_with_secret(&secret, &claims);
        let err = validate_admin_jwt(&state, &token).unwrap_err().to_string();
        assert!(err.contains("Invalid token timestamps"));

        let claims = AdminJwtClaims {
            sub: "admin".to_string(),
            iat: now,
            exp: now + (JWT_TTL.as_secs() as i64) + 60,
            jti: uuid::Uuid::new_v4().to_string(),
        };
        let token = encode_with_secret(&secret, &claims);
        let err = validate_admin_jwt(&state, &token).unwrap_err().to_string();
        assert!(err.contains("Token lifetime exceeds 7 days"));
    }

    #[test]
    fn test_validate_admin_jwt_respects_revoke_before() {
        let (state, _tmp) = new_state();
        ensure_admin_auth_material(&state).expect("init auth material");
        let secret = state
            .auth_db
            .as_ref()
            .unwrap()
            .get_jwt_secret()
            .expect("jwt secret");

        let now = now_unix();
        state
            .auth_db
            .as_ref()
            .unwrap()
            .set_revoke_before(now)
            .expect("set revoke_before");

        let claims = AdminJwtClaims {
            sub: "admin".to_string(),
            iat: now,
            exp: now + 60,
            jti: uuid::Uuid::new_v4().to_string(),
        };
        let token = encode_with_secret(&secret, &claims);
        let err = validate_admin_jwt(&state, &token).unwrap_err().to_string();
        assert!(err.contains("Token revoked"));
    }

    #[test]
    fn test_set_admin_password_hash_and_verify_credentials() {
        let (state, _tmp) = new_state();
        assert!(!has_admin_password(&state));

        set_admin_password_hash(&state, "testpass123").expect("set password");
        assert!(has_admin_password(&state));

        let ok = verify_admin_credentials(&state, "admin", "testpass123").expect("verify");
        assert!(ok);

        let wrong = verify_admin_credentials(&state, "admin", "wrongpass").expect("verify wrong");
        assert!(!wrong);
    }

    #[test]
    fn test_set_admin_password_rejects_empty() {
        let (state, _tmp) = new_state();
        let err = set_admin_password_hash(&state, "");
        assert!(err.is_err());
    }

    #[test]
    fn test_set_and_get_admin_username() {
        let (state, _tmp) = new_state();
        set_admin_username(&state, "myuser").expect("set username");
        let username = get_admin_username(&state);
        assert_eq!(username, Some("myuser".to_string()));
    }

    #[test]
    fn test_set_admin_username_rejects_empty() {
        let (state, _tmp) = new_state();
        let err = set_admin_username(&state, "   ");
        assert!(err.is_err());
    }

    #[test]
    fn test_set_remote_access_enabled_toggle() {
        let (state, _tmp) = new_state();
        assert!(!is_remote_access_enabled(&state));

        set_remote_access_enabled(&state, true).expect("enable");
        assert!(is_remote_access_enabled(&state));

        set_remote_access_enabled(&state, false).expect("disable");
        assert!(!is_remote_access_enabled(&state));
    }

    #[test]
    fn test_revoke_all_invalidates_existing_tokens() {
        let (state, _tmp) = new_state();
        let (token, _claims) = issue_admin_jwt(&state, "admin").expect("issue token");
        validate_admin_jwt(&state, &token).expect("token should be valid before revoke");

        std::thread::sleep(std::time::Duration::from_secs(1));

        revoke_all_admin_sessions(&state).expect("revoke all");
        let err = validate_admin_jwt(&state, &token).unwrap_err().to_string();
        assert!(err.contains("Token revoked"));
    }

    #[test]
    fn test_has_admin_password_with_custom_username_and_credentials() {
        let (state, _tmp) = new_state();
        set_admin_username(&state, "operator").expect("set username");
        set_admin_password_hash(&state, "secure1pass").expect("set password");

        assert!(has_admin_password(&state));
        let ok = verify_admin_credentials(&state, "operator", "secure1pass").expect("verify");
        assert!(ok);

        let wrong_user =
            verify_admin_credentials(&state, "admin", "secure1pass").expect("wrong user");
        assert!(!wrong_user);
    }

    #[test]
    fn test_password_strength_rejects_empty() {
        let err = validate_password_strength("").unwrap_err().to_string();
        assert!(err.contains("cannot be empty"));
    }

    #[test]
    fn test_password_strength_rejects_short() {
        let err = validate_password_strength("ab1").unwrap_err().to_string();
        assert!(err.contains("at least"));
    }

    #[test]
    fn test_password_strength_rejects_digits_only() {
        let err = validate_password_strength("123456")
            .unwrap_err()
            .to_string();
        assert!(err.contains("letters and digits"));
    }

    #[test]
    fn test_password_strength_rejects_letters_only() {
        let err = validate_password_strength("abcdef")
            .unwrap_err()
            .to_string();
        assert!(err.contains("letters and digits"));
    }

    #[test]
    fn test_password_strength_accepts_valid() {
        validate_password_strength("abc123").expect("should pass");
        validate_password_strength("Test99").expect("should pass");
        validate_password_strength("longpassword1").expect("should pass");
    }

    #[test]
    fn test_record_failed_login_increments_count() {
        let (state, _tmp) = new_state();
        assert_eq!(get_failed_login_count(&state), 0);

        let count = record_failed_login(&state).expect("record fail");
        assert_eq!(count, 1);
        assert_eq!(get_failed_login_count(&state), 1);

        let count = record_failed_login(&state).expect("record fail");
        assert_eq!(count, 2);
        assert_eq!(get_failed_login_count(&state), 2);
    }

    #[test]
    fn test_reset_failed_login_count_works() {
        let (state, _tmp) = new_state();
        record_failed_login(&state).expect("record fail");
        record_failed_login(&state).expect("record fail");
        assert_eq!(get_failed_login_count(&state), 2);

        reset_failed_login_count(&state).expect("reset");
        assert_eq!(get_failed_login_count(&state), 0);
    }

    #[test]
    fn test_failed_login_limit_preserves_remote_and_password() {
        let (state, _tmp) = new_state();
        set_admin_password_hash(&state, "pass123abc").expect("set password");
        set_remote_access_enabled(&state, true).expect("enable remote");
        assert!(has_admin_password(&state));
        assert!(is_remote_access_enabled(&state));

        for i in 0..MAX_LOGIN_ATTEMPTS {
            let count = record_failed_login(&state).expect("record fail");
            assert_eq!(count, i + 1);
        }

        assert!(is_remote_access_enabled(&state));
        assert!(has_admin_password(&state));
        assert_eq!(get_failed_login_count(&state), MAX_LOGIN_ATTEMPTS);
    }

    #[test]
    fn test_failed_login_limit_allows_successful_recovery_without_local_reset() {
        let (state, _tmp) = new_state();
        set_admin_password_hash(&state, "pass123abc").expect("set password");
        set_remote_access_enabled(&state, true).expect("enable remote");

        for _ in 0..MAX_LOGIN_ATTEMPTS {
            let _ = record_failed_login(&state);
        }
        assert!(is_remote_access_enabled(&state));
        assert!(has_admin_password(&state));
        assert!(verify_admin_credentials(&state, "admin", "pass123abc").expect("verify"));

        reset_failed_login_count(&state).expect("reset");
        assert!(has_admin_password(&state));
        assert!(is_remote_access_enabled(&state));
        assert_eq!(get_failed_login_count(&state), 0);
    }

    #[test]
    fn test_set_admin_password_rejects_weak_password() {
        let (state, _tmp) = new_state();
        let err = set_admin_password_hash(&state, "abc");
        assert!(err.is_err());
        let err = set_admin_password_hash(&state, "123456");
        assert!(err.is_err());
    }
}

pub fn json_filter(input: &str) -> Result<String, String> {
    let (path, _) = input.split_once('=').ok_or("expected PATH=VALUE")?;
    let path = path.trim();
    let normalized = if path.starts_with('$') {
        path.to_string()
    } else {
        format!("$.{path}")
    };
    if !bifrost_admin::search::is_valid_json_path(&normalized) {
        return Err("invalid JSONPath; use $, .member, [index], or [*]".to_string());
    }
    Ok(input.to_string())
}

pub fn header_filter(input: &str) -> Result<String, String> {
    let (name, _) = input.split_once('=').ok_or("expected NAME=VALUE")?;
    if name.trim().is_empty() {
        return Err("header name must not be empty".to_string());
    }
    Ok(input.to_string())
}

pub fn duration(input: &str) -> Result<String, String> {
    crate::commands::search::parse_duration_ms(input)
        .filter(|value| *value >= 0)
        .ok_or("expected a non-negative duration, e.g. 30s, 5m, 2h, or 1d")?;
    Ok(input.to_string())
}

pub fn time(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    if chrono::DateTime::parse_from_rfc3339(trimmed).is_ok() || trimmed.parse::<i64>().is_ok() {
        return Ok(input.to_string());
    }
    duration(input)
}

pub fn include(input: &str) -> Result<String, String> {
    let value = input.trim().to_ascii_lowercase();
    match value.as_str() {
        "request-body" | "req-body" | "response-body" | "res-body" | "request-headers"
        | "req-headers" | "response-headers" | "res-headers" | "bodies" | "headers" => Ok(value),
        _ => Err(format!("unknown include part: {input}")),
    }
}

pub fn body_field(side: &str, path: &str) -> String {
    let path = path.trim();
    let suffix = path.strip_prefix("$.").unwrap_or(path);
    format!("{side}.body.{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_paths_validate_without_normalizing_away_invalid_syntax() {
        for path in ["$=42", "$[0].id=1", "$.a[*].b=2", "a-b.c_d=A=B", "$.x="] {
            assert!(json_filter(path).is_ok(), "{path}");
        }
        for path in [
            "",
            "=x",
            "$.a",
            "$.=1",
            "$..a=1",
            "$[oops]=1",
            "$[1:3]=x",
            "$$.a=1",
        ] {
            assert!(json_filter(path).is_err(), "{path}");
        }
    }

    #[test]
    fn body_fields_preserve_root_and_array_paths() {
        assert_eq!(body_field("req", "$"), "req.body.$");
        assert_eq!(body_field("res", "$[0].id"), "res.body.$[0].id");
        assert_eq!(body_field("req", "$.user.id"), "req.body.user.id");
        assert_eq!(body_field("res", "user.id"), "res.body.user.id");
    }

    #[test]
    fn headers_include_and_time_reject_malformed_arguments() {
        assert!(header_filter("X-Test=A=B").is_ok());
        assert!(header_filter("X-Test=").is_ok());
        assert!(header_filter("=x").is_err());
        assert!(header_filter("X-Test").is_err());
        assert_eq!(include(" BODIES ").unwrap(), "bodies");
        assert!(include("bodise").is_err());
        assert!(include("").is_err());
        for part in [
            "request-body",
            "req-body",
            "response-body",
            "res-body",
            "request-headers",
            "req-headers",
            "response-headers",
            "res-headers",
            "headers",
        ] {
            assert!(include(part).is_ok());
        }
        for input in ["30s", "5m", "2h", "1d", "1w", "1.5s", "0", "12ms"] {
            assert!(duration(input).is_ok(), "{input}");
            assert!(time(input).is_ok(), "{input}");
        }
        assert!(time("2026-09-11T12:00:00+08:00").is_ok());
        assert!(time("1789070000000").is_ok());
        for input in [
            "yesterday",
            "9years",
            "-1s",
            "999999999999999999999999999d",
            "",
        ] {
            assert!(time(input).is_err(), "{input}");
        }
    }
}

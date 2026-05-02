use bifrost_core::text::truncate_chars_with_suffix;
use reqwest::Response;
use serde_json::Value;

pub type AssertResult = Result<(), String>;

pub fn assert_status(response: &Response, expected: u16) -> AssertResult {
    let actual = response.status().as_u16();
    if actual == expected {
        Ok(())
    } else {
        Err(format!("Expected status {}, got {}", expected, actual))
    }
}

pub fn assert_status_ok(response: &Response) -> AssertResult {
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!(
            "Expected success status, got {}",
            response.status()
        ))
    }
}

pub fn assert_header_exists(response: &Response, header: &str) -> AssertResult {
    if response.headers().contains_key(header) {
        Ok(())
    } else {
        Err(format!("Header '{}' not found", header))
    }
}

pub fn assert_header_value(response: &Response, header: &str, expected: &str) -> AssertResult {
    match response.headers().get(header) {
        Some(value) => {
            let actual = value.to_str().unwrap_or("");
            if actual == expected {
                Ok(())
            } else {
                Err(format!(
                    "Header '{}' expected '{}', got '{}'",
                    header, expected, actual
                ))
            }
        }
        None => Err(format!("Header '{}' not found", header)),
    }
}

pub fn assert_header_contains(response: &Response, header: &str, substring: &str) -> AssertResult {
    match response.headers().get(header) {
        Some(value) => {
            let actual = value.to_str().unwrap_or("");
            if actual.contains(substring) {
                Ok(())
            } else {
                Err(format!(
                    "Header '{}' does not contain '{}', value: '{}'",
                    header, substring, actual
                ))
            }
        }
        None => Err(format!("Header '{}' not found", header)),
    }
}

pub fn assert_body_contains(body: &str, substring: &str) -> AssertResult {
    if body.contains(substring) {
        Ok(())
    } else {
        let preview = truncate_chars_with_suffix(body, 200, "...");
        Err(format!(
            "Body does not contain '{}', preview: '{}'",
            substring, preview
        ))
    }
}

pub fn assert_body_not_contains(body: &str, substring: &str) -> AssertResult {
    if !body.contains(substring) {
        Ok(())
    } else {
        Err(format!("Body should not contain '{}'", substring))
    }
}

pub fn assert_json_field(json: &Value, path: &str, expected: &str) -> AssertResult {
    let parts: Vec<&str> = path.split('.').collect();
    let mut current = json;

    for part in &parts {
        match current.get(part) {
            Some(v) => current = v,
            None => return Err(format!("JSON path '{}' not found", path)),
        }
    }

    let actual = match current {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => current.to_string(),
    };

    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "JSON path '{}' expected '{}', got '{}'",
            path, expected, actual
        ))
    }
}

pub fn assert_json_field_exists(json: &Value, path: &str) -> AssertResult {
    let parts: Vec<&str> = path.split('.').collect();
    let mut current = json;

    for part in &parts {
        match current.get(part) {
            Some(v) => current = v,
            None => return Err(format!("JSON path '{}' not found", path)),
        }
    }

    Ok(())
}

pub fn assert_json_field_contains(json: &Value, path: &str, substring: &str) -> AssertResult {
    let parts: Vec<&str> = path.split('.').collect();
    let mut current = json;

    for part in &parts {
        match current.get(part) {
            Some(v) => current = v,
            None => return Err(format!("JSON path '{}' not found", path)),
        }
    }

    let actual = match current {
        Value::String(s) => s.clone(),
        _ => current.to_string(),
    };

    if actual.contains(substring) {
        Ok(())
    } else {
        Err(format!(
            "JSON path '{}' does not contain '{}', value: '{}'",
            path, substring, actual
        ))
    }
}

pub fn assert_json_header(json: &Value, header: &str, expected: &str) -> AssertResult {
    let headers = json.get("headers").ok_or("No 'headers' field in JSON")?;

    let header_lower = header.to_lowercase();
    for (key, value) in headers.as_object().ok_or("'headers' is not an object")? {
        if key.to_lowercase() == header_lower {
            let actual = value.as_str().unwrap_or("");
            if actual == expected {
                return Ok(());
            } else {
                return Err(format!(
                    "Header '{}' expected '{}', got '{}'",
                    header, expected, actual
                ));
            }
        }
    }

    Err(format!("Header '{}' not found in JSON response", header))
}

pub fn assert_json_header_contains(json: &Value, header: &str, substring: &str) -> AssertResult {
    let headers = json.get("headers").ok_or("No 'headers' field in JSON")?;

    let header_lower = header.to_lowercase();
    for (key, value) in headers.as_object().ok_or("'headers' is not an object")? {
        if key.to_lowercase() == header_lower {
            let actual = value.as_str().unwrap_or("");
            if actual.contains(substring) {
                return Ok(());
            } else {
                return Err(format!(
                    "Header '{}' does not contain '{}', value: '{}'",
                    header, substring, actual
                ));
            }
        }
    }

    Err(format!("Header '{}' not found in JSON response", header))
}

pub fn assert_is_number(value: &str) -> AssertResult {
    if value.parse::<f64>().is_ok() {
        Ok(())
    } else {
        Err(format!("'{}' is not a valid number", value))
    }
}

pub fn assert_is_uuid(value: &str) -> AssertResult {
    let uuid_re =
        regex::Regex::new(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")
            .unwrap();

    if uuid_re.is_match(value) {
        Ok(())
    } else {
        Err(format!("'{}' is not a valid UUID", value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_assert_body_contains_multibyte_preview() {
        let body = "前".repeat(300);
        let err = assert_body_contains(&body, "missing").expect_err("expected failure");

        assert!(err.contains("preview"));
        assert!(err.contains("..."));
    }
}

use super::parse_header_value;

pub fn parse_res_cookies_value(value: &str) -> Vec<(String, bifrost_proxy::ResCookieValue)> {
    let value = value.trim();
    if value.is_empty() {
        return Vec::new();
    }

    if let Ok(json) = serde_json::from_str::<serde_json::Value>(value) {
        if let Some(obj) = json.as_object() {
            return obj
                .iter()
                .filter_map(|(name, val)| {
                    let cookie_value = if val.is_string() {
                        bifrost_proxy::ResCookieValue::simple(
                            val.as_str().unwrap_or("").to_string(),
                        )
                    } else {
                        let obj = val.as_object()?;
                        bifrost_proxy::ResCookieValue {
                            value: obj
                                .get("value")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            max_age: obj
                                .get("maxAge")
                                .or_else(|| obj.get("Max-Age"))
                                .or_else(|| obj.get("max_age"))
                                .and_then(|v| v.as_i64()),
                            path: obj.get("path").and_then(|v| v.as_str()).map(String::from),
                            domain: obj.get("domain").and_then(|v| v.as_str()).map(String::from),
                            secure: obj.get("secure").and_then(|v| v.as_bool()).unwrap_or(false),
                            http_only: obj
                                .get("httpOnly")
                                .or_else(|| obj.get("http_only"))
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false),
                            same_site: obj
                                .get("sameSite")
                                .or_else(|| obj.get("same_site"))
                                .and_then(|v| v.as_str())
                                .map(String::from),
                        }
                    };
                    Some((name.clone(), cookie_value))
                })
                .collect();
        }
    }

    if let Some(headers) = parse_header_value(value) {
        return headers
            .into_iter()
            .map(|(k, v)| (k, bifrost_proxy::ResCookieValue::simple(v)))
            .collect();
    }

    Vec::new()
}

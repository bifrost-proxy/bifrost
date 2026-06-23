use std::collections::HashMap;

use bifrost_core::rule_share::{
    decode_rule_share_payload, RuleShareMode, RuleSharePayload, RULE_SHARE_QUERY_PARAM,
};
use http_body_util::BodyExt;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use serde::{Deserialize, Serialize};

use super::{error_response, full_body, json_response, method_not_allowed, BoxBody};
use crate::rule_share_import::import_rule_share_payload;
use crate::state::SharedAdminState;
use crate::SharedPushManager;

#[derive(Debug, Deserialize)]
struct ConfirmRuleShareRequest {
    payload: String,
    target_url: String,
}

#[derive(Debug, Serialize)]
struct ConfirmRuleShareResponse {
    success: bool,
    rule_name: String,
    requested_name: String,
    content_hash: String,
    disabled_rules: Vec<String>,
    redirect_url: String,
}

pub async fn handle_rule_share_confirm_page(
    req: Request<Incoming>,
    state: SharedAdminState,
) -> Response<BoxBody> {
    if req.method() != Method::GET {
        return method_not_allowed();
    }

    let query = req.uri().query().unwrap_or_default();
    let params: HashMap<String, String> = serde_urlencoded::from_str(query).unwrap_or_default();
    let Some(encoded_payload) = params
        .get("payload")
        .filter(|value| !value.trim().is_empty())
    else {
        return error_response(StatusCode::BAD_REQUEST, "Missing rule share payload");
    };
    let Some(target_url) = params
        .get("target")
        .filter(|value| !value.trim().is_empty())
    else {
        return error_response(StatusCode::BAD_REQUEST, "Missing target URL");
    };

    let payload = match decode_rule_share_payload(encoded_payload) {
        Ok(payload) => payload,
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Invalid rule share payload: {error}"),
            )
        }
    };
    if let Err(error) = validate_target_url(target_url) {
        return error_response(StatusCode::BAD_REQUEST, &error);
    }

    html_response(render_confirm_page(
        &payload,
        encoded_payload,
        target_url,
        state.csrf_token(),
    ))
}

pub async fn handle_rule_share_confirm_api(
    req: Request<Incoming>,
    state: SharedAdminState,
    push_manager: Option<SharedPushManager>,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }

    let body = match req.into_body().collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read request body: {error}"),
            )
        }
    };
    let request: ConfirmRuleShareRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Invalid request body: {error}"),
            )
        }
    };
    if let Err(error) = validate_target_url(&request.target_url) {
        return error_response(StatusCode::BAD_REQUEST, &error);
    }
    let payload = match decode_rule_share_payload(&request.payload) {
        Ok(payload) => payload,
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Invalid rule share payload: {error}"),
            )
        }
    };

    match import_rule_share_payload(state, push_manager.as_ref(), payload).await {
        Ok(outcome) => json_response(&ConfirmRuleShareResponse {
            success: true,
            rule_name: outcome.rule_name,
            requested_name: outcome.requested_name,
            content_hash: outcome.content_hash,
            disabled_rules: outcome.disabled_rules,
            redirect_url: request.target_url,
        }),
        Err(error) => error_response(
            StatusCode::BAD_REQUEST,
            &format!("Failed to import rule share payload: {error}"),
        ),
    }
}

fn validate_target_url(target_url: &str) -> Result<(), String> {
    let parsed =
        url::Url::parse(target_url).map_err(|error| format!("Invalid target URL: {error}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(format!(
            "Unsupported target URL scheme: {}",
            parsed.scheme()
        ));
    }
    if parsed
        .query_pairs()
        .any(|(key, _)| key == RULE_SHARE_QUERY_PARAM)
    {
        return Err("Target URL must not contain a rule share payload".to_string());
    }
    Ok(())
}

fn render_confirm_page(
    payload: &RuleSharePayload,
    encoded_payload: &str,
    target_url: &str,
    csrf_token: &str,
) -> String {
    let payload_json = serde_json::to_string(encoded_payload).unwrap_or_else(|_| "\"\"".into());
    let target_json = serde_json::to_string(target_url).unwrap_or_else(|_| "\"\"".into());
    let csrf_json = serde_json::to_string(csrf_token).unwrap_or_else(|_| "\"\"".into());
    let mode = match payload.mode {
        RuleShareMode::EnableExclusive => "enable_exclusive",
    };
    let exclusive_scope = serde_json::to_string(&payload.exclusive_scope)
        .unwrap_or_else(|_| "\"my_rules\"".into())
        .trim_matches('"')
        .to_string();

    format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Bifrost Rule Share Confirmation</title>
  <style>
    :root {{ color-scheme: light dark; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }}
    body {{ margin: 0; background: #f5f6f7; color: #182026; }}
    main {{ max-width: 920px; margin: 0 auto; padding: 32px 20px 48px; }}
    h1 {{ font-size: 24px; margin: 0 0 16px; }}
    .panel {{ background: #fff; border: 1px solid #d8dde3; border-radius: 8px; padding: 20px; box-shadow: 0 8px 28px rgba(15, 23, 42, .08); }}
    dl {{ display: grid; grid-template-columns: 160px minmax(0, 1fr); gap: 10px 16px; margin: 0 0 18px; }}
    dt {{ color: #65717f; }}
    dd {{ margin: 0; overflow-wrap: anywhere; }}
    pre {{ white-space: pre-wrap; word-break: break-word; background: #111827; color: #f8fafc; border-radius: 6px; padding: 16px; max-height: 320px; overflow: auto; }}
    .actions {{ display: flex; gap: 12px; align-items: center; margin-top: 18px; flex-wrap: wrap; }}
    button, a.button {{ appearance: none; border: 1px solid #1b6bd8; border-radius: 6px; padding: 9px 14px; font-size: 14px; text-decoration: none; cursor: pointer; }}
    button {{ background: #1b6bd8; color: #fff; }}
    a.button {{ background: #fff; color: #1b6bd8; }}
    #status {{ color: #8a3b0d; min-height: 20px; }}
    @media (prefers-color-scheme: dark) {{
      body {{ background: #111827; color: #e5e7eb; }}
      .panel {{ background: #18212f; border-color: #2d3748; }}
      dt {{ color: #9ca3af; }}
      a.button {{ background: transparent; }}
    }}
  </style>
</head>
<body>
  <main>
    <h1>Apply Shared Bifrost Rule</h1>
    <section class="panel">
      <dl>
        <dt>Rule name</dt><dd>{name}</dd>
        <dt>Content hash</dt><dd>{hash}</dd>
        <dt>Mode</dt><dd>{mode}</dd>
        <dt>Exclusive scope</dt><dd>{scope}</dd>
        <dt>Return target</dt><dd>{target}</dd>
      </dl>
      <pre>{content}</pre>
      <div class="actions">
        <button id="apply" type="button">Apply Rule</button>
        <a class="button" href="{target_href}">Cancel</a>
        <span id="status" role="status"></span>
      </div>
    </section>
  </main>
  <script>
    const payload = {payload_json};
    const targetUrl = {target_json};
    const csrfToken = {csrf_json};
    const applyButton = document.getElementById('apply');
    const statusEl = document.getElementById('status');
    applyButton.addEventListener('click', async () => {{
      applyButton.disabled = true;
      statusEl.textContent = 'Applying...';
      try {{
        const response = await fetch('/_bifrost/api/rules/share-confirm', {{
          method: 'POST',
          headers: {{
            'Content-Type': 'application/json',
            'X-Bifrost-CSRF': csrfToken,
          }},
          body: JSON.stringify({{ payload, target_url: targetUrl }}),
        }});
        const data = await response.json().catch(() => ({{}}));
        if (!response.ok) {{
          throw new Error(data.error || data.message || 'Failed to apply shared rule');
        }}
        window.location.assign(data.redirect_url || targetUrl);
      }} catch (error) {{
        statusEl.textContent = error instanceof Error ? error.message : String(error);
        applyButton.disabled = false;
      }}
    }});
  </script>
</body>
</html>"#,
        name = escape_html(&payload.name),
        hash = escape_html(&payload.content_hash),
        mode = escape_html(mode),
        scope = escape_html(&exclusive_scope),
        target = escape_html(target_url),
        target_href = escape_html(target_url),
        content = escape_html(&payload.content),
        payload_json = payload_json,
        target_json = target_json,
        csrf_json = csrf_json,
    )
}

fn html_response(body: String) -> Response<BoxBody> {
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/html; charset=utf-8")
        .header("Cache-Control", "no-store")
        .body(full_body(body))
        .unwrap()
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use bifrost_core::rule_share::{encode_rule_share_payload, new_rule_share_payload};

    #[test]
    fn validate_target_url_rejects_nested_share_payload() {
        let payload =
            new_rule_share_payload("debug", "example.com bp://127.0.0.1:3000").expect("payload");
        let encoded = encode_rule_share_payload(&payload).expect("encode");
        let target = format!("https://example.com/path?__bifrost_rule={encoded}");

        assert!(validate_target_url(&target)
            .expect_err("nested share payload must be rejected")
            .contains("must not contain"));
    }

    #[test]
    fn render_confirm_page_escapes_payload_content() {
        let payload = new_rule_share_payload(
            "debug<script>",
            "example.com resBody://`<script>alert(1)</script>`",
        )
        .expect("payload");
        let html = render_confirm_page(&payload, "payload", "https://example.com/app", "csrf");

        assert!(html.contains("debug&lt;script&gt;"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(!html.contains("<script>alert(1)</script>"));
    }
}

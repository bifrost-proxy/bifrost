use std::path::Path;

use bifrost_core::profile::{
    analyze_compatibility, convert_resolved_surge_to_bifrost_preview,
    explain_surge_request_with_plan, parse_surge_profile, resolve_surge_profile,
    CompatibilityReport, ConversionPreview, ExplainReport, ProfileDiagnostic, ProfileResource,
    ProfileRuntimePlan, ProfileSection, ProfileSource,
};
use http_body_util::BodyExt;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use serde::{Deserialize, Serialize};

use super::{error_response, json_response, method_not_allowed, BoxBody};

const MAX_PROFILE_BYTES: usize = 1024 * 1024;
type HandlerResult<T> = Result<T, Box<Response<BoxBody>>>;

#[derive(Debug, Deserialize)]
pub struct SurgeImportRequest {
    pub content: String,
    #[serde(default)]
    pub source_label: Option<String>,
    #[serde(default)]
    pub explain_url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SurgeExplainRequest {
    pub content: String,
    pub url: String,
    #[serde(default)]
    pub source_label: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SurgeImportResponse {
    pub source_label: String,
    pub sections: Vec<ProfileSection>,
    pub diagnostics: Vec<ProfileDiagnostic>,
    pub compatibility: CompatibilityReport,
    pub resources: Vec<ProfileResource>,
    pub runtime_plan: ProfileRuntimePlan,
    pub conversion_preview: ConversionPreview,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explain: Option<ExplainReport>,
}

#[derive(Debug, Serialize)]
pub struct SurgeExplainResponse {
    pub source_label: String,
    pub report: ExplainReport,
    pub resources: Vec<ProfileResource>,
    pub diagnostics: Vec<ProfileDiagnostic>,
}

pub async fn handle_profile(req: Request<Incoming>, path: &str) -> Response<BoxBody> {
    match (req.method(), path) {
        (&Method::POST, "/api/profile/surge/import") => handle_surge_import(req).await,
        (&Method::POST, "/api/profile/surge/explain") => handle_surge_explain(req).await,
        _ => method_not_allowed(),
    }
}

async fn handle_surge_import(req: Request<Incoming>) -> Response<BoxBody> {
    let request = match read_json::<SurgeImportRequest>(req).await {
        Ok(request) => request,
        Err(response) => return *response,
    };
    match build_surge_import_response(request) {
        Ok(response) => json_response(&response),
        Err(response) => *response,
    }
}

async fn handle_surge_explain(req: Request<Incoming>) -> Response<BoxBody> {
    let request = match read_json::<SurgeExplainRequest>(req).await {
        Ok(request) => request,
        Err(response) => return *response,
    };
    match build_surge_explain_response(request) {
        Ok(response) => json_response(&response),
        Err(response) => *response,
    }
}

fn build_surge_import_response(request: SurgeImportRequest) -> HandlerResult<SurgeImportResponse> {
    validate_profile_size(&request.content)?;
    if request.content.trim().is_empty() {
        return Err(Box::new(error_response(
            StatusCode::BAD_REQUEST,
            "Surge profile content is empty",
        )));
    }

    let source_label = request
        .source_label
        .filter(|label| !label.trim().is_empty())
        .unwrap_or_else(|| "web-import".to_string());
    let document = parse_surge_profile(&request.content, ProfileSource::Inline);
    let compatibility = analyze_compatibility(&document);
    let resolved = resolve_surge_profile(document, Path::new("."));
    let conversion_preview = convert_resolved_surge_to_bifrost_preview(&resolved);
    let explain = match request.explain_url {
        Some(url) if !url.trim().is_empty() => Some(
            explain_surge_request_with_plan(&resolved.runtime_plan, url.trim()).map_err(
                |error| Box::new(error_response(StatusCode::BAD_REQUEST, &error.to_string())),
            )?,
        ),
        _ => None,
    };

    Ok(SurgeImportResponse {
        source_label,
        sections: resolved.document.sections.clone(),
        diagnostics: resolved.document.diagnostics.clone(),
        compatibility,
        resources: resolved.resources,
        runtime_plan: resolved.runtime_plan,
        conversion_preview,
        explain,
    })
}

fn build_surge_explain_response(
    request: SurgeExplainRequest,
) -> HandlerResult<SurgeExplainResponse> {
    validate_profile_size(&request.content)?;
    if request.content.trim().is_empty() {
        return Err(Box::new(error_response(
            StatusCode::BAD_REQUEST,
            "Surge profile content is empty",
        )));
    }
    if request.url.trim().is_empty() {
        return Err(Box::new(error_response(
            StatusCode::BAD_REQUEST,
            "Explain URL is empty",
        )));
    }

    let source_label = request
        .source_label
        .filter(|label| !label.trim().is_empty())
        .unwrap_or_else(|| "web-import".to_string());
    let document = parse_surge_profile(&request.content, ProfileSource::Inline);
    let resolved = resolve_surge_profile(document, Path::new("."));
    let report = explain_surge_request_with_plan(&resolved.runtime_plan, request.url.trim())
        .map_err(|error| Box::new(error_response(StatusCode::BAD_REQUEST, &error.to_string())))?;
    let mut diagnostics = resolved.document.diagnostics.clone();
    diagnostics.extend(resolved.runtime_plan.diagnostics.clone());

    Ok(SurgeExplainResponse {
        source_label,
        report,
        resources: resolved.resources,
        diagnostics,
    })
}

fn validate_profile_size(content: &str) -> HandlerResult<()> {
    if content.len() > MAX_PROFILE_BYTES {
        return Err(Box::new(error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Surge profile content exceeds 1 MiB dry-run limit",
        )));
    }
    Ok(())
}

async fn read_json<T: for<'de> Deserialize<'de>>(req: Request<Incoming>) -> HandlerResult<T> {
    let body_bytes = req
        .into_body()
        .collect()
        .await
        .map_err(|error| {
            Box::new(error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read request body: {error}"),
            ))
        })?
        .to_bytes();

    if body_bytes.len() > MAX_PROFILE_BYTES + 4096 {
        return Err(Box::new(error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Request body exceeds profile import limit",
        )));
    }

    serde_json::from_slice(&body_bytes).map_err(|error| {
        Box::new(error_response(
            StatusCode::BAD_REQUEST,
            &format!("Invalid JSON: {error}"),
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROFILE: &str = r#"
[General]
dns-server = 8.8.8.8
[Host]
api.hosted.example = 203.0.113.10
[Proxy]
ProxyA = http, 127.0.0.1, 8080
[Proxy Group]
Proxy = select, ProxyA, DIRECT
[MITM]
hostname = %APPEND% *.example.com, -private.example.com
[URL Rewrite]
^https://rewrite\.example/path https://target.example/path 302
[Rule]
DOMAIN,api.hosted.example,DIRECT
DOMAIN,rewrite.example,Proxy
DOMAIN-SUFFIX,example.com,Proxy
FINAL,DIRECT
"#;

    #[test]
    fn surge_import_response_contains_runtime_plan_and_optional_explain() {
        let response = build_surge_import_response(SurgeImportRequest {
            content: PROFILE.to_string(),
            source_label: Some("fixture.conf".to_string()),
            explain_url: Some("https://rewrite.example/path".to_string()),
        })
        .expect("import response");

        assert_eq!(response.source_label, "fixture.conf");
        assert!(response
            .compatibility
            .items
            .iter()
            .any(|item| item.capability == "DOMAIN-SUFFIX"));
        assert!(response
            .runtime_plan
            .http_pipeline
            .iter()
            .any(|item| item.section == "URL Rewrite"));
        assert!(response
            .explain
            .as_ref()
            .is_some_and(|report| report.http_pipeline.iter().any(|entry| entry.matched)));
    }

    #[test]
    fn surge_explain_response_reports_dns_and_policy_decision() {
        let response = build_surge_explain_response(SurgeExplainRequest {
            content: PROFILE.to_string(),
            url: "https://api.hosted.example/path".to_string(),
            source_label: None,
        })
        .expect("explain response");

        assert_eq!(response.source_label, "web-import");
        assert_eq!(
            response.report.dns_decision.matched_host_mapping.as_deref(),
            Some("Host mapping api.hosted.example -> 203.0.113.10")
        );
        assert_eq!(
            response
                .report
                .policy_decision
                .as_ref()
                .map(|trace| trace.terminal_policy.as_str()),
            Some("DIRECT")
        );
    }
}

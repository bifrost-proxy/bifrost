use crate::handlers::im_gateway::ImGatewayService;
use crate::handlers::{
    empty_body, error_response, json_response, json_response_with_status, method_not_allowed,
    BoxBody,
};
use bifrost_skills::{
    default_roots, SkillDraft, SkillExecutor, SkillInvocation, SkillManifest, SkillPackager,
    SkillRegistry, SkillScope, SkillStore, SkillValidator,
};
use http_body_util::BodyExt;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;

pub async fn handle_agent_skills(
    req: Request<Incoming>,
    service: &ImGatewayService,
    rest: &str,
) -> Response<BoxBody> {
    let store = store_for(service);
    let path = rest.trim_end_matches('/');
    if path.is_empty() {
        return match *req.method() {
            Method::GET => list_skills(&store),
            Method::POST => create_skill(req, &store).await,
            _ => method_not_allowed(),
        };
    }
    if path == "/validate" {
        return validate_skill(req).await;
    }
    if path == "/import" {
        return import_skill(req, &store).await;
    }
    let Some(name_rest) = path.strip_prefix('/') else {
        return error_response(StatusCode::NOT_FOUND, "skill endpoint not found");
    };
    let (name, action) = name_rest
        .split_once('/')
        .map(|(name, action)| (name, Some(action)))
        .unwrap_or((name_rest, None));
    match (req.method(), action) {
        (&Method::GET, None) => get_skill(&store, name),
        (&Method::PATCH, None) => patch_skill(req, &store, name).await,
        (&Method::DELETE, None) => delete_skill(&store, name),
        (&Method::POST, Some("test")) => test_skill(req, &store, name).await,
        (&Method::POST, Some("package")) => package_skill(&store, name),
        _ => method_not_allowed(),
    }
}

fn list_skills(store: &SkillStore) -> Response<BoxBody> {
    match SkillRegistry::without_watcher(Arc::new(store.clone())) {
        Ok(registry) => json_response(&serde_json::json!({
            "skills": registry.list(),
        })),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

fn get_skill(store: &SkillStore, name: &str) -> Response<BoxBody> {
    let records = match store.read_all() {
        Ok(records) => records,
        Err(error) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    };
    let Some(record) = records.into_iter().find(|record| record.name == name) else {
        return error_response(StatusCode::NOT_FOUND, "skill not found");
    };
    let skill_md = store
        .skill_md(record.scope.clone(), name)
        .unwrap_or_default();
    json_response(&serde_json::json!({
        "record": record,
        "skill_md": skill_md,
        "manifest": record.manifest,
        "effective_scope": record.effective_scope,
        "shadow_scopes": record.shadow_scopes,
    }))
}

#[derive(Deserialize)]
struct CreateSkillRequest {
    manifest: SkillManifest,
    skill_md: String,
    assets: Option<Vec<AssetPayload>>,
}

#[derive(Deserialize)]
struct AssetPayload {
    path: PathBuf,
    content: String,
}

async fn create_skill(req: Request<Incoming>, store: &SkillStore) -> Response<BoxBody> {
    let body: CreateSkillRequest = match read_body_json(req).await {
        Ok(body) => body,
        Err(resp) => return resp,
    };
    commit_payload(store, body, StatusCode::CREATED)
}

#[derive(Deserialize)]
struct PatchSkillRequest {
    enabled: Option<bool>,
    skill_md: Option<String>,
    manifest_overrides: Option<SkillManifest>,
    assets: Option<Vec<AssetPayload>>,
}

async fn patch_skill(req: Request<Incoming>, store: &SkillStore, name: &str) -> Response<BoxBody> {
    let body: PatchSkillRequest = match read_body_json(req).await {
        Ok(body) => body,
        Err(resp) => return resp,
    };
    let current = match store.read_all() {
        Ok(records) => records.into_iter().find(|record| record.name == name),
        Err(error) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    };
    let Some(current) = current else {
        return error_response(StatusCode::NOT_FOUND, "skill not found");
    };
    if let Some(enabled) = body.enabled {
        if body.skill_md.is_none() && body.manifest_overrides.is_none() && body.assets.is_none() {
            return match store.enable(current.scope, name, enabled) {
                Ok(()) => json_response(
                    &serde_json::json!({ "record": store.read_all().ok().and_then(|records| records.into_iter().find(|r| r.name == name)) }),
                ),
                Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
            };
        }
    }
    let payload = CreateSkillRequest {
        manifest: body.manifest_overrides.unwrap_or(current.manifest),
        skill_md: body
            .skill_md
            .unwrap_or_else(|| store.skill_md(current.scope, name).unwrap_or_default()),
        assets: body.assets,
    };
    commit_payload(store, payload, StatusCode::OK)
}

fn delete_skill(store: &SkillStore, name: &str) -> Response<BoxBody> {
    let records = match store.read_all() {
        Ok(records) => records,
        Err(error) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    };
    let Some(record) = records.into_iter().find(|record| record.name == name) else {
        return error_response(StatusCode::NOT_FOUND, "skill not found");
    };
    match store.delete(record.scope, name) {
        Ok(()) => Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(empty_body())
            .unwrap(),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

#[derive(Deserialize)]
struct TestSkillRequest {
    inputs: Option<serde_json::Value>,
    timeout_ms: Option<u64>,
}

async fn test_skill(req: Request<Incoming>, store: &SkillStore, name: &str) -> Response<BoxBody> {
    let body: TestSkillRequest = match read_body_json(req).await {
        Ok(body) => body,
        Err(resp) => return resp,
    };
    let record = match store.read_all() {
        Ok(records) => records.into_iter().find(|record| record.name == name),
        Err(error) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    };
    let Some(record) = record else {
        return error_response(StatusCode::NOT_FOUND, "skill not found");
    };
    match SkillExecutor::default()
        .execute(
            &record,
            SkillInvocation {
                input: body.inputs.unwrap_or_else(|| serde_json::json!({})),
                timeout_ms: body.timeout_ms,
            },
        )
        .await
    {
        Ok(report) => json_response(&report),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

fn package_skill(store: &SkillStore, name: &str) -> Response<BoxBody> {
    let record = match store.read_all() {
        Ok(records) => records.into_iter().find(|record| record.name == name),
        Err(error) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    };
    let Some(record) = record else {
        return error_response(StatusCode::NOT_FOUND, "skill not found");
    };
    match SkillPackager::package(store, record.scope, name) {
        Ok(bytes) => json_response(&serde_json::json!({
            "download_url": format!("/agent/skills/{name}/download?token=local"),
            "expires_at_unix": chrono::Utc::now().timestamp() + 300,
            "bytes": bytes.len(),
        })),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

#[derive(Deserialize)]
struct ImportSkillRequest {
    path: PathBuf,
    scope: Option<SkillScope>,
}

async fn import_skill(req: Request<Incoming>, store: &SkillStore) -> Response<BoxBody> {
    let body: ImportSkillRequest = match read_body_json(req).await {
        Ok(body) => body,
        Err(resp) => return resp,
    };
    let bytes = match std::fs::read(&body.path) {
        Ok(bytes) => bytes,
        Err(error) => {
            return error_response(StatusCode::BAD_REQUEST, &format!("read archive: {error}"))
        }
    };
    match SkillPackager::import(store, body.scope.unwrap_or(SkillScope::Project), &bytes) {
        Ok(record) => json_response_with_status(
            StatusCode::CREATED,
            &serde_json::json!({ "record": record }),
        ),
        Err(error) => error_response(StatusCode::UNPROCESSABLE_ENTITY, &error.to_string()),
    }
}

#[derive(Deserialize)]
struct ValidateSkillRequest {
    manifest: SkillManifest,
    skill_md: String,
}

async fn validate_skill(req: Request<Incoming>) -> Response<BoxBody> {
    let body: ValidateSkillRequest = match read_body_json(req).await {
        Ok(body) => body,
        Err(resp) => return resp,
    };
    let temp = match tempfile::tempdir() {
        Ok(temp) => temp,
        Err(error) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    };
    if let Err(error) = std::fs::write(temp.path().join("SKILL.md"), body.skill_md) {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string());
    }
    let result =
        SkillValidator::new().validate_manifest(&body.manifest, Some(temp.path()), Vec::new());
    if result.ok {
        json_response(&serde_json::json!({ "ok": true, "warnings": result.warnings }))
    } else {
        json_response_with_status(
            StatusCode::UNPROCESSABLE_ENTITY,
            &serde_json::json!({ "errors": result.errors }),
        )
    }
}

fn commit_payload(
    store: &SkillStore,
    body: CreateSkillRequest,
    status: StatusCode,
) -> Response<BoxBody> {
    let assets = body
        .assets
        .unwrap_or_default()
        .into_iter()
        .map(|asset| (asset.path, asset.content.into_bytes()))
        .collect();
    match store.commit(SkillDraft {
        manifest: body.manifest,
        skill_md: body.skill_md,
        draft_dir: None,
        assets,
    }) {
        Ok(record) => json_response_with_status(status, &serde_json::json!({ "record": record })),
        Err(error) => error_response(StatusCode::UNPROCESSABLE_ENTITY, &error.to_string()),
    }
}

fn store_for(service: &ImGatewayService) -> SkillStore {
    let config = service.agent_config_store.load();
    SkillStore::new(default_roots(
        bifrost_agent::config::agent_home_dir(),
        config.resolve_work_dir(),
    ))
}

async fn read_body_json<T: serde::de::DeserializeOwned>(
    req: Request<Incoming>,
) -> Result<T, Response<BoxBody>> {
    let body = req.collect().await.map_err(|error| {
        error_response(
            StatusCode::BAD_REQUEST,
            &format!("Failed to read request body: {error}"),
        )
    })?;
    serde_json::from_slice(&body.to_bytes()).map_err(|error| {
        error_response(
            StatusCode::BAD_REQUEST,
            &format!("Invalid request body: {error}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bifrost_skills::{Entrypoint, SkillAuthor, TriggerRule};
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    fn manifest(name: &str, scope: SkillScope) -> SkillManifest {
        SkillManifest {
            name: name.to_string(),
            version: "0.1.0".to_string(),
            description: "test skill".to_string(),
            scope,
            entrypoint: Entrypoint::Inline {
                instructions_md: "say ok".to_string(),
            },
            allowed_tools: Vec::new(),
            slash_command: None,
            triggers: vec![TriggerRule::DescriptionMatch],
            inputs_schema: None,
            outputs_schema: None,
            metadata: BTreeMap::new(),
            created_by: SkillAuthor::User { id: "u".into() },
            created_at_unix: 1,
            updated_at_unix: 1,
            checksum: String::new(),
            schema_version: 1,
        }
    }

    #[test]
    fn agent_skills_crud_store_happy_path() {
        let dir = tempdir().unwrap();
        let store = SkillStore::new(vec![bifrost_skills::ScopeRoot::new(
            SkillScope::Project,
            dir.path(),
        )]);
        let record = store
            .commit(SkillDraft {
                manifest: manifest("admin-skill", SkillScope::Project),
                skill_md: "---\nname: admin-skill\n---\n# Admin".into(),
                draft_dir: None,
                assets: Vec::new(),
            })
            .unwrap();
        assert_eq!(record.name, "admin-skill");
        assert!(store
            .verify_checksum(SkillScope::Project, "admin-skill")
            .unwrap());
        store
            .enable(SkillScope::Project, "admin-skill", false)
            .unwrap();
        assert!(
            !store
                .read_one(SkillScope::Project, "admin-skill")
                .unwrap()
                .enabled
        );
        store.delete(SkillScope::Project, "admin-skill").unwrap();
        assert!(store.read_one(SkillScope::Project, "admin-skill").is_err());
    }

    #[test]
    fn agent_skills_detects_slash_conflict() {
        let dir = tempdir().unwrap();
        let store = SkillStore::new(vec![bifrost_skills::ScopeRoot::new(
            SkillScope::Project,
            dir.path(),
        )]);
        let mut first = manifest("first-skill", SkillScope::Project);
        first.slash_command = Some("/first".into());
        first.triggers = vec![TriggerRule::SlashCommand];
        store
            .commit(SkillDraft {
                manifest: first,
                skill_md: "---\nname: first-skill\n---\n# First".into(),
                draft_dir: None,
                assets: Vec::new(),
            })
            .unwrap();
        let mut second = manifest("second-skill", SkillScope::Project);
        second.slash_command = Some("/first".into());
        second.triggers = vec![TriggerRule::SlashCommand];
        let error = store
            .commit(SkillDraft {
                manifest: second,
                skill_md: "---\nname: second-skill\n---\n# Second".into(),
                draft_dir: None,
                assets: Vec::new(),
            })
            .unwrap_err();
        assert!(error.to_string().contains("validation"));
    }
}

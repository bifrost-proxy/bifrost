use crate::ai_workflow::{
    normalize_workflow, parse_workflow_document, preview_workflow, render_workflow, schema_payload,
    validate_workflow, workflow_template, workflow_templates_payload, WorkflowDocument,
    WorkflowStore,
};
use crate::handlers::{
    error_response, json_response, json_response_with_status, method_not_allowed, BoxBody,
};
use http_body_util::BodyExt;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use serde::Deserialize;
use serde_json::{json, Value};

type HandlerResult<T> = Result<T, Box<Response<BoxBody>>>;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DraftBody {
    #[serde(default)]
    draft: Option<String>,
    #[serde(default)]
    workflow: Option<WorkflowDocument>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunRequest {
    #[serde(default)]
    inputs: Value,
}

pub async fn handle_ai_workflow(req: Request<Incoming>, path: &str) -> Response<BoxBody> {
    let rest = path
        .strip_prefix("/api/ai/workflows")
        .unwrap_or_default()
        .trim_end_matches('/');
    if rest.is_empty() {
        return match *req.method() {
            Method::GET => list_workflows(),
            Method::POST => apply_workflow(req).await,
            _ => method_not_allowed(),
        };
    }

    match rest {
        "/schema" => match *req.method() {
            Method::GET => json_response(&schema_payload()),
            _ => method_not_allowed(),
        },
        "/templates" => match *req.method() {
            Method::GET => json_response(&workflow_templates_payload()),
            _ => method_not_allowed(),
        },
        "/schedules" => match *req.method() {
            Method::GET => list_schedule_states(),
            _ => method_not_allowed(),
        },
        "/validate" => match *req.method() {
            Method::POST => validate_draft(req).await,
            _ => method_not_allowed(),
        },
        "/preview" => match *req.method() {
            Method::POST => preview_draft(req).await,
            _ => method_not_allowed(),
        },
        "/render" => match *req.method() {
            Method::POST => render_draft(req).await,
            _ => method_not_allowed(),
        },
        _ => handle_workflow_resource(req, rest).await,
    }
}

fn list_workflows() -> Response<BoxBody> {
    match WorkflowStore::default().list() {
        Ok(workflows) => json_response(&json!({ "workflows": workflows })),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

fn list_schedule_states() -> Response<BoxBody> {
    match WorkflowStore::default().list_schedule_states() {
        Ok(schedules) => json_response(&json!({ "schedules": schedules })),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

async fn validate_draft(req: Request<Incoming>) -> Response<BoxBody> {
    let workflow = match read_draft(req).await {
        Ok(workflow) => workflow,
        Err(response) => return *response,
    };
    json_response(&validate_workflow(&workflow))
}

async fn preview_draft(req: Request<Incoming>) -> Response<BoxBody> {
    let workflow = match read_draft(req).await {
        Ok(workflow) => workflow,
        Err(response) => return *response,
    };
    json_response(&preview_workflow(&workflow))
}

async fn render_draft(req: Request<Incoming>) -> Response<BoxBody> {
    let workflow = match read_draft(req).await {
        Ok(workflow) => workflow,
        Err(response) => return *response,
    };
    json_response(&json!({ "reactFlow": render_workflow(&workflow) }))
}

async fn apply_workflow(req: Request<Incoming>) -> Response<BoxBody> {
    let body: Value = match read_body_json(req).await {
        Ok(body) => body,
        Err(response) => return *response,
    };
    let workflow = match workflow_from_value(&body) {
        Ok(workflow) => workflow,
        Err(response) => return *response,
    };
    let dry_run = body
        .get("dryRun")
        .or_else(|| body.get("dry_run"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let base_revision = body
        .get("baseRevision")
        .or_else(|| body.get("base_revision"))
        .and_then(Value::as_u64);
    let preview = preview_workflow(&workflow);
    if dry_run {
        return json_response(&json!({
            "dryRun": true,
            "workflow": workflow,
            "validation": validate_workflow(&workflow),
            "preview": preview,
        }));
    }
    if !preview.blocking_errors.is_empty() {
        return json_response_with_status(
            StatusCode::BAD_REQUEST,
            &json!({ "error": "workflow validation failed", "validation": validate_workflow(&workflow) }),
        );
    }
    let store = WorkflowStore::default();
    match store.save(workflow, base_revision) {
        Ok(saved) => json_response_with_status(StatusCode::CREATED, &json!({ "workflow": saved })),
        Err(error) => error_response(StatusCode::BAD_REQUEST, &error),
    }
}

fn workflow_from_value(body: &Value) -> HandlerResult<WorkflowDocument> {
    if let Some(workflow) = body.get("workflow") {
        return serde_json::from_value::<WorkflowDocument>(workflow.clone())
            .map(normalize_workflow)
            .map_err(|error| {
                Box::new(error_response(
                    StatusCode::BAD_REQUEST,
                    &format!("invalid workflow payload: {error}"),
                ))
            });
    }
    if let Some(draft) = body.get("draft").and_then(Value::as_str) {
        return parse_workflow_document(draft)
            .map(normalize_workflow)
            .map_err(|error| Box::new(error_response(StatusCode::BAD_REQUEST, &error)));
    }
    Err(Box::new(error_response(
        StatusCode::BAD_REQUEST,
        "request body must contain workflow or draft",
    )))
}

async fn handle_workflow_resource(req: Request<Incoming>, rest: &str) -> Response<BoxBody> {
    let Some(rest) = rest.strip_prefix('/') else {
        return error_response(StatusCode::NOT_FOUND, "workflow endpoint not found");
    };
    let parts = rest.split('/').collect::<Vec<_>>();
    if parts.is_empty() || parts[0].is_empty() {
        return error_response(StatusCode::NOT_FOUND, "workflow endpoint not found");
    }
    let workflow_id = parts[0];
    if workflow_id == "templates" {
        return match (req.method(), parts.as_slice()) {
            (&Method::GET, [_, template_id]) => match workflow_template(template_id) {
                Some(template) => json_response(&json!({ "template": template })),
                None => error_response(StatusCode::NOT_FOUND, "workflow template not found"),
            },
            _ => method_not_allowed(),
        };
    }
    let store = WorkflowStore::default();
    match (req.method(), parts.as_slice()) {
        (&Method::GET, [_]) => match store.get(workflow_id) {
            Ok(workflow) => json_response(&json!({ "workflow": workflow })),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                error_response(StatusCode::NOT_FOUND, "workflow not found")
            }
            Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
        },
        (&Method::POST, [_, "run"]) => {
            let body: RunRequest = match read_body_json(req).await {
                Ok(body) => body,
                Err(response) => return *response,
            };
            match store.create_run_async(workflow_id, body.inputs).await {
                Ok(run) => json_response_with_status(StatusCode::CREATED, &json!({ "run": run })),
                Err(error) => error_response(StatusCode::BAD_REQUEST, &error),
            }
        }
        (&Method::GET, [_, "runs", run_id]) => match store.get_run(run_id) {
            Ok(run) if run.workflow_id == workflow_id => json_response(&json!({ "run": run })),
            Ok(_) => error_response(StatusCode::NOT_FOUND, "workflow run not found"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                error_response(StatusCode::NOT_FOUND, "workflow run not found")
            }
            Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
        },
        _ => method_not_allowed(),
    }
}

async fn read_draft(req: Request<Incoming>) -> HandlerResult<WorkflowDocument> {
    let body: DraftBody = read_body_json(req).await?;
    if let Some(workflow) = body.workflow {
        return Ok(normalize_workflow(workflow));
    }
    if let Some(draft) = body.draft {
        return parse_workflow_document(&draft)
            .map(normalize_workflow)
            .map_err(|error| Box::new(error_response(StatusCode::BAD_REQUEST, &error)));
    }
    Err(Box::new(error_response(
        StatusCode::BAD_REQUEST,
        "request body must contain workflow or draft",
    )))
}

async fn read_body_json<T: serde::de::DeserializeOwned>(
    req: Request<Incoming>,
) -> HandlerResult<T> {
    let body = req.collect().await.map_err(|error| {
        Box::new(error_response(
            StatusCode::BAD_REQUEST,
            &format!("failed to read request body: {error}"),
        ))
    })?;
    serde_json::from_slice(&body.to_bytes()).map_err(|error| {
        Box::new(error_response(
            StatusCode::BAD_REQUEST,
            &format!("invalid JSON request body: {error}"),
        ))
    })
}

use super::*;

use crate::im_gateway::feishu_menu::{
    FeishuAppProvisioner, FeishuMenuApplyStatus, FeishuMenuState, FeishuMenuSyncOptions,
};

pub(super) async fn handle_provider_feishu_menu(
    req: Request<Incoming>,
    service: &ImGatewayService,
    provider_id: &str,
    action: &str,
) -> Response<BoxBody> {
    let Some(provider) = service.provider_store.get(provider_id) else {
        return error_response(StatusCode::NOT_FOUND, "Provider not found");
    };
    let feishu = service.connection_manager.feishu_provider();
    let provisioner = FeishuAppProvisioner::new(feishu, &service.feishu_menu_state_store);

    match action {
        "preview" if req.method() == Method::GET => match provisioner.preview(&provider) {
            Ok(preview) => json_response(&preview),
            Err(error) => provision_error_response(error),
        },
        "status" if req.method() == Method::GET => {
            match provisioner.preview(&provider) {
                Ok(preview) => {
                    let state = service.feishu_menu_state_store.get(provider_id).unwrap_or(
                        FeishuMenuState {
                            provider_id: provider_id.to_string(),
                            status: FeishuMenuApplyStatus::NotApplied,
                            desired_digest: Some(preview.desired_digest.clone()),
                            updated_at: 0,
                            ..FeishuMenuState::default()
                        },
                    );
                    json_response(&serde_json::json!({
                        "provider_id": provider_id,
                        "preset": preview.preset,
                        "desired_digest": preview.desired_digest,
                        "state": state,
                    }))
                }
                Err(error) => provision_error_response(error),
            }
        }
        "sync" if req.method() == Method::POST => {
            let options: FeishuMenuSyncOptions = match read_body_json(req).await {
                Ok(options) => options,
                Err(response) => return response,
            };
            match provisioner.reconcile(&provider, &options).await {
                Ok(result) => json_response(&result),
                Err(error) => provision_error_response(error),
            }
        }
        "preview" | "status" | "sync" => method_not_allowed(),
        _ => error_response(StatusCode::NOT_FOUND, "Feishu menu endpoint not found"),
    }
}

pub(super) async fn reconcile_feishu_menu_for_connect(
    service: &ImGatewayService,
    provider: &ImProviderConfig,
    publish: bool,
    source: &str,
) -> serde_json::Value {
    if provider.provider_type != ImProviderType::Feishu {
        return serde_json::Value::Null;
    }
    let options = FeishuMenuSyncOptions {
        publish,
        ..FeishuMenuSyncOptions::default()
    };
    let provisioner = FeishuAppProvisioner::new(
        service.connection_manager.feishu_provider(),
        &service.feishu_menu_state_store,
    );
    match provisioner.reconcile(provider, &options).await {
        Ok(result) => {
            info!(
                provider_id = %provider.id,
                source,
                publish,
                skipped = result.skipped,
                "Feishu bot command menu reconciled before connection"
            );
            serde_json::json!({"success": true, "result": result})
        }
        Err(error) => {
            warn!(
                provider_id = %provider.id,
                source,
                publish,
                stage = %error.stage,
                error_kind = %error.error,
                error = %error,
                "Feishu bot command menu reconcile failed; continuing connection"
            );
            serde_json::json!({"success": false, "error": error})
        }
    }
}

fn provision_error_response(
    error: crate::im_gateway::feishu_menu::FeishuProvisionError,
) -> Response<BoxBody> {
    let status = match error.error.as_str() {
        "invalid_provider"
        | "missing_app_credentials"
        | "menu_validation_failed"
        | "invalid_publish_options" => StatusCode::BAD_REQUEST,
        "app_under_review" => StatusCode::CONFLICT,
        "unsupported_app_type" => StatusCode::UNPROCESSABLE_ENTITY,
        "state_persist_failed" => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::BAD_GATEWAY,
    };
    json_response_with_status(status, &error)
}

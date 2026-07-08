use super::*;

// ---------------------------------------------------------------------------
// Providers
// ---------------------------------------------------------------------------

pub(super) async fn handle_providers(
    req: Request<Incoming>,
    service: &ImGatewayService,
    rest: &str,
) -> Response<BoxBody> {
    let rest = rest.trim_end_matches('/');

    if rest == "/feishu-setup/start" {
        return handle_provider_feishu_setup_start(req, service).await;
    }
    if let Some(session_id) = rest
        .strip_prefix("/feishu-setup/")
        .and_then(|value| value.strip_suffix("/status"))
    {
        return handle_provider_feishu_setup_status(req, service, session_id).await;
    }
    if let Some(session_id) = rest
        .strip_prefix("/feishu-setup/")
        .and_then(|value| value.strip_suffix("/provider"))
    {
        return handle_provider_feishu_setup_create_provider(req, service, session_id).await;
    }

    // GET /providers  |  POST /providers
    if rest.is_empty() {
        return match *req.method() {
            Method::GET => {
                let providers = service.provider_store.list();
                let safe: Vec<_> = providers.iter().map(sanitize_provider).collect();
                json_response(&safe)
            }
            Method::POST => {
                let payload: serde_json::Value = match read_body_json(req).await {
                    Ok(v) => v,
                    Err(resp) => return resp,
                };
                let mut config = match parse_provider_create_payload(payload) {
                    Ok(v) => v,
                    Err(e) => {
                        return error_response(
                            StatusCode::BAD_REQUEST,
                            &format!("Invalid request body: {e}"),
                        );
                    }
                };
                let now = now_ms();
                if config.created_at == 0 {
                    config.created_at = now;
                }
                config.updated_at = now;
                normalize_provider_agent_config(&mut config);
                match service.provider_store.add(config) {
                    Ok(()) => json_response(&serde_json::json!({"success": true})),
                    Err(e) => error_response(StatusCode::BAD_REQUEST, &e.to_string()),
                }
            }
            _ => method_not_allowed(),
        };
    }

    // Sub-paths: /:id, /:id/status, /:id/policy, /:id/policy/bind-shell
    if let Some(id_and_rest) = rest.strip_prefix('/') {
        // Check for /:id/policy/bind-shell
        if let Some(id) = extract_segment_before(id_and_rest, "/policy/bind-shell") {
            return handle_provider_policy_bind_shell(req, service, id).await;
        }
        // Check for /:id/policy
        if let Some(id) = extract_segment_before(id_and_rest, "/policy") {
            return handle_provider_policy(req, service, id).await;
        }
        // Check for /:id/status
        if let Some(id) = extract_segment_before(id_and_rest, "/status") {
            return handle_provider_status(&req, service, id);
        }
        // Check for /:id/connect
        if let Some(id) = extract_segment_before(id_and_rest, "/connect") {
            return handle_provider_connect(req, service, id).await;
        }
        // Check for /:id/disconnect
        if let Some(id) = extract_segment_before(id_and_rest, "/disconnect") {
            return handle_provider_disconnect(req, service, id).await;
        }
        // Check for /:id/weixin-login/start
        if let Some(id) = extract_segment_before(id_and_rest, "/weixin-login/start") {
            return handle_provider_weixin_login_start(req, service, id).await;
        }
        // Check for /:id/weixin-login/status
        if let Some(id) = extract_segment_before(id_and_rest, "/weixin-login/status") {
            return handle_provider_weixin_login_status(req, service, id).await;
        }
        // Check for /:id/weixin-login/complete
        if let Some(id) = extract_segment_before(id_and_rest, "/weixin-login/complete") {
            return handle_provider_weixin_login_complete(req, service, id).await;
        }
        // Check for /:id/messages
        if let Some(id) = extract_segment_before(id_and_rest, "/messages") {
            return handle_provider_messages(req, service, id).await;
        }
        // /:id
        let id = id_and_rest.split('/').next().unwrap_or(id_and_rest);
        if !id.is_empty() && !id.contains('/') {
            return handle_provider_by_id(req, service, id).await;
        }
    }

    error_response(StatusCode::NOT_FOUND, "Provider endpoint not found")
}

pub(super) async fn handle_provider_by_id(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    match *req.method() {
        Method::GET => match service.provider_store.get(id) {
            Some(p) => json_response(&sanitize_provider(&p)),
            None => error_response(StatusCode::NOT_FOUND, "Provider not found"),
        },
        Method::PATCH => {
            let patch: serde_json::Value = match read_body_json(req).await {
                Ok(v) => v,
                Err(resp) => return resp,
            };
            let Some(mut existing) = service.provider_store.get(id) else {
                return error_response(StatusCode::NOT_FOUND, "Provider not found");
            };
            apply_provider_patch(&mut existing, &patch);
            match service.provider_store.update(existing) {
                Ok(()) => {
                    if disables_provider_connection(&patch) {
                        service.connection_manager.stop_connection(id);
                    }
                    json_response(&serde_json::json!({"success": true}))
                }
                Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
            }
        }
        Method::DELETE => match service.provider_store.delete(id) {
            Ok(()) => {
                service.connection_manager.stop_connection(id);
                service.weixin_login_pending.write().remove(id);
                json_response(&serde_json::json!({"success": true}))
            }
            Err(e) => error_response(StatusCode::NOT_FOUND, &e.to_string()),
        },
        _ => method_not_allowed(),
    }
}

fn disables_provider_connection(patch: &serde_json::Value) -> bool {
    patch
        .get("enabled")
        .and_then(|value| value.as_bool())
        .is_some_and(|enabled| !enabled)
        || patch
            .get("event_connection_enabled")
            .and_then(|value| value.as_bool())
            .is_some_and(|enabled| !enabled)
}

#[cfg(test)]
mod tests {
    use super::{apply_weixin_login_account_to_provider, disables_provider_connection};
    use crate::im_gateway::types::{ImProviderConfig, ImProviderType};
    use crate::im_gateway::weixin::WeixinLoginAccount;

    #[test]
    fn provider_patch_detects_connection_disabling_changes() {
        assert!(disables_provider_connection(&serde_json::json!({
            "enabled": false
        })));
        assert!(disables_provider_connection(&serde_json::json!({
            "event_connection_enabled": false
        })));
        assert!(!disables_provider_connection(&serde_json::json!({
            "enabled": true,
            "event_connection_enabled": true
        })));
        assert!(!disables_provider_connection(&serde_json::json!({
            "display_name": "Renamed"
        })));
    }

    #[test]
    fn weixin_login_completion_keeps_fixed_provider_base_url() {
        let mut provider = ImProviderConfig {
            id: "weixin-main".to_string(),
            provider_type: ImProviderType::Weixin,
            display_name: "Weixin Main".to_string(),
            enabled: false,
            base_url: Some("https://evil.example".to_string()),
            app_id: None,
            secret_ref: None,
            owner_open_id: None,
            event_connection_enabled: false,
            event_types: Vec::new(),
            agent_config: None,
            created_at: 0,
            updated_at: 0,
        };
        let account = WeixinLoginAccount {
            account_id: "bot@im.bot".to_string(),
            user_id: "owner@im.wechat".to_string(),
            base_url: "http://127.0.0.1:12345".to_string(),
            bot_token: "token".to_string(),
        };

        apply_weixin_login_account_to_provider(&mut provider, account);

        assert_eq!(
            provider.base_url.as_deref(),
            Some("https://ilinkai.weixin.qq.com")
        );
        assert_eq!(provider.app_id.as_deref(), Some("bot@im.bot"));
        assert_eq!(provider.owner_open_id.as_deref(), Some("owner@im.wechat"));
        assert_eq!(provider.secret_ref.as_deref(), Some("token"));
        assert!(provider.enabled);
        assert!(provider.event_connection_enabled);
        assert_eq!(provider.event_types, vec!["message.receive"]);
    }
}

pub(super) fn handle_provider_status(
    req: &Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    if req.method() != Method::GET {
        return method_not_allowed();
    }
    let status = service.connection_manager.get_status(id);
    match status {
        Some(s) => json_response(&s),
        None => {
            if service.provider_store.get(id).is_some() {
                json_response(&crate::im_gateway::types::ConnectionStatus::default())
            } else {
                error_response(StatusCode::NOT_FOUND, "Provider not found")
            }
        }
    }
}

fn apply_weixin_login_account_to_provider(
    provider: &mut crate::im_gateway::types::ImProviderConfig,
    account: crate::im_gateway::weixin::WeixinLoginAccount,
) {
    provider.app_id = Some(account.account_id);
    provider.owner_open_id = Some(account.user_id);
    provider.base_url = None;
    provider.secret_ref = Some(account.bot_token);
    provider.enabled = true;
    provider.event_connection_enabled = true;
    if provider.event_types.is_empty() {
        provider.event_types = vec!["message.receive".to_string()];
    }
    provider.updated_at = now_ms();
    normalize_provider_base_url(provider);
    normalize_provider_agent_config(provider);
}

pub(super) async fn handle_provider_weixin_login_start(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }
    let Some(provider) = service.provider_store.get(id) else {
        return error_response(StatusCode::NOT_FOUND, "Provider not found");
    };
    if provider.provider_type != crate::im_gateway::types::ImProviderType::Weixin {
        return error_response(StatusCode::BAD_REQUEST, "Provider is not a weixin provider");
    }
    match service
        .connection_manager
        .weixin_provider()
        .start_login(provider.base_url.as_deref())
        .await
    {
        Ok(login) => {
            service.weixin_login_pending.write().insert(
                provider.id.clone(),
                PendingWeixinLogin {
                    login: login.clone(),
                    created_at_ms: now_ms(),
                },
            );
            json_response(&serde_json::json!({
                "success": true,
                "scan_url": login.scan_url,
                "expires_in_seconds": login.expires_in_seconds,
            }))
        }
        Err(e) => error_response(
            StatusCode::BAD_GATEWAY,
            &format!("Failed to start weixin login: {e}"),
        ),
    }
}

pub(super) async fn handle_provider_weixin_login_status(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    if req.method() != Method::GET {
        return method_not_allowed();
    }
    let Some(mut provider) = service.provider_store.get(id) else {
        return error_response(StatusCode::NOT_FOUND, "Provider not found");
    };
    if provider.provider_type != crate::im_gateway::types::ImProviderType::Weixin {
        return error_response(StatusCode::BAD_REQUEST, "Provider is not a weixin provider");
    }
    let Some(pending) = service.weixin_login_pending.read().get(id).cloned() else {
        return json_response(&serde_json::json!({
            "success": true,
            "status": if provider.secret_ref.is_some() { "authorized" } else { "idle" },
            "provider": sanitize_provider(&provider),
        }));
    };
    let expires_at_ms =
        pending.created_at_ms + pending.login.expires_in_seconds.saturating_mul(1000);
    if now_ms() >= expires_at_ms {
        service.weixin_login_pending.write().remove(id);
        return json_response(&serde_json::json!({
            "success": true,
            "status": "expired",
            "expires_at": expires_at_ms,
        }));
    }
    match service
        .connection_manager
        .weixin_provider()
        .complete_login(
            &pending.login.poll_key,
            provider.base_url.as_deref(),
            1,
            std::time::Duration::ZERO,
        )
        .await
    {
        Ok(account) => {
            let account_id = account.account_id.clone();
            let user_id = account.user_id.clone();
            apply_weixin_login_account_to_provider(&mut provider, account);
            if let Err(e) = service.provider_store.update(provider.clone()) {
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
            }
            service.weixin_login_pending.write().remove(id);
            json_response(&serde_json::json!({
                "success": true,
                "status": "confirmed",
                "provider": sanitize_provider(&provider),
                "account": {
                    "account_id": account_id,
                    "user_id": user_id,
                }
            }))
        }
        Err(e) => {
            let error = e.to_string();
            if error.contains("expired") {
                service.weixin_login_pending.write().remove(id);
                return json_response(&serde_json::json!({
                    "success": true,
                    "status": "expired",
                    "expires_at": expires_at_ms,
                }));
            }
            json_response(&serde_json::json!({
                "success": true,
                "status": "pending",
                "expires_at": expires_at_ms,
            }))
        }
    }
}

pub(super) async fn handle_provider_weixin_login_complete(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }
    let Some(mut provider) = service.provider_store.get(id) else {
        return error_response(StatusCode::NOT_FOUND, "Provider not found");
    };
    if provider.provider_type != crate::im_gateway::types::ImProviderType::Weixin {
        return error_response(StatusCode::BAD_REQUEST, "Provider is not a weixin provider");
    }
    let Some(pending) = service.weixin_login_pending.read().get(id).cloned() else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "No pending weixin login; start QR login first",
        );
    };
    match service
        .connection_manager
        .weixin_provider()
        .complete_login(
            &pending.login.poll_key,
            provider.base_url.as_deref(),
            20,
            std::time::Duration::from_secs(2),
        )
        .await
    {
        Ok(account) => {
            let account_id = account.account_id.clone();
            let user_id = account.user_id.clone();
            apply_weixin_login_account_to_provider(&mut provider, account);
            if let Err(e) = service.provider_store.update(provider.clone()) {
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
            }
            service.weixin_login_pending.write().remove(id);
            json_response(&serde_json::json!({
                "success": true,
                "provider": sanitize_provider(&provider),
                "account": {
                    "account_id": account_id,
                    "user_id": user_id,
                }
            }))
        }
        Err(e) => error_response(
            StatusCode::BAD_GATEWAY,
            &format!("Failed to complete weixin login: {e}"),
        ),
    }
}

#[derive(Debug, Deserialize)]
struct FeishuSetupStartRequest {
    #[serde(default)]
    brand: Option<String>,
    #[serde(default)]
    provider: Option<serde_json::Value>,
}

pub(super) async fn handle_provider_feishu_setup_start(
    req: Request<Incoming>,
    service: &ImGatewayService,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }
    let body: FeishuSetupStartRequest = match read_body_json(req).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let brand = parse_feishu_setup_brand(body.brand.as_deref());
    match request_feishu_app_registration(brand).await {
        Ok(start) => {
            let session_id = format!("fas_{}", uuid::Uuid::new_v4().simple());
            let expires_at_ms = now_ms() + start.expires_in_seconds.saturating_mul(1000);
            service.feishu_setup_pending.write().insert(
                session_id.clone(),
                PendingFeishuSetup {
                    device_code: start.device_code,
                    interval_seconds: start.interval_seconds,
                    expires_at_ms,
                    app_id: None,
                    app_secret: None,
                    owner_open_id: None,
                    brand,
                    provider_payload: body.provider,
                    created_provider_id: None,
                    auto_connect: true,
                },
            );
            save_pending_feishu_setups(service);
            json_response(&serde_json::json!({
                "success": true,
                "session_id": session_id,
                "verification_url": start.verification_url,
                "expires_at": expires_at_ms,
                "expires_in_seconds": start.expires_in_seconds,
                "interval_seconds": start.interval_seconds,
                "brand": feishu_setup_brand_label(brand),
            }))
        }
        Err(e) => error_response(
            StatusCode::BAD_GATEWAY,
            &format!("Failed to start Feishu setup: {e}"),
        ),
    }
}

pub(super) async fn handle_provider_feishu_setup_status(
    req: Request<Incoming>,
    service: &ImGatewayService,
    session_id: &str,
) -> Response<BoxBody> {
    if req.method() != Method::GET {
        return method_not_allowed();
    }
    let Some(pending) = service.feishu_setup_pending.read().get(session_id).cloned() else {
        return error_response(StatusCode::NOT_FOUND, "Feishu setup session not found");
    };
    if let Some(app_id) = pending.app_id.as_deref() {
        if pending.created_provider_id.is_some() {
            return json_response(&confirmed_feishu_setup_response(&pending, service));
        }
        return json_response(&serde_json::json!({
            "success": true,
            "status": "confirmed",
            "app_id": app_id,
            "secret_configured": pending.app_secret.as_deref().is_some_and(|s| !s.is_empty()),
            "owner_open_id": pending.owner_open_id,
            "brand": feishu_setup_brand_label(pending.brand),
            "base_url": pending.brand.provider_base_url(),
        }));
    }
    if now_ms() >= pending.expires_at_ms {
        service.feishu_setup_pending.write().remove(session_id);
        save_pending_feishu_setups(service);
        return json_response(&serde_json::json!({
            "success": true,
            "status": "expired",
            "expires_at": pending.expires_at_ms,
        }));
    }
    match poll_feishu_app_registration_once(pending.brand, &pending.device_code).await {
        Ok(FeishuAppRegistrationPoll::Pending) => json_response(&serde_json::json!({
            "success": true,
            "status": "pending",
            "expires_at": pending.expires_at_ms,
            "interval_seconds": pending.interval_seconds,
        })),
        Ok(FeishuAppRegistrationPoll::Confirmed(result)) => {
            let mut final_brand = pending.brand;
            if result.app_secret.is_empty()
                && result
                    .tenant_brand
                    .as_deref()
                    .is_some_and(|tenant| tenant == "lark")
            {
                match poll_feishu_app_registration_once(
                    FeishuSetupBrand::Lark,
                    &pending.device_code,
                )
                .await
                {
                    Ok(FeishuAppRegistrationPoll::Confirmed(lark_result)) => {
                        final_brand = FeishuSetupBrand::Lark;
                        return persist_and_respond_feishu_setup(
                            service,
                            session_id,
                            pending,
                            final_brand,
                            lark_result,
                        )
                        .await;
                    }
                    Ok(FeishuAppRegistrationPoll::Pending) => {}
                    Ok(FeishuAppRegistrationPoll::Expired) => {
                        service.feishu_setup_pending.write().remove(session_id);
                        save_pending_feishu_setups(service);
                        return json_response(&serde_json::json!({
                            "success": true,
                            "status": "expired",
                            "expires_at": pending.expires_at_ms,
                        }));
                    }
                    Err(error) => {
                        warn!(
                            session_id,
                            error = %error,
                            "Feishu setup lark endpoint retry failed"
                        );
                    }
                }
            }
            persist_and_respond_feishu_setup(service, session_id, pending, final_brand, result)
                .await
        }
        Ok(FeishuAppRegistrationPoll::Expired) => {
            service.feishu_setup_pending.write().remove(session_id);
            save_pending_feishu_setups(service);
            json_response(&serde_json::json!({
                "success": true,
                "status": "expired",
                "expires_at": pending.expires_at_ms,
            }))
        }
        Err(e) => error_response(
            StatusCode::BAD_GATEWAY,
            &format!("Failed to check Feishu setup: {e}"),
        ),
    }
}

async fn persist_and_respond_feishu_setup(
    service: &ImGatewayService,
    session_id: &str,
    pending: PendingFeishuSetup,
    brand: FeishuSetupBrand,
    result: FeishuAppRegistrationResult,
) -> Response<BoxBody> {
    match persist_confirmed_feishu_setup(service, session_id, pending, brand, result).await {
        Ok(pending) => json_response(&confirmed_feishu_setup_response(&pending, service)),
        Err(error) => error_response(StatusCode::BAD_GATEWAY, &error),
    }
}

async fn persist_confirmed_feishu_setup(
    service: &ImGatewayService,
    session_id: &str,
    mut pending: PendingFeishuSetup,
    brand: FeishuSetupBrand,
    result: FeishuAppRegistrationResult,
) -> Result<PendingFeishuSetup, String> {
    if result.app_id.is_empty() || result.app_secret.is_empty() {
        return Err("Feishu setup completed but app credentials were incomplete".to_string());
    }
    pending.app_id = Some(result.app_id);
    pending.app_secret = Some(result.app_secret);
    pending.owner_open_id = result.owner_open_id;
    pending.brand = brand;
    if let Err(error) = maybe_create_pending_feishu_setup_provider(service, &mut pending).await {
        warn!(
            session_id,
            error = %error,
            "failed to create Feishu provider from persisted setup draft"
        );
    }
    service
        .feishu_setup_pending
        .write()
        .insert(session_id.to_string(), pending.clone());
    save_pending_feishu_setups(service);
    Ok(pending)
}

fn confirmed_feishu_setup_response(
    pending: &PendingFeishuSetup,
    service: &ImGatewayService,
) -> serde_json::Value {
    let mut response = serde_json::json!({
        "success": true,
        "status": "confirmed",
        "app_id": pending.app_id,
        "secret_configured": pending.app_secret.as_deref().is_some_and(|s| !s.is_empty()),
        "owner_open_id": pending.owner_open_id,
        "brand": feishu_setup_brand_label(pending.brand),
        "base_url": pending.brand.provider_base_url(),
    });
    if let Some(provider_id) = pending.created_provider_id.as_deref() {
        response["provider_id"] = serde_json::Value::String(provider_id.to_string());
        if let Some(provider) = service.provider_store.get(provider_id) {
            response["provider"] = sanitize_provider(&provider);
        }
    }
    response
}

async fn maybe_create_pending_feishu_setup_provider(
    service: &ImGatewayService,
    pending: &mut PendingFeishuSetup,
) -> Result<Option<String>, String> {
    if let Some(provider_id) = pending.created_provider_id.as_deref() {
        if service.provider_store.get(provider_id).is_some() {
            return Ok(Some(provider_id.to_string()));
        }
    }
    let Some(mut payload) = pending.provider_payload.clone() else {
        return Ok(None);
    };
    let Some(app_id) = pending.app_id.clone() else {
        return Ok(None);
    };
    let Some(app_secret) = pending.app_secret.clone() else {
        return Ok(None);
    };
    payload["provider_type"] = serde_json::Value::String("feishu".to_string());
    payload["app_id"] = serde_json::Value::String(app_id);
    payload["app_secret"] = serde_json::Value::String(app_secret);
    payload["base_url"] = serde_json::Value::String(pending.brand.provider_base_url().to_string());
    if payload
        .get("owner_open_id")
        .and_then(|value| value.as_str())
        .is_none_or(|value| value.trim().is_empty())
    {
        if let Some(owner) = pending.owner_open_id.as_deref() {
            payload["owner_open_id"] = serde_json::Value::String(owner.to_string());
        }
    }
    let config = parse_feishu_setup_provider_payload(payload)?;
    let provider_id = config.id.clone();
    if service.provider_store.get(&provider_id).is_some() {
        return Err(format!("provider with id '{}' already exists", provider_id));
    }
    service
        .provider_store
        .add(config.clone())
        .map_err(|error| error.to_string())?;
    pending.created_provider_id = Some(config.id.clone());
    if pending.auto_connect {
        if let Err(error) = start_provider_event_connection(service, &config.id).await {
            warn!(
                provider_id = %config.id,
                error = %error,
                "failed to auto-connect Feishu provider created from setup draft"
            );
        }
    }
    Ok(Some(config.id))
}

fn parse_feishu_setup_provider_payload(
    payload: serde_json::Value,
) -> Result<ImProviderConfig, String> {
    let mut config = parse_provider_create_payload(payload)
        .map_err(|error| format!("Invalid request body: {error}"))?;
    config.enabled = true;
    config.event_connection_enabled = true;
    if config.event_types.is_empty() {
        config.event_types = vec!["message.receive".to_string()];
    }
    let now = now_ms();
    if config.created_at == 0 {
        config.created_at = now;
    }
    config.updated_at = now;
    normalize_provider_agent_config(&mut config);
    Ok(config)
}

pub(super) async fn poll_and_complete_feishu_setup_session(
    service: &ImGatewayService,
    session_id: &str,
) -> Result<(), String> {
    let Some(pending) = service.feishu_setup_pending.read().get(session_id).cloned() else {
        return Err("Feishu setup session not found".to_string());
    };
    if pending.created_provider_id.is_some() {
        return Ok(());
    }
    if pending.app_id.is_some() {
        let mut pending = pending;
        maybe_create_pending_feishu_setup_provider(service, &mut pending).await?;
        service
            .feishu_setup_pending
            .write()
            .insert(session_id.to_string(), pending);
        save_pending_feishu_setups(service);
        return Ok(());
    }
    if now_ms() >= pending.expires_at_ms {
        service.feishu_setup_pending.write().remove(session_id);
        save_pending_feishu_setups(service);
        return Err("Feishu setup session expired before confirmation".to_string());
    }
    let mut result =
        match poll_feishu_app_registration_once(pending.brand, &pending.device_code).await? {
            FeishuAppRegistrationPoll::Pending => return Err("authorization pending".to_string()),
            FeishuAppRegistrationPoll::Expired => {
                service.feishu_setup_pending.write().remove(session_id);
                save_pending_feishu_setups(service);
                return Err("Feishu setup session expired before confirmation".to_string());
            }
            FeishuAppRegistrationPoll::Confirmed(result) => result,
        };
    let brand = if result.app_secret.is_empty()
        && result
            .tenant_brand
            .as_deref()
            .is_some_and(|tenant| tenant == "lark")
    {
        match poll_feishu_app_registration_once(FeishuSetupBrand::Lark, &pending.device_code).await
        {
            Ok(FeishuAppRegistrationPoll::Confirmed(lark_result)) => {
                result = lark_result;
                FeishuSetupBrand::Lark
            }
            Ok(FeishuAppRegistrationPoll::Expired) => {
                service.feishu_setup_pending.write().remove(session_id);
                save_pending_feishu_setups(service);
                return Err("Feishu setup session expired before confirmation".to_string());
            }
            Ok(FeishuAppRegistrationPoll::Pending) | Err(_) => pending.brand,
        }
    } else {
        pending.brand
    };
    persist_confirmed_feishu_setup(service, session_id, pending, brand, result).await?;
    Ok(())
}

pub(super) async fn handle_provider_feishu_setup_create_provider(
    req: Request<Incoming>,
    service: &ImGatewayService,
    session_id: &str,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }
    let payload: serde_json::Value = match read_body_json(req).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let Some(pending) = service.feishu_setup_pending.read().get(session_id).cloned() else {
        return error_response(StatusCode::NOT_FOUND, "Feishu setup session not found");
    };
    if let Some(provider_id) = pending.created_provider_id.as_deref() {
        let Some(provider) = service.provider_store.get(provider_id) else {
            return error_response(
                StatusCode::NOT_FOUND,
                "Feishu setup provider was created but is no longer configured",
            );
        };
        service.feishu_setup_pending.write().remove(session_id);
        save_pending_feishu_setups(service);
        return json_response(&serde_json::json!({
            "success": true,
            "provider": sanitize_provider(&provider),
        }));
    }
    let (Some(app_id), Some(app_secret)) = (pending.app_id.clone(), pending.app_secret.clone())
    else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Feishu setup has not completed yet",
        );
    };
    let mut payload = payload;
    payload["provider_type"] = serde_json::Value::String("feishu".to_string());
    payload["app_id"] = serde_json::Value::String(app_id);
    payload["app_secret"] = serde_json::Value::String(app_secret);
    payload["base_url"] = serde_json::Value::String(pending.brand.provider_base_url().to_string());
    if payload
        .get("owner_open_id")
        .and_then(|value| value.as_str())
        .is_none_or(|value| value.trim().is_empty())
    {
        if let Some(owner) = pending.owner_open_id.as_deref() {
            payload["owner_open_id"] = serde_json::Value::String(owner.to_string());
        }
    }
    let mut config = match parse_provider_create_payload(payload) {
        Ok(v) => v,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Invalid request body: {e}"),
            );
        }
    };
    config.enabled = true;
    config.event_connection_enabled = true;
    if config.event_types.is_empty() {
        config.event_types = vec!["message.receive".to_string()];
    }
    let now = now_ms();
    if config.created_at == 0 {
        config.created_at = now;
    }
    config.updated_at = now;
    normalize_provider_agent_config(&mut config);
    match service.provider_store.add(config.clone()) {
        Ok(()) => {
            service.feishu_setup_pending.write().remove(session_id);
            save_pending_feishu_setups(service);
            json_response(&serde_json::json!({
                "success": true,
                "provider": sanitize_provider(&config),
            }))
        }
        Err(e) => error_response(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

/// POST /providers/:id/connect — start event long connection for a provider.
///
/// If `owner_open_id` is not configured, this will auto-detect it from the
/// Feishu Application Info API and persist it to the provider store.
pub(super) async fn handle_provider_connect(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }

    match start_provider_event_connection(service, id).await {
        Ok(()) => {
            json_response(&serde_json::json!({"success": true, "message": "Connection started"}))
        }
        Err(error) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to start connection: {error}"),
        ),
    }
}

async fn start_provider_event_connection(
    service: &ImGatewayService,
    id: &str,
) -> Result<(), String> {
    let Some(mut provider) = service.provider_store.get(id) else {
        return Err("Provider not found".to_string());
    };

    let app_secret = match provider.secret_ref.as_deref() {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => return Err("Provider has no secret configured".to_string()),
    };

    // Auto-detect owner_open_id if not set for Feishu. Weixin gets owner from QR login.
    let feishu = service.connection_manager.feishu_provider().clone();
    if provider.provider_type == crate::im_gateway::types::ImProviderType::Feishu
        && provider
            .owner_open_id
            .as_deref()
            .unwrap_or_default()
            .is_empty()
    {
        match feishu.fetch_bot_owner_open_id(&provider).await {
            Ok(owner_id) => {
                info!(
                    provider_id = id,
                    owner_open_id = %owner_id,
                    "auto-detected bot owner"
                );
                provider.owner_open_id = Some(owner_id);
                // Persist to store
                if let Err(e) = service.provider_store.update(provider.clone()) {
                    warn!(provider_id = id, error = %e, "failed to persist auto-detected owner_open_id");
                }
            }
            Err(e) => {
                warn!(
                    provider_id = id,
                    error = %e,
                    "failed to auto-detect owner, connection will proceed without owner filter"
                );
            }
        }
    }

    // Create event channel
    let (tx, rx) = mpsc::unbounded_channel::<ImEvent>();

    // Spawn the event processing loop
    let client = service.provider_client(&provider);
    let provider_for_loop = provider.clone();
    let event_store = service.event_store.clone();
    let message_log_store = service.message_log_store.clone();
    let route_store = service.route_store.clone();
    let provider_store = service.provider_store.clone();
    let agent_config_store = service.agent_config_store.clone();
    let agent_client = service.agent_client.clone();
    let agent_tools = service.agent_tools.clone();
    let schedule_store = service.schedule_store.clone();
    let scheduler = service.scheduler.clone();
    let target_store = service.target_store.clone();
    let connection_manager = service.connection_manager.clone();
    let agent_session_manager = service.agent_session_manager.clone();
    let external_cli_config_store = service.external_cli_config_store.clone();
    let queue_manager = service.queue_manager.clone();
    let progress_registry = service.progress_registry.clone();
    tokio::spawn(async move {
        run_event_loop(
            rx,
            client,
            provider_for_loop,
            event_store,
            message_log_store,
            route_store,
            provider_store,
            agent_config_store,
            agent_client,
            agent_tools,
            schedule_store,
            scheduler,
            target_store,
            connection_manager,
            agent_session_manager,
            external_cli_config_store,
            queue_manager,
            progress_registry,
        )
        .await;
    });

    // Start the long connection
    match service
        .connection_manager
        .start_connection(&provider, &app_secret, tx)
        .await
    {
        Ok(()) => {
            info!(provider_id = id, "provider event connection started");
            Ok(())
        }
        Err(e) => {
            service.connection_manager.mark_failed(id, e.to_string());
            Err(e.to_string())
        }
    }
}

/// POST /providers/:id/disconnect — stop event long connection.
pub(super) async fn handle_provider_disconnect(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }

    if service.provider_store.get(id).is_none() {
        return error_response(StatusCode::NOT_FOUND, "Provider not found");
    }

    service.connection_manager.stop_connection(id);
    info!(provider_id = id, "provider event connection stopped");
    json_response(&serde_json::json!({"success": true, "message": "Connection stopped"}))
}

/// GET /providers/:id/messages — list message logs for a provider.
///   Query params: ?direction=inbound|outbound  &source=user|bot
/// DELETE /providers/:id/messages — clear message logs for a provider.
pub(super) async fn handle_provider_messages(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    if service.provider_store.get(id).is_none() {
        return error_response(StatusCode::NOT_FOUND, "Provider not found");
    }

    match *req.method() {
        Method::GET => {
            let query = req.uri().query().unwrap_or("");
            let params = parse_query_params(query);
            let direction_filter = params.get("direction").map(|s| s.as_str());
            let source_filter = params.get("source").map(|s| s.as_str());

            let mut messages = service.message_log_store.list_by_provider(id);

            // Filter by direction
            if let Some(dir) = direction_filter {
                messages.retain(|m| match dir {
                    "inbound" => matches!(m.direction, MessageDirection::Inbound),
                    "outbound" => matches!(m.direction, MessageDirection::Outbound),
                    _ => true,
                });
            }

            // Filter by source: "user" = inbound from user, "bot" = outbound from bot
            if let Some(src) = source_filter {
                messages.retain(|m| match src {
                    "user" => matches!(m.direction, MessageDirection::Inbound),
                    "bot" => matches!(m.direction, MessageDirection::Outbound),
                    _ => true,
                });
            }

            json_response(&messages)
        }
        Method::DELETE => match service.message_log_store.clear_by_provider(id) {
            Ok(()) => {
                json_response(&serde_json::json!({"success": true, "message": "Messages cleared"}))
            }
            Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        },
        _ => method_not_allowed(),
    }
}

/// Build a session key that is unique per (provider, user) pair.
///
/// This ensures that the same user chatting through different bots gets
/// independent agent sessions, preventing cross-bot history contamination.
pub(super) fn build_session_key(provider_id: &str, user_id: Option<&str>) -> String {
    let user = user_id.unwrap_or("unknown");
    format!("{provider_id}:{user}")
}

pub(super) fn effective_agent_config_for_provider(
    base: &crate::im_gateway::agent::ImAgentConfig,
    provider: &ImProviderConfig,
) -> crate::im_gateway::agent::ImAgentConfig {
    let mut config = base.clone();
    if let Some(agent_config) = provider.agent_config.as_ref() {
        if let Some(runner) = agent_config.runner.as_ref() {
            config.runner = Some(runner.clone());
        }
        if let Some(work_dir) = agent_config
            .work_dir
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            config.work_dir = Some(work_dir.to_string());
        }
        if let Some(instructions) = agent_config
            .base_instructions
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            config.base_instructions = Some(instructions.to_string());
        }
        if let Some(instructions) = agent_config
            .developer_instructions
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            config.developer_instructions = Some(instructions.to_string());
        }
        if let Some(instructions) = agent_config
            .user_instructions
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            config.user_instructions = Some(instructions.to_string());
        }
    }
    config
}

pub(super) fn effective_agent_work_dir_for_provider(
    base: &crate::im_gateway::agent::ImAgentConfig,
    provider: &ImProviderConfig,
) -> Option<std::path::PathBuf> {
    Some(effective_agent_config_for_provider(base, provider).resolve_work_dir())
}

struct FeishuAppRegistrationStart {
    device_code: String,
    verification_url: String,
    expires_in_seconds: u64,
    interval_seconds: u64,
}

struct FeishuAppRegistrationResult {
    app_id: String,
    app_secret: String,
    owner_open_id: Option<String>,
    tenant_brand: Option<String>,
}

enum FeishuAppRegistrationPoll {
    Pending,
    Confirmed(FeishuAppRegistrationResult),
    Expired,
}

pub(super) fn parse_feishu_setup_brand(value: Option<&str>) -> FeishuSetupBrand {
    match value {
        Some("lark") => FeishuSetupBrand::Lark,
        _ => FeishuSetupBrand::Feishu,
    }
}

fn feishu_setup_brand_label(brand: FeishuSetupBrand) -> &'static str {
    match brand {
        FeishuSetupBrand::Feishu => "feishu",
        FeishuSetupBrand::Lark => "lark",
    }
}

async fn request_feishu_app_registration(
    brand: FeishuSetupBrand,
) -> Result<FeishuAppRegistrationStart, String> {
    let client = bifrost_core::outbound_reqwest_client_builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(feishu_setup_reqwest_error)?;
    let endpoint = format!("{}/oauth/v1/app/registration", brand.accounts_base());
    let response = client
        .post(endpoint)
        .form(&[
            ("action", "begin"),
            ("archetype", "PersonalAgent"),
            ("auth_method", "client_secret"),
            ("request_user_info", "open_id tenant_brand"),
        ])
        .send()
        .await
        .map_err(feishu_setup_reqwest_error)?;
    let status = response.status();
    let data: serde_json::Value = response.json().await.map_err(feishu_setup_reqwest_error)?;
    if !status.is_success() || data.get("error").is_some() {
        return Err(feishu_setup_error_message(&data, status));
    }
    let user_code = json_string(&data, "user_code")
        .ok_or_else(|| "app registration response missing user_code".to_string())?;
    let device_code = json_string(&data, "device_code")
        .ok_or_else(|| "app registration response missing device_code".to_string())?;
    let expires_in_seconds = json_u64(&data, "expires_in").unwrap_or(300);
    let interval_seconds = json_u64(&data, "interval").unwrap_or(5);
    let verification_url = json_string(&data, "verification_uri_complete")
        .or_else(|| {
            json_string(&data, "verification_uri").map(|uri| format!("{uri}?user_code={user_code}"))
        })
        .unwrap_or_else(|| {
            format!(
                "{}/page/launcher?user_code={}",
                brand.open_base(),
                user_code
            )
        });
    Ok(FeishuAppRegistrationStart {
        device_code,
        verification_url,
        expires_in_seconds,
        interval_seconds,
    })
}

async fn poll_feishu_app_registration_once(
    brand: FeishuSetupBrand,
    device_code: &str,
) -> Result<FeishuAppRegistrationPoll, String> {
    let client = bifrost_core::outbound_reqwest_client_builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(feishu_setup_reqwest_error)?;
    let endpoint = format!("{}/oauth/v1/app/registration", brand.accounts_base());
    let response = client
        .post(endpoint)
        .form(&[("action", "poll"), ("device_code", device_code)])
        .send()
        .await
        .map_err(feishu_setup_reqwest_error)?;
    let status = response.status();
    let data: serde_json::Value = response.json().await.map_err(feishu_setup_reqwest_error)?;
    let error = json_string(&data, "error").unwrap_or_default();
    if error.is_empty() {
        let app_id = json_string(&data, "client_id").unwrap_or_default();
        if app_id.is_empty() {
            return Err("app registration poll response missing client_id".to_string());
        }
        let user_info = data.get("user_info").and_then(|value| value.as_object());
        let owner_open_id = user_info.and_then(|obj| {
            obj.get("open_id")
                .and_then(|value| value.as_str())
                .map(str::to_string)
        });
        let tenant_brand = user_info.and_then(|obj| {
            obj.get("tenant_brand")
                .and_then(|value| value.as_str())
                .map(str::to_string)
        });
        return Ok(FeishuAppRegistrationPoll::Confirmed(
            FeishuAppRegistrationResult {
                app_id,
                app_secret: json_string(&data, "client_secret").unwrap_or_default(),
                owner_open_id,
                tenant_brand,
            },
        ));
    }
    match error.as_str() {
        "authorization_pending" | "slow_down" => Ok(FeishuAppRegistrationPoll::Pending),
        "expired_token" | "invalid_grant" => Ok(FeishuAppRegistrationPoll::Expired),
        "access_denied" => Err("app registration denied by user".to_string()),
        _ => Err(feishu_setup_error_message(&data, status)),
    }
}

fn feishu_setup_reqwest_error(error: reqwest::Error) -> String {
    let mut message = error.to_string();
    let mut source = std::error::Error::source(&error);
    while let Some(error) = source {
        message.push_str(": ");
        message.push_str(&error.to_string());
        source = error.source();
    }
    message
}

fn feishu_setup_error_message(data: &serde_json::Value, status: reqwest::StatusCode) -> String {
    json_string(data, "error_description")
        .or_else(|| json_string(data, "error"))
        .unwrap_or_else(|| format!("HTTP {status}"))
}

fn json_string(data: &serde_json::Value, key: &str) -> Option<String> {
    data.get(key)
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn json_u64(data: &serde_json::Value, key: &str) -> Option<u64> {
    data.get(key).and_then(|value| {
        value
            .as_u64()
            .or_else(|| value.as_i64().and_then(|n| u64::try_from(n).ok()))
    })
}

// ---------------------------------------------------------------------------

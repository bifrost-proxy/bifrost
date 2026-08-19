use bifrost_admin::im_gateway::types::{ImProviderConfig, ImProviderType};
use bifrost_admin::{AdminRouter, AdminState, ImGatewayService, PushManager};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, StatusCode};
use hyper_util::rt::TokioIo;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::net::TcpListener;

async fn start_admin(service: Arc<ImGatewayService>) -> (String, tokio::task::JoinHandle<()>) {
    let state = Arc::new(AdminState::new(0));
    state.set_im_gateway_service(service);
    let push_manager = Arc::new(PushManager::new(state.clone()));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, peer)) = listener.accept().await else {
                return;
            };
            let state = state.clone();
            let push_manager = push_manager.clone();
            tokio::spawn(async move {
                let service = service_fn(move |request: Request<Incoming>| {
                    let state = state.clone();
                    let push_manager = push_manager.clone();
                    async move {
                        Ok::<_, hyper::Error>(
                            AdminRouter::handle(request, state, Some(push_manager), Some(peer))
                                .await,
                        )
                    }
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    (format!("http://{address}/_bifrost/api/im-gateway"), task)
}

fn provider(id: &str) -> ImProviderConfig {
    ImProviderConfig {
        id: id.to_string(),
        provider_type: ImProviderType::Webhook,
        display_name: "Isolated provider API test".to_string(),
        enabled: true,
        base_url: None,
        app_id: None,
        secret_ref: None,
        owner_open_id: None,
        event_connection_enabled: true,
        event_types: Vec::new(),
        agent_config: None,
        created_at: 1,
        updated_at: 1,
    }
}

#[tokio::test]
async fn isolated_provider_status_disconnect_disable_and_delete_do_not_require_a_worker() {
    let data_dir = TempDir::new().unwrap();
    let service = Arc::new(ImGatewayService::new(data_dir.path()));
    service
        .provider_store
        .add(provider("isolated-api"))
        .unwrap();
    let (base, server) = start_admin(service.clone()).await;
    let client = reqwest::Client::new();

    let status = client
        .get(format!("{base}/providers/isolated-api/status"))
        .send()
        .await
        .unwrap();
    assert_eq!(status.status(), StatusCode::OK);

    let disconnect = client
        .post(format!("{base}/providers/isolated-api/disconnect"))
        .send()
        .await
        .unwrap();
    assert_eq!(disconnect.status(), StatusCode::OK);

    let disable = client
        .patch(format!("{base}/providers/isolated-api"))
        .json(&serde_json::json!({"enabled": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(disable.status(), StatusCode::OK);
    assert!(!service.provider_store.get("isolated-api").unwrap().enabled);

    let delete = client
        .delete(format!("{base}/providers/isolated-api"))
        .send()
        .await
        .unwrap();
    assert_eq!(delete.status(), StatusCode::OK);
    assert!(service.provider_store.get("isolated-api").is_none());

    server.abort();
}

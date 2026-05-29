use std::path::Path;

pub fn direct_reqwest_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder().no_proxy()
}

pub fn direct_blocking_reqwest_client_builder() -> reqwest::blocking::ClientBuilder {
    reqwest::blocking::Client::builder().no_proxy()
}

pub fn load_reqwest_certificate(path: &Path) -> std::result::Result<reqwest::Certificate, String> {
    let pem = std::fs::read(path).map_err(|error| format!("read CA certificate: {error}"))?;
    reqwest::Certificate::from_pem(&pem).map_err(|error| format!("parse CA certificate: {error}"))
}

pub fn proxied_reqwest_client_builder(
    proxy_url: &str,
    ca_cert_path: Option<&Path>,
) -> std::result::Result<reqwest::ClientBuilder, String> {
    let proxy = reqwest::Proxy::all(proxy_url)
        .map_err(|error| format!("invalid proxy URL '{proxy_url}': {error}"))?;
    let mut builder = direct_reqwest_client_builder().proxy(proxy);
    if let Some(path) = ca_cert_path {
        match load_reqwest_certificate(path) {
            Ok(cert) => {
                builder = builder.add_root_certificate(cert);
            }
            Err(error) => {
                tracing::warn!(
                    proxy_url = %proxy_url,
                    ca_cert_path = %path.display(),
                    error = %error,
                    "proxied HTTP client could not load CA; TLS-intercepted HTTPS requests may fail"
                );
            }
        }
    }
    Ok(builder)
}

pub fn direct_ureq_agent_builder() -> ureq::AgentBuilder {
    ureq::AgentBuilder::new().try_proxy_from_env(false)
}

pub fn direct_ureq_agent() -> ureq::Agent {
    direct_ureq_agent_builder().build()
}

#[cfg(test)]
mod tests {
    use super::{
        direct_blocking_reqwest_client_builder, direct_reqwest_client_builder,
        direct_ureq_agent_builder,
    };
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Mutex, OnceLock};
    use std::thread;
    use std::time::Duration;

    fn proxy_env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn with_invalid_proxy_env<T>(f: impl FnOnce() -> T) -> T {
        let _guard = proxy_env_lock()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let vars = ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY"];
        let saved: Vec<(String, Option<String>)> = vars
            .iter()
            .map(|key| (key.to_string(), std::env::var(key).ok()))
            .collect();

        for key in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"] {
            std::env::set_var(key, "http://127.0.0.1:1");
        }
        std::env::remove_var("NO_PROXY");

        let result = f();

        for (key, value) in saved {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }

        result
    }

    fn spawn_local_http_server() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0_u8; 1024];
            let _ = stream.read(&mut buffer);
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
            let _ = stream.flush();
        });
        format!("http://{addr}")
    }

    #[test]
    fn blocking_reqwest_builder_bypasses_proxy_env() {
        with_invalid_proxy_env(|| {
            let url = spawn_local_http_server();
            let response = direct_blocking_reqwest_client_builder()
                .timeout(Duration::from_secs(2))
                .build()
                .unwrap()
                .get(url)
                .send()
                .unwrap()
                .text()
                .unwrap();
            assert_eq!(response, "ok");
        });
    }

    #[test]
    fn async_reqwest_builder_bypasses_proxy_env() {
        with_invalid_proxy_env(|| {
            let url = spawn_local_http_server();
            let runtime = tokio::runtime::Runtime::new().unwrap();
            let response = runtime.block_on(async move {
                direct_reqwest_client_builder()
                    .timeout(Duration::from_secs(2))
                    .build()
                    .unwrap()
                    .get(url)
                    .send()
                    .await
                    .unwrap()
                    .text()
                    .await
                    .unwrap()
            });
            assert_eq!(response, "ok");
        });
    }

    #[test]
    fn ureq_builder_bypasses_proxy_env() {
        with_invalid_proxy_env(|| {
            let url = spawn_local_http_server();
            let response = direct_ureq_agent_builder()
                .timeout(Duration::from_secs(2))
                .build()
                .get(&url)
                .call()
                .unwrap()
                .into_string()
                .unwrap();
            assert_eq!(response, "ok");
        });
    }
}

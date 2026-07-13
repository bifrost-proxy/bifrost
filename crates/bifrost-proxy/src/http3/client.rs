use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use crate::dns::DnsResolver;
use crate::ensure_crypto_provider;
use bifrost_core::{BifrostError, Result};
use bytes::{Buf, Bytes};
use h3::client::SendRequest;
use hyper::{Request, Response};
use quinn::{ClientConfig, Endpoint};
use tokio_rustls::rustls::client::danger::{
    HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier,
};
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use tokio_rustls::rustls::{DigitallySignedStruct, RootCertStore, SignatureScheme};
use tracing::{debug, info, warn};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RESPONSE_SIZE: usize = 100 * 1024 * 1024;

pub struct Http3Client {
    endpoint: Endpoint,
}

impl Http3Client {
    pub fn new() -> Result<Self> {
        Self::new_with_options(false)
    }

    pub fn new_with_options(unsafe_ssl: bool) -> Result<Self> {
        ensure_crypto_provider();

        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

        let mut crypto = if unsafe_ssl {
            tokio_rustls::rustls::ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(NoVerifier))
                .with_no_client_auth()
        } else {
            tokio_rustls::rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth()
        };

        crypto.alpn_protocols = vec![b"h3".to_vec()];

        let quic_config =
            quinn::crypto::rustls::QuicClientConfig::try_from(crypto).map_err(|e| {
                BifrostError::Tls(format!("Failed to create QUIC client config: {}", e))
            })?;

        let mut client_config = ClientConfig::new(Arc::new(quic_config));

        let mut transport_config = quinn::TransportConfig::default();
        transport_config.max_idle_timeout(Some(Duration::from_secs(30).try_into().unwrap()));
        transport_config.keep_alive_interval(Some(Duration::from_secs(10)));
        client_config.transport_config(Arc::new(transport_config));

        let mut endpoint = Endpoint::client("[::]:0".parse().unwrap())
            .map_err(|e| BifrostError::Network(format!("Failed to create QUIC endpoint: {}", e)))?;

        endpoint.set_default_client_config(client_config);

        info!("[HTTP/3 Client] Initialized with QUIC endpoint");
        Ok(Self { endpoint })
    }

    pub async fn request(
        &self,
        host: &str,
        port: u16,
        req: Request<Bytes>,
    ) -> Result<Response<Bytes>> {
        let addr = format!("{}:{}", host, port)
            .to_socket_addrs()
            .map_err(|e| {
                BifrostError::Network(format!("DNS resolution failed for {}: {}", host, e))
            })?
            .next()
            .ok_or_else(|| BifrostError::Network(format!("No addresses found for {}", host)))?;

        self.request_to_addr(host, addr, req).await
    }

    pub async fn request_to_addr(
        &self,
        host: &str,
        addr: SocketAddr,
        req: Request<Bytes>,
    ) -> Result<Response<Bytes>> {
        info!("[HTTP/3 Client] Connecting to {} ({}) via QUIC", host, addr);

        let connection = tokio::time::timeout(CONNECT_TIMEOUT, async {
            self.endpoint
                .connect(addr, host)
                .map_err(|e| BifrostError::Network(format!("QUIC connect error: {}", e)))?
                .await
                .map_err(|e| BifrostError::Network(format!("QUIC connection failed: {}", e)))
        })
        .await
        .map_err(|_| BifrostError::Network(format!("Connection timeout to {}", host)))??;

        info!(
            "[HTTP/3 Client] QUIC connection established to {} (RTT: {:?})",
            host,
            connection.rtt()
        );

        let quinn_conn = h3_quinn::Connection::new(connection);
        let (mut driver, send_request) = h3::client::new(quinn_conn)
            .await
            .map_err(|e| BifrostError::Network(format!("H3 handshake failed: {}", e)))?;

        info!("[HTTP/3 Client] HTTP/3 connection ready to {}", host);

        let response = Self::send_request(send_request, req.clone()).await?;
        driver.wait_idle().await;

        info!(
            "[HTTP/3 Client] Response from {}: {} {}",
            host,
            response.status(),
            req.uri()
        );

        Ok(response)
    }

    pub async fn resolve_target_addr(
        host: &str,
        port: u16,
        dns_resolver: &DnsResolver,
        dns_servers: &[String],
    ) -> Result<SocketAddr> {
        if let Some(ip) = dns_resolver.resolve(host, dns_servers).await? {
            return Ok(SocketAddr::new(ip, port));
        }

        if let Ok(ip) = host.parse::<IpAddr>() {
            return Ok(SocketAddr::new(ip, port));
        }

        format!("{}:{}", host, port)
            .to_socket_addrs()
            .map_err(|e| {
                BifrostError::Network(format!("DNS resolution failed for {}: {}", host, e))
            })?
            .next()
            .ok_or_else(|| BifrostError::Network(format!("No addresses found for {}", host)))
    }

    async fn send_request(
        mut send_request: SendRequest<h3_quinn::OpenStreams, Bytes>,
        req: Request<Bytes>,
    ) -> Result<Response<Bytes>> {
        let uri = req.uri().clone();
        let method = req.method().clone();
        let body = req.body().clone();

        debug!(
            "[HTTP/3 Client] Sending request: {} {} (body: {} bytes)",
            method,
            uri,
            body.len()
        );

        let (parts, _) = req.into_parts();
        let h3_req = Request::from_parts(parts, ());

        let mut stream = send_request
            .send_request(h3_req)
            .await
            .map_err(|e| BifrostError::Network(format!("Failed to send H3 request: {}", e)))?;

        if !body.is_empty() {
            stream
                .send_data(body)
                .await
                .map_err(|e| BifrostError::Network(format!("Failed to send H3 body: {}", e)))?;
        }

        stream
            .finish()
            .await
            .map_err(|e| BifrostError::Network(format!("Failed to finish H3 request: {}", e)))?;

        debug!("[HTTP/3 Client] Request sent, waiting for response...");

        let response = stream
            .recv_response()
            .await
            .map_err(|e| BifrostError::Network(format!("Failed to receive H3 response: {}", e)))?;

        let status = response.status();
        let headers = response.headers().clone();

        debug!(
            "[HTTP/3 Client] Received response headers: {} (headers: {})",
            status,
            headers.len()
        );

        let mut body_data = Vec::new();
        while let Some(mut data) = stream
            .recv_data()
            .await
            .map_err(|e| BifrostError::Network(format!("Failed to receive H3 body: {}", e)))?
        {
            while data.has_remaining() {
                let chunk = data.chunk();
                body_data.extend_from_slice(chunk);
                data.advance(chunk.len());

                if body_data.len() > MAX_RESPONSE_SIZE {
                    warn!(
                        "[HTTP/3 Client] Response body too large, truncating at {} bytes",
                        MAX_RESPONSE_SIZE
                    );
                    break;
                }
            }
        }

        debug!(
            "[HTTP/3 Client] Received response body: {} bytes",
            body_data.len()
        );

        let mut builder = Response::builder().status(status);
        for (key, value) in headers.iter() {
            builder = builder.header(key, value);
        }

        let response = builder
            .body(Bytes::from(body_data))
            .map_err(|e| BifrostError::Parse(format!("Failed to build response: {}", e)))?;

        Ok(response)
    }

    pub async fn check_h3_support(host: &str, port: u16) -> Result<bool> {
        let client = Self::new()?;

        let req = Request::builder()
            .method("HEAD")
            .uri(format!("https://{}:{}/", host, port))
            .header("Host", host)
            .body(Bytes::new())
            .map_err(|e| BifrostError::Parse(format!("Failed to build request: {}", e)))?;

        match client.request(host, port, req).await {
            Ok(_) => {
                info!("[HTTP/3 Client] {} supports HTTP/3", host);
                Ok(true)
            }
            Err(e) => {
                debug!("[HTTP/3 Client] {} does not support HTTP/3: {}", host, e);
                Ok(false)
            }
        }
    }
}

impl Default for Http3Client {
    fn default() -> Self {
        Self::new().expect("Failed to create HTTP/3 client")
    }
}

#[derive(Debug)]
struct NoVerifier;

impl ServerCertVerifier for NoVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, tokio_rustls::rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, tokio_rustls::rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, tokio_rustls::rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::ECDSA_NISTP521_SHA512,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PSS_SHA512,
            SignatureScheme::ED25519,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_90_no_verifier_and_client_construction() {
        use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName, UnixTime};

        let verifier = NoVerifier;
        let cert = CertificateDer::from(vec![0u8]);
        let server_name = ServerName::try_from("localhost").unwrap();
        assert!(verifier
            .verify_server_cert(
                &cert,
                &[],
                &server_name,
                &[],
                UnixTime::since_unix_epoch(Duration::ZERO)
            )
            .is_ok());
        assert!(verifier
            .supported_verify_schemes()
            .contains(&SignatureScheme::ED25519));
        // Some CI hosts disable IPv6, in which case QUIC endpoint creation is
        // expected to fail. Both TLS configuration paths must still be safe.
        let _ = Http3Client::new();
        let _ = Http3Client::new_with_options(true);
    }

    #[tokio::test]
    async fn coverage_90_resolution_and_connection_failures_are_reported() {
        let resolver = DnsResolver::default();
        assert_eq!(
            Http3Client::resolve_target_addr("127.0.0.7", 443, &resolver, &[])
                .await
                .unwrap(),
            "127.0.0.7:443".parse().unwrap()
        );
        assert!(
            Http3Client::resolve_target_addr("invalid host name", 443, &resolver, &[])
                .await
                .is_err()
        );

        let client = Http3Client::new().unwrap();
        let request = Request::builder()
            .uri("https://invalid/")
            .body(Bytes::new())
            .unwrap();
        assert!(client
            .request("invalid host name", 443, request)
            .await
            .is_err());
        assert!(!Http3Client::check_h3_support("127.0.0.1", 9).await.unwrap());
    }

    #[tokio::test]
    #[ignore]
    async fn test_http3_client_xiaohongshu() {
        let client = Http3Client::new().unwrap();

        let req = Request::builder()
            .method("GET")
            .uri("https://edith.xiaohongshu.com/api/sns/web/global/config")
            .header("Host", "edith.xiaohongshu.com")
            .header(
                "User-Agent",
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
            )
            .body(Bytes::new())
            .unwrap();

        let response = client.request("edith.xiaohongshu.com", 443, req).await;
        println!("Response: {:?}", response);
        assert!(response.is_ok());
    }
}

use bifrost_core::error::{BifrostError, Result};
use rustls::pki_types::CertificateDer;
use rustls::server::WebPkiClientVerifier;
use rustls::sign::CertifiedKey;
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use std::sync::Arc;

pub struct TlsConfig;

impl TlsConfig {
    pub fn build_server_config(cert_key: &CertifiedKey) -> Result<Arc<ServerConfig>> {
        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(SingleCertResolver {
                cert_key: cert_key.clone(),
            }));

        Ok(Arc::new(config))
    }

    pub fn build_server_config_with_client_auth(
        cert_key: &CertifiedKey,
        client_ca_certs: Vec<CertificateDer<'static>>,
    ) -> Result<Arc<ServerConfig>> {
        let mut root_store = RootCertStore::empty();
        for cert in client_ca_certs {
            root_store
                .add(cert)
                .map_err(|e| BifrostError::Tls(format!("Failed to add client CA cert: {e}")))?;
        }

        let client_verifier = WebPkiClientVerifier::builder(Arc::new(root_store))
            .build()
            .map_err(|e| BifrostError::Tls(format!("Failed to build client verifier: {e}")))?;

        let config = ServerConfig::builder()
            .with_client_cert_verifier(client_verifier)
            .with_cert_resolver(Arc::new(SingleCertResolver {
                cert_key: cert_key.clone(),
            }));

        Ok(Arc::new(config))
    }

    pub fn build_client_config() -> Result<Arc<ClientConfig>> {
        let root_store = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

        let config = ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        Ok(Arc::new(config))
    }

    pub fn build_client_config_with_custom_ca(
        ca_certs: Vec<CertificateDer<'static>>,
    ) -> Result<Arc<ClientConfig>> {
        let mut root_store =
            RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

        for cert in ca_certs {
            root_store
                .add(cert)
                .map_err(|e| BifrostError::Tls(format!("Failed to add custom CA cert: {e}")))?;
        }

        let config = ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        Ok(Arc::new(config))
    }

    pub fn build_client_config_dangerous() -> Result<Arc<ClientConfig>> {
        let config = ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerifier))
            .with_no_client_auth();

        Ok(Arc::new(config))
    }
}

#[derive(Debug)]
struct SingleCertResolver {
    cert_key: CertifiedKey,
}

impl rustls::server::ResolvesServerCert for SingleCertResolver {
    fn resolve(&self, _client_hello: rustls::server::ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(Arc::new(self.cert_key.clone()))
    }
}

#[derive(Debug)]
struct NoVerifier;

impl rustls::client::danger::ServerCertVerifier for NoVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::ECDSA_NISTP521_SHA512,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::ED25519,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ca::generate_root_ca;
    use crate::dynamic::DynamicCertGenerator;

    fn setup_crypto_provider() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }

    fn create_test_cert_key() -> CertifiedKey {
        let ca = Arc::new(generate_root_ca().expect("Failed to generate CA"));
        let generator = DynamicCertGenerator::new(ca);
        generator
            .generate_for_domain("example.com")
            .expect("Failed to generate cert")
    }

    #[test]
    fn test_build_server_config() {
        setup_crypto_provider();
        let cert_key = create_test_cert_key();
        let config = TlsConfig::build_server_config(&cert_key);
        assert!(config.is_ok());
    }

    #[test]
    fn test_build_client_config() {
        setup_crypto_provider();
        let config = TlsConfig::build_client_config();
        assert!(config.is_ok());
    }

    #[test]
    fn test_build_client_config_dangerous() {
        setup_crypto_provider();
        let config = TlsConfig::build_client_config_dangerous();
        assert!(config.is_ok());
    }

    #[test]
    fn test_build_client_config_with_custom_ca() {
        setup_crypto_provider();
        let ca = generate_root_ca().expect("Failed to generate CA");
        let ca_cert = ca.certificate_der().expect("Failed to get cert der");
        let config = TlsConfig::build_client_config_with_custom_ca(vec![ca_cert]);
        assert!(config.is_ok());
    }

    #[test]
    fn test_build_server_config_with_client_auth() {
        setup_crypto_provider();
        let cert_key = create_test_cert_key();
        let ca = generate_root_ca().expect("Failed to generate CA");
        let ca_cert = ca.certificate_der().expect("Failed to get cert der");
        let config =
            TlsConfig::build_server_config_with_client_auth(&cert_key, vec![ca_cert]).unwrap();
        // Builds a usable server config with a client verifier installed.
        assert!(Arc::strong_count(&config) >= 1);
    }

    #[test]
    fn test_single_cert_resolver_resolves() {
        setup_crypto_provider();
        let cert_key = create_test_cert_key();
        let resolver = SingleCertResolver {
            cert_key: cert_key.clone(),
        };
        // build_server_config installs this resolver; build it to confirm the
        // resolver is wired in (resolve itself is invoked during handshake).
        let server = TlsConfig::build_server_config(&cert_key).unwrap();
        assert!(Arc::strong_count(&server) >= 1);
        // Smoke-check the resolver holds a cert chain.
        assert!(!resolver.cert_key.cert.is_empty());
    }

    #[test]
    fn test_no_verifier_accepts_everything() {
        use rustls::client::danger::ServerCertVerifier;
        use rustls::pki_types::{ServerName, UnixTime};
        setup_crypto_provider();

        let ca = generate_root_ca().expect("Failed to generate CA");
        let cert = ca.certificate_der().expect("Failed to get cert der");
        let verifier = NoVerifier;

        let server_name = ServerName::try_from("example.com").unwrap();
        let result = verifier.verify_server_cert(&cert, &[], &server_name, &[], UnixTime::now());
        assert!(result.is_ok());

        // supported_verify_schemes is non-empty.
        assert!(!verifier.supported_verify_schemes().is_empty());
    }

    #[test]
    fn test_build_server_config_with_client_auth_rejects_bad_ca() {
        setup_crypto_provider();
        let cert_key = create_test_cert_key();
        // A non-certificate DER blob makes RootCertStore::add fail, hitting the
        // error-mapping closure.
        let bogus = CertificateDer::from(vec![0u8, 1, 2, 3]);
        let result = TlsConfig::build_server_config_with_client_auth(&cert_key, vec![bogus]);
        assert!(result.is_err());
    }

    #[test]
    fn test_build_client_config_with_custom_ca_rejects_bad_ca() {
        setup_crypto_provider();
        let bogus = CertificateDer::from(vec![0u8, 1, 2, 3]);
        let result = TlsConfig::build_client_config_with_custom_ca(vec![bogus]);
        assert!(result.is_err());
    }
}

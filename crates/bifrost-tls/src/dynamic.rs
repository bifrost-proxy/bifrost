use crate::ca::CertificateAuthority;
use bifrost_core::error::{BifrostError, Result};
use rcgen::{
    CertificateParams, DnType, ExtendedKeyUsagePurpose, Issuer, KeyPair, KeyUsagePurpose, SanType,
    PKCS_ECDSA_P256_SHA256,
};
use rustls::crypto::ring::sign::any_supported_type;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::sign::CertifiedKey;
use std::sync::Arc;
use time::{Duration, OffsetDateTime};

#[derive(Debug)]
pub struct DynamicCertGenerator {
    ca: Arc<CertificateAuthority>,
    // 预解析并缓存 CA Issuer，避免每次签发 leaf 都重复解析 CA 证书。
    // 如果初始化失败，会自动回退到“每次签发时临时构建 Issuer”的旧逻辑。
    ca_issuer: Option<Issuer<'static, KeyPair>>,
    // 复用同一把 leaf key，避免每次域名生成都做一次昂贵的 keypair 生成。
    // 如果初始化失败，会自动回退到“每次生成新 keypair”的旧逻辑。
    leaf_keypair_pkcs8_der: Option<Vec<u8>>,
    leaf_signing_key: Option<Arc<dyn rustls::sign::SigningKey>>,
}

impl DynamicCertGenerator {
    pub fn new(ca: Arc<CertificateAuthority>) -> Self {
        let ca_issuer = {
            // rcgen 的 Issuer 需要可拥有的 SigningKey；这里通过序列化再反序列化的方式获得。
            // 失败时回退到旧逻辑（每次签发 leaf 时临时构建 Issuer）。
            let ca_key_der = ca.key_pair.serialize_der();
            let ca_key_pair = KeyPair::try_from(ca_key_der.as_slice()).ok();
            let ca_cert_der = ca.certificate_der().ok();

            ca_key_pair.and_then(|kp| {
                ca_cert_der.and_then(|cert_der| Issuer::from_ca_cert_der(&cert_der, kp).ok())
            })
        };

        let (leaf_keypair_pkcs8_der, leaf_signing_key) =
            match KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256) {
                Ok(key_pair) => {
                    let pkcs8_der = key_pair.serialize_der();
                    let key_der: PrivateKeyDer<'static> =
                        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(pkcs8_der.clone()));
                    match any_supported_type(&key_der) {
                        Ok(signing_key) => (Some(pkcs8_der), Some(signing_key)),
                        Err(_) => (None, None),
                    }
                }
                Err(_) => (None, None),
            };

        Self {
            ca,
            ca_issuer,
            leaf_keypair_pkcs8_der,
            leaf_signing_key,
        }
    }

    pub fn generate_for_domain(&self, domain: &str) -> Result<CertifiedKey> {
        let mut params = CertificateParams::default();
        params.distinguished_name.push(DnType::CommonName, domain);
        params
            .distinguished_name
            .push(DnType::OrganizationName, "Bifrost Proxy");

        if domain.parse::<std::net::IpAddr>().is_ok() {
            params.subject_alt_names =
                vec![SanType::IpAddress(domain.parse().map_err(|e| {
                    BifrostError::Tls(format!("Invalid IP address: {e}"))
                })?)];
        } else {
            params.subject_alt_names =
                vec![SanType::DnsName(domain.to_string().try_into().map_err(
                    |e| BifrostError::Tls(format!("Invalid DNS name: {e}")),
                )?)];
        }

        params.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyEncipherment,
        ];
        params.extended_key_usages = vec![
            ExtendedKeyUsagePurpose::ServerAuth,
            ExtendedKeyUsagePurpose::ClientAuth,
        ];
        params.not_before = OffsetDateTime::now_utc() - Duration::days(1);
        params.not_after = OffsetDateTime::now_utc() + Duration::days(90);

        let (key_pair, signing_key) = if let (Some(pkcs8_der), Some(signing_key)) =
            (&self.leaf_keypair_pkcs8_der, &self.leaf_signing_key)
        {
            let key_pair = KeyPair::from_pkcs8_der_and_sign_algo(
                &PrivatePkcs8KeyDer::from(pkcs8_der.as_slice()),
                &PKCS_ECDSA_P256_SHA256,
            )
            .map_err(|e| BifrostError::Tls(format!("Failed to load reusable key pair: {e}")))?;
            (key_pair, signing_key.clone())
        } else {
            let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
                .map_err(|e| BifrostError::Tls(format!("Failed to generate key pair: {e}")))?;
            let key_der: PrivateKeyDer<'static> =
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der()));
            let signing_key = any_supported_type(&key_der)
                .map_err(|e| BifrostError::Tls(format!("Failed to create signing key: {e}")))?;
            (key_pair, signing_key)
        };

        let cert = if let Some(ref issuer) = self.ca_issuer {
            params
                .signed_by(&key_pair, issuer)
                .map_err(|e| BifrostError::Tls(format!("Failed to sign certificate: {e}")))?
        } else {
            // 回退路径：每次都用 DER 临时构建 Issuer（兼容旧行为）。
            // 注意：rcgen 的 Issuer 需要可拥有的 SigningKey，因此这里同样通过序列化再反序列化获得。
            let ca_key_der = self.ca.key_pair.serialize_der();
            let ca_key_pair = KeyPair::try_from(ca_key_der.as_slice())
                .map_err(|e| BifrostError::Tls(format!("Failed to load CA key pair: {e}")))?;
            let ca_cert_der = self.ca.certificate_der()?;
            let issuer = Issuer::from_ca_cert_der(&ca_cert_der, ca_key_pair)
                .map_err(|e| BifrostError::Tls(format!("Failed to create issuer: {e}")))?;
            params
                .signed_by(&key_pair, &issuer)
                .map_err(|e| BifrostError::Tls(format!("Failed to sign certificate: {e}")))?
        };

        let cert_der = CertificateDer::from(cert.der().to_vec());
        let ca_cert_der = self.ca.certificate_der()?;

        let cert_chain = vec![cert_der, ca_cert_der];

        Ok(CertifiedKey::new(cert_chain, signing_key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ca::{generate_root_ca, load_root_ca};
    use crate::init_crypto_provider;
    use std::fs;
    use tempfile::tempdir;
    use x509_parser::parse_x509_certificate;

    #[test]
    fn test_generate_for_domain() {
        init_crypto_provider();
        let ca = Arc::new(generate_root_ca().expect("Failed to generate CA"));
        let generator = DynamicCertGenerator::new(ca);

        let cert_key = generator
            .generate_for_domain("example.com")
            .expect("Failed to generate certificate");
        assert_eq!(cert_key.cert.len(), 2);
    }

    #[test]
    fn test_generate_for_wildcard_domain() {
        init_crypto_provider();
        let ca = Arc::new(generate_root_ca().expect("Failed to generate CA"));
        let generator = DynamicCertGenerator::new(ca);

        let cert_key = generator
            .generate_for_domain("*.example.com")
            .expect("Failed to generate wildcard certificate");
        assert_eq!(cert_key.cert.len(), 2);
    }

    #[test]
    fn test_generate_for_ip() {
        init_crypto_provider();
        let ca = Arc::new(generate_root_ca().expect("Failed to generate CA"));
        let generator = DynamicCertGenerator::new(ca);

        let cert_key = generator
            .generate_for_domain("127.0.0.1")
            .expect("Failed to generate certificate for IP");
        assert_eq!(cert_key.cert.len(), 2);
    }

    #[test]
    fn test_generate_for_subdomain() {
        init_crypto_provider();
        let ca = Arc::new(generate_root_ca().expect("Failed to generate CA"));
        let generator = DynamicCertGenerator::new(ca);

        let cert_key = generator
            .generate_for_domain("api.sub.example.com")
            .expect("Failed to generate subdomain certificate");
        assert_eq!(cert_key.cert.len(), 2);
    }

    #[test]
    fn test_generate_for_domain_has_browser_safe_validity_period() {
        init_crypto_provider();
        let ca = Arc::new(generate_root_ca().expect("Failed to generate CA"));
        let generator = DynamicCertGenerator::new(ca);

        let cert_key = generator
            .generate_for_domain("example.com")
            .expect("Failed to generate certificate");
        let cert = parse_x509_certificate(cert_key.cert[0].as_ref())
            .expect("Failed to parse leaf certificate")
            .1;
        let validity_days = (cert.validity().not_after.timestamp()
            - cert.validity().not_before.timestamp())
            / 86_400;

        assert!(
            validity_days >= 80,
            "leaf validity should cover normal proxy uptime"
        );
        assert!(
            validity_days <= 100,
            "leaf validity should stay browser-safe"
        );
    }

    #[test]
    fn test_generate_for_domain_invalid_dns_name_errors() {
        init_crypto_provider();
        let ca = Arc::new(generate_root_ca().expect("Failed to generate CA"));
        let generator = DynamicCertGenerator::new(ca);

        // A name with non-ASCII bytes is not a valid IA5String DNS SAN and is
        // not parseable as an IP, so the DnsName conversion path returns a Tls
        // error.
        let err = generator
            .generate_for_domain("exämple.com")
            .expect_err("invalid DNS name should error");
        assert!(matches!(err, BifrostError::Tls(_)));
    }

    #[test]
    fn test_generate_for_domain_uses_issuer_fallback_path() {
        init_crypto_provider();
        let ca = Arc::new(generate_root_ca().expect("Failed to generate CA"));
        // Construct a generator manually with the cached issuer and reusable
        // leaf key disabled so generate_for_domain exercises BOTH fallback
        // branches (fresh keypair + per-call Issuer construction).
        let generator = DynamicCertGenerator {
            ca,
            ca_issuer: None,
            leaf_keypair_pkcs8_der: None,
            leaf_signing_key: None,
        };

        let cert_key = generator
            .generate_for_domain("fallback.example.com")
            .expect("fallback path should still produce a cert");
        assert_eq!(cert_key.cert.len(), 2);
    }

    #[test]
    fn test_generate_for_domain_with_rsa_ca() {
        init_crypto_provider();
        let dir = tempdir().expect("Failed to create temp dir");
        let cert_path = dir.path().join("ca.crt");
        let key_path = dir.path().join("ca.key");

        fs::write(&cert_path, include_str!("../testdata/test-rsa-ca.crt"))
            .expect("Failed to write RSA cert");
        fs::write(&key_path, include_str!("../testdata/test-rsa-ca-pkcs8.key"))
            .expect("Failed to write RSA key");

        let ca = Arc::new(load_root_ca(&cert_path, &key_path).expect("Failed to load RSA CA"));
        let generator = DynamicCertGenerator::new(ca);

        let cert_key = generator
            .generate_for_domain("example.com")
            .expect("Failed to generate certificate with RSA CA");

        assert_eq!(cert_key.cert.len(), 2);

        let leaf = parse_x509_certificate(cert_key.cert[0].as_ref())
            .expect("Failed to parse leaf certificate")
            .1;
        let issuer = parse_x509_certificate(cert_key.cert[1].as_ref())
            .expect("Failed to parse issuer certificate")
            .1;

        leaf.verify_signature(Some(issuer.public_key()))
            .expect("leaf certificate should verify against RSA issuer");
    }
}

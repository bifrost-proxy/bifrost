use std::fs;
use std::path::Path;

use base64::prelude::*;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum MobileConfigError {
    #[error("failed to read certificate: {0}")]
    Read(#[from] std::io::Error),
    #[error("certificate PEM is missing a CERTIFICATE block")]
    MissingPemBlock,
    #[error("failed to decode certificate PEM: {0}")]
    Decode(#[from] base64::DecodeError),
}

#[derive(Debug, Clone)]
pub struct MobileConfigOptions {
    pub organization: String,
    pub display_name: String,
    pub identifier: String,
}

impl Default for MobileConfigOptions {
    fn default() -> Self {
        Self {
            organization: "Bifrost Proxy".to_string(),
            display_name: "Bifrost CA".to_string(),
            identifier: "dev.bifrost.mobile-ca".to_string(),
        }
    }
}

pub fn read_certificate_der_from_file(path: &Path) -> Result<Vec<u8>, MobileConfigError> {
    let data = fs::read(path)?;
    if data.starts_with(b"-----BEGIN CERTIFICATE-----") {
        return decode_pem_certificate(&String::from_utf8_lossy(&data));
    }
    Ok(data)
}

fn decode_pem_certificate(pem: &str) -> Result<Vec<u8>, MobileConfigError> {
    let begin = "-----BEGIN CERTIFICATE-----";
    let end = "-----END CERTIFICATE-----";
    let Some(start) = pem.find(begin) else {
        return Err(MobileConfigError::MissingPemBlock);
    };
    let Some(stop) = pem[start..].find(end) else {
        return Err(MobileConfigError::MissingPemBlock);
    };
    let block = &pem[start + begin.len()..start + stop];
    let normalized = block
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<String>();
    Ok(BASE64_STANDARD.decode(normalized)?)
}

pub fn generate_ios_mobileconfig(cert_der: &[u8], options: &MobileConfigOptions) -> String {
    let profile_uuid = Uuid::new_v4();
    let cert_uuid = Uuid::new_v4();
    let payload_identifier = xml_escape(&options.identifier);
    let cert_identifier = xml_escape(&format!("{}.root", options.identifier));
    let display_name = xml_escape(&options.display_name);
    let organization = xml_escape(&options.organization);
    let cert_base64 = wrap_base64(&BASE64_STANDARD.encode(cert_der));

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>PayloadContent</key>
  <array>
    <dict>
      <key>PayloadCertificateFileName</key>
      <string>bifrost-ca.crt</string>
      <key>PayloadContent</key>
      <data>
{cert_base64}
      </data>
      <key>PayloadDescription</key>
      <string>Installs the Bifrost root CA certificate. Manually installed profiles still require enabling full trust in iOS Certificate Trust Settings.</string>
      <key>PayloadDisplayName</key>
      <string>{display_name}</string>
      <key>PayloadIdentifier</key>
      <string>{cert_identifier}</string>
      <key>PayloadType</key>
      <string>com.apple.security.root</string>
      <key>PayloadUUID</key>
      <string>{cert_uuid}</string>
      <key>PayloadVersion</key>
      <integer>1</integer>
    </dict>
  </array>
  <key>PayloadDescription</key>
  <string>Bifrost CA profile for HTTPS inspection. After installing on a personal iPhone or iPad, open Settings &gt; General &gt; About &gt; Certificate Trust Settings and enable full trust for Bifrost CA.</string>
  <key>PayloadDisplayName</key>
  <string>{display_name}</string>
  <key>PayloadIdentifier</key>
  <string>{payload_identifier}</string>
  <key>PayloadOrganization</key>
  <string>{organization}</string>
  <key>PayloadRemovalDisallowed</key>
  <false/>
  <key>PayloadType</key>
  <string>Configuration</string>
  <key>PayloadUUID</key>
  <string>{profile_uuid}</string>
  <key>PayloadVersion</key>
  <integer>1</integer>
</dict>
</plist>
"#
    )
}

fn wrap_base64(value: &str) -> String {
    value
        .as_bytes()
        .chunks(52)
        .map(|chunk| format!("        {}", String::from_utf8_lossy(chunk)))
        .collect::<Vec<_>>()
        .join("\n")
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mobileconfig_contains_root_payload_and_manual_trust_notice() {
        let profile = generate_ios_mobileconfig(b"test-cert", &MobileConfigOptions::default());

        assert!(profile.contains("<string>com.apple.security.root</string>"));
        assert!(profile.contains("Certificate Trust Settings"));
        assert!(profile.contains("<data>"));
    }

    #[test]
    fn decodes_pem_certificate_block() {
        let pem = "-----BEGIN CERTIFICATE-----\nAQIDBA==\n-----END CERTIFICATE-----\n";

        let der = decode_pem_certificate(pem).expect("pem should decode");

        assert_eq!(der, vec![1, 2, 3, 4]);
    }
}

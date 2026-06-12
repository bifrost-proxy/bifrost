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

#[derive(Debug, Clone)]
pub struct IosWifiProxyProfileOptions {
    pub organization: String,
    pub display_name: String,
    pub identifier: String,
    pub ssid: String,
    pub proxy_host: String,
    pub proxy_port: u16,
}

impl IosWifiProxyProfileOptions {
    pub fn new(ssid: String, proxy_host: String, proxy_port: u16) -> Self {
        Self {
            organization: "Bifrost Proxy".to_string(),
            display_name: "Bifrost Wi-Fi Proxy".to_string(),
            identifier: "dev.bifrost.mobile-ios-wifi-proxy".to_string(),
            ssid,
            proxy_host,
            proxy_port,
        }
    }
}

pub fn read_certificate_der_from_file(path: &Path) -> Result<Vec<u8>, MobileConfigError> {
    let data = fs::read(path)?;
    read_certificate_der_from_bytes(&data)
}

pub fn read_certificate_der_from_bytes(data: &[u8]) -> Result<Vec<u8>, MobileConfigError> {
    if data.starts_with(b"-----BEGIN CERTIFICATE-----") {
        return decode_pem_certificate(&String::from_utf8_lossy(data));
    }
    Ok(data.to_vec())
}

pub(crate) fn decode_pem_certificate(pem: &str) -> Result<Vec<u8>, MobileConfigError> {
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

pub fn generate_ios_wifi_proxy_mobileconfig(
    cert_der: &[u8],
    options: &IosWifiProxyProfileOptions,
) -> String {
    let profile_uuid = Uuid::new_v4();
    let cert_uuid = Uuid::new_v4();
    let wifi_uuid = Uuid::new_v4();
    let payload_identifier = xml_escape(&options.identifier);
    let cert_identifier = xml_escape(&format!("{}.root", options.identifier));
    let wifi_identifier = xml_escape(&format!("{}.wifi", options.identifier));
    let display_name = xml_escape(&options.display_name);
    let organization = xml_escape(&options.organization);
    let ssid = xml_escape(&options.ssid);
    let proxy_host = xml_escape(&options.proxy_host);
    let proxy_port = options.proxy_port;
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
      <string>Bifrost CA</string>
      <key>PayloadIdentifier</key>
      <string>{cert_identifier}</string>
      <key>PayloadType</key>
      <string>com.apple.security.root</string>
      <key>PayloadUUID</key>
      <string>{cert_uuid}</string>
      <key>PayloadVersion</key>
      <integer>1</integer>
    </dict>
    <dict>
      <key>AutoJoin</key>
      <true/>
      <key>EncryptionType</key>
      <string>Any</string>
      <key>PayloadDescription</key>
      <string>Configures manual HTTP proxy for the managed Wi-Fi network named {ssid}. This payload does not contain a Wi-Fi password or join credentials. Removing this profile may remove the managed Wi-Fi network entry from iOS.</string>
      <key>PayloadDisplayName</key>
      <string>Bifrost Wi-Fi Proxy</string>
      <key>PayloadIdentifier</key>
      <string>{wifi_identifier}</string>
      <key>PayloadType</key>
      <string>com.apple.wifi.managed</string>
      <key>PayloadUUID</key>
      <string>{wifi_uuid}</string>
      <key>PayloadVersion</key>
      <integer>1</integer>
      <key>ProxyServer</key>
      <string>{proxy_host}</string>
      <key>ProxyServerPort</key>
      <integer>{proxy_port}</integer>
      <key>ProxyType</key>
      <string>Manual</string>
      <key>SSID_STR</key>
      <string>{ssid}</string>
    </dict>
  </array>
  <key>PayloadDescription</key>
  <string>Installs Bifrost CA and configures the managed Wi-Fi network {ssid} to use Bifrost proxy {proxy_host}:{proxy_port}. This profile does not contain a Wi-Fi password or join credentials. Removing this profile may remove the managed Wi-Fi network entry from iOS. Personal iPhone and iPad devices still require profile installation confirmation and may require enabling full trust for Bifrost CA.</string>
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
    fn wifi_proxy_mobileconfig_contains_wifi_proxy_and_root_payloads() {
        let profile = generate_ios_wifi_proxy_mobileconfig(
            b"test-cert",
            &IosWifiProxyProfileOptions::new(
                "Office & Lab".to_string(),
                "192.168.8.34".to_string(),
                9900,
            ),
        );

        assert!(profile.contains("<string>com.apple.security.root</string>"));
        assert!(profile.contains("<string>com.apple.wifi.managed</string>"));
        assert!(profile.contains("<key>SSID_STR</key>"));
        assert!(profile.contains("<string>Office &amp; Lab</string>"));
        assert!(profile.contains("<key>ProxyType</key>"));
        assert!(profile.contains("<string>Manual</string>"));
        assert!(profile.contains("<key>ProxyServer</key>"));
        assert!(profile.contains("<string>192.168.8.34</string>"));
        assert!(profile.contains("<key>ProxyServerPort</key>"));
        assert!(profile.contains("<integer>9900</integer>"));
        assert!(profile.contains("does not contain a Wi-Fi password"));
        assert!(profile.contains("Removing this profile may remove the managed Wi-Fi network"));
        assert!(!profile.contains("<key>Password</key>"));
        assert!(!profile.contains("<key>Passphrase</key>"));
    }

    #[test]
    fn decodes_pem_certificate_block() {
        let pem = "-----BEGIN CERTIFICATE-----\nAQIDBA==\n-----END CERTIFICATE-----\n";

        let der = decode_pem_certificate(pem).expect("pem should decode");

        assert_eq!(der, vec![1, 2, 3, 4]);
    }
}

#[cfg(test)]
mod more_tests {
    use super::*;
    use std::fs;

    #[test]
    fn read_certificate_der_from_bytes_returns_raw_der_when_not_pem() {
        let data = vec![1u8, 2, 3, 4];
        let result = read_certificate_der_from_bytes(&data).expect("der should succeed");
        assert_eq!(result, data);
    }

    #[test]
    fn read_certificate_der_from_file_decodes_pem() {
        let temp_dir = std::env::temp_dir();
        let path = temp_dir.join("bifrost-mobileconfig-test.pem");
        fs::write(
            &path,
            "-----BEGIN CERTIFICATE-----\nAQIDBA==\n-----END CERTIFICATE-----\n",
        )
        .expect("write pem");

        let der = read_certificate_der_from_file(&path).expect("read der from file");
        assert_eq!(der, vec![1, 2, 3, 4]);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn ios_wifi_proxy_profile_options_new_sets_default_labels() {
        let opts = IosWifiProxyProfileOptions::new(
            "Office & Lab".to_string(),
            "proxy.example.local".to_string(),
            8080,
        );

        assert_eq!(opts.organization, "Bifrost Proxy");
        assert_eq!(opts.display_name, "Bifrost Wi-Fi Proxy");
        assert_eq!(opts.identifier, "dev.bifrost.mobile-ios-wifi-proxy");
        assert_eq!(opts.ssid, "Office & Lab");
        assert_eq!(opts.proxy_host, "proxy.example.local");
        assert_eq!(opts.proxy_port, 8080);
    }

    #[test]
    fn xml_escape_escapes_special_characters() {
        let escaped = xml_escape(r#"5 < 10 & "quotes" 'apos'"#);
        assert_eq!(
            escaped,
            "5 &lt; 10 &amp; &quot;quotes&quot; &apos;apos&apos;",
        );
    }

    #[test]
    fn wrap_base64_inserts_indentation_and_newlines() {
        let long = "A".repeat(120);
        let wrapped = wrap_base64(&long);

        for line in wrapped.lines() {
            assert!(line.starts_with("        "));
            assert!(!line.trim().is_empty());
        }
        // Combined content without indentation should equal original string.
        let joined: String = wrapped
            .lines()
            .map(|line| line.trim_start())
            .collect::<String>();
        assert_eq!(joined, long);
    }
}

use std::str::FromStr;

#[derive(Debug, Clone, PartialEq)]
pub enum ConfigKey {
    ServerTimeoutSecs,
    ServerHttp1MaxHeaderSize,
    ServerHttp2MaxHeaderListSize,
    ServerWebSocketHandshakeMaxHeaderSize,

    TlsEnabled,
    TlsUnsafeSsl,
    TlsDisconnectOnChange,
    TlsExclude,
    TlsInclude,
    TlsAppExclude,
    TlsAppInclude,
    TlsIpExclude,
    TlsIpInclude,

    TrafficMaxRecords,
    TrafficMaxDbSize,
    TrafficMaxBodySize,
    TrafficMaxBufferSize,
    TrafficRetentionDays,
    TrafficSseStreamFlushBytes,
    TrafficSseStreamFlushIntervalMs,
    TrafficWsPayloadFlushBytes,
    TrafficWsPayloadFlushIntervalMs,
    TrafficWsPayloadMaxOpenFiles,

    AccessMode,
    AccessAllowLan,
    AccessUserPassEnabled,
    AccessUserPassAccounts,
    AccessUserPassLoopbackRequiresAuth,
}

impl ConfigKey {
    pub fn is_list(&self) -> bool {
        matches!(
            self,
            Self::TlsExclude
                | Self::TlsInclude
                | Self::TlsAppExclude
                | Self::TlsAppInclude
                | Self::TlsIpExclude
                | Self::TlsIpInclude
                | Self::AccessUserPassAccounts
        )
    }

    pub fn is_size(&self) -> bool {
        matches!(
            self,
            Self::TrafficMaxBodySize
                | Self::TrafficMaxBufferSize
                | Self::TrafficMaxDbSize
                | Self::ServerHttp1MaxHeaderSize
                | Self::ServerHttp2MaxHeaderListSize
                | Self::ServerWebSocketHandshakeMaxHeaderSize
                | Self::TrafficSseStreamFlushBytes
                | Self::TrafficWsPayloadFlushBytes
        )
    }

    pub fn all_keys() -> Vec<&'static str> {
        vec![
            "tls.enabled",
            "server.timeout-secs",
            "server.http1-max-header-size",
            "server.http2-max-header-list-size",
            "server.websocket-handshake-max-header-size",
            "tls.unsafe-ssl",
            "tls.disconnect-on-change",
            "tls.exclude",
            "tls.include",
            "tls.app-exclude",
            "tls.app-include",
            "tls.ip-exclude",
            "tls.ip-include",
            "traffic.max-records",
            "traffic.max-db-size",
            "traffic.max-body-size",
            "traffic.max-buffer-size",
            "traffic.retention-days",
            "traffic.sse-stream-flush-bytes",
            "traffic.sse-stream-flush-interval-ms",
            "traffic.ws-payload-flush-bytes",
            "traffic.ws-payload-flush-interval-ms",
            "traffic.ws-payload-max-open-files",
            "access.mode",
            "access.allow-lan",
            "access.userpass.enabled",
            "access.userpass.accounts",
            "access.userpass.loopback-requires-auth",
        ]
    }
}

impl FromStr for ConfigKey {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().replace('_', "-").as_str() {
            "server.timeout-secs" => Ok(Self::ServerTimeoutSecs),
            "server.http1-max-header-size" => Ok(Self::ServerHttp1MaxHeaderSize),
            "server.http2-max-header-list-size" => Ok(Self::ServerHttp2MaxHeaderListSize),
            "server.websocket-handshake-max-header-size" => {
                Ok(Self::ServerWebSocketHandshakeMaxHeaderSize)
            }
            "tls.enabled" => Ok(Self::TlsEnabled),
            "tls.unsafe-ssl" => Ok(Self::TlsUnsafeSsl),
            "tls.disconnect-on-change" => Ok(Self::TlsDisconnectOnChange),
            "tls.exclude" => Ok(Self::TlsExclude),
            "tls.include" => Ok(Self::TlsInclude),
            "tls.app-exclude" => Ok(Self::TlsAppExclude),
            "tls.app-include" => Ok(Self::TlsAppInclude),
            "tls.ip-exclude" => Ok(Self::TlsIpExclude),
            "tls.ip-include" => Ok(Self::TlsIpInclude),
            "traffic.max-records" => Ok(Self::TrafficMaxRecords),
            "traffic.max-db-size" => Ok(Self::TrafficMaxDbSize),
            "traffic.max-body-size" => Ok(Self::TrafficMaxBodySize),
            "traffic.max-buffer-size" => Ok(Self::TrafficMaxBufferSize),
            "traffic.retention-days" => Ok(Self::TrafficRetentionDays),
            "traffic.sse-stream-flush-bytes" => Ok(Self::TrafficSseStreamFlushBytes),
            "traffic.sse-stream-flush-interval-ms" => Ok(Self::TrafficSseStreamFlushIntervalMs),
            "traffic.ws-payload-flush-bytes" => Ok(Self::TrafficWsPayloadFlushBytes),
            "traffic.ws-payload-flush-interval-ms" => Ok(Self::TrafficWsPayloadFlushIntervalMs),
            "traffic.ws-payload-max-open-files" => Ok(Self::TrafficWsPayloadMaxOpenFiles),
            "access.mode" => Ok(Self::AccessMode),
            "access.allow-lan" => Ok(Self::AccessAllowLan),
            "access.userpass.enabled" => Ok(Self::AccessUserPassEnabled),
            "access.userpass.accounts" => Ok(Self::AccessUserPassAccounts),
            "access.userpass.loopback-requires-auth" => {
                Ok(Self::AccessUserPassLoopbackRequiresAuth)
            }
            _ => Err(format!(
                "Unknown config key: '{}'\n\nAvailable keys:\n{}",
                s,
                ConfigKey::all_keys()
                    .iter()
                    .map(|k| format!("  - {}", k))
                    .collect::<Vec<_>>()
                    .join("\n")
            )),
        }
    }
}

impl std::fmt::Display for ConfigKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::ServerTimeoutSecs => "server.timeout-secs",
            Self::ServerHttp1MaxHeaderSize => "server.http1-max-header-size",
            Self::ServerHttp2MaxHeaderListSize => "server.http2-max-header-list-size",
            Self::ServerWebSocketHandshakeMaxHeaderSize => {
                "server.websocket-handshake-max-header-size"
            }
            Self::TlsEnabled => "tls.enabled",
            Self::TlsUnsafeSsl => "tls.unsafe-ssl",
            Self::TlsDisconnectOnChange => "tls.disconnect-on-change",
            Self::TlsExclude => "tls.exclude",
            Self::TlsInclude => "tls.include",
            Self::TlsAppExclude => "tls.app-exclude",
            Self::TlsAppInclude => "tls.app-include",
            Self::TlsIpExclude => "tls.ip-exclude",
            Self::TlsIpInclude => "tls.ip-include",
            Self::TrafficMaxRecords => "traffic.max-records",
            Self::TrafficMaxDbSize => "traffic.max-db-size",
            Self::TrafficMaxBodySize => "traffic.max-body-size",
            Self::TrafficMaxBufferSize => "traffic.max-buffer-size",
            Self::TrafficRetentionDays => "traffic.retention-days",
            Self::TrafficSseStreamFlushBytes => "traffic.sse-stream-flush-bytes",
            Self::TrafficSseStreamFlushIntervalMs => "traffic.sse-stream-flush-interval-ms",
            Self::TrafficWsPayloadFlushBytes => "traffic.ws-payload-flush-bytes",
            Self::TrafficWsPayloadFlushIntervalMs => "traffic.ws-payload-flush-interval-ms",
            Self::TrafficWsPayloadMaxOpenFiles => "traffic.ws-payload-max-open-files",
            Self::AccessMode => "access.mode",
            Self::AccessAllowLan => "access.allow-lan",
            Self::AccessUserPassEnabled => "access.userpass.enabled",
            Self::AccessUserPassAccounts => "access.userpass.accounts",
            Self::AccessUserPassLoopbackRequiresAuth => "access.userpass.loopback-requires-auth",
        };
        write!(f, "{}", s)
    }
}

pub fn parse_bool(s: &str) -> Result<bool, String> {
    match s.to_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Ok(true),
        "false" | "no" | "off" | "0" => Ok(false),
        _ => Err(format!(
            "Invalid boolean value: '{}'. Use true/false/yes/no/on/off/1/0",
            s
        )),
    }
}

pub fn parse_list(s: &str) -> Vec<String> {
    s.split(',')
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect()
}

pub fn parse_size(s: &str) -> Result<usize, String> {
    let s = s.to_uppercase();
    if let Some(num) = s.strip_suffix("KB") {
        return num
            .trim()
            .parse::<usize>()
            .map(|n| n * 1024)
            .map_err(|e| format!("Invalid size: {}", e));
    }
    if let Some(num) = s.strip_suffix("MB") {
        return num
            .trim()
            .parse::<usize>()
            .map(|n| n * 1024 * 1024)
            .map_err(|e| format!("Invalid size: {}", e));
    }
    if let Some(num) = s.strip_suffix("GB") {
        return num
            .trim()
            .parse::<usize>()
            .map(|n| n * 1024 * 1024 * 1024)
            .map_err(|e| format!("Invalid size: {}", e));
    }
    if let Some(num) = s.strip_suffix('B') {
        return num
            .trim()
            .parse::<usize>()
            .map_err(|e| format!("Invalid size: {}", e));
    }
    s.parse::<usize>()
        .map_err(|e| format!("Invalid size: {}", e))
}

pub fn format_size(bytes: usize) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{} GB", bytes / (1024 * 1024 * 1024))
    } else if bytes >= 1024 * 1024 {
        format!("{} MB", bytes / (1024 * 1024))
    } else if bytes >= 1024 {
        format!("{} KB", bytes / 1024)
    } else {
        format!("{} B", bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn parse_bool_accepts_true_values_case_insensitively() {
        for value in ["true", "TRUE", "yes", "YeS", "on", "ON", "1"] {
            assert!(parse_bool(value).unwrap(), "value={value}");
        }
    }

    #[test]
    fn parse_bool_accepts_false_values_case_insensitively() {
        for value in ["false", "FALSE", "no", "No", "off", "OFF", "0"] {
            assert!(!parse_bool(value).unwrap(), "value={value}");
        }
    }

    #[test]
    fn parse_bool_rejects_invalid_values() {
        let err = parse_bool("maybe").unwrap_err();
        assert!(err.contains("Invalid boolean value"));
        assert!(err.contains("true/false/yes/no/on/off/1/0"));
    }

    #[test]
    fn parse_list_splits_trims_and_ignores_empty_items() {
        let items = parse_list(" foo, bar , , baz ,,  ");
        assert_eq!(items, vec!["foo", "bar", "baz"]);

        let empty: Vec<String> = parse_list("");
        assert!(empty.is_empty());
    }

    #[test]
    fn parse_size_parses_plain_numbers_and_units() {
        // Plain number
        assert_eq!(parse_size("42").unwrap(), 42);

        // Bytes
        assert_eq!(parse_size("123B").unwrap(), 123);
        assert_eq!(parse_size(" 123B").unwrap(), 123);

        // KB/MB/GB with mixed case and spaces
        assert_eq!(parse_size("1kb").unwrap(), 1024);
        assert_eq!(parse_size("2 KB").unwrap(), 2 * 1024);
        assert_eq!(parse_size("1mb").unwrap(), 1024 * 1024);
        assert_eq!(parse_size("3 MB").unwrap(), 3 * 1024 * 1024);
        assert_eq!(parse_size("1gb").unwrap(), 1024 * 1024 * 1024);
    }

    #[test]
    fn parse_size_rejects_invalid_values() {
        let err = parse_size("not-a-number").unwrap_err();
        assert!(err.starts_with("Invalid size:"));

        let err = parse_size("10XB").unwrap_err();
        assert!(err.starts_with("Invalid size:"));
    }

    #[test]
    fn format_size_uses_expected_units() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(999), "999 B");
        assert_eq!(format_size(1024), "1 KB");
        assert_eq!(format_size(1024 * 1024), "1 MB");
        assert_eq!(format_size(2 * 1024 * 1024 * 1024), "2 GB");
    }

    #[test]
    fn config_key_all_keys_roundtrip_display_and_parse() {
        for key_str in ConfigKey::all_keys() {
            let parsed: ConfigKey = key_str.parse().unwrap();
            assert_eq!(parsed.to_string(), *key_str);
        }
    }

    #[test]
    fn config_key_from_str_is_case_and_style_insensitive() {
        assert_eq!(
            ConfigKey::from_str("server.timeout-secs").unwrap(),
            ConfigKey::ServerTimeoutSecs
        );
        // Underscores are treated as dashes
        assert_eq!(
            ConfigKey::from_str("SERVER.TIMEOUT_SECS").unwrap(),
            ConfigKey::ServerTimeoutSecs
        );
        assert_eq!(
            ConfigKey::from_str("tls.app_exclude").unwrap(),
            ConfigKey::TlsAppExclude
        );
        assert_eq!(
            ConfigKey::from_str("traffic.max_db_size").unwrap(),
            ConfigKey::TrafficMaxDbSize
        );
    }

    #[test]
    fn unknown_config_key_error_lists_available_keys() {
        let err = ConfigKey::from_str("does.not.exist").unwrap_err();
        assert!(err.contains("Unknown config key"));
        // Error message should include at least one known key to help users
        assert!(err.contains("traffic.max-records"));
    }

    #[test]
    fn is_list_matches_expected_variants() {
        use ConfigKey::*;

        let list_keys = [
            TlsExclude,
            TlsInclude,
            TlsAppExclude,
            TlsAppInclude,
            TlsIpExclude,
            TlsIpInclude,
            AccessUserPassAccounts,
        ];

        for key in list_keys {
            assert!(key.is_list(), "{key:?} should be a list key");
        }

        let non_list_keys = [
            ServerTimeoutSecs,
            TlsEnabled,
            TrafficMaxRecords,
            AccessMode,
            AccessAllowLan,
        ];

        for key in non_list_keys {
            assert!(!key.is_list(), "{key:?} should not be a list key");
        }
    }

    #[test]
    fn is_size_matches_expected_variants() {
        use ConfigKey::*;

        let size_keys = [
            TrafficMaxBodySize,
            TrafficMaxBufferSize,
            TrafficMaxDbSize,
            ServerHttp1MaxHeaderSize,
            ServerHttp2MaxHeaderListSize,
            ServerWebSocketHandshakeMaxHeaderSize,
            TrafficSseStreamFlushBytes,
            TrafficWsPayloadFlushBytes,
        ];

        for key in size_keys {
            assert!(key.is_size(), "{key:?} should be a size key");
        }

        let non_size_keys = [
            ServerTimeoutSecs,
            TrafficMaxRecords,
            TrafficRetentionDays,
            AccessMode,
        ];

        for key in non_size_keys {
            assert!(!key.is_size(), "{key:?} should not be a size key");
        }
    }
}

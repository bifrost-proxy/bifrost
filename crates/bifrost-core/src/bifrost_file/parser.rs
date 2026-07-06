use super::types::*;
use serde::de::DeserializeOwned;
use serde::Deserialize;

#[derive(Debug)]
pub enum ParseError {
    EmptyFile,
    InvalidHeader(String),
    InvalidVersion(String),
    InvalidType(String),
    MissingSeparator,
    InvalidMeta(String),
    InvalidContent(String),
    TypeMismatch {
        expected: BifrostFileType,
        actual: BifrostFileType,
    },
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::EmptyFile => write!(f, "Empty file"),
            ParseError::InvalidHeader(msg) => write!(f, "Invalid header: {}", msg),
            ParseError::InvalidVersion(msg) => write!(f, "Invalid version: {}", msg),
            ParseError::InvalidType(msg) => write!(f, "Invalid type: {}", msg),
            ParseError::MissingSeparator => write!(f, "Missing content separator '---'"),
            ParseError::InvalidMeta(msg) => write!(f, "Invalid meta: {}", msg),
            ParseError::InvalidContent(msg) => write!(f, "Invalid content: {}", msg),
            ParseError::TypeMismatch { expected, actual } => {
                write!(f, "Type mismatch: expected {}, got {}", expected, actual)
            }
        }
    }
}

impl std::error::Error for ParseError {}

pub type Result<T> = std::result::Result<T, ParseError>;

pub struct BifrostFileParser;

impl BifrostFileParser {
    pub fn parse_header(first_line: &str) -> Result<BifrostFileHeader> {
        let trimmed = first_line.trim();
        let parts: Vec<&str> = trimmed.split_whitespace().collect();

        if parts.len() != 2 {
            return Err(ParseError::InvalidHeader(format!(
                "Expected 'VV TYPE', got: '{}'",
                trimmed
            )));
        }

        let version: u8 = parts[0]
            .parse()
            .map_err(|_| ParseError::InvalidVersion(parts[0].to_string()))?;

        let file_type: BifrostFileType = parts[1].parse().map_err(ParseError::InvalidType)?;

        Ok(BifrostFileHeader { version, file_type })
    }

    pub fn parse_raw(content: &str) -> Result<BifrostFileRaw> {
        let mut lines = content.lines();

        let header_line = lines.next().ok_or(ParseError::EmptyFile)?;
        let header = Self::parse_header(header_line)?;

        lines.next();

        let remaining: String = lines.collect::<Vec<&str>>().join("\n");

        let separator = "\n---\n";
        let (meta_raw, content_raw) = if let Some(pos) = remaining.find(separator) {
            let (meta, content) = remaining.split_at(pos);
            (meta.to_string(), content[separator.len()..].to_string())
        } else if let Some(pos) = remaining.find("\n---") {
            let (meta, content) = remaining.split_at(pos);
            let content_start = if content.len() > 4 { 4 } else { content.len() };
            (meta.to_string(), content[content_start..].to_string())
        } else {
            return Err(ParseError::MissingSeparator);
        };

        Ok(BifrostFileRaw {
            header,
            meta_raw,
            content_raw,
        })
    }

    pub fn parse_rules(content: &str) -> Result<RuleFile> {
        let raw = Self::parse_raw(content)?;

        if raw.header.file_type != BifrostFileType::Rules {
            return Err(ParseError::TypeMismatch {
                expected: BifrostFileType::Rules,
                actual: raw.header.file_type,
            });
        }

        #[derive(Deserialize)]
        struct MetaWrapper {
            meta: RuleFileMeta,
            #[serde(default)]
            options: serde_json::Value,
        }

        let toml_value: toml::Value =
            toml::from_str(&raw.meta_raw).map_err(|e| ParseError::InvalidMeta(e.to_string()))?;
        let json_value = toml_to_json(toml_value);
        let parsed: MetaWrapper = serde_json::from_value(json_value)
            .map_err(|e| ParseError::InvalidMeta(e.to_string()))?;

        Ok(BifrostFile {
            header: raw.header,
            meta: parsed.meta,
            options: parsed.options,
            content: raw.content_raw,
        })
    }

    pub fn parse_network(content: &str) -> Result<BifrostFile<ExportMeta, Vec<NetworkRecord>>> {
        let raw = Self::parse_raw(content)?;

        if raw.header.file_type != BifrostFileType::Network {
            return Err(ParseError::TypeMismatch {
                expected: BifrostFileType::Network,
                actual: raw.header.file_type,
            });
        }

        let meta = Self::parse_export_meta(&raw.meta_raw)?;

        let records: Vec<NetworkRecord> = serde_json::from_str(&raw.content_raw)
            .map_err(|e| ParseError::InvalidContent(e.to_string()))?;

        Ok(BifrostFile {
            header: raw.header,
            meta,
            options: serde_json::Value::Null,
            content: records,
        })
    }

    pub fn parse_script(content: &str) -> Result<BifrostFile<ExportMeta, Vec<ScriptItem>>> {
        let raw = Self::parse_raw(content)?;

        if raw.header.file_type != BifrostFileType::Script {
            return Err(ParseError::TypeMismatch {
                expected: BifrostFileType::Script,
                actual: raw.header.file_type,
            });
        }

        let meta = Self::parse_export_meta(&raw.meta_raw)?;

        let scripts: Vec<ScriptItem> = serde_json::from_str(&raw.content_raw)
            .map_err(|e| ParseError::InvalidContent(e.to_string()))?;

        Ok(BifrostFile {
            header: raw.header,
            meta,
            options: serde_json::Value::Null,
            content: scripts,
        })
    }

    pub fn parse_values(content: &str) -> Result<BifrostFile<ExportMeta, ValuesContent>> {
        let raw = Self::parse_raw(content)?;

        if raw.header.file_type != BifrostFileType::Values {
            return Err(ParseError::TypeMismatch {
                expected: BifrostFileType::Values,
                actual: raw.header.file_type,
            });
        }

        let meta = Self::parse_export_meta(&raw.meta_raw)?;

        let values: ValuesContent = serde_json::from_str(&raw.content_raw)
            .map_err(|e| ParseError::InvalidContent(e.to_string()))?;

        Ok(BifrostFile {
            header: raw.header,
            meta,
            options: serde_json::Value::Null,
            content: values,
        })
    }

    pub fn parse_template(content: &str) -> Result<BifrostFile<ExportMeta, TemplateContent>> {
        let raw = Self::parse_raw(content)?;

        if raw.header.file_type != BifrostFileType::Template {
            return Err(ParseError::TypeMismatch {
                expected: BifrostFileType::Template,
                actual: raw.header.file_type,
            });
        }

        let meta = Self::parse_export_meta(&raw.meta_raw)?;

        let template: TemplateContent = serde_json::from_str(&raw.content_raw)
            .map_err(|e| ParseError::InvalidContent(e.to_string()))?;

        Ok(BifrostFile {
            header: raw.header,
            meta,
            options: serde_json::Value::Null,
            content: template,
        })
    }

    pub fn detect_type(content: &str) -> Result<BifrostFileType> {
        let first_line = content.lines().next().ok_or(ParseError::EmptyFile)?;
        let header = Self::parse_header(first_line)?;
        Ok(header.file_type)
    }

    fn parse_export_meta(meta_raw: &str) -> Result<ExportMeta> {
        #[derive(Deserialize)]
        struct MetaWrapper {
            meta: ExportMeta,
        }

        let toml_value: toml::Value =
            toml::from_str(meta_raw).map_err(|e| ParseError::InvalidMeta(e.to_string()))?;
        let json_value = toml_to_json(toml_value);
        let parsed: MetaWrapper = serde_json::from_value(json_value)
            .map_err(|e| ParseError::InvalidMeta(e.to_string()))?;

        Ok(parsed.meta)
    }

    pub fn parse_tolerant(content: &str) -> ParseResultWithWarnings<BifrostFileRaw> {
        let mut warnings = Vec::new();

        let (header, header_warnings) = Self::parse_header_tolerant(content);
        warnings.extend(header_warnings);

        let (meta_raw, content_raw, sep_warnings) =
            Self::split_meta_content_tolerant(content, &header);
        warnings.extend(sep_warnings);

        ParseResultWithWarnings {
            data: BifrostFileRaw {
                header,
                meta_raw,
                content_raw,
            },
            warnings,
        }
    }

    fn parse_header_tolerant(content: &str) -> (BifrostFileHeader, Vec<ParseWarning>) {
        let mut warnings = Vec::new();
        let first_line = content.lines().next().unwrap_or("");

        if let Ok(header) = Self::parse_header(first_line) {
            return (header, warnings);
        }

        let parts: Vec<&str> = first_line.split_whitespace().collect();

        let version = parts
            .first()
            .and_then(|s| s.parse::<u8>().ok())
            .unwrap_or_else(|| {
                warnings.push(ParseWarning {
                    level: WarningLevel::Warning,
                    message: "Version number missing or invalid, using default: 01".into(),
                    field: Some("version".into()),
                });
                1
            });

        let file_type = parts
            .get(1)
            .and_then(|s| s.parse::<BifrostFileType>().ok())
            .unwrap_or_else(|| {
                let inferred = Self::infer_type_from_content(content);
                warnings.push(ParseWarning {
                    level: WarningLevel::Warning,
                    message: format!("File type missing or invalid, inferred as: {}", inferred),
                    field: Some("file_type".into()),
                });
                inferred
            });

        (BifrostFileHeader { version, file_type }, warnings)
    }

    fn infer_type_from_content(content: &str) -> BifrostFileType {
        let trimmed = content.trim();

        for line in trimmed.lines().skip(2) {
            let line_trimmed = line.trim();
            if line_trimmed.is_empty() || line_trimmed.starts_with('[') {
                continue;
            }

            if line_trimmed.starts_with('[') && !line_trimmed.contains('=') {
                if line_trimmed.contains("\"script_type\"") {
                    return BifrostFileType::Script;
                }
                if line_trimmed.contains("\"request_headers\"")
                    || line_trimmed.contains("\"response_body\"")
                {
                    return BifrostFileType::Network;
                }
                if line_trimmed.contains("\"requests\"") {
                    return BifrostFileType::Template;
                }
                return BifrostFileType::Values;
            }
            break;
        }

        BifrostFileType::Rules
    }

    fn split_meta_content_tolerant(
        content: &str,
        header: &BifrostFileHeader,
    ) -> (String, String, Vec<ParseWarning>) {
        let mut warnings = Vec::new();

        let after_header: String = content.lines().skip(1).collect::<Vec<_>>().join("\n");

        if let Some(pos) = after_header.find("\n---\n") {
            let meta = after_header[..pos].trim().to_string();
            let body = after_header[pos + 5..].to_string();
            return (meta, body, warnings);
        }

        if let Some(pos) = after_header.find("\n---") {
            let meta = after_header[..pos].trim().to_string();
            let body = after_header[pos + 4..].trim().to_string();
            warnings.push(ParseWarning {
                level: WarningLevel::Info,
                message: "Separator found without trailing newline".into(),
                field: None,
            });
            return (meta, body, warnings);
        }

        warnings.push(ParseWarning {
            level: WarningLevel::Warning,
            message: "Content separator '---' not found, attempting to infer".into(),
            field: None,
        });

        let (meta, body) = match header.file_type {
            BifrostFileType::Rules => Self::split_rules_content(&after_header, &mut warnings),
            _ => Self::split_json_content(&after_header, &mut warnings),
        };
        (meta, body, warnings)
    }

    fn split_rules_content(content: &str, warnings: &mut Vec<ParseWarning>) -> (String, String) {
        let mut meta_end = 0;
        let mut in_toml = false;

        for (i, line) in content.lines().enumerate() {
            let trimmed = line.trim();

            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                in_toml = true;
                meta_end = content.lines().take(i + 1).map(|l| l.len() + 1).sum();
                continue;
            }

            if trimmed.contains('=') && !trimmed.starts_with('#') {
                in_toml = true;
                meta_end = content.lines().take(i + 1).map(|l| l.len() + 1).sum();
                continue;
            }

            if !trimmed.is_empty()
                && !trimmed.starts_with('#')
                && (trimmed.contains("://") || trimmed.contains(' '))
                && !trimmed.contains('=')
            {
                let content_start: usize = content.lines().take(i).map(|l| l.len() + 1).sum();

                return (
                    content[..meta_end].trim().to_string(),
                    content[content_start..].to_string(),
                );
            }
        }

        if !in_toml {
            warnings.push(ParseWarning {
                level: WarningLevel::Warning,
                message: "No meta section found, treating entire content as rules".into(),
                field: None,
            });
            return (String::new(), content.to_string());
        }

        (content.to_string(), String::new())
    }

    fn split_json_content(content: &str, warnings: &mut Vec<ParseWarning>) -> (String, String) {
        for (i, line) in content.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') || trimmed.starts_with('{') {
                let content_start: usize = content.lines().take(i).map(|l| l.len() + 1).sum();

                return (
                    content[..content_start].trim().to_string(),
                    content[content_start..].to_string(),
                );
            }
        }

        if content.trim().starts_with('[') || content.trim().starts_with('{') {
            warnings.push(ParseWarning {
                level: WarningLevel::Warning,
                message: "No meta section found, treating entire content as JSON".into(),
                field: None,
            });
            return (String::new(), content.to_string());
        }

        (content.to_string(), String::new())
    }

    pub fn parse_rules_tolerant(content: &str) -> ParseResultWithWarnings<RuleFile> {
        let ParseResultWithWarnings {
            data: raw,
            mut warnings,
        } = Self::parse_tolerant(content);

        let meta = Self::parse_rules_meta_tolerant(&raw.meta_raw, &mut warnings);

        ParseResultWithWarnings {
            data: BifrostFile {
                header: raw.header,
                meta,
                options: serde_json::Value::Null,
                content: raw.content_raw,
            },
            warnings,
        }
    }

    fn parse_rules_meta_tolerant(meta_raw: &str, warnings: &mut Vec<ParseWarning>) -> RuleFileMeta {
        if meta_raw.is_empty() {
            warnings.push(ParseWarning {
                level: WarningLevel::Warning,
                message: "Meta section is empty, using defaults".into(),
                field: None,
            });
            return RuleFileMeta::default();
        }

        #[derive(Deserialize, Default)]
        struct MetaWrapper {
            #[serde(default)]
            meta: PartialRuleFileMeta,
        }

        #[derive(Deserialize, Default)]
        struct PartialRuleFileMeta {
            name: Option<String>,
            enabled: Option<bool>,
            sort_order: Option<i32>,
            version: Option<String>,
            created_at: Option<String>,
            updated_at: Option<String>,
            description: Option<String>,
            group: Option<String>,
            #[serde(default)]
            sync: RuleSyncMeta,
        }

        let parsed: MetaWrapper = match toml::from_str::<toml::Value>(meta_raw) {
            Ok(toml_val) => {
                let json_val = toml_to_json(toml_val);
                serde_json::from_value(json_val).unwrap_or_default()
            }
            Err(e) => {
                warnings.push(ParseWarning {
                    level: WarningLevel::Error,
                    message: format!("Failed to parse meta as TOML: {}", e),
                    field: None,
                });
                MetaWrapper::default()
            }
        };

        let now = chrono::Utc::now().to_rfc3339();
        let partial = parsed.meta;

        if partial.name.is_none() {
            warnings.push(ParseWarning {
                level: WarningLevel::Info,
                message: "Field 'name' missing, using default".into(),
                field: Some("name".into()),
            });
        }

        RuleFileMeta {
            name: partial.name.unwrap_or_else(|| "unnamed".to_string()),
            enabled: partial.enabled.unwrap_or(true),
            sort_order: partial.sort_order.unwrap_or(0),
            version: partial.version.unwrap_or_else(|| "1.0.0".to_string()),
            created_at: partial.created_at.unwrap_or_else(|| now.clone()),
            updated_at: partial.updated_at.unwrap_or(now),
            description: partial.description,
            group: partial.group,
            sync: partial.sync,
        }
    }

    pub fn parse_json_tolerant<T: DeserializeOwned + Default>(
        json_str: &str,
    ) -> ParseResultWithWarnings<T> {
        let mut warnings = Vec::new();

        if let Ok(parsed) = serde_json::from_str::<T>(json_str) {
            return ParseResultWithWarnings::ok(parsed);
        }

        if let Some(repaired) = try_repair_json(json_str) {
            if let Ok(parsed) = serde_json::from_str::<T>(&repaired) {
                warnings.push(ParseWarning {
                    level: WarningLevel::Warning,
                    message: "JSON was repaired automatically".into(),
                    field: None,
                });
                return ParseResultWithWarnings {
                    data: parsed,
                    warnings,
                };
            }
        }

        warnings.push(ParseWarning {
            level: WarningLevel::Error,
            message: "Failed to parse JSON content".into(),
            field: None,
        });

        ParseResultWithWarnings {
            data: T::default(),
            warnings,
        }
    }
}

fn toml_to_json(toml_val: toml::Value) -> serde_json::Value {
    match toml_val {
        toml::Value::String(s) => serde_json::Value::String(s),
        toml::Value::Integer(i) => serde_json::Value::Number(i.into()),
        toml::Value::Float(f) => serde_json::Number::from_f64(f)
            .map_or(serde_json::Value::Null, serde_json::Value::Number),
        toml::Value::Boolean(b) => serde_json::Value::Bool(b),
        toml::Value::Array(arr) => {
            serde_json::Value::Array(arr.into_iter().map(toml_to_json).collect())
        }
        toml::Value::Table(table) => {
            let map: serde_json::Map<String, serde_json::Value> = table
                .into_iter()
                .map(|(k, v)| (k, toml_to_json(v)))
                .collect();
            serde_json::Value::Object(map)
        }
        toml::Value::Datetime(dt) => serde_json::Value::String(dt.to_string()),
    }
}

fn try_repair_json(json_str: &str) -> Option<String> {
    let mut repaired = json_str.trim_end().to_string();

    if repaired.ends_with(",]") {
        repaired = repaired[..repaired.len() - 2].to_string() + "]";
    }
    if repaired.ends_with(",}") {
        repaired = repaired[..repaired.len() - 2].to_string() + "}";
    }

    let open_brackets = repaired.matches('[').count();
    let close_brackets = repaired.matches(']').count();
    if open_brackets > close_brackets {
        repaired.push_str(&"]".repeat(open_brackets - close_brackets));
    }

    let open_braces = repaired.matches('{').count();
    let close_braces = repaired.matches('}').count();
    if open_braces > close_braces {
        repaired.push_str(&"}".repeat(open_braces - close_braces));
    }

    if serde_json::from_str::<serde_json::Value>(&repaired).is_ok() {
        Some(repaired)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_header() {
        let header = BifrostFileParser::parse_header("01 rules").unwrap();
        assert_eq!(header.version, 1);
        assert_eq!(header.file_type, BifrostFileType::Rules);

        let header = BifrostFileParser::parse_header("02 network").unwrap();
        assert_eq!(header.version, 2);
        assert_eq!(header.file_type, BifrostFileType::Network);
    }

    #[test]
    fn test_parse_rules() {
        let content = r#"01 rules

[meta]
name = "test-rules"
enabled = true
sort_order = 0
version = "1.0.0"
created_at = "2024-01-01T00:00:00Z"
updated_at = "2024-01-01T00:00:00Z"

[options]
rule_count = 1

---
example.com proxy://localhost:3000
"#;

        let file = BifrostFileParser::parse_rules(content).unwrap();
        assert_eq!(file.meta.name, "test-rules");
        assert!(file.meta.enabled);
        assert_eq!(file.content.trim(), "example.com proxy://localhost:3000");
    }

    #[test]
    fn test_parse_tolerant_missing_header() {
        let content = r#"[meta]
name = "test"

---
example.com proxy://localhost:3000
"#;

        let result = BifrostFileParser::parse_tolerant(content);
        assert!(result.has_warnings());
    }

    #[test]
    fn test_detect_type() {
        assert_eq!(
            BifrostFileParser::detect_type("01 rules\n").unwrap(),
            BifrostFileType::Rules
        );
        assert_eq!(
            BifrostFileParser::detect_type("01 network\n").unwrap(),
            BifrostFileType::Network
        );
        assert_eq!(
            BifrostFileParser::detect_type("01 script\n").unwrap(),
            BifrostFileType::Script
        );
        assert_eq!(
            BifrostFileParser::detect_type("01 values\n").unwrap(),
            BifrostFileType::Values
        );
        assert_eq!(
            BifrostFileParser::detect_type("01 template\n").unwrap(),
            BifrostFileType::Template
        );
    }

    #[test]
    fn parse_network_accepts_legacy_record_without_active_rules() {
        let content = r#"01 network

[meta]
name = "legacy-network"
version = "1.0.0"
created_at = "2026-05-20T00:00:00Z"

[options]
count = 1

---
[
  {
    "id": "REQ-legacy",
    "method": "GET",
    "url": "http://legacy.example.test/",
    "status": 200,
    "duration_ms": 1,
    "timestamp": 1760000000000
  }
]
"#;

        let file = BifrostFileParser::parse_network(content).unwrap();

        assert_eq!(file.content.len(), 1);
        assert_eq!(file.content[0].listener_port, None);
        assert!(file.content[0].active_rules.is_none());
    }

    #[test]
    fn parse_error_display_covers_all_variants() {
        let cases = [
            ParseError::EmptyFile,
            ParseError::InvalidHeader("h".into()),
            ParseError::InvalidVersion("v".into()),
            ParseError::InvalidType("t".into()),
            ParseError::MissingSeparator,
            ParseError::InvalidMeta("m".into()),
            ParseError::InvalidContent("c".into()),
            ParseError::TypeMismatch {
                expected: BifrostFileType::Rules,
                actual: BifrostFileType::Network,
            },
        ];
        for err in cases {
            assert!(!err.to_string().is_empty());
        }
    }

    #[test]
    fn parse_header_rejects_malformed() {
        assert!(matches!(
            BifrostFileParser::parse_header("only-one"),
            Err(ParseError::InvalidHeader(_))
        ));
        assert!(matches!(
            BifrostFileParser::parse_header("xx rules"),
            Err(ParseError::InvalidVersion(_))
        ));
        assert!(matches!(
            BifrostFileParser::parse_header("01 bogus"),
            Err(ParseError::InvalidType(_))
        ));
    }

    #[test]
    fn parse_raw_errors_on_empty_and_missing_separator() {
        assert!(matches!(
            BifrostFileParser::parse_raw(""),
            Err(ParseError::EmptyFile)
        ));
        assert!(matches!(
            BifrostFileParser::parse_raw("01 rules\n\n[meta]\nname=\"x\"\n"),
            Err(ParseError::MissingSeparator)
        ));
    }

    #[test]
    fn parse_raw_handles_separator_without_trailing_newline() {
        // "\n---" at end of input (no trailing newline) goes through the second branch.
        let raw = BifrostFileParser::parse_raw("01 rules\n\nname=\"x\"\n---").unwrap();
        assert_eq!(raw.header.file_type, BifrostFileType::Rules);
        assert_eq!(raw.content_raw, "");
    }

    #[test]
    fn parse_rules_detects_type_mismatch() {
        let content = "01 network\n\n[meta]\nname=\"x\"\n\n---\nexample.com proxy://x\n";
        assert!(matches!(
            BifrostFileParser::parse_rules(content),
            Err(ParseError::TypeMismatch { .. })
        ));
    }

    #[test]
    fn parse_rules_errors_on_invalid_toml_meta() {
        let content = "01 rules\n\nnot valid toml = = =\n---\nbody\n";
        assert!(matches!(
            BifrostFileParser::parse_rules(content),
            Err(ParseError::InvalidMeta(_))
        ));
    }

    #[test]
    fn parse_script_round_trip_and_errors() {
        let content = r#"01 script

[meta]
name = "s"
version = "1.0.0"
created_at = "2026-01-01T00:00:00Z"

---
[{"name":"hello","script_type":"request","content":"print(1)"}]
"#;
        let file = BifrostFileParser::parse_script(content).unwrap();
        assert_eq!(file.content.len(), 1);
        assert_eq!(file.content[0].name, "hello");

        // Type mismatch.
        let mismatch = content.replacen("01 script", "01 rules", 1);
        assert!(matches!(
            BifrostFileParser::parse_script(&mismatch),
            Err(ParseError::TypeMismatch { .. })
        ));

        // Invalid content JSON.
        let bad = "01 script\n\n[meta]\nname=\"s\"\ncreated_at=\"t\"\n---\nnot-json\n";
        assert!(matches!(
            BifrostFileParser::parse_script(bad),
            Err(ParseError::InvalidContent(_))
        ));
    }

    #[test]
    fn parse_values_round_trip_and_mismatch() {
        let content = r#"01 values

[meta]
name = "v"
created_at = "2026-01-01T00:00:00Z"

---
{"KEY":"VAL"}
"#;
        let file = BifrostFileParser::parse_values(content).unwrap();
        assert_eq!(file.content.get("KEY").map(String::as_str), Some("VAL"));

        let mismatch = content.replacen("01 values", "01 rules", 1);
        assert!(matches!(
            BifrostFileParser::parse_values(&mismatch),
            Err(ParseError::TypeMismatch { .. })
        ));
    }

    #[test]
    fn parse_template_round_trip_and_mismatch() {
        let content = r#"01 template

[meta]
name = "t"
created_at = "2026-01-01T00:00:00Z"

---
{"requests":[]}
"#;
        let file = BifrostFileParser::parse_template(content).unwrap();
        assert!(file.content.requests.is_empty());

        let mismatch = content.replacen("01 template", "01 network", 1);
        assert!(matches!(
            BifrostFileParser::parse_template(&mismatch),
            Err(ParseError::TypeMismatch { .. })
        ));
    }

    #[test]
    fn parse_network_errors_on_invalid_content() {
        let bad = "01 network\n\n[meta]\nname=\"n\"\ncreated_at=\"t\"\n---\nnot-json\n";
        assert!(matches!(
            BifrostFileParser::parse_network(bad),
            Err(ParseError::InvalidContent(_))
        ));
    }

    #[test]
    fn parse_export_meta_errors_on_invalid_toml() {
        // network parse routes through parse_export_meta; invalid TOML meta errors.
        let bad = "01 network\n\n= = bad\n---\n[]\n";
        assert!(matches!(
            BifrostFileParser::parse_network(bad),
            Err(ParseError::InvalidMeta(_))
        ));
    }

    #[test]
    fn detect_type_errors_on_empty() {
        assert!(matches!(
            BifrostFileParser::detect_type(""),
            Err(ParseError::EmptyFile)
        ));
    }

    #[test]
    fn parse_header_tolerant_infers_defaults() {
        // No valid header line -> defaults version 1 and inferred type, with warnings.
        let (header, warnings) =
            BifrostFileParser::parse_header_tolerant("garbage line\n\nfoo.com proxy://x\n");
        assert_eq!(header.version, 1);
        assert!(!warnings.is_empty());
    }

    #[test]
    fn infer_type_from_content_defaults_to_rules() {
        let inferred = BifrostFileParser::infer_type_from_content("xx\n\nexample.com proxy://x\n");
        assert_eq!(inferred, BifrostFileType::Rules);
    }

    #[test]
    fn split_meta_content_tolerant_handles_separator_no_newline() {
        let header = BifrostFileHeader {
            version: 1,
            file_type: BifrostFileType::Rules,
        };
        let (meta, body, warnings) = BifrostFileParser::split_meta_content_tolerant(
            "01 rules\nname=\"x\"\n---body-here",
            &header,
        );
        assert!(meta.contains("name"));
        assert_eq!(body, "body-here");
        assert!(warnings.iter().any(|w| w.level == WarningLevel::Info));
    }

    #[test]
    fn split_rules_content_separates_meta_and_rules() {
        let mut warnings = Vec::new();
        let (meta, body) = BifrostFileParser::split_rules_content(
            "[meta]\nname = \"x\"\nexample.com proxy://localhost:3000\n",
            &mut warnings,
        );
        assert!(meta.contains("[meta]"));
        assert!(body.contains("example.com proxy://localhost:3000"));
    }

    #[test]
    fn split_rules_content_treats_all_as_rules_without_meta() {
        let mut warnings = Vec::new();
        let (meta, body) =
            BifrostFileParser::split_rules_content("# just a comment\n# another\n", &mut warnings);
        assert!(meta.is_empty());
        assert!(body.contains("just a comment"));
        assert!(warnings.iter().any(|w| w.level == WarningLevel::Warning));
    }

    #[test]
    fn split_json_content_finds_array_start() {
        let mut warnings = Vec::new();
        let (meta, body) =
            BifrostFileParser::split_json_content("name = \"x\"\n[\n  1\n]\n", &mut warnings);
        assert!(meta.contains("name"));
        assert!(body.trim_start().starts_with('['));
    }

    #[test]
    fn split_json_content_treats_all_as_json_when_leading_bracket() {
        let mut warnings = Vec::new();
        let (meta, body) = BifrostFileParser::split_json_content("[1, 2, 3]", &mut warnings);
        // The first line itself starts with '[', so it is detected via the loop
        // branch: meta is empty and the whole input is the body.
        assert!(meta.is_empty());
        assert!(body.contains("[1, 2, 3]"));
    }

    #[test]
    fn split_json_content_no_json_returns_all_as_meta() {
        let mut warnings = Vec::new();
        let (meta, body) = BifrostFileParser::split_json_content("name = \"x\"\n", &mut warnings);
        assert!(meta.contains("name"));
        assert!(body.is_empty());
    }

    #[test]
    fn parse_rules_tolerant_uses_defaults_when_meta_missing() {
        let result = BifrostFileParser::parse_rules_tolerant("01 rules\n\nexample.com proxy://x\n");
        // With no meta section, an empty meta yields the default RuleFileMeta
        // (empty name) and emits a warning about the missing meta.
        assert!(result.data.meta.name.is_empty());
        assert!(result.has_warnings());
    }

    #[test]
    fn parse_rules_meta_tolerant_handles_empty_invalid_and_valid() {
        let mut warnings = Vec::new();
        // Empty meta -> default RuleFileMeta (empty name) + warning.
        let meta = BifrostFileParser::parse_rules_meta_tolerant("", &mut warnings);
        assert!(meta.name.is_empty());
        assert!(!warnings.is_empty());

        // Invalid TOML -> error warning + defaults.
        let mut warnings2 = Vec::new();
        let meta2 = BifrostFileParser::parse_rules_meta_tolerant("= = =", &mut warnings2);
        assert_eq!(meta2.name, "unnamed");
        assert!(warnings2.iter().any(|w| w.level == WarningLevel::Error));

        // Valid meta with a name -> fields are applied. The tolerant partial
        // schema requires a `sync` table to be present, so include it.
        let mut warnings3 = Vec::new();
        let meta3 = BifrostFileParser::parse_rules_meta_tolerant(
            "[meta]\nname = \"real\"\nenabled = false\nsort_order = 5\n[meta.sync]\nrule_id = \"r1\"\n",
            &mut warnings3,
        );
        assert_eq!(meta3.name, "real");
        assert!(!meta3.enabled);
        assert_eq!(meta3.sort_order, 5);
    }

    #[test]
    fn parse_json_tolerant_ok_repair_and_failure() {
        // Valid JSON.
        let ok: ParseResultWithWarnings<Vec<i32>> =
            BifrostFileParser::parse_json_tolerant("[1,2,3]");
        assert_eq!(ok.data, vec![1, 2, 3]);
        assert!(!ok.has_warnings());

        // Repairable (trailing comma + missing bracket).
        let repaired: ParseResultWithWarnings<Vec<i32>> =
            BifrostFileParser::parse_json_tolerant("[1,2,3,]");
        assert_eq!(repaired.data, vec![1, 2, 3]);
        assert!(repaired.has_warnings());

        // Unrepairable -> default + error warning.
        let failed: ParseResultWithWarnings<Vec<i32>> =
            BifrostFileParser::parse_json_tolerant("totally not json {[");
        assert!(failed.data.is_empty());
        assert!(failed.has_errors());
    }

    #[test]
    fn toml_to_json_converts_all_value_kinds() {
        let toml_val: toml::Value =
            toml::from_str("s = \"x\"\ni = 3\nf = 1.5\nb = true\narr = [1, 2]\n[tbl]\nk = \"v\"\n")
                .unwrap();
        let json = toml_to_json(toml_val);
        assert_eq!(json["s"], serde_json::json!("x"));
        assert_eq!(json["i"], serde_json::json!(3));
        assert_eq!(json["b"], serde_json::json!(true));
        assert_eq!(json["arr"], serde_json::json!([1, 2]));
        assert_eq!(json["tbl"]["k"], serde_json::json!("v"));
    }

    #[test]
    fn toml_to_json_handles_datetime() {
        let toml_val: toml::Value = toml::from_str("dt = 2026-01-01T00:00:00Z\n").unwrap();
        let json = toml_to_json(toml_val);
        assert!(json["dt"].is_string());
    }

    #[test]
    fn try_repair_json_closes_brackets_and_braces() {
        assert_eq!(try_repair_json("[1, 2,]").as_deref(), Some("[1, 2]"));
        assert_eq!(try_repair_json("{\"a\":1,}").as_deref(), Some("{\"a\":1}"));
        assert!(try_repair_json("[1, 2").is_some());
        assert!(try_repair_json("{\"a\": 1").is_some());
        // Hopelessly broken -> None.
        assert!(try_repair_json("][}{").is_none());
    }
}

#[cfg(test)]
mod extra_tests {
    use super::*;

    #[test]
    fn parse_rules_meta_tolerant_handles_invalid_toml_with_error_warning() {
        let mut warnings = Vec::new();
        let meta = BifrostFileParser::parse_rules_meta_tolerant("not-toml", &mut warnings);
        assert_eq!(meta.name, "unnamed");
        assert!(meta.enabled);
        assert!(meta.version.starts_with('1'));
        assert!(warnings
            .iter()
            .any(|w| matches!(w.level, WarningLevel::Error)));
    }

    #[test]
    fn parse_json_tolerant_repairs_trailing_commas() {
        let json = "[1,2,3,]";
        let result: ParseResultWithWarnings<Vec<u32>> =
            BifrostFileParser::parse_json_tolerant(json);
        assert!(result.has_warnings());
        assert!(!result.has_errors());
        assert_eq!(result.data, vec![1, 2, 3]);
    }

    #[test]
    fn parse_json_tolerant_returns_default_and_error_on_unrepairable_input() {
        let json = "this is not json";
        let result: ParseResultWithWarnings<serde_json::Value> =
            BifrostFileParser::parse_json_tolerant(json);
        assert!(result.has_errors());
        // default for serde_json::Value is Null
        assert!(result.data.is_null());
    }
}

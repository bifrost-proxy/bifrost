use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use ipnet::IpNet;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{BifrostError, Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileDocument {
    pub source: ProfileSource,
    pub raw_text: String,
    pub sections: Vec<ProfileSection>,
    pub diagnostics: Vec<ProfileDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProfileSource {
    LocalPath(PathBuf),
    ManagedUrl(String),
    Inline,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSection {
    pub name: String,
    pub kind: ProfileSectionKind,
    pub line: usize,
    pub entries: Vec<ProfileEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProfileSectionKind {
    General,
    Proxy,
    ProxyGroup,
    Rule,
    Dns,
    Host,
    Mitm,
    UrlRewrite,
    MapLocal,
    HeaderRewrite,
    Script,
    Module,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProfileEntry {
    KeyValue(KeyValueEntry),
    Rule(RuleNode),
    Proxy(ProxyNode),
    PolicyGroup(PolicyGroupNode),
    Directive(DirectiveNode),
    Comment(SourceLine),
    Raw(SourceLine),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceLine {
    pub line: usize,
    pub column: usize,
    pub raw: String,
    pub content: String,
    pub comment: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyValueEntry {
    pub source: SourceLine,
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleNode {
    pub source: SourceLine,
    pub rule_type: String,
    pub value: Option<String>,
    pub policy: String,
    pub parameters: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyNode {
    pub source: SourceLine,
    pub name: String,
    pub protocol: String,
    pub fields: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyGroupNode {
    pub source: SourceLine,
    pub name: String,
    pub group_type: String,
    pub policies: Vec<String>,
    pub parameters: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectiveNode {
    pub source: SourceLine,
    pub directive: String,
    pub arguments: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileDiagnostic {
    pub severity: DiagnosticSeverity,
    pub line: usize,
    pub column: usize,
    pub code: String,
    pub message: String,
    pub suggestion: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SupportLevel {
    FullySupported,
    TranslatedWithBehaviorNote,
    NeedsManualReview,
    NotSupportedYet,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompatibilityReport {
    pub source: ProfileSource,
    pub summary: CompatibilitySummary,
    pub items: Vec<CompatibilityItem>,
    pub diagnostics: Vec<ProfileDiagnostic>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompatibilitySummary {
    pub fully_supported: usize,
    pub translated_with_behavior_note: usize,
    pub needs_manual_review: usize,
    pub not_supported_yet: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompatibilityItem {
    pub level: SupportLevel,
    pub section: String,
    pub line: usize,
    pub capability: String,
    pub message: String,
    pub suggestion: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplainRequest {
    pub url: String,
    pub host: String,
    pub resolved_ip: Option<IpAddr>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplainReport {
    pub request: ExplainRequest,
    pub matched_rule: Option<RuleNode>,
    pub target_policy: Option<String>,
    pub timeline: Vec<ExplainStep>,
    pub diagnostics: Vec<ProfileDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplainStep {
    pub stage: String,
    pub line: Option<usize>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversionPreview {
    pub format: String,
    pub content: String,
    pub report: CompatibilityReport,
}

pub fn parse_surge_profile_file(path: &Path) -> Result<ProfileDocument> {
    let text = std::fs::read_to_string(path)?;
    Ok(parse_surge_profile(
        &text,
        ProfileSource::LocalPath(path.to_path_buf()),
    ))
}

pub fn parse_surge_profile(text: &str, source: ProfileSource) -> ProfileDocument {
    let mut document = ProfileDocument {
        source,
        raw_text: text.to_string(),
        sections: Vec::new(),
        diagnostics: Vec::new(),
    };
    let mut current: Option<ProfileSection> = None;

    for (index, raw_line) in text.lines().enumerate() {
        let line_no = index + 1;
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if is_section_header(trimmed) {
            if let Some(section) = current.take() {
                document.sections.push(section);
            }
            let name = trimmed
                .trim_start_matches('[')
                .trim_end_matches(']')
                .trim()
                .to_string();
            current = Some(ProfileSection {
                kind: section_kind(&name),
                name,
                line: line_no,
                entries: Vec::new(),
            });
            continue;
        }

        let Some(section) = current.as_mut() else {
            document.diagnostics.push(ProfileDiagnostic {
                severity: DiagnosticSeverity::Warning,
                line: line_no,
                column: 1,
                code: "surge.line_outside_section".to_string(),
                message: "Line is outside any Surge profile section".to_string(),
                suggestion: Some(
                    "Move this line under a section such as [General] or [Rule]".to_string(),
                ),
            });
            continue;
        };

        section.entries.push(parse_entry(
            section.kind,
            raw_line,
            line_no,
            &mut document.diagnostics,
        ));
    }

    if let Some(section) = current {
        document.sections.push(section);
    }

    document
}

pub fn analyze_compatibility(document: &ProfileDocument) -> CompatibilityReport {
    let mut items = Vec::new();
    for section in &document.sections {
        for entry in &section.entries {
            if let Some(item) = analyze_entry(section, entry) {
                items.push(item);
            }
        }
    }

    let mut summary = CompatibilitySummary::default();
    for item in &items {
        match item.level {
            SupportLevel::FullySupported => summary.fully_supported += 1,
            SupportLevel::TranslatedWithBehaviorNote => summary.translated_with_behavior_note += 1,
            SupportLevel::NeedsManualReview => summary.needs_manual_review += 1,
            SupportLevel::NotSupportedYet => summary.not_supported_yet += 1,
        }
    }

    CompatibilityReport {
        source: document.source.clone(),
        summary,
        items,
        diagnostics: document.diagnostics.clone(),
    }
}

pub fn explain_surge_request(document: &ProfileDocument, input: &str) -> Result<ExplainReport> {
    let request = parse_explain_request(input)?;
    let mut timeline = vec![
        ExplainStep {
            stage: "input".to_string(),
            line: None,
            message: format!("URL={} host={}", request.url, request.host),
        },
        ExplainStep {
            stage: "dns".to_string(),
            line: None,
            message: "Surge Bridge dry-run does not resolve DNS; using URL host and optional literal IP only".to_string(),
        },
    ];

    let mut diagnostics = document.diagnostics.clone();
    for rule in iter_rules(document) {
        let (matched, reason) = rule_matches_request(rule, &request);
        timeline.push(ExplainStep {
            stage: "rule".to_string(),
            line: Some(rule.source.line),
            message: reason,
        });
        if matched {
            timeline.push(ExplainStep {
                stage: "policy".to_string(),
                line: Some(rule.source.line),
                message: format!("Selected policy {}", rule.policy),
            });
            timeline.push(ExplainStep {
                stage: "mitm".to_string(),
                line: None,
                message: explain_mitm(document, &request.host),
            });
            return Ok(ExplainReport {
                request,
                matched_rule: Some(rule.clone()),
                target_policy: Some(rule.policy.clone()),
                timeline,
                diagnostics,
            });
        }
    }

    diagnostics.push(ProfileDiagnostic {
        severity: DiagnosticSeverity::Warning,
        line: 1,
        column: 1,
        code: "surge.rule.no_match".to_string(),
        message: "No Surge rule matched this request; Surge profiles should end with FINAL"
            .to_string(),
        suggestion: Some(
            "Add a FINAL rule or inspect unsupported rules in the compatibility report".to_string(),
        ),
    });
    timeline.push(ExplainStep {
        stage: "rule".to_string(),
        line: None,
        message: "No matching rule found".to_string(),
    });

    Ok(ExplainReport {
        request,
        matched_rule: None,
        target_policy: None,
        timeline,
        diagnostics,
    })
}

pub fn convert_surge_to_bifrost_preview(document: &ProfileDocument) -> ConversionPreview {
    let report = analyze_compatibility(document);
    let mut content = String::new();
    content.push_str("# Bifrost Native Profile Preview\n");
    content.push_str("# Generated from a Surge profile in dry-run mode.\n");
    content.push_str("# Unsupported or behavior-changing items are kept as comments.\n\n");

    content.push_str("[policies]\n");
    for group in iter_policy_groups(document) {
        match group.group_type.as_str() {
            "select" => {
                content.push_str(&format!(
                    "{} = select, {}\n",
                    group.name,
                    group.policies.join(", ")
                ));
            }
            _ => {
                content.push_str(&format!(
                    "# line {}: {} = {}, {}  # not supported in the first Bridge preview\n",
                    group.source.line,
                    group.name,
                    group.group_type,
                    group.policies.join(", ")
                ));
            }
        }
    }

    content.push_str("\n[rules]\n");
    for rule in iter_rules(document) {
        let line = match rule.rule_type.as_str() {
            "DOMAIN" => format!(
                "host == {} -> {}",
                rule.value.as_deref().unwrap_or(""),
                rule.policy
            ),
            "DOMAIN-SUFFIX" => format!(
                "host suffix {} -> {}",
                rule.value.as_deref().unwrap_or(""),
                rule.policy
            ),
            "DOMAIN-KEYWORD" => format!(
                "# line {}: host keyword {} -> {}  # behavior note: host-only keyword",
                rule.source.line,
                rule.value.as_deref().unwrap_or(""),
                rule.policy
            ),
            "IP-CIDR" | "IP-CIDR6" => format!(
                "ip cidr {} -> {}",
                rule.value.as_deref().unwrap_or(""),
                rule.policy
            ),
            "FINAL" => format!("final -> {}", rule.policy),
            _ => format!(
                "# line {}: {}  # not supported in the first Bridge preview",
                rule.source.line, rule.source.content
            ),
        };
        content.push_str(&line);
        content.push('\n');
    }

    ConversionPreview {
        format: "bifrost-native-profile-preview".to_string(),
        content,
        report,
    }
}

fn parse_entry(
    kind: ProfileSectionKind,
    raw_line: &str,
    line_no: usize,
    diagnostics: &mut Vec<ProfileDiagnostic>,
) -> ProfileEntry {
    let source = source_line(raw_line, line_no);
    let trimmed = source.content.trim();

    if trimmed.is_empty() {
        return ProfileEntry::Comment(source);
    }
    if is_comment(trimmed) {
        return ProfileEntry::Comment(source);
    }
    if let Some(directive) = parse_directive(&source) {
        return ProfileEntry::Directive(directive);
    }

    match kind {
        ProfileSectionKind::Rule => parse_rule(source, diagnostics),
        ProfileSectionKind::Proxy => parse_proxy(source, diagnostics),
        ProfileSectionKind::ProxyGroup => parse_policy_group(source, diagnostics),
        ProfileSectionKind::General
        | ProfileSectionKind::Dns
        | ProfileSectionKind::Host
        | ProfileSectionKind::Mitm
        | ProfileSectionKind::UrlRewrite
        | ProfileSectionKind::MapLocal
        | ProfileSectionKind::HeaderRewrite
        | ProfileSectionKind::Script
        | ProfileSectionKind::Module
        | ProfileSectionKind::Unknown => parse_key_value_or_raw(source),
    }
}

fn parse_rule(source: SourceLine, diagnostics: &mut Vec<ProfileDiagnostic>) -> ProfileEntry {
    let fields = split_csv(&source.content);
    if fields.is_empty() {
        return ProfileEntry::Raw(source);
    }
    let rule_type = fields[0].to_ascii_uppercase();
    if rule_type == "FINAL" {
        if fields.len() < 2 {
            diagnostics.push(ProfileDiagnostic {
                severity: DiagnosticSeverity::Error,
                line: source.line,
                column: 1,
                code: "surge.rule.final_missing_policy".to_string(),
                message: "FINAL rule is missing a target policy".to_string(),
                suggestion: Some("Use FINAL,<POLICY>".to_string()),
            });
            return ProfileEntry::Raw(source);
        }
        return ProfileEntry::Rule(RuleNode {
            source,
            rule_type,
            value: None,
            policy: fields[1].clone(),
            parameters: fields.into_iter().skip(2).collect(),
        });
    }

    if fields.len() < 3 {
        diagnostics.push(ProfileDiagnostic {
            severity: DiagnosticSeverity::Error,
            line: source.line,
            column: 1,
            code: "surge.rule.missing_fields".to_string(),
            message: "Surge rule must contain TYPE, VALUE, POLICY".to_string(),
            suggestion: Some("Example: DOMAIN-SUFFIX,example.com,DIRECT".to_string()),
        });
        return ProfileEntry::Raw(source);
    }

    ProfileEntry::Rule(RuleNode {
        source,
        rule_type,
        value: Some(fields[1].clone()),
        policy: fields[2].clone(),
        parameters: fields.into_iter().skip(3).collect(),
    })
}

fn parse_proxy(source: SourceLine, diagnostics: &mut Vec<ProfileDiagnostic>) -> ProfileEntry {
    let content = source.content.clone();
    let Some((name, value)) = content.split_once('=') else {
        diagnostics.push(ProfileDiagnostic {
            severity: DiagnosticSeverity::Warning,
            line: source.line,
            column: 1,
            code: "surge.proxy.expected_assignment".to_string(),
            message: "Proxy entries should use NAME = protocol, host, port".to_string(),
            suggestion: None,
        });
        return ProfileEntry::Raw(source);
    };
    let name = name.trim().to_string();
    let value = value.to_string();
    let fields = split_csv(&value);
    if fields.is_empty() {
        return ProfileEntry::Raw(source);
    }
    ProfileEntry::Proxy(ProxyNode {
        source,
        name,
        protocol: fields[0].to_ascii_lowercase(),
        fields: fields.into_iter().skip(1).collect(),
    })
}

fn parse_policy_group(
    source: SourceLine,
    diagnostics: &mut Vec<ProfileDiagnostic>,
) -> ProfileEntry {
    let content = source.content.clone();
    let Some((name, value)) = content.split_once('=') else {
        diagnostics.push(ProfileDiagnostic {
            severity: DiagnosticSeverity::Warning,
            line: source.line,
            column: 1,
            code: "surge.policy_group.expected_assignment".to_string(),
            message: "Policy group entries should use NAME = type, policy...".to_string(),
            suggestion: None,
        });
        return ProfileEntry::Raw(source);
    };
    let name = name.trim().to_string();
    let value = value.to_string();
    let fields = split_csv(&value);
    if fields.is_empty() {
        return ProfileEntry::Raw(source);
    }
    let mut policies = Vec::new();
    let mut parameters = BTreeMap::new();
    for field in fields.iter().skip(1) {
        if let Some((key, value)) = field.split_once('=') {
            parameters.insert(key.trim().to_string(), value.trim().to_string());
        } else {
            policies.push(field.to_string());
        }
    }
    ProfileEntry::PolicyGroup(PolicyGroupNode {
        source,
        name,
        group_type: fields[0].to_ascii_lowercase(),
        policies,
        parameters,
    })
}

fn parse_key_value_or_raw(source: SourceLine) -> ProfileEntry {
    let content = source.content.clone();
    if let Some((key, value)) = content.split_once('=') {
        let key = key.trim().to_string();
        let value = value.trim().to_string();
        return ProfileEntry::KeyValue(KeyValueEntry { source, key, value });
    }
    ProfileEntry::Raw(source)
}

fn parse_directive(source: &SourceLine) -> Option<DirectiveNode> {
    let content = source.content.trim_start();
    if !content.starts_with("#!") {
        return None;
    }
    let body = content.trim_start_matches("#!").trim();
    let (directive, arguments) = body
        .split_once(char::is_whitespace)
        .map(|(directive, arguments)| (directive, arguments.trim()))
        .unwrap_or((body, ""));
    Some(DirectiveNode {
        source: source.clone(),
        directive: directive.to_ascii_uppercase(),
        arguments: arguments.to_string(),
    })
}

fn analyze_entry(section: &ProfileSection, entry: &ProfileEntry) -> Option<CompatibilityItem> {
    match entry {
        ProfileEntry::Rule(rule) => Some(analyze_rule(section, rule)),
        ProfileEntry::PolicyGroup(group) => Some(analyze_policy_group(section, group)),
        ProfileEntry::Directive(directive) => Some(analyze_directive(section, directive)),
        ProfileEntry::Proxy(proxy) => Some(CompatibilityItem {
            level: SupportLevel::TranslatedWithBehaviorNote,
            section: section.name.clone(),
            line: proxy.source.line,
            capability: format!("proxy:{}", proxy.protocol),
            message: "Proxy node is parsed and preserved for conversion preview; runtime outbound proxy activation is not part of Surge Bridge".to_string(),
            suggestion: Some("Review the converted Bifrost profile before enabling in a later runtime iteration".to_string()),
        }),
        ProfileEntry::KeyValue(kv) => Some(analyze_key_value(section, kv)),
        ProfileEntry::Raw(raw) if !raw.content.trim().is_empty() => Some(CompatibilityItem {
            level: SupportLevel::NeedsManualReview,
            section: section.name.clone(),
            line: raw.line,
            capability: "raw".to_string(),
            message: "Line is preserved but not semantically understood by the first Surge Bridge parser".to_string(),
            suggestion: Some("Keep the line for manual review or add parser support before conversion".to_string()),
        }),
        _ => None,
    }
}

fn analyze_rule(section: &ProfileSection, rule: &RuleNode) -> CompatibilityItem {
    let (level, message, suggestion) = match rule.rule_type.as_str() {
        "DOMAIN" => (
            SupportLevel::FullySupported,
            "Exact host rule can be evaluated in Surge-compatible order",
            None,
        ),
        "DOMAIN-SUFFIX" => (
            SupportLevel::FullySupported,
            "Domain suffix rule can be evaluated against bare domain and subdomains",
            None,
        ),
        "DOMAIN-KEYWORD" => (
            SupportLevel::TranslatedWithBehaviorNote,
            "Keyword rule is interpreted against host only in the first Bridge preview",
            Some("Use profile explain to compare any path-sensitive expectation"),
        ),
        "IP-CIDR" | "IP-CIDR6" => (
            SupportLevel::FullySupported,
            "CIDR rule can be evaluated when a resolved IP is available",
            None,
        ),
        "RULE-SET" | "DOMAIN-SET" => (
            SupportLevel::TranslatedWithBehaviorNote,
            "Remote set references are parsed but not fetched or expanded in this dry-run slice",
            Some("Run managed resource expansion in a later Bridge iteration before activation"),
        ),
        "FINAL" => (
            SupportLevel::FullySupported,
            "FINAL rule works as the ordered evaluator fallback",
            None,
        ),
        "GEOIP" => (
            SupportLevel::NotSupportedYet,
            "GEOIP needs a geo database and is planned after the first Bridge parser",
            Some("Keep this profile in dry-run until geo database support lands"),
        ),
        "PROCESS-NAME" => (
            SupportLevel::NotSupportedYet,
            "PROCESS-NAME needs cross-platform process metadata in the evaluator",
            Some("Use existing Bifrost process attribution separately until profile runtime support lands"),
        ),
        "SCRIPT" => (
            SupportLevel::NeedsManualReview,
            "Script decision rules require manual review and sandbox migration",
            Some("Migrate script content separately before enabling dynamic decisions"),
        ),
        _ => (
            SupportLevel::NeedsManualReview,
            "Rule type is preserved but not part of the first supported matrix",
            Some("Inspect this rule before converting to a Bifrost-native runtime"),
        ),
    };

    CompatibilityItem {
        level,
        section: section.name.clone(),
        line: rule.source.line,
        capability: rule.rule_type.clone(),
        message: message.to_string(),
        suggestion: suggestion.map(str::to_string),
    }
}

fn analyze_policy_group(section: &ProfileSection, group: &PolicyGroupNode) -> CompatibilityItem {
    let (level, message, suggestion) = match group.group_type.as_str() {
        "select" => (
            SupportLevel::NeedsManualReview,
            "select group is parsed but not active in the first Bridge runtime",
            Some("Iteration two will add policy group runtime state"),
        ),
        "url-test" | "fallback" => (
            SupportLevel::NotSupportedYet,
            "Active health-based policy group runtime is planned for iteration two",
            Some("Keep this group as review-only until Policy Health Store exists"),
        ),
        "load-balance" | "subnet" => (
            SupportLevel::NotSupportedYet,
            "This policy group type is outside the first replacement scope",
            Some("Track under later policy scheduler work"),
        ),
        _ => (
            SupportLevel::NeedsManualReview,
            "Unknown policy group type is preserved for manual review",
            None,
        ),
    };
    CompatibilityItem {
        level,
        section: section.name.clone(),
        line: group.source.line,
        capability: format!("policy-group:{}", group.group_type),
        message: message.to_string(),
        suggestion: suggestion.map(str::to_string),
    }
}

fn analyze_directive(section: &ProfileSection, directive: &DirectiveNode) -> CompatibilityItem {
    let (level, message) = match directive.directive.as_str() {
        "INCLUDE" => (
            SupportLevel::TranslatedWithBehaviorNote,
            "include directive is detected and preserved; local/remote loading is not performed in this dry-run parser",
        ),
        "MANAGED-CONFIG" => (
            SupportLevel::TranslatedWithBehaviorNote,
            "managed profile directive is detected and preserved for review",
        ),
        "REQUIREMENT" | "IOS-ONLY" | "MACOS-ONLY" | "TVOS-ONLY" => (
            SupportLevel::NeedsManualReview,
            "requirement directive is preserved but not evaluated against device context",
        ),
        _ => (
            SupportLevel::NeedsManualReview,
            "unknown directive is preserved for manual review",
        ),
    };
    CompatibilityItem {
        level,
        section: section.name.clone(),
        line: directive.source.line,
        capability: format!("directive:{}", directive.directive),
        message: message.to_string(),
        suggestion: Some("Review this directive before enabling the converted profile".to_string()),
    }
}

fn analyze_key_value(section: &ProfileSection, kv: &KeyValueEntry) -> CompatibilityItem {
    let (level, message) = match section.kind {
        ProfileSectionKind::General
        | ProfileSectionKind::Dns
        | ProfileSectionKind::Host
        | ProfileSectionKind::Mitm => (
            SupportLevel::TranslatedWithBehaviorNote,
            "Configuration is parsed and preserved; active runtime mapping is planned for a later iteration",
        ),
        ProfileSectionKind::UrlRewrite
        | ProfileSectionKind::MapLocal
        | ProfileSectionKind::HeaderRewrite
        | ProfileSectionKind::Script => (
            SupportLevel::NeedsManualReview,
            "HTTP pipeline entry is parsed as text and needs manual migration",
        ),
        _ => (
            SupportLevel::NeedsManualReview,
            "Key/value entry is preserved for manual review",
        ),
    };
    CompatibilityItem {
        level,
        section: section.name.clone(),
        line: kv.source.line,
        capability: kv.key.clone(),
        message: message.to_string(),
        suggestion: None,
    }
}

fn parse_explain_request(input: &str) -> Result<ExplainRequest> {
    let normalized = if input.contains("://") {
        input.to_string()
    } else {
        format!("https://{input}")
    };
    let url = Url::parse(&normalized).map_err(|err| {
        BifrostError::Parse(format!("invalid profile explain URL '{input}': {err}"))
    })?;
    let host = url
        .host_str()
        .ok_or_else(|| BifrostError::Parse(format!("profile explain URL has no host: {input}")))?
        .to_ascii_lowercase();
    let resolved_ip = host.parse::<IpAddr>().ok();
    Ok(ExplainRequest {
        url: normalized,
        host,
        resolved_ip,
    })
}

fn rule_matches_request(rule: &RuleNode, request: &ExplainRequest) -> (bool, String) {
    match rule.rule_type.as_str() {
        "DOMAIN" => {
            let expected = rule.value.as_deref().unwrap_or("").to_ascii_lowercase();
            let matched = request.host == expected;
            (
                matched,
                format!(
                    "{} DOMAIN {} against host {}",
                    if matched { "matched" } else { "skipped" },
                    expected,
                    request.host
                ),
            )
        }
        "DOMAIN-SUFFIX" => {
            let suffix = rule
                .value
                .as_deref()
                .unwrap_or("")
                .trim_start_matches('.')
                .to_ascii_lowercase();
            let matched = request.host == suffix || request.host.ends_with(&format!(".{suffix}"));
            (
                matched,
                format!(
                    "{} DOMAIN-SUFFIX {} against host {}",
                    if matched { "matched" } else { "skipped" },
                    suffix,
                    request.host
                ),
            )
        }
        "DOMAIN-KEYWORD" => {
            let keyword = rule.value.as_deref().unwrap_or("").to_ascii_lowercase();
            let matched = request.host.contains(&keyword);
            (
                matched,
                format!(
                    "{} DOMAIN-KEYWORD {} against host {}",
                    if matched { "matched" } else { "skipped" },
                    keyword,
                    request.host
                ),
            )
        }
        "IP-CIDR" | "IP-CIDR6" => {
            let Some(ip) = request.resolved_ip else {
                return (
                    false,
                    format!(
                        "skipped {} {} because no resolved IP is available in dry-run explain",
                        rule.rule_type,
                        rule.value.as_deref().unwrap_or("")
                    ),
                );
            };
            let cidr = rule.value.as_deref().unwrap_or("");
            let matched = cidr
                .parse::<IpNet>()
                .map(|net| net.contains(&ip))
                .unwrap_or(false);
            (
                matched,
                format!(
                    "{} {} {} against ip {}",
                    if matched { "matched" } else { "skipped" },
                    rule.rule_type,
                    cidr,
                    ip
                ),
            )
        }
        "FINAL" => (
            true,
            format!("matched FINAL fallback policy {}", rule.policy),
        ),
        _ => (
            false,
            format!(
                "skipped unsupported rule type {} at line {}",
                rule.rule_type, rule.source.line
            ),
        ),
    }
}

fn explain_mitm(document: &ProfileDocument, host: &str) -> String {
    for section in &document.sections {
        if section.kind != ProfileSectionKind::Mitm {
            continue;
        }
        for entry in &section.entries {
            let ProfileEntry::KeyValue(kv) = entry else {
                continue;
            };
            if matches!(kv.key.as_str(), "hostname" | "hostnames") {
                return format!(
                    "MITM hostname scope is present at line {}; dry-run only, review whether {} is included",
                    kv.source.line, host
                );
            }
        }
    }
    "No MITM hostname scope found in parsed profile".to_string()
}

fn iter_rules(document: &ProfileDocument) -> impl Iterator<Item = &RuleNode> {
    document
        .sections
        .iter()
        .filter(|section| section.kind == ProfileSectionKind::Rule)
        .flat_map(|section| section.entries.iter())
        .filter_map(|entry| match entry {
            ProfileEntry::Rule(rule) => Some(rule),
            _ => None,
        })
}

fn iter_policy_groups(document: &ProfileDocument) -> impl Iterator<Item = &PolicyGroupNode> {
    document
        .sections
        .iter()
        .filter(|section| section.kind == ProfileSectionKind::ProxyGroup)
        .flat_map(|section| section.entries.iter())
        .filter_map(|entry| match entry {
            ProfileEntry::PolicyGroup(group) => Some(group),
            _ => None,
        })
}

fn source_line(raw_line: &str, line: usize) -> SourceLine {
    let (content, comment) = split_inline_comment(raw_line);
    let column = raw_line
        .char_indices()
        .find(|(_, ch)| !ch.is_whitespace())
        .map(|(idx, _)| idx + 1)
        .unwrap_or(1);
    SourceLine {
        line,
        column,
        raw: raw_line.to_string(),
        content: content.trim().to_string(),
        comment,
    }
}

fn is_section_header(trimmed: &str) -> bool {
    trimmed.starts_with('[') && trimmed.ends_with(']') && trimmed.len() > 2
}

fn section_kind(name: &str) -> ProfileSectionKind {
    match name.to_ascii_lowercase().as_str() {
        "general" => ProfileSectionKind::General,
        "proxy" => ProfileSectionKind::Proxy,
        "proxy group" => ProfileSectionKind::ProxyGroup,
        "rule" => ProfileSectionKind::Rule,
        "dns" => ProfileSectionKind::Dns,
        "host" => ProfileSectionKind::Host,
        "mitm" => ProfileSectionKind::Mitm,
        "url rewrite" => ProfileSectionKind::UrlRewrite,
        "map local" => ProfileSectionKind::MapLocal,
        "header rewrite" => ProfileSectionKind::HeaderRewrite,
        "script" => ProfileSectionKind::Script,
        "module" => ProfileSectionKind::Module,
        _ => ProfileSectionKind::Unknown,
    }
}

fn is_comment(trimmed: &str) -> bool {
    (trimmed.starts_with('#') && !trimmed.starts_with("#!"))
        || trimmed.starts_with(';')
        || trimmed.starts_with("//")
}

fn split_inline_comment(raw: &str) -> (String, Option<String>) {
    if raw.trim_start().starts_with("#!") {
        return (raw.to_string(), None);
    }
    let bytes = raw.as_bytes();
    let mut idx = 0;
    while idx < bytes.len() {
        let is_delim = match bytes[idx] {
            b'#' | b';' => true,
            b'/' if idx + 1 < bytes.len() && bytes[idx + 1] == b'/' => true,
            _ => false,
        };
        let has_leading_space = idx == 0 || bytes[idx - 1].is_ascii_whitespace();
        if is_delim && has_leading_space {
            return (raw[..idx].to_string(), Some(raw[idx..].trim().to_string()));
        }
        idx += 1;
    }
    (raw.to_string(), None)
}

fn split_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[General]
dns-server = 8.8.8.8 // inline comment

[Proxy]
ProxyA = http, 127.0.0.1, 8080

[Proxy Group]
Proxy = select, ProxyA, DIRECT
Auto = url-test, ProxyA, DIRECT

[MITM]
hostname = *.example.com

[Rule]
DOMAIN,exact.example.com,DIRECT
DOMAIN-SUFFIX,example.com,Proxy
DOMAIN-KEYWORD,google,DIRECT
IP-CIDR,192.168.0.0/16,DIRECT
GEOIP,US,DIRECT
FINAL,ProxyA
"#;

    #[test]
    fn parse_surge_sections_and_preserve_line_numbers() {
        let doc = parse_surge_profile(SAMPLE, ProfileSource::Inline);
        assert_eq!(doc.sections.len(), 5);
        let rule_section = doc
            .sections
            .iter()
            .find(|section| section.kind == ProfileSectionKind::Rule)
            .unwrap();
        assert_eq!(rule_section.line, 15);
        let first_rule = match &rule_section.entries[0] {
            ProfileEntry::Rule(rule) => rule,
            other => panic!("expected rule, got {other:?}"),
        };
        assert_eq!(first_rule.source.line, 16);
        assert_eq!(first_rule.rule_type, "DOMAIN");
        assert_eq!(first_rule.value.as_deref(), Some("exact.example.com"));
        assert_eq!(first_rule.policy, "DIRECT");
    }

    #[test]
    fn parse_directives_before_comment_handling() {
        let doc = parse_surge_profile(
            "[Rule]\n#!include rules.dconf\nFINAL,DIRECT\n",
            ProfileSource::Inline,
        );
        let entry = &doc.sections[0].entries[0];
        match entry {
            ProfileEntry::Directive(directive) => {
                assert_eq!(directive.directive, "INCLUDE");
                assert_eq!(directive.arguments, "rules.dconf");
            }
            other => panic!("expected directive, got {other:?}"),
        }
    }

    #[test]
    fn compatibility_report_classifies_supported_matrix() {
        let doc = parse_surge_profile(SAMPLE, ProfileSource::Inline);
        let report = analyze_compatibility(&doc);
        assert!(report.summary.fully_supported >= 4);
        assert!(report.summary.translated_with_behavior_note >= 2);
        assert!(report.summary.needs_manual_review >= 1);
        assert!(report.summary.not_supported_yet >= 2);
    }

    #[test]
    fn explain_uses_ordered_first_match() {
        let doc = parse_surge_profile(SAMPLE, ProfileSource::Inline);
        let report = explain_surge_request(&doc, "https://sub.example.com/path").unwrap();
        let matched = report.matched_rule.unwrap();
        assert_eq!(matched.rule_type, "DOMAIN-SUFFIX");
        assert_eq!(matched.policy, "Proxy");
        assert!(report
            .timeline
            .iter()
            .any(|step| step.message.contains("Selected policy Proxy")));
    }

    #[test]
    fn explain_uses_final_fallback() {
        let doc = parse_surge_profile(SAMPLE, ProfileSource::Inline);
        let report = explain_surge_request(&doc, "https://unknown.test").unwrap();
        let matched = report.matched_rule.unwrap();
        assert_eq!(matched.rule_type, "FINAL");
        assert_eq!(matched.policy, "ProxyA");
    }

    #[test]
    fn conversion_preview_keeps_behavior_notes() {
        let doc = parse_surge_profile(SAMPLE, ProfileSource::Inline);
        let preview = convert_surge_to_bifrost_preview(&doc);
        assert!(preview.content.contains("[policies]"));
        assert!(preview.content.contains("host suffix example.com -> Proxy"));
        assert!(preview.content.contains("behavior note: host-only keyword"));
    }
}

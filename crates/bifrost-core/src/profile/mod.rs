use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ipnet::IpNet;
use regex::Regex;
use reqwest::header::{ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

use crate::{direct_blocking_reqwest_client_builder, format_reqwest_error, BifrostError, Result};

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
    pub dns_decision: DnsDecisionTrace,
    pub policy_decision: Option<PolicyDecisionTrace>,
    pub mitm_decision: MitmDecisionTrace,
    pub http_pipeline: Vec<HttpPipelineTrace>,
    pub timeline: Vec<ExplainStep>,
    pub diagnostics: Vec<ProfileDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsDecisionTrace {
    pub matched_host_mapping: Option<String>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyDecisionTrace {
    pub requested_policy: String,
    pub terminal_policy: String,
    pub chain: Vec<String>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MitmDecisionTrace {
    pub included: bool,
    pub excluded: bool,
    pub matched_patterns: Vec<String>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpPipelineTrace {
    pub section: String,
    pub line: usize,
    pub matched: bool,
    pub action: String,
    pub reason: String,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedProfileDocument {
    pub document: ProfileDocument,
    pub resources: Vec<ProfileResource>,
    pub runtime_plan: ProfileRuntimePlan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileResource {
    pub kind: ProfileResourceKind,
    pub reference: String,
    pub source_line: usize,
    pub status: ProfileResourceStatus,
    pub cache_key: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub loaded_from_cache: bool,
    pub item_count: usize,
    pub diagnostics: Vec<ProfileDiagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProfileResourceKind {
    Include,
    RuleSet,
    DomainSet,
    ManagedProfile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProfileResourceStatus {
    Loaded,
    CacheHit,
    Missing,
    FetchFailed,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileRuntimePlan {
    pub mode: String,
    pub proxies: Vec<RuntimeProxy>,
    pub rules: Vec<RuntimeRule>,
    pub policy_groups: Vec<RuntimePolicyGroup>,
    pub dns: Vec<RuntimeKeyValue>,
    pub mitm: Vec<RuntimeKeyValue>,
    pub http_pipeline: Vec<RuntimeKeyValue>,
    pub diagnostics: Vec<ProfileDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeProxy {
    pub source: SourceLine,
    pub name: String,
    pub protocol: String,
    pub fields: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeRule {
    pub source: SourceLine,
    pub rule_type: String,
    pub value: Option<String>,
    pub policy: String,
    pub parameters: Vec<String>,
    pub origin: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimePolicyGroup {
    pub source: SourceLine,
    pub name: String,
    pub group_type: String,
    pub policies: Vec<String>,
    pub parameters: BTreeMap<String, String>,
    pub missing_members: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeKeyValue {
    pub section: String,
    pub key: String,
    pub value: String,
    pub source: SourceLine,
}

pub fn parse_surge_profile_file(path: &Path) -> Result<ProfileDocument> {
    let text = std::fs::read_to_string(path)?;
    Ok(parse_surge_profile(
        &text,
        ProfileSource::LocalPath(path.to_path_buf()),
    ))
}

pub fn load_surge_profile_file(path: &Path) -> Result<ResolvedProfileDocument> {
    let document = parse_surge_profile_file(path)?;
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    Ok(resolve_surge_profile(document, base_dir))
}

pub fn load_surge_profile_path_or_url(path_or_url: &Path) -> Result<ResolvedProfileDocument> {
    let reference = path_or_url.to_string_lossy();
    if is_remote_reference(reference.as_ref()) {
        load_surge_profile_url(reference.as_ref())
    } else {
        load_surge_profile_file(path_or_url)
    }
}

pub fn load_surge_profile_url(url: &str) -> Result<ResolvedProfileDocument> {
    let mut resolver = ProfileResolver::new(Path::new("."));
    let remote = match resolver.fetch_remote_text(ProfileResourceKind::ManagedProfile, url, 1) {
        Ok(remote) => remote,
        Err(message) => resolver
            .read_cached_remote_text(ProfileResourceKind::ManagedProfile, url, 1, Some(message))
            .ok_or_else(|| {
                BifrostError::Network(format!(
                    "fetch managed profile {url} and no cached copy is available"
                ))
            })?,
    };
    let document = parse_surge_profile(&remote.text, ProfileSource::ManagedUrl(url.to_string()));
    let mut runtime_plan = resolver.build_runtime_plan(&document);
    runtime_plan
        .diagnostics
        .extend(document.diagnostics.iter().cloned());
    let mut resources = resolver.resources;
    resources.insert(
        0,
        ProfileResource {
            kind: ProfileResourceKind::ManagedProfile,
            reference: url.to_string(),
            source_line: 1,
            status: remote.status,
            cache_key: Some(remote.cache_key),
            etag: remote.etag,
            last_modified: remote.last_modified,
            loaded_from_cache: remote.loaded_from_cache,
            item_count: runtime_plan.rules.len(),
            diagnostics: Vec::new(),
        },
    );
    Ok(ResolvedProfileDocument {
        document,
        resources,
        runtime_plan,
    })
}

pub fn resolve_surge_profile(
    document: ProfileDocument,
    base_dir: &Path,
) -> ResolvedProfileDocument {
    let mut resolver = ProfileResolver::new(base_dir);
    let mut runtime_plan = resolver.build_runtime_plan(&document);
    runtime_plan
        .diagnostics
        .extend(document.diagnostics.iter().cloned());
    ResolvedProfileDocument {
        document,
        resources: resolver.resources,
        runtime_plan,
    }
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
            let source = source_line(raw_line, line_no);
            if let Some(directive) = parse_directive(&source) {
                let mut section = ProfileSection {
                    name: "Directives".to_string(),
                    kind: ProfileSectionKind::Unknown,
                    line: line_no,
                    entries: Vec::new(),
                };
                section.entries.push(ProfileEntry::Directive(directive));
                document.sections.push(section);
                continue;
            }
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
    let dns_decision = legacy_dns_decision();
    let mitm_decision = legacy_mitm_decision(document, &request.host);
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
                dns_decision,
                policy_decision: None,
                mitm_decision,
                http_pipeline: Vec::new(),
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
        dns_decision,
        policy_decision: None,
        mitm_decision,
        http_pipeline: Vec::new(),
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

pub fn convert_resolved_surge_to_bifrost_preview(
    resolved: &ResolvedProfileDocument,
) -> ConversionPreview {
    let report = analyze_compatibility(&resolved.document);
    let mut content = String::new();
    content.push_str("# Bifrost Native Profile Preview\n");
    content.push_str("# Generated from a resolved Surge profile in dry-run mode.\n");
    content.push_str("# Includes local include/rule-set/domain-set expansion where available.\n\n");

    content.push_str("[resources]\n");
    for resource in &resolved.resources {
        content.push_str(&format!(
            "# line {}: {:?} {} -> {:?} ({} items)\n",
            resource.source_line,
            resource.kind,
            resource.reference,
            resource.status,
            resource.item_count
        ));
    }

    content.push_str("\n[policies]\n");
    for proxy in &resolved.runtime_plan.proxies {
        content.push_str(&format!(
            "{} = proxy, {}, {}\n",
            proxy.name,
            proxy.protocol,
            proxy.fields.join(", ")
        ));
    }
    for group in &resolved.runtime_plan.policy_groups {
        match group.group_type.as_str() {
            "select" | "fallback" | "url-test" => {
                content.push_str(&format!(
                    "{} = {}, {}\n",
                    group.name,
                    group.group_type,
                    group.policies.join(", ")
                ));
                if !group.missing_members.is_empty() {
                    content.push_str(&format!(
                        "#   missing members: {}\n",
                        group.missing_members.join(", ")
                    ));
                }
            }
            _ => {
                content.push_str(&format!(
                    "# line {}: {} = {}, {}  # unsupported policy group type\n",
                    group.source.line,
                    group.name,
                    group.group_type,
                    group.policies.join(", ")
                ));
            }
        }
    }

    content.push_str("\n[dns]\n");
    for item in &resolved.runtime_plan.dns {
        content.push_str(&format!("{} = {}\n", item.key, item.value));
    }

    content.push_str("\n[mitm]\n");
    for item in &resolved.runtime_plan.mitm {
        content.push_str(&format!("{} = {}\n", item.key, item.value));
    }

    content.push_str("\n[http_pipeline]\n");
    for item in &resolved.runtime_plan.http_pipeline {
        content.push_str(&format!(
            "# line {} [{}] {} = {}\n",
            item.source.line, item.section, item.key, item.value
        ));
    }

    content.push_str("\n[rules]\n");
    for rule in &resolved.runtime_plan.rules {
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
                "# line {}: {}  # not supported by resolved Bridge preview",
                rule.source.line, rule.source.content
            ),
        };
        content.push_str(&line);
        content.push_str(&format!(" # origin: {}\n", rule.origin));
    }

    ConversionPreview {
        format: "bifrost-native-profile-preview".to_string(),
        content,
        report,
    }
}

pub fn compile_resolved_surge_to_bifrost_rules(
    resolved: &ResolvedProfileDocument,
) -> ConversionPreview {
    let report = analyze_compatibility(&resolved.document);
    let mut content = String::new();
    content.push_str("# Generated from a resolved Surge profile.\n");
    content.push_str("# Saved disabled by default; review the behavior notes before enabling.\n");
    content.push_str("# Surge ordered first-match rules are emitted in source order.\n\n");

    for resource in &resolved.resources {
        content.push_str(&format!(
            "# resource line {}: {:?} {} -> {:?} ({} items)\n",
            resource.source_line,
            resource.kind,
            resource.reference,
            resource.status,
            resource.item_count
        ));
    }
    if !resolved.resources.is_empty() {
        content.push('\n');
    }

    for rule in &resolved.runtime_plan.rules {
        let Some(patterns) = bifrost_patterns_for_surge_rule(rule) else {
            content.push_str(&format!(
                "# line {}: unsupported Surge rule {}\n",
                rule.source.line, rule.source.content
            ));
            continue;
        };
        let decision =
            resolve_policy_decision(&resolved.runtime_plan, &rule.policy, rule.source.line);
        let Some(operation) =
            bifrost_operation_for_policy(&resolved.runtime_plan, &decision.trace.terminal_policy)
        else {
            content.push_str(&format!(
                "# line {}: {} -> {} cannot be activated yet ({})\n",
                rule.source.line,
                rule.source.content,
                decision.trace.terminal_policy,
                decision.trace.reason
            ));
            continue;
        };
        if decision.trace.chain.len() > 1 {
            content.push_str(&format!(
                "# line {} policy chain: {}\n",
                rule.source.line,
                decision.trace.chain.join(" -> ")
            ));
        }
        for pattern in patterns {
            content.push_str(&format!(
                "{} {} # surge line {}, origin {}\n",
                pattern, operation, rule.source.line, rule.origin
            ));
        }
    }

    ConversionPreview {
        format: "bifrost-rule-file".to_string(),
        content,
        report,
    }
}

pub fn explain_surge_request_with_plan(
    plan: &ProfileRuntimePlan,
    input: &str,
) -> Result<ExplainReport> {
    let request = parse_explain_request(input)?;
    let (dns_decision, dns_steps) = explain_dns_from_plan(plan, &request);
    let mut timeline = vec![ExplainStep {
        stage: "input".to_string(),
        line: None,
        message: format!("URL={} host={}", request.url, request.host),
    }];
    timeline.extend(dns_steps);

    let mut diagnostics = plan.diagnostics.clone();
    for runtime_rule in &plan.rules {
        let rule = RuleNode {
            source: runtime_rule.source.clone(),
            rule_type: runtime_rule.rule_type.clone(),
            value: runtime_rule.value.clone(),
            policy: runtime_rule.policy.clone(),
            parameters: runtime_rule.parameters.clone(),
        };
        let (matched, reason) = rule_matches_request(&rule, &request);
        timeline.push(ExplainStep {
            stage: "rule".to_string(),
            line: Some(rule.source.line),
            message: format!("{reason} (origin: {})", runtime_rule.origin),
        });
        if matched {
            timeline.push(ExplainStep {
                stage: "policy".to_string(),
                line: Some(rule.source.line),
                message: format!("Selected policy {}", rule.policy),
            });
            let policy_decision = resolve_policy_decision(plan, &rule.policy, rule.source.line);
            timeline.extend(policy_decision.steps);
            diagnostics.extend(policy_decision.diagnostics);
            let (mitm_decision, mitm_steps) = explain_mitm_from_plan(plan, &request.host);
            timeline.extend(mitm_steps);
            let (http_pipeline, pipeline_steps) = explain_http_pipeline_from_plan(plan, &request);
            timeline.extend(pipeline_steps);
            return Ok(ExplainReport {
                request,
                matched_rule: Some(rule),
                target_policy: Some(runtime_rule.policy.clone()),
                dns_decision,
                policy_decision: Some(policy_decision.trace),
                mitm_decision,
                http_pipeline,
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
        message: "No resolved Surge rule matched this request; profiles should end with FINAL"
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
        dns_decision,
        policy_decision: None,
        mitm_decision: MitmDecisionTrace {
            included: false,
            excluded: false,
            matched_patterns: Vec::new(),
            reason: "MITM is not evaluated because no rule matched".to_string(),
        },
        http_pipeline: Vec::new(),
        timeline,
        diagnostics,
    })
}

struct PolicyDecisionResolution {
    trace: PolicyDecisionTrace,
    steps: Vec<ExplainStep>,
    diagnostics: Vec<ProfileDiagnostic>,
}

fn resolve_policy_decision(
    plan: &ProfileRuntimePlan,
    policy: &str,
    source_line: usize,
) -> PolicyDecisionResolution {
    let mut resolver = PolicyDecisionResolver {
        plan,
        steps: Vec::new(),
        diagnostics: Vec::new(),
        seen: BTreeSet::new(),
    };
    let (terminal_policy, reason, chain) = resolver.resolve(policy, source_line);
    PolicyDecisionResolution {
        trace: PolicyDecisionTrace {
            requested_policy: policy.to_string(),
            terminal_policy,
            chain,
            reason,
        },
        steps: resolver.steps,
        diagnostics: resolver.diagnostics,
    }
}

fn bifrost_patterns_for_surge_rule(rule: &RuntimeRule) -> Option<Vec<String>> {
    let value = rule.value.as_deref().unwrap_or("").trim();
    match rule.rule_type.as_str() {
        "DOMAIN" => non_empty(value).map(|value| vec![value.to_string()]),
        "DOMAIN-SUFFIX" => non_empty(value).map(|value| {
            let suffix = value.trim_start_matches('.');
            vec![suffix.to_string(), format!("*.{suffix}")]
        }),
        "DOMAIN-KEYWORD" => {
            non_empty(value).map(|value| vec![format!("/.*{}.*/", regex::escape(value))])
        }
        "IP-CIDR" | "IP-CIDR6" => non_empty(value).map(|value| vec![value.to_string()]),
        "FINAL" => Some(vec!["/.*/".to_string()]),
        _ => None,
    }
}

fn bifrost_operation_for_policy(plan: &ProfileRuntimePlan, policy: &str) -> Option<String> {
    if policy.eq_ignore_ascii_case("DIRECT") {
        return Some("passthrough://".to_string());
    }
    if policy.eq_ignore_ascii_case("REJECT")
        || policy.eq_ignore_ascii_case("REJECT-TINYGIF")
        || policy.eq_ignore_ascii_case("REJECT-DROP")
    {
        return Some("statusCode://403".to_string());
    }
    let proxy = plan.proxies.iter().find(|proxy| proxy.name == policy)?;
    bifrost_proxy_operation(proxy)
}

fn bifrost_proxy_operation(proxy: &RuntimeProxy) -> Option<String> {
    let host = proxy.fields.first()?.trim();
    let port = proxy.fields.get(1)?.trim();
    if host.is_empty() || port.is_empty() {
        return None;
    }
    let scheme = match proxy.protocol.as_str() {
        "http" | "https" | "socks5" | "socks" => proxy.protocol.as_str(),
        _ => "http",
    };
    Some(format!("proxy://{scheme}://{host}:{port}"))
}

fn non_empty(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

struct PolicyDecisionResolver<'a> {
    plan: &'a ProfileRuntimePlan,
    steps: Vec<ExplainStep>,
    diagnostics: Vec<ProfileDiagnostic>,
    seen: BTreeSet<String>,
}

impl<'a> PolicyDecisionResolver<'a> {
    fn resolve(&mut self, policy: &str, source_line: usize) -> (String, String, Vec<String>) {
        let normalized = policy.trim();
        if normalized.is_empty() {
            self.push_diagnostic(
                source_line,
                "surge.policy.empty",
                "Matched rule selected an empty policy",
                "Review the rule target policy",
            );
            return (
                normalized.to_string(),
                "empty policy target".to_string(),
                vec![normalized.to_string()],
            );
        }

        if is_builtin_policy(normalized) {
            self.steps.push(ExplainStep {
                stage: "policy".to_string(),
                line: Some(source_line),
                message: format!("Terminal built-in policy {normalized}"),
            });
            return (
                normalized.to_string(),
                "built-in policy".to_string(),
                vec![normalized.to_string()],
            );
        }

        if self
            .plan
            .proxies
            .iter()
            .any(|proxy| proxy.name == normalized)
        {
            self.steps.push(ExplainStep {
                stage: "policy".to_string(),
                line: Some(source_line),
                message: format!("Terminal proxy policy {normalized}"),
            });
            return (
                normalized.to_string(),
                "proxy endpoint".to_string(),
                vec![normalized.to_string()],
            );
        }

        let Some(group) = self
            .plan
            .policy_groups
            .iter()
            .find(|group| group.name == normalized)
        else {
            self.push_diagnostic(
                source_line,
                "surge.policy.missing",
                &format!("Selected policy {normalized} does not match a proxy, built-in policy, or policy group"),
                "Inspect the effective profile for missing policy group members",
            );
            self.steps.push(ExplainStep {
                stage: "policy".to_string(),
                line: Some(source_line),
                message: format!("Missing terminal policy {normalized}"),
            });
            return (
                normalized.to_string(),
                "missing policy target".to_string(),
                vec![normalized.to_string()],
            );
        };

        if !self.seen.insert(group.name.clone()) {
            self.push_diagnostic(
                group.source.line,
                "surge.policy.cycle",
                &format!("Policy group cycle detected at {}", group.name),
                "Break the policy group cycle before activating this profile",
            );
            self.steps.push(ExplainStep {
                stage: "policy-group".to_string(),
                line: Some(group.source.line),
                message: format!("Stopped policy group cycle at {}", group.name),
            });
            return (
                group.name.clone(),
                "policy group cycle".to_string(),
                vec![group.name.clone()],
            );
        }

        let Some(candidate) = self.select_group_candidate(group) else {
            self.push_diagnostic(
                group.source.line,
                "surge.policy_group.empty",
                &format!("Policy group {} has no candidate policies", group.name),
                "Add at least one policy member to the group",
            );
            self.steps.push(ExplainStep {
                stage: "policy-group".to_string(),
                line: Some(group.source.line),
                message: format!("Policy group {} has no candidates", group.name),
            });
            return (
                group.name.clone(),
                "empty policy group".to_string(),
                vec![group.name.clone()],
            );
        };

        self.steps.push(ExplainStep {
            stage: "policy-group".to_string(),
            line: Some(group.source.line),
            message: format!(
                "{} group {} selected {} ({})",
                group.group_type,
                group.name,
                candidate,
                group_selection_reason(group)
            ),
        });
        let (terminal, reason, mut chain) = self.resolve(&candidate, group.source.line);
        let mut full_chain = vec![group.name.clone()];
        full_chain.append(&mut chain);
        (terminal, reason, full_chain)
    }

    fn select_group_candidate(&self, group: &RuntimePolicyGroup) -> Option<String> {
        if let Some(selected) = group.parameters.get("selected") {
            if group.policies.iter().any(|policy| policy == selected) {
                return Some(selected.clone());
            }
        }
        group.policies.first().cloned()
    }

    fn push_diagnostic(&mut self, line: usize, code: &str, message: &str, suggestion: &str) {
        self.diagnostics.push(ProfileDiagnostic {
            severity: DiagnosticSeverity::Warning,
            line,
            column: 1,
            code: code.to_string(),
            message: message.to_string(),
            suggestion: Some(suggestion.to_string()),
        });
    }
}

struct ProfileResolver {
    base_dir: PathBuf,
    cache_dir: PathBuf,
    resources: Vec<ProfileResource>,
    diagnostics: Vec<ProfileDiagnostic>,
}

impl ProfileResolver {
    fn new(base_dir: &Path) -> Self {
        Self {
            base_dir: base_dir.to_path_buf(),
            cache_dir: profile_cache_dir(base_dir),
            resources: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn build_runtime_plan(&mut self, document: &ProfileDocument) -> ProfileRuntimePlan {
        let mut plan = ProfileRuntimePlan {
            mode: "surge-compatible-dry-run".to_string(),
            proxies: Vec::new(),
            rules: Vec::new(),
            policy_groups: Vec::new(),
            dns: Vec::new(),
            mitm: Vec::new(),
            http_pipeline: Vec::new(),
            diagnostics: Vec::new(),
        };

        self.apply_document(document, "root", &mut plan);
        self.validate_policy_groups(&mut plan);
        plan.diagnostics.extend(self.diagnostics.iter().cloned());
        plan
    }

    fn apply_document(
        &mut self,
        document: &ProfileDocument,
        origin: &str,
        plan: &mut ProfileRuntimePlan,
    ) {
        for section in &document.sections {
            for entry in &section.entries {
                match entry {
                    ProfileEntry::Directive(directive) => {
                        self.apply_directive(directive, plan);
                    }
                    ProfileEntry::Rule(rule) => self.apply_rule(rule, origin, plan),
                    ProfileEntry::Proxy(proxy) => {
                        plan.proxies.push(RuntimeProxy {
                            source: proxy.source.clone(),
                            name: proxy.name.clone(),
                            protocol: proxy.protocol.clone(),
                            fields: proxy.fields.clone(),
                        });
                    }
                    ProfileEntry::PolicyGroup(group) => {
                        plan.policy_groups.push(RuntimePolicyGroup {
                            source: group.source.clone(),
                            name: group.name.clone(),
                            group_type: group.group_type.clone(),
                            policies: group.policies.clone(),
                            parameters: group.parameters.clone(),
                            missing_members: Vec::new(),
                        });
                    }
                    ProfileEntry::KeyValue(kv) => self.apply_key_value(section, kv, plan),
                    ProfileEntry::Raw(raw) => self.apply_raw(section, raw, plan),
                    _ => {}
                }
            }
        }
    }

    fn apply_directive(&mut self, directive: &DirectiveNode, plan: &mut ProfileRuntimePlan) {
        match directive.directive.as_str() {
            "INCLUDE" => self.load_include(directive, plan),
            "MANAGED-CONFIG" => self.load_managed_profile(directive, plan),
            _ => {}
        }
    }

    fn apply_rule(&mut self, rule: &RuleNode, origin: &str, plan: &mut ProfileRuntimePlan) {
        match rule.rule_type.as_str() {
            "RULE-SET" => self.load_rule_set(rule, plan),
            "DOMAIN-SET" => self.load_domain_set(rule, plan),
            _ => plan.rules.push(RuntimeRule {
                source: rule.source.clone(),
                rule_type: rule.rule_type.clone(),
                value: rule.value.clone(),
                policy: rule.policy.clone(),
                parameters: rule.parameters.clone(),
                origin: origin.to_string(),
            }),
        }
    }

    fn apply_key_value(
        &mut self,
        section: &ProfileSection,
        kv: &KeyValueEntry,
        plan: &mut ProfileRuntimePlan,
    ) {
        let item = RuntimeKeyValue {
            section: section.name.clone(),
            key: kv.key.clone(),
            value: kv.value.clone(),
            source: kv.source.clone(),
        };
        match section.kind {
            ProfileSectionKind::General | ProfileSectionKind::Dns | ProfileSectionKind::Host => {
                plan.dns.push(item)
            }
            ProfileSectionKind::Mitm => plan.mitm.push(item),
            ProfileSectionKind::UrlRewrite
            | ProfileSectionKind::MapLocal
            | ProfileSectionKind::HeaderRewrite
            | ProfileSectionKind::Script => plan.http_pipeline.push(item),
            _ => {}
        }
    }

    fn apply_raw(
        &mut self,
        section: &ProfileSection,
        raw: &SourceLine,
        plan: &mut ProfileRuntimePlan,
    ) {
        if !matches!(
            section.kind,
            ProfileSectionKind::UrlRewrite
                | ProfileSectionKind::MapLocal
                | ProfileSectionKind::HeaderRewrite
                | ProfileSectionKind::Script
        ) {
            return;
        }
        let content = raw.content.trim();
        if content.is_empty() {
            return;
        }
        plan.http_pipeline.push(RuntimeKeyValue {
            section: section.name.clone(),
            key: "raw".to_string(),
            value: content.to_string(),
            source: raw.clone(),
        });
    }

    fn load_include(&mut self, directive: &DirectiveNode, plan: &mut ProfileRuntimePlan) {
        let reference = directive.arguments.trim();
        if reference.is_empty() {
            self.record_missing_resource(
                ProfileResourceKind::Include,
                reference.to_string(),
                directive.source.line,
                "include directive has no target",
            );
            return;
        }
        let Some(resource) = self.read_resource_text(
            ProfileResourceKind::Include,
            reference,
            directive.source.line,
        ) else {
            return;
        };
        let source = if is_remote_reference(reference) {
            ProfileSource::ManagedUrl(reference.to_string())
        } else {
            ProfileSource::LocalPath(self.resolve_path(reference))
        };
        let included = parse_surge_profile(&resource.text, source);
        let before_rules = plan.rules.len();
        self.apply_document(&included, &format!("include:{}", reference), plan);
        let item_count = plan.rules.len().saturating_sub(before_rules);
        self.resources.push(ProfileResource {
            kind: ProfileResourceKind::Include,
            reference: reference.to_string(),
            source_line: directive.source.line,
            status: resource.status,
            cache_key: Some(resource.cache_key),
            etag: resource.etag,
            last_modified: resource.last_modified,
            loaded_from_cache: resource.loaded_from_cache,
            item_count,
            diagnostics: included.diagnostics,
        });
    }

    fn load_managed_profile(&mut self, directive: &DirectiveNode, plan: &mut ProfileRuntimePlan) {
        let reference = directive.arguments.trim();
        if reference.is_empty() {
            self.record_missing_resource(
                ProfileResourceKind::ManagedProfile,
                reference.to_string(),
                directive.source.line,
                "managed profile directive has no URL",
            );
            return;
        }
        let Some(resource) = self.read_resource_text(
            ProfileResourceKind::ManagedProfile,
            reference,
            directive.source.line,
        ) else {
            return;
        };
        let managed = parse_surge_profile(
            &resource.text,
            if is_remote_reference(reference) {
                ProfileSource::ManagedUrl(reference.to_string())
            } else {
                ProfileSource::LocalPath(self.resolve_path(reference))
            },
        );
        let before_rules = plan.rules.len();
        self.apply_document(&managed, &format!("managed:{}", reference), plan);
        let item_count = plan.rules.len().saturating_sub(before_rules);
        self.resources.push(ProfileResource {
            kind: ProfileResourceKind::ManagedProfile,
            reference: reference.to_string(),
            source_line: directive.source.line,
            status: resource.status,
            cache_key: Some(resource.cache_key),
            etag: resource.etag,
            last_modified: resource.last_modified,
            loaded_from_cache: resource.loaded_from_cache,
            item_count,
            diagnostics: managed.diagnostics,
        });
    }

    fn load_rule_set(&mut self, rule: &RuleNode, plan: &mut ProfileRuntimePlan) {
        let reference = rule.value.as_deref().unwrap_or("").trim();
        let Some(resource) =
            self.read_resource_text(ProfileResourceKind::RuleSet, reference, rule.source.line)
        else {
            return;
        };
        let mut count = 0;
        for (index, raw_line) in resource.text.lines().enumerate() {
            let source = source_line(raw_line, index + 1);
            if source.content.trim().is_empty() || is_comment(source.content.trim()) {
                continue;
            }
            let fields = split_csv(&source.content);
            if fields.len() < 2 {
                self.diagnostics.push(ProfileDiagnostic {
                    severity: DiagnosticSeverity::Warning,
                    line: rule.source.line,
                    column: 1,
                    code: "surge.ruleset.invalid_line".to_string(),
                    message: format!(
                        "RULE-SET {} contains an invalid line: {}",
                        reference, source.content
                    ),
                    suggestion: Some("Use lines such as DOMAIN-SUFFIX,example.com".to_string()),
                });
                continue;
            }
            plan.rules.push(RuntimeRule {
                source: SourceLine {
                    line: rule.source.line,
                    column: rule.source.column,
                    raw: source.raw,
                    content: source.content,
                    comment: source.comment,
                },
                rule_type: fields[0].to_ascii_uppercase(),
                value: Some(fields[1].clone()),
                policy: rule.policy.clone(),
                parameters: fields.into_iter().skip(2).collect(),
                origin: format!("RULE-SET:{reference}:{}", index + 1),
            });
            count += 1;
        }
        self.resources.push(ProfileResource {
            kind: ProfileResourceKind::RuleSet,
            reference: reference.to_string(),
            source_line: rule.source.line,
            status: resource.status,
            cache_key: Some(resource.cache_key),
            etag: resource.etag,
            last_modified: resource.last_modified,
            loaded_from_cache: resource.loaded_from_cache,
            item_count: count,
            diagnostics: Vec::new(),
        });
    }

    fn load_domain_set(&mut self, rule: &RuleNode, plan: &mut ProfileRuntimePlan) {
        let reference = rule.value.as_deref().unwrap_or("").trim();
        let Some(resource) =
            self.read_resource_text(ProfileResourceKind::DomainSet, reference, rule.source.line)
        else {
            return;
        };
        let mut count = 0;
        for (index, raw_line) in resource.text.lines().enumerate() {
            let source = source_line(raw_line, index + 1);
            let domain = source.content.trim().trim_start_matches('.');
            if domain.is_empty() || is_comment(domain) {
                continue;
            }
            plan.rules.push(RuntimeRule {
                source: SourceLine {
                    line: rule.source.line,
                    column: rule.source.column,
                    raw: source.raw,
                    content: domain.to_string(),
                    comment: source.comment,
                },
                rule_type: "DOMAIN-SUFFIX".to_string(),
                value: Some(domain.to_string()),
                policy: rule.policy.clone(),
                parameters: rule.parameters.clone(),
                origin: format!("DOMAIN-SET:{reference}:{}", index + 1),
            });
            count += 1;
        }
        self.resources.push(ProfileResource {
            kind: ProfileResourceKind::DomainSet,
            reference: reference.to_string(),
            source_line: rule.source.line,
            status: resource.status,
            cache_key: Some(resource.cache_key),
            etag: resource.etag,
            last_modified: resource.last_modified,
            loaded_from_cache: resource.loaded_from_cache,
            item_count: count,
            diagnostics: Vec::new(),
        });
    }

    fn read_resource_text(
        &mut self,
        kind: ProfileResourceKind,
        reference: &str,
        source_line: usize,
    ) -> Option<LoadedResourceText> {
        if reference.is_empty() {
            self.record_missing_resource(
                kind,
                reference.to_string(),
                source_line,
                "resource reference is empty",
            );
            return None;
        }
        if is_remote_reference(reference) {
            match self.fetch_remote_text(kind, reference, source_line) {
                Ok(resource) => return Some(resource),
                Err(message) => {
                    if let Some(resource) = self.read_cached_remote_text(
                        kind,
                        reference,
                        source_line,
                        Some(message.clone()),
                    ) {
                        return Some(resource);
                    }
                    self.record_fetch_failed_resource(
                        kind,
                        reference.to_string(),
                        source_line,
                        &message,
                    );
                    return None;
                }
            }
        }
        let path = self.resolve_path(reference);
        match std::fs::read_to_string(&path) {
            Ok(text) => Some(LoadedResourceText {
                text: text.clone(),
                status: ProfileResourceStatus::Loaded,
                cache_key: content_cache_key(kind, reference, &text),
                etag: None,
                last_modified: None,
                loaded_from_cache: false,
            }),
            Err(_) => {
                self.record_missing_resource(
                    kind,
                    reference.to_string(),
                    source_line,
                    "local resource could not be read",
                );
                None
            }
        }
    }

    fn fetch_remote_text(
        &mut self,
        kind: ProfileResourceKind,
        reference: &str,
        source_line: usize,
    ) -> std::result::Result<LoadedResourceText, String> {
        let cache_entry = RemoteCacheEntry::new(&self.cache_dir, kind, reference);
        let cached_metadata = cache_entry.read_metadata();
        let client = direct_blocking_reqwest_client_builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|error| format!("build HTTP client: {error}"))?;
        let mut request = client.get(reference);
        if let Some(metadata) = &cached_metadata {
            if let Some(etag) = &metadata.etag {
                request = request.header(IF_NONE_MATCH, etag);
            }
            if let Some(last_modified) = &metadata.last_modified {
                request = request.header(IF_MODIFIED_SINCE, last_modified);
            }
        }
        let response = request.send().map_err(|error| {
            format!(
                "fetch remote resource {reference}: {}",
                format_reqwest_error(&error)
            )
        })?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_MODIFIED {
            return self
                .read_cached_remote_text(kind, reference, source_line, None)
                .ok_or_else(|| {
                    format!("remote resource {reference} returned 304 but no cached body exists")
                });
        }
        if !status.is_success() {
            return Err(format!(
                "remote resource {reference} returned HTTP {status}"
            ));
        }
        let etag = response
            .headers()
            .get(ETAG)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let last_modified = response
            .headers()
            .get(LAST_MODIFIED)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let text = response.text().map_err(|error| {
            format!(
                "read remote resource {reference}: {}",
                format_reqwest_error(&error)
            )
        })?;
        let cache_key = content_cache_key(kind, reference, &text);
        let metadata = RemoteResourceCacheMetadata {
            kind,
            reference: reference.to_string(),
            cache_key: cache_key.clone(),
            etag: etag.clone(),
            last_modified: last_modified.clone(),
        };
        if let Err(error) = cache_entry.write(&text, &metadata) {
            self.diagnostics.push(ProfileDiagnostic {
                severity: DiagnosticSeverity::Warning,
                line: source_line,
                column: 1,
                code: "surge.resource.cache_write_failed".to_string(),
                message: format!("Remote resource loaded but cache write failed: {error}"),
                suggestion: Some(
                    "Check BIFROST_PROFILE_CACHE_DIR or BIFROST_DATA_DIR permissions".to_string(),
                ),
            });
        }
        Ok(LoadedResourceText {
            text,
            status: ProfileResourceStatus::Loaded,
            cache_key,
            etag,
            last_modified,
            loaded_from_cache: false,
        })
    }

    fn read_cached_remote_text(
        &mut self,
        kind: ProfileResourceKind,
        reference: &str,
        source_line: usize,
        stale_reason: Option<String>,
    ) -> Option<LoadedResourceText> {
        let cache_entry = RemoteCacheEntry::new(&self.cache_dir, kind, reference);
        let metadata = cache_entry.read_metadata()?;
        let text = std::fs::read_to_string(cache_entry.body_path()).ok()?;
        if let Some(reason) = stale_reason {
            self.diagnostics.push(ProfileDiagnostic {
                severity: DiagnosticSeverity::Warning,
                line: source_line,
                column: 1,
                code: "surge.resource.stale_cache_used".to_string(),
                message: format!("Using cached remote resource after fetch failed: {reason}"),
                suggestion: Some(
                    "Refresh the profile when the remote endpoint is reachable".to_string(),
                ),
            });
        }
        Some(LoadedResourceText {
            text,
            status: ProfileResourceStatus::CacheHit,
            cache_key: metadata.cache_key,
            etag: metadata.etag,
            last_modified: metadata.last_modified,
            loaded_from_cache: true,
        })
    }

    fn resolve_path(&self, reference: &str) -> PathBuf {
        let path = Path::new(reference);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.base_dir.join(path)
        }
    }

    fn validate_policy_groups(&mut self, plan: &mut ProfileRuntimePlan) {
        let proxies: std::collections::BTreeSet<String> = plan
            .proxies
            .iter()
            .map(|proxy| proxy.name.clone())
            .chain(plan.policy_groups.iter().map(|group| group.name.clone()))
            .chain([
                "DIRECT".to_string(),
                "REJECT".to_string(),
                "REJECT-TINYGIF".to_string(),
                "REJECT-DROP".to_string(),
            ])
            .collect();
        let group_names: std::collections::BTreeSet<String> = plan
            .policy_groups
            .iter()
            .map(|group| group.name.clone())
            .collect();
        for group in &mut plan.policy_groups {
            group.missing_members = group
                .policies
                .iter()
                .filter(|policy| !proxies.contains(*policy) && !group_names.contains(*policy))
                .cloned()
                .collect();
        }
    }

    fn record_fetch_failed_resource(
        &mut self,
        kind: ProfileResourceKind,
        reference: String,
        source_line: usize,
        message: &str,
    ) {
        let diagnostic = ProfileDiagnostic {
            severity: DiagnosticSeverity::Warning,
            line: source_line,
            column: 1,
            code: "surge.resource.fetch_failed".to_string(),
            message: message.to_string(),
            suggestion: Some("Check the remote URL or use an already cached copy".to_string()),
        };
        self.resources.push(ProfileResource {
            kind,
            reference,
            source_line,
            status: ProfileResourceStatus::FetchFailed,
            cache_key: None,
            etag: None,
            last_modified: None,
            loaded_from_cache: false,
            item_count: 0,
            diagnostics: vec![diagnostic.clone()],
        });
        self.diagnostics.push(diagnostic);
    }

    fn record_missing_resource(
        &mut self,
        kind: ProfileResourceKind,
        reference: String,
        source_line: usize,
        message: &str,
    ) {
        let diagnostic = ProfileDiagnostic {
            severity: DiagnosticSeverity::Warning,
            line: source_line,
            column: 1,
            code: "surge.resource.missing".to_string(),
            message: message.to_string(),
            suggestion: Some("Check the path relative to the imported profile".to_string()),
        };
        self.resources.push(ProfileResource {
            kind,
            reference,
            source_line,
            status: ProfileResourceStatus::Missing,
            cache_key: None,
            etag: None,
            last_modified: None,
            loaded_from_cache: false,
            item_count: 0,
            diagnostics: vec![diagnostic.clone()],
        });
        self.diagnostics.push(diagnostic);
    }
}

#[derive(Debug, Clone)]
struct LoadedResourceText {
    text: String,
    status: ProfileResourceStatus,
    cache_key: String,
    etag: Option<String>,
    last_modified: Option<String>,
    loaded_from_cache: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RemoteResourceCacheMetadata {
    kind: ProfileResourceKind,
    reference: String,
    cache_key: String,
    etag: Option<String>,
    last_modified: Option<String>,
}

#[derive(Debug, Clone)]
struct RemoteCacheEntry {
    body_path: PathBuf,
    metadata_path: PathBuf,
}

impl RemoteCacheEntry {
    fn new(cache_dir: &Path, kind: ProfileResourceKind, reference: &str) -> Self {
        let key = reference_cache_key(kind, reference)
            .trim_start_matches("remote-sha256:")
            .to_string();
        Self {
            body_path: cache_dir.join(format!("{key}.body")),
            metadata_path: cache_dir.join(format!("{key}.json")),
        }
    }

    fn body_path(&self) -> &Path {
        &self.body_path
    }

    fn read_metadata(&self) -> Option<RemoteResourceCacheMetadata> {
        let text = std::fs::read_to_string(&self.metadata_path).ok()?;
        serde_json::from_str(&text).ok()
    }

    fn write(&self, text: &str, metadata: &RemoteResourceCacheMetadata) -> std::io::Result<()> {
        if let Some(parent) = self.body_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.body_path, text)?;
        let metadata_text = serde_json::to_string_pretty(metadata)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        std::fs::write(&self.metadata_path, metadata_text)?;
        Ok(())
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
            "Local and remote set references are expanded in the resolved dry-run runtime plan; remote sets use conditional cache metadata",
            Some("Inspect the effective profile before activation, especially when the reference is remote"),
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
            SupportLevel::TranslatedWithBehaviorNote,
            "select group is parsed into the dry-run policy graph; active runtime switching is still gated",
            Some("Inspect missing members in the effective profile before activation"),
        ),
        "url-test" | "fallback" => (
            SupportLevel::NeedsManualReview,
            "Health-based policy group is represented in the graph but active probing still needs Policy Health Store",
            Some("Keep this group dry-run until active health probing exists"),
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
            "local and remote include directives can be loaded into the resolved dry-run plan",
        ),
        "MANAGED-CONFIG" => (
            SupportLevel::TranslatedWithBehaviorNote,
            "managed profile directive can be fetched and expanded into the dry-run plan",
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

fn is_builtin_policy(policy: &str) -> bool {
    matches!(
        policy.to_ascii_uppercase().as_str(),
        "DIRECT" | "REJECT" | "REJECT-TINYGIF" | "REJECT-DROP"
    )
}

fn group_selection_reason(group: &RuntimePolicyGroup) -> &'static str {
    match group.group_type.as_str() {
        "select" => "dry-run uses selected parameter when present, otherwise the first candidate",
        "fallback" => "dry-run uses the first candidate because Policy Health Store is not active",
        "url-test" => {
            "dry-run uses the first candidate because active latency probing is not running"
        }
        _ => "dry-run uses the first candidate for unknown group type",
    }
}

fn legacy_dns_decision() -> DnsDecisionTrace {
    DnsDecisionTrace {
        matched_host_mapping: None,
        notes: vec![
            "Legacy profile explain does not build the resolved DNS runtime plan".to_string(),
        ],
    }
}

fn explain_dns_from_plan(
    plan: &ProfileRuntimePlan,
    request: &ExplainRequest,
) -> (DnsDecisionTrace, Vec<ExplainStep>) {
    let mut notes = Vec::new();
    let mut steps = Vec::new();
    let mut matched_host_mapping = None;

    for item in &plan.dns {
        let section = item.section.to_ascii_lowercase();
        if section == "host" {
            if host_pattern_matches(&item.key, &request.host) {
                let note = format!("Host mapping {} -> {}", item.key, item.value);
                matched_host_mapping = Some(note.clone());
                notes.push(note.clone());
                steps.push(ExplainStep {
                    stage: "dns".to_string(),
                    line: Some(item.source.line),
                    message: format!(
                        "{note}; dry-run records the mapping but does not rewrite the request IP"
                    ),
                });
            } else {
                steps.push(ExplainStep {
                    stage: "dns".to_string(),
                    line: Some(item.source.line),
                    message: format!(
                        "skipped Host mapping {} for host {}",
                        item.key, request.host
                    ),
                });
            }
            continue;
        }

        if matches!(
            item.key.as_str(),
            "dns-server" | "default-nameserver" | "encrypted-dns-server" | "doh-server"
        ) {
            let note = format!("DNS provider {} = {}", item.key, item.value);
            notes.push(note.clone());
            steps.push(ExplainStep {
                stage: "dns".to_string(),
                line: Some(item.source.line),
                message: format!("{note}; dry-run explain does not perform network DNS resolution"),
            });
        } else {
            steps.push(ExplainStep {
                stage: "dns".to_string(),
                line: Some(item.source.line),
                message: format!(
                    "preserved DNS config [{}] {} = {}",
                    item.section, item.key, item.value
                ),
            });
        }
    }

    if steps.is_empty() {
        notes.push("No DNS or Host entries found in resolved profile".to_string());
        steps.push(ExplainStep {
            stage: "dns".to_string(),
            line: None,
            message: "No DNS or Host runtime entries; using URL host and optional literal IP only"
                .to_string(),
        });
    }

    (
        DnsDecisionTrace {
            matched_host_mapping,
            notes,
        },
        steps,
    )
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

fn legacy_mitm_decision(document: &ProfileDocument, host: &str) -> MitmDecisionTrace {
    MitmDecisionTrace {
        included: false,
        excluded: false,
        matched_patterns: Vec::new(),
        reason: explain_mitm(document, host),
    }
}

fn explain_mitm_from_plan(
    plan: &ProfileRuntimePlan,
    host: &str,
) -> (MitmDecisionTrace, Vec<ExplainStep>) {
    let mut hostname_entries = Vec::new();
    for kv in &plan.mitm {
        if matches!(kv.key.as_str(), "hostname" | "hostnames") {
            hostname_entries.push(kv);
        }
    }
    if hostname_entries.is_empty() {
        return (
            MitmDecisionTrace {
                included: false,
                excluded: false,
                matched_patterns: Vec::new(),
                reason: "No MITM hostname scope found in resolved profile".to_string(),
            },
            vec![ExplainStep {
                stage: "mitm".to_string(),
                line: None,
                message: "No MITM hostname scope found in resolved profile".to_string(),
            }],
        );
    }

    let mut matched_patterns = Vec::new();
    let mut excluded = false;
    let mut steps = Vec::new();
    for kv in hostname_entries {
        for raw_pattern in split_profile_list(&kv.value) {
            let (is_exclusion, pattern) = mitm_pattern(raw_pattern.as_str());
            if host_pattern_matches(pattern, host) {
                matched_patterns.push(raw_pattern.clone());
                if is_exclusion {
                    excluded = true;
                }
                steps.push(ExplainStep {
                    stage: "mitm".to_string(),
                    line: Some(kv.source.line),
                    message: format!(
                        "{} MITM hostname pattern {} for host {}",
                        if is_exclusion {
                            "excluded by"
                        } else {
                            "included by"
                        },
                        raw_pattern,
                        host
                    ),
                });
            } else {
                steps.push(ExplainStep {
                    stage: "mitm".to_string(),
                    line: Some(kv.source.line),
                    message: format!(
                        "skipped MITM hostname pattern {} for host {}",
                        raw_pattern, host
                    ),
                });
            }
        }
    }
    let included = !excluded
        && matched_patterns.iter().any(|pattern| {
            !pattern.trim_start().starts_with('-') && !pattern.trim_start().starts_with('!')
        });
    let reason = if excluded {
        format!("host {host} is excluded from MITM")
    } else if included {
        format!("host {host} is included in MITM dry-run scope")
    } else {
        format!("host {host} is not included in MITM dry-run scope")
    };
    (
        MitmDecisionTrace {
            included,
            excluded,
            matched_patterns,
            reason,
        },
        steps,
    )
}

fn explain_http_pipeline_from_plan(
    plan: &ProfileRuntimePlan,
    request: &ExplainRequest,
) -> (Vec<HttpPipelineTrace>, Vec<ExplainStep>) {
    let mut traces = Vec::new();
    let mut steps = Vec::new();
    for item in &plan.http_pipeline {
        let (matched, action, reason) = pipeline_entry_decision(item, request);
        traces.push(HttpPipelineTrace {
            section: item.section.clone(),
            line: item.source.line,
            matched,
            action: action.clone(),
            reason: reason.clone(),
        });
        steps.push(ExplainStep {
            stage: "http-pipeline".to_string(),
            line: Some(item.source.line),
            message: format!(
                "{} [{}] {} ({})",
                if matched { "matched" } else { "skipped" },
                item.section,
                action,
                reason
            ),
        });
    }
    if steps.is_empty() {
        steps.push(ExplainStep {
            stage: "http-pipeline".to_string(),
            line: None,
            message: "No URL Rewrite, Map Local, Header Rewrite, or Script entries found"
                .to_string(),
        });
    }
    (traces, steps)
}

fn pipeline_entry_decision(
    item: &RuntimeKeyValue,
    request: &ExplainRequest,
) -> (bool, String, String) {
    let section = item.section.to_ascii_lowercase();
    let content = runtime_item_content(item);
    let pattern = pipeline_match_pattern(&section, item);
    let matched = pattern
        .as_deref()
        .map(|pattern| url_pattern_matches(pattern, &request.url))
        .unwrap_or(false);
    let action = if item.key == "raw" {
        content
    } else {
        format!("{} = {}", item.key, item.value)
    };
    let reason = match pattern {
        Some(pattern) if matched => format!("pattern {pattern} matched {}", request.url),
        Some(pattern) => format!("pattern {pattern} did not match {}", request.url),
        None => "no URL pattern could be inferred in dry-run explain".to_string(),
    };
    (matched, action, reason)
}

fn pipeline_match_pattern(section: &str, item: &RuntimeKeyValue) -> Option<String> {
    if section == "script" {
        let content = runtime_item_content(item);
        let fields: Vec<&str> = content.split_whitespace().collect();
        if fields.len() >= 2 && fields[0].starts_with("http-") {
            return Some(fields[1].to_string());
        }
    }
    if item.key != "raw" {
        return Some(item.key.clone());
    }
    runtime_item_content(item)
        .split_whitespace()
        .next()
        .map(str::to_string)
}

fn runtime_item_content(item: &RuntimeKeyValue) -> String {
    if item.key == "raw" {
        item.value.clone()
    } else {
        format!("{} = {}", item.key, item.value)
    }
}

fn url_pattern_matches(pattern: &str, url: &str) -> bool {
    let trimmed = trim_profile_token(pattern);
    if trimmed.is_empty() {
        return false;
    }
    Regex::new(trimmed)
        .map(|regex| regex.is_match(url))
        .unwrap_or_else(|_| url.contains(trimmed))
}

fn split_profile_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(trim_profile_list_item)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

fn mitm_pattern(raw_pattern: &str) -> (bool, &str) {
    let pattern = raw_pattern.trim();
    if let Some(rest) = pattern.strip_prefix('-') {
        return (true, rest.trim());
    }
    if let Some(rest) = pattern.strip_prefix('!') {
        return (true, rest.trim());
    }
    (false, pattern)
}

fn host_pattern_matches(pattern: &str, host: &str) -> bool {
    let pattern = trim_profile_token(pattern).to_ascii_lowercase();
    let host = host.to_ascii_lowercase();
    if pattern == "*" {
        return true;
    }
    if let Some(suffix) = pattern.strip_prefix("*.") {
        return host.ends_with(&format!(".{suffix}"));
    }
    if let Some(suffix) = pattern.strip_prefix('.') {
        return host == suffix || host.ends_with(&format!(".{suffix}"));
    }
    host == pattern
}

fn trim_profile_token(value: &str) -> &str {
    value.trim().trim_matches('"').trim_matches('\'').trim()
}

fn trim_profile_list_item(mut value: &str) -> &str {
    value = trim_profile_token(value);
    loop {
        let trimmed = value.trim_start();
        if !trimmed.starts_with('%') {
            return trim_profile_token(trimmed);
        }
        let Some(end) = trimmed[1..].find('%') else {
            return "";
        };
        value = &trimmed[end + 2..];
    }
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

fn is_remote_reference(reference: &str) -> bool {
    reference.starts_with("http://") || reference.starts_with("https://")
}

fn profile_cache_dir(base_dir: &Path) -> PathBuf {
    if let Some(path) = std::env::var_os("BIFROST_PROFILE_CACHE_DIR") {
        return PathBuf::from(path);
    }
    if let Some(data_dir) = std::env::var_os("BIFROST_DATA_DIR") {
        return PathBuf::from(data_dir).join("profile-resource-cache");
    }
    dirs::cache_dir()
        .map(|dir| dir.join("bifrost").join("profile-resource-cache"))
        .unwrap_or_else(|| base_dir.join(".bifrost-profile-cache"))
}

fn content_cache_key(kind: ProfileResourceKind, reference: &str, content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("{kind:?}:{reference}:").as_bytes());
    hasher.update(content.as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

fn reference_cache_key(kind: ProfileResourceKind, reference: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("{kind:?}:{reference}").as_bytes());
    format!("remote-sha256:{:x}", hasher.finalize())
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
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex, OnceLock};
    use std::thread::JoinHandle;

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
        assert!(report.summary.translated_with_behavior_note >= 3);
        assert!(report.summary.needs_manual_review >= 1);
        assert!(report.summary.not_supported_yet >= 1);
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

    #[test]
    fn resolved_profile_expands_local_sets_and_include() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("included.conf"),
            "[Rule]\nDOMAIN,included.example,DIRECT\n",
        )
        .unwrap();
        std::fs::write(
            temp.path().join("rules.list"),
            "DOMAIN-SUFFIX,rules.example\nDOMAIN,exact.rules.example\n",
        )
        .unwrap();
        std::fs::write(
            temp.path().join("domains.list"),
            "domainset.example\n.sub.domainset.example\n",
        )
        .unwrap();
        let profile = r#"
#!include included.conf
[Proxy]
ProxyA = http, 127.0.0.1, 8080

[Proxy Group]
Proxy = select, ProxyA, DIRECT, MissingProxy

[Rule]
RULE-SET,rules.list,Proxy
DOMAIN-SET,domains.list,DIRECT
FINAL,Proxy
"#;
        let document = parse_surge_profile(profile, ProfileSource::Inline);
        let resolved = resolve_surge_profile(document, temp.path());

        assert_eq!(resolved.resources.len(), 3);
        assert!(resolved
            .resources
            .iter()
            .all(|resource| resource.status == ProfileResourceStatus::Loaded));
        assert!(resolved.resources.iter().all(|resource| resource
            .cache_key
            .as_deref()
            .is_some_and(|key| key.starts_with("sha256:"))));
        assert!(resolved.runtime_plan.rules.iter().any(|rule| {
            rule.rule_type == "DOMAIN" && rule.value.as_deref() == Some("included.example")
        }));
        assert!(resolved.runtime_plan.rules.iter().any(|rule| {
            rule.rule_type == "DOMAIN-SUFFIX"
                && rule.value.as_deref() == Some("rules.example")
                && rule.policy == "Proxy"
        }));
        assert!(resolved.runtime_plan.rules.iter().any(|rule| {
            rule.rule_type == "DOMAIN-SUFFIX"
                && rule.value.as_deref() == Some("domainset.example")
                && rule.policy == "DIRECT"
        }));
        let proxy = &resolved.runtime_plan.policy_groups[0];
        assert_eq!(proxy.missing_members, vec!["MissingProxy"]);
    }

    #[test]
    fn resolved_explain_uses_expanded_rule_set_before_final() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("rules.list"),
            "DOMAIN-SUFFIX,expanded.example\n",
        )
        .unwrap();
        let document = parse_surge_profile(
            "[Rule]\nRULE-SET,rules.list,Proxy\nFINAL,DIRECT\n",
            ProfileSource::Inline,
        );
        let resolved = resolve_surge_profile(document, temp.path());
        let report =
            explain_surge_request_with_plan(&resolved.runtime_plan, "https://api.expanded.example")
                .unwrap();
        let matched = report.matched_rule.unwrap();
        assert_eq!(matched.rule_type, "DOMAIN-SUFFIX");
        assert_eq!(matched.policy, "Proxy");
        assert!(report
            .timeline
            .iter()
            .any(|step| step.message.contains("RULE-SET:rules.list")));
    }

    #[test]
    fn remote_resources_report_fetch_failure_when_unreachable() {
        let document = parse_surge_profile(
            "[Rule]\nRULE-SET,http://127.0.0.1:9/rules.list,Proxy\nFINAL,DIRECT\n",
            ProfileSource::Inline,
        );
        let resolved = resolve_surge_profile(document, Path::new("."));
        assert_eq!(resolved.resources.len(), 1);
        assert_eq!(
            resolved.resources[0].status,
            ProfileResourceStatus::FetchFailed
        );
        assert!(resolved.resources[0].cache_key.is_none());
        assert!(resolved
            .runtime_plan
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "surge.resource.fetch_failed"));
    }

    #[test]
    fn remote_rule_set_is_fetched_cached_and_expanded() {
        let temp = tempfile::tempdir().unwrap();
        with_profile_cache_dir(temp.path().join("cache"), || {
            let body = "DOMAIN-SUFFIX,remote.example\n";
            let (url, handle, requests) = start_http_fixture(vec![http_ok(body)]);
            let document = parse_surge_profile(
                &format!("[Rule]\nRULE-SET,{url},Proxy\nFINAL,DIRECT\n"),
                ProfileSource::Inline,
            );
            let resolved = resolve_surge_profile(document, temp.path());
            handle.join().unwrap();

            assert_eq!(requests.lock().unwrap().len(), 1);
            let resource = &resolved.resources[0];
            assert_eq!(resource.status, ProfileResourceStatus::Loaded);
            assert_eq!(resource.etag.as_deref(), Some("\"surge-test-v1\""));
            assert!(!resource.loaded_from_cache);
            assert!(resource
                .cache_key
                .as_deref()
                .is_some_and(|key| key.starts_with("sha256:")));
            assert!(resolved.runtime_plan.rules.iter().any(|rule| {
                rule.rule_type == "DOMAIN-SUFFIX"
                    && rule.value.as_deref() == Some("remote.example")
                    && rule.policy == "Proxy"
            }));
        });
    }

    #[test]
    fn remote_rule_set_uses_conditional_cache_on_not_modified() {
        let temp = tempfile::tempdir().unwrap();
        with_profile_cache_dir(temp.path().join("cache"), || {
            let body = "DOMAIN-SUFFIX,cached.example\n";
            let (url, handle, requests) = start_http_fixture(vec![http_ok(body), http_304()]);
            let profile = format!("[Rule]\nRULE-SET,{url},Proxy\nFINAL,DIRECT\n");

            let first = resolve_surge_profile(
                parse_surge_profile(&profile, ProfileSource::Inline),
                temp.path(),
            );
            let second = resolve_surge_profile(
                parse_surge_profile(&profile, ProfileSource::Inline),
                temp.path(),
            );
            handle.join().unwrap();

            let requests = requests.lock().unwrap();
            assert_eq!(requests.len(), 2);
            assert!(requests[1].contains("if-none-match: \"surge-test-v1\""));
            assert_eq!(first.resources[0].status, ProfileResourceStatus::Loaded);
            assert_eq!(second.resources[0].status, ProfileResourceStatus::CacheHit);
            assert!(second.resources[0].loaded_from_cache);
            assert!(second.runtime_plan.rules.iter().any(|rule| {
                rule.rule_type == "DOMAIN-SUFFIX" && rule.value.as_deref() == Some("cached.example")
            }));
        });
    }

    #[test]
    fn managed_profile_url_is_loaded_as_top_level_profile() {
        let temp = tempfile::tempdir().unwrap();
        with_profile_cache_dir(temp.path().join("cache"), || {
            let body = "[Rule]\nDOMAIN,managed.example,DIRECT\nFINAL,Proxy\n";
            let (url, handle, _) = start_http_fixture(vec![http_ok(body)]);
            let resolved = load_surge_profile_url(&url).unwrap();
            handle.join().unwrap();

            assert!(matches!(
                resolved.document.source,
                ProfileSource::ManagedUrl(_)
            ));
            assert_eq!(
                resolved.resources[0].kind,
                ProfileResourceKind::ManagedProfile
            );
            assert_eq!(resolved.resources[0].status, ProfileResourceStatus::Loaded);
            assert!(resolved.runtime_plan.rules.iter().any(|rule| {
                rule.rule_type == "DOMAIN" && rule.value.as_deref() == Some("managed.example")
            }));
        });
    }

    #[test]
    fn compile_resolved_surge_to_bifrost_rules_emits_reviewable_rule_file() {
        let document = parse_surge_profile(
            r#"
[Proxy]
ProxyA = http, 127.0.0.1, 8080
[Proxy Group]
Proxy = select, ProxyA, DIRECT
[Rule]
DOMAIN,api.example.com,DIRECT
DOMAIN-SUFFIX,example.com,Proxy
FINAL,REJECT
"#,
            ProfileSource::Inline,
        );
        let resolved = resolve_surge_profile(document, Path::new("."));
        let compiled = compile_resolved_surge_to_bifrost_rules(&resolved);

        assert_eq!(compiled.format, "bifrost-rule-file");
        assert!(compiled.content.contains("api.example.com passthrough://"));
        assert!(compiled
            .content
            .contains("*.example.com proxy://http://127.0.0.1:8080"));
        assert!(compiled.content.contains("/.*/ statusCode://403"));
        assert!(compiled.content.contains("policy chain: Proxy -> ProxyA"));
    }

    #[test]
    fn managed_profile_url_uses_stale_cache_when_remote_is_unreachable() {
        let temp = tempfile::tempdir().unwrap();
        with_profile_cache_dir(temp.path().join("cache"), || {
            let body = "[Rule]\nDOMAIN,stale-managed.example,DIRECT\nFINAL,DIRECT\n";
            let (url, handle, _) = start_http_fixture(vec![http_ok(body)]);
            let first = load_surge_profile_url(&url).unwrap();
            handle.join().unwrap();

            let second = load_surge_profile_url(&url).unwrap();

            assert_eq!(first.resources[0].status, ProfileResourceStatus::Loaded);
            assert_eq!(second.resources[0].status, ProfileResourceStatus::CacheHit);
            assert!(second.resources[0].loaded_from_cache);
            assert!(second.runtime_plan.rules.iter().any(|rule| {
                rule.rule_type == "DOMAIN" && rule.value.as_deref() == Some("stale-managed.example")
            }));
            assert!(second
                .runtime_plan
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "surge.resource.stale_cache_used"));
        });
    }

    #[test]
    fn policy_decision_resolves_select_group_to_selected_terminal_proxy() {
        let document = parse_surge_profile(
            r#"
[Proxy]
ProxyA = http, 127.0.0.1, 8080
ProxyB = http, 127.0.0.1, 8081
[Proxy Group]
Proxy = select, ProxyA, ProxyB, selected=ProxyB
[Rule]
DOMAIN,select.example,Proxy
FINAL,DIRECT
"#,
            ProfileSource::Inline,
        );
        let resolved = resolve_surge_profile(document, Path::new("."));
        let report =
            explain_surge_request_with_plan(&resolved.runtime_plan, "https://select.example")
                .unwrap();
        let decision = report.policy_decision.unwrap();
        assert_eq!(decision.requested_policy, "Proxy");
        assert_eq!(decision.terminal_policy, "ProxyB");
        assert_eq!(decision.chain, vec!["Proxy", "ProxyB"]);
        assert!(report.timeline.iter().any(|step| {
            step.stage == "policy-group" && step.message.contains("selected ProxyB")
        }));
    }

    #[test]
    fn policy_decision_resolves_url_test_group_to_first_candidate_in_dry_run() {
        let document = parse_surge_profile(
            r#"
[Proxy]
ProxyA = http, 127.0.0.1, 8080
[Proxy Group]
Auto = url-test, ProxyA, DIRECT, url=http://example.com/generate_204
[Rule]
DOMAIN,auto.example,Auto
FINAL,DIRECT
"#,
            ProfileSource::Inline,
        );
        let resolved = resolve_surge_profile(document, Path::new("."));
        let report =
            explain_surge_request_with_plan(&resolved.runtime_plan, "https://auto.example")
                .unwrap();
        let decision = report.policy_decision.unwrap();
        assert_eq!(decision.terminal_policy, "ProxyA");
        assert_eq!(decision.chain, vec!["Auto", "ProxyA"]);
        assert!(report.timeline.iter().any(|step| {
            step.message
                .contains("active latency probing is not running")
        }));
    }

    #[test]
    fn policy_decision_reports_missing_policy_member() {
        let document = parse_surge_profile(
            r#"
[Proxy Group]
Proxy = select, MissingProxy
[Rule]
DOMAIN,missing.example,Proxy
FINAL,DIRECT
"#,
            ProfileSource::Inline,
        );
        let resolved = resolve_surge_profile(document, Path::new("."));
        let report =
            explain_surge_request_with_plan(&resolved.runtime_plan, "https://missing.example")
                .unwrap();
        let decision = report.policy_decision.unwrap();
        assert_eq!(decision.terminal_policy, "MissingProxy");
        assert!(report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "surge.policy.missing"));
    }

    #[test]
    fn policy_decision_detects_policy_group_cycle() {
        let document = parse_surge_profile(
            r#"
[Proxy Group]
A = select, B
B = select, A
[Rule]
DOMAIN,cycle.example,A
FINAL,DIRECT
"#,
            ProfileSource::Inline,
        );
        let resolved = resolve_surge_profile(document, Path::new("."));
        let report =
            explain_surge_request_with_plan(&resolved.runtime_plan, "https://cycle.example")
                .unwrap();
        let decision = report.policy_decision.unwrap();
        assert_eq!(decision.terminal_policy, "A");
        assert!(report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "surge.policy.cycle"));
    }

    #[test]
    fn explain_reports_dns_mitm_and_http_pipeline_decisions() {
        let document = parse_surge_profile(
            r#"
[General]
dns-server = 8.8.8.8
[Host]
api.hosted.example = 203.0.113.10
[MITM]
hostname = %APPEND% *.example.com, -private.example.com
[URL Rewrite]
^https://rewrite\.example/path https://target.example/path 302
[Map Local]
^https://assets\.example/app\.js data/app.js
[Header Rewrite]
^https://headers\.example header-replace User-Agent Bifrost
[Script]
http-response ^https://script\.example script-path=scripts/response.js
[Rule]
DOMAIN,api.hosted.example,DIRECT
DOMAIN,rewrite.example,DIRECT
DOMAIN,assets.example,DIRECT
DOMAIN,headers.example,DIRECT
DOMAIN,script.example,DIRECT
DOMAIN,private.example.com,DIRECT
DOMAIN-SUFFIX,example.com,DIRECT
FINAL,DIRECT
"#,
            ProfileSource::Inline,
        );
        let resolved = resolve_surge_profile(document, Path::new("."));
        assert!(resolved
            .runtime_plan
            .http_pipeline
            .iter()
            .any(|item| item.section == "URL Rewrite" && item.key == "raw"));

        let dns_report = explain_surge_request_with_plan(
            &resolved.runtime_plan,
            "https://api.hosted.example/path",
        )
        .unwrap();
        assert_eq!(
            dns_report.dns_decision.matched_host_mapping.as_deref(),
            Some("Host mapping api.hosted.example -> 203.0.113.10")
        );

        let mitm_report =
            explain_surge_request_with_plan(&resolved.runtime_plan, "https://sub.example.com/path")
                .unwrap();
        assert!(mitm_report.mitm_decision.included);
        assert!(mitm_report
            .mitm_decision
            .matched_patterns
            .contains(&"*.example.com".to_string()));

        let excluded_report = explain_surge_request_with_plan(
            &resolved.runtime_plan,
            "https://private.example.com/path",
        )
        .unwrap();
        assert!(excluded_report.mitm_decision.excluded);
        assert!(excluded_report
            .mitm_decision
            .reason
            .contains("excluded from MITM"));

        let rewrite_report =
            explain_surge_request_with_plan(&resolved.runtime_plan, "https://rewrite.example/path")
                .unwrap();
        assert!(rewrite_report
            .http_pipeline
            .iter()
            .any(|entry| entry.section == "URL Rewrite" && entry.matched));

        let script_report =
            explain_surge_request_with_plan(&resolved.runtime_plan, "https://script.example/path")
                .unwrap();
        assert!(script_report
            .http_pipeline
            .iter()
            .any(|entry| entry.section == "Script" && entry.matched));
    }

    fn start_http_fixture(
        responses: Vec<String>,
    ) -> (String, JoinHandle<()>, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/resource.conf", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let thread_requests = Arc::clone(&requests);
        let handle = std::thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0_u8; 4096];
                let read = stream.read(&mut buffer).unwrap();
                thread_requests
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buffer[..read]).to_ascii_lowercase());
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        (url, handle, requests)
    }

    fn http_ok(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: \"surge-test-v1\"\r\nLast-Modified: Wed, 02 Jul 2026 00:00:00 GMT\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
    }

    fn http_304() -> String {
        "HTTP/1.1 304 Not Modified\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
    }

    fn with_profile_cache_dir<T>(cache_dir: PathBuf, run: impl FnOnce() -> T) -> T {
        let _guard = profile_cache_env_lock().lock().unwrap();
        let previous = std::env::var_os("BIFROST_PROFILE_CACHE_DIR");
        std::env::set_var("BIFROST_PROFILE_CACHE_DIR", cache_dir);
        let result = run();
        match previous {
            Some(value) => std::env::set_var("BIFROST_PROFILE_CACHE_DIR", value),
            None => std::env::remove_var("BIFROST_PROFILE_CACHE_DIR"),
        }
        result
    }

    fn profile_cache_env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }
}

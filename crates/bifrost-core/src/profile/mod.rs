use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use ipnet::IpNet;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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
    RemoteDryRun,
    Missing,
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

pub fn explain_surge_request_with_plan(
    plan: &ProfileRuntimePlan,
    input: &str,
) -> Result<ExplainReport> {
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
            message: "Resolved runtime plan dry-run does not resolve DNS; using URL host and optional literal IP only".to_string(),
        },
    ];

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
            timeline.push(ExplainStep {
                stage: "mitm".to_string(),
                line: None,
                message: explain_mitm_from_plan(plan, &request.host),
            });
            return Ok(ExplainReport {
                request,
                matched_rule: Some(rule),
                target_policy: Some(runtime_rule.policy.clone()),
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
        timeline,
        diagnostics,
    })
}

struct ProfileResolver {
    base_dir: PathBuf,
    resources: Vec<ProfileResource>,
    diagnostics: Vec<ProfileDiagnostic>,
}

impl ProfileResolver {
    fn new(base_dir: &Path) -> Self {
        Self {
            base_dir: base_dir.to_path_buf(),
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
                    _ => {}
                }
            }
        }
    }

    fn apply_directive(&mut self, directive: &DirectiveNode, plan: &mut ProfileRuntimePlan) {
        match directive.directive.as_str() {
            "INCLUDE" => self.load_include(directive, plan),
            "MANAGED-CONFIG" => self.record_remote_resource(
                ProfileResourceKind::ManagedProfile,
                directive.arguments.clone(),
                directive.source.line,
                "Managed profile URL is recorded but not fetched during dry-run resolution",
            ),
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
        if is_remote_reference(reference) {
            self.record_remote_resource(
                ProfileResourceKind::Include,
                reference.to_string(),
                directive.source.line,
                "Remote include is recorded but not fetched during dry-run resolution",
            );
            return;
        }
        let path = self.resolve_path(reference);
        let Ok(text) = std::fs::read_to_string(&path) else {
            self.record_missing_resource(
                ProfileResourceKind::Include,
                reference.to_string(),
                directive.source.line,
                "include target could not be read",
            );
            return;
        };
        let included = parse_surge_profile(&text, ProfileSource::LocalPath(path.clone()));
        let before_rules = plan.rules.len();
        self.apply_document(&included, &format!("include:{}", reference), plan);
        let item_count = plan.rules.len().saturating_sub(before_rules);
        self.resources.push(ProfileResource {
            kind: ProfileResourceKind::Include,
            reference: reference.to_string(),
            source_line: directive.source.line,
            status: ProfileResourceStatus::Loaded,
            cache_key: Some(content_cache_key(
                ProfileResourceKind::Include,
                reference,
                &text,
            )),
            item_count,
            diagnostics: included.diagnostics,
        });
    }

    fn load_rule_set(&mut self, rule: &RuleNode, plan: &mut ProfileRuntimePlan) {
        let reference = rule.value.as_deref().unwrap_or("").trim();
        if is_remote_reference(reference) {
            self.record_remote_resource(
                ProfileResourceKind::RuleSet,
                reference.to_string(),
                rule.source.line,
                "Remote RULE-SET is recorded but not fetched during dry-run resolution",
            );
            return;
        }
        let Some(text) =
            self.read_local_resource(ProfileResourceKind::RuleSet, reference, rule.source.line)
        else {
            return;
        };
        let mut count = 0;
        for (index, raw_line) in text.lines().enumerate() {
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
            status: ProfileResourceStatus::Loaded,
            cache_key: Some(content_cache_key(
                ProfileResourceKind::RuleSet,
                reference,
                &text,
            )),
            item_count: count,
            diagnostics: Vec::new(),
        });
    }

    fn load_domain_set(&mut self, rule: &RuleNode, plan: &mut ProfileRuntimePlan) {
        let reference = rule.value.as_deref().unwrap_or("").trim();
        if is_remote_reference(reference) {
            self.record_remote_resource(
                ProfileResourceKind::DomainSet,
                reference.to_string(),
                rule.source.line,
                "Remote DOMAIN-SET is recorded but not fetched during dry-run resolution",
            );
            return;
        }
        let Some(text) =
            self.read_local_resource(ProfileResourceKind::DomainSet, reference, rule.source.line)
        else {
            return;
        };
        let mut count = 0;
        for (index, raw_line) in text.lines().enumerate() {
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
            status: ProfileResourceStatus::Loaded,
            cache_key: Some(content_cache_key(
                ProfileResourceKind::DomainSet,
                reference,
                &text,
            )),
            item_count: count,
            diagnostics: Vec::new(),
        });
    }

    fn read_local_resource(
        &mut self,
        kind: ProfileResourceKind,
        reference: &str,
        source_line: usize,
    ) -> Option<String> {
        if reference.is_empty() {
            self.record_missing_resource(
                kind,
                reference.to_string(),
                source_line,
                "resource reference is empty",
            );
            return None;
        }
        let path = self.resolve_path(reference);
        match std::fs::read_to_string(&path) {
            Ok(text) => Some(text),
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

    fn record_remote_resource(
        &mut self,
        kind: ProfileResourceKind,
        reference: String,
        source_line: usize,
        message: &str,
    ) {
        let cache_key = reference_cache_key(kind, &reference);
        let diagnostic = ProfileDiagnostic {
            severity: DiagnosticSeverity::Info,
            line: source_line,
            column: 1,
            code: "surge.resource.remote_dry_run".to_string(),
            message: message.to_string(),
            suggestion: Some(
                "Managed network fetching will be enabled behind explicit cache and trust controls"
                    .to_string(),
            ),
        };
        self.resources.push(ProfileResource {
            kind,
            reference,
            source_line,
            status: ProfileResourceStatus::RemoteDryRun,
            cache_key: Some(cache_key),
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
            item_count: 0,
            diagnostics: vec![diagnostic.clone()],
        });
        self.diagnostics.push(diagnostic);
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
            "Local set references are expanded in the resolved dry-run runtime plan; remote sets are recorded for cache/trust review",
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
            "local include can be loaded into the resolved dry-run plan; remote include is recorded but not fetched",
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

fn explain_mitm_from_plan(plan: &ProfileRuntimePlan, host: &str) -> String {
    for kv in &plan.mitm {
        if matches!(kv.key.as_str(), "hostname" | "hostnames") {
            return format!(
                "MITM hostname scope is present at line {}; dry-run only, review whether {} is included",
                kv.source.line, host
            );
        }
    }
    "No MITM hostname scope found in resolved profile".to_string()
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
    fn remote_resources_are_recorded_without_fetching() {
        let document = parse_surge_profile(
            "[Rule]\nRULE-SET,https://example.com/rules.list,Proxy\nFINAL,DIRECT\n",
            ProfileSource::Inline,
        );
        let resolved = resolve_surge_profile(document, Path::new("."));
        assert_eq!(resolved.resources.len(), 1);
        assert_eq!(
            resolved.resources[0].status,
            ProfileResourceStatus::RemoteDryRun
        );
        assert!(resolved.resources[0]
            .cache_key
            .as_deref()
            .is_some_and(|key| key.starts_with("remote-sha256:")));
        assert!(resolved
            .runtime_plan
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "surge.resource.remote_dry_run"));
    }
}

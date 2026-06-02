use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest as Sha2Digest, Sha256};

mod runtime;
mod scheduler;

pub use scheduler::ensure_workflow_scheduler_started;

pub const WORKFLOW_API_VERSION: &str = "bifrost.ai.workflow/v1alpha1";
pub const WORKFLOW_KIND: &str = "Workflow";
pub const WORKFLOW_SCHEMA_REF: &str = "bifrost://schemas/ai-workflow/v1alpha1";
pub const DEFAULT_ASR_WORKFLOW_TEMPLATE_ID: &str = "default-asr-transcription";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowDocument {
    #[serde(rename = "apiVersion", default)]
    pub api_version: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub metadata: WorkflowMetadata,
    #[serde(default)]
    pub spec: WorkflowSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui: Option<WorkflowUi>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowMetadata {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_ref: Option<String>,
    #[serde(default)]
    pub inputs: Vec<WorkflowInput>,
    #[serde(default)]
    pub triggers: Vec<Value>,
    #[serde(default)]
    pub nodes: Vec<WorkflowNode>,
    #[serde(default)]
    pub edges: Vec<WorkflowEdge>,
    #[serde(default)]
    pub outputs: Vec<WorkflowOutput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_policy: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowInput {
    #[serde(default)]
    pub name: String,
    #[serde(rename = "type", default)]
    pub input_type: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowNode {
    #[serde(default)]
    pub id: String,
    #[serde(rename = "type", default)]
    pub node_type: String,
    #[serde(default)]
    pub inputs: Vec<NodeInput>,
    #[serde(default)]
    pub outputs: Vec<WorkflowOutput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct NodeInput {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub source: Value,
    #[serde(rename = "as", default, skip_serializing_if = "Option::is_none")]
    pub as_type: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowOutput {
    #[serde(default)]
    pub name: String,
    #[serde(rename = "type", default)]
    pub output_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_template: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowEdge {
    #[serde(default)]
    pub from: String,
    #[serde(default)]
    pub to: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowUi {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub react_flow: Option<ReactFlowGraph>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ReactFlowGraph {
    #[serde(default)]
    pub nodes: Vec<Value>,
    #[serde(default)]
    pub edges: Vec<Value>,
    #[serde(default)]
    pub viewport: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowDiagnostic {
    pub severity: DiagnosticSeverity,
    pub code: String,
    pub path: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_fix: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Error,
    Warning,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowValidationReport {
    pub schema_version: String,
    pub valid: bool,
    pub errors: Vec<WorkflowDiagnostic>,
    pub warnings: Vec<WorkflowDiagnostic>,
    pub auto_fixes: Vec<WorkflowDiagnostic>,
    pub requires_confirmation: Vec<WorkflowDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowPreview {
    pub draft_hash: String,
    pub blocking_errors: Vec<WorkflowDiagnostic>,
    pub warnings: Vec<WorkflowDiagnostic>,
    pub markdown: String,
    pub react_flow: ReactFlowGraph,
    pub effective_inputs: Vec<Value>,
    pub permission_risks: Vec<Value>,
    pub dry_run_runbook: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowSummary {
    pub id: String,
    pub name: String,
    pub revision: u64,
    pub node_count: usize,
    pub edge_count: usize,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunRecord {
    pub id: String,
    pub workflow_id: String,
    pub workflow_revision: u64,
    pub status: String,
    pub created_at: String,
    pub finished_at: Option<String>,
    pub node_states: Vec<Value>,
    pub events: Vec<Value>,
    pub artifacts_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowScheduleState {
    #[serde(default)]
    pub workflow_id: String,
    #[serde(default)]
    pub trigger_index: usize,
    #[serde(default)]
    pub last_run_at_ms: Option<u64>,
    #[serde(default)]
    pub next_run_at_ms: Option<u64>,
    #[serde(default)]
    pub last_run_id: Option<String>,
    #[serde(default)]
    pub last_status: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowApplyRequest {
    pub workflow: WorkflowDocument,
    #[serde(default)]
    pub base_revision: Option<u64>,
    #[serde(default)]
    pub preview_hash: Option<String>,
    #[serde(default)]
    pub confirmed_by: Option<String>,
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowTemplate {
    pub id: String,
    pub name: String,
    pub description: String,
    pub tags: Vec<String>,
    pub workflow: WorkflowDocument,
    pub draft: String,
}

pub fn parse_workflow_document(input: &str) -> Result<WorkflowDocument, String> {
    serde_json::from_str(input).or_else(|json_error| {
        serde_yaml::from_str(input)
            .map_err(|yaml_error| format!("invalid workflow JSON/YAML: {json_error}; {yaml_error}"))
    })
}

pub fn normalize_workflow(mut workflow: WorkflowDocument) -> WorkflowDocument {
    if workflow.api_version.trim().is_empty() {
        workflow.api_version = WORKFLOW_API_VERSION.to_string();
    }
    if workflow.kind.trim().is_empty() {
        workflow.kind = WORKFLOW_KIND.to_string();
    }
    if workflow.spec.schema_ref.is_none() {
        workflow.spec.schema_ref = Some(WORKFLOW_SCHEMA_REF.to_string());
    }
    if workflow.metadata.name.trim().is_empty() && !workflow.metadata.id.trim().is_empty() {
        workflow.metadata.name = workflow.metadata.id.clone();
    }
    workflow
}

pub fn default_asr_workflow_template() -> WorkflowTemplate {
    let draft = r#"apiVersion: bifrost.ai.workflow/v1alpha1
kind: Workflow
metadata:
  id: default-asr-transcription
  name: Default ASR Transcription Workflow
  description: Scan an audio directory, transcribe new files, then generate a Daily Agent report.
  labels:
    bifrost.io/template: default-asr
spec:
  schemaRef: bifrost://schemas/ai-workflow/v1alpha1
  resourcePolicy:
    default: deny
  triggers:
    - type: schedule
      enabled: false
      cron: "0 8 * * *"
      timezone: Asia/Shanghai
      description: Enable and adjust this schedule after choosing the audio directory.
  inputs:
    - name: audio_dir
      type: file_set
      required: true
      description: Directory containing audio files to transcribe.
    - name: focus_topics
      type: text
      required: false
      default: "行动项、客户问题、研发风险"
      description: Topics that the Daily Agent should emphasize in the final report.
  nodes:
    - id: transcribe_daily_audio
      type: asr_transcription
      inputs:
        - name: audio_dir
          source:
            type: workflow_input
            name: audio_dir
          as: file_set
      noUpdatePolicy:
        skipDownstream: true
      outputs:
        - name: daily_markdown
          type: document
          pathTemplate: daily/{{run.date}}.md
        - name: transcription_manifest
          type: json
          pathTemplate: manifests/{{run.date}}.json
    - id: run_daily_agent
      type: runner
      runner: codex
      prompt: Read the Daily Markdown and generate a concise report with action items, risks, and follow-ups.
      retryStrategy:
        maxAttempts: 2
      inputs:
        - name: daily_markdown
          source:
            type: node_output
            nodeId: transcribe_daily_audio
            output: daily_markdown
          as: document
        - name: focus_topics
          source:
            type: workflow_input
            name: focus_topics
          as: text
      outputs:
        - name: daily_report
          type: document
          pathTemplate: reports/daily-agent/{{run.date}}.md
  edges:
    - from: transcribe_daily_audio
      to: run_daily_agent
  outputs:
    - name: final_report
      type: document
      from: run_daily_agent.outputs.daily_report
"#;
    let workflow = parse_workflow_document(draft)
        .map(normalize_workflow)
        .expect("default ASR workflow template must parse");
    WorkflowTemplate {
        id: DEFAULT_ASR_WORKFLOW_TEMPLATE_ID.to_string(),
        name: "Default ASR Transcription Workflow".to_string(),
        description: "Start from the built-in ASR replacement flow: scan an audio directory, skip downstream work when there is no update, and run a configurable Daily Agent report.".to_string(),
        tags: vec!["default".to_string(), "asr".to_string(), "runner".to_string()],
        workflow,
        draft: draft.to_string(),
    }
}

pub fn workflow_templates() -> Vec<WorkflowTemplate> {
    vec![default_asr_workflow_template()]
}

pub fn workflow_template(template_id: &str) -> Option<WorkflowTemplate> {
    workflow_templates()
        .into_iter()
        .find(|template| template.id == template_id)
}

pub fn validate_workflow(workflow: &WorkflowDocument) -> WorkflowValidationReport {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut auto_fixes = Vec::new();
    let mut requires_confirmation = Vec::new();

    if workflow.api_version != WORKFLOW_API_VERSION {
        errors.push(error(
            "invalid_api_version",
            "/apiVersion",
            format!("apiVersion must be {WORKFLOW_API_VERSION}"),
        ));
    }
    if workflow.kind != WORKFLOW_KIND {
        errors.push(error("invalid_kind", "/kind", "kind must be Workflow"));
    }
    if workflow.metadata.id.trim().is_empty() {
        errors.push(error(
            "missing_metadata_id",
            "/metadata/id",
            "metadata.id is required",
        ));
    } else if !is_safe_id(&workflow.metadata.id) {
        errors.push(error(
            "invalid_metadata_id",
            "/metadata/id",
            "metadata.id may only contain ASCII letters, numbers, '-' and '_'",
        ));
    }
    if workflow.spec.nodes.is_empty() {
        errors.push(error(
            "missing_nodes",
            "/spec/nodes",
            "spec.nodes must contain at least one node",
        ));
    }

    let mut node_ids = BTreeSet::new();
    let mut output_names: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (index, node) in workflow.spec.nodes.iter().enumerate() {
        let node_path = format!("/spec/nodes/{index}");
        if node.id.trim().is_empty() {
            errors.push(error(
                "missing_node_id",
                format!("{node_path}/id"),
                "node id is required",
            ));
        } else if !node_ids.insert(node.id.as_str()) {
            errors.push(error(
                "duplicate_node_id",
                format!("{node_path}/id"),
                format!("duplicate node id `{}`", node.id),
            ));
        }
        if !matches!(
            node.node_type.as_str(),
            "script" | "runner" | "asr_transcription" | "notification"
        ) {
            errors.push(error(
                "unsupported_node_type",
                format!("{node_path}/type"),
                format!("unsupported node type `{}`", node.node_type),
            ));
        }
        if node.node_type == "runner" && node.inputs.is_empty() {
            errors.push(error(
                "runner_requires_explicit_inputs",
                format!("{node_path}/inputs"),
                "runner nodes must declare explicit effective inputs",
            ));
        }
        if node.node_type == "notification" && !node.extra.contains_key("channel") {
            errors.push(error(
                "notification_requires_channel",
                node_path.clone(),
                "notification nodes must declare channel provider and target",
            ));
        }
        if node.node_type == "asr_transcription"
            && !node.extra.contains_key("noUpdatePolicy")
            && !node.extra.contains_key("no_update_policy")
        {
            warnings.push(warning(
                "asr_no_update_policy_missing",
                node_path.clone(),
                "ASR transcription nodes should declare noUpdatePolicy.skipDownstream=true",
            ));
        }

        let mut names = BTreeSet::new();
        for (output_index, output) in node.outputs.iter().enumerate() {
            if output.name.trim().is_empty() {
                errors.push(error(
                    "missing_output_name",
                    format!("{node_path}/outputs/{output_index}/name"),
                    "output name is required",
                ));
            } else {
                names.insert(output.name.as_str());
            }
            if let Some(path_template) = &output.path_template {
                validate_safe_path_template(
                    path_template,
                    &format!("{node_path}/outputs/{output_index}/pathTemplate"),
                    &mut errors,
                );
            }
        }
        output_names.insert(node.id.as_str(), names);
    }

    let input_names: BTreeSet<&str> = workflow
        .spec
        .inputs
        .iter()
        .map(|input| input.name.as_str())
        .collect();
    for (node_index, node) in workflow.spec.nodes.iter().enumerate() {
        for (input_index, input) in node.inputs.iter().enumerate() {
            validate_input_source(
                &input.source,
                &format!("/spec/nodes/{node_index}/inputs/{input_index}/source"),
                &input_names,
                &output_names,
                &mut errors,
            );
        }
    }

    for (index, edge) in workflow.spec.edges.iter().enumerate() {
        if !node_ids.contains(edge.from.as_str()) {
            errors.push(error(
                "edge_from_not_found",
                format!("/spec/edges/{index}/from"),
                format!("edge source node `{}` does not exist", edge.from),
            ));
        }
        if !node_ids.contains(edge.to.as_str()) {
            errors.push(error(
                "edge_to_not_found",
                format!("/spec/edges/{index}/to"),
                format!("edge target node `{}` does not exist", edge.to),
            ));
        }
    }
    if has_cycle(&workflow.spec.nodes, &workflow.spec.edges) {
        errors.push(error(
            "dag_cycle",
            "/spec/edges",
            "workflow edges must form a DAG",
        ));
    }

    for (index, output) in workflow.spec.outputs.iter().enumerate() {
        if let Some(from) = &output.from {
            validate_output_ref(
                from,
                &format!("/spec/outputs/{index}/from"),
                &output_names,
                &mut errors,
            );
        }
    }

    let default_policy = workflow
        .spec
        .resource_policy
        .as_ref()
        .and_then(|policy| policy.get("default"))
        .and_then(Value::as_str);
    if default_policy != Some("deny") {
        warnings.push(warning(
            "resource_policy_default_deny_missing",
            "/spec/resourcePolicy",
            "spec.resourcePolicy.default should be deny",
        ));
        auto_fixes.push(warning(
            "resource_policy_default_deny_autofix",
            "/spec/resourcePolicy/default",
            "set default to deny",
        ));
    }

    for (node_index, node) in workflow.spec.nodes.iter().enumerate() {
        if node.node_type == "script" || node.node_type == "notification" {
            requires_confirmation.push(warning(
                "side_effect_requires_confirmation",
                format!("/spec/nodes/{node_index}"),
                format!(
                    "{} node `{}` may write files, run code, or send IM messages and requires preview confirmation",
                    node.node_type, node.id
                ),
            ));
        }
    }

    WorkflowValidationReport {
        schema_version: WORKFLOW_SCHEMA_REF.to_string(),
        valid: errors.is_empty(),
        errors,
        warnings,
        auto_fixes,
        requires_confirmation,
    }
}

pub fn preview_workflow(workflow: &WorkflowDocument) -> WorkflowPreview {
    let validation = validate_workflow(workflow);
    let react_flow = render_workflow(workflow);
    let draft_hash = workflow_hash(workflow);
    let mut markdown = String::new();
    markdown.push_str(&format!("## {}\n\n", workflow.metadata.name));
    markdown.push_str(&format!(
        "- Workflow: `{}` revision `{}`\n",
        workflow.metadata.id, workflow.metadata.revision
    ));
    markdown.push_str(&format!(
        "- Nodes: `{}`; Edges: `{}`\n",
        workflow.spec.nodes.len(),
        workflow.spec.edges.len()
    ));
    markdown.push_str("\n### DAG\n");
    for node in &workflow.spec.nodes {
        let upstream: Vec<&str> = workflow
            .spec
            .edges
            .iter()
            .filter(|edge| edge.to == node.id)
            .map(|edge| edge.from.as_str())
            .collect();
        markdown.push_str(&format!(
            "- `{}` ({}) after {:?}\n",
            node.id, node.node_type, upstream
        ));
    }

    let effective_inputs = workflow
        .spec
        .nodes
        .iter()
        .filter(|node| node.node_type == "runner")
        .map(|node| {
            json!({
                "nodeId": node.id,
                "inputs": node.inputs.iter().map(|input| json!({
                    "name": input.name,
                    "source": input.source,
                    "as": input.as_type,
                })).collect::<Vec<_>>()
            })
        })
        .collect::<Vec<_>>();

    let permission_risks = validation
        .requires_confirmation
        .iter()
        .map(|diagnostic| {
            json!({
                "code": diagnostic.code,
                "path": diagnostic.path,
                "message": diagnostic.message,
            })
        })
        .collect::<Vec<_>>();

    let dry_run_runbook = workflow
        .spec
        .nodes
        .iter()
        .map(|node| {
            format!(
                "Validate and schedule `{}` ({}) with declared inputs only",
                node.id, node.node_type
            )
        })
        .collect::<Vec<_>>();

    WorkflowPreview {
        draft_hash,
        blocking_errors: validation.errors,
        warnings: validation.warnings,
        markdown,
        react_flow,
        effective_inputs,
        permission_risks,
        dry_run_runbook,
    }
}

pub fn render_workflow(workflow: &WorkflowDocument) -> ReactFlowGraph {
    let nodes = workflow
        .spec
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            json!({
                "id": node.id,
                "type": "workflowNode",
                "position": { "x": 80 + ((index % 3) as i64 * 260), "y": 80 + ((index / 3) as i64 * 170) },
                "data": { "label": node.id, "kind": node.node_type, "outputs": node.outputs },
            })
        })
        .collect();
    let edges = workflow
        .spec
        .edges
        .iter()
        .enumerate()
        .map(|(index, edge)| {
            json!({
                "id": format!("edge-{}-{}-{}", edge.from, edge.to, index),
                "source": edge.from,
                "target": edge.to,
            })
        })
        .collect();
    ReactFlowGraph {
        nodes,
        edges,
        viewport: json!({ "x": 0, "y": 0, "zoom": 1 }),
    }
}

pub fn workflow_hash(workflow: &WorkflowDocument) -> String {
    let bytes = serde_json::to_vec(workflow).unwrap_or_default();
    let digest = Sha256::digest(&bytes);
    format!(
        "sha256:{}",
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}

#[derive(Debug, Clone)]
pub struct WorkflowStore {
    root: PathBuf,
}

impl WorkflowStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root_dir(&self) -> PathBuf {
        self.root.clone()
    }

    pub fn definitions_dir(&self) -> PathBuf {
        self.root.join("definitions")
    }

    pub fn runs_dir(&self) -> PathBuf {
        self.root.join("runs")
    }

    pub fn scheduler_dir(&self) -> PathBuf {
        self.root.join("scheduler")
    }

    pub fn list(&self) -> io::Result<Vec<WorkflowSummary>> {
        let dir = self.definitions_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut summaries = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            if entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let workflow = self.read_path(&entry.path())?;
            let metadata = entry.metadata()?;
            let updated_at = metadata
                .modified()
                .ok()
                .map(DateTime::<Utc>::from)
                .unwrap_or_else(Utc::now)
                .to_rfc3339();
            summaries.push(WorkflowSummary {
                id: workflow.metadata.id,
                name: workflow.metadata.name,
                revision: workflow.metadata.revision,
                node_count: workflow.spec.nodes.len(),
                edge_count: workflow.spec.edges.len(),
                updated_at,
            });
        }
        summaries.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(summaries)
    }

    pub fn get(&self, id: &str) -> io::Result<WorkflowDocument> {
        self.read_path(&self.definition_path(id))
    }

    pub fn save(
        &self,
        workflow: WorkflowDocument,
        base_revision: Option<u64>,
    ) -> Result<WorkflowDocument, String> {
        let mut workflow = normalize_workflow(workflow);
        let report = validate_workflow(&workflow);
        if !report.valid {
            return Err(format!(
                "workflow validation failed: {}",
                report
                    .errors
                    .iter()
                    .map(|item| item.code.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        let path = self.definition_path(&workflow.metadata.id);
        if path.exists() {
            let current = self.read_path(&path).map_err(|error| error.to_string())?;
            if let Some(base_revision) = base_revision {
                if current.metadata.revision != base_revision {
                    return Err(format!(
                        "base revision mismatch: current={}, requested={base_revision}",
                        current.metadata.revision
                    ));
                }
            }
            workflow.metadata.revision = current.metadata.revision.saturating_add(1);
        } else {
            workflow.metadata.revision = workflow.metadata.revision.max(1);
        }
        fs::create_dir_all(self.definitions_dir()).map_err(|error| error.to_string())?;
        let body = serde_json::to_string_pretty(&workflow).map_err(|error| error.to_string())?;
        fs::write(path, body).map_err(|error| error.to_string())?;
        Ok(workflow)
    }

    pub fn create_run(
        &self,
        workflow_id: &str,
        inputs: Value,
    ) -> Result<WorkflowRunRecord, String> {
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            tokio::task::block_in_place(|| {
                handle.block_on(self.create_run_async(workflow_id, inputs))
            })
        } else {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| format!("build workflow runtime failed: {error}"))?
                .block_on(self.create_run_async(workflow_id, inputs))
        }
    }

    pub async fn create_run_async(
        &self,
        workflow_id: &str,
        inputs: Value,
    ) -> Result<WorkflowRunRecord, String> {
        let workflow = self.get(workflow_id).map_err(|error| error.to_string())?;
        let report = validate_workflow(&workflow);
        if !report.valid {
            return Err("cannot run invalid workflow".to_string());
        }
        runtime::create_run(&workflow, inputs, &self.runs_dir()).await
    }

    pub fn get_run(&self, run_id: &str) -> io::Result<WorkflowRunRecord> {
        let body = fs::read_to_string(self.runs_dir().join(run_id).join("run.json"))?;
        serde_json::from_str(&body)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    pub fn list_runs(&self, workflow_id: &str) -> io::Result<Vec<WorkflowRunRecord>> {
        let dir = self.runs_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut runs = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let path = entry.path().join("run.json");
            if !path.exists() {
                continue;
            }
            let body = fs::read_to_string(path)?;
            let run: WorkflowRunRecord = serde_json::from_str(&body)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            if run.workflow_id == workflow_id {
                runs.push(run);
            }
        }
        runs.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(runs)
    }

    pub fn list_schedule_states(&self) -> io::Result<Vec<WorkflowScheduleState>> {
        let mut states_by_key = BTreeMap::<(String, usize), WorkflowScheduleState>::new();
        let dir = self.scheduler_dir();
        if dir.exists() {
            for entry in fs::read_dir(dir)? {
                let entry = entry?;
                if entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
                    continue;
                }
                let body = fs::read_to_string(entry.path())?;
                let state: WorkflowScheduleState = serde_json::from_str(&body)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                states_by_key.insert((state.workflow_id.clone(), state.trigger_index), state);
            }
        }
        let now = scheduler::now_ms();
        for workflow in self.list()? {
            let document = self.get(&workflow.id)?;
            for (trigger_index, trigger) in document.spec.triggers.iter().enumerate() {
                let Some(schedule) = scheduler::parse_schedule_trigger(trigger) else {
                    continue;
                };
                if !schedule.enabled {
                    continue;
                }
                states_by_key
                    .entry((document.metadata.id.clone(), trigger_index))
                    .or_insert_with(|| WorkflowScheduleState {
                        workflow_id: document.metadata.id.clone(),
                        trigger_index,
                        next_run_at_ms: scheduler::compute_next_schedule_run(&schedule, now),
                        updated_at_ms: now,
                        ..Default::default()
                    });
            }
        }
        let mut states = states_by_key.into_values().collect::<Vec<_>>();
        states.sort_by(|a, b| {
            a.workflow_id
                .cmp(&b.workflow_id)
                .then(a.trigger_index.cmp(&b.trigger_index))
        });
        Ok(states)
    }

    pub fn get_schedule_state(
        &self,
        workflow_id: &str,
        trigger_index: usize,
    ) -> io::Result<Option<WorkflowScheduleState>> {
        let path = self.schedule_state_path(workflow_id, trigger_index);
        if !path.exists() {
            return Ok(None);
        }
        let body = fs::read_to_string(path)?;
        serde_json::from_str(&body)
            .map(Some)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    pub fn save_schedule_state(&self, state: &WorkflowScheduleState) -> io::Result<()> {
        fs::create_dir_all(self.scheduler_dir())?;
        fs::write(
            self.schedule_state_path(&state.workflow_id, state.trigger_index),
            serde_json::to_string_pretty(state)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
        )
    }

    fn read_path(&self, path: &Path) -> io::Result<WorkflowDocument> {
        let body = fs::read_to_string(path)?;
        serde_json::from_str(&body)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    fn definition_path(&self, id: &str) -> PathBuf {
        self.definitions_dir()
            .join(format!("{}.json", sanitize_id(id)))
    }

    fn schedule_state_path(&self, workflow_id: &str, trigger_index: usize) -> PathBuf {
        self.scheduler_dir().join(format!(
            "{}-{}.json",
            sanitize_id(workflow_id),
            trigger_index
        ))
    }
}

impl Default for WorkflowStore {
    fn default() -> Self {
        Self::new(bifrost_storage::data_dir().join("agent/workflows"))
    }
}

pub fn schema_payload() -> Value {
    json!({
        "apiVersion": WORKFLOW_API_VERSION,
        "kind": WORKFLOW_KIND,
        "schemaRef": WORKFLOW_SCHEMA_REF,
        "nodeTypes": ["script", "runner", "asr_transcription", "notification"],
        "inputSources": ["workflow_input", "node_output", "literal_text", "literal_script", "file_ref", "artifact_query"],
        "requiredFlow": ["workflow_draft_create", "workflow_validate", "workflow_preview", "workflow_apply"],
        "defaultTemplateId": DEFAULT_ASR_WORKFLOW_TEMPLATE_ID,
    })
}

pub fn workflow_templates_payload() -> Value {
    json!({ "templates": workflow_templates() })
}

fn validate_input_source(
    source: &Value,
    path: &str,
    input_names: &BTreeSet<&str>,
    output_names: &BTreeMap<&str, BTreeSet<&str>>,
    errors: &mut Vec<WorkflowDiagnostic>,
) {
    let Some(source_type) = source.get("type").and_then(Value::as_str) else {
        errors.push(error(
            "missing_input_source_type",
            path,
            "input source type is required",
        ));
        return;
    };
    match source_type {
        "workflow_input" => {
            let name = source
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !input_names.contains(name) {
                errors.push(error(
                    "workflow_input_not_found",
                    path,
                    format!("workflow input `{name}` does not exist"),
                ));
            }
        }
        "node_output" => {
            let node_id = source
                .get("nodeId")
                .or_else(|| source.get("node_id"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let output = source
                .get("output")
                .and_then(Value::as_str)
                .unwrap_or_default();
            match output_names.get(node_id) {
                Some(names) if names.contains(output) => {}
                Some(_) => errors.push(error(
                    "node_output_not_found",
                    path,
                    format!("node `{node_id}` has no output `{output}`"),
                )),
                None => errors.push(error(
                    "node_output_node_not_found",
                    path,
                    format!("node `{node_id}` does not exist"),
                )),
            }
        }
        "literal_text" | "literal_script" | "artifact_query" => {}
        "file_ref" => {
            if let Some(file_path) = source.get("path").and_then(Value::as_str) {
                validate_safe_path_template(file_path, path, errors);
            }
        }
        other => errors.push(error(
            "unsupported_input_source",
            path,
            format!("unsupported input source `{other}`"),
        )),
    }
}

fn validate_output_ref(
    ref_path: &str,
    path: &str,
    output_names: &BTreeMap<&str, BTreeSet<&str>>,
    errors: &mut Vec<WorkflowDiagnostic>,
) {
    let Some((node_id, output)) = ref_path.split_once(".outputs.") else {
        errors.push(error(
            "invalid_output_ref",
            path,
            "output ref must use node.outputs.outputName",
        ));
        return;
    };
    match output_names.get(node_id) {
        Some(names) if names.contains(output) => {}
        Some(_) => errors.push(error(
            "output_ref_not_found",
            path,
            format!("node `{node_id}` has no output `{output}`"),
        )),
        None => errors.push(error(
            "output_ref_node_not_found",
            path,
            format!("node `{node_id}` does not exist"),
        )),
    }
}

fn validate_safe_path_template(
    path_template: &str,
    path: &str,
    errors: &mut Vec<WorkflowDiagnostic>,
) {
    if Path::new(path_template).is_absolute()
        || path_template.contains(':')
        || path_template.split(['/', '\\']).any(|part| part == "..")
    {
        errors.push(error(
            "unsafe_path_template",
            path,
            "path templates must stay inside the workflow artifact directory",
        ));
    }
}

fn is_safe_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

fn has_cycle(nodes: &[WorkflowNode], edges: &[WorkflowEdge]) -> bool {
    let mut indegree = BTreeMap::<&str, usize>::new();
    let mut outgoing = BTreeMap::<&str, Vec<&str>>::new();
    for node in nodes {
        indegree.insert(node.id.as_str(), 0);
    }
    for edge in edges {
        if indegree.contains_key(edge.from.as_str()) && indegree.contains_key(edge.to.as_str()) {
            *indegree.entry(edge.to.as_str()).or_default() += 1;
            outgoing
                .entry(edge.from.as_str())
                .or_default()
                .push(edge.to.as_str());
        }
    }
    let mut queue = indegree
        .iter()
        .filter_map(|(node, count)| (*count == 0).then_some(*node))
        .collect::<VecDeque<_>>();
    let mut visited = 0;
    while let Some(node) = queue.pop_front() {
        visited += 1;
        if let Some(targets) = outgoing.get(node) {
            for target in targets {
                if let Some(count) = indegree.get_mut(target) {
                    *count -= 1;
                    if *count == 0 {
                        queue.push_back(target);
                    }
                }
            }
        }
    }
    visited != indegree.len()
}

fn sanitize_id(id: &str) -> String {
    id.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn error(
    code: impl Into<String>,
    path: impl Into<String>,
    message: impl Into<String>,
) -> WorkflowDiagnostic {
    WorkflowDiagnostic {
        severity: DiagnosticSeverity::Error,
        code: code.into(),
        path: path.into(),
        message: message.into(),
        suggested_fix: None,
    }
}

fn warning(
    code: impl Into<String>,
    path: impl Into<String>,
    message: impl Into<String>,
) -> WorkflowDiagnostic {
    WorkflowDiagnostic {
        severity: DiagnosticSeverity::Warning,
        code: code.into(),
        path: path.into(),
        message: message.into(),
        suggested_fix: None,
    }
}

#[cfg(test)]
mod tests;

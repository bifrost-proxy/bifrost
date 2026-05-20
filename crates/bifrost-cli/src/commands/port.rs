use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use bifrost_admin::{
    RuleSetRef, TemporaryPortActiveSummary, TemporaryPortBindRequest, TemporaryPortBinding,
    TemporaryPortError, TemporaryPortManager, TemporaryPortRuleItem, TemporaryPortStatus,
    TemporaryPortUpdateRequest,
};
use bifrost_core::{Rule, RuleParser, ValueStore};
use bifrost_proxy::{ProxyConfig, TlsConfig};
use bifrost_storage::{RuleFile, RulesStorage};
use parking_lot::RwLock;
use tokio::task::JoinHandle;

use crate::cli::PortCommands;
use crate::commands::start::spawn_managed_proxy_task;
use crate::parsing::{DynamicRulesResolver, SharedDynamicRulesResolver};

struct BindingRuntime {
    binding: TemporaryPortBinding,
    resolver: SharedDynamicRulesResolver,
    task: JoinHandle<bifrost_core::Result<()>>,
}

struct BoundRulesLoad {
    rules: Vec<Rule>,
    values: HashMap<String, String>,
    missing: Vec<RuleSetRef>,
}

pub(crate) struct CliTemporaryPortManager {
    base_config: ProxyConfig,
    tls_config: Arc<TlsConfig>,
    admin_state: Arc<bifrost_admin::AdminState>,
    push_manager: Arc<bifrost_admin::PushManager>,
    access_control: bifrost_admin::SharedAccessControl,
    rules_storage: RulesStorage,
    values_storage: bifrost_admin::SharedValuesStorage,
    bindings: RwLock<HashMap<u16, BindingRuntime>>,
}

impl CliTemporaryPortManager {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        base_config: ProxyConfig,
        tls_config: Arc<TlsConfig>,
        admin_state: Arc<bifrost_admin::AdminState>,
        push_manager: Arc<bifrost_admin::PushManager>,
        access_control: bifrost_admin::SharedAccessControl,
        rules_storage: RulesStorage,
        values_storage: bifrost_admin::SharedValuesStorage,
    ) -> Self {
        Self {
            base_config,
            tls_config,
            admin_state,
            push_manager,
            access_control,
            rules_storage,
            values_storage,
            bindings: RwLock::new(HashMap::new()),
        }
    }

    pub(crate) async fn reload_all(&self) {
        let ports: Vec<u16> = self.bindings.read().keys().copied().collect();
        for port in ports {
            if let Err(error) = self.reload_binding(port).await {
                tracing::warn!(
                    target: "bifrost_cli::temp_ports",
                    port,
                    error = %error.message,
                    "failed to reload temporary port binding"
                );
            }
        }
    }

    async fn reload_binding(&self, port: u16) -> Result<(), TemporaryPortError> {
        let rule_refs = {
            let bindings = self.bindings.read();
            let runtime = bindings
                .get(&port)
                .ok_or_else(|| TemporaryPortError::not_found(format!("Port {port} not found")))?;
            runtime.binding.rule_refs.clone()
        };
        let loaded = self.load_bound_rules(&rule_refs, false)?;
        let mut bindings = self.bindings.write();
        let runtime = bindings
            .get_mut(&port)
            .ok_or_else(|| TemporaryPortError::not_found(format!("Port {port} not found")))?;
        runtime
            .resolver
            .update_stored_rules(loaded.rules, loaded.values);
        runtime.binding.status = if loaded.missing.is_empty() {
            TemporaryPortStatus::Running
        } else {
            TemporaryPortStatus::Degraded
        };
        runtime.binding.missing_refs = loaded.missing;
        runtime.binding.updated_at = chrono::Utc::now().timestamp();
        Ok(())
    }

    fn load_bound_rules(
        &self,
        refs: &[RuleSetRef],
        fail_on_missing: bool,
    ) -> Result<BoundRulesLoad, TemporaryPortError> {
        if refs.is_empty() {
            return Err(TemporaryPortError::bad_request(
                "At least one --rule or --group-rule must be provided",
            ));
        }

        let mut all_rules = Vec::new();
        let mut inline_values = HashMap::new();
        let mut missing = Vec::new();

        for rule_ref in refs {
            match self.load_rule_file(rule_ref) {
                Ok((rule_file, _group_id, _group_name)) => {
                    let parser = RuleParser::new();
                    let (result, file_inline_values) =
                        parser.parse_rules_tolerant_with_inline_values(&rule_file.content);
                    if !result.errors.is_empty() {
                        let first = &result.errors[0];
                        return Err(TemporaryPortError::bad_request(format!(
                            "Failed to parse rule '{}': line {}, column {}: {}",
                            rule_file.name, first.line, first.start_column, first.message
                        )));
                    }
                    for mut rule in result.rules {
                        rule.file = Some(match rule_ref {
                            RuleSetRef::LocalRule { name } => format!("local:{name}"),
                            RuleSetRef::GroupRule { group_id, name } => {
                                format!("group:{group_id}/{name}")
                            }
                            RuleSetRef::RuleFile { path } => format!("file:{path}"),
                            RuleSetRef::InlineRule { content } => inline_rule_label(content),
                        });
                        all_rules.push(rule);
                    }
                    for (k, v) in file_inline_values {
                        inline_values.entry(k).or_insert(v);
                    }
                }
                Err(error) => {
                    if fail_on_missing {
                        return Err(error);
                    }
                    missing.push(rule_ref.clone());
                }
            }
        }

        if all_rules.is_empty() {
            return Err(TemporaryPortError::bad_request(
                "No bound rules could be loaded",
            ));
        }

        let mut values = self.values_storage.read().as_hashmap();
        for (k, v) in inline_values {
            values.entry(k).or_insert(v);
        }

        Ok(BoundRulesLoad {
            rules: all_rules,
            values,
            missing,
        })
    }

    fn load_rule_file(
        &self,
        rule_ref: &RuleSetRef,
    ) -> Result<(RuleFile, Option<String>, Option<String>), TemporaryPortError> {
        match rule_ref {
            RuleSetRef::LocalRule { name } => self
                .rules_storage
                .load(name)
                .map(|rule| (rule, None, None))
                .map_err(|e| {
                    TemporaryPortError::bad_request(format!("Local rule '{name}' not found: {e}"))
                }),
            RuleSetRef::GroupRule { group_id, name } => {
                let group_dir = self
                    .admin_state
                    .group_name_cache()
                    .get(group_id)
                    .unwrap_or_else(|| group_id.clone());
                let group_storage =
                    RulesStorage::with_dir(self.rules_storage.base_dir().join(&group_dir))
                        .map_err(|e| {
                            TemporaryPortError::bad_request(format!(
                                "Group rule directory for '{group_id}' not available: {e}"
                            ))
                        })?;
                group_storage
                    .load(name)
                    .map(|rule| (rule, Some(group_id.clone()), Some(group_dir)))
                    .map_err(|e| {
                        TemporaryPortError::bad_request(format!(
                            "Group rule '{group_id}/{name}' not found: {e}"
                        ))
                    })
            }
            RuleSetRef::RuleFile { path } => {
                let content = std::fs::read_to_string(path).map_err(|e| {
                    TemporaryPortError::bad_request(format!(
                        "Rule file '{path}' could not be read: {e}"
                    ))
                })?;
                let name = Path::new(path)
                    .file_name()
                    .and_then(|value| value.to_str())
                    .filter(|value| !value.is_empty())
                    .unwrap_or(path)
                    .to_string();
                Ok((RuleFile::new(format!("file:{name}"), content), None, None))
            }
            RuleSetRef::InlineRule { content } => {
                if content.trim().is_empty() {
                    return Err(TemporaryPortError::bad_request(
                        "Inline rule content is empty",
                    ));
                }
                Ok((
                    RuleFile::new(inline_rule_label(content), content.clone()),
                    None,
                    None,
                ))
            }
        }
    }

    fn build_config(&self, port: u16, host: String) -> ProxyConfig {
        let mut config = self.base_config.clone();
        config.port = port;
        config.host = host;
        config.socks5_port = None;
        config
    }

    fn binding_active_summary(
        &self,
        binding: &TemporaryPortBinding,
    ) -> Result<TemporaryPortActiveSummary, TemporaryPortError> {
        let mut content_parts = Vec::new();
        let mut rules = Vec::new();
        for rule_ref in &binding.rule_refs {
            let (rule_file, group_id, group_name) = self.load_rule_file(rule_ref)?;
            content_parts.push(rule_file.content.clone());
            rules.push(TemporaryPortRuleItem {
                name: rule_file.name,
                rule_count: count_rules(&rule_file.content),
                group_id,
                group_name,
                content: Some(rule_file.content),
            });
        }
        Ok(TemporaryPortActiveSummary {
            port: binding.port,
            total: rules.len(),
            rules,
            merged_content: content_parts.join("\n"),
        })
    }
}

#[async_trait]
impl TemporaryPortManager for CliTemporaryPortManager {
    async fn bind(
        &self,
        req: TemporaryPortBindRequest,
    ) -> Result<TemporaryPortBinding, TemporaryPortError> {
        let port = if req.port == 0 {
            pick_available_port(req.host.as_deref().unwrap_or(&self.base_config.host))?
        } else {
            req.port
        };
        if port == self.base_config.port {
            return Err(TemporaryPortError::conflict(format!(
                "Port {port} is the main proxy port"
            )));
        }
        if self.bindings.read().contains_key(&port) {
            return Err(TemporaryPortError::conflict(format!(
                "Temporary port {port} already exists"
            )));
        }
        let host = req.host.unwrap_or_else(|| self.base_config.host.clone());
        let loaded = self.load_bound_rules(&req.rule_refs, true)?;
        let resolver: SharedDynamicRulesResolver = Arc::new(DynamicRulesResolver::new(
            Vec::new(),
            loaded.rules,
            loaded.values,
        ));
        let config = self.build_config(port, host.clone());
        let task = spawn_managed_proxy_task(
            config,
            resolver.clone(),
            self.tls_config.clone(),
            self.admin_state.clone(),
            self.push_manager.clone(),
            self.access_control.clone(),
        )
        .await
        .map_err(|e| TemporaryPortError::internal(format!("Failed to start port {port}: {e}")))?;

        let now = chrono::Utc::now().timestamp();
        let binding = TemporaryPortBinding {
            port,
            host,
            name: req.name,
            status: if loaded.missing.is_empty() {
                TemporaryPortStatus::Running
            } else {
                TemporaryPortStatus::Degraded
            },
            rule_refs: req.rule_refs,
            missing_refs: loaded.missing,
            created_at: now,
            updated_at: now,
        };

        self.bindings.write().insert(
            port,
            BindingRuntime {
                binding: binding.clone(),
                resolver,
                task,
            },
        );
        Ok(binding)
    }

    async fn update(
        &self,
        port: u16,
        req: TemporaryPortUpdateRequest,
    ) -> Result<TemporaryPortBinding, TemporaryPortError> {
        let rule_refs = {
            let bindings = self.bindings.read();
            let runtime = bindings
                .get(&port)
                .ok_or_else(|| TemporaryPortError::not_found(format!("Port {port} not found")))?;
            req.rule_refs
                .clone()
                .unwrap_or_else(|| runtime.binding.rule_refs.clone())
        };
        let loaded = self.load_bound_rules(&rule_refs, true)?;
        let mut bindings = self.bindings.write();
        let runtime = bindings
            .get_mut(&port)
            .ok_or_else(|| TemporaryPortError::not_found(format!("Port {port} not found")))?;
        runtime
            .resolver
            .update_stored_rules(loaded.rules, loaded.values);
        if req.name.is_some() {
            runtime.binding.name = req.name;
        }
        runtime.binding.rule_refs = rule_refs;
        runtime.binding.missing_refs = loaded.missing;
        runtime.binding.status = TemporaryPortStatus::Running;
        runtime.binding.updated_at = chrono::Utc::now().timestamp();
        Ok(runtime.binding.clone())
    }

    async fn destroy(&self, port: u16) -> Result<TemporaryPortBinding, TemporaryPortError> {
        let runtime = self
            .bindings
            .write()
            .remove(&port)
            .ok_or_else(|| TemporaryPortError::not_found(format!("Port {port} not found")))?;
        runtime.task.abort();
        Ok(runtime.binding)
    }

    async fn list(&self) -> Vec<TemporaryPortBinding> {
        let mut bindings: Vec<_> = self
            .bindings
            .read()
            .values()
            .map(|runtime| runtime.binding.clone())
            .collect();
        bindings.sort_by_key(|binding| binding.port);
        bindings
    }

    async fn show(&self, port: u16) -> Result<TemporaryPortBinding, TemporaryPortError> {
        self.bindings
            .read()
            .get(&port)
            .map(|runtime| runtime.binding.clone())
            .ok_or_else(|| TemporaryPortError::not_found(format!("Port {port} not found")))
    }

    async fn active_summary(
        &self,
        port: u16,
    ) -> Result<TemporaryPortActiveSummary, TemporaryPortError> {
        let binding = self.show(port).await?;
        self.binding_active_summary(&binding)
    }
}

pub fn handle_port_command(action: PortCommands) -> bifrost_core::Result<()> {
    let port = crate::process::read_runtime_port().unwrap_or(9900);
    let client = PortApiClient::new(port);
    match action {
        PortCommands::Bind {
            port,
            host,
            name,
            rules,
            rule_files,
            rule_texts,
            group_rules,
        } => {
            let req = TemporaryPortBindRequest {
                port,
                host,
                name,
                rule_refs: build_rule_refs(rules, rule_files, rule_texts, group_rules)?,
            };
            print_binding(&client.bind(&req)?);
        }
        PortCommands::List => {
            let bindings = client.list()?;
            if bindings.is_empty() {
                println!("No temporary ports.");
            } else {
                for binding in bindings {
                    print_binding(&binding);
                }
            }
        }
        PortCommands::Show { port } => print_binding(&client.show(port)?),
        PortCommands::Active { port } => print_active_summary(&client.active(port)?),
        PortCommands::Update {
            port,
            name,
            rules,
            rule_files,
            rule_texts,
            group_rules,
        } => {
            let req = TemporaryPortUpdateRequest {
                name,
                rule_refs: Some(build_rule_refs(rules, rule_files, rule_texts, group_rules)?),
            };
            print_binding(&client.update(port, &req)?);
        }
        PortCommands::Destroy { port } => {
            client.destroy(port)?;
            println!("Temporary port {port} destroyed.");
        }
    }
    Ok(())
}

fn build_rule_refs(
    rules: Vec<String>,
    rule_files: Vec<std::path::PathBuf>,
    rule_texts: Vec<String>,
    group_rules: Vec<String>,
) -> bifrost_core::Result<Vec<RuleSetRef>> {
    let mut refs = Vec::new();
    refs.extend(rules.into_iter().map(|name| RuleSetRef::LocalRule { name }));
    refs.extend(rule_files.into_iter().map(|path| RuleSetRef::RuleFile {
        path: path.to_string_lossy().to_string(),
    }));
    refs.extend(
        rule_texts
            .into_iter()
            .map(|content| RuleSetRef::InlineRule { content }),
    );
    for value in group_rules {
        let (group_id, name) = value.split_once('/').ok_or_else(|| {
            bifrost_core::BifrostError::Config(format!(
                "Invalid group rule reference '{value}', expected <group_id>/<rule_name>"
            ))
        })?;
        if group_id.is_empty() || name.is_empty() {
            return Err(bifrost_core::BifrostError::Config(format!(
                "Invalid group rule reference '{value}', expected <group_id>/<rule_name>"
            )));
        }
        refs.push(RuleSetRef::GroupRule {
            group_id: group_id.to_string(),
            name: name.to_string(),
        });
    }
    if refs.is_empty() {
        return Err(bifrost_core::BifrostError::Config(
            "At least one --rule, --rule-file, --rule-text, or --group-rule must be provided"
                .to_string(),
        ));
    }
    Ok(refs)
}

fn inline_rule_label(content: &str) -> String {
    let preview: String = content
        .split_whitespace()
        .take(3)
        .collect::<Vec<_>>()
        .join(" ");
    if preview.is_empty() {
        "inline:<empty>".to_string()
    } else {
        let mut label = preview;
        if label.len() > 48 {
            label.truncate(48);
        }
        format!("inline:{label}")
    }
}

fn count_rules(content: &str) -> usize {
    content
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.starts_with('#') && !trimmed.starts_with("```")
        })
        .count()
}

fn pick_available_port(host: &str) -> Result<u16, TemporaryPortError> {
    std::net::TcpListener::bind((host, 0))
        .and_then(|listener| listener.local_addr())
        .map(|addr| addr.port())
        .map_err(|e| TemporaryPortError::internal(format!("Failed to pick available port: {e}")))
}

fn print_binding(binding: &TemporaryPortBinding) {
    println!("Temporary port: {}:{}", binding.host, binding.port);
    if let Some(name) = &binding.name {
        println!("Name: {name}");
    }
    println!("Status: {:?}", binding.status);
    println!("Rules:");
    for rule_ref in &binding.rule_refs {
        match rule_ref {
            RuleSetRef::LocalRule { name } => println!("  - local:{name}"),
            RuleSetRef::GroupRule { group_id, name } => println!("  - group:{group_id}/{name}"),
            RuleSetRef::RuleFile { path } => println!("  - file:{path}"),
            RuleSetRef::InlineRule { content } => println!("  - {}", inline_rule_label(content)),
        }
    }
}

fn print_active_summary(summary: &TemporaryPortActiveSummary) {
    println!("Temporary Port Active Rules: {}", summary.port);
    println!("Total bound rule file(s): {}", summary.total);
    for rule in &summary.rules {
        match (&rule.group_id, &rule.group_name) {
            (Some(group_id), Some(group_name)) => {
                println!("  - {}/{} ({})", group_id, rule.name, group_name)
            }
            _ => println!("  - {} ({} rules)", rule.name, rule.rule_count),
        }
    }
    println!("Merged Rules");
    println!("------------");
    if summary.merged_content.trim().is_empty() {
        println!("(empty)");
    } else {
        println!("{}", summary.merged_content.trim());
    }
}

struct PortApiClient {
    admin_port: u16,
}

impl PortApiClient {
    fn new(admin_port: u16) -> Self {
        Self { admin_port }
    }

    fn bind(&self, req: &TemporaryPortBindRequest) -> bifrost_core::Result<TemporaryPortBinding> {
        self.post_json("/_bifrost/api/ports", req)
    }

    fn update(
        &self,
        port: u16,
        req: &TemporaryPortUpdateRequest,
    ) -> bifrost_core::Result<TemporaryPortBinding> {
        self.patch_json(&format!("/_bifrost/api/ports/{port}"), req)
    }

    fn list(&self) -> bifrost_core::Result<Vec<TemporaryPortBinding>> {
        self.get_json("/_bifrost/api/ports")
    }

    fn show(&self, port: u16) -> bifrost_core::Result<TemporaryPortBinding> {
        self.get_json(&format!("/_bifrost/api/ports/{port}"))
    }

    fn active(&self, port: u16) -> bifrost_core::Result<TemporaryPortActiveSummary> {
        self.get_json(&format!("/_bifrost/api/ports/{port}/active-summary"))
    }

    fn destroy(&self, port: u16) -> bifrost_core::Result<()> {
        let url = self.url(&format!("/_bifrost/api/ports/{port}"));
        bifrost_core::direct_ureq_agent_builder()
            .timeout(std::time::Duration::from_secs(3))
            .build()
            .delete(&url)
            .call()
            .map_err(|e| self.api_error("DELETE", &url, e))?;
        Ok(())
    }

    fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> bifrost_core::Result<T> {
        let url = self.url(path);
        bifrost_core::direct_ureq_agent_builder()
            .timeout(std::time::Duration::from_secs(3))
            .build()
            .get(&url)
            .call()
            .map_err(|e| self.api_error("GET", &url, e))?
            .into_json()
            .map_err(|e| bifrost_core::BifrostError::Config(format!("Invalid response: {e}")))
    }

    fn post_json<T: serde::Serialize, R: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &T,
    ) -> bifrost_core::Result<R> {
        let url = self.url(path);
        bifrost_core::direct_ureq_agent_builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .post(&url)
            .send_json(serde_json::to_value(body).unwrap_or_default())
            .map_err(|e| self.api_error("POST", &url, e))?
            .into_json()
            .map_err(|e| bifrost_core::BifrostError::Config(format!("Invalid response: {e}")))
    }

    fn patch_json<T: serde::Serialize, R: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &T,
    ) -> bifrost_core::Result<R> {
        let url = self.url(path);
        bifrost_core::direct_ureq_agent_builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .patch(&url)
            .send_json(serde_json::to_value(body).unwrap_or_default())
            .map_err(|e| self.api_error("PATCH", &url, e))?
            .into_json()
            .map_err(|e| bifrost_core::BifrostError::Config(format!("Invalid response: {e}")))
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.admin_port, path)
    }

    fn api_error(&self, method: &str, url: &str, error: ureq::Error) -> bifrost_core::BifrostError {
        if let ureq::Error::Status(status, response) = error {
            let body = response.into_string().unwrap_or_default();
            let message = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|value| {
                    value
                        .get("error")
                        .and_then(|error| error.as_str())
                        .map(ToString::to_string)
                })
                .filter(|message| !message.is_empty())
                .unwrap_or_else(|| body.trim().to_string());
            return bifrost_core::BifrostError::Config(format!(
                "{method} {url} failed with status {status}: {message}"
            ));
        }
        bifrost_core::BifrostError::Config(format!(
            "{method} {url} failed: {error}. Is the main Bifrost proxy running?"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_port_binding_group_rule_ref() {
        let refs = build_rule_refs(
            vec!["local-a".to_string()],
            Vec::new(),
            Vec::new(),
            vec!["group-1/rule-a".to_string()],
        )
        .unwrap();
        assert_eq!(
            refs,
            vec![
                RuleSetRef::LocalRule {
                    name: "local-a".to_string()
                },
                RuleSetRef::GroupRule {
                    group_id: "group-1".to_string(),
                    name: "rule-a".to_string()
                }
            ]
        );
    }

    #[test]
    fn test_parse_port_binding_group_rule_ref_rejects_missing_slash() {
        let error = build_rule_refs(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec!["broken".to_string()],
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("expected <group_id>/<rule_name>"));
    }

    #[test]
    fn test_parse_port_binding_file_and_inline_refs() {
        let refs = build_rule_refs(
            Vec::new(),
            vec![std::path::PathBuf::from("/tmp/rules.txt")],
            vec!["inline.test status://218 resBody://(inline)".to_string()],
            Vec::new(),
        )
        .unwrap();
        assert_eq!(
            refs,
            vec![
                RuleSetRef::RuleFile {
                    path: "/tmp/rules.txt".to_string()
                },
                RuleSetRef::InlineRule {
                    content: "inline.test status://218 resBody://(inline)".to_string()
                }
            ]
        );
    }

    #[test]
    fn test_parse_port_binding_requires_any_rule_input() {
        let error = build_rule_refs(Vec::new(), Vec::new(), Vec::new(), Vec::new()).unwrap_err();
        assert!(error.to_string().contains("--rule-file"));
        assert!(error.to_string().contains("--rule-text"));
    }
}

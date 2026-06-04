use crate::config::{
    agent_home_dir, builtin_provider_ids, get_builtin_provider, AgentConfig, ModelProviderConfig,
    CONFIG_FILENAME,
};
use std::collections::HashSet;

impl AgentConfig {
    pub fn preflight_model_config(&self) -> Result<(), String> {
        let (provider_id, provider) = self.resolve_model_provider_config();
        let provider_label = provider
            .name
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(provider_id.as_str());
        let mut issues = Vec::new();

        if self
            .model
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_none()
        {
            issues.push("未配置模型名称 `model`。".to_string());
        }

        if provider
            .base_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_none()
        {
            issues.push(format!(
                "模型 Provider `{provider_id}` 缺少 `base_url`，无法知道请求发往哪里。"
            ));
        }

        let mut reported_env = HashSet::new();
        let has_static_auth_header = has_non_empty_static_auth_header(&provider);
        if let Some(issue) = missing_api_key_issue(
            &provider,
            provider_label,
            has_static_auth_header,
            &mut reported_env,
        ) {
            issues.push(issue);
        }
        for issue in
            missing_auth_header_issues(&provider, has_static_auth_header, &mut reported_env)
        {
            issues.push(issue);
        }

        if issues.is_empty() {
            return Ok(());
        }

        Err(format_model_config_guidance(provider_id.as_str(), issues))
    }

    pub(crate) fn resolve_model_provider_config(&self) -> (String, ModelProviderConfig) {
        let provider_id = self
            .model_provider
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("aidp_crawl")
            .to_string();

        let builtin = if builtin_provider_ids().contains(&provider_id.as_str()) {
            get_builtin_provider(&provider_id)
        } else {
            empty_provider_fallback()
        };
        let provider = if let Some(user_provider) = self.model_providers.get(&provider_id) {
            ModelProviderConfig {
                name: user_provider.name.clone().or(builtin.name),
                base_url: user_provider.base_url.clone().or(builtin.base_url),
                wire_api: user_provider.wire_api.or(builtin.wire_api),
                env_key: user_provider.env_key.clone().or(builtin.env_key),
                api_key: user_provider.api_key.clone().or(builtin.api_key),
                http_headers: user_provider.http_headers.clone().or(builtin.http_headers),
                env_http_headers: user_provider
                    .env_http_headers
                    .clone()
                    .or(builtin.env_http_headers),
                request_max_retries: user_provider
                    .request_max_retries
                    .or(builtin.request_max_retries),
                stream_idle_timeout_ms: user_provider
                    .stream_idle_timeout_ms
                    .or(builtin.stream_idle_timeout_ms),
                stream_max_retries: user_provider
                    .stream_max_retries
                    .or(builtin.stream_max_retries),
            }
        } else {
            builtin
        };
        (provider_id, provider)
    }
}

fn empty_provider_fallback() -> ModelProviderConfig {
    ModelProviderConfig {
        name: None,
        base_url: None,
        wire_api: None,
        env_key: None,
        api_key: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: None,
        stream_idle_timeout_ms: None,
        stream_max_retries: None,
    }
}

fn missing_api_key_issue(
    provider: &ModelProviderConfig,
    provider_label: &str,
    has_static_auth_header: bool,
    reported_env: &mut HashSet<String>,
) -> Option<String> {
    if let Some(api_key) = provider.api_key.as_deref() {
        let trimmed = api_key.trim();
        if trimmed.is_empty() {
            return Some(format!(
                "Provider `{provider_label}` 的 `api_key` 为空。请填入有效 AK，或填 `$ENV_NAME` 从环境变量读取。"
            ));
        }
        if let Some(env_name) = trimmed.strip_prefix('$').map(str::trim) {
            if env_name.is_empty() {
                return Some(format!(
                    "Provider `{provider_label}` 的 `api_key` 环境变量引用为空。请改成 `$MODELHUB_AK` 这类有效变量名。"
                ));
            }
            if std::env::var(env_name)
                .map(|value| value.trim().is_empty())
                .unwrap_or(true)
            {
                reported_env.insert(env_name.to_string());
                return Some(format!(
                    "环境变量 `{env_name}` 未设置或为空，Provider `{provider_label}` 无法读取 AK。"
                ));
            }
        }
        return None;
    }

    if has_static_auth_header {
        return None;
    }

    let env_key = provider.env_key.as_deref()?.trim();
    if env_key.is_empty() {
        return Some(format!(
            "Provider `{provider_label}` 的 `env_key` 为空，无法定位 AK 环境变量。"
        ));
    }
    if std::env::var(env_key)
        .map(|value| value.trim().is_empty())
        .unwrap_or(true)
    {
        reported_env.insert(env_key.to_string());
        return Some(format!(
            "环境变量 `{env_key}` 未设置或为空，Provider `{provider_label}` 缺少 AK。"
        ));
    }
    None
}

fn missing_auth_header_issues(
    provider: &ModelProviderConfig,
    has_static_auth_header: bool,
    reported_env: &mut HashSet<String>,
) -> Vec<String> {
    let mut issues = Vec::new();
    let Some(headers) = provider.env_http_headers.as_ref() else {
        return issues;
    };
    for (header_name, env_name) in headers {
        if !is_auth_header(header_name) {
            continue;
        }
        if has_static_auth_header {
            continue;
        }
        let env_name = env_name.trim();
        if env_name.is_empty() || reported_env.contains(env_name) {
            continue;
        }
        if std::env::var(env_name)
            .map(|value| value.trim().is_empty())
            .unwrap_or(true)
        {
            reported_env.insert(env_name.to_string());
            issues.push(format!(
                "鉴权 Header `{header_name}` 依赖的环境变量 `{env_name}` 未设置或为空。"
            ));
        }
    }
    issues
}

fn has_non_empty_static_auth_header(provider: &ModelProviderConfig) -> bool {
    provider
        .http_headers
        .as_ref()
        .map(|headers| {
            headers
                .iter()
                .any(|(name, value)| is_auth_header(name) && !value.trim().is_empty())
        })
        .unwrap_or(false)
}

fn is_auth_header(header_name: &str) -> bool {
    matches!(
        header_name.trim().to_ascii_lowercase().as_str(),
        "authorization" | "api-key" | "x-api-key"
    )
}

fn format_model_config_guidance(provider_id: &str, issues: Vec<String>) -> String {
    let config_path = agent_home_dir().join(CONFIG_FILENAME);
    let issue_lines = issues
        .into_iter()
        .map(|issue| format!("- {issue}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "内置 Agent 模型配置不完整，暂未开始执行。\n\n缺少配置：\n{issue_lines}\n\n请到 Web UI 的 Settings → Agent → Model Configuration 配置 Model、Model Provider 和 API Key；如果使用环境变量，请在启动 Bifrost 的 shell 或服务环境中设置对应变量后重启 Bifrost。\n\n也可以直接编辑 `{}`，当前 Provider: `{provider_id}`。API Key 支持直接填写，或填写 `$ENV_NAME` 从环境变量读取。",
        config_path.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ModelWireApi;
    use std::collections::HashMap;

    fn preflight_test_config(provider: ModelProviderConfig) -> AgentConfig {
        let mut model_providers = HashMap::new();
        model_providers.insert("preflight-test".to_string(), provider);
        AgentConfig {
            model: Some("test-model".to_string()),
            model_provider: Some("preflight-test".to_string()),
            model_providers,
            ..Default::default()
        }
    }

    fn local_test_provider() -> ModelProviderConfig {
        ModelProviderConfig {
            name: Some("Local Test".to_string()),
            base_url: Some("http://127.0.0.1:12345/chat/completions".to_string()),
            wire_api: Some(ModelWireApi::ChatCompletions),
            env_key: None,
            api_key: None,
            http_headers: None,
            env_http_headers: None,
            request_max_retries: None,
            stream_idle_timeout_ms: None,
            stream_max_retries: None,
        }
    }

    #[test]
    fn test_preflight_model_config_reports_missing_model() {
        let config = AgentConfig {
            model: None,
            ..preflight_test_config(local_test_provider())
        };

        let error = config.preflight_model_config().unwrap_err();

        assert!(error.contains("内置 Agent 模型配置不完整"));
        assert!(error.contains("未配置模型名称"));
        assert!(error.contains("Settings → Agent → Model Configuration"));
        assert!(error.contains("config.toml"));
    }

    #[test]
    fn test_preflight_model_config_reports_missing_env_key() {
        let env_name = "BIFROST_AGENT_TEST_MISSING_PREFLIGHT_AK";
        std::env::remove_var(env_name);
        let mut provider = local_test_provider();
        provider.env_key = Some(env_name.to_string());
        let config = preflight_test_config(provider);

        let error = config.preflight_model_config().unwrap_err();

        assert!(error.contains(env_name));
        assert!(error.contains("缺少 AK"));
        assert!(error.contains("启动 Bifrost"));
    }

    #[test]
    fn test_preflight_model_config_allows_local_provider_without_key() {
        let config = preflight_test_config(local_test_provider());

        config.preflight_model_config().unwrap();
    }

    #[test]
    fn test_preflight_model_config_requires_auth_header_env_only() {
        let auth_env = "BIFROST_AGENT_TEST_MISSING_AUTH_HEADER_AK";
        let optional_env = "BIFROST_AGENT_TEST_OPTIONAL_LOGID";
        std::env::remove_var(auth_env);
        std::env::remove_var(optional_env);
        let mut provider = local_test_provider();
        provider.env_http_headers = Some(HashMap::from([
            ("api-key".to_string(), auth_env.to_string()),
            ("X-TT-LOGID".to_string(), optional_env.to_string()),
        ]));
        let config = preflight_test_config(provider);

        let error = config.preflight_model_config().unwrap_err();

        assert!(error.contains(auth_env));
        assert!(!error.contains(optional_env));
    }

    #[test]
    fn test_preflight_model_config_accepts_api_key_env_reference() {
        let env_name = "BIFROST_AGENT_TEST_PRESENT_PREFLIGHT_AK";
        std::env::set_var(env_name, "test-ak");
        let mut provider = local_test_provider();
        provider.api_key = Some(format!("${env_name}"));
        let config = preflight_test_config(provider);

        config.preflight_model_config().unwrap();

        std::env::remove_var(env_name);
    }

    #[test]
    fn test_preflight_model_config_accepts_static_auth_header_without_env_key() {
        std::env::remove_var("MODELHUB_AK");
        let mut model_providers = HashMap::new();
        model_providers.insert(
            "aidp_crawl".to_string(),
            ModelProviderConfig {
                http_headers: Some(HashMap::from([(
                    "api-key".to_string(),
                    "static-ak".to_string(),
                )])),
                ..empty_provider_fallback()
            },
        );
        let config = AgentConfig {
            model: Some("test-model".to_string()),
            model_provider: Some("aidp_crawl".to_string()),
            model_providers,
            ..Default::default()
        };

        config.preflight_model_config().unwrap();
    }
}

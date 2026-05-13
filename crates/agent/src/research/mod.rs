pub mod config;
pub mod digest;
pub mod fetch;
pub mod normalize;
pub mod provider;
pub mod providers;
pub mod store;
pub mod task;

use crate::config::{agent_home_dir, AgentConfig};
use anyhow::anyhow;
use config::{ResearchProviderType, ResearchSiteKind, ResearchSource};
use futures::stream::{FuturesUnordered, StreamExt};
use provider::{
    FetchedDocument, ResearchFetchRequest, ResearchProvider, ResearchProviderKind,
    ResearchSearchProviderEvent, ResearchSearchRequest, ResearchSearchResponse,
};
use providers::fixed_site::FixedSiteProvider;
use providers::generic_http::GenericHttpProvider;
use providers::sogou_wechat_cdp::SogouWechatCdpProvider;
use providers::volc_web_search::VolcWebSearchProvider;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use store::{
    item_from_input, KnowledgeItem, KnowledgeItemInput, KnowledgeSearchResult, KnowledgeStore,
};
use tokio::sync::mpsc;

pub use config::{
    Freshness, ResearchCacheConfig, ResearchConfig, ResearchDefaults, ResearchFetchPolicy,
    ResearchProviderConfig, ResearchTaskConfig, ResearchTaskTrigger, WechatResearchConfig,
};
pub use digest::{ResearchDigestRequest, ResearchDigestResponse};
pub use provider::{ResearchProviderKind as ProviderKind, ResearchSearchResult};
pub use store::{KnowledgeSaveReport, KnowledgeStoreStats};

pub struct ResearchRuntime {
    config: ResearchConfig,
    providers: HashMap<String, Arc<dyn ResearchProvider>>,
    store: Option<Arc<KnowledgeStore>>,
    http: reqwest::Client,
    reports_root: PathBuf,
}

impl ResearchRuntime {
    pub fn from_agent_config(agent_config: &AgentConfig) -> anyhow::Result<Self> {
        let config = agent_config
            .research
            .clone()
            .ok_or_else(|| anyhow!("research is not configured"))?;
        Self::from_config(config)
    }

    pub fn from_config(config: ResearchConfig) -> anyhow::Result<Self> {
        Self::from_config_with_home(config, agent_home_dir())
    }

    pub fn from_config_with_home(
        config: ResearchConfig,
        research_home: PathBuf,
    ) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(
                config.fetch_policy.timeout_secs,
            ))
            .build()?;
        let mut providers: HashMap<String, Arc<dyn ResearchProvider>> = HashMap::new();
        for (id, provider_config) in &config.providers {
            if !provider_config.enabled {
                continue;
            }
            match provider_config.provider_type {
                ResearchProviderType::GenericWebSearch
                | ResearchProviderType::Tavily
                | ResearchProviderType::Exa
                | ResearchProviderType::CustomHttp => {
                    providers.insert(
                        id.clone(),
                        Arc::new(GenericHttpProvider::new(
                            id.clone(),
                            provider_config.clone(),
                        )?),
                    );
                }
                ResearchProviderType::VolcWebSearch => {
                    providers.insert(
                        id.clone(),
                        Arc::new(VolcWebSearchProvider::new(
                            id.clone(),
                            provider_config.clone(),
                        )?),
                    );
                }
                ResearchProviderType::SogouWechatCdp => {
                    providers.insert(
                        id.clone(),
                        Arc::new(SogouWechatCdpProvider::new(
                            id.clone(),
                            provider_config.clone(),
                        )?),
                    );
                }
                ResearchProviderType::FixedSite => {
                    providers.insert(
                        id.clone(),
                        Arc::new(FixedSiteProvider::new(id.clone(), provider_config.clone())?),
                    );
                }
                ResearchProviderType::Mcp => {
                    tracing::warn!(provider = %id, "research mcp provider is reserved for a later phase");
                }
            }
        }

        let store = if config.cache.enabled && config.cache.store == "sqlite" {
            let path = config
                .cache
                .db_path
                .as_ref()
                .map(|value| expand_tilde(value))
                .unwrap_or_else(|| research_home.join("research.db"));
            let store = KnowledgeStore::new(path);
            store.init()?;
            Some(Arc::new(store))
        } else {
            None
        };
        let reports_root = research_home.join("reports");

        Ok(Self {
            config,
            providers,
            store,
            http,
            reports_root,
        })
    }

    pub fn config(&self) -> &ResearchConfig {
        &self.config
    }

    pub fn store(&self) -> Option<Arc<KnowledgeStore>> {
        self.store.clone()
    }

    pub fn provider_ids(&self) -> Vec<String> {
        let mut ids = self.providers.keys().cloned().collect::<Vec<_>>();
        ids.sort();
        ids
    }

    pub async fn search(
        &self,
        mut req: ResearchSearchRequest,
    ) -> anyhow::Result<ResearchSearchResponse> {
        if req.sources.is_empty() {
            req.sources = self.config.defaults.sources.clone();
        }
        if req.limit.is_none() {
            req.limit = Some(self.config.defaults.limit);
        }
        if req.language.is_none() {
            req.language = self.config.defaults.language.clone();
        }

        let plan = self.search_provider_plan(&req);
        let attempted_providers = plan.len();
        let events = self.search_provider_events(req.clone(), plan).await;
        let successful_providers = events.iter().filter(|event| event.error.is_none()).count();
        let provider_errors = events
            .iter()
            .filter_map(|event| {
                event
                    .error
                    .as_ref()
                    .map(|error| format!("{}: {error}", event.provider_id))
            })
            .collect::<Vec<_>>();
        let results = events
            .into_iter()
            .flat_map(|event| event.results)
            .collect::<Vec<_>>();

        if results.is_empty() && !req.provider_ids.is_empty() && attempted_providers == 0 {
            return Err(anyhow!(
                "no selected research providers are enabled/configured: {}",
                req.provider_ids.join(", ")
            ));
        }

        if results.is_empty()
            && attempted_providers > 0
            && successful_providers == 0
            && !provider_errors.is_empty()
        {
            return Err(anyhow!(
                "all selected research providers failed: {}",
                provider_errors.join("; ")
            ));
        }

        let results = normalize::dedupe_results(results);
        Ok(ResearchSearchResponse {
            query: req.query,
            results,
        })
    }

    pub async fn search_stream_events(
        &self,
        mut req: ResearchSearchRequest,
    ) -> anyhow::Result<Vec<ResearchSearchProviderEvent>> {
        if req.sources.is_empty() {
            req.sources = self.config.defaults.sources.clone();
        }
        if req.limit.is_none() {
            req.limit = Some(self.config.defaults.limit);
        }
        if req.language.is_none() {
            req.language = self.config.defaults.language.clone();
        }
        let plan = self.search_provider_plan(&req);
        if !req.provider_ids.is_empty() && plan.is_empty() {
            return Err(anyhow!(
                "no selected research providers are enabled/configured: {}",
                req.provider_ids.join(", ")
            ));
        }
        Ok(self.search_provider_events(req, plan).await)
    }

    pub async fn search_stream_channel(
        self: Arc<Self>,
        mut req: ResearchSearchRequest,
    ) -> anyhow::Result<mpsc::Receiver<ResearchSearchProviderEvent>> {
        if req.sources.is_empty() {
            req.sources = self.config.defaults.sources.clone();
        }
        if req.limit.is_none() {
            req.limit = Some(self.config.defaults.limit);
        }
        if req.language.is_none() {
            req.language = self.config.defaults.language.clone();
        }
        let plan = self.search_provider_plan(&req);
        if !req.provider_ids.is_empty() && plan.is_empty() {
            return Err(anyhow!(
                "no selected research providers are enabled/configured: {}",
                req.provider_ids.join(", ")
            ));
        }
        let (tx, rx) = mpsc::channel(plan.len().max(1));
        for provider in plan {
            let runtime = self.clone();
            let tx = tx.clone();
            let provider_id = provider.id().to_string();
            let provider_req = req.clone();
            tokio::spawn(async move {
                let mut event = match provider.search(provider_req.clone()).await {
                    Ok(response) => ResearchSearchProviderEvent {
                        provider_id,
                        results: response.results,
                        error: None,
                    },
                    Err(error) => ResearchSearchProviderEvent {
                        provider_id,
                        results: Vec::new(),
                        error: Some(error.to_string()),
                    },
                };
                if provider_req.fetch_content && !event.results.is_empty() {
                    runtime
                        .fetch_search_result_content(&mut event.results)
                        .await;
                }
                event.results = normalize::dedupe_results(event.results);
                let _ = tx.send(event).await;
            });
        }
        drop(tx);
        Ok(rx)
    }

    pub async fn fetch(&self, req: ResearchFetchRequest) -> anyhow::Result<FetchedDocument> {
        if is_wechat_or_sogou_url(&req.url) {
            for provider in self.ordered_wechat_providers() {
                if let Some(doc) = provider.fetch(req.clone()).await? {
                    return Ok(doc);
                }
            }
        }
        fetch::fetch_document(&self.http, &self.config.fetch_policy, req).await
    }

    pub fn save_knowledge(&self, items: &[KnowledgeItem]) -> anyhow::Result<KnowledgeSaveReport> {
        let Some(store) = &self.store else {
            return Err(anyhow!("research knowledge store is disabled"));
        };
        store.upsert_items(items)
    }

    pub fn search_knowledge(
        &self,
        query: &str,
        limit: usize,
        since_days: Option<u32>,
    ) -> anyhow::Result<Vec<KnowledgeSearchResult>> {
        let Some(store) = &self.store else {
            return Err(anyhow!("research knowledge store is disabled"));
        };
        store.search(query, limit, since_days)
    }

    pub fn digest(
        &self,
        task_id: Option<String>,
        date: Option<String>,
        query: Option<String>,
    ) -> anyhow::Result<ResearchDigestResponse> {
        let query_text =
            query.unwrap_or_else(|| task_id.clone().unwrap_or_else(|| "research".to_string()));
        let items = self
            .search_knowledge(&query_text, 50, Some(30))
            .unwrap_or_default();
        let task_id = task_id.unwrap_or_else(|| "manual_research".to_string());
        let date = date.unwrap_or_else(today_utc);
        digest::write_markdown_report(
            &self.reports_root,
            &task_id,
            &date,
            Some(&query_text),
            &items,
        )
    }

    pub async fn run_task(&self, task_id: &str) -> anyhow::Result<ResearchDigestResponse> {
        let task = self
            .config
            .tasks
            .iter()
            .find(|task| task.id == task_id)
            .ok_or_else(|| anyhow!("research task '{task_id}' not found"))?;
        let mut saved_items = Vec::new();
        for query in &task.queries {
            let response = self
                .search(ResearchSearchRequest {
                    query: query.clone(),
                    sources: task.sources.clone(),
                    provider_ids: Vec::new(),
                    freshness: None,
                    limit: Some(task.max_results_per_query),
                    fetch_content: task.fetch_content,
                    language: Some(task.language.clone()),
                })
                .await?;
            for result in response.results {
                let fetched = if task.fetch_content {
                    self.fetch(ResearchFetchRequest {
                        url: result.url.clone(),
                        format: "markdown".to_string(),
                        max_bytes: Some(self.config.fetch_policy.max_response_bytes),
                    })
                    .await
                    .ok()
                } else {
                    None
                };
                saved_items.push(item_from_input(KnowledgeItemInput {
                    source: source_name(&result.source).to_string(),
                    provider: result.provider,
                    query: Some(query.clone()),
                    title: fetched
                        .as_ref()
                        .and_then(|doc| doc.title.clone())
                        .unwrap_or(result.title),
                    url: fetched
                        .as_ref()
                        .map(|doc| doc.url.clone())
                        .unwrap_or(result.url),
                    author: fetched
                        .as_ref()
                        .and_then(|doc| doc.author.clone())
                        .or(result.author),
                    published_at: fetched
                        .as_ref()
                        .and_then(|doc| doc.published_at.clone())
                        .or(result.published_at),
                    content_markdown: fetched
                        .as_ref()
                        .map(|doc| doc.content_markdown.clone())
                        .or(result.content_markdown),
                    summary: result.snippet,
                    tags: vec!["research-task".to_string()],
                }));
            }
        }
        if !saved_items.is_empty() {
            let _ = self.save_knowledge(&saved_items)?;
        }
        self.digest(
            Some(task.id.clone()),
            Some(today_utc()),
            task.queries.first().cloned(),
        )
    }

    pub fn stats(&self) -> anyhow::Result<Option<KnowledgeStoreStats>> {
        self.store.as_ref().map(|store| store.stats()).transpose()
    }

    fn ordered_providers(&self, kind: ResearchProviderKind) -> Vec<Arc<dyn ResearchProvider>> {
        let mut providers = Vec::new();
        for id in &self.config.provider_order {
            if let Some(provider) = self.providers.get(id) {
                if provider.kind() == kind {
                    providers.push(provider.clone());
                }
            }
        }
        let mut remaining = self
            .providers
            .values()
            .filter(|provider| provider.kind() == kind)
            .filter(|provider| !providers.iter().any(|p| p.id() == provider.id()))
            .cloned()
            .collect::<Vec<_>>();
        remaining.sort_by(|left, right| left.id().cmp(right.id()));
        providers.extend(remaining);
        providers
    }

    fn ordered_web_providers(&self) -> Vec<Arc<dyn ResearchProvider>> {
        let mut providers = Vec::new();
        for id in &self.config.provider_order {
            if let Some(provider) = self.providers.get(id) {
                if matches!(
                    provider.kind(),
                    ResearchProviderKind::FixedSite | ResearchProviderKind::GenericWebSearch
                ) {
                    providers.push(provider.clone());
                }
            }
        }
        let mut fixed_site = self
            .ordered_providers(ResearchProviderKind::FixedSite)
            .into_iter()
            .filter(|provider| !providers.iter().any(|p| p.id() == provider.id()))
            .collect::<Vec<_>>();
        let generic = self
            .ordered_providers(ResearchProviderKind::GenericWebSearch)
            .into_iter()
            .filter(|provider| !providers.iter().any(|p| p.id() == provider.id()))
            .collect::<Vec<_>>();
        providers.append(&mut fixed_site);
        providers.extend(generic);
        providers
    }

    fn search_provider_plan(&self, req: &ResearchSearchRequest) -> Vec<Arc<dyn ResearchProvider>> {
        let provider_selected = |provider: &Arc<dyn ResearchProvider>| {
            req.provider_ids.is_empty()
                || req
                    .provider_ids
                    .iter()
                    .any(|selected| selected == provider.id())
        };
        let mut providers = Vec::new();
        if req
            .sources
            .iter()
            .any(|source| source == &ResearchSource::Web)
        {
            providers.extend(
                self.ordered_web_providers()
                    .into_iter()
                    .filter(provider_selected),
            );
        }
        if req
            .sources
            .iter()
            .any(|source| source == &ResearchSource::Wechat)
        {
            providers.extend(
                self.ordered_wechat_providers()
                    .into_iter()
                    .filter(provider_selected),
            );
        }
        providers
    }

    async fn search_provider_events(
        &self,
        req: ResearchSearchRequest,
        providers: Vec<Arc<dyn ResearchProvider>>,
    ) -> Vec<ResearchSearchProviderEvent> {
        let mut searches = FuturesUnordered::new();
        for provider in providers {
            let provider_id = provider.id().to_string();
            let provider_req = req.clone();
            searches.push(async move {
                match provider.search(provider_req).await {
                    Ok(response) => ResearchSearchProviderEvent {
                        provider_id,
                        results: response.results,
                        error: None,
                    },
                    Err(error) => {
                        tracing::warn!(
                            provider = %provider_id,
                            error = %error,
                            "research provider failed; continuing with remaining providers"
                        );
                        ResearchSearchProviderEvent {
                            provider_id,
                            results: Vec::new(),
                            error: Some(error.to_string()),
                        }
                    }
                }
            });
        }
        let mut events = Vec::new();
        while let Some(mut event) = searches.next().await {
            if req.fetch_content && !event.results.is_empty() {
                self.fetch_search_result_content(&mut event.results).await;
            }
            event.results = normalize::dedupe_results(event.results);
            events.push(event);
        }
        events
    }

    async fn fetch_search_result_content(&self, results: &mut [ResearchSearchResult]) {
        for result in results {
            if result.content_markdown.is_some() {
                continue;
            }
            let Ok(doc) = self
                .fetch(ResearchFetchRequest {
                    url: result.url.clone(),
                    format: "markdown".to_string(),
                    max_bytes: Some(self.config.fetch_policy.max_response_bytes),
                })
                .await
            else {
                continue;
            };
            result.canonical_url = Some(doc.canonical_url);
            result.content_hash = Some(doc.content_hash);
            result.content_markdown = Some(doc.content_markdown);
            result.retrieved_at = Some(doc.retrieved_at);
            if result.author.is_none() {
                result.author = doc.author;
            }
            if result.published_at.is_none() {
                result.published_at = doc.published_at;
            }
            if result.site_name.is_none() {
                result.site_name = doc.site_name;
            }
        }
    }

    fn ordered_wechat_providers(&self) -> Vec<Arc<dyn ResearchProvider>> {
        let kinds = [ResearchProviderKind::BrowserCdp];
        let mut providers = Vec::new();
        for id in &self.config.provider_order {
            if let Some(provider) = self.providers.get(id) {
                if kinds.contains(&provider.kind()) {
                    providers.push(provider.clone());
                }
            }
        }
        let mut remaining = self
            .providers
            .values()
            .filter(|provider| kinds.contains(&provider.kind()))
            .filter(|provider| !providers.iter().any(|p| p.id() == provider.id()))
            .cloned()
            .collect::<Vec<_>>();
        remaining.sort_by(|left, right| left.id().cmp(right.id()));
        providers.extend(remaining);
        providers
    }
}

pub fn default_enabled_config() -> ResearchConfig {
    let mut config = ResearchConfig {
        enabled: true,
        preset: Some("personal-cn".to_string()),
        ..ResearchConfig::default()
    };
    apply_preset(&mut config, "personal-cn");
    config
}

pub fn apply_preset(config: &mut ResearchConfig, preset: &str) {
    config.preset = Some(preset.to_string());
    match preset {
        "personal-cn" | "ai-tech" => {
            add_fixed_site_provider(config, "arxiv", ResearchSiteKind::Arxiv);
            add_fixed_site_provider(config, "hacker_news", ResearchSiteKind::HackerNews);
            add_fixed_site_provider(
                config,
                "github_repositories",
                ResearchSiteKind::GithubRepositories,
            );
            add_volc_web_search_provider(config);
            add_sogou_wechat_cdp_provider(config);
        }
        _ => {}
    }
}

fn add_fixed_site_provider(config: &mut ResearchConfig, id: &str, site: ResearchSiteKind) {
    config
        .providers
        .entry(id.to_string())
        .or_insert_with(|| ResearchProviderConfig {
            provider_type: ResearchProviderType::FixedSite,
            site: Some(site),
            ..Default::default()
        });
    let id = id.to_string();
    if !config.provider_order.contains(&id) {
        config.provider_order.push(id);
    }
}

fn add_sogou_wechat_cdp_provider(config: &mut ResearchConfig) {
    let id = "sogou_wechat_cdp".to_string();
    config
        .providers
        .entry(id.clone())
        .or_insert_with(|| ResearchProviderConfig {
            provider_type: ResearchProviderType::SogouWechatCdp,
            ..Default::default()
        });
    if !config.provider_order.contains(&id) {
        config.provider_order.push(id.clone());
    }
    let wechat = config.wechat.get_or_insert_with(Default::default);
    wechat.enabled = true;
    wechat.provider.get_or_insert(id);
}

fn add_volc_web_search_provider(config: &mut ResearchConfig) {
    let id = "volc_web_search".to_string();
    config
        .providers
        .entry(id.clone())
        .or_insert_with(|| ResearchProviderConfig {
            enabled: false,
            provider_type: ResearchProviderType::VolcWebSearch,
            base_url: Some("https://open.feedcoopapi.com/search_api/web_search".to_string()),
            env_key: Some("ARK_TOKEN".to_string()),
            search_type: Some("web".to_string()),
            count: Some(10),
            need_content: Some(true),
            need_url: Some(true),
            need_summary: Some(false),
            content_formats: Some("markdown".to_string()),
            query_rewrite: Some(false),
            ..Default::default()
        });
    if !config.provider_order.contains(&id) {
        config.provider_order.push(id);
    }
}

fn source_name(source: &ResearchSource) -> &'static str {
    match source {
        ResearchSource::Web => "web",
        ResearchSource::Wechat => "wechat",
    }
}

fn is_wechat_or_sogou_url(raw: &str) -> bool {
    let Ok(url) = url::Url::parse(raw) else {
        return false;
    };
    url.host_str().is_some_and(|host| {
        host == "mp.weixin.qq.com"
            || host == "weixin.sogou.com"
            || host.ends_with(".mp.weixin.qq.com")
            || host.ends_with(".weixin.sogou.com")
    })
}

fn expand_tilde(value: &str) -> PathBuf {
    if let Some(rest) = value.strip_prefix("~/") {
        return crate::config::user_home_dir().join(rest);
    }
    PathBuf::from(value)
}

fn today_utc() -> String {
    let days = store::now_unix() / 86_400;
    // Civil date conversion, enough for report naming without pulling chrono into the agent crate.
    civil_from_days(days)
}

fn civil_from_days(days_since_epoch: i64) -> String {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if m <= 2 { 1 } else { 0 };
    format!("{year:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::research::config::ResearchProviderConfig;
    use async_trait::async_trait;

    struct EmptyOkProvider(&'static str);

    #[async_trait]
    impl ResearchProvider for EmptyOkProvider {
        fn id(&self) -> &str {
            self.0
        }

        fn kind(&self) -> ResearchProviderKind {
            ResearchProviderKind::GenericWebSearch
        }

        async fn search(
            &self,
            req: ResearchSearchRequest,
        ) -> anyhow::Result<ResearchSearchResponse> {
            Ok(ResearchSearchResponse {
                query: req.query,
                results: Vec::new(),
            })
        }
    }

    struct FailingProvider(&'static str);

    #[async_trait]
    impl ResearchProvider for FailingProvider {
        fn id(&self) -> &str {
            self.0
        }

        fn kind(&self) -> ResearchProviderKind {
            ResearchProviderKind::GenericWebSearch
        }

        async fn search(
            &self,
            _req: ResearchSearchRequest,
        ) -> anyhow::Result<ResearchSearchResponse> {
            Err(anyhow!("mock provider failure"))
        }
    }

    struct CountingProvider(&'static str, usize);

    #[async_trait]
    impl ResearchProvider for CountingProvider {
        fn id(&self) -> &str {
            self.0
        }

        fn kind(&self) -> ResearchProviderKind {
            ResearchProviderKind::GenericWebSearch
        }

        async fn search(
            &self,
            req: ResearchSearchRequest,
        ) -> anyhow::Result<ResearchSearchResponse> {
            let limit = req.limit.unwrap_or(10).min(self.1);
            let results = (0..limit)
                .map(|index| ResearchSearchResult {
                    id: format!("{}-{index}", self.0),
                    source: ResearchSource::Web,
                    provider: self.0.to_string(),
                    title: format!("{} result {index}", self.0),
                    url: format!("https://example.com/{}/{index}", self.0),
                    canonical_url: Some(format!("https://example.com/{}/{index}", self.0)),
                    snippet: None,
                    site_name: None,
                    author: None,
                    published_at: None,
                    score: None,
                    content_hash: None,
                    content_markdown: None,
                    retrieved_at: None,
                })
                .collect();
            Ok(ResearchSearchResponse {
                query: req.query,
                results,
            })
        }
    }

    #[test]
    fn default_research_config_is_disabled() {
        assert!(!ResearchConfig::default().enabled);
    }

    #[test]
    fn runtime_registers_sqlite_store() {
        let dir = tempfile::tempdir().unwrap();
        let config = ResearchConfig {
            enabled: true,
            cache: config::ResearchCacheConfig {
                db_path: Some(dir.path().join("research.db").display().to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let runtime = ResearchRuntime::from_config(config).unwrap();
        assert!(runtime.store().is_some());
    }

    #[test]
    fn runtime_uses_explicit_home_for_default_store() {
        let dir = tempfile::tempdir().unwrap();
        let config = ResearchConfig {
            enabled: true,
            ..Default::default()
        };
        let runtime =
            ResearchRuntime::from_config_with_home(config, dir.path().join("agent")).unwrap();
        assert!(runtime.store().is_some());
        assert!(dir.path().join("agent/research.db").exists());
    }

    #[tokio::test]
    async fn search_returns_empty_results_when_some_provider_succeeds_with_no_matches() {
        let dir = tempfile::tempdir().unwrap();
        let config = ResearchConfig {
            enabled: true,
            provider_order: vec!["empty".to_string(), "failing".to_string()],
            defaults: ResearchDefaults {
                sources: vec![ResearchSource::Web],
                limit: 10,
                ..Default::default()
            },
            cache: ResearchCacheConfig {
                enabled: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let providers = HashMap::from([
            (
                "empty".to_string(),
                Arc::new(EmptyOkProvider("empty")) as Arc<dyn ResearchProvider>,
            ),
            (
                "failing".to_string(),
                Arc::new(FailingProvider("failing")) as Arc<dyn ResearchProvider>,
            ),
        ]);
        let runtime = ResearchRuntime {
            config,
            providers,
            store: None,
            http: reqwest::Client::new(),
            reports_root: dir.path().join("reports"),
        };

        let response = runtime
            .search(ResearchSearchRequest {
                query: "语音大模型".to_string(),
                sources: vec![ResearchSource::Web],
                provider_ids: Vec::new(),
                freshness: None,
                limit: Some(10),
                fetch_content: false,
                language: Some("zh-CN".to_string()),
            })
            .await
            .unwrap();

        assert!(response.results.is_empty());
    }

    #[tokio::test]
    async fn search_respects_selected_provider_ids() {
        let dir = tempfile::tempdir().unwrap();
        let config = ResearchConfig {
            enabled: true,
            provider_order: vec!["empty".to_string(), "failing".to_string()],
            defaults: ResearchDefaults {
                sources: vec![ResearchSource::Web],
                limit: 10,
                ..Default::default()
            },
            cache: ResearchCacheConfig {
                enabled: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let providers = HashMap::from([
            (
                "empty".to_string(),
                Arc::new(EmptyOkProvider("empty")) as Arc<dyn ResearchProvider>,
            ),
            (
                "failing".to_string(),
                Arc::new(FailingProvider("failing")) as Arc<dyn ResearchProvider>,
            ),
        ]);
        let runtime = ResearchRuntime {
            config,
            providers,
            store: None,
            http: reqwest::Client::new(),
            reports_root: dir.path().join("reports"),
        };

        let response = runtime
            .search(ResearchSearchRequest {
                query: "语音大模型".to_string(),
                sources: vec![ResearchSource::Web],
                provider_ids: vec!["empty".to_string()],
                freshness: None,
                limit: Some(10),
                fetch_content: false,
                language: Some("zh-CN".to_string()),
            })
            .await
            .unwrap();

        assert!(response.results.is_empty());
    }

    #[tokio::test]
    async fn search_limit_applies_per_provider() {
        let dir = tempfile::tempdir().unwrap();
        let config = ResearchConfig {
            enabled: true,
            provider_order: vec!["one".to_string(), "two".to_string()],
            defaults: ResearchDefaults {
                sources: vec![ResearchSource::Web],
                limit: 2,
                ..Default::default()
            },
            cache: ResearchCacheConfig {
                enabled: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let providers = HashMap::from([
            (
                "one".to_string(),
                Arc::new(CountingProvider("one", 5)) as Arc<dyn ResearchProvider>,
            ),
            (
                "two".to_string(),
                Arc::new(CountingProvider("two", 5)) as Arc<dyn ResearchProvider>,
            ),
        ]);
        let runtime = ResearchRuntime {
            config,
            providers,
            store: None,
            http: reqwest::Client::new(),
            reports_root: dir.path().join("reports"),
        };

        let response = runtime
            .search(ResearchSearchRequest {
                query: "语音大模型".to_string(),
                sources: vec![ResearchSource::Web],
                provider_ids: Vec::new(),
                freshness: None,
                limit: Some(2),
                fetch_content: false,
                language: Some("zh-CN".to_string()),
            })
            .await
            .unwrap();

        assert_eq!(response.results.len(), 4);
        assert_eq!(
            response
                .results
                .iter()
                .filter(|item| item.provider == "one")
                .count(),
            2
        );
        assert_eq!(
            response
                .results
                .iter()
                .filter(|item| item.provider == "two")
                .count(),
            2
        );
    }

    #[test]
    fn provider_config_deserializes_type() {
        let value: ResearchProviderConfig = toml::from_str(
            r#"
enabled = true
type = "generic_web_search"
base_url = "https://example.com/search"
"#,
        )
        .unwrap();
        assert_eq!(
            value.provider_type,
            config::ResearchProviderType::GenericWebSearch
        );
    }

    #[test]
    fn provider_config_deserializes_sogou_cdp_type() {
        let value: ResearchProviderConfig = toml::from_str(
            r#"
enabled = true
type = "sogou_wechat_cdp"
cdp_endpoint = "http://127.0.0.1:9333"
"#,
        )
        .unwrap();
        assert_eq!(
            value.provider_type,
            config::ResearchProviderType::SogouWechatCdp
        );
        assert_eq!(value.cdp_endpoint_or_default(), "http://127.0.0.1:9333");
    }

    #[test]
    fn ai_tech_preset_registers_curated_fixed_site_sources() {
        let mut config = ResearchConfig {
            enabled: true,
            ..Default::default()
        };
        apply_preset(&mut config, "ai-tech");
        assert_eq!(config.preset.as_deref(), Some("ai-tech"));
        assert_eq!(
            config
                .providers
                .get("arxiv")
                .and_then(|provider| provider.site.as_ref()),
            Some(&ResearchSiteKind::Arxiv)
        );
        assert_eq!(
            config
                .providers
                .get("hacker_news")
                .and_then(|provider| provider.site.as_ref()),
            Some(&ResearchSiteKind::HackerNews)
        );
        assert_eq!(
            config
                .providers
                .get("github_repositories")
                .and_then(|provider| provider.site.as_ref()),
            Some(&ResearchSiteKind::GithubRepositories)
        );
        assert_eq!(
            config
                .providers
                .get("sogou_wechat_cdp")
                .map(|provider| &provider.provider_type),
            Some(&config::ResearchProviderType::SogouWechatCdp)
        );
        let volc = config.providers.get("volc_web_search").unwrap();
        assert_eq!(
            volc.provider_type,
            config::ResearchProviderType::VolcWebSearch
        );
        assert!(!volc.enabled);
        assert_eq!(volc.env_key.as_deref(), Some("ARK_TOKEN"));
        assert_eq!(volc.search_type.as_deref(), Some("web"));
        assert_eq!(volc.content_formats.as_deref(), Some("markdown"));
        assert_eq!(
            config
                .wechat
                .as_ref()
                .and_then(|wechat| wechat.provider.as_deref()),
            Some("sogou_wechat_cdp")
        );
    }

    #[test]
    fn ordered_web_providers_respects_explicit_provider_order_before_preset_sources() {
        let mut providers = std::collections::HashMap::new();
        providers.insert(
            "mock".to_string(),
            ResearchProviderConfig {
                provider_type: config::ResearchProviderType::GenericWebSearch,
                base_url: Some("https://example.com/search".to_string()),
                ..Default::default()
            },
        );
        providers.insert(
            "arxiv".to_string(),
            ResearchProviderConfig {
                provider_type: config::ResearchProviderType::FixedSite,
                site: Some(ResearchSiteKind::Arxiv),
                ..Default::default()
            },
        );
        let runtime = ResearchRuntime::from_config_with_home(
            ResearchConfig {
                enabled: true,
                providers,
                provider_order: vec!["mock".to_string(), "arxiv".to_string()],
                ..Default::default()
            },
            tempfile::tempdir().unwrap().path().join("agent"),
        )
        .unwrap();
        let ids = runtime
            .ordered_web_providers()
            .into_iter()
            .map(|provider| provider.id().to_string())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["mock", "arxiv"]);
    }

    #[test]
    fn detects_wechat_and_sogou_fetch_urls() {
        assert!(is_wechat_or_sogou_url("https://mp.weixin.qq.com/s/a"));
        assert!(is_wechat_or_sogou_url(
            "https://weixin.sogou.com/link?url=abc"
        ));
        assert!(!is_wechat_or_sogou_url("https://example.com/link?url=abc"));
    }

    #[test]
    fn wechat_provider_order_uses_sogou_cdp_only() {
        let mut providers = std::collections::HashMap::new();
        providers.insert(
            "sogou_wechat_cdp".to_string(),
            ResearchProviderConfig {
                provider_type: config::ResearchProviderType::SogouWechatCdp,
                cdp_endpoint: Some("http://127.0.0.1:9222".to_string()),
                ..Default::default()
            },
        );
        let runtime = ResearchRuntime::from_config_with_home(
            ResearchConfig {
                enabled: true,
                providers,
                provider_order: vec!["sogou_wechat_cdp".to_string()],
                ..Default::default()
            },
            tempfile::tempdir().unwrap().path().join("agent"),
        )
        .unwrap();
        let ids = runtime
            .ordered_wechat_providers()
            .into_iter()
            .map(|provider| provider.id().to_string())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["sogou_wechat_cdp"]);
    }
}

# Bifrost Research Pack 设计方案

## 功能模块描述

Bifrost Research Pack 是内置在 Bifrost Agent 中的个人研究能力包，目标不是企业级爬虫系统，而是给本地 Agent 提供可配置、可沉淀、可复查的搜索与研究日报能力。

对用户暴露的入口应保持简单：

```toml
[research]
enabled = true
preset = "personal-cn"
```

启用后，Agent 获得以下高层能力：

- 联网搜索：`research_search`
- 微信公众号补充检索：通过 `research_search` fallback，必要时再独立暴露 `research_wechat_search`
- 网页正文提取：`research_fetch`
- 本地知识库检索：`knowledge_search`
- 本地知识库沉淀：`knowledge_save`
- 手动研究日报：`research_digest`
- 后续定时研究任务：Research task runner

设计原则：

1. 模型只看到少量高层工具，不直接感知 Tavily、Exa、火山、Sogou 微信 CDP、缓存、去重、限流等 provider 细节。
2. P1 先落 direct HTTP provider、SQLite/FTS、本地工具和系统 Skill；P2 再补完整 scheduler、WebUI task 管理、通知和 MCP provider。
3. 研究资料属于外部上下文，和长期记忆 `memories` 不是同一个系统。Research Pack 保存“来源材料和报告”，Memory 保存“用户偏好、事实和长期工作上下文”。
4. 默认安全：禁止 localhost/private IP fetch，限制响应大小、跳转次数、超时和微信公众号 fallback 频率。

## 当前仓库落点

当前仓库已经具备以下接入点：

- `crates/agent/src/config.rs`：`AgentConfig` 已有 `mcp_servers`、`skills`、`memories`、provider env key 解析模式，适合增加 `research: Option<ResearchConfig>`。
- `crates/agent/src/tools/mod.rs`：`ToolRegistry::with_defaults(shell_timeout_secs)` 目前只接 shell timeout，适合新增 `ToolRegistry::with_agent_config(&AgentConfig)` 注册 research tools。
- `crates/agent/src/assets/samples/`：系统 Skill 通过 `include_dir!` 编译进 binary，并安装到 `~/.bifrost/agent/skills/.system/`。Research Skill 应作为系统 Skill 放在这里。
- `crates/agent/src/session.rs`：run turn 已支持本地 tools + MCP manager；P1 research tools 不依赖 MCP manager，避免改动 session loop。
- `design/long-term-memory.md`：memory 已经转向文件化 read-path，不适合作为 Research Pack 的正文索引数据库；research 自己维护 SQLite/FTS。
- `crates/bifrost-admin/src/handlers/im_gateway.rs`：Agent settings API 当前挂在 IM Gateway service 下，前端 `BASE = "/im-gateway"`，research admin API 可先放在 `/im-gateway/agent/research/*`，后续再抽独立 agent admin namespace。
- `web/src/pages/Settings/tabs/AgentTab.tsx`：Agent settings 已经是左侧 section + URL `agentSection` 恢复，新增 `ResearchSection.tsx` 是自然扩展。
- `design/im-gateway.md` 和 `crates/bifrost-admin/src/im_gateway/scheduler.rs`：已有 schedule/concurrency/retry/run store 的设计与部分实现可复用理念，但 Research Task 不应强依赖 IM Gateway schedule store。

## 目标验证清单

必须实现：

- `AgentConfig` 支持 `[research]` 配置，并能从 `$BIFROST_DATA_DIR/agent/config.toml` 和项目级 `.bifrost/agent/config.toml` 正常加载/合并。
- 内置统一工具：`research_search`、`research_fetch`、`knowledge_search`、`knowledge_save`、`research_digest`。
- direct/custom HTTP provider：`generic_web_search` 和 `custom_http`；微信来源使用 `sogou_wechat_cdp`。
- 固定站点 provider：`fixed_site`，P1 内置 `arxiv`、`hacker_news`、`github_repositories` 三个高质量 AI/技术源。
- 本地知识库：SQLite + FTS5，支持 URL 去重、content hash 去重、FTS 检索。
- 系统 Skill：`research`，提供 `/research` slash command 和搜索/研究/日报触发规则。
- P1 支持手动搜索、抓取、沉淀和生成 Markdown 报告。

必须不破坏：

- 现有 Agent tool registry、MCP tool_search、Skill progressive disclosure 和 memories read/write path。
- IM Gateway 的 provider、route、schedule 权限语义。
- WebUI Agent 左侧导航和 URL 恢复行为。
- 用户数据目录隔离：测试与开发必须使用临时 `BIFROST_DATA_DIR`。

必须真实验证：

- 通过真实 Agent chat 请求触发 research tools，而不是只测 provider 函数。
- 通过真实 CLI `bifrost agent research search ...` 验证配置、provider 和 store。
- 通过真实 WebUI Settings 验证 light/dark 主题下 Research section 可读可操作。
- 微信 fallback 用本地 mock HTTP 服务验证；Sogou/微信公众号真实抓取通过 opt-in CDP E2E 验证，主 CI 默认跳过公网反爬路径。
- 通过真实 `/agent/chat` 请求验证 Agent Loop 能看到并调用 `research_search`、`research_fetch`、`knowledge_save`、`research_digest`，生成可上传到飞书文档的 Markdown。

必须交付：

- 完整方案最终交付：设计文档、Rust 配置/工具/存储实现、系统 Skill、CLI 文档、Admin API、WebUI、E2E、human_tests、README/帮助文本同步。
- P1 最小交付以 CLI + Agent tools + Skill + SQLite + manual digest 为主；WebUI Research section 可作为 P1.5，但 API 与前端类型必须在 P1 设计中保持稳定。
- 每个实现 PR 都必须完成两轮 Review/Fix/Test，且实际执行对应 human_tests。

## 推荐目录结构

```text
crates/agent/src/
  research/
    mod.rs
    config.rs
    provider.rs
    providers/
      generic_http.rs
      sogou_wechat_cdp.rs
      volc.rs          # P2/P3 专用 provider
      tavily.rs        # P2/P3 专用 provider
      exa.rs           # P2/P3 专用 provider
    fetch.rs
    normalize.rs
    dedupe.rs
    store.rs
    digest.rs
    task.rs            # P2
    scheduler.rs       # P2
  tools/
    research_search.rs
    research_fetch.rs
    knowledge_search.rs
    knowledge_save.rs
    research_digest.rs
  assets/samples/research/
    SKILL.md
    manifest.json

crates/bifrost-admin/src/handlers/
  agent_research.rs

web/src/pages/Settings/tabs/agent/
  ResearchSection.tsx
```

单文件超过 1000 行时必须主动拆分，禁止让任一实现文件超过 1500 行。

## 配置设计

`AgentConfig` 新增：

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub research: Option<ResearchConfig>,
```

核心配置：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub preset: Option<String>,
    #[serde(default)]
    pub providers: HashMap<String, ResearchProviderConfig>,
    #[serde(default)]
    pub provider_order: Vec<String>,
    #[serde(default)]
    pub wechat: Option<WechatResearchConfig>,
    #[serde(default)]
    pub cache: ResearchCacheConfig,
    #[serde(default)]
    pub defaults: ResearchDefaults,
    #[serde(default)]
    pub fetch_policy: ResearchFetchPolicy,
    #[serde(default)]
    pub tasks: Vec<ResearchTaskConfig>,
}
```

Provider 配置：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchProviderConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub r#type: ResearchProviderType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env_key: Option<String>,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub env_headers: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fetch_url: Option<String>,
    #[serde(default)]
    pub request_template: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site: Option<ResearchSiteKind>,
}
```

Secret 规则与 model provider 一致：

- 推荐 `api_key = "$VOLCENGINE_API_KEY"` 或 `env_key = "VOLCENGINE_API_KEY"`。
- Admin API 和 WebUI 永不返回解析后的 secret。
- CLI、日志、Recent Calls、reports metadata 不记录 secret 明文。

最小配置：

```toml
[research]
enabled = true
preset = "personal-cn"

[research.providers.volc]
enabled = true
type = "generic_web_search"
base_url = "https://your-search-api-endpoint"
api_key = "$VOLCENGINE_API_KEY"

[research.cache]
enabled = true
store = "sqlite"
```

Preset 内置高质量固定站点：

```toml
[research]
enabled = true
preset = "ai-tech"

[research.providers.arxiv]
enabled = true
type = "fixed_site"
site = "arxiv"

[research.providers.hacker_news]
enabled = true
type = "fixed_site"
site = "hacker_news"

[research.providers.github_repositories]
enabled = true
type = "fixed_site"
site = "github_repositories"
```

固定站点 provider 和通用 API provider 的边界：

- 固定站点 provider 有确定的搜索入口、解析规则、失败语义和 Markdown 沉淀 metadata。第一期内置 `arxiv`、`hacker_news`、`github_repositories`，并把 `sogou_wechat_cdp` 作为微信固定 workflow。
- 通用 API provider 包括 `generic_web_search`、`sogou_wechat_cdp`、后续 Tavily/Exa/Volc/custom HTTP/MCP。它们是补充入口，不要求 Agent 临场决定去哪些站点搜索。
- `personal-cn` 和 `ai-tech` preset 会自动补齐固定站点 provider 与 `sogou_wechat_cdp`。用户显式配置的 `--web-provider` 会排在 `provider_order` 前面，避免默认 source 覆盖用户付费 API；单个内置站点不可用时继续尝试剩余 provider，只有全部选中 provider 都失败才把搜索视为失败。

微信公众号 fallback：

```toml
[research.wechat]
enabled = true
mode = "fallback"
min_results_before_fallback = 3
rate_limit_per_minute = 6
max_pages_per_query = 1
provider = "sogou_wechat_cdp"

[research.providers.sogou_wechat_cdp]
enabled = true
type = "sogou_wechat_cdp"
search_url = "http://127.0.0.1:8000/search_articles"
fetch_url = "http://127.0.0.1:8000/fetch_article"
```

第一期同时支持浏览器 CDP 方式抓取 Sogou 微信公众号结果，用于需要用户交互式登录、验证码或站点本身依赖浏览器状态的场景。用户先打开带远程调试端口的本机浏览器并完成登录/验证，Research Pack 通过 CDP 复用该 profile：

```toml
[research.wechat]
enabled = true
mode = "fallback"
provider = "sogou_wechat_cdp"

[research.providers.sogou_wechat_cdp]
enabled = true
type = "sogou_wechat_cdp"
cdp_endpoint = "http://127.0.0.1:9222"
```

CDP provider 的职责：

- 搜索：打开 `https://weixin.sogou.com/weixin?type=2&p=44351200&ie=utf8&query=...`，等待搜索结果 DOM，提取标题、摘要、公众号、时间和 Sogou `/link` URL。
- 详情：通过浏览器打开 Sogou `/link` 或 `mp.weixin.qq.com` URL，等待跳转和公众号正文 DOM，提取标题、作者、发布时间、正文。
- 登录态：不保存用户 cookie，不注入 secret，只连接用户显式提供的 CDP endpoint；如果站点返回验证码/反爬挑战，工具返回明确错误，不伪造成功结果。

## Provider 抽象

```rust
#[async_trait::async_trait]
pub trait ResearchProvider: Send + Sync {
    fn id(&self) -> &str;
    fn kind(&self) -> ResearchProviderKind;

    async fn search(
        &self,
        req: ResearchSearchRequest,
    ) -> anyhow::Result<ResearchSearchResponse>;

    async fn fetch(
        &self,
        req: ResearchFetchRequest,
    ) -> anyhow::Result<Option<FetchedDocument>> {
        Ok(None)
    }
}
```

P1 provider：

- `fixed_site`：固定站点 adapter。每个 provider 绑定一个 `site`，有明确搜索入口、解析规则、失败语义和标准化输出；P1 内置：
  - `arxiv`：`export.arxiv.org/api/query`，搜索论文条目，抽取标题、摘要、作者、发布时间和 `arxiv.org/abs/...` URL。
  - `hacker_news`：HN Algolia `search_by_date`，搜索技术社区近期讨论，抽取标题、作者、发布时间、分数和原文 URL。
  - `github_repositories`：GitHub Search repositories API，搜索活跃开源项目，抽取仓库名、描述、owner、更新时间、star 数和仓库 URL。
- `generic_web_search`：POST JSON、Bearer token、自定义 header、模板化响应解析。
- `sogou_wechat_cdp`：本地 HTTP 服务，支持 search/fetch，配合 fallback 限流。
- `sogou_wechat_cdp`：连接本机 Microsoft Edge CDP endpoint，使用真实浏览器会话抓取 Sogou 微信公众号搜索和详情正文，适合用户交互式登录后复用登录态。

P2/P3 provider：

- `volc_web_search`：火山专用适配器。
- `tavily` / `exa`：官方 API 专用适配器。
- `mcp_provider`：通过 MCP server tool 调用搜索/抽取。该阶段需要把 `McpManager` 或可调用 tool facade 注入 research runtime；P1 不做，避免扩大 session loop 改动。

Preset 行为：

- `personal-cn`：默认安装固定技术源 `arxiv`、`hacker_news`、`github_repositories` 和微信源 `sogou_wechat_cdp`；中文 query 在 web 结果不足时进入微信 fallback，浏览器未连接/未登录时记录该 provider 失败并继续返回其他来源结果。
- `ai-tech`：同样安装三类固定技术源，面向英文技术资料、论文、开源项目与社区讨论；Tavily/Exa/Volc/custom HTTP 作为补充 fallback，而不是默认唯一搜索路径。
- 用户显式传入 `--web-provider` 时，该 provider 会放在 `provider_order` 首位，确保测试和付费 provider 可优先命中；固定站点仍作为后续高质量 source。

## 数据结构

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchSearchRequest {
    pub query: String,
    #[serde(default)]
    pub sources: Vec<ResearchSource>,
    pub freshness: Option<Freshness>,
    pub limit: Option<usize>,
    #[serde(default)]
    pub fetch_content: bool,
    pub language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchSearchResult {
    pub id: String,
    pub source: ResearchSource,
    pub provider: String,
    pub title: String,
    pub url: String,
    pub canonical_url: Option<String>,
    pub snippet: Option<String>,
    pub site_name: Option<String>,
    pub author: Option<String>,
    pub published_at: Option<String>,
    pub score: Option<f32>,
    pub content_hash: Option<String>,
    pub content_markdown: Option<String>,
    pub retrieved_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarkdownArtifact {
    pub title: Option<String>,
    pub url: String,
    pub canonical_url: String,
    pub source: Option<ResearchSource>,
    pub provider: Option<String>,
    pub site_name: Option<String>,
    pub author: Option<String>,
    pub published_at: Option<String>,
    pub retrieved_at: i64,
    pub content_markdown: String,
    pub content_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeItem {
    pub id: String,
    pub source: String,
    pub provider: String,
    pub query: Option<String>,
    pub title: String,
    pub url: String,
    pub canonical_url: String,
    pub author: Option<String>,
    pub published_at: Option<String>,
    pub content_markdown: Option<String>,
    pub summary: Option<String>,
    pub tags: Vec<String>,
    pub content_hash: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}
```

标准化要求：

- provider 原始响应不得直接暴露给模型。
- URL 必须 canonicalize 后再生成 id 和 dedupe key。
- 搜索结果 id 建议为 `sha256:<canonical_url>`，正文 hash 建议为 `sha256:<normalized_markdown>`。
- `published_at` P1 允许保留原始字符串；P2 再收敛为 RFC3339。
- `research_fetch` 必须返回 `markdown_artifact`，包含正文 Markdown 和 source/provider/canonical URL/retrieved_at 等可沉淀元数据。
- `research_search(fetch_content=true)` 会对 top results 执行 fetch，并把 `content_markdown`、`content_hash`、`retrieved_at` 回填到搜索结果，供 Agent 直接保存或生成博客。

## 工具设计

### `research_search`

统一搜索入口。默认 `sources = ["web"]`；中文 query 且 web 结果不足时允许 fallback 到 wechat。

输入：

```json
{
  "query": "AI Agent MCP 中文社区",
  "sources": ["web", "wechat"],
  "freshness": "week",
  "limit": 10,
  "fetch_content": false
}
```

输出：

```json
[
  {
    "provider_id": "volc_web_search",
    "results": [
      {
        "id": "sha256:...",
        "source": "web",
        "provider": "volc_web_search",
        "title": "...",
        "url": "...",
        "snippet": "...",
        "site_name": "...",
        "published_at": "...",
        "score": 0.82
      }
    ]
  },
  {
    "provider_id": "sogou_wechat_cdp",
    "results": []
  }
]
```

Agent tool 输出必须与 provider 完成顺序一致：任一 provider 完成搜索与正文解析后，先产出该 provider 的事件，再继续等待其他 provider。受当前 OpenAI-style tool call 协议限制，模型侧仍在单次 tool result 中接收完整 JSON 数组；运行时必须额外向 turn progress 通道发送 `ToolProgress`，使 IM/Web 进度卡在 `research_search` 执行期间就能看到 provider 级增量结果，而不是等所有 provider 全部完成。

### `research_fetch`

抓取正文并返回 Markdown。

输入：

```json
{
  "url": "https://example.com/article",
  "format": "markdown",
  "max_bytes": 200000
}
```

输出：

```json
{
  "url": "...",
  "title": "...",
  "author": "...",
  "published_at": "...",
  "content_markdown": "...",
  "content_hash": "sha256:...",
  "fetched_at": 1778572800
}
```

P1 可以使用 `reqwest` + HTML 提取 + `html2md`；如果 readability 依赖过重，先以安全抓取和 basic markdown 转换交付，再在 P2 优化正文质量。

### `knowledge_save`

保存搜索结果或正文。

```json
{
  "items": [
    {
      "url": "...",
      "title": "...",
      "content_markdown": "...",
      "summary": "...",
      "tags": ["ai", "agent", "mcp"]
    }
  ]
}
```

返回：

```json
{
  "saved": 5,
  "duplicates": 2
}
```

### `knowledge_search`

检索本地知识库。

```json
{
  "query": "MCP web search",
  "limit": 10,
  "since_days": 30
}
```

返回：

```json
{
  "results": [
    {
      "title": "...",
      "url": "...",
      "summary": "...",
      "matched_text": "...",
      "created_at": 1778572800
    }
  ]
}
```

### `research_digest`

手动生成 Markdown 报告。P1 可基于已保存 items 或本次搜索输入生成；P2 接入 task run。

```json
{
  "task_id": "daily_ai_research",
  "date": "2026-05-12",
  "format": "markdown"
}
```

返回：

```json
{
  "report_path": "~/.bifrost/agent/reports/daily_ai_research/2026-05-12.md",
  "items_used": 32,
  "summary": "今日重点..."
}
```

## Runtime 装配

新增 `ResearchRuntime`：

```rust
pub struct ResearchRuntime {
    config: ResearchConfig,
    providers: HashMap<String, Arc<dyn ResearchProvider>>,
    store: Option<Arc<KnowledgeStore>>,
    http: reqwest::Client,
}
```

新增 registry 构造：

```rust
impl ToolRegistry {
    pub fn with_agent_config(config: &AgentConfig) -> Self {
        let mut registry = Self::with_defaults(config.get_shell_timeout_secs());
        if config.research.as_ref().is_some_and(|research| research.enabled) {
            match ResearchRuntime::from_config(config) {
                Ok(runtime) => {
                    let runtime = Arc::new(runtime);
                    registry.register(Arc::new(ResearchSearchTool::new(runtime.clone())));
                    registry.register(Arc::new(ResearchFetchTool::new(runtime.clone())));
                    registry.register(Arc::new(KnowledgeSearchTool::new(runtime.clone())));
                    registry.register(Arc::new(KnowledgeSaveTool::new(runtime.clone())));
                    registry.register(Arc::new(ResearchDigestTool::new(runtime)));
                }
                Err(error) => {
                    tracing::warn!(%error, "research runtime disabled due to invalid config");
                }
            }
        }
        registry
    }
}
```

迁移调用点：

- `AgentClient` 示例和 IM Gateway 初始化处从 `ToolRegistry::with_defaults(config.get_shell_timeout_secs())` 改为 `ToolRegistry::with_agent_config(&config)`。
- 保留 `with_defaults`，保证单元测试和无 config 调用点兼容。
- P1 不让 research tool 调 MCP manager。MCP server 仍通过现有 deferred `tool_search` 暴露给模型。
- `ResearchRuntime::from_config` 必须返回 `Result`；provider 配置错误不能 panic。启动时可选择不注册 research tools 并记录错误，工具执行时必须返回结构化配置错误，避免影响非 research Agent 会话。

## SQLite 知识库

默认路径：

```text
$BIFROST_DATA_DIR/agent/research.db
```

建表：

```sql
CREATE TABLE IF NOT EXISTS knowledge_items (
  id TEXT PRIMARY KEY,
  source TEXT NOT NULL,
  provider TEXT NOT NULL,
  query TEXT,
  title TEXT NOT NULL,
  url TEXT NOT NULL,
  canonical_url TEXT NOT NULL,
  author TEXT,
  published_at TEXT,
  content_markdown TEXT,
  summary TEXT,
  tags_json TEXT NOT NULL DEFAULT '[]',
  content_hash TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_knowledge_canonical
ON knowledge_items(canonical_url);

CREATE INDEX IF NOT EXISTS idx_knowledge_created
ON knowledge_items(created_at);

CREATE VIRTUAL TABLE IF NOT EXISTS knowledge_fts
USING fts5(
  title,
  summary,
  content_markdown,
  content='knowledge_items',
  content_rowid='rowid',
  tokenize='unicode61'
);
```

FTS 同步策略：

- P1 可在 `upsert` 后显式维护 FTS 行，减少 trigger 调试成本。
- P2 可改成 SQLite trigger，但必须补充 update/delete 回归测试。

不做：

- P1 不引入向量库。
- P1 不把 research.db 作为长期记忆 recall 源。

## Fetch 安全策略

默认策略：

```toml
[research.fetch_policy]
allow_private_ip = false
allow_localhost = false
max_redirects = 5
max_response_bytes = 500000
timeout_secs = 20
user_agent = "BifrostResearch/0.1"
```

必须实现：

- DNS 解析后拒绝 private/link-local/loopback/multicast/reserved IP。
- redirect 每一跳都重新做 host/IP 检查。
- 限制 `Content-Length` 和 streaming body 累计 bytes。
- 默认只允许 `http` 和 `https`。
- `research_fetch` 工具 schema 中 `max_bytes` 不能超过配置上限。

## 微信公众号 fallback

P1 只支持 HTTP 模式，Bifrost 不关心用户本地服务底层是搜狗、浏览器自动化还是其他渠道。

搜索 endpoint 推荐约定：

```http
POST /search_articles
Content-Type: application/json

{
  "query": "微信公众号 AI 智能体 最新",
  "limit": 8,
  "freshness": "week"
}
```

抓取 endpoint：

```http
POST /fetch_article
Content-Type: application/json

{
  "url": "https://mp.weixin.qq.com/s/..."
}
```

fallback 合并规则：

1. 先走 web providers。
2. 中文 query 或 `sources` 包含 `wechat` 时判断 fallback。
3. web 结果少于 `min_results_before_fallback` 时调用 `sogou_wechat_cdp`。
4. 合并后按 canonical URL、content hash、`title + site + published_at` 去重。
5. wechat 结果保留 `source = "wechat"`，provider 为配置 id。

## 系统 Skill

新增：

```text
crates/agent/src/assets/samples/research/
  SKILL.md
  manifest.json
```

`manifest.json`：

```json
{
  "name": "research",
  "version": "0.1.0",
  "description": "用于联网搜索、微信公众号补充检索、本地知识库查询和生成研究日报的 Bifrost 内置技能。",
  "scope": "system",
  "entrypoint": {
    "kind": "inline",
    "instructions_md": "See SKILL.md"
  },
  "allowed_tools": [
    { "kind": "registry", "name": "research_search" },
    { "kind": "registry", "name": "research_fetch" },
    { "kind": "registry", "name": "knowledge_search" },
    { "kind": "registry", "name": "knowledge_save" },
    { "kind": "registry", "name": "research_digest" },
    { "kind": "registry", "name": "read_file" },
    { "kind": "registry", "name": "write_file" }
  ],
  "slash_command": "/research",
  "triggers": [
    { "kind": "description_match" },
    { "kind": "keyword", "any_of": ["搜索", "检索", "日报", "研究", "微信公众号", "web search"] },
    { "kind": "slash_command" }
  ],
  "metadata": {},
  "created_by": { "agent": { "session_id": "system" } },
  "created_at_unix": 0,
  "updated_at_unix": 0,
  "checksum": "",
  "schema_version": 1
}
```

`SKILL.md` 规则：

- 先 `knowledge_search`。
- 本地结果不足再 `research_search`。
- 对重要来源调用 `research_fetch`。
- 需要沉淀时调用 `knowledge_save`。
- 需要日报时调用 `research_digest`。
- 中文主题结果不足时允许微信公众号 fallback。
- 输出事实判断必须带来源链接。
- 来源冲突时必须标注不确定性。

## Research Digest

报告目录：

```text
$BIFROST_DATA_DIR/agent/reports/<task_id>/<yyyy-mm-dd>.md
```

报告格式：

```markdown
# 每日 AI Agent 研究 - 2026-05-12

## TL;DR
- ...

## 重点发现
### 1. ...
来源：
- [title](url)

摘要：
...

## 值得跟进
- ...

## 原始检索 Query
- AI Agent framework latest

## 本次使用来源
| 来源 | 数量 |
|---|---:|
| web | 21 |
| wechat | 6 |
```

摘要生成流程：

1. 确定性代码完成搜索、抓取、去重、保存和 source table。
2. 再调用 `AgentClient` 生成 Markdown 摘要。
3. Prompt 明确“不编造来源中没有的信息；重要判断必须带来源链接；冲突必须标注”。
4. 报告写入临时文件后 rename，避免半写入。

## P2 定时任务

Research Task 不强依赖 IM Gateway schedule store，单独放在 `crates/agent/src/research/task.rs` 和 `scheduler.rs`。

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchTask {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub trigger: ResearchTaskTrigger,
    pub queries: Vec<String>,
    pub sources: Vec<ResearchSource>,
    pub max_results_per_query: usize,
    pub fetch_content: bool,
    pub dedupe_days: u32,
    pub summarize: bool,
    pub language: String,
    pub output_dir: Option<PathBuf>,
    pub notify: Option<ResearchNotifyConfig>,
    pub concurrency_policy: ResearchConcurrencyPolicy,
    pub retry: ResearchRetryPolicy,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_run_at: Option<i64>,
    pub next_run_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResearchTaskTrigger {
    Cron { expr: String, timezone: String },
    Interval { every_ms: u64 },
}
```

Runner：

```text
tick every 60s
  -> load enabled tasks
  -> find due tasks
  -> apply concurrency policy
  -> run deterministic search/fetch/dedupe/save
  -> summarize with AgentClient
  -> write report
  -> notify if configured
  -> update run record and next_run_at
```

P2 持久化：

- `research_tasks.json`：任务配置。
- `research_runs.jsonl` 或 SQLite table：运行历史。
- 报告 Markdown 文件：用户可直接读取。

## Admin API

P1/P2 API 先挂在 IM Gateway Agent namespace，复用当前前端 `BASE = "/im-gateway"`：

```text
GET    /api/im-gateway/agent/research/config
PATCH  /api/im-gateway/agent/research/config
GET    /api/im-gateway/agent/research/providers
POST   /api/im-gateway/agent/research/providers/test
GET    /api/im-gateway/agent/research/items?query=...
GET    /api/im-gateway/agent/research/reports
GET    /api/im-gateway/agent/research/reports/:task_id/:date

P2:
GET    /api/im-gateway/agent/research/tasks
POST   /api/im-gateway/agent/research/tasks
GET    /api/im-gateway/agent/research/tasks/:id
PATCH  /api/im-gateway/agent/research/tasks/:id
DELETE /api/im-gateway/agent/research/tasks/:id
POST   /api/im-gateway/agent/research/tasks/:id/run
GET    /api/im-gateway/agent/research/runs
GET    /api/im-gateway/agent/research/runs/:id
```

实现方式：

- 新增 `crates/bifrost-admin/src/handlers/agent_research.rs`。
- `im_gateway.rs` 的 Agent 分支路由到 `handle_agent_research`。
- Config PATCH 走 `AgentConfigStore`，仅允许更新 `research` 字段。
- provider test 返回脱敏后的 request/response summary。

## WebUI 设计

Agent Settings 新增 section：

```text
Research
  Search Workbench
  Supported Sources
  Providers
  WeChat fallback
  Knowledge Store
  Tasks        # P2
  Reports
```

文件：

```text
web/src/pages/Settings/tabs/agent/ResearchSection.tsx
web/src/pages/Settings/tabs/agent/types.ts
web/src/pages/Settings/tabs/AgentTab.tsx
```

要求：

- 加入 `AgentSectionId = "research"` 和 `AGENT_SECTION_NAV`，URL 使用 `agentSection=research`。
- 使用 CSS/Ant Design token，不硬编码颜色，必须支持 light/dark。
- 页面只渲染当前 section，不做锚点滚动。
- Provider API key 输入只显示 `$ENV_VAR` 或 masked 状态，不显示解析后 secret。
- Search Workbench 提供关键词输入、source 选择、limit、`fetch_content` 开关；提交后调用 Admin API 触发内置站点聚合搜索，并在 Top 资源中展示标题、source、provider、site、author、published_at、retrieved_at、canonical_url、content_hash 和完整 `content_markdown`。
- Supported Sources 必须明确展示每个内置/配置来源的 supported、configured/enabled、authorized 和 logged-in/browser 状态；`sogou_wechat_cdp` 必须展示 `https://weixin.sogou.com/weixin?type=2&p=44351200&ie=utf8&query={query}` 模板。
- Knowledge Store 显示 DB path、item count、last indexed time、搜索框和结果链接。
- Report 列表支持打开 Markdown 内容，P1 可以只读展示。
- P2 再做 task list、Run now、View last report。

## CLI 设计

新增命令：

```text
bifrost agent research init
bifrost agent research provider test <provider>
bifrost agent research search "MCP server web search"
bifrost agent research fetch <url>
bifrost agent research knowledge search "MCP"
bifrost agent research report latest <task_id>

P2:
bifrost agent research task list
bifrost agent research task run daily_ai_research
```

快捷初始化：

```bash
bifrost agent research init \
  --preset personal-cn \
  --web-provider volc \
  --wechat-url http://127.0.0.1:8000 \
  --wechat-cdp-endpoint http://127.0.0.1:9222 \
  --yes
```

生成：

```text
$BIFROST_DATA_DIR/agent/config.toml
$BIFROST_DATA_DIR/agent/research.db
$BIFROST_DATA_DIR/agent/reports/
```

## MCP 兼容策略

P1 不把 Research Pack 设计成 MCP-only，也不让 research tool 直接调 MCP manager。

原因：

- 当前 `ToolHandler::execute(&self, arguments, work_dir)` 没有 `McpManager`。
- session loop 虽然能路由本地工具和 MCP 工具，但 research provider 内部再调 MCP 会引入嵌套 tool runtime 和权限边界问题。
- 现有 MCP deferred `tool_search` 能继续让模型直接发现 MCP 工具，不阻塞用户使用 Tavily/Exa MCP。

P1 必须保证 Research Pack 本地 tools 对 Agent Loop 可见。实现上，`ToolRegistry::with_agent_config_and_home` 在 `research.enabled=true` 时注册 `research_search`、`research_fetch`、`knowledge_search`、`knowledge_save`、`research_digest`。直接 `/agent/chat` 和 `/agent/tools` 入口必须基于当前 `AgentConfig` 动态构造 registry，避免服务启动后再 PATCH config 时工具列表仍停留在旧配置。验证方式是启动真实 Bifrost 服务并调用：

```bash
GET  /_bifrost/api/im-gateway/agent/tools
POST /_bifrost/api/im-gateway/agent/chat
```

测试模型第一轮请求必须看到五个 Research tools，并实际发起 `knowledge_search -> research_search -> research_fetch -> knowledge_save -> research_digest` 的工具调用链。

P2 方案：

```toml
[research.providers.tavily_mcp]
enabled = true
type = "mcp"
server = "tavily"
search_tool = "search"
extract_tool = "extract"
```

实现前必须先设计一个 `McpToolInvoker` facade，明确 timeout、approval、tool output limit、audit 和并发语义。

## 通知设计

P1 手动 digest 只写 Markdown，并在 CLI/API/Agent response 返回路径。

P2 通知：

- IM Gateway：发送摘要或报告链接到 `notify_target`。
- WebUI：Reports 列表和最新 run 状态。
- CLI：`report latest` 输出路径和摘要。

通知不应把完整长正文推送到 IM；默认只推 TL;DR + 报告路径，避免刷屏和泄露过多外部内容。

## 安全与隐私

网络：

- 默认拒绝 localhost/private IP fetch。
- 微信服务地址是本地 HTTP provider 的例外，仅用于 `sogou_wechat_cdp` provider 的 search/fetch endpoint，不允许普通 `research_fetch` 抓 localhost。
- 限制 timeout、redirect、bytes 和 content type。

Secret：

- 配置鼓励 `$ENV_VAR`，Admin API 返回脱敏。
- 日志和报告不得包含 API key、Authorization header、cookie。

外部内容：

- Research Skill 强制来源链接。
- 不把广告语、snippet 或单一来源当事实。
- 冲突来源必须标注不确定。

数据：

- `research.db` 是本地用户数据，不随项目提交。
- 报告默认写到用户数据目录；只有用户显式指定 `output_dir` 才写到项目目录。

## 开发拆分

### PR 1：Config + Runtime skeleton

文件：

- `crates/agent/src/research/config.rs`
- `crates/agent/src/research/mod.rs`
- `crates/agent/src/config.rs`

完成：

- `ResearchConfig`
- `ResearchRuntime`
- provider trait
- 配置加载/merge/default

验证：

- `cargo test -p bifrost-agent research_config`

### PR 2：Generic Web Search + Sogou WeChat CDP provider

文件：

- `crates/agent/src/research/providers/generic_http.rs`
- `crates/agent/src/research/providers/sogou_wechat_cdp.rs`
- `crates/agent/src/research/normalize.rs`
- `crates/agent/src/research/dedupe.rs`

完成：

- POST JSON
- Bearer token / env headers
- 标准化结果
- fallback merge / dedupe

### PR 3：SQLite Knowledge Store

文件：

- `crates/agent/src/research/store.rs`

依赖：

- 优先评估 workspace 是否已有 SQLite 依赖；若无，新增 `rusqlite = { version = "...", features = ["bundled"] }`。

完成：

- init db
- upsert item
- FTS search
- canonical URL dedupe
- content hash dedupe

### PR 4：Agent tools + registry

文件：

- `crates/agent/src/tools/research_search.rs`
- `crates/agent/src/tools/research_fetch.rs`
- `crates/agent/src/tools/knowledge_search.rs`
- `crates/agent/src/tools/knowledge_save.rs`
- `crates/agent/src/tools/research_digest.rs`
- `crates/agent/src/tools/mod.rs`

完成：

- 5 个工具
- `ToolRegistry::with_agent_config`
- IM Gateway Agent 初始化接入

### PR 5：System Skill

文件：

- `crates/agent/src/assets/samples/research/SKILL.md`
- `crates/agent/src/assets/samples/research/manifest.json`

验证：

- Agent 启动后自动安装 system skill。
- `/research` 可见。
- 搜索类请求触发 research skill metadata。

### PR 6：CLI + Manual Digest

文件：

- `crates/bifrost-cli/src/commands/agent.rs` 或现有 agent command 模块
- `crates/agent/src/research/digest.rs`

完成：

- `init`
- `provider test`
- `search`
- `fetch`
- `knowledge search`
- `report latest`

### PR 7：Admin API + WebUI

文件：

- `crates/bifrost-admin/src/handlers/agent_research.rs`
- `web/src/pages/Settings/tabs/agent/ResearchSection.tsx`
- `web/src/pages/Settings/tabs/agent/types.ts`
- `web/src/pages/Settings/tabs/AgentTab.tsx`

完成：

- 配置 provider
- 测试 provider
- 搜索本地知识库
- 查看报告

### PR 8：Task runner + notifications

文件：

- `crates/agent/src/research/task.rs`
- `crates/agent/src/research/scheduler.rs`
- `crates/agent/src/research/digest.rs`

完成：

- cron / interval
- manual run
- concurrency policy
- retry
- run history
- IM/WebUI/CLI notification

## 第一版最小可交付范围

P1 必须包含：

1. `ResearchConfig`
2. `generic_web_search` provider
3. `sogou_wechat_cdp` provider
4. SQLite FTS store
5. `research_search`
6. `research_fetch`
7. `knowledge_search`
8. `knowledge_save`
9. `research_digest` 手动运行
10. system research skill
11. CLI `init/search/fetch/provider test/knowledge search/report latest`

P1 不包含：

- 完整 scheduler UI。
- MCP provider。
- 向量检索。
- 企业级爬虫队列。
- 多用户权限模型。

## 测试方案

### 单元测试

- `research_config_defaults_disabled`：默认不启用 research，不注册 research tools。
- `research_config_loads_provider_env_key`：加载 `$ENV_VAR` 或 `env_key`，但序列化不泄露解析值。
- `generic_http_normalizes_results`：provider 原始响应标准化为 `ResearchSearchResult`。
- `wechat_fallback_runs_when_web_results_below_threshold`：web 结果不足时调用 wechat provider。
- `fetch_policy_rejects_localhost_and_private_ip`：普通 fetch 拒绝 localhost/private IP。
- `knowledge_store_upserts_and_dedupes_canonical_url`：URL 去重。
- `knowledge_store_dedupes_content_hash`：正文 hash 去重。
- `knowledge_store_fts_search_returns_match`：FTS 检索返回 matched text。
- `tool_registry_registers_research_tools_when_enabled`：启用后注册 5 个工具。
- `tool_registry_omits_research_tools_when_disabled`：关闭时不暴露工具。

### E2E 测试

新增或扩展 `bifrost-e2e`：

- `agent_research_search_generic_http`：启动真实 Bifrost + mock search provider，通过 Agent chat 触发 `research_search`，断言模型可见工具和 provider 请求。
- `agent_research_fetch_policy_blocks_localhost`：通过真实工具调用验证 localhost fetch 被拒绝。
- `agent_research_knowledge_store_roundtrip`：调用 `knowledge_save` 后 `knowledge_search` 能返回结果。
- `agent_research_wechat_fallback`：mock web 返回 1 条、mock wechat 返回多条，断言合并去重。
- `agent_research_digest_writes_markdown`：手动 digest 写入临时 data dir reports，并包含来源链接。
- `agent_research_chat_loop_generates_blog_markdown`：启动真实 Bifrost + mock model + mock research provider，通过 `/agent/chat` 验证 Research tools 暴露、工具调用链、Markdown artifact、digest report 和上传飞书凭据不可用时的可上传 Markdown 输出。
- `agent_research_fixed_site_preset_sources`：验证 `ai-tech` preset 注册 `arxiv`、`hacker_news`、`github_repositories`，且用户显式 provider 顺序优先。

### 真实场景测试 human_tests

实现 PR 必须创建 `human_tests/agent-research-pack.md` 并更新 `human_tests/readme.md`，建议用例：

- `TC-ARP-01`：CLI init 生成 research 配置和数据目录。
- `TC-ARP-02`：CLI provider test 使用 mock generic provider 成功。
- `TC-ARP-03`：统一搜索入口返回标准化结果。
- `TC-ARP-04`：research_fetch 拒绝 localhost/private IP。
- `TC-ARP-05`：Research tools 受配置开关控制。
- `TC-ARP-06`：knowledge save/search 持久化和去重。
- `TC-ARP-07`：系统 Research Skill 文件存在并声明工具。
- `TC-ARP-08`：WebUI Research section light/dark 主题可读，API key 不泄露。
- `TC-ARP-09` 至 `TC-ARP-12`：Sogou 微信公众号 CDP 初始化、真实搜索、详情抓取、CI skip 语义。
- `TC-ARP-13`：preset 安装高质量固定站点 provider。
- `TC-ARP-14`：搜索和详情抓取输出 Markdown artifact。
- `TC-ARP-15`：真实 `/agent/chat` 调用 Research tools 并生成博客 Markdown。
- `TC-ARP-16`：飞书凭据不可用时生成可上传 Markdown 并记录阻塞，不伪造上传成功。

每条用例必须使用临时 `BIFROST_DATA_DIR` 和 `--no-system-proxy`，并记录实际命令、实际结果和清理步骤。

### 项目校验

每个实现 PR 收尾必须执行：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

如果涉及 WebUI：

```bash
pnpm --dir web build
pnpm --dir web exec playwright test web/tests/ui/admin-settings.spec.ts --grep "Research"
```

如果涉及完整 E2E：

```bash
CARGO_TARGET_DIR=target/research-pack-e2e \
BIFROST_E2E_RUNNER_JOBS=1 \
cargo run -p bifrost-e2e -- --test agent_research_search_generic_http --test-timeout 180
```

最后执行 `rust-project-validate` 技能对应校验流程；本地 CI `scripts/ci/local-ci.sh` 视修改范围和成本最后执行。

## Review/Fix/Test 闭环方案

每个实现 PR 至少两轮。

第 1 轮：

- 复读用户目标和本 PR scope。
- 执行 `git status --short`、`git diff`，如有 staged 内容执行 `git diff --cached`。
- review 配置合并、secret 脱敏、provider 错误处理、fetch 安全、SQLite 去重、tool schema、Skill 触发、WebUI 双主题。
- 修复发现的问题。
- 跑本 PR 最小测试集和对应 human_tests。

第 2 轮：

- 基于最新 diff 再次检查目标遗漏、测试缺口和文档/API/CLI help 是否一致。
- 复跑失败路径和受影响测试。
- 再次确认 `human_tests/readme.md` 已同步。
- 若仍发现 P0/P1/P2 或测试失败，追加第 3 轮，直到关闭。

## 文档更新要求

实现 PR 需要同步：

- `README.md`：Research Pack 功能、配置示例和 CLI 示例。
- CLI help：`bifrost agent research -h`。
- `design/agent-research-pack.md`：保持方案与实现一致。
- `human_tests/agent-research-pack.md` 和 `human_tests/readme.md`。
- 如引入新 provider 类型，补充 provider 配置表。

### 2026-05-14 火山联网搜索 Provider 补充

新增 `volc_web_search` 作为一等 Research provider，而不是通过通用 HTTP provider 临时适配。默认 endpoint 为 `https://open.feedcoopapi.com/search_api/web_search`，默认密钥来源为环境变量 `ARK_TOKEN`，请求使用 `Authorization: Bearer $ARK_TOKEN`。

配置字段覆盖火山 APIKey 接入的核心参数：`search_type`（`web` / `web_summary` / `image`）、`count`、`need_content`、`need_url`、`need_summary`、`content_formats`、`time_range`、`query_rewrite`、`sites`、`block_hosts`、`auth_info_level`、`industry`。后端负责映射为火山文档中的 `Query`、`SearchType`、`Count`、`Filter`、`NeedSummary`、`TimeRange`、`QueryControl`、`ContentFormats`、`Industry`。

响应归一化规则：

- `Result.WebResults[]` 映射为 `ResearchSearchResult`，保留 `Title`、`SiteName`、`Url`、`Summary/Snippet`、`Content`、`PublishTime`、`RankScore`，并生成 `canonical_url`、`content_hash`、`retrieved_at`。
- `Result.ImageResults[]` 暂映射为 web source 的图片型搜索结果，`Image.Url` 放入 snippet，后续如 UI 需要原生图片结构再扩展协议。
- `ResponseMetadata.Error` 转为 provider error，进入 Research runtime 的 provider fallback 机制。

WebUI 要求：

- `Supported Sources` 显示 `Volc Web Search` 的 supported/configured/enabled/authorized 状态。
- `Research Search` 区域必须按 provider 展示选择框，并把选择透传为 `/agent/research/search/stream` 的 `provider_ids`。只选择 `exa` 时后端只尝试 `exa`，不能退回到 Web/WeChat 粗粒度自动混跑。
- `Research Search` 必须按 provider 流式返回结果：后端并发执行 provider，任一 provider 完成后立即输出 NDJSON event，WebUI 立刻 append 到结果列表；`limit=10` 表示每个 provider 最多搜索 10 条，不是所有 provider 共用 10 条。
- `Providers` 区域必须按 provider 类型展示最小配置面：固定站点 `arxiv`、`hacker_news`、`github_repositories` 只有启用开关和测试入口；火山/Tavily/Exa 只配置 env/API key 和各自必要参数；只有 `generic_web_search`、`custom_http` 这类外部/custom provider 展示 URL 输入。不存在 Sogou WeChat CDP Bridge 配置入口。
- `Providers` 区域支持配置火山 `ARK_TOKEN` env key、可选明文 API key、SearchType、Count、正文/URL/摘要/Query Rewrite 开关、ContentFormats、TimeRange、Sites、BlockHosts、AuthInfoLevel、Industry；火山 API 地址作为固定内置地址展示，不要求用户手填 endpoint。
- `Providers` 区域必须对需要凭据的 provider 展示明确获取入口和配置说明：火山指向联网搜索控制台开通页与 API Key 管理页，说明 `ARK_TOKEN` 是本地环境变量名且变量值必须是联网搜索 API Key；Tavily 指向 Tavily Platform 和官方 quickstart；Exa 指向 Exa Dashboard API Keys 与官方 quickstart。UI 要提示推荐通过环境变量配置，明文 direct API key 仅用于本地测试。
- 未设置 `ARK_TOKEN` 且未填 API key 时，capability 显示未授权；设置后显示 authorized/configured。
- `Providers` 配置列表默认直接内置所有 provider 入口：`volc_web_search`、`sogou_wechat_cdp`、`arxiv`、`hacker_news`、`github_repositories`、`generic_web_search`、`tavily`、`exa`、`custom_http`、`mcp`。固定站点默认启用；需要外部凭据、endpoint 或 CDP 的 provider 默认可见但不强制启用。
- 需要额外配置的 provider 必须在同一列表内给配置入口：Sogou CDP 展示 `CDP Endpoint` 与 `Browser Data`，默认提示 Bifrost 专用 Edge 操作目录 `~/.bifrost/web/edge-user-data`；通用/自定义 provider 展示 endpoint；Tavily/Exa 使用内置 endpoint，只展示凭据输入。
- 触发 Sogou WeChat 搜索时，provider 必须自动检测 CDP endpoint；如果不可达，自动使用固定 Edge 操作目录启动 Microsoft Edge CDP 后继续搜索。遇到验证码/授权时，流程应升级为可见浏览器人工处理，处理完成后继续 CDP 抓取，而不是直接失败。

### 2026-05-14 WebUI 直接搜索失败修复

WebUI 的 Research workbench 必须支持首次进入页面后直接输入关键词搜索。搜索按钮在发起 `/agent/research/search` 前静默保存当前 Research 配置，并在配置尚未启用时自动启用，避免用户还没有手动保存 provider 设置时后端返回 `research is not configured`。

Provider fallback 的错误语义调整为：只有所有已尝试 provider 都失败时才返回 `all selected research providers failed`。如果某个 provider 成功请求但返回 0 条结果，同时另一个 provider 报错，接口应返回空结果或其余可用结果，而不是把整次搜索标记为失败。这覆盖了中文关键词 `语音大模型` 下固定站点无命中但微信/Sogou 暂不可用的常见路径。

测试要求补充：

- E2E 使用 `语音大模型` 作为关键词，分别覆盖火山 `volc_web_search`、通用 `generic_web_search`、`tavily`、`exa`、`custom_http`，再执行 web 汇总搜索和 Sogou WeChat CDP 真实搜索。
- 汇总搜索必须验证 provider 覆盖 `volc_mock`、`mock`、`tavily_mock`、`exa_mock`、`custom_mock`；微信来源由 `sogou_wechat_cdp` 真实 CDP 用例覆盖。
- 每条最终结果必须包含 `canonical_url`、`content_markdown`、`content_hash`、`retrieved_at`，证明搜索结果和正文抓取均已完成归一化。
- `/agent/chat` 必须通过 Research tools 集成链路处理 `语音大模型`：`knowledge_search -> research_search -> research_fetch -> knowledge_save -> research_digest`。中文主题保存后必须能被 digest 检索命中，避免 FTS 对连续中文词无结果时生成空报告。
- Agent `research_search` tool 必须使用 provider stream channel：每个 provider 完成时立刻向 turn progress 输出 `ToolProgress`，最终 ToolResult 保留 provider event 数组，供模型按完成顺序消费。

真实 Sogou 微信 CDP 验证要求保持 fail-closed：`agent research provider test sogou_wechat_cdp --query "语音大模型"` 能证明搜索结果解析；详情抓取必须进入原始微信文章 DOM 并读取 `#js_content` 后才能判定通过。如果 Sogou 或微信返回验证码/挑战页，provider 返回错误，human_tests 记录为环境阻断，需要使用 `BIFROST_RESEARCH_CDP_ENDPOINT` 指向已完成验证/登录的本地浏览器后重跑，不能把挑战页正文当成文章正文。

真实 Sogou E2E 脚本补充两条本地验证路径：

- 默认使用 Bifrost 专用 Edge 操作目录 `~/.bifrost/web/edge-user-data` 启动无头 CDP，避免与用户日常 Edge profile 争抢单实例/目录锁；如需复用已验证会话，可显式设置 `BIFROST_RESEARCH_BROWSER_USER_DATA_DIR` 或 `BIFROST_RESEARCH_CDP_ENDPOINT`。
- 遇到验证码/挑战时可设置 `BIFROST_RESEARCH_MANUAL_AUTH=1`，脚本会用 Microsoft Edge 可见窗口打开当前失败的 Sogou `/link` 或微信文章详情 URL，并通过 Edge CDP `/json/list` 断言页面确实已打开；人工完成授权后，脚本关闭可见窗口，使用同一个 Edge profile 回到无头 CDP 并继续执行原文 fetch。

## 残余问题与审查重点

需要审查确认：

1. Admin API 是否暂挂 `/api/im-gateway/agent/research/*`，还是先抽 `/api/agent/research/*`。
2. P1 是否必须包含 WebUI，还是先 CLI + Agent tools + Skill，WebUI 放 P1.5。
3. `generic_web_search` 的 response template 第一版要支持几种格式；建议先支持一种 Bifrost 标准 JSON 和一个 OpenAI-style web search 响应。
4. SQLite 依赖选型：是否接受 `rusqlite bundled` 增加构建体积。
5. `research_digest` 是否允许直接调用主模型，还是必须通过已有 Agent session 以便记录完整会话。
6. Sogou 微信 CDP 是否需要额外人工授权状态检测；默认不暴露 HTTP Bridge endpoint 配置，遇到验证码/挑战时切换到可见 Edge 人工处理。
7. 外部上下文进入后是否要影响 `memories.disable_on_external_context`；建议 P1 只在 Research Skill 里说明“不自动写入长期记忆”，P2 再接 memory pollution metadata。

当前建议：

- 接受 `/api/im-gateway/agent/research/*` 作为短期路径，后续统一 Agent namespace 时再迁移。
- P1 以 CLI + Agent tools + Skill + SQLite + manual digest 为主，WebUI 若排期紧可放 P1.5，但类型和 API 先设计好。
- MCP provider 明确放 P2，避免第一版改动 session loop。

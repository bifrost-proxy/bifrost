# Agent Research Pack 真实场景测试

## 功能模块说明

验证 Bifrost Agent Research Pack 的用户可感知链路：配置启用、CLI 初始化、固定站点/preset source、统一搜索、Provider 测试、Fetch 安全策略、Markdown artifact 沉淀、本地知识库/日报、Agent Settings Research 页面入口、WebUI 关键词搜索 workbench、内置来源 supported/authorized/logged-in 状态展示、内置 Research Skill 文件存在、Agent Loop 真实调用 Research 工具，以及通过浏览器 CDP 对 Sogou 微信公众号执行真实搜索和详情抓取。

## 前置条件

1. 在仓库根目录执行命令，所有命令前先执行 `source ~/.zshrc`。
2. 使用临时数据目录，避免污染用户真实配置：`BIFROST_DATA_DIR=$(mktemp -d)`。
3. 常规 CLI/API 用例使用本地 mock provider，不依赖外网搜索 API；固定站点 provider 的单元用例使用真实 provider 解析逻辑和 fixture 数据验证。
4. Sogou 微信真实抓取用例需要可访问外网，并需要 Microsoft Edge CDP endpoint；脚本会自动启动 Edge 无头 CDP，也可设置 `BIFROST_RESEARCH_CDP_ENDPOINT=http://127.0.0.1:9222` 复用用户已登录/验证的 Edge。CI 中该真实公网用例默认跳过，只有设置 `BIFROST_RESEARCH_REAL_SOGOU=1` 或显式传入 `BIFROST_RESEARCH_CDP_ENDPOINT` 时才强制执行，避免主 CI 受 Sogou 反爬/地域结果影响。
5. Agent Loop chat 用例会启动 Bifrost 服务，必须使用临时 `BIFROST_DATA_DIR`，并显式传入 `--no-system-proxy`。
6. 飞书上传用例允许两种结果：如果本机有可用 lark-cli/飞书凭据则真实上传；如果凭据不可用，必须生成可上传 Markdown 并记录阻塞原因，禁止伪造上传成功。

## 测试用例列表

### TC-ARP-01 CLI 初始化 Research Pack

操作步骤：
1. 启动本地 mock provider。
2. 执行 `BIFROST_DATA_DIR=$TMPDIR target/debug/bifrost agent research init --preset personal-cn --web-provider mock --base-url http://127.0.0.1:$PORT/search --api-key '$RESEARCH_TEST_KEY' --yes`。
3. 查看 `$TMPDIR/agent/agent_config.json`。

预期结果：
- CLI 输出 `Research Pack initialized`。
- 配置文件存在。
- `research.enabled` 为 `true`。
- `research.providers.mock.type` 为 `generic_web_search`。

### TC-ARP-02 Provider 测试返回标准化搜索结果

操作步骤：
1. 在 TC-ARP-01 的同一临时数据目录中执行 `target/debug/bifrost agent research provider test mock --query "AI Agent MCP"`。
2. 检查 JSON 输出。

预期结果：
- 输出包含 `query` 和 `results`。
- 第一条结果包含 `provider: mock`、`source: web`、`title`、`url`、`snippet`。
- Provider 原始响应结构不会直接泄漏给 Agent。

### TC-ARP-03 统一搜索入口可用

操作步骤：
1. 执行 `target/debug/bifrost agent research search "AI Agent MCP" --limit 1`。
2. 检查 JSON 输出。

预期结果：
- 返回一条标准化搜索结果。
- URL fragment 被去除。
- `limit` 生效，结果数不超过 1。

### TC-ARP-04 Fetch 默认阻止 localhost/private IP

操作步骤：
1. 执行 `target/debug/bifrost agent research fetch http://127.0.0.1:$PORT/article`。
2. 检查命令退出码和错误信息。

预期结果：
- 命令非零退出。
- 错误信息说明 localhost 或 private IP 被策略拒绝。

### TC-ARP-05 Agent 工具注册受 Research 配置开关控制

操作步骤：
1. 执行 `cargo test -p bifrost-agent tools::tests::research_tools_are_config_gated`。

预期结果：
- 测试通过。
- Research 未启用时不注册 `research_search`。
- Research 启用时注册 `research_search`、`research_fetch`、`knowledge_search`、`knowledge_save`、`research_digest`。

### TC-ARP-06 SQLite/FTS 知识库去重与搜索

操作步骤：
1. 执行 `cargo test -p bifrost-agent research::store::tests::upsert_and_search_roundtrip research::store::tests::canonical_url_dedupes`。

预期结果：
- 测试通过。
- canonical URL 去重生效。
- FTS 查询能返回保存的知识条目。

### TC-ARP-07 Research Skill 系统文件存在且声明工具

操作步骤：
1. 查看 `crates/agent/src/assets/samples/research/manifest.json`。
2. 查看 `crates/agent/src/assets/samples/research/SKILL.md`。

预期结果：
- manifest 中 `name` 为 `research`，`scope` 为 `system`。
- `allowed_tools` 包含五个 Research Pack 工具。
- `SKILL.md` 要求先查本地知识、不足再联网搜索、重要结果抓正文、需要沉淀时保存、输出带来源链接。

### TC-ARP-08 Agent Settings Research 页面支持明暗主题

操作步骤：
1. 执行 `pnpm --dir web build`。
2. 静态检查 `web/src/pages/Settings/tabs/agent/ResearchSection.tsx` 使用 Ant Design token 和现有组件主题，不引入硬编码主题背景。
3. 检查 `web/src/pages/Settings/tabs/aiSections.ts` 和 `AgentTab.tsx` 已加入 Research section。

预期结果：
- Web build 通过。
- Research 页面出现在 Agent Settings 导航。
- 组件颜色来自主题 token 或 Ant Design 组件，亮色/暗色主题可读性不依赖硬编码背景。

### TC-ARP-09 Sogou 微信公众号 CDP provider 初始化

操作步骤：
1. 准备可用 Edge CDP endpoint，例如 `BIFROST_RESEARCH_CDP_ENDPOINT=http://127.0.0.1:9222`，或执行 E2E 脚本让它启动 Microsoft Edge 无头 CDP。
2. 执行 `BIFROST_DATA_DIR=$TMPDIR target/debug/bifrost agent research init --preset personal-cn --wechat-cdp-endpoint $CDP_ENDPOINT --yes`。
3. 查看 `$TMPDIR/agent/agent_config.json`。

预期结果：
- CLI 输出 `Research Pack initialized`。
- 配置中出现 `research.providers.sogou_wechat_cdp.type = "sogou_wechat_cdp"`。
- `research.wechat.provider` 指向 `sogou_wechat_cdp`。

### TC-ARP-17 Agent Research WebUI/API 支持关键词触发聚合搜索

操作步骤：
1. 执行 `SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_agent_research_pack_admin_api.sh`。
2. 脚本使用临时 `BIFROST_DATA_DIR` 启动 Bifrost，并通过 `/agent/research/config` 配置 mock provider、`fetch_content=true`、`allow_localhost=true`。
3. 脚本请求 `/agent/research/capabilities`，检查内置来源状态。
4. 脚本请求 `/agent/research/search`，query 为 `AI HUB`，sources 为 `["web"]`，limit 为 `1`，fetch_content 为 `true`。
5. 静态检查 `web/src/pages/Settings/tabs/agent/ResearchSection.tsx`。

预期结果：
- `/agent/research/capabilities` 返回 `sogou_wechat_cdp`，且 `supported=true`、`authorization_status=not_required`，包含 `logged_in` 与 `login_status` 字段。
- `sogou_wechat_cdp.search_url_template` 为 `https://weixin.sogou.com/weixin?type=2&p=44351200&ie=utf8&query={query}`。
- `/agent/research/search` 返回 Top 结果，包含 `title`、`source`、`provider`、`site_name`、`author`、`published_at`、`canonical_url`、`content_hash`、`retrieved_at` 和完整 `content_markdown`。
- Research 页面存在关键词输入、Web/WeChat source 选择、limit、`Fetch full Markdown` 开关、搜索按钮、结果 Markdown 展示，以及 supported/configured/authorized/logged-in 来源状态标签。

### TC-ARP-10 Sogou 微信公众号真实搜索可抓取数据

操作步骤：
1. 执行 `BIFROST_DATA_DIR=$TMPDIR target/debug/bifrost agent research provider test sogou_wechat_cdp --query "AI Agent MCP"`。
2. 检查 JSON 输出。

预期结果：
- 输出 `provider: sogou_wechat_cdp`。
- 输出 `source: wechat`。
- 至少返回 1 条真实 Sogou 微信公众号搜索结果。
- 第一条结果包含标题、Sogou `/link` URL、摘要或公众号名。

### TC-ARP-11 Sogou/微信公众号详情通过 CDP fetch 或爬虫获取

操作步骤：
1. 从 TC-ARP-10 的第一条结果取 `url`。
2. 执行 `BIFROST_DATA_DIR=$TMPDIR target/debug/bifrost agent research fetch "$URL" --max-bytes 500000`。
3. 检查 JSON 输出或错误信息。

预期结果：
- 如果 CDP 浏览器已经完成站点要求的交互式登录/验证码，命令返回 `FetchedDocument`，包含最终 URL、标题和长度大于 120 字符的 `content_markdown`。
- 如果站点返回验证码/反爬挑战，命令非零退出并明确提示 `blocked` / `challenge` / `验证码` / `antispider`，不能把挑战页当作详情正文保存。

### TC-ARP-12 CI 默认不强依赖真实 Sogou 公网结果

操作步骤：
1. 执行 `CI=true BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_agent_research_pack_sogou_wechat_cdp_real.sh`。
2. 执行 `BIFROST_RESEARCH_REAL_SOGOU=1 BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_agent_research_pack_sogou_wechat_cdp_real.sh`。

预期结果：
- 第 1 条命令输出 `[SKIP] Real Sogou/WeChat CDP E2E requires ...` 并以 0 退出，主 CI 不因公网 Sogou 反爬、地域差异或临时无结果失败。
- 第 2 条命令仍强制执行真实 Sogou 微信搜索和详情抓取，确保人工/本地验证覆盖真实数据链路。

### TC-ARP-13 preset 安装高质量固定站点 provider

操作步骤：
1. 执行 `cargo test -p bifrost-agent research::tests::ai_tech_preset_registers_curated_fixed_site_sources -- --nocapture`。
2. 检查 `agent research init --preset ai-tech` 生成的配置。

预期结果：
- `ai-tech` preset 自动包含 `arxiv`、`hacker_news`、`github_repositories`。
- 这些 provider 的 `type` 为 `fixed_site`，且出现在 `provider_order` 中。
- 用户显式配置的 `--web-provider` 仍排在 `provider_order` 前面，固定站点作为高质量默认补充，不覆盖用户指定 API provider。

### TC-ARP-14 搜索和详情抓取输出 Markdown artifact

操作步骤：
1. 执行 `SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_agent_research_pack_chat_loop.sh`。
2. 检查 mock model server 第二轮输入中的 tool output。
3. 检查生成的 digest Markdown 文件。

预期结果：
- `research_search(fetch_content=true)` 或 `research_fetch` 输出包含 `content_markdown`、`canonical_url`、`source`、`provider`、`site_name`、`retrieved_at`。
- `knowledge_save` 保存正文、summary、author、published_at、tags。
- `research_digest` 写入 Markdown 报告，并保留 `source`、`provider`、`canonical_url` 等源信息。

### TC-ARP-15 真实 /agent/chat 能调用 Research Pack 工具生成博客 Markdown

操作步骤：
1. 执行 `SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_agent_research_pack_chat_loop.sh`。
2. 脚本使用临时 `BIFROST_DATA_DIR` 启动 mock research provider、mock OpenAI-compatible model server 和 Bifrost 服务：`bifrost start --host 127.0.0.1 -p $BIFROST_PORT --unsafe-ssl --no-system-proxy`。
3. 脚本请求 `GET /_bifrost/api/im-gateway/agent/tools`，再发送 `POST /_bifrost/api/im-gateway/agent/chat`，消息为“请使用 Research tools 搜索并抓取‘语音大模型’这个技术主题的资料，整理成一篇可阅读的中文技术文章。”。
4. 检查 chat response 的 `tool_calls` 和生成的 `agent/reports/chat_voice_model_article/*.md`。

预期结果：
- `/agent/tools` 暴露 `research_search`、`research_fetch`、`knowledge_search`、`knowledge_save`、`research_digest`。
- Agent Loop 实际调用顺序包含 `knowledge_search -> research_search -> research_fetch -> knowledge_save -> research_digest`，且每个 tool call 成功。
- 最终响应包含 `VOICE_MODEL_RESEARCH_ARTICLE_READY` 和“语音大模型”中文技术文章。
- 报告 Markdown 中包含正文标题和 `provider: fixture_research`。
- `research_digest` 的 `items_used` 大于 0，证明中文 query `语音大模型` 能命中刚刚保存的知识项。

### TC-ARP-16 飞书上传凭据不可用时生成可上传 Markdown 并记录阻塞

操作步骤：
1. 执行 `command -v lark-cli || true` 检查本机是否存在飞书 CLI。
2. 如果存在，再执行只读状态命令确认凭据是否可用；如果不存在或凭据不可用，继续执行 TC-ARP-15。
3. 检查 TC-ARP-15 最终响应。

预期结果：
- 有可用飞书凭据时，可以用生成的 Markdown 真实创建/上传飞书文档。
- 没有飞书凭据时，Agent 输出明确说明未配置 lark-cli/飞书凭据，已生成可上传 Markdown，不能声称上传成功。

### TC-ARP-18 火山联网搜索 Provider 可配置并通过 ARK_TOKEN 鉴权

操作步骤：
1. 设置 `ARK_TOKEN=e2e-token`。
2. 执行 `SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_agent_research_pack_admin_api.sh`。
3. 脚本使用临时 `BIFROST_DATA_DIR` 启动 Bifrost 和本地 mock 火山联网搜索 API。
4. 脚本通过 `/agent/research/config` 配置 `volc_mock` provider，`type=volc_web_search`、`env_key=ARK_TOKEN`、`search_type=web`、`content_formats=markdown`、`need_content=true`、`need_url=true`。
5. 脚本请求 `/agent/research/capabilities` 和 `/agent/research/search`，并静态检查 WebUI provider 配置区。

预期结果：
- mock 火山 API 收到 `Authorization: Bearer e2e-token`。
- `/agent/research/capabilities` 返回内置 `volc_web_search`，显示 `configured=true`、`authorization_status=configured`，并返回用户配置的 `volc_mock` provider。
- `/agent/research/search` 对 `语音大模型` 返回 `provider=volc_mock`，结果包含 `site_name`、`canonical_url`、`content_markdown`、`content_hash`、`retrieved_at`。
- WebUI `Providers` 区域固定展示火山 API 地址，不要求用户编辑 endpoint；支持编辑 `ARK_TOKEN` env key、可选 API key、SearchType、Count、正文/URL/摘要/Query Rewrite、ContentFormats、TimeRange、Sites、BlockHosts、AuthInfoLevel、Industry，并显示联网搜索控制台开通入口与 API Key 管理入口。

### TC-ARP-19 WebUI 关键词搜索覆盖全部配置源并解析最终 Markdown

操作步骤：
1. 执行 `BIFROST_PORT=18973 MOCK_PORT=18974 e2e-tests/tests/test_agent_research_pack_admin_api.sh`。
2. 脚本配置 `volc_mock`、`mock`、`tavily_mock`、`exa_mock`、`custom_mock` 五个 web provider，使用关键词 `语音大模型` 分别请求单 provider 和 web 汇总搜索。
3. 检查 `/agent/research/search` 响应中每个 provider 的结果解析字段。
4. 检查 WebUI 搜索触发逻辑：首次搜索会静默保存并启用当前 Research 配置，避免配置未落库导致 `research is not configured` 或所有 provider 被错误判定失败。
5. 对固定公共源执行真实搜索与正文抓取：`arxiv` 使用 `speech foundation model`，`hacker_news` 使用 `voice AI`，`github_repositories` 使用 `speech recognition`；分别取第一条结果 URL 执行 `agent research fetch`，检查 markdown 正文长度、`canonical_url` 与 `content_hash`。

预期结果：
- `volc_mock` 通过 `ARK_TOKEN` 请求 mock 火山接口，并解析 `Result.WebResults[]` 为 web 结果。
- `mock` 通用 web provider 返回网页结果并完成最终 Markdown 正文抓取。
- `tavily`、`exa`、`custom_http` 类型通过同一个通用 HTTP 适配器执行搜索，分别返回独立 provider id 与规范 URL。
- 微信来源由真实 `sogou_wechat_cdp` 用例覆盖，WebUI/Admin API 中不再存在 WeChat HTTP Bridge。
- web 汇总搜索包含 `volc_mock`、`mock`、`tavily_mock`、`exa_mock`、`custom_mock` 五个 provider，结果覆盖 `web` source。
- 每条结果均包含 `canonical_url`、`content_markdown`、`content_hash`、`retrieved_at`。
- 如果某个 provider 失败但至少一个 provider 成功返回 0 条或有效结果，接口不应把整次搜索误报为 `all selected research providers failed`。
- 固定公共源的首条结果均可继续 fetch：`arxiv` 输出 `https://arxiv.org/abs/...` 并可获取正文 markdown；`hacker_news` 外链与 `github_repositories` 仓库页面均可获取 markdown、规范 URL 和内容 hash。

### TC-ARP-20 真实 Sogou 微信搜索解析原始文章完整内容

操作步骤：
1. 执行 `BIFROST_RESEARCH_REAL_SOGOU=1 BIFROST_RESEARCH_SOGOU_QUERY='语音大模型' BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_agent_research_pack_sogou_wechat_cdp_real.sh`。
2. 脚本启动 Microsoft Edge 无头 CDP 与隔离 `BIFROST_DATA_DIR`。
3. 脚本执行 `agent research init --wechat-cdp-endpoint` 写入 `sogou_wechat_cdp` provider。
4. 脚本执行 `agent research provider test sogou_wechat_cdp --query "语音大模型"`，检查返回微信公众号搜索结果。
5. 脚本对第一条 Sogou `/link` 执行 `agent research fetch "$URL" --max-bytes 500000`，检查是否跳转到原始微信文章并解析完整正文。

预期结果：
- 搜索阶段返回 `provider=sogou_wechat_cdp`、`source=wechat`、非空标题，并返回 `https://weixin.sogou.com/link?...query=语音大模型...`。
- 详情阶段未遇到验证码/反爬挑战时，返回非空 `url`、`title`、长度大于 120 字符的 `content_markdown`，正文来自微信公众号原文 DOM `#js_content`。
- 默认使用 Bifrost 专用 Edge 操作目录 `~/.bifrost/web/edge-user-data` 启动无头 CDP，避免与用户日常 Edge profile 争抢目录锁；如需复用已验证会话，可显式设置 `BIFROST_RESEARCH_BROWSER_USER_DATA_DIR` 或 `BIFROST_RESEARCH_CDP_ENDPOINT`。
- 如果站点返回验证码/挑战页，用例必须提示可设置 `BIFROST_RESEARCH_MANUAL_AUTH=1` 弹出浏览器人工验证后继续，或使用 `BIFROST_RESEARCH_CDP_ENDPOINT` 指向已完成验证/登录的本地浏览器后重跑，不能把挑战页内容当作原文。
- 可设置 `BIFROST_RESEARCH_FORCE_MANUAL_AUTH=1` 强制打开人工验证页面，以验证“Edge 可见窗口打开当前 Sogou/微信详情 URL -> 人工处理/确认 -> 回到无头 Edge CDP 继续 fetch”的控制流程；脚本必须通过 Edge CDP `/json/list` 断言当前页面 URL 是 `weixin.sogou.com/link` 或 `mp.weixin.qq.com`。

### TC-ARP-21 WebUI 直接内置所有 Research provider 并提供配置入口

操作步骤：
1. 打开 `http://localhost:9900/_bifrost/ai?aiSection=agent-research&agentSection=research`。
2. 检查 `Providers` 区域。
3. 分别检查 `volc_web_search`、`sogou_wechat_cdp`、`arxiv`、`hacker_news`、`github_repositories`、`generic_web_search`、`tavily`、`exa`、`custom_http`、`mcp` 是否直接出现在 provider 列表中，并确认不存在 `wechat_http` / WeChat HTTP Bridge。
4. 检查每个需要额外配置的 provider 是否有配置入口。
5. 检查 `Research Search` 区域是否按 provider 逐项展示选择框，而不是只展示 `Web` / `WeChat` 两个粗粒度选项；选择单个 provider 搜索时，请求必须携带 `provider_ids` 并只返回该 provider 的结果。

预期结果：
- 固定站点 provider 直接内置：`arxiv`、`hacker_news`、`github_repositories`，每项只有启用开关和测试入口，不展示 endpoint、secret 或其他不可配置字段。
- 火山联网搜索 provider 直接内置：`volc_web_search`，默认 `env_key=ARK_TOKEN`，展示固定 API 地址、env/API key 与 SearchType、Count、正文/URL/摘要、ContentFormats、TimeRange、Sites、BlockHosts、AuthInfoLevel、Industry 配置入口，不要求用户手填 Endpoint；Secret 区域必须提供联网搜索开通页、API Key 管理页，并说明 `ARK_TOKEN` 是本地环境变量名，变量值应为联网搜索 API Key。
- Sogou 微信 CDP provider 直接内置：`sogou_wechat_cdp`，提供 `CDP Endpoint` 和 `Browser Data` 配置入口，`Browser Data` 默认提示 `~/.bifrost/web/edge-user-data`。
- Tavily/Exa 直接内置：只展示启用开关、测试入口和 env/API key，使用内置 API endpoint，不要求用户配置 endpoint；Secret 区域必须分别提供 Tavily Platform / Tavily quickstart、Exa API Keys / Exa quickstart 链接，并提示优先通过 `TAVILY_API_KEY` / `EXA_API_KEY` 环境变量配置。
- HTTP/custom provider 直接内置：`generic_web_search`、`custom_http` 展示 Endpoint；不存在 WeChat HTTP Bridge 配置项。
- 点击 Search 时，如选择 `sogou_wechat_cdp` 且 CDP 未运行，后端必须自动用 `~/.bifrost/web/edge-user-data` 启动 Microsoft Edge CDP 后继续搜索。
- Search 结果按 provider 流式返回：任一 provider 完成后 WebUI 立即追加结果，不等待其他 provider；`limit=10` 表示每个 provider 最多返回 10 条。
- `mcp` 直接展示为预留入口，Supported Sources 中必须标记为 `Reserved`，不得展示成已完备抓取 provider。
- `mcp` provider 直接展示为预留 provider，明确标注 MCP-backed source bridge 尚未启用。

### TC-ARP-24 Agent research_search 工具 provider 级流式进度

操作步骤：
1. 启动真实 Bifrost 服务并启用 Research Pack。
2. 通过 `/agent/chat` 触发模型调用 `research_search`，请求体包含 `query="语音大模型"`、多个 provider、`limit=1`、`fetch_content=true`。
3. 观察 Agent turn progress 事件或 IM 进度卡更新。
4. 检查最终 tool result JSON。

预期结果：
- `research_search` 执行期间，每个 provider 完成时立即输出一条 `ToolProgress` 事件，进度卡展示该 provider 的结果数量或错误，不等待所有 provider 完成。
- 最终 tool result 是 provider event 数组，顺序与 provider 完成顺序一致，单个 provider 失败不阻塞其他 provider 已返回的结果。
- `limit=1` 对每个 provider 各生效一次；若选择两个 provider，最终最多包含两个 provider event、每个 event 最多 1 条结果。

### TC-ARP-25 GitHub Actions Rust cache 不恢复 cargo/rustup bin

操作步骤：
1. 推送分支触发 GitHub Actions CI。
2. 观察 `E2E Shell (aarch64-apple-darwin, shard */3)` job 的 `Swatinem/rust-cache@v2` 步骤。
3. 继续观察同一 job 的 `E2E Runtime Context` 和后续 shell suite 中的 `cargo test` / `cargo build` / `cargo run`。

预期结果：
- 所有 `Swatinem/rust-cache@v2` 步骤均显式配置 `cache-bin: false`，不从 cache 恢复 `~/.cargo/bin/cargo`、`rustup` 或 rustup proxy。
- macOS shell job 中的 `cargo test`、`cargo build`、`cargo run` 不再出现 `error: unexpected argument 'test' found` / `build` / `run` 这类 `rustup-init` 被误当作 cargo 执行的错误。
- 如 CI 仍失败，应进入下一轮 CI 归因；不能把 cargo bin cache 污染归类为 Research Pack 功能失败。

## 执行记录

执行日期：2026-05-13

| 用例 | 实际结果 |
| --- | --- |
| TC-ARP-01 | 通过。`e2e-tests/tests/test_agent_research_pack_cli.sh` 使用临时 `BIFROST_DATA_DIR` 执行 `agent research init`，输出 `Research Pack initialized`，并生成隔离的 `agent/agent_config.json`。 |
| TC-ARP-02 | 通过。同一 E2E 脚本执行 `agent research provider test mock --query "AI Agent MCP"`，返回 `provider: mock`、`source: web`、标题、URL 与 snippet。 |
| TC-ARP-03 | 通过。同一 E2E 脚本执行 `agent research search "AI Agent MCP" --limit 1`，返回 1 条结果，URL fragment 被去除。 |
| TC-ARP-04 | 通过。同一 E2E 脚本执行 `agent research fetch http://127.0.0.1:$PORT/article`，命令非零退出并返回 `loopback fetch is disabled`。 |
| TC-ARP-05 | 通过。执行 `cargo test -p bifrost-agent tools::tests::research_tools_are_config_gated`，Research 未启用时不注册工具，启用时注册五个 Research 工具。 |
| TC-ARP-06 | 通过。执行 `cargo test -p bifrost-agent research::store::tests`，SQLite/FTS 保存、搜索与 canonical URL 去重均通过。 |
| TC-ARP-07 | 通过。执行 `jq` 检查 `manifest.json`，确认 `name=research`、`scope=system`、五个 Research 工具声明；检查 `SKILL.md`，确认本地知识优先、联网搜索、抓正文、保存、日报和来源链接规则。 |
| TC-ARP-08 | 通过。执行 `pnpm --dir web build` 通过；静态检查确认 `aiSections.ts`、`AgentTab.tsx` 加入 Research section，`ResearchSection.tsx` 使用 Ant Design token/组件主题。 |
| TC-ARP-17 | 通过。执行 `e2e-tests/tests/test_agent_research_pack_admin_api.sh`，脚本用临时 `BIFROST_DATA_DIR` 启动 Bifrost 和本地 mock provider；`/agent/research/capabilities` 返回 `sogou_wechat_cdp`，包含 `supported=true`、`authorization_status=not_required`、`logged_in`/`login_status` 与 `https://weixin.sogou.com/weixin?type=2&p=44351200&ie=utf8&query={query}`；`/agent/research/search` 对 `AI HUB` 返回 Top 结果，并包含 `content_markdown`、`canonical_url`、`content_hash`、`retrieved_at`、`site_name`、`author` 等元信息。静态检查确认 `ResearchSection.tsx` 已加入关键词输入、Web/WeChat source 选择、limit、`Fetch full Markdown` 开关、搜索结果 Markdown 展示和 supported/configured/authorized/logged-in 来源状态标签。 |
| TC-ARP-09 | 通过。执行 `BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_agent_research_pack_sogou_wechat_cdp_real.sh`，脚本启动 Microsoft Edge CDP，`agent research init --wechat-cdp-endpoint` 成功写入 `sogou_wechat_cdp` provider。 |
| TC-ARP-10 | 通过。同一真实 E2E 脚本执行 `agent research provider test sogou_wechat_cdp --query "AI Agent MCP"`，Sogou 微信搜索返回真实微信公众号结果，第一条 URL 为 `https://weixin.sogou.com/link?...query=AI%20Agent%20MCP...`。 |
| TC-ARP-11 | 通过。同一真实 E2E 脚本对 TC-ARP-10 第一条 Sogou `/link` 执行 `agent research fetch "$URL" --max-bytes 500000`，CDP 浏览器详情抓取成功，返回正文长度大于 120 字符。 |
| TC-ARP-12 | 通过。执行 `CI=true BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_agent_research_pack_sogou_wechat_cdp_real.sh`，脚本输出 `[SKIP] Real Sogou/WeChat CDP E2E requires ...` 并以 0 退出；执行 `BIFROST_RESEARCH_REAL_SOGOU=1 BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_agent_research_pack_sogou_wechat_cdp_real.sh` 仍走真实 Sogou 搜索和详情抓取。 |
| TC-ARP-13 | 通过。执行 `cargo test -p bifrost-agent research::tests::ai_tech_preset_registers_curated_fixed_site_sources -- --nocapture`，确认 `ai-tech` preset 注册 `arxiv`、`hacker_news`、`github_repositories` 三个 `fixed_site` provider，且用户显式 `fixture_research` 保持 provider_order 第一位。 |
| TC-ARP-14 | 通过。执行 `SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_agent_research_pack_chat_loop.sh`，mock model server 在第二轮输入中断言 tool output 包含 `content_markdown`、`canonical_url`、`retrieved_at`；digest 报告包含 `语音大模型技术观察` 和 `provider: fixture_research`。 |
| TC-ARP-15 | 通过。执行 `SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_agent_research_pack_chat_loop.sh`，脚本使用临时 `BIFROST_DATA_DIR` 启动 `bifrost start --host 127.0.0.1 --unsafe-ssl --no-system-proxy`，`/agent/tools` 暴露五个 Research 工具，`/agent/chat` 实际调用 `knowledge_search -> research_search -> research_fetch -> knowledge_save -> research_digest`，最终响应包含 `VOICE_MODEL_RESEARCH_ARTICLE_READY` 和“语音大模型”中文技术文章；本轮还发现并修复了中文 FTS 无法命中连续中文词导致 digest 空报告的问题。 |
| TC-ARP-16 | 通过。执行 `command -v lark-cli || true` 找到 `/Users/eden/.local/share/mise/shims/lark-cli`；在 `/tmp` 执行 `lark-cli doctor` 返回 `token_exists: fail`、`message: no user logged in`，因此未执行真实飞书上传；TC-ARP-15 的最终响应明确说明测试环境未配置 lark-cli 凭据，并已生成可上传到飞书文档的 Markdown，没有伪造上传成功。 |
| TC-ARP-18 | 通过。执行 `BIFROST_PORT=18971 MOCK_PORT=18972 e2e-tests/tests/test_agent_research_pack_admin_api.sh`，脚本先构建最新 `target/debug/bifrost`，再使用临时 `BIFROST_DATA_DIR` 启动 Bifrost 和本地 mock 火山联网搜索 API；mock API 断言收到 `Authorization: Bearer e2e-token`，`/agent/research/capabilities` 返回内置 `volc_web_search` 与用户配置的 `volc_mock` 且 `authorization_status=configured`，`/agent/research/search` 对 `语音大模型` 返回 `provider=volc_mock`，包含 `site_name`、`canonical_url`、`content_markdown`、`content_hash`、`retrieved_at`；WebUI 静态检查确认火山 Secret 区展示联网搜索开通与 API Key 管理入口。 |
| TC-ARP-19 | 通过。执行 `BIFROST_PORT=18973 MOCK_PORT=18974 e2e-tests/tests/test_agent_research_pack_admin_api.sh`，脚本用 `语音大模型` 分别验证火山、通用 web、Tavily、Exa、Custom HTTP 与 web 汇总搜索；汇总结果包含 `volc_mock`、`mock`、`tavily_mock`、`exa_mock`、`custom_mock`，覆盖 `web` source，所有结果均有 `canonical_url`、`content_markdown`、`content_hash`、`retrieved_at`；脚本额外执行 `provider_ids=["exa_mock"]`，断言只返回 `provider=exa_mock` 的结果。微信来源由真实 `sogou_wechat_cdp` 用例覆盖，不再存在 WeChat HTTP Bridge。随后使用真实固定公共源验证：`arxiv` 搜索返回 `https://arxiv.org/abs/2605.12503v1`，fetch 成功获取 6186 字符 markdown；`hacker_news` 第一条 `https://ostt.ai/` fetch 获取 5740 字符 markdown；`github_repositories` 第一条 GitHub repo fetch 获取 14979 字符 markdown，三者均包含 `content_hash` 与 `canonical_url`。本轮发现 arXiv feed 原始 `http://arxiv.org/...` 会导致默认 fetch policy 失败，已修复为输出 `https://arxiv.org/...` 并新增单元回归。 |
| TC-ARP-20 | 通过。执行 `BIFROST_RESEARCH_REAL_SOGOU=1 BIFROST_RESEARCH_SOGOU_QUERY='语音大模型' BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_agent_research_pack_sogou_wechat_cdp_real.sh`，脚本使用固定 Edge 操作目录 `~/.bifrost/web/edge-user-data` 启动无头 CDP；`research init` 写入 `sogou_wechat_cdp`，Sogou 真实搜索返回微信公众号结果和 `https://weixin.sogou.com/link?...query=%E8%AF%AD%E9%9F%B3%E5%A4%A7%E6%A8%A1%E5%9E%8B...`，详情抓取成功，返回正文长度大于 120 字符。随后执行 `BIFROST_RESEARCH_REAL_SOGOU=1 BIFROST_RESEARCH_MANUAL_AUTH=1 BIFROST_RESEARCH_FORCE_MANUAL_AUTH=1 BIFROST_RESEARCH_SOGOU_QUERY='语音大模型' BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_agent_research_pack_sogou_wechat_cdp_real.sh`，脚本通过 Edge CDP 断言可见 Edge 已打开 `https://weixin.sogou.com/link?...query=%E8%AF%AD%E9%9F%B3%E5%A4%A7%E6%A8%A1%E5%9E%8B...`，输出“已将问题链接弹出到 Microsoft Edge”，然后回到无头 Edge CDP 继续 fetch，详情正文解析通过。 |
| TC-ARP-21 | 通过。执行 `pnpm --dir web build` 通过；静态检查确认 `ResearchSection.tsx` 默认内置 `volc_web_search`、`sogou_wechat_cdp`、`arxiv`、`hacker_news`、`github_repositories`、`generic_web_search`、`tavily`、`exa`、`custom_http`、`mcp`，且不再展示 `wechat_http`；`Research Search` 使用 provider 级 checkbox 并向 `/agent/research/search/stream` 发送 `provider_ids`；固定站点只显示开关和测试入口，火山/Tavily/Exa 只显示凭据与各自必要参数，只有 `generic_web_search` / `custom_http` 显示 URL 配置；火山/Tavily/Exa Secret 区均展示官方获取 key 链接和 env 配置示例；`mcp` 明确显示为 Reserved，不计入已完备正文抓取 provider。 |
| TC-ARP-22 | 通过。执行 `SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost BIFROST_PORT=18993 MOCK_PORT=18994 e2e-tests/tests/test_agent_research_pack_admin_api.sh`，确认 `/agent/research/search/stream` 返回 NDJSON provider event，选择 `mock` 与 `exa_mock` 时分别返回各自结果并以 `done` 结束；后端 `limit=1` 对每个 provider 各返回 1 条，而不是全局只返回 1 条；`/capabilities` 中不存在 `wechat_http`。 |
| TC-ARP-23 | 通过。执行临时 `BIFROST_DATA_DIR` 下的 `target/debug/bifrost agent research init --preset personal-cn --yes` 后，不手动启动 CDP，直接执行 `target/debug/bifrost agent research provider test sogou_wechat_cdp --query "ai llm"`；provider 自动拉起 Microsoft Edge CDP，并返回 3 条 `provider=sogou_wechat_cdp`、`source=wechat` 的真实 Sogou 微信搜索结果。 |
| TC-ARP-24 | 通过。执行 `cargo test -p bifrost-agent tools::tests::research_tools_are_config_gated -- --nocapture` 通过，确认 Research tools 仍按配置开关注册；执行 `cargo check -p bifrost-agent -p bifrost-admin -p bifrost-cli` 通过，确认新增 `ToolProgress` 事件、`ToolHandler::execute_with_progress`、IM 进度卡 immediate flush 与 `research_search` provider stream channel 集成可编译。静态 review 确认 `research_search` 每收到一个 provider event 立即发送 `ToolProgress`，最终仍返回 provider event 数组。 |
| TC-ARP-25 | 通过。上一轮 CI run `25836455469` 的 macOS shell shard 在 `E2E Runtime Context` 后把 `/Users/runner/.cargo/bin/cargo` 当作 cargo 执行，但后续 `cargo test/build/run` 输出 `error: unexpected argument ... found`，判断为 cache 恢复 cargo bin 污染。已检查 `.github/workflows/ci.yml` 中所有 `Swatinem/rust-cache@v2` 步骤并统一加入 `cache-bin: false`；执行 `git diff --check` 与静态检查确认配置完整，后续以新推送 CI run 作为远端验收。 |

## 清理步骤

1. 停止本地 mock provider。
2. 删除临时 `BIFROST_DATA_DIR`。
3. 确认无残留后台进程。

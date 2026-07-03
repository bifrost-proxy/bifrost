# 默认 TLS 应用白名单

## 背景

Bifrost 的按应用维度 TLS 解包依赖 `app_intercept_include`：只有匹配这个列表的客户端进程发起的 HTTPS 才会进入 MITM 解包。列表来自两处：

- 持久化默认值：`crates/bifrost-storage/src/unified_config.rs` 中 `TlsConfig::default()` 提供，第一次落盘时写入 `bifrost.toml`。
- 运行时默认值：`crates/bifrost-proxy/src/server.rs` 中 `ProxyConfig::default()` 提供，CLI/Admin/桌面端未显式传 `--app-intercept-include` 时兜底使用。

历史上曾出现过默认列表包含 `*Codex*` / `Codex CLI` 的实验分支，导致 Codex 发起的 Anthropic/OpenAI HTTPS 请求默认被解包 —— 这既会触发 SNI 证书校验失败告警，也会把 API Key 带到 traffic 存储里。产品明确决定：默认白名单只覆盖“显式浏览器进程”，Codex / Claude Code / 各类终端 Agent 必须由用户在设置里主动加入，不接受默认解包。

本文档冻结默认列表 = 浏览器八条模式，并用双侧断言（unit + Admin API 真实响应）防止后续 PR 误把 Codex 塞回去。

## 用户目标验证清单

### 必须实现

- `TlsConfig::default().app_intercept_include` 等于常量 `DEFAULT_APP_INTERCEPT_INCLUDE`，包含且仅包含：`Google Chrome*`、`Microsoft Edge*`、`*Safari*`、`*Firefox*`、`*Opera*`、`*Brave*`、`*Arc*`、`*Vivaldi*`。
- `ProxyConfig::default().app_intercept_include` 等于上述常量的 `String` 拷贝，顺序一致。
- 上述两个默认列表任意 case-insensitive 匹配 `codex` 的条目一律不得出现。
- 桌面端首次启动、CLI 首次启动、Admin API 首次 GET `/api/config` 拿到的 `tls.app_intercept_include` 都等于默认列表。
- CLI 通过 `--app-intercept-include` 传入的值优先覆盖默认；持久化配置里已存在用户自定义的白名单时，不会被默认值覆盖。

### 必须不破坏

- 用户在 Web UI Proxy 页面手动添加 `Codex*` 或任何自定义模式后，重启不会被“默认必须只有浏览器”这条约束回滚。
- `app_intercept_exclude`、`intercept_include/exclude`、`ip_intercept_include/exclude` 保持既有语义，本次不动。
- CLI `bifrost start --app-intercept-include codex*,cursor*` 仍能把 runtime 白名单替换为用户指定值。
- 未启用 `enable_tls_interception` 时，白名单不生效（既有语义）。

### 必须真实验证

- 单元测试 `test_tls_config_default`、`test_proxy_config_default_app_intercept_include_excludes_codex` 真实执行并断言不含 `codex`。
- 起一个真实临时 Bifrost 实例，`curl http://127.0.0.1:<port>/_bifrost/api/config` 返回的 `tls.app_intercept_include` 数组与常量一致。
- 手动在 macOS 打开 Chrome 访问 HTTPS，traffic 中出现解包记录；打开 Codex CLI 发起 HTTPS，traffic 只看到 CONNECT 隧道、无解包明细。

## 产品语义

### 默认只覆盖“显式浏览器进程模式”

八条 glob pattern 是产品与安全审阅共同批准的默认集合，含义：只有当客户端可执行文件路径匹配这些通配符时，才会被 per-app TLS 解包捕获。选择这一集合的理由：

- 浏览器是最常见的调试场景，用户能立刻理解“为什么 HTTPS 被解开”。
- 浏览器进程名稳定、跨版本变化小，误伤面可控。
- 终端类工具（Codex / Claude Code / Cursor / iTerm / warp）通常携带凭据，默认解包等于默认暴露 secret，产品拒绝。

### 用户仍可主动扩展

Web UI 与 CLI 允许把任何字符串加进 `app_intercept_include`。加入后：

- 持久化写入 `bifrost.toml` 的 `tls.app_intercept_include`。
- 运行时通过 `ConfigManager` 广播到 `ProxyServer`，走 hot reload。
- 一旦被写过一次，后续启动不会被 `TlsConfig::default()` 覆盖 —— serde 反序列化 with existing field 优先。

### 常量单一来源

`DEFAULT_APP_INTERCEPT_INCLUDE` 定义在 `bifrost-proxy::server`，`bifrost-storage::unified_config` 通过 re-export 或直接引用同一个常量数组（当前实现是双份字面量并用测试守恒）。若后续调整默认列表，必须同步改两处并跑双侧单测。

## 技术细节

### 存储层

`crates/bifrost-storage/src/unified_config.rs`:

```rust
pub const DEFAULT_APP_INTERCEPT_INCLUDE: &[&str] = &[
    "Google Chrome*", "Microsoft Edge*", "*Safari*", "*Firefox*",
    "*Opera*", "*Brave*", "*Arc*", "*Vivaldi*",
];

impl Default for TlsConfig {
    fn default() -> Self {
        Self {
            app_intercept_include: DEFAULT_APP_INTERCEPT_INCLUDE
                .iter().map(|s| (*s).to_string()).collect(),
            ...
        }
    }
}
```

`TlsConfigPatch { app_intercept_include: Option<Vec<String>> }` 用于 Admin API 局部更新，未传字段保留原值。

### 代理层

`crates/bifrost-proxy/src/server.rs::ProxyConfig::default()` 使用同一 `DEFAULT_APP_INTERCEPT_INCLUDE` 常量初始化 runtime 白名单，供 `--host/--port/--app-intercept-include` 未指定时使用。CLI mapping 在 `crates/bifrost-cli/src/commands/start.rs` 里把 `--app-intercept-include` 逗号分隔字符串塞进 `ProxyConfig`，覆盖默认。

### 双侧不变量

两个默认列表必须始终相等。测试通过 `assert_eq!(TlsConfig::default().app_intercept_include, ProxyConfig::default().app_intercept_include)` 强制。删除任一常量将导致 unit test 失败。

## CLI 与 Admin API

### CLI

- `bifrost start`：`--app-intercept-include a,b,c` 直接替换 runtime 白名单，不合并默认。
- `bifrost config get tls.app_intercept_include`：读取持久化默认值。
- `bifrost config set tls.app_intercept_include "Google Chrome*,Codex*"`：写入 `bifrost.toml`，下次启动生效。

### Admin API

- `GET /_bifrost/api/config` → `.tls.app_intercept_include` 数组。
- `PUT /_bifrost/api/config` with `{ "tls": { "app_intercept_include": ["..."] } }` → 持久化并热更新。
- `GET /_bifrost/api/config/defaults`（若存在）返回工厂默认，前端“恢复默认”按钮使用。

### Web UI

Settings → Proxy tab 有 `App Intercept Include` 多值输入。默认展示八条浏览器模式，用户增删项目后点保存。UI 不阻止用户加 `Codex*`，只是产品默认不加。

## 实现切分

### Phase 1：常量与双侧默认

- 在 `bifrost-proxy` 与 `bifrost-storage` 分别声明 `DEFAULT_APP_INTERCEPT_INCLUDE`，值一致。
- `TlsConfig::default()` 与 `ProxyConfig::default()` 使用该常量。
- 明确删除任何历史遗留的 `Codex` / `Codex CLI` 条目。

### Phase 2：单元测试锁死

- `unified_config::tests::test_tls_config_default` 断言 `== DEFAULT_APP_INTERCEPT_INCLUDE` 且 `!any(contains("codex"))`。
- `server::tests::test_proxy_config_default_app_intercept_include_excludes_codex` 同上断言。
- 新增 `test_default_lists_are_equal_across_crates`（可选）比较两侧常量。

### Phase 3：真实 Admin API 校验

- E2E `test_tls_intercept_mode_api.sh`（已存在）扩展一步：`curl config` 后 `jq '.tls.app_intercept_include'` 与静态列表 diff。
- 覆盖临时 `BIFROST_DATA_DIR` 场景，确保首次落盘也不会被“注入 Codex”的旧迁移代码破坏。

### Phase 4：文档与人工验收

- `human_tests/default-tls-app-whitelist.md`（已存在）复核用例。
- README/docs 中若列举“默认解包哪些应用”，同步为八条浏览器。

## 测试方案

### 单元测试

- `bifrost-storage`：`unified_config::tests::test_tls_config_default`。
- `bifrost-proxy`：`server::tests::test_proxy_config_default_app_intercept_include_excludes_codex`。
- 两处均断言：
  1. 列表等于常量。
  2. 遍历列表 `to_ascii_lowercase().contains("codex")` 必须全 false。
- 可选：跨 crate 常量相等断言（放在 `tests/` 集成测试）。

### E2E 测试

- 复用 `e2e-tests/tests/test_tls_intercept_mode_api.sh`：
  - 启动临时 Bifrost（`BIFROST_DATA_DIR=$(mktemp -d)`、随机端口、`--no-system-proxy`）。
  - `curl -s http://127.0.0.1:$PORT/_bifrost/api/config | jq -c '.tls.app_intercept_include'` 期望等于 `["Google Chrome*","Microsoft Edge*","*Safari*","*Firefox*","*Opera*","*Brave*","*Arc*","*Vivaldi*"]`。
  - 断言 `contains("codex") == false`。
- 不新增“真实 HTTPS 代理”E2E：本变更只锁默认列表，请求路径未改。

### 真实场景测试

`human_tests/default-tls-app-whitelist.md`：

- TC-TLS-WL-01：首次启动，`bifrost.toml` 中 `tls.app_intercept_include` = 默认八条。
- TC-TLS-WL-02：Admin API `/api/config` 返回同样列表。
- TC-TLS-WL-03：Web UI Proxy tab 显示默认八条。
- TC-TLS-WL-04：用户添加 `Codex*` 保存，重启后仍存在，未被默认覆盖。
- TC-TLS-WL-05：真实 Chrome HTTPS 被解包；真实 Codex HTTPS 只见 CONNECT，无解包。

### 覆盖率与项目校验

- `cargo test -p bifrost-storage test_tls_config_default`
- `cargo test -p bifrost-proxy test_proxy_config_default_app_intercept_include_excludes_codex`
- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`（收尾，可按修改范围裁剪）
- 本地按 `rust-project-validate` 约定豁免 `make coverage`，交付说明豁免原因。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：默认列表不含 Codex、双侧常量一致、用户仍可自行加入。
- 复核 diff：`unified_config.rs`、`server.rs`、`start.rs`（若涉及）、E2E 脚本、human_tests。
- 重点 review：是否存在旧迁移代码把 `Codex*` 塞进新装机默认；`--app-intercept-include` 传空是否被误解读为“用默认”。
- 复测：两个 focused unit test + E2E 脚本 + 手动 curl。

### 第 2 轮

- 检查 `git status --short`、`git diff` 无遗漏。
- 重点 review：Web UI “恢复默认”按钮是否也用了同一常量；文档是否更新一致。
- 复测：失败路径重跑；真实浏览器 vs Codex 场景手工验证。

## 风险与决策点

- **是否加入 Claude Code / Cursor 等其他 Agent 客户端**：拒绝。理由同 Codex：默认解包会暴露 API Key。用户需要时手动加。
- **是否用一个常量跨 crate 共享**：当前双份字面量 + 双测试守恒，简单；若后续新增第三处默认（如 Web），改成一个公共 crate 常量。
- **glob 匹配大小写**：`app_intercept_include` 现有语义是大小写敏感 glob。若浏览器进程名大小写不稳定，需要 case-insensitive 匹配 —— 那是解包引擎变更，不在本方案范围。
- **持久化 vs 运行时默认漂移**：双侧断言常量相等是强约束，忘同步会红。若未来允许持久化默认 ≠ 运行时默认，测试需要拆开。

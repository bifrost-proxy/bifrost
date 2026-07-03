# 多行规则过滤器 E2E 验证方案

## 背景

Bifrost 规则语法支持 `line`...`` 多行块，用于把域名匹配与 `reqHeaders://` / `resHeaders://` / `includeFilter://` / `excludeFilter://` 等多条指令绑定成一个逻辑规则。历史上 `includeFilter` / `excludeFilter` 只有解析层的单元测试覆盖：能识别指令、能进入 resolver 内部结构，但没有真实的黑盒 E2E 证明它们能在代理链路里正确决定 "改还是不改"。

真实使用中，用户经常在同一个域名下针对不同 method + 路径子集应用不同的头修改。若 filter 语义在运行时被漏跑或误跑，用户看到的表现就是 "为什么我明明写了 exclude 内部路径，X 头还是被塞了"。为消除这个盲区，本方案给多行规则块的 filter 建立一条端到端 shell E2E：mock echo 上游 + 真实代理 + curl 客户端，从代理进出两侧同时验证。

## 用户目标验证清单

### 必须实现

- 有一条独立的 shell E2E 脚本，专门验证多行规则 `line`...`` 的 `includeFilter` / `excludeFilter` 在代理链路真实生效。
- 脚本用一个专属规则夹具，包含两个多行块：一个负责基础转发到 mock echo（保证所有请求可达上游），一个负责带 filter 的头修改。
- 断言必须同时校验请求侧和响应侧：mock echo 回显的请求头 + 代理返回的响应头。
- 覆盖 4 种命中/未命中组合：GET+/api/ 命中、GET+/api/internal/ 被 exclude、POST+/api/ 不满足 method、GET+/home 不满足路径。
- 脚本纳入 `scripts/run_all_e2e.sh` 的 `STABLE_SHELL_TESTS` 列表，默认回归入口跑。
- `e2e-tests/rules/COVERAGE.md` 记录该夹具与场景。

### 必须不破坏

- 现有 `line`...`` 解析单元测试保持通过。
- 其它 filter 相关的解析/resolver 单元测试不受影响。
- shell E2E 使用独立数据目录、独立端口，不影响并发跑的其它测试。
- 不依赖真实网络：mock echo 全本地。

### 必须真实验证

- 代理必须真实构建（脚本会 fallback 到 `cargo build --release --bin bifrost`）。
- 请求真实通过代理，不通过 loopback bypass。
- 断言层不吞异常：命中失败要打印 mock echo 回显与代理响应，便于定位。

## 产品语义

### 多行规则块与 filter 的组合语义

一个 `line`...`` 块内的指令按 "同一个虚拟规则" 处理：

- 匹配 pattern（第一行的 URL/host）决定这条虚拟规则是否命中；
- `includeFilter://` 是 AND 语义，全部满足才算命中；
- `excludeFilter://` 是 OR 语义，命中任意一条则跳过；
- `reqHeaders://` / `resHeaders://` 只在最终 gate 通过后执行。

因此本 E2E 需要验证的是：**候选 matcher 命中之后，filter 每次请求都按当前 method/path 重新判定**。这也是与 `multi-demand-resolver-cache.md` 的直接对应场景——resolver 缓存不能缓存 filter 结果。

### 规则夹具约定

夹具位置：`e2e-tests/rules/regression/line_block_filter_effect.txt`

```text
line`
http://127.0.0.1:__ECHO_HTTP_PORT__
line-block-filter.local
`

line`
line-block-filter.local
reqHeaders://X-Line-Block-Request=matched
resHeaders://X-Line-Block-Response=matched
includeFilter://m:GET
includeFilter:///api/
excludeFilter:///api/internal/
`
```

`__ECHO_HTTP_PORT__` 是占位符，脚本在运行期由 `e2e-tests/test_utils/rule_fixture.sh` 渲染成真实 mock 端口。

## 技术细节

### 脚本入口

`e2e-tests/tests/test_multiline_rule_filter_e2e.sh`（当前实现 242 行）：

1. 加载 `e2e-tests/test_utils/{assert,process,rule_fixture}.sh`。
2. 若 `target/release/bifrost` 不存在，执行 `cargo build --release --bin bifrost`。
3. 启动 mock HTTP echo (`e2e-tests/mock_servers/http_echo_server.py`)，端口写入夹具占位符。
4. 启动 Bifrost：
   - `BIFROST_DATA_DIR=.bifrost-e2e-line-block-filter-<port>-<pid>`
   - flags：`--skip-cert-check --unsafe-ssl --no-system-proxy --rules-file <rendered-fixture>`
5. 用 curl 分别发送 4 组请求（同一个 `line-block-filter.local` 域名，通过代理直连）：
   - `GET /api/echo` → 期望 `X-Line-Block-Request: matched` 出现在 echo 回显、`X-Line-Block-Response: matched` 出现在代理响应。
   - `GET /api/internal/echo` → 期望两个 header 都缺失，但请求仍到达 mock（基础转发规则生效）。
   - `POST /api/echo` → 期望两个 header 都缺失（method 不满足 include）。
   - `GET /home` → 期望两个 header 都缺失（路径不满足 include）。
6. 断言 mock echo 服务收到了全部 4 个请求。
7. 断言状态码为 2xx。
8. cleanup：kill 代理、kill mock echo、清理临时数据目录。

### mock echo 服务

`e2e-tests/mock_servers/http_echo_server.py`：

- 用 Python http.server 实现，回显请求 method、path、请求头到响应体（JSON）。
- 由 `e2e-tests/mock_servers/start_servers.sh` 统一 spawn/关闭。
- 端口从空闲池分配，写入 `RENDERED_FIXTURE`。

### 数据目录隔离与并发

- `BIFROST_DATA_DIR` 带上端口和 PID，允许并发跑不冲突。
- `--no-system-proxy` 避免 macOS 系统代理被改。
- 使用非 9900 的 admin port（脚本从空闲端口池取）。
- `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` 与 `BIFROST_DISABLE_TRAY=1` 由脚本框架统一注入。

## CLI + Web + Admin API

本方案是 E2E 补齐，没有新增或修改 CLI 子命令 / Web 路由 / Admin API。用到的入口是既有：

- `bifrost --rules-file` 直接加载规则文件。
- `bifrost --skip-cert-check --unsafe-ssl --no-system-proxy` 用于隔离化启动。
- curl 通过 `http_proxy` / `-x` 走代理发请求。

## Sync 边界

- 夹具是本地文件，不参与 rule sync。
- 独立数据目录内的规则不影响用户主 data dir。

## Phase 拆分

### Phase 1：夹具与脚本骨架

- 新增 `e2e-tests/rules/regression/line_block_filter_effect.txt`。
- 新增 `e2e-tests/tests/test_multiline_rule_filter_e2e.sh`，实现 mock echo + 代理 spawn + 4 组 curl。
- 断言 mock echo 收到请求。

### Phase 2：请求/响应双侧断言

- 断言 echo 回显请求头是否含 `X-Line-Block-Request`。
- 断言代理响应头是否含 `X-Line-Block-Response`。
- 4 组请求命中/未命中矩阵完整覆盖。

### Phase 3：接入回归入口

- `scripts/run_all_e2e.sh` `STABLE_SHELL_TESTS` 添加 `test_multiline_rule_filter_e2e.sh`（当前脚本 49 行位置）。
- `e2e-tests/rules/COVERAGE.md` 更新。

### Phase 4：稳定性与文档

- 复跑 100 次观察 flake。
- 与 `test_rule_filter_routing_diagnostics.sh` 一起构成 filter 场景的双 E2E。
- 文档：本设计文档、`e2e-tests/rules/COVERAGE.md`。

## 测试方案

### E2E 测试

主脚本：

- `e2e-tests/tests/test_multiline_rule_filter_e2e.sh`

回归时应一并跑：

- `e2e-tests/tests/test_rule_filter_routing_diagnostics.sh` — 单行规则 + filter 的 routing 诊断。

两者共同覆盖：

- 多行规则块中的 include/exclude 命中/未命中；
- 单行规则中的 include/exclude；
- 请求侧 (`reqHeaders`) 与响应侧 (`resHeaders`) 双向断言；
- resolver 候选缓存命中后 filter 仍按当前请求评估的运行时行为（对应 `multi-demand-resolver-cache.md` 中的 `test_header_filter_not_stale_across_requests`）。

### 单元测试

resolver 侧的过滤器语义已由 `crates/bifrost-core/src/rule/resolver/tests.rs` 覆盖：

- `test_include_filter_method`
- `test_include_filter_header_exists`
- `test_include_filter_client_ip`
- `test_exclude_filter_path`
- `test_exclude_filter_whistle_style_wildcard_url`
- `test_exclude_filter_whistle_style_wildcard_path_prefix`
- `test_long_exclude_filter_chain_uses_regular_prefix_matching`
- `test_combined_include_exclude_filters`
- `test_header_filter_not_stale_across_requests`

多行 `line`...`` 解析覆盖在 `crates/bifrost-core/src/rule/parser` 相关单元测试中，本方案不重复。

### 真实场景

在本机手动执行：

```bash
bash e2e-tests/tests/test_multiline_rule_filter_e2e.sh
```

预期：4 组请求全部按矩阵通过；无 leaked 进程；无临时目录残留。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核用户目标：多行规则块的 filter 在代理链路真实生效，请求/响应双侧断言。
- 复核 diff：夹具、脚本、`run_all_e2e.sh` STABLE_SHELL_TESTS、`COVERAGE.md`。
- 重点 review：
  - 夹具占位符渲染是否正确写回 mock 端口；
  - 4 组请求的期望矩阵是否覆盖了 method-only 未命中、path-only 未命中、exclude 命中、正常命中；
  - 数据目录/端口是否 PID/端口双维度隔离；
  - cleanup 是否在 assertion 失败路径也执行（`trap`）。
- 复测：`bash e2e-tests/tests/test_multiline_rule_filter_e2e.sh` + `bash scripts/run_all_e2e.sh --tag stable-shell`。

### 第 2 轮

- 基于最新 diff 复查脚本、夹具、`COVERAGE.md`。
- 重点 review：flake 风险（端口/进程孤儿）、断言失败信息可读性、超时上限、mock echo 是否会把 `X-Line-Block-Request` 大小写归一化。
- 若发现 assertion 只覆盖部分组合、mock 未回显完整请求头、代理响应未包含所有目标 header，追加第 3 轮。

## 校验要求

- 先执行新增 E2E 脚本：`bash e2e-tests/tests/test_multiline_rule_filter_e2e.sh`。
- 顺带执行：`bash e2e-tests/tests/test_rule_filter_routing_diagnostics.sh`。
- 任务结束前执行 rust-project-validate 规定的校验流程：
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  - `cargo test -p bifrost-core rule::resolver`
  - `cargo test --workspace --all-features`（如时间允许）
- 若工作区级校验存在与本次改动无关的阻塞，需要在结果里明确说明失败位置和原因。
- 本机 no-local-coverage 约定时不跑 `make coverage`。

## 文档更新要求

- 更新 `e2e-tests/rules/COVERAGE.md`，补充多行规则过滤器专项回归夹具说明（fixture 名 + scenario matrix）。
- 本次不涉及新协议、新 Hook、CLI 参数或 README 配置说明，无需更新 `README.md`。

## 风险与决策点

- **mock echo 大小写行为**：Python `http.server` 收到的 header 名是原样；断言时用 case-insensitive 匹配，避免因客户端库归一化误报。
- **端口冲突**：`__ECHO_HTTP_PORT__` 从空闲池分配，脚本失败时端口会被 OS 回收；不用担心跨脚本泄漏。
- **cargo build 时间**：首次运行会 build release，耗时较长。CI 中依赖已经预 build 的 artifact 时，脚本会跳过 build。
- **规则语法演进**：未来 `line`...`` 或 `includeFilter` 语义变化时，本 E2E 会最先失败并暴露语义漂移。这是好事，不建议弱化断言。
- **与 resolver 缓存回归的关系**：本 E2E 是黑盒验证 filter 语义 + 缓存共存正确；resolver 内部 `test_header_filter_not_stale_across_requests` 是白盒防线，两者互补。
- **flake 风险**：并发跑时若 mock echo 未完全就绪就发请求，会得到 connect refused。脚本采用 poll wait 直到 echo 端口可连接。

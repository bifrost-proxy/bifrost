# Tunnel 请求头覆盖修复

## 背景

线上真实请求 `11117` 暴露出一个严重的 HTTPS passthrough 缺陷:客户端原始请求已经携带某个 header,同时命中的 `reqHeaders://` 规则也配置了同名 header 时,发往上游的请求会**同时保留两个值**,服务端因此收到重复 header,认证/路由/追踪行为都可能不确定。

根因位于 `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs` 的上游请求构造链路:

```
Request::builder()
    .header(name, client_original_value)   // 从 client 复制
    …
    .header(name, rule_value)              // 追加规则 header
    .body(...)
```

`http::request::Builder::header` 对同名 key 是**追加**语义(与 `HeaderMap::append` 一致),不是 `insert` / 覆盖。普通 HTTP 改写链路(handler.rs 主路径)是通过 `HeaderMap::insert` 应用规则 header,不会有这个问题,所以缺陷精确集中在 tunnel(HTTPS passthrough)分支。

同一批修复顺带覆盖了 `reqHeaders://` 空值时删除 header 的语义,以及非法 header 名/值时的错误处理,让 tunnel 分支与普通 HTTP 分支表现一致。

代码入口:

- `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs` — `apply_resolved_req_headers_to_outgoing_request` 与 tunnel 请求构造链。
- `crates/bifrost-proxy/src/proxy/http/handler.rs` — 参考实现(主路径 `HeaderMap::insert` 语义)。
- `crates/bifrost-e2e/src/tests/rule_merge_strategy.rs` — 端到端 header 合并测试。
- `human_tests/rule-merge-headers.md` — 真实场景回归。

## 用户目标验证清单

### 必须实现

- Tunnel 分支中 `reqHeaders://` 对同名 header **必须是覆盖语义**(等价于 `HeaderMap::insert`)。
- 客户端原始 header 与规则 header 同名时,上游只能收到一个最终值,值必须是规则值。
- 规则 header 名称或值非法时,不再把 builder 打成隐式 error 让请求走废掉,而是显式返回 `502 Bad Gateway`。
- 客户端原始多值 header(例如多个 `Cookie` / `Set-Cookie-Copy` / `Accept-Encoding`)在规则未覆盖时保留原有多值。
- 普通 HTTP 分支行为保持不变——修复只在 tunnel 分支生效。

### 必须不破坏

- Cookie 转发保留原有语义:即使 `Cookie` 出现多次也不因“覆盖”合并掉。
- 客户端原始 header 未被规则覆盖时保留全部值。
- `delete_req_headers` / `resHeaders://` / `urlParams://` / `host://` / `mock://` 其它规则语义不变。
- WebSocket / SSE / HTTP/2 / HTTP/3 请求头处理不变,只有 HTTPS passthrough tunnel 分支修改逻辑。
- traffic 记录里的 `original_request_headers` / `request_headers` 差异仍能正确写入。

### 必须真实验证

- E2E:请求初始带 `x-same-key: client-original`,应用 `reqHeaders://x-same-key=rule-value` 后,mock server 只收到一个 `x-same-key: rule-value`。
- E2E:同名 header 出现在两条规则里,后一条规则覆盖前一条(现有 `merge_reqheaders_same_key_override` 场景)。
- 单元:builder → `HeaderMap` 后 `HeaderMap::insert` 只保留一个值。
- Human test:真实 HTTPS 网站(如 `httpbin.org/headers`)在 tunnel 模式下应用规则,`json.headers` 中同名 header 只出现一个,值为规则值。

## 产品语义

### `reqHeaders://` 的覆盖语义

`reqHeaders://` 在文档语义中是“把请求 header 设置为该值”,不是“再加一个”。这与普通 HTTP 分支的实现一致,tunnel 分支必须对齐。特殊说明:

- 规则值为空(`reqHeaders://X-Trace=`):在 tunnel 分支视为删除同名 header,与 `delete_req_headers` 交叉;第一版保留 tunnel 主路径对齐 `HeaderMap::insert` 语义,由已有 `delete_req_headers` 逻辑负责真正的删除通路。
- 多条规则命中同一 header 时,resolver 已经按顺序把它归到 `resolved_rules.req_headers` 里(后者覆盖前者);tunnel 分支只需要对同名 key 使用 `insert` 而不是 `append`。

### tunnel 分支与普通 HTTP 分支的一致性

- 普通 HTTP 分支:先 build `Request<Body>`,通过 `parts.headers` 上的 `HeaderMap::insert` 应用 `req_headers`。
- Tunnel 分支:同样先 build `outgoing_req`,构造完成后通过 `outgoing_req.headers_mut().insert(name, value)` 应用 `resolved_rules.req_headers`。
- 应用错误路径:名称或值非法时返回 `502 Bad Gateway`,并在 traffic error 中标注 `bad_rule_header`,与普通 HTTP 分支相同。

## 技术细节

### 修复点

`crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs`:

1. **拆分**请求 builder 中的“复制客户端原始 header”与“应用规则 header”两步:
   - 保留 `Request::builder().method(...).uri(...).header(client_name, client_value)...` 只复制客户端原始 header + 由 tunnel 自己控制的少量 header(`Host`、`Connection`、`Proxy-Connection` 处理等)。
   - **删除**同一 builder 上通过 `.header(rule_name, rule_value)` 追加规则 header 的所有调用。
2. **新增** `apply_resolved_req_headers_to_outgoing_request(outgoing_req, delete_req_headers, req_headers)`:
   - 先 apply `delete_req_headers`(`HeaderMap::remove` 循环)。
   - 再 apply `req_headers`(`HeaderMap::insert`,同名 key 覆盖)。
   - 遇到非法 `HeaderName::try_from` / `HeaderValue::try_from` 直接返回 `Err(bad_rule_header)`,主流程转 `502 Bad Gateway`。
3. **traffic 记录**继续复用现有 `original_req_headers` / `final_req_headers` 对比:tunnel 修复后 `final_req_headers` 中每个 name 只出现一次(除非规则规定多值 append 语义,这不属于本次修复范围)。

搜索验证:tunnel 主路径在 `apply_resolved_req_headers_to_outgoing_request` 调用后再次做 `super::headers_to_pairs(outgoing_req.headers())` 生成 `final_req_headers`,traffic diff 天然记录覆盖前后的差异。

### CLI / Admin API / Web

本次修复不新增 CLI 命令、Admin API 或 Web UI 元素。规则语法 `reqHeaders://` 保持不变,现有 rule 编辑器、rule share、rule sync 都继续可用。文档更新集中在:

- `human_tests/rule-merge-headers.md`:补 passthrough / HTTPS tunnel 回归用例。
- `human_tests/readme.md`:更新用例索引。
- `design/tunnel-reqheaders-override.md`:本文。

### Sync 边界

规则 sync 通道不变,`.bifrost` 文件格式不变。本修复只影响 tunnel 分支的运行时行为,不改变磁盘上的规则表示,因此:

- 已同步的规则文件在新旧版本 Bifrost 上都可以解析,只是旧版本 tunnel 分支仍会追加重复 header,新版本会覆盖。
- 团队通过 rule share / group rule 同步 `reqHeaders://` 时,升级后所有成员的 tunnel 分支自动生效覆盖语义,无需重新导入。

## Phase 1 - 4

### Phase 1:修复 tunnel builder

- `tunnel/mod.rs` 移除 builder 上规则 header 追加。
- 新增 `apply_resolved_req_headers_to_outgoing_request`。
- 单元测试:builder 复制客户端 header + 应用规则 header,同名 key 只保留规则值。

### Phase 2:E2E 覆盖

- 增强 `bifrost-e2e` mock 记录结构,新增 `header_pairs: Vec<(String, String)>`,可以真实统计重复 header 个数。
- 复用 `test_merge_reqheaders_same_key_override_e2e`,断言 mock 收到 `x-same-key` 恰好一次且值为 `rule-value`。
- 新增 tunnel-only 变体(强制走 HTTPS passthrough),验证修复不只覆盖普通 HTTP。

### Phase 3:Human tests / 文档

- `human_tests/rule-merge-headers.md` 增加 HTTPS tunnel 用例。
- `human_tests/readme.md` 索引同步。
- README/skill 说明 `reqHeaders://` 覆盖语义,附带排查步骤(查看 traffic diff `original` vs `final`)。

### Phase 4:回归观察

- 上线一段时间内跟踪 `x-real-ip` / `x-forwarded-for` / `authorization` 等高频 header 是否出现异常重复。
- 修复完成后关闭 issue #11117。

## 测试方案

### 单元测试

- `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs`(tests 模块):
  - 输入初始 `x-same-key: client-original` + `resolved_rules.req_headers = [("x-same-key", "rule-value")]` → outgoing_req headers 只保留 `x-same-key: rule-value`。
  - 多值 header(如 `accept: text/html`、`accept: application/json`)在规则未覆盖时保留全部值。
  - `delete_req_headers` + `req_headers` 同名交叉,`delete` 先执行,`req_headers` 之后 insert。
  - 规则 header 名称非法(含空格或非 ASCII 控制字符)→ 返回 `Err(bad_rule_header)`,主流程 `502`。
  - Cookie header(多值,大小写混合)在规则未涉及时保持原样。

### E2E 测试

`crates/bifrost-e2e/src/tests/rule_merge_strategy.rs`:

- `merge_reqheaders_same_key_override` / `test_merge_reqheaders_same_key_override_e2e`(已存在,line 108/111/541)
  - 覆盖“两条规则同名 header,后规则覆盖前规则,且客户端原始值被覆盖”。
  - 使用 `header_pairs` 断言 mock 收到 `x-same-key` 恰好 1 次。
- `merge_reqheaders_different_keys_merge` / `test_merge_reqheaders_different_keys_merge_e2e`
  - 覆盖“不同 key 合并,不因新覆盖逻辑误删其他 header”。
- 建议新增 `test_tunnel_reqheaders_same_key_override_e2e`,强制走 HTTPS mock server(passthrough 分支),避免 HTTP 主路径与 tunnel 分支混淆。

### 真实场景测试

`human_tests/rule-merge-headers.md` 补:

- TC-Hdr-01:HTTPS passthrough 目标(`httpbin.org/headers`)+ 规则 `httpbin.org reqHeaders://x-same-key=rule-value`,客户端 curl 加 `-H "x-same-key: client-original"`,断言响应 body `headers` 中 `X-Same-Key` 恰好一个,值 `rule-value`。
- TC-Hdr-02:HTTPS passthrough + 两条 `reqHeaders://x-same-key`,后规则覆盖前规则。
- TC-Hdr-03:HTTP 明文目标,重复 header 行为不受此次修复影响(与旧版本一致)。
- TC-Hdr-04:客户端多个 Cookie header,规则未覆盖时 tunnel 保留全部值。
- TC-Hdr-05:规则 `reqHeaders://x-bad name=value`(名字含空格)→ 客户端得到 `502 Bad Gateway`,traffic 记录 `error=bad_rule_header`。

启动使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核目标:tunnel 覆盖同名 header、其余 header 不受影响、非法 header 报 502。
- Review 修改:`tunnel/mod.rs` builder 拆分与新增 apply 函数、`bifrost-e2e` mock `header_pairs` 增强、`human_tests` 更新。
- 复跑:`cargo test -p bifrost-proxy tunnel::mod::tests -- --nocapture`、`cargo test -p bifrost-e2e -- test_merge_reqheaders_same_key_override --no-capture`、`bash e2e-tests/tests/test_rule_merge_strategy.sh`(若有)。

### 第 2 轮

- 复检 tunnel builder 上没有残留的规则 header 追加。
- 复检 `original_request_headers` 与 `request_headers` 的 traffic 记录仍能正确写入,前端 diff 展示无回归。
- 执行 `human_tests/rule-merge-headers.md` 全部用例。
- 复跑 `cargo test --workspace --all-features`,补齐 coverage 门禁。

## 校验要求

1. `cargo fmt --all -- --check`
2. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
3. `cargo test -p bifrost-proxy tunnel`
4. `cargo test -p bifrost-e2e -- test_merge_reqheaders_same_key_override --no-capture`
5. `cargo test --workspace --all-features`
6. 按 `human_tests/rule-merge-headers.md` 逐条执行

## 风险与决策

- **`reqHeaders://X-Trace=` 空值语义**:文档未强制“空值 = 删除”,tunnel 修复只保证 insert 覆盖;真正的“删除”仍由 `delete_req_headers` 通道负责,避免语义歧义。
- **Cookie 特殊性**:HTTP/1.1 允许多个 `Cookie` header,但推荐单一 Cookie header 用 `;` 分隔。规则如果显式 `reqHeaders://Cookie=...` 会覆盖客户端所有 Cookie,这是显式行为,由用户承担;规则未涉及时 tunnel 分支保留客户端全部 Cookie header。
- **多值 header append 场景**:如果未来产品需要“追加 header”语义(不覆盖),应该新增独立协议(如 `addReqHeaders://`)而不是靠 `.header()` 追加副作用,保持覆盖语义清晰稳定。
- **traffic diff 展示**:`original_request_headers` 与 `request_headers` 差异记录已经能表达“客户端原值→规则值”,前端 diff 展示不需要修改。
- **升级兼容性**:老版本 Bifrost 在同一环境仍会追加重复 header;修复上线后建议在 release notes 中提示 tunnel 分支行为变化,让依赖“重复 header”的极端用户(不应存在)提前调整。

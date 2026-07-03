# PAC Proxy Auto-Config 真实场景测试

## 功能模块说明

验证 `pac://` 规则已经从占位协议变成真实 PAC Proxy Auto-Config 路由能力。覆盖 PAC 脚本来源、`FindProxyForURL` 执行、远程 PAC 拉取、PAC 返回上游代理后的真实代理链路、PAC 与已有 `host`/`proxy` 转发规则组合，以及系统代理不变量。

## 前置条件

- 在仓库根目录执行。
- 当前分支包含 PAC runtime 实现、E2E 脚本和文档更新。
- 执行真实 Bifrost 服务用例时必须使用临时数据目录、`--no-system-proxy`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` 和 `BIFROST_DISABLE_TRAY=1`。

## 测试用例列表

### TC-PAC-01：PAC engine 与规则解析单元回归

操作步骤：

1. 执行：
   ```bash
   cargo test -p bifrost-script pac -- --nocapture
   cargo test -p bifrost-core parse_bare_domain -- --nocapture
   cargo test -p bifrost-cli pac_ -- --nocapture
   ```

预期结果：

- PAC engine 能执行 `FindProxyForURL` 并解析 `DIRECT` / `PROXY` / 多候选返回值。
- 规则解析支持 rewrite 场景中的裸 host 目标。
- CLI resolver 能把 Values、本地文件、远程 PAC 映射到上游代理，`DIRECT` 能清除已有代理，Final URL 单测通过。

### TC-PAC-02：真实 E2E - 远程 PAC 与双 Bifrost 上游代理链路

操作步骤：

1. 执行：
   ```bash
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 bash e2e-tests/tests/test_pac_proxy_auto_config.sh
   ```

预期结果：

- E2E 构建当前 `target/release/bifrost`。
- 脚本启动 Mock HTTP Server、上游 Bifrost、入口 Bifrost。
- Values PAC 返回 `PROXY 127.0.0.1:<upstream_port>` 后，请求经入口 Bifrost -> 上游 Bifrost -> Mock Server，断言 200、路径和 query 保留。
- 远程 PAC 通过 Mock Server 的 `.pac` 响应拉取脚本，返回上游 Bifrost 代理后同样成功转发。
- 专项脚本最终 0 退出，`e2e-tests/rules/forwarding/pac.txt` 仅作为 parallel-fixtures 的 PAC 语法 fixture。

### TC-PAC-03：真实 E2E - PAC 与既有转发/代理规则组合

操作步骤：

1. 执行 TC-PAC-02 的同一条 E2E 命令。
2. 检查输出中的 `PAC with forwarding rules clears existing proxy` 场景。

预期结果：

- 入口 Bifrost 的规则同时包含 `proxy://127.0.0.1:1`、`host://127.0.0.1:<echo_port>` 和 `pac://{...}`。
- PAC 对 `pac-forward.local/forward` 返回 `DIRECT` 后清除已有不可达 proxy，最终通过 host 转发到 Mock Server。
- 断言 200、`parsed_path` 为 `/forward`、query 保留。

### TC-PAC-04：系统代理不变量与文档同步

操作步骤：

1. 执行：
   ```bash
   rg -n 'no_proxy\(\)|outbound_blocking_reqwest_client_builder|HTTP_PROXY|HTTPS_PROXY|--no-system-proxy|BIFROST_DISABLE_TRAY|BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT' crates/bifrost-core/src/http_client.rs crates/bifrost-core/src/rule/value_source.rs e2e-tests/tests/test_pac_proxy_auto_config.sh design/pac-proxy-auto-config.md
   rg -n 'not yet implemented|尚未实现|未实现|不会拉取远程 `\.pac`|不会执行任何 JavaScript PAC' docs docs-en site/src/content/docs crates/bifrost-core/src/syntax.rs design/pac-proxy-auto-config.md
   ```

预期结果：

- 第一条命令 0 退出，证明远程 PAC 加载路径和真实 E2E 均显式保留“不吃系统代理 / 不改系统代理 / 不拉托盘登录弹窗”的约束。
- 第二条命令非 0 退出，证明 PAC 文档和语法提示不再残留“未实现”描述。

## 清理步骤

- E2E 脚本退出时会停止入口 Bifrost、上游 Bifrost、Mock Server，并删除 `.bifrost-e2e-pac.*` 临时目录。
- 若测试中断，执行：
  ```bash
  pkill -f 'e2e-tests/tests/test_pac_proxy_auto_config.sh' || true
  pkill -f 'target/release/bifrost.*pac' || true
  ```

## 执行记录

| 日期 | 用例 | 结果 | 证据 |
| --- | --- | --- | --- |
| 2026-07-02 | TC-PAC-01 | 通过 | 执行 `cargo test -p bifrost-script pac -- --nocapture`，5 passed；执行 `cargo test -p bifrost-core parse_bare_domain -- --nocapture`，3 passed；执行 `cargo test -p bifrost-cli pac_ -- --nocapture`，lib 8 passed、main 8 passed，包含 PAC 执行失败和不支持代理 scheme 的 fail-closed 回归。 |
| 2026-07-02 | TC-PAC-02..03 | 通过 | 执行 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 bash e2e-tests/tests/test_pac_proxy_auto_config.sh`。脚本构建当前 release binary，启动 Mock HTTP Server、上游 Bifrost 和入口 Bifrost；Values PAC 与远程 PAC 均经入口 Bifrost -> 上游 Bifrost -> Mock Server 返回 200；PAC + `proxy://127.0.0.1:1` + `host://127.0.0.1:<echo_port>` 场景返回 200 并确认 `/forward` 与 query 保留。专项脚本 13/13 通过；`python3 check_rules.py rules/forwarding/pac.txt` 语法 fixture 1/1 通过。 |
| 2026-07-02 | TC-PAC-04 | 通过 | 执行系统代理不变量扫描命中 `outbound_blocking_reqwest_client_builder`、`.no_proxy()`、E2E 的 `--no-system-proxy`、`BIFROST_DISABLE_TRAY` 与 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT`；执行旧提示残留扫描，确认 `not yet implemented`、`尚未实现`、`未实现`、远程 PAC 不拉取和不执行 PAC JS 等旧描述均无残留。 |
| 2026-07-03 | TC-PAC-01..03 | 通过 | rebase 到 `origin/main` 后补充旧 shorthand `pac://(PROXY 127.0.0.1:3000)` 兼容，并收窄预解析避免误判 PAC JS 源码。执行 `cargo test -p bifrost-script pac::tests::parses_proxy_result --lib`、`cargo test -p bifrost-cli parsing::rules::tests::test_pac_ --lib` 均通过；执行 `SKIP_BUILD=true BIFROST_BIN=target/release/bifrost BIFROST_DISABLE_TRAY=1 BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 bash e2e-tests/tests/test_pac_proxy_auto_config.sh`，13/13 通过；执行 `BIFROST_BIN=target/debug/bifrost BIFROST_E2E_PROXY_READY_TIMEOUT=60 BIFROST_DISABLE_TRAY=1 BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 bash e2e-tests/test_rules.sh --no-build --use-binary e2e-tests/rules/forwarding/pac.txt`，parallel PAC 语法 fixture 1/1 通过。 |

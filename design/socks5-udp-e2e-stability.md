# SOCKS5 UDP E2E 稳定性

> 实现状态：已发布 (implemented, refreshed against code as of 2026-07-03)。
> 核心 UDP relay fallback 逻辑位于 `crates/bifrost-proxy/src/server.rs`（`is_udp_relay_fallback_bind_error`
> line 82；startup fallback line 812–835）。E2E 脚本 `e2e-tests/tests/test_socks5_udp.sh` 与
> `test_socks5_udp_rules.sh` 已接入双 readiness gate 和临时数据目录。真实场景在
> `human_tests/proxy-socks5.md`。

## 背景

`e2e-tests/tests/test_socks5_udp.sh` 与 `test_socks5_udp_rules.sh` 用来验证 SOCKS5 UDP ASSOCIATE
的行为和 UDP 规则路由。CI 会以高并发运行这批 shell E2E，历史上遇到两类不稳定问题：

- **Readiness race**：脚本原本 `sleep 5` 后只检查 Bifrost 进程仍在，就开始发起 UDP ASSOCIATE。
  慢机上进程活但 admin API / SOCKS5 listener 尚未 bind，第一个 UDP ASSOCIATE 直接
  `Connection refused`。
- **共用固定数据目录**：脚本用仓库根固定路径作为 `BIFROST_DATA_DIR`。并行 / 重试运行时残留
  runtime 文件容易互相污染，失败信息难定位。
- **Windows ARM 高并发 UDP bind 失败**：Windows ARM 自定义 runner 在并发高时会以 `os error 10013`
  拒绝 UDP bind，即使 TCP 同端口可用。修复前 unified proxy 把 UDP bind 失败视为致命错误，
  代理进程退出，牵连不相关的 admin API 测试失败。
- **Frames SSE harness 硬失败伪装成功**：同批 CI artifact 里 `test_frames_admin_api.sh` 曾在 SSE
  流量生成失败后仍报 pass，是概率性 pass 风险。

本设计覆盖以上四点的稳定化措施。

## 用户目标验证清单

### 必须实现

- SOCKS5 UDP E2E 脚本使用可注入的 `BIFROST_DATA_DIR`（runner 提供）而非仓库根固定路径；
  直接手工执行时兜底旧路径。
- 脚本启动 Bifrost 后 poll 两条 readiness surface：admin API `/_bifrost/api/system` 与
  SOCKS5 TCP listener `PROXY_HOST:SOCKS5_PORT`，达到 ready 才继续。
- Readiness 超时时打印代理日志再退出。
- 脚本 teardown 清理 HTTP proxy 与 SOCKS5 listener 两个端口。
- Rules 变体在 initial start 与 restart 后都应用相同 readiness gate。
- Unified proxy 中，SOCKS5 UDP relay 无法 bind TCP listener 同端口时（例如 UDP 端口被占用或
  Windows `os error 10013`），自动 fallback 到 ephemeral port，并把真实 relay 地址发布给 SOCKS5
  UDP ASSOCIATE 客户端。其他 UDP relay 启动错误保持致命。
- Windows x86 E2E runner 并发保留 8；Windows ARM 自定义 runner 并发降到 2；`runner_jobs` 矩阵值
  必须挂在 `e2e-windows-runner` job（该 job 是唯一 export `BIFROST_E2E_RUNNER_JOBS` 的入口）。
- Frames harness `test_frames_admin_api.sh`：SSE 生成失败必须硬失败退出；只在本地 SSE fixture
  自身启动失败时跳过依赖 SSE 的断言。

### 必须不破坏

- HTTP/HTTPS/SOCKS5 TCP 代理服务的 bind 行为不变。
- 非 Windows 上 UDP bind 失败仍然按“非 fallback error”处理（保持原致命语义），避免掩盖其它
  bind 类问题（例如权限不足、端口配置错误）。
- SOCKS5 UDP ASSOCIATE 客户端收到的 relay 地址仍是能实际接收数据的地址，无静默丢包。
- E2E 测试脚本仍能在本机手工执行（无 runner 提供 `BIFROST_DATA_DIR` 时兜底本地路径）。
- Frames harness 只在 setup 失败时跳过 SSE 断言，不改变原本的功能覆盖面。

### 必须真实验证

- E2E 脚本本机执行（`BIFROST_DATA_DIR=<tmp> PROXY_PORT=<free> SOCKS5_PORT=<free> bash ...`）能稳定
  通过。
- Windows ARM runner 上 UDP bind 冲突时能 fallback 到 ephemeral port，SOCKS5 UDP ASSOCIATE
  能拿到真实 relay 端口。
- `test_frames_admin_api.sh` 在 SSE fixture 可用 / 不可用两种场景下行为符合预期。

## 产品语义

### Readiness gate = 双端口 + 日志

- 只判断 admin API 或只判断 SOCKS5 listener 都会漏 case（前者忽略 SOCKS5 bind 滞后，后者忽略
  admin 未启的场景）。
- Poll 间隔与总超时以“慢机能过、快机不慢”为原则：默认 100ms 一次，总超时 30s。
- 超时后打印 proxy 日志：包含 stdout + stderr，便于 CI 直接看根因，而不是 Python 侧只看到
  `Connection refused`。

### UDP relay fallback

- 只对可识别的 fallback 类错误触发：
  - 端口被占用 / bind conflict；
  - Windows CI 特有 `os error 10013`（`is_udp_relay_fallback_bind_error` 见 `server.rs:82`）。
- fallback 后 relay 起在 ephemeral port；relay 实际地址通过 SOCKS5 UDP ASSOCIATE reply BND 字段
  发布给客户端。
- 其他 UDP relay 启动错误（例如权限不足、绑定接口错误）保持致命，不 fallback。
- fallback 必须记录 warn 日志，包含原地址、错误、fallback 地址三项。

### CI 并发策略

- Windows x86：保留 `runner_jobs=8`。
- Windows ARM custom runner：`runner_jobs=2`。
- 矩阵值只挂在 `e2e-windows-runner` job（唯一 export `BIFROST_E2E_RUNNER_JOBS` 的入口），
  避免把并发限制误应用到其它 job。

## 技术细节

### 关键源码位置

- `crates/bifrost-proxy/src/server.rs`
  - `is_udp_relay_fallback_bind_error()` (line 82)：识别 fallback-eligible 错误。
    包含关键字判断：`os error 10013`、`address already in use`（对 Linux/macOS）。
  - `unified_start()` (line 812–835)：
    ```
    let udp_addr = SocketAddr::new(addr.ip(), addr.port());
    let mut udp_relay = UdpRelay::new(udp_addr)...;
    let udp_relay_started_addr = match udp_relay.start().await {
        Ok(a) => a,
        Err(error) if is_udp_relay_fallback_bind_error(&error) => {
            warn!("UDP relay bind failed on {udp_addr}: {error}. Falling back to ephemeral port.");
            udp_relay = UdpRelay::new(fallback_addr)...;
            udp_relay.start().await?
        }
        Err(other) => return Err(other),
    };
    ```
  - `udp_relay_addr` / `udp_relay` 保存实际启动地址（line 649–651、677–678、832–835），
    供 SOCKS5 UDP ASSOCIATE reply BND 字段读取。

### E2E 脚本改造

`e2e-tests/tests/test_socks5_udp.sh` / `test_socks5_udp_rules.sh`：

```bash
DATA_DIR="${BIFROST_DATA_DIR:-<repo>/e2e-tests/.data/socks5_udp}"
PROXY_PORT="${PROXY_PORT:-<default>}"
SOCKS5_PORT="${SOCKS5_PORT:-<default>}"

# start bifrost
BIFROST_DATA_DIR="$DATA_DIR" \
  bifrost start -p "$PROXY_PORT" --socks5 "$SOCKS5_PORT" ... >proxy.log 2>&1 &
PID=$!

# readiness: admin + SOCKS5 TCP
deadline=$((SECONDS + 30))
while (( SECONDS < deadline )); do
  if curl -s "http://127.0.0.1:$PROXY_PORT/_bifrost/api/system" >/dev/null \
     && nc -z "$PROXY_HOST" "$SOCKS5_PORT"; then
    ready=1; break
  fi
  sleep 0.1
done

if [[ -z "$ready" ]]; then
  echo "readiness timeout; proxy log:" >&2
  cat proxy.log >&2
  kill $PID; wait $PID 2>/dev/null
  exit 1
fi

# run tests …

# teardown: cleanup both HTTP proxy + SOCKS5 listener ports
```

Rules 变体在 initial start 与 restart（`bifrost port reload` / 主进程重启）后都必须复用同一
readiness gate 函数。

### CI 并发矩阵

`.github/workflows/e2e.yml` 中 `e2e-windows-runner` job 的 matrix 定义示例：

```yaml
strategy:
  matrix:
    runner_jobs:
      - { os: windows-latest,       jobs: 8 }
      - { os: windows-11-arm,       jobs: 2 }
env:
  BIFROST_E2E_RUNNER_JOBS: ${{ matrix.runner_jobs.jobs }}
```

其他 job 不 export `BIFROST_E2E_RUNNER_JOBS`。

### Frames harness

`e2e-tests/tests/test_frames_admin_api.sh`：

- 代理 SSE 生成最多重试 10 次，stream timeout 拉长。
- 缺失 required WebSocket 或 available-SSE 的 setup traffic 时**退出**，不再报 pass。
- 只在本地 SSE fixture 自身启动失败时跳过 SSE 相关断言。

## CLI + Web + Admin API

本设计属于测试与运行时稳定性范畴，**不新增** CLI 参数、Web 页面或 Admin API：

- `bifrost start` 已支持 `-p <port>` / `--socks5 <port>` / `--no-system-proxy`；
  E2E 脚本使用现有参数。
- Admin API `GET /_bifrost/api/system` 已存在，作为 readiness surface；不改动响应结构。
- UDP relay fallback 是内部行为，无用户可见 CLI 配置项（未来若需要 override，可考虑
  `--socks5-udp-strict`，本期不实现）。

## Sync 边界

- CI 并发矩阵与 runner 配置仅影响 GitHub Actions 侧；不与 Bifrost Sync 交互。
- UDP relay fallback 是本机运行时行为，不进入任何 Sync / share 通道。
- E2E 脚本使用的临时数据目录只在测试进程生命周期内存在，不上传远端。

## 实现切分

### Phase 1：E2E readiness gate

- `test_socks5_udp.sh` / `test_socks5_udp_rules.sh` 引入双 readiness gate + proxy log 输出。
- Teardown 清理两个端口。
- Rules 变体的 initial + restart 都跑同一 readiness 函数。

### Phase 2：临时数据目录

- 所有 SOCKS5 UDP E2E 脚本改用 `BIFROST_DATA_DIR` 环境变量，兜底本地路径。
- 补充 `scripts/run_all_e2e.sh` 中默认注入 `BIFROST_DATA_DIR`（若未定义）。

### Phase 3：UDP relay fallback

- 新增 `is_udp_relay_fallback_bind_error()`。
- `unified_start()` 中匹配 fallback 分支，起 ephemeral port，publish 真实地址。
- warn 日志包含原地址 / 错误 / fallback 地址。

### Phase 4：CI 并发 + Frames harness

- `runner_jobs` 矩阵值挂到 `e2e-windows-runner`。
- `test_frames_admin_api.sh` 引入 SSE 重试 + 硬失败 + 精确 skip 语义。

## 测试方案

### 单元测试

- **不适用**：本设计主要是 shell harness + 运行时 bind fallback。
- 可选：为 `is_udp_relay_fallback_bind_error()` 补一个 Rust 单元测试
  `udp_relay_fallback_error_recognized_by_message` 验证 `os error 10013` 与
  `address already in use` 会返回 true，其它 IO error 返回 false。

### E2E 测试

- `BIFROST_DATA_DIR=<tmp> PROXY_PORT=<free> SOCKS5_PORT=<free> bash e2e-tests/tests/test_socks5_udp.sh`
- `BIFROST_DATA_DIR=<tmp> PROXY_PORT=<free> SOCKS5_PORT=<free> bash e2e-tests/tests/test_socks5_udp_rules.sh`
- `cargo run -p bifrost-e2e -- --category group_rules --jobs 2 --test-timeout 120 --port <non-9900>`
- `bash e2e-tests/tests/test_frames_admin_api.sh`
- 相关脚本与固件：
  - `e2e-tests/tests/test_socks5_udp.sh`
  - `e2e-tests/tests/test_socks5_udp_rules.sh`
  - `e2e-tests/tests/test_socks5_tls_rules.sh`
  - `e2e-tests/rules/socks5_udp/dns_redirect_domain.txt`
  - `e2e-tests/rules/socks5_udp/dns_redirect_ip.txt`
  - `e2e-tests/tests/quic_socks5_client/main.go`
  - `e2e-tests/tests/quic_socks5_test.py`

### 真实场景测试

`human_tests/proxy-socks5.md`（已存在）需要包含：

- SOCKS5 UDP readiness 回归用例（模拟慢机 / 高并发）。
- Windows ARM runner bind fallback 回归用例（可在 Windows ARM 真机上跑，或通过 mock UDP
  端口占用触发）。
- Frames harness `TC-PWS-08`（在 `human_tests/proxy-websocket-sse.md`）验证 SSE fixture 失败时
  的显式 skip 行为。

### 校验命令

- `bash e2e-tests/tests/test_socks5_udp.sh`
- `bash e2e-tests/tests/test_socks5_udp_rules.sh`
- `bash e2e-tests/tests/test_frames_admin_api.sh`
- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-proxy udp_relay_fallback -- --nocapture`（若补充上述单测）

## Review / Fix / Test 闭环

- 第 1 轮：本机手工跑 SOCKS5 UDP + rules 变体三次，确认 readiness gate 无 flaky；`grep` proxy log
  输出格式在超时时可读。
- 第 2 轮：在 Windows ARM 真机或高并发 mock 环境下触发 `os error 10013`，确认 fallback 到
  ephemeral port，SOCKS5 UDP ASSOCIATE reply BND 与真实 relay 一致。
- 第 3 轮：跑 Frames harness 两次（正常 + SSE fixture 故意坏），确认硬失败 vs 精确 skip 行为。
- 每轮结束都复跑 `cargo fmt`、`cargo clippy`、必要的 `cargo test`。

## 风险与决策

- **风险：readiness gate poll 过密造成 CPU 抖动**。缓解：默认 100ms 间隔 + 30s 总超时，量级可控；
  高负载 runner 上可通过环境变量调 poll 间隔（未来扩展）。
- **风险：UDP fallback 掩盖真实端口配置错误**。缓解：`is_udp_relay_fallback_bind_error()` 只匹配
  明确关键字（`os error 10013` + `address already in use`），其他错误保持致命；warn 日志清楚
  记录 fallback。
- **风险：CI 并发降低影响吞吐**。缓解：只对 Windows ARM custom runner 降到 2，其它平台保持
  原并发；短期损失换稳定性。
- **决策：不引入 CLI `--socks5-udp-strict` 开关**。理由：fallback 是安全默认，用户很少需要禁用；
  一旦有真实需求再单独设计。
- **决策：不为 SOCKS5 UDP client 增加“端口重选”协议**。理由：SOCKS5 协议本身通过 UDP ASSOCIATE
  的 reply BND 字段告知客户端 relay 地址，实现只需 publish 真实地址即可。
- **决策：临时数据目录默认由 runner 注入**。理由：CI 侧统一路径管理更容易做隔离与清理；本机
  开发者仍能手工执行。

## 文档更新要求

- 更新 `human_tests/proxy-socks5.md` 加入 SOCKS5 UDP readiness 与 Windows ARM UDP bind fallback
  两条回归用例。
- 更新 `human_tests/proxy-websocket-sse.md` 中 `TC-PWS-08` 说明 SSE fixture 失败时的行为。
- CI workflow 描述文档（若有）需注明 Windows ARM runner_jobs=2 的原因。
- 本次不改用户可见文档（README / docs / site）。

# CLI status / start：runtime marker 缺失时发现存活实例

## 背景

`bifrost status` 目前只读取当前 `BIFROST_DATA_DIR` 下的 `runtime.json`，再判断其中 PID
是否存活。如果代理仍在监听、Admin 页面可以访问，但 marker 被删除、写入中断、来自旧
版本或调用进程使用了不同的数据目录，CLI 会错误输出 `Status: Stopped`。

这个误判会放大为有破坏性的启动链路：Agent/skill 按 `status=false` 执行
`bifrost start --yes`，start 看不到 marker 后只把监听端口视作普通进程占用，并可能
终止已经运行的 Bifrost。

## 用户目标验证清单

### 必须实现

- marker 缺失或陈旧，但目标端口的 Bifrost Admin API 存活时，`bifrost status`
  输出 running，并在 JSON 中标记 `runtime_source=admin_api`。
- `bifrost -p <port> status` 使用显式端口作为 fallback 探测目标；默认仍是 9900。
- 同一场景执行 `bifrost start --yes -p <port>` 时复用现有服务，不终止、不重启、
  不生成第二实例。
- 探测必须校验 Admin overview 的端口、版本和非零 PID；平台能解析 listener owner
  时，overview PID 必须与监听 PID 一致。若调用者因权限或 PID namespace 无法检查
  服务进程，成功的 loopback Admin 响应本身作为存活证据。

### 必须不破坏

- 有效 `runtime.json` 仍是首选来源；Admin API 部分故障时继续显示 marker 所代表的
  运行进程及字段级错误。
- 普通进程占用端口时不得误报为 Bifrost，仍走既有端口冲突保护。
- fallback 只读，不把发现的外部实例写回当前数据目录，避免错误取得 stop/restart
  ownership。
- 不启停用户当前 9900 实例，不修改系统代理。
- `status --format json` schema v1 只增加字段，不删除或改变既有字段类型。

### 必须真实验证

- 隔离数据目录与动态端口启动真实 daemon，确认 Admin API ready。
- 删除隔离目录的 `runtime.json` 与 `bifrost.pid`，保留进程和端口存活。
- `status --format json` 断言 `running=true`、PID/port 正确、
  `runtime_source=admin_api`。
- `start --yes` 断言输出复用提示、原 PID 仍存活、端口 owner 未变化。
- 普通 HTTP listener 返回非 Bifrost 内容时，status 仍为 stopped。

## 设计

共享入口 `process::discover_bifrost_runtime(port)` 以 2 秒本地超时读取：

`GET http://127.0.0.1:<port>/_bifrost/api/system/overview`

只有以下条件同时成立才返回一个只读的 `RuntimeInfo` 快照：

1. `server.port` 与请求端口相同；
2. `system.version` 非空；
3. `system.pid` 非零；
4. 如果能解析监听进程 PID，它与 `system.pid` 相同。

不能把 `kill(pid, 0)` / `OpenProcess` 作为 fallback 的硬门禁：不同权限、容器或 PID
namespace 下，CLI 可能不能观察服务 PID，但仍能正常访问 loopback Admin API。这正是
“页面可访问而 CLI 报 stopped”的一类真实终端条件。

快照使用 `RuntimeStartMode::Unknown`、`restartable_runtime=false`，不会写入
`runtime.json`，因此只能用于“状态展示”和“避免误杀”；不能凭 fallback 获得生命周期
控制权。

`status` 的选择顺序：

1. 有效 marker；
2. marker 记录端口上的 Admin API；
3. CLI 全局 `-p/--port`（默认 9900）上的 Admin API；
4. 保留陈旧 marker 供原有 stale 输出使用，或输出 stopped。

`start` 在 marker 分支之后、证书和端口冲突动作之前直接探测目标端口；识别到 Bifrost
后直接复用并成功返回。这里不能先依赖 bind-based `is_port_in_use`：Bifrost listener
在部分平台启用 socket reuse，第二次 bind 可能成功并造成“端口空闲”的假阴性。非
Bifrost listener 仍继续执行既有冲突处理。

## 终端用户触发条件

按当前代码，以下任一条件都会让旧版 CLI 报 stopped，即使服务仍可访问：

1. 启动进程与终端 CLI 的数据目录不同：`BIFROST_DATA_DIR`、`HOME`、Desktop 历史
   AppSupport 默认目录、sudo/普通用户环境或多安装副本不一致。
2. `runtime.json` 缺失、权限不可读、JSON 损坏或处于非原子覆盖的短暂窗口；
   `read_runtime_info()` 对所有文件/解析错误直接返回 `None`，不区分原因。
3. marker 仍在但 PID 已陈旧；服务由 watchdog、升级 helper 或外部 launcher 拉起了新
   PID，而 marker 尚未恢复。
4. CLI 无法查询 PID：Unix `kill(pid, 0)` 的任意错误都被当成 stopped，Windows
   `OpenProcess` 失败也被当成 stopped；权限边界或 PID namespace 可触发。
5. 启动完成但 marker 尚未落盘的窄竞态窗口，或 marker 被升级/清理流程移除而 listener
   继续存活。仓库现有升级设计也明确包含“runtime marker 缺失但 Admin PID/port 仍活跃”
   的恢复场景。

## 测试计划

- 单元测试：overview 的正确端口/PID/version；listener PID 不匹配；错误端口与空版本。
- CLI 集成测试：真实执行二进制的 JSON status、text status 与 `start --yes`，覆盖
  `main -> run_status -> gather_status`、fallback 文本渲染和安全复用调用链。
- E2E：新增 `e2e-tests/tests/test_status_runtime_discovery_e2e.sh`，覆盖 marker 删除后
  status fallback、start 复用、非 Bifrost listener 三条路径。
- human_tests：在 `human_tests/cli-start-stop-status.md` 增加回归用例，并立即按文档执行。
- 收尾：先执行 e2e-test，再执行 rust-project-validate；远端 CI 执行
  `bash scripts/ci/coverage-all.sh --json --gate`。

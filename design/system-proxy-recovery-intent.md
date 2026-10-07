# 系统代理意图与故障恢复

## 目标和边界

- 用户开启/关闭意图是持久配置，临时过载、OS 写入失败和进程重启不得改写为观察到的开关状态。
- 数据面不健康时，fail-open 恢复 Bifrost 接管前的配置；恢复健康并通过所有权检查后自动恢复 Bifrost。
- 保留企业代理、不同网络服务的独立配置及用户/其他应用的修改。
- 不保证重启期间已有 TCP 连接不断开；恢复原配置也不保证 VPN、企业网络或上游服务本身可达。

## 意图与启动参数

`system_proxy.intent_revision` 在每次明确的 enabled 更新时递增，包括相同值的再次选择。配置文件原子替换并同步目录；同一配置目录的写入由有界文件锁保护。其他配置保存不得覆盖磁盘上更新的代理意图。

API 按以下顺序执行：接受并保存请求 → 在代理互斥锁内重新验证版本和目标端口 → 写 OS → 回读。写入或回读失败返回错误和已接受意图；不会把失败观察保存成 disabled。较旧请求不得在较新请求后重放。

Settings 开关显示配置意图；OS 尚未启用或关闭后仍在生效时，用独立警告展示差异。失败请求不能回滚随后由推送或刷新得到的新意图，因此暂停期间也能明确选择关闭。

`runtime.json.system_proxy_config_revision` 标识启动时配置版本。版本仍相同时保留本次 `--system-proxy` / `--no-system-proxy` 覆盖；后续明确切换优先。旧 runtime 没有版本时，以持久配置为准。

自动重启同时携带生成 argv 的意图版本。子进程启动时如果配置已经更新，忽略过期自动参数，重新采用最新配置。自动恢复还携带所有权 generation；空 token 表示没有接管证明，只启动 core，不重新取得 OS 代理所有权。明确停止优先于自动恢复。

## 持久所有权

schema v3 区分 `pending_apply`、`applied`、`suspending`、`suspended`、`resuming`、`restoring`。旧 `applied=false` 不能被当成已完成暂停。

每个服务分别记录 HTTP、HTTPS、bypass 的原值、最后一次 Bifrost 写入值、可能中断的单条命令及 relinquished 状态。每条命令之前保存 journal，之后回读并保存结果。文件使用临时文件、文件同步、原子替换、目录同步；错误保留未完成工作。

- 恢复仅修改仍匹配该 generation 所记录写入的字段。
- 用户修改某个协议或 bypass 后，只释放该字段，不能覆盖整项服务。
- 自动重试、GUI/sudo 权限重试不重新拍摄原始快照。
- 明确的新 enable 可建立新 generation，并只为重新接管字段更新基线。
- 授权取消按 generation 保留抑制状态；恢复不能反复弹出同一授权，新的明确请求才能重试。
- 带认证的网络服务保持原样，因为无法完整读取/恢复密码。
- PAC/SOCKS 不在本次 HTTP/HTTPS 接管范围内，不应被改写。

旧 aggregate 快照无法重建多服务的不同原值。迁移只能保守处理仍匹配 Bifrost 的协议，不能承诺完整恢复历史企业代理。新 journal 以 network-service 名称识别服务；服务消失或被重命名时保留 journal 并报告恢复未完成，不猜测对应服务，也不宣称成功。稳定 UUID 映射仍需专门的原生验证。

## 运行中恢复

同一 core 只保留一个 OS 写入协调线程；wake 通知使其重新采样，不启动第二个写入者。

- 探针结果绑定当前目标端口与 generation。
- 健康恢复需要至少三个连续 canary 成功，且跨度至少两秒。
- fail-open 使用最后已知恢复策略和 grace；配置暂时不可读不能阻止对已确认所有权的失效目标做安全暂停。
- 未知意图不能授权 acquisition/resume；fail-closed 仍保持其策略。
- 暂停过程中失败的 `suspending` / `resuming` / `pending_apply` 必须继续恢复，不能只检查 `applied` 布尔值。
- 接受的关闭即使第一次 OS 操作失败，也要继续有界重试。
- 变化、拒绝、错误和实际应用结果分别记录，不能把尝试写入当成成功。

canary 验证本地 accept/HTTP dispatch，不证明 DNS、上游 TLS 或公网可达。

## 端口和进程交接

端口切换先启动并探测新 listener，再 generation-fenced retarget。journal 已改变或回读不确定时，即使部分命令失败也保留新旧 listener；旧端口只有在 OS 回读确认已不再引用时才退出。未完成交接阻止继续叠加切换；服务整体关闭时释放所有 listener。

Desktop watchdog 在破坏性操作之前取得生命周期 guard，并重新验证 epoch、PID/start identity、端口、runtime/PID marker 和 generation。旧探针不能终止新实例。自动替换不用 `--yes`，不触发通用发现后停止逻辑；失败重试有预算和 half-open，外部 runtime 不被接管。

异常 Desktop core 退出时 helper 暂停并保留 lease，让 Desktop 接替。明确停止执行最终恢复。helper 每次清理重试都在 OS 锁内检查 generation 与 runtime identity；同 generation 的进程交接也不能被旧 helper 清理。独立 CLI profile 清理另有目标和 CA 范围及 writer lock。

## 验证

自动测试覆盖：意图持久化失败、不同进程版本顺序、旧启动参数、API 目标端口竞态、部分 OS 写入、权限取消、原代理恢复、字段接管变化、journal 中断、端口交接、旧 watchdog 与手动替换竞争、失败重试和停止取消。

Desktop 的轻量测试 host 引用生产恢复模块，见 `desktop/src-tauri/recovery-tests/README.md`。它不能替代 Tauri 原生构建和 macOS 系统配置验证。

Linux 真进程回归 `e2e-tests/tests/test_proxy_recovery_handoff.py` 使用 Python 3.11+、临时 HOME/data-dir 与动态端口。设置 `BIFROST_BIN` 指向本次 CLI 构建后执行，验证明确的会话覆盖、过期自动启动参数、端口切换、旧 listener 退役，以及持久的 enabled/revision 未改变。脚本拒绝在非 Linux 平台运行；它不验证 macOS OS 写入。

`e2e-tests/tests/test_runtime_pressure_degradation.sh` 验证 critical 压力下的真实转发和管理 API 降级边界；它使用 `--no-system-proxy`，不能替代 OS 故障恢复测试。

原生验收必须在明确隔离且允许修改系统代理的 macOS 环境进行：不同服务/协议配置、权限对话框、睡眠唤醒、企业/VPN 基线、服务禁用/重命名、SIGKILL 和部分命令失败。云端 Linux 单元测试与 Darwin source-check 不得记作这些场景已通过。

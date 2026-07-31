# 代理网络稳定性与管理面性能治理

## 背景

2026-07-31 的运行日志同时出现外部网络抖动与 Bifrost 管理面自激负载：跨多个域名的 TCP connect timeout/reset 属于真实网络退化；Group 目录名补全在远端失败时被 Rules 页面轮询反复触发；系统代理 reconcile 在已经指向当前 Bifrost 时仍周期性进入完整服务检查；少量同步文件和系统命令运行在 Tokio worker 上，使轻量健康接口也可能因线程饥饿延迟。

本轮只治理 Bifrost 对故障的放大效应，不通过改变用户请求重试、DNS 缓存或代理响应状态来掩盖外部网络问题。

## 用户目标验证清单

### 必须实现

- Group 远端缓存解析 single-flight，失败后指数退避，Rules 页面轮询不能形成固定周期请求风暴。
- Group 缓存和 Badge 缓存的同步文件操作不占用 Tokio async worker。
- 系统代理已经收敛时不重复执行高成本完整 reconcile；发生漂移、配置变化或系统唤醒时仍能恢复。
- Admin API 设置、读取和验证系统代理时，阻塞 OS 命令在 blocking pool 中执行。
- WebSocket 上游非 `101 Switching Protocols` 与 DNS/TCP/TLS 传输错误分开记录。
- panic 诊断写入失败不得因 stdout/stderr Broken pipe 触发递归 panic。

### 必须不破坏

- 本地规则、Group 规则内容、排序、启停和本地目录降级语义保持不变。
- Group name/id 的历史反向映射和 Badge 跳转字段保持不变。
- 系统代理 enable/disable、bypass、外部代理 ownership、crash recovery、restart handoff 和 wake reconcile 保持不变。
- macOS 恢复 disabled 系统代理时，执行前已保存但未启用的 server/port/bypass 必须精确写回，不得残留 Bifrost 的隔离端口。
- HTTP、CONNECT、WebSocket/WSS 的返回状态、代理规则匹配和 Traffic 记录语义保持不变。
- Desktop watchdog 的 liveness/readiness 判定和恢复阈值保持不变。
- 不停止或重启共享的 9900 服务；测试使用隔离端口和数据目录。

### 必须真实验证

- 远端 Group 服务不可用时持续请求 active-summary，本地 Group 规则始终可见，远端解析次数受退避约束。
- 系统代理纯决策测试覆盖已收敛、漂移、外部 owner、disabled 和 bypass 变化；macOS 真实测试必须带恢复 trap。
- Admin 健康接口与代理数据面在 Group 失败和并发轮询下保持响应。
- WebSocket 上游拒绝仍返回既有 502，但日志分类不再标记成网络传输失败。
- panic writer 返回 Broken pipe 时报告函数返回错误且不 panic。

## 设计

### Group 缓存状态机

用带 generation 的互斥状态替代单一 `AtomicBool`：

- `generation`：登录态或缓存显式失效时递增。
- `in_flight_generation`：同一 generation 只允许一个远端解析任务。
- `resolved_generation`：全部本地目录已有远端映射后停止触发。
- `consecutive_failures` 与 `retry_not_before`：失败按 5、10、20、40 秒递增，最高 5 分钟；成功或显式失效后清零。
- 旧 generation 的异步完成结果不得修改新 generation。

active-summary 始终先使用本地目录名生成响应。远端补全是旁路任务，不参与本次响应，也不得删除、禁用或重写本地规则。任务结束后的 Badge cache 重建通过 `spawn_blocking` 执行。

### 系统代理 reconcile

- Admin API 的 `get_current`、enable/disable、全服务 ownership 验证均移入 `spawn_blocking`。
- 周期任务先执行轻量 ownership 检查。已经由当前 Bifrost 持有，且距离上次完整检查不足 5 分钟时跳过完整 apply。
- 完整检查保留在首次 enable、配置变化、系统唤醒、ownership 漂移和低频审计路径。
- 外部代理被识别为 owner 时只更新本地 managed flag，不写系统设置。
- 重复 Admin toggle 仍由同一 manager 写锁串行，API 响应格式不变。
- `scutil --proxy` 在代理 disabled 时可能省略 dormant endpoint；备份路径会从 `networksetup -getwebproxy/-getsecurewebproxy` 补读 server/port。恢复 disabled 状态时先写回保存的 endpoint/bypass，再关闭 enable 开关，避免 macOS 保留 Bifrost 写入值。

### 错误分类与 panic 防护

- WebSocket 上游返回非 101 是 `upstream_handshake_rejected`，使用结构化 warning；连接、读写、DNS 和 TLS 错误仍为 transport error。
- 代理响应保持原 502，避免兼容性变化。
- panic hook 使用可注入的 `Write` 接口生成诊断，忽略最终 stderr 写入错误，不使用会因 Broken pipe 再 panic 的 `eprintln!`。

## 灰度与回滚

- 三个风险域拆成独立提交，但位于同一功能分支和同一 MR。
- 不包含数据格式或 API schema 迁移；回滚任一提交不会要求清理用户数据。
- system proxy 真实验证前记录原始 OS 状态，并用 trap 在成功或失败时恢复。
- macOS focused E2E 对每个 network service 比较执行前后 Web/Secure Web 的 enable/server/port/auth 快照，disabled 不等于允许忽略 dormant 配置。
- 若 Group 退避出现兼容问题，可保留本地响应并只关闭远端自动补全；用户规则仍然生效。

## 性能和稳定性门槛

- active-summary 远端失败时，同 generation 内只有一个 in-flight；连续失败后的下一次请求不得绕过退避。
- 已收敛系统代理的常规轮询不执行写操作，完整检查次数相对 30 秒固定周期显著下降。
- Admin system proxy API 的 OS 命令不占用 Tokio worker。
- 同一代理回放下响应状态、规则命中和 Traffic 结果不变；吞吐回退不超过 3%，p99 延迟回退不超过 5%。
- CI 通过 `bash scripts/ci/coverage-all.sh --json --gate` 的 crate 与 workspace coverage 门禁。

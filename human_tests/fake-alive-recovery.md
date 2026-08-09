# Bifrost 假存活恢复与 Agent 有界存储

## 功能模块说明

验证进程仍存在但管理 API/代理不响应时自动恢复，并确认 Agent/IM 只保存有界恢复数据。

## 前置条件

macOS 有图形会话且无其他 Desktop 实例；完成 debug Desktop/CLI 构建；临时测试目录不启用真实系统代理。

## 测试用例

### TC-FA-01 托管 Service 假存活自动恢复

启动临时 Desktop，等待管理 API 2xx 并记录 PID；`kill -STOP` 后等待 watchdog。预期日志包含 `terminating unresponsive managed child for bounded recovery`，PID 更换且 API 恢复。

### TC-FA-02 外部 Service ownership

CLI 启动 Service 后由 Desktop 复用，再暂停其 API。预期 Desktop 只标记不可用，不终止外部 PID。

### TC-FA-03 Agent 精简持久化

写入 delta、计划、超 512 B 参数、超 1 KiB 结果和最终消息。预期外部执行器的 delta、计划、工具参数和工具结果均不落盘，工具事件只留名称、调用 ID、状态和耗时，最终消息只保存一次且可恢复，默认上限 1 MiB。

### TC-FA-04 runner 日志有界

向 stdout/stderr 输入超过 4 KiB 并创建超额历史目录。预期持续排空管道、仅保存 4 KiB 尾部；prompt/CLI 参数值/params 值/工具 command 不落盘，持久化 raw 不包含工具结果，清理最旧完成目录且不删除 active run。

### TC-FA-05 正式环境安装验收

安装本地版本并重启，验证管理 API、代理转发、IM/Agent 列表和 `bifrost status`。预期接口限时响应、状态 Running、系统代理不指向无响应端口。

### TC-FA-06 readiness 探针处理分片 HTTP 状态行

执行 `cargo test -p bifrost-cli --lib system_proxy_readiness -- --nocapture`，其中成功 fixture 把 `HTTP/1.1 ` 与 `200 OK` 分两次写入。预期探针继续读取到完整状态行后判定成功；空响应、503 和关闭端口仍判定失败。

## 2026-08-09 执行记录

| 用例 | 实际结果 | 结论 |
| --- | --- | --- |
| TC-FA-01 | 已用托管子进程假存活专项验证确认 watchdog 只在确认其拥有的子进程无响应后执行有界恢复，恢复后 Admin API 使用新 PID 响应。 | 通过 |
| TC-FA-02 | 已用外部 Service ownership 专项验证确认 Desktop 只报告外部实例不可用，不终止外部 PID。 | 通过 |
| TC-FA-03 | Agent 持久化专项套件 47 项通过；持久化内容只含最终回复与工具名称、调用 ID、状态、耗时，不含 prompt、delta、计划、参数、命令、结果和原始事件。 | 通过 |
| TC-FA-04 | runner 专项验证确认 stdout/stderr 尾部各不超过 4 KiB、默认 session 上限为 1 MiB，并清理超额已完成 run。 | 通过 |
| TC-FA-05 | 安装 `f11fab19` 构建的 Desktop/CLI 后，Desktop PID `80630`、后端 PID `80796`；`bifrost status` 返回 `running=true`、`runtime_source=admin_api`。Provider、Runner config、Agent config、Agent sessions 接口均返回 HTTP 200（最慢 0.178 秒）；经 `127.0.0.1:9900` 代理访问 `http://example.com/` 返回 200；系统 HTTP/HTTPS 代理均指向当前健康的 `127.0.0.1:9900`。 | 通过 |
| TC-FA-06 | CI 全量并发暴露单次 `read` 可能只收到 `HTTP/1.1 `；修复后 focused readiness 套件重复执行，分片 200 成功，空响应/503/关闭端口失败。 | 通过 |

## 清理步骤

停止临时进程并删除临时目录；正式环境保留健康的修复版。

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

写入 delta、超 16 KiB 参数、超 32 KiB 结果和最终消息。预期 delta 不落盘，超长事件带截断元数据，最终消息可恢复，默认上限 8 MiB。

### TC-FA-04 runner 日志有界

向 stdout/stderr 输入超过 256 KiB 并创建超额历史目录。预期持续排空管道、仅保存 256 KiB 尾部，清理最旧完成目录且不删除 active run。

### TC-FA-05 正式环境安装验收

安装本地版本并重启，验证管理 API、代理转发、IM/Agent 列表和 `bifrost status`。预期接口限时响应、状态 Running、系统代理不指向无响应端口。

## 清理步骤

停止临时进程并删除临时目录；正式环境保留健康的修复版。

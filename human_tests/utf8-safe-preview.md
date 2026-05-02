# UTF-8 安全 Preview 截断真实场景测试

## 功能模块说明

验证 Bifrost 在生成 Agent compaction 输入、IM Gateway 任务输出 preview、CLI/API 错误 preview 时，对中文、emoji 等多字节 UTF-8 内容执行截断不会触发 panic，也不会生成非法 UTF-8 字符串。

## 前置条件

- 在仓库根目录 `/Users/eden/work/github/bifrost` 执行。
- 不需要启动 Bifrost 服务；本用例通过真实 cargo 测试和 E2E 回归脚本触发对应代码路径。
- 本用例不涉及系统代理，不使用 9900 端口。

## 测试用例列表

### TC-USP-01：回归 - Agent compaction tool arguments 中文边界不 panic

**操作步骤：**

1. 执行：
   ```bash
   cargo test -p bifrost-agent compact::tests::test_format_history_handles_multibyte_tool_arguments -- --nocapture
   ```
2. 检查命令退出码。

**预期结果：**

- 测试通过。
- 不出现 `end byte index 500 is not a char boundary`。
- 输出显示该测试 `ok`。

### TC-USP-02：回归 - IM Gateway 任务输出中文 preview 不 panic

**操作步骤：**

1. 执行：
   ```bash
   cargo test -p bifrost-admin im_gateway::task_executor::tests::test_truncate_preview_multibyte_boundary -- --nocapture
   ```
2. 检查命令退出码。

**预期结果：**

- 测试通过。
- 不出现 UTF-8 char boundary panic。
- preview 保留中文前缀并带 `...[truncated]` 标记。

### TC-USP-03：E2E 回归 - 全链路 UTF-8 preview 截断集合验证

**操作步骤：**

1. 执行：
   ```bash
   bash e2e-tests/tests/test_utf8_safe_preview_e2e.sh
   ```
2. 检查脚本输出中的每个 `PASS`。

**预期结果：**

- 脚本以 0 退出。
- core helper、Agent compaction、IM Gateway task preview、proxy body preview、E2E assertion preview 相关检查全部 PASS。
- 不出现 `char boundary` panic。

## 清理步骤

- 本用例不启动服务、不创建持久化数据目录，无需额外清理。

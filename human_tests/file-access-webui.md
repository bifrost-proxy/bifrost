# File Access WebUI 策略配置测试

## 功能模块说明

WebUI Settings → Remote Invoke 的 Grants 列表提供行级 `File Access` 按钮，用于为每个 active grant 绑定 exact `grant_id` 文件访问策略。配置存储在 `<data-dir>/file-access.toml`。页面不再提供独立的 File Access 管理卡片，也不允许手动录入不存在的 grant。

## 前置条件

1. 启动 Bifrost 服务（带临时数据目录，禁止使用 9900）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-system-proxy
   ```
2. 在浏览器中打开 `http://localhost:8800/_bifrost/`，导航到 Settings → Remote Invoke。
3. 至少准备一个 `status = active` 的 grant。

## 测试用例

### TC-FAW-01: 不再展示独立 File Access 管理模块

- **操作步骤**：打开 Settings → Remote Invoke，检查 Grants 区域附近的卡片和按钮。
- **预期结果**：
  - 页面没有独立的 "File Access" 管理卡片。
  - 页面没有 "Manage Policies" 全局按钮。
  - File Access 配置入口只出现在 active grant 行动作中。

### TC-FAW-02: 每个 active grant 行展示 File Access 按钮

- **操作步骤**：准备一个 active grant，打开 Grants 列表。
- **预期结果**：
  - 该 grant 行显示 `File Access` 按钮。
  - removed grant 行不显示 `File Access` 按钮。

### TC-FAW-03: 打开 per-grant File Access 编辑器

- **操作步骤**：点击某个 active grant 行的 `File Access` 按钮。
- **预期结果**：
  - 弹出标题为 `File Access: <caller>` 的模态框。
  - 模态框提示该策略绑定当前 grant，grant 被 revoke 后策略会同步删除。
  - Grant ID 输入框只读，值为当前行的 `grant_id`。

### TC-FAW-04: 新策略不支持手动录入不存在 grant

- **操作步骤**：
  1. 打开任意 active grant 的 File Access 编辑器。
  2. 检查 Grant ID 控件。
  3. 尝试把 Grant ID 改为 `not-connected-grant`。
- **预期结果**：
  - Grant ID 控件为 disabled / readonly。
  - 无法录入或保存 `not-connected-grant`。

### TC-FAW-05: 保存只读 + 指定目录策略

- **操作步骤**：
  1. 打开 active grant 的 File Access 编辑器。
  2. Type 选择 `Read Only`。
  3. Directories 选择 `Selected`。
  4. Allowed Roots 输入 `/tmp/test-project`。
  5. 点击 `Save`。
- **预期结果**：
  - 保存成功，弹出 `File access config saved`。
  - `<data-dir>/file-access.toml` 包含该 grant 的 exact `grant_id` 策略。
  - 该策略 roots 包含 `/tmp/test-project`，ops 只包含读类操作。

### TC-FAW-06: 保存读写 + 所有目录策略

- **操作步骤**：
  1. 打开 active grant 的 File Access 编辑器。
  2. Type 选择 `Read Write`。
  3. Directories 选择 `All`。
  4. 点击 `Save`。
- **预期结果**：
  - 保存成功。
  - `<data-dir>/file-access.toml` 中该策略 `roots = ["/"]`。
  - ops 包含读写文件操作。

### TC-FAW-07: 指定目录为空时拒绝保存

- **操作步骤**：
  1. 打开 active grant 的 File Access 编辑器。
  2. Directories 选择 `Selected`。
  3. 清空 Allowed Roots。
  4. 点击 `Save`。
- **预期结果**：
  - 弹出 `Enter at least one allowed directory`。
  - 不写入空 roots 的策略。

### TC-FAW-08: 编辑已有策略保持同一个 grant 绑定

- **操作步骤**：
  1. 为 active grant 保存一条策略。
  2. 再次点击同一 grant 行的 `File Access`。
  3. 修改 Name 或 Type 后保存。
- **预期结果**：
  - 编辑器加载已有策略值。
  - 保存后仍只有该 `grant_id` 的一条策略，不产生重复条目。

### TC-FAW-09: Deny Patterns 可编辑并保存

- **操作步骤**：
  1. 打开 active grant 的 File Access 编辑器。
  2. 在 Deny Patterns 中添加 `**/node_modules/**`。
  3. 在 Write Deny Patterns 中添加 `**/package-lock.json`。
  4. 点击 `Save`。
- **预期结果**：
  - 保存成功。
  - `file-access.toml` 中包含更新后的 `denies` 和 `write_denies`。

### TC-FAW-10: 字节限制和开关可编辑并保存

- **操作步骤**：
  1. 打开 active grant 的 File Access 编辑器。
  2. Max Read Bytes 设为 `1048576`。
  3. Max Write Bytes 设为 `524288`。
  4. 切换 Respect .gitignore、Allow Overwrite、Allow Recursive Delete。
  5. 点击 `Save`。
- **预期结果**：
  - 保存成功。
  - `file-access.toml` 中包含对应字节限制和开关值。

### TC-FAW-11: 多个 grant 分别配置独立策略

- **操作步骤**：
  1. 准备两个 active grants。
  2. 分别点击两行的 `File Access`。
  3. 第一个保存只读指定目录，第二个保存读写所有目录。
- **预期结果**：
  - `file-access.toml` 中存在两条不同 `grant_id` 策略。
  - 两条策略互不覆盖。

### TC-FAW-12: Revoke grant 自动清理对应 File Access 策略

- **操作步骤**：
  1. 准备一个 active grant，并通过该 grant 行 `File Access` 保存策略。
  2. 在 Grants 列表中点击该 grant 的 `Revoke` 并确认。
  3. 重新获取 `/api/remote-invoke/file-access-config` 或检查 `file-access.toml`。
- **预期结果**：
  - 被 revoke 的 grant 从 Grants 列表消失或变为 removed。
  - 绑定该 `grant_id` 的 File Access 策略同步消失。
  - `file-access.toml` 中不再包含该 `match.grant_id` 或 legacy `grant_id` 条目。
  - SSH fingerprint / caller fingerprint 类型默认策略不受影响。

### TC-FAW-13: Grant 删除后重新连接可重新配置

- **操作步骤**：
  1. Revoke 已有 grant 并确认对应策略被删除。
  2. 让 caller 重新连接生成新的 active grant。
  3. 点击新 grant 行的 `File Access` 保存策略。
- **预期结果**：
  - 新 grant 可正常打开 File Access 编辑器。
  - 保存后写入新的 `grant_id` 策略。
  - 旧 grant 的幽灵配置不会恢复。

### TC-FAW-14: removed grant 不允许保存策略

- **操作步骤**：
  1. 打开 active grant 的 File Access 编辑器。
  2. 在保存前 revoke 该 grant 或刷新为 removed 状态。
  3. 点击 `Save`。
- **预期结果**：
  - 保存失败并提示 `Grant is not connected`。
  - 不写入 removed grant 策略。

### TC-FAW-15: GET API 直接验证

- **操作步骤**：
  ```bash
  curl -s http://localhost:8800/api/remote-invoke/file-access-config | python3 -m json.tool
  ```
- **预期结果**：
  - 返回 JSON，包含 `grant` 数组。
  - per-grant 策略项包含当前 active grant 的 `grant_id`。

### TC-FAW-16: SSH Key 默认 File Policy 不受 per-grant 清理影响

- **操作步骤**：
  1. 在 SSH Key 卡片配置 `match.ssh_fingerprint` 默认 File Policy。
  2. 为某个 active grant 配置 exact `grant_id` 策略。
  3. Revoke 该 active grant。
- **预期结果**：
  - exact `grant_id` 策略被删除。
  - `match.ssh_fingerprint` 默认策略仍保留。

## 清理步骤

1. 停止 Bifrost 服务。
2. 删除测试数据目录：`rm -rf ./.bifrost-test`。

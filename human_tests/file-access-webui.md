# File Access WebUI 策略配置测试

## 功能模块说明

WebUI Settings 页面中的 File Access 策略配置卡片和编辑器，用于管理 per-grant 的文件访问策略。配置存储在 `<data-dir>/file-access.toml`。

## 前置条件

1. 启动 Bifrost 服务（带临时数据目录）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-system-proxy
   ```
2. 在浏览器中打开 `http://localhost:8800/_bifrost/`，导航到 Settings 页面

## 测试用例

### TC-FAW-01: File Access 卡片在 Settings 页面显示

- **操作步骤**：打开 Settings 页面，滚动到 Remote Invoke 区域
- **预期结果**：
  - 可以看到 "File Access" 卡片（带 FileOutlined 图标）
  - 卡片右上角有 Reload 和 "Manage Policies" 按钮
  - 卡片内有蓝色 info Alert 提示 "File access is governed by per-grant policies..."
  - 显示 "Grant Policies: 0 configured"
  - 显示空状态 "No per-grant file access policies configured..."

### TC-FAW-02: 打开 File Access 编辑器

- **操作步骤**：点击 File Access 卡片的 "Manage Policies" 按钮
- **预期结果**：
  - 弹出 "Manage File Access Policies" 模态框
  - 模态框内有 info Alert 提示
  - 显示 "Grant Policies" 标题和 "Add Policy" 按钮
  - 初始为空状态

### TC-FAW-03: 添加新的 Grant Policy

- **操作步骤**：
  1. 在编辑器中点击 "Add Policy"
  2. 填写 Grant ID: `g-test-001`
  3. 填写 Name: `Test Project`
  4. 在 Allowed Roots 中输入 `/tmp/test-project`
  5. Allowed Operations 保持默认的读操作
  6. 点击 "Save"
- **预期结果**：
  - 保存成功，弹出 "File access config saved" 消息
  - File Access 卡片现在显示 "Grant Policies: 1 configured"
  - 列表显示策略条目：名称 "Test Project"、标签 "g-test-001"、"read-only" 标签
  - `<data-dir>/file-access.toml` 文件已创建，包含正确的 TOML 内容

### TC-FAW-04: 编辑已有 Grant Policy — 添加写操作

- **操作步骤**：
  1. 点击 "Manage Policies" 打开编辑器
  2. 在已有的 g-test-001 策略中，点击 "All Ops" 快捷按钮
  3. 点击 "Save"
- **预期结果**：
  - 保存成功
  - 卡片中策略标签变为 "read+write"（橙色）
  - 描述中显示 "write ops 6"

### TC-FAW-05: 添加多个 Grant Policy

- **操作步骤**：
  1. 打开编辑器，点击 "Add Policy" 添加第二个策略
  2. Grant ID: `g-test-002`, Name: `Read Only Project`
  3. Allowed Roots: `/tmp/readonly-project`
  4. 保持默认读操作
  5. 点击 "Save"
- **预期结果**：
  - 卡片显示 "Grant Policies: 2 configured"
  - 列表显示两个策略条目

### TC-FAW-06: 修改 Deny Patterns

- **操作步骤**：
  1. 打开编辑器
  2. 在 g-test-001 策略的 Deny Patterns 中添加 `**/node_modules/**`
  3. 在 Write Deny Patterns 中添加 `**/package-lock.json`
  4. 点击 "Save"
- **预期结果**：
  - 保存成功
  - `file-access.toml` 中包含更新的 denies 和 write_denies

### TC-FAW-07: 修改字节限制和开关

- **操作步骤**：
  1. 打开编辑器
  2. 在 g-test-001 策略中：
     - Max Read Bytes 设为 `1048576`（1MB）
     - Max Write Bytes 设为 `524288`（512KB）
     - 关闭 "Allow Recursive Delete" 开关
     - 打开 "Respect .gitignore" 开关（应该默认已开启）
  3. 点击 "Save"
- **预期结果**：
  - 保存成功
  - TOML 文件中包含 `max_read_bytes = 1048576` 和 `max_write_bytes = 524288`

### TC-FAW-08: 删除 Grant Policy

- **操作步骤**：
  1. 打开编辑器
  2. 点击 g-test-002 策略卡片右上角的红色删除按钮
  3. 点击 "Save"
- **预期结果**：
  - 卡片显示 "Grant Policies: 1 configured"
  - 只剩 g-test-001

### TC-FAW-09: Grant ID 重复校验

- **操作步骤**：
  1. 打开编辑器
  2. 添加一个新策略，Grant ID 填写 `g-test-001`（已存在）
  3. 点击 "Save"
- **预期结果**：
  - 弹出错误消息 "Duplicate grant ID: g-test-001"
  - 不保存

### TC-FAW-10: Grant ID 为空校验

- **操作步骤**：
  1. 打开编辑器
  2. 添加一个新策略，Grant ID 留空
  3. 点击 "Save"
- **预期结果**：
  - 弹出错误消息 "Every file access policy needs a grant ID"
  - 不保存

### TC-FAW-11: Reload 按钮刷新配置

- **操作步骤**：点击 File Access 卡片的 Reload（刷新）按钮
- **预期结果**：
  - 卡片数据刷新，显示最新的策略配置
  - 刷新过程中按钮显示 loading 状态

### TC-FAW-12: GET API 直接验证

- **操作步骤**：
  ```bash
  curl -s http://localhost:8800/api/remote-invoke/file-access-config | python3 -m json.tool
  ```
- **预期结果**：
  - 返回 JSON，包含 `grant` 数组
  - 数组中包含之前配置的策略数据

## 清理步骤

1. 停止 Bifrost 服务
2. 删除测试数据目录：`rm -rf ./.bifrost-test`

# 响应头变化来源展示

## 功能模块说明

验证 Traffic 详情不会把 HTTP 协议规范化误导成用户规则删除，并验证 network `.bifrost` 导出、导入后仍能区分上游原始响应头和发送给客户端的响应头。

## 前置条件

1. 在仓库根目录构建当前源码：

   ```bash
   CARGO_TARGET_DIR=./.bifrost-ui-target cargo build --bin bifrost
   ```

2. 使用临时数据目录和非正式端口启动后端，必须保留托盘、登录页和系统代理护栏：

   ```bash
   export BIFROST_HEADER_TEST_DIR="$(mktemp -d)"
   export BIFROST_DATA_DIR="$BIFROST_HEADER_TEST_DIR/data"
   export BIFROST_DISABLE_TRAY=1
   export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1
   ./.bifrost-ui-target/debug/bifrost start --host 127.0.0.1 -p 18891 --unsafe-ssl --skip-cert-check --no-system-proxy --access-mode allow_all
   ```

3. 在另一个终端启动前端：

   ```bash
   cd web
   BACKEND_PORT=18891 WEB_PORT=18892 pnpm exec vite --host 127.0.0.1 --port 18892
   ```

4. 导入固定双快照 fixture：

   ```bash
   curl -fsS -X POST -H 'Content-Type: text/plain' \
     --data-binary @human_tests/fixtures/response-header-change-display.bifrost \
     http://127.0.0.1:18891/_bifrost/api/bifrost-file/import
   ```

## 测试用例

### TC-RHCD-01：无规则时 Connection 显示为协议处理

操作步骤：

1. 用浏览器打开 `http://127.0.0.1:18892/_bifrost/traffic`。
2. 选择路径为 `/human-protocol-only` 的记录。
3. 打开 Response 的 Header 面板。
4. 查看页签、变化摘要和 `connection` 行。

预期结果：

- 页签为 `Sent to client` 和 `Upstream original`。
- 摘要为 `Configured changes: 0`、`Protocol handling: 1`。
- `connection: keep-alive` 带 `Protocol handling` 标识，使用信息色，不使用红色危险删除语义。
- 提示说明该变化来自 HTTP 转发兼容处理，不代表规则、脚本或断点修改。

### TC-RHCD-02：配置修改与协议处理分开统计

操作步骤：

1. 选择路径为 `/human-configured` 的记录。
2. 打开 Response 的 Header 面板。
3. 查看摘要、`x-bifrost-test` 和 `connection` 两行。

预期结果：

- 摘要为 `Configured changes: 1`、`Protocol handling: 1`。
- `x-bifrost-test: added` 使用配置新增语义。
- `connection: keep-alive` 仍使用协议处理语义，两类变化不会合并成同一种“删除”。

### TC-RHCD-03：亮色和暗色主题均清晰可辨

操作步骤：

1. 在亮色主题查看 TC-RHCD-01 的摘要、页签和协议处理行。
2. 点击左下角主题按钮切换为暗色主题。
3. 再次查看同一元素和提示。

预期结果：

- 两种主题下文字、背景、边框和选中页签均清晰可辨。
- 协议处理保持中性信息语义，配置删除才使用危险色。
- 切换主题不改变变化计数和当前选中记录。

### TC-RHCD-04：导出再导入后保留两个响应头快照

操作步骤：

1. 导出协议处理记录：

   ```bash
   curl -fsS -X POST -H 'Content-Type: application/json' \
     -d '{"record_ids":["OUT-REQ-human-protocol-only"],"include_body":false}' \
     http://127.0.0.1:18891/_bifrost/api/bifrost-file/export/network \
     -o "$BIFROST_HEADER_TEST_DIR/response-header-roundtrip.bifrost"
   ```

2. 确认导出内容同时包含两个字段：

   ```bash
   rg '"response_headers"|"original_response_headers"|"connection"' "$BIFROST_HEADER_TEST_DIR/response-header-roundtrip.bifrost"
   ```

3. 清空流量并重新导入该文件：

   ```bash
   curl -fsS -X DELETE http://127.0.0.1:18891/_bifrost/api/traffic
   curl -fsS -X POST -H 'Content-Type: text/plain' \
     --data-binary @"$BIFROST_HEADER_TEST_DIR/response-header-roundtrip.bifrost" \
     http://127.0.0.1:18891/_bifrost/api/bifrost-file/import
   ```

4. 刷新 Traffic 页面，选择重新导入的 `/human-protocol-only` 记录并查看 Response Header。

预期结果：

- 导出文件的 `response_headers` 不含 `connection`，`original_response_headers` 含 `connection: keep-alive`。
- 重新导入后仍显示 `Configured changes: 0`、`Protocol handling: 1`。
- 历史旧格式文件缺少 `original_response_headers` 时仍能导入，不产生虚假的响应头修改。

## 清理步骤

1. 关闭测试浏览器页面和 Vite 进程。
2. 仅停止本用例记录 PID 的 18891 测试进程，不得按进程名全局清理。
3. 删除本次 `mktemp` 生成的 `$BIFROST_HEADER_TEST_DIR` 测试目录。
4. 确认正式端口 9900 的监听进程未发生变化。

## 最近执行记录

- 2026-08-05，使用当前源码、临时数据目录、后端 `18891` 和前端 `18892` 执行：
  - TC-RHCD-01 通过：外部 Chromium 浏览器中确认 `Sent to client` / `Upstream original`、`Configured changes: 0`、`Protocol handling: 1`；`connection` 为信息色且 Tooltip 明确“不代表规则、脚本或断点修改”。
  - TC-RHCD-02 通过：配置 fixture 显示 `Configured changes: 1`、`Protocol handling: 1`；`x-bifrost-test` 为绿色新增，`connection` 为蓝色协议处理。
  - TC-RHCD-03 通过：实际切换 `data-theme=dark`，亮暗主题下页签、摘要和两类差异均清晰可辨，计数保持不变。
  - TC-RHCD-04 通过：真实导出文件同时包含最终 `response_headers` 和上游 `original_response_headers`；清空后重新导入仍显示 0/1；另导入缺少新字段的旧格式 fixture，页面不显示差异页签且正常显示原始 `connection`，未产生虚假修改。
  - 正式端口 9900 执行前后均由原 PID `22551` 监听，测试未触碰正式服务。

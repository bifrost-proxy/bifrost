# Bifrost Badge Hover 规则详情面板测试用例

## 功能模块说明

在被代理页面中注入的 Bifrost Badge（左下角圆点），hover 时向上展开一个面板展示当前启用的规则详情。

- 面板数据在 HTML 注入时内联（非跨域 fetch），避免跨站安全风险
- 规则数据通过 `AdminState.badge_rules_cache` 缓存，规则变更时自动刷新（启动/API 操作/热重载）
- 面板展示：规则列表（My Rules + 分组规则）、合并规则内容（可折叠）
- 规则行可点击在新窗口中打开对应的规则编辑页面

## 前置条件

```bash
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --intercept --enable-badge-injection
```

---

## 测试用例

### TC-BHP-01：Badge 注入包含面板 HTML、内联数据和脚本

**操作步骤**：
```bash
curl -x http://127.0.0.1:8800 http://httpbin.org/html -s
```

**预期结果**：
- HTML 中包含 `__bifrost_badge__`、`__bb_panel__`（面板容器）
- HTML 中包含 `<script>` 标签及 `merged_content`、`admin_port` 内联数据
- **不**包含 `fetch(` 调用（数据完全内联，无跨域请求）

---

### TC-BHP-02：Hover 展开面板展示规则列表

**操作步骤**：
1. 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name":"test-badge-rule","content":"example.com mock 200\nhttpbin.org mock 302 https://google.com","enabled":true}' -s
   ```
2. 通过代理获取 HTML 并在浏览器中打开
3. 鼠标悬浮到左下角 Badge 圆点上

**预期结果**：
- 面板从 Badge 上方向上展开
- 面板标题显示 "Active Rules" + 活跃规则数
- "My Rules" 分区显示 `test-badge-rule`（2 rules）
- 面板滚动条仅在最外层卡片上，无内部嵌套滚动条

---

### TC-BHP-03：规则行点击跳转到规则编辑页

**操作步骤**：
1. Hover 展开面板后，检查规则行的 HTML

**预期结果**：
- 私有规则链接格式：`http://127.0.0.1:8800/_bifrost/rules?rule=test-badge-rule`
- 小组规则链接格式：`http://127.0.0.1:8800/_bifrost/rules?group={group_id}&rule={name}`
- 链接带 `target="_blank" rel="noopener"`，点击在新窗口打开

---

### TC-BHP-04：Merged Rules 折叠展开

**操作步骤**：
1. Hover 展开面板
2. 点击 "▾ Merged Rules" 标题

**预期结果**：
- 折叠区域展开，显示合并后的规则文本
- 内容为等宽字体，保留换行格式
- 再次点击可折叠收起

---

### TC-BHP-05：Merged Rules 一键复制

**操作步骤**：
1. 使用临时数据目录和非 9900 端口启动 Bifrost，带 `--no-system-proxy --enable-badge-injection`
2. 创建并启用一条包含多行规则内容的规则
3. 通过代理在浏览器中打开测试 HTML 页面
4. 鼠标悬浮到左下角 Badge 圆点上
5. 点击 "▾ Merged Rules" 标题展开代码框
6. 点击代码框右上角的 "Copy" 按钮
7. 读取系统剪贴板内容，或粘贴到可编辑输入框中检查内容，禁止只看按钮状态

**预期结果**：
- Merged Rules 代码框右上角显示复制按钮
- 点击复制后按钮短暂显示 "Copied"
- 剪贴板内容等于当前展开的合并规则文本，包含规则换行与缩进
- 如果浏览器拒绝写入剪贴板，按钮显示 "Failed"，不能在剪贴板为空时显示 "Copied"

---

### TC-BHP-06：Badge 弹窗层级高于页面高 z-index 浮层

**操作步骤**：
1. 使用临时数据目录和非 9900 端口启动 Bifrost，带 `--no-system-proxy --enable-badge-injection`
2. 通过代理打开一个包含 `z-index: 2147483646` 固定定位覆盖层的测试 HTML 页面
3. 鼠标悬浮到左下角 Badge 圆点上
4. 观察 Badge 和展开后的面板是否被覆盖层遮挡

**预期结果**：
- Badge 圆点显示在页面覆盖层上方
- Hover 后面板显示在页面覆盖层上方
- 面板中的规则列表和 Merged Rules 区域可正常点击

---

### TC-BHP-07：暗色模式适配

**操作步骤**：
1. 系统切换到暗色模式
2. Hover 展开面板

**预期结果**：
- 面板背景为深色（#1f1f1f）
- 文字颜色适配暗色主题
- Merged Rules 代码块背景也为深色

---

### TC-BHP-08：高性能缓存验证

**操作步骤**：
1. 查看启动日志确认初始缓存加载
2. 通过 API 创建/删除规则，然后请求代理 HTML 查看面板数据

**预期结果**：
- 面板数据在规则变更后自动更新（无需重启服务）
- 高并发下 badge 注入不会触发文件系统 IO（使用缓存读取）

---

### TC-BHP-09：禁用 badge 后面板不注入

**操作步骤**：
```bash
curl -X PUT http://127.0.0.1:8800/_bifrost/api/config/performance \
  -H "Content-Type: application/json" -d '{"inject_bifrost_badge":false}' -s
curl -x http://127.0.0.1:8800 http://httpbin.org/html -s | grep "__bb_panel__"
```

**预期结果**：
- 返回的 HTML 中不包含 `__bifrost_badge__` 和 `__bb_panel__`

---

### TC-BHP-10：回归 - Merged Rules 中的 HTML/Script 标签片段不会逃逸为页面注入

**操作步骤**：
1. 使用临时数据目录和非 9900 端口启动 Bifrost，带 `--no-system-proxy --enable-badge-injection`：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-human-badge-escape \
     CARGO_TARGET_DIR=./.bifrost-target-human-badge-escape \
     cargo run --bin bifrost -- start -p 18880 --unsafe-ssl --no-system-proxy --enable-badge-injection
   ```
2. 启动一个本地 HTML 服务：
   ```bash
   python3 -m http.server 18881 --bind 127.0.0.1 --directory e2e-tests/test_data/badge_injection
   ```
3. 创建一条启用规则，规则目标不要匹配当前测试页面，但 Merged Rules 内容包含多种 HTML/Script 标签注入形态：
   ```bash
   python3 - <<'PY' >/tmp/bifrost-badge-escape-rule.json
   import json
   payload = {
       "name": "badge-html-tag-escaping-regression",
       "content": "\n".join([
           "not-current-test.local htmlAppend://{vconsole-inject}",
           "``` vconsole-inject",
           '<script src="https://unpkg.com/vconsole/dist/vconsole.min.js"></script>',
           "<script>new VConsole();</script>",
           "<!-- <img src=x onerror=alert(1)> <svg onload=alert(1)>",
           '<iframe srcdoc="<script>alert(1)</script>"></iframe></textarea>',
           "```",
       ]),
       "enabled": True,
   }
   print(json.dumps(payload))
   PY
   curl -X POST http://127.0.0.1:18880/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     --data-binary @/tmp/bifrost-badge-escape-rule.json -s
   ```
4. 通过代理请求不匹配该规则的普通 HTML 页面：
   ```bash
   curl -x http://127.0.0.1:18880 http://127.0.0.1:18881/index.html -s -o /tmp/bifrost-badge-escape.html
   ```
5. 检查响应 HTML：
   ```bash
   python3 - <<'PY'
   from pathlib import Path
   html = Path("/tmp/bifrost-badge-escape.html").read_text()
   assert "__bifrost_badge__" in html
   assert "not-current-test.local htmlAppend://{vconsole-inject}" in html
   assert "\\u003Cscript" in html
   assert "\\u003C/script\\u003E" in html
   assert "\\u003C!--" in html
   assert "\\u003Cimg" in html
   assert "\\u003Csvg" in html
   assert "\\u003Ciframe" in html
   assert '<script src=\\"https://unpkg.com/vconsole/dist/vconsole.min.js\\"' not in html
   assert '</script>\\n<script>new VConsole();</script>' not in html
   assert "<!-- <img" not in html
   assert "<svg onload" not in html
   assert "<iframe srcdoc" not in html
   print("TC-BHP-10 passed")
   PY
   ```

**预期结果**：
- 页面仍正常注入 Badge，包含 `__bifrost_badge__`
- Merged Rules 数据中仍可看到规则文本 `not-current-test.local htmlAppend://{vconsole-inject}`
- 内联数据中的 `<script>`、`</script>`、`<!--`、`<img>`、`<svg>`、`<iframe>`、`</textarea>` 等标签片段全部以 `\u003C...` / `\u003E` 等形式存在
- 响应 HTML 中不包含从 Merged Rules 逃逸出来的原始 vConsole `<script src=...>`、`<script>new VConsole()`、HTML 注释或事件属性标签片段

---

### TC-BHP-11：回归 - 响应头误标为 HTML 的 JSON 数据接口不注入 Badge

**操作步骤**：
1. 使用临时数据目录和非 9900 端口启动 Bifrost，带 `--no-system-proxy --enable-badge-injection`：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-human-badge-json \
     CARGO_TARGET_DIR=./.bifrost-target-human-badge-json \
     cargo run --bin bifrost -- start -p 18882 --unsafe-ssl --no-system-proxy --enable-badge-injection
   ```
2. 启动一个本地上游服务，返回 `Content-Type: text/html; charset=utf-8` 但 body 是 JSON：
   ```bash
   python3 - <<'PY'
   from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

   class Handler(BaseHTTPRequestHandler):
       def do_GET(self):
           body = b'{"code":200,"data":{"mode":"slide","challenge_code":99999}}'
           self.send_response(200)
           self.send_header("Content-Type", "text/html; charset=utf-8")
           self.send_header("Content-Length", str(len(body)))
           self.end_headers()
           self.wfile.write(body)

       def log_message(self, *_):
           pass

   ThreadingHTTPServer(("127.0.0.1", 18883), Handler).serve_forever()
   PY
   ```
3. 通过代理请求该误标 JSON 接口：
   ```bash
   curl -x http://127.0.0.1:18882 http://127.0.0.1:18883/captcha/get -s -o /tmp/bifrost-badge-mislabeled-json.txt
   ```
4. 检查响应内容：
   ```bash
   python3 - <<'PY'
   import json
   from pathlib import Path

   body = Path("/tmp/bifrost-badge-mislabeled-json.txt").read_text()
   parsed = json.loads(body)
   assert parsed["code"] == 200
   assert parsed["data"]["mode"] == "slide"
   assert "__bifrost_badge__" not in body
   assert "__bb_copy" not in body
   print("TC-BHP-11 passed")
   PY
   ```

**预期结果**：
- 响应体仍是可被 `json.loads` 解析的原始 JSON
- 响应体包含 `{"code":200,...}` 业务数据
- 响应体不包含 `__bifrost_badge__`、`__bb_panel__`、`__bb_copy` 或任何 Badge 注入片段
- 该用例覆盖真实请求 `16473` 暴露的场景：响应头声称 `text/html`，但 body 实际是 JSON 数据接口

---

### TC-BHP-12：回归 - Group 规则启用后 Badge active rules 立即刷新

**操作步骤**：
1. 使用临时数据目录和非 9900 端口启动 Bifrost，带 `--no-system-proxy --enable-badge-injection`。
2. 准备一个 Group 规则 `badge-cache-rule`，初始为 disabled。
3. 通过 Group Rule API 启用该规则：
   ```bash
   curl -X PUT http://127.0.0.1:8800/_bifrost/api/group-rules/{group_id}/badge-cache-rule/enable -s
   ```
4. 不重启 Bifrost，立即通过代理请求一个 HTML 页面：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/html -s -o /tmp/bifrost-badge-group-cache.html
   ```
5. 检查注入脚本内联数据和规则行链接。

**预期结果**：
- HTML 中包含 `__bifrost_badge__` 和 `__bb_panel__`。
- 内联 `rules` 数组包含 `badge-cache-rule`。
- Hover 面板标题 active 数量包含该 Group 规则，不需要重启服务或再创建本地个人规则触发刷新。
- 代理处理链路也使用本地 Group 目录重新加载规则；启用或修改已启用 Group 规则后，不需要等待远端 group 列表刷新才生效。
- Group 规则行链接仍使用可被 Rules 页面识别的 `group` 参数，点击后能定位到对应 Group 规则页面。

**回归目的**：防止 Group 规则 enable/disable 后 Badge 预览缓存或代理处理链路任一侧没有实时刷新；同时保护历史 Group 跳转字段契约。

---

### TC-BHP-13：稳定性 - Group 规则快速启停后 Badge 与 active summary 最终一致

**操作步骤**：
1. 使用临时数据目录和非 9900 端口启动 Bifrost，带 `--no-system-proxy --enable-badge-injection`。
2. 准备 Group 规则 `badge-rapid-toggle-rule`，内容使用唯一 host 和状态码，初始为 disabled。
3. 连续执行 3 轮启用与停用：
   ```bash
   curl -X PUT http://127.0.0.1:8800/_bifrost/api/group-rules/{group_id}/badge-rapid-toggle-rule/enable -s
   curl http://127.0.0.1:8800/_bifrost/api/rules/active-summary -s
   curl -x http://127.0.0.1:8800 http://badge-rapid-toggle.example.test/ -s -o /tmp/bifrost-badge-rapid-toggle-enabled.txt
   curl -X PUT http://127.0.0.1:8800/_bifrost/api/group-rules/{group_id}/badge-rapid-toggle-rule/disable -s
   curl http://127.0.0.1:8800/_bifrost/api/rules/active-summary -s
   ```
4. 每次 enable/disable 后允许最多 2 秒轮询 active summary 和新代理 HTML 中的 Badge 内联数据。

**预期结果**：
- 每次 enable 后，active summary 和 Badge 内联 `rules` 都包含 `badge-rapid-toggle-rule`，`merged_content` 包含该规则内容。
- 每次 disable 后，active summary 和 Badge 内联 `rules` 都不再包含 `badge-rapid-toggle-rule`。
- 即使本地写入、配置通知或页面请求存在短暂延迟，2 秒轮询窗口内必须收敛到一致状态。
- 代理命中结果与 active summary 一致，不出现 Badge 已刷新但代理仍使用旧规则，或代理已变更但 Badge 仍显示旧规则。

**回归目的**：覆盖系统短暂卡顿、本地修改延迟较高、连续快速操作导致的 Badge cache 与 runtime rules 不一致风险。

---

### TC-BHP-14：回归 - Group 远端同步已启用规则后 Badge cache 立即刷新

**操作步骤**：
1. 使用临时数据目录和非 9900 端口启动 Bifrost，带 `--no-system-proxy --enable-badge-injection`，并登录到测试 Sync 服务。
2. 在本地 Group 规则目录中准备 enabled 规则 `badge-sync-cache-rule`，内容为 `badge-sync-cache.example.com status://230`，并先请求一个代理 HTML 页面，确认 Badge 内联 `merged_content` 包含 `status://230`。
3. 在远端同一 Group 下准备同名规则 `badge-sync-cache-rule`，内容为 `badge-sync-cache.example.com status://231`。
4. 打开或刷新管理端 Rules 页面中的该 Group，或直接请求：
   ```bash
   curl http://127.0.0.1:8800/_bifrost/api/group-rules/{group_id} -s
   ```
5. 不手动保存任何规则，立即请求新的代理 HTML 页面并检查 Badge 内联数据：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/html -s -o /tmp/bifrost-badge-group-sync-cache.html
   ```

**预期结果**：
- `/_bifrost/api/group-rules/{group_id}` 返回 200，Rules 页面看到的该规则内容/规则数量与远端同步后的状态一致。
- 新代理 HTML 的 Badge 内联 `rules` 数组仍包含 `badge-sync-cache-rule`，active 数量与 `/_bifrost/api/rules/active-summary` 一致。
- Badge 内联 `merged_content` 包含 `status://231`，不再包含旧的 `status://230`。
- 整个过程不需要通过手动编辑/保存任意规则来触发补刷新。

**回归目的**：覆盖 Group list-sync 修改本地已启用规则但未刷新 `badge_rules_cache` 的问题，防止 Badge active 数量少于管理端 enabled 数量。

---

### TC-BHP-15：回归 - HTTPS 上游连接失败的 502 页面保留 Badge 与操作面板

**操作步骤**：
1. 使用临时数据目录、非正式端口和当前源码构建的二进制启动 Bifrost，禁止修改系统代理：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-human-error-page \
     BIFROST_DISABLE_TRAY=1 \
     BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
     target/debug/bifrost start -p 18894 --skip-cert-check --unsafe-ssl --no-system-proxy --enable-badge-injection
   ```
2. 创建规则，将 HTTPS 测试域名直连到本地未监听端口：
   ```bash
   curl -X POST http://127.0.0.1:18894/_bifrost/api/rules \
     -H 'Content-Type: application/json' \
     -d '{"name":"badge-error-page-human","content":"badge-error-page.test host://127.0.0.1:18895","enabled":true}'
   ```
3. 通过代理请求测试域名并保存真实 502 响应：
   ```bash
   curl -k -x http://127.0.0.1:18894 \
     https://badge-error-page.test/connection-error \
     -D /tmp/bifrost-error-page.headers \
     -o /tmp/bifrost-error-page.html
   ```
4. 使用 Chrome 打开 `/tmp/bifrost-error-page.html`，确认 502 状态卡片在视口中水平、垂直居中，视觉层级清晰；页面展示错误摘要、Status、Request target（`badge-error-page.test:443`）、Upstream target（`127.0.0.1:18895`）、Time、Request URL 和 `What you can do` 分步引导。
5. 确认页面提供 `Open Bifrost rules` 与 `Try again` 操作，并在窄屏尺寸下没有横向溢出；切换浏览器深色模式后文字、边框和错误状态仍清晰可辨。
6. 鼠标悬浮 Badge，确认面板展开并显示 `badge-error-page-human`、Merged Rules 和 Copy；检查规则行链接指向 `http://127.0.0.1:18894/_bifrost/rules?...`。
7. 点击 Copy，确认剪贴板含测试规则且按钮显示 `Copied`；再关闭 Badge 注入后重试同一路径。
8. 保持 Badge 开启，分别以浏览器导航风格 `Accept: text/html,application/xhtml+xml,...` 和通用 `Accept: */*` 重试；再发起一个不带 `Accept` 的请求。

**预期结果**：
- 代理响应状态为 502，`Content-Type` 为 `text/html; charset=utf-8`；居中状态卡片保留 Status、错误类型/消息、Request target、Upstream target、Time、Request URL；请求域名和规则改写后的实际 IP/域名及端口不能混淆，并给出上游服务、规则目标和重试三步引导。
- Rules 与重试入口可用，长 URL 可自动折行，窄屏与深色模式下布局和对比度正常。
- 明确接受 HTML 的请求返回美化 HTML + Badge；只有 `*/*` 或缺失 `Accept` 时返回原始 `text/plain; charset=utf-8`，不会把 HTML 页面强加给 CLI/SDK。
- Chrome 左下角显示与普通代理 HTML 页面相同的 Bifrost Badge；hover 面板可展开，规则列表、Merged Rules、Copy 和规则跳转功能存在。
- 错误页面在亮色与暗色系统主题下都保持可读，Badge 面板沿用现有双主题样式。
- 使用 `--disable-badge-injection` 启动时，同一路径恢复 `text/plain; charset=utf-8`，不包含 `__bifrost_badge__` 或 `__bb_panel__`。

**回归目的**：覆盖连接错误在普通响应注入阶段之前提前返回，导致截图同类 502 页面缺失左下角 Bifrost 操作入口的问题。

---

## TC-BHP-15 执行记录

| 日期 | 执行范围 | 实际结果 | 结论 |
| --- | --- | --- | --- |
| 2026-07-14 | 使用 `target/debug/bifrost`、临时数据目录、18894 代理端口和未监听的 18895 上游端口执行 HTTPS `host://` 连接失败；真实响应保存为 `/tmp/bifrost-error-page.html`，并用 Chrome 打开、hover Badge、展开 Merged Rules、点击 Copy；随后通过 Performance API 关闭 Badge 并复请求同一路径 | 502 响应为 `text/html; charset=utf-8`，保留完整诊断文本；Chrome 左下角显示 Badge，hover 面板显示 `Default` 与 `badge-error-page-human`，规则链接指向 18894，Merged Rules 可展开，Copy 后剪贴板包含 `badge-error-page.test host://127.0.0.1:18895` 且按钮显示 `Copied`；关闭开关后响应恢复 `text/plain; charset=utf-8`，Badge/面板标记均不存在。当前系统亮色主题真实截图可读；暗色契约由错误页 `color-scheme: light dark` 与复用 Badge 的 `prefers-color-scheme: dark` 样式单元断言覆盖 | 通过 |
| 2026-07-14 | 美化 UI 与内容协商追加复测：用当前源码重新构建二进制，在相同隔离端口生成真实 HTTP/HTTPS 502；分别发送浏览器导航风格 `Accept` 与通用 `*/*`，检查响应状态、DOM/CSS、引导与操作入口，再关闭 Badge 复请求；尝试用 Chrome 打开本机测试页 | 浏览器风格 `Accept` 返回 502 HTML，包含居中卡片、错误摘要、Status/Host/Time/Request URL、三步引导、Rules/重试入口、窄屏与深色样式、Badge/面板；`*/*` 返回原始纯文本，无 Badge；关闭 Badge 后即使接受 HTML 也保持纯文本。HTTP/HTTPS 自动 E2E 140/140，临时实例与文件已清理。Chrome 自动化访问本机测试 URL 被浏览器安全策略 `ERR_BLOCKED_BY_CLIENT` 明确阻断，因此本轮新增 UI 的真实 Chrome 视觉、窄屏和深色交互未完成；上一轮 Badge hover/Copy 的真实 Chrome 结果仍有效 | 部分通过（Chrome 环境阻塞） |
| 2026-07-14 | 地址信息增强复测：用当前源码重建 CLI，通过隔离代理分别触发 HTTP 直连和 HTTPS `host://` 改写后的未监听端口错误；检查 HTML 明细与纯文本原始诊断 | HTTP/HTTPS HTML 均明确显示 `Request target`（原请求域名/IP + 端口）和 `Upstream target`（实际连接 IP + 端口）；HTTPS 用例验证请求域名与 `127.0.0.1:<port>` 不再混为 Host；纯文本 `Host` 同样包含实际上游端口。Badge 开关、浏览器 Accept、`*/*` 回退及既有交互回归共 144/144 通过，临时实例已清理 | 通过 |

---

## 清理步骤

```bash
# TC-BHP-15 的隔离实例在启动终端按 Ctrl+C 停止后执行：
rm -rf ./.bifrost-human-error-page
curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-badge-rule -s
curl -X DELETE http://127.0.0.1:18880/_bifrost/api/rules/badge-html-tag-escaping-regression -s
curl -X PUT http://127.0.0.1:8800/_bifrost/api/config/performance \
  -H "Content-Type: application/json" -d '{"inject_bifrost_badge":true}' -s
rm -f /tmp/bifrost-badge-escape-rule.json /tmp/bifrost-badge-escape.html /tmp/bifrost-badge-mislabeled-json.txt /tmp/bifrost-badge-group-cache.html /tmp/bifrost-badge-rapid-toggle-enabled.txt /tmp/bifrost-badge-group-sync-cache.html
rm -f /tmp/bifrost-error-page.headers /tmp/bifrost-error-page.html
```

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

## 清理步骤

```bash
curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-badge-rule -s
curl -X DELETE http://127.0.0.1:18880/_bifrost/api/rules/badge-html-tag-escaping-regression -s
curl -X PUT http://127.0.0.1:8800/_bifrost/api/config/performance \
  -H "Content-Type: application/json" -d '{"inject_bifrost_badge":true}' -s
rm -f /tmp/bifrost-badge-escape-rule.json /tmp/bifrost-badge-escape.html /tmp/bifrost-badge-mislabeled-json.txt
```

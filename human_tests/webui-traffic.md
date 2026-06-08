# Web UI Traffic 页面测试用例

## 功能模块说明

Bifrost Web UI 的 Traffic 页面是核心功能页面，用于实时展示和分析通过代理的所有 HTTP/HTTPS/WebSocket/SSE 流量。主要功能包括：

- 流量列表表格（虚拟滚动，支持大量记录）
- 流量筛选与过滤
- 流量详情面板（Overview、Header、Query、Cookie、Body、Raw、Messages、Script 等 Tab）
- 右键上下文菜单（复制 URL、复制 cURL、下载 HAR、Replay、导出等）
- 清空流量
- 搜索功能

## 前置条件

1. 启动 Bifrost 服务（使用临时数据目录避免污染正式环境）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-system-proxy
   ```
2. 生成测试流量（在另一个终端执行）：
   ```bash
   # 普通 GET 请求
   curl -x http://127.0.0.1:8800 http://httpbin.org/get

   # 带 JSON 响应的请求
   curl -x http://127.0.0.1:8800 http://httpbin.org/json

   # POST 请求带 JSON Body
   curl -x http://127.0.0.1:8800 -X POST http://httpbin.org/post \
     -H "Content-Type: application/json" \
     -d '{"name":"bifrost","version":"1.0"}'

   # 带 Query 参数的请求
   curl -x http://127.0.0.1:8800 "http://httpbin.org/get?foo=bar&lang=zh"

   # 带 Cookie 的请求
   curl -x http://127.0.0.1:8800 -b "session=abc123;token=xyz" http://httpbin.org/cookies

   # 不同状态码的请求
   curl -x http://127.0.0.1:8800 http://httpbin.org/status/404
   curl -x http://127.0.0.1:8800 http://httpbin.org/status/500
   curl -x http://127.0.0.1:8800 http://httpbin.org/status/301

   # 不同 HTTP 方法
   curl -x http://127.0.0.1:8800 -X PUT http://httpbin.org/put -d "data=test"
   curl -x http://127.0.0.1:8800 -X DELETE http://httpbin.org/delete
   curl -x http://127.0.0.1:8800 -X PATCH http://httpbin.org/patch -d "data=patch"

   # 生成较大响应体
   curl -x http://127.0.0.1:8800 http://httpbin.org/bytes/10240

   # HTML 响应
   curl -x http://127.0.0.1:8800 http://httpbin.org/html
   ```
3. 确保浏览器可访问 `http://127.0.0.1:8800/_bifrost/traffic`

---

## 测试用例

### TC-WTR-01：访问 Traffic 页面

**操作步骤**：
1. 在浏览器中打开 `http://127.0.0.1:8800/_bifrost/traffic`

**预期结果**：
- 页面正常加载，显示 Traffic 页面
- 左侧导航栏高亮 "Traffic" 菜单项
- 页面上方显示工具栏（含清空按钮、过滤按钮等）
- 页面主体显示流量表格

---

### TC-WTR-02：流量表格列显示

**前置条件**：已通过前置条件生成测试流量

**操作步骤**：
1. 在浏览器中打开 `http://127.0.0.1:8800/_bifrost/traffic`
2. 观察流量表格的表头列

**预期结果**：
- 表格包含以下列（从左到右）：`#`（序号）、状态圆点、`Protocol`、`Method`、`Status`、`Client`、`Host`、`Path`、`Type`、`Size`、`Time`、`Start Time`、`End Time`、`Rules`
- `#` 列显示 5 位序号（如 `00001`）
- 状态圆点列显示彩色圆点，代表请求状态
- `Protocol` 列显示协议标签（如 `http`、`https`）
- `Method` 列显示彩色 HTTP 方法标签（如 `GET`、`POST`）
- `Status` 列显示彩色状态码标签（如绿色 `200`、橙色 `404`、红色 `500`）
- `Host` 列显示请求目标主机名
- `Path` 列显示请求路径
- `Type` 列显示响应内容类型简写
- `Size` 列显示响应大小
- `Time` 列显示请求耗时
- `Start Time` 列显示请求开始时间（等宽字体）
- `End Time` 列显示请求结束时间
- `Rules` 列显示命中规则数（无命中时显示 `-`）

---

### TC-WTR-03：点击流量记录打开详情面板

**操作步骤**：
1. 在流量表格中点击任意一条 GET 请求记录

**预期结果**：
- 页面右侧（或下方）显示详情面板
- 详情面板顶部显示请求 URL、Method 标签和状态码
- 详情面板分为上下两个区域：Request（请求）和 Response（响应）
- 两个区域之间有可拖拽的分隔条

---

### TC-WTR-04：详情面板 Request 区域 - Overview Tab

**前置条件**：已点击一条流量记录打开详情面板

**操作步骤**：
1. 在 Request 区域点击 "Overview" Tab

**预期结果**：
- 显示请求概览信息
- 包含 General 区域，展示 URL、Method、Status、Protocol 等基本信息
- 包含 Timing 区域（如果有 timing 数据），展示时间分布条形图：DNS lookup、Connection established、TLS handshake、Request sent、Waiting (TTFB)、Content download
- Timing 表格中每项显示毫秒数
- 包含 Total 耗时

---

### TC-WTR-05：详情面板 Request 区域 - Header Tab

**操作步骤**：
1. 在 Request 区域点击 "Header" Tab

**预期结果**：
- 显示请求头列表，以键值对形式展示
- 包含常见请求头如 `Host`、`User-Agent`、`Accept` 等
- 头部名称和值清晰可读

---

### TC-WTR-06：详情面板 Request 区域 - Query Tab

**前置条件**：已通过 `curl -x http://127.0.0.1:8800 "http://httpbin.org/get?foo=bar&lang=zh"` 生成带 Query 参数的请求

**操作步骤**：
1. 点击该带 Query 参数的请求记录
2. 在 Request 区域点击 "Query" Tab

**预期结果**：
- Query Tab 可见（仅当 URL 含查询参数时显示）
- 以键值对形式展示查询参数：`foo = bar`、`lang = zh`

---

### TC-WTR-07：详情面板 Request 区域 - Cookie Tab

**前置条件**：已通过 `curl -x http://127.0.0.1:8800 -b "session=abc123;token=xyz" http://httpbin.org/cookies` 生成带 Cookie 的请求

**操作步骤**：
1. 点击该带 Cookie 的请求记录
2. 在 Request 区域点击 "Cookie" Tab

**预期结果**：
- Cookie Tab 可见（仅当请求头包含 Cookie 时显示）
- 以键值对形式展示 Cookie：`session = abc123`、`token = xyz`

---

### TC-WTR-08：详情面板 Request 区域 - Body Tab

**前置条件**：已通过 POST 请求生成带 Body 的流量

**操作步骤**：
1. 点击该 POST 请求记录
2. 在 Request 区域点击 "Body" Tab

**预期结果**：
- Body Tab 可见（仅当请求有 Body 时显示）
- 显示请求体内容 `{"name":"bifrost","version":"1.0"}`
- JSON 内容有语法高亮

---

### TC-WTR-09：详情面板 Request 区域 - Raw Tab

**操作步骤**：
1. 在 Request 区域点击 "Raw" Tab

**预期结果**：
- 显示原始 HTTP 请求文本，包含请求行（如 `GET /get HTTP/1.1`）和所有请求头
- 如果有请求体，也一并显示在头部之后

---

### TC-WTR-10：详情面板 Response 区域 - Header Tab

**操作步骤**：
1. 在 Response 区域点击 "Header" Tab

**预期结果**：
- 显示响应头列表，以键值对形式展示
- 包含常见响应头如 `Content-Type`、`Content-Length`、`Server` 等

---

### TC-WTR-11：详情面板 Response 区域 - Body Tab

**前置条件**：已点击一条返回 JSON 的请求（如 `http://httpbin.org/json`）

**操作步骤**：
1. 在 Response 区域点击 "Body" Tab

**预期结果**：
- Body Tab 可见（仅当响应有 Body 时显示）
- 显示响应体内容
- JSON 内容有语法高亮

---

### TC-WTR-12：详情面板 Response 区域 - Set-Cookie Tab

**前置条件**：请求的响应中包含 `Set-Cookie` 头

**操作步骤**：
1. 点击该请求记录
2. 在 Response 区域查看是否有 "Set-Cookie" Tab

**预期结果**：
- 当响应头包含 `Set-Cookie` 时，显示 "Set-Cookie" Tab
- 点击后以结构化形式展示 Set-Cookie 内容

---

### TC-WTR-13：详情面板 Response 区域 - Raw Tab

**操作步骤**：
1. 在 Response 区域点击 "Raw" Tab

**预期结果**：
- 显示原始 HTTP 响应文本，包含状态行（如 `HTTP/1.1 200 OK`）和所有响应头
- 如果有响应体，也一并显示在头部之后

---

### TC-WTR-14：详情面板 Response 区域 - Messages Tab（WebSocket 流量）

**前置条件**：生成 WebSocket 流量（需要 WebSocket 服务端支持）

**操作步骤**：
1. 在流量表格中找到 WebSocket 类型的流量记录
2. 点击该记录
3. 在 Response 区域查看 "Messages" Tab

**预期结果**：
- Messages Tab 可见，标签显示消息计数，如 `Messages (5)`
- 消息列表展示发送（Send）和接收（Receive）方向的消息帧
- 每条消息显示帧类型（Text / Binary）、方向标识、内容和时间戳

---

### TC-WTR-15：详情面板 Response 区域 - Script Tab

**前置条件**：请求经过了脚本处理（配置了 req-script 或 res-script 规则）

**操作步骤**：
1. 点击有脚本执行记录的请求
2. 在 Request 区域或 Response 区域查看 "Script" Tab

**预期结果**：
- Script Tab 可见（仅当有脚本执行结果时显示）
- 显示脚本执行日志和结果

---

### TC-WTR-16：Overview 显示 Timing 信息

**操作步骤**：
1. 点击一条已完成的 HTTP 请求记录
2. 在 Request 区域的 "Overview" Tab 查看 Timing 区域

**预期结果**：
- 显示 Timing 条形图，各阶段用不同颜色区分：
  - DNS lookup（紫色）
  - Connection established（绿色）
  - TLS handshake（黄色，仅 HTTPS）
  - Request sent（橙色）
  - Waiting (TTFB)（蓝色）
  - Content download（青色）
- 条形图下方表格列出每个阶段的毫秒数
- 最后一行显示 Total 总耗时

---

### TC-WTR-17：Overview 显示命中规则信息

**前置条件**：配置一条规则（如 `httpbin.org status://201`）并发起匹配请求

**操作步骤**：
1. 通过代理访问匹配规则的 URL
2. 在流量表格中点击该请求记录
3. 在 Request 区域的 "Overview" Tab 查看

**预期结果**：
- Overview 中显示命中的规则信息
- 每条命中规则显示：匹配模式（Pattern）、协议（Protocol）、目标值（Value）
- 如果规则有名称，也一并显示

---

### TC-WTR-18：Body 视图 JSON Pretty Print

**前置条件**：已点击一条返回 JSON 的请求

**操作步骤**：
1. 在 Response 区域点击 "Body" Tab
2. 观察 Body 视图的显示格式下拉菜单

**预期结果**：
- 默认以 JSON 高亮模式显示，JSON 内容格式化缩进
- 格式下拉菜单显示 "JSON"
- JSON 的键、字符串值、数字值、布尔值等分别以不同颜色高亮
- 可切换到 "Tree" 模式，以树形结构展示 JSON 对象

---

### TC-WTR-19：Body 视图 Hex 模式

**操作步骤**：
1. 在 Body Tab 中，点击格式下拉菜单
2. 选择 "Hex"

**预期结果**：
- Body 内容以十六进制视图显示
- 左侧显示偏移地址
- 中间显示十六进制字节值
- 右侧显示对应的 ASCII 可打印字符（不可打印字符显示为 `.`）

---

### TC-WTR-20：流量表格按 Method 筛选

**操作步骤**：
1. 通过 URL 参数访问：`http://127.0.0.1:8800/_bifrost/traffic` 页面
2. 在过滤面板中选择 Method 为 `POST`，或通过 API 验证：
   ```bash
   curl "http://127.0.0.1:8800/_bifrost/api/traffic?method=POST"
   ```

**预期结果**：
- 仅显示 HTTP 方法为 POST 的请求
- 其他方法（GET、PUT、DELETE 等）的请求不显示
- API 返回的 `records` 数组中所有记录的 `m` 字段值为 `POST`

---

### TC-WTR-21：流量表格按状态码筛选

**操作步骤**：
1. 通过 API 验证按状态码筛选：
   ```bash
   curl "http://127.0.0.1:8800/_bifrost/api/traffic?status=404"
   ```
2. 通过状态码范围筛选：
   ```bash
   curl "http://127.0.0.1:8800/_bifrost/api/traffic?status_min=400&status_max=499"
   ```

**预期结果**：
- 精确筛选：仅返回状态码为 404 的请求
- 范围筛选：仅返回状态码在 400-499 之间的请求（如 404）
- 不匹配的状态码不出现在结果中

---

### TC-WTR-22：流量表格按 Host 筛选

**操作步骤**：
1. 通过 API 验证按 Host 筛选：
   ```bash
   curl "http://127.0.0.1:8800/_bifrost/api/traffic?host=httpbin"
   ```

**预期结果**：
- 仅返回 Host 包含 "httpbin" 的请求
- 匹配方式为模糊匹配（LIKE %keyword%）

---

### TC-WTR-23：流量表格按 Content-Type 筛选

**操作步骤**：
1. 通过 API 验证按 Content-Type 筛选：
   ```bash
   curl "http://127.0.0.1:8800/_bifrost/api/traffic?content_type=json"
   ```

**预期结果**：
- 仅返回响应 Content-Type 包含 "json" 的请求
- 如 `application/json` 类型的响应会被匹配

---

### TC-WTR-23B：主筛选器按代理端口筛选

**操作步骤**：
1. 使用隔离数据目录启动 Bifrost，端口为 `$MAIN_PORT`，启动参数必须包含 `--no-system-proxy`。
2. 通过 `$MAIN_PORT` 代理发起请求 `/port-filter-main`，再准备另一条 `listener_port` 不同的 traffic 记录 `/port-filter-other`。
3. 打开 Traffic 页面：
   ```text
   http://127.0.0.1:$MAIN_PORT/_bifrost/traffic
   ```
4. 点击主筛选器 `Add Filter`，在字段下拉中选择 `Port`。
5. 确认操作符自动变为 `Equals`，输入 `$MAIN_PORT`。
6. 使用 API 验证服务端筛选：
   ```bash
   curl "http://127.0.0.1:$MAIN_PORT/_bifrost/api/traffic?listener_port=$MAIN_PORT&limit=50"
   ```
7. 切换到 Fuzzy Search，使用 condition `listener_port equals $MAIN_PORT` 验证搜索筛选。

**预期结果**：
- 主筛选器字段下拉包含 `Port`。
- 选择 `Port` 后，输入框提示为代理端口，操作符默认为 `Equals`。
- 表格只展示 `listener_port=$MAIN_PORT` 的记录，隐藏其他端口记录。
- API 返回 records 中每条记录的 `lp` 都等于 `$MAIN_PORT`。
- Fuzzy Search 使用相同 port condition 时只返回对应端口记录。

---

### TC-WTR-23C：主筛选器临时停用单条筛选条件

**操作步骤**：
1. 使用隔离数据目录启动 Bifrost，端口为 `$MAIN_PORT`，启动参数必须包含 `--no-system-proxy`，禁止使用 `9900`。
2. 通过 `$MAIN_PORT` 代理发起两条请求：
   ```bash
   curl -x http://127.0.0.1:$MAIN_PORT http://127.0.0.1:$MOCK_PORT/filter-enabled-target
   curl -x http://127.0.0.1:$MAIN_PORT http://127.0.0.1:$MOCK_PORT/filter-enabled-other
   ```
3. 打开 Traffic 页面：
   ```text
   http://127.0.0.1:$MAIN_PORT/_bifrost/traffic
   ```
4. 点击主筛选器 `Add Filter`，确认新增筛选条件行最左侧 checkbox 默认处于选中状态。
5. 在字段下拉中选择 `Path`，操作符保持 `Contains`，输入 `/filter-enabled-target`。
6. 取消勾选该筛选条件行最左侧 checkbox，不删除筛选条件。
7. 再次勾选该 checkbox。

**预期结果**：
- 新增筛选条件默认启用，checkbox 默认选中。
- 输入 `/filter-enabled-target` 且 checkbox 选中时，表格只展示 target 流量，隐藏 other 流量。
- 取消勾选 checkbox 后，该筛选条件仍保留在页面上，但不再参与过滤，target 和 other 流量都可见。
- 重新勾选 checkbox 后，该条件再次生效，表格重新只展示 target 流量。

---

### TC-WTR-24：右键上下文菜单 - Copy URL

**操作步骤**：
1. 在流量表格中右键点击一条请求记录
2. 在弹出的上下文菜单中点击 "Copy URL"

**预期结果**：
- 显示上下文菜单，包含 "Copy URL" 选项
- 点击后 URL 被复制到剪贴板
- 显示 Toast 消息 "URL copied to clipboard"
- 菜单自动关闭

---

### TC-WTR-25：右键上下文菜单 - Copy as cURL

**操作步骤**：
1. 在流量表格中右键点击一条请求记录
2. 在弹出的上下文菜单中点击 "Copy as cURL"

**预期结果**：
- 点击后生成 cURL 命令并复制到剪贴板
- cURL 命令包含请求方法、URL、请求头和请求体（如有）
- 显示 Toast 消息 "cURL command copied to clipboard"

---

### TC-WTR-26：右键上下文菜单 - Replay

**操作步骤**：
1. 在流量表格中右键点击一条非 CONNECT（非 Tunnel）请求记录
2. 在弹出的上下文菜单中点击 "Replay"

**预期结果**：
- 菜单中显示 "Replay" 选项（仅对非 Tunnel 请求显示）
- 点击后页面跳转到 `/replay` 页面
- Replay 页面自动填充该请求的 URL、方法、请求头等信息

---

### TC-WTR-27：右键上下文菜单 - Download as HAR

**操作步骤**：
1. 在流量表格中右键点击一条请求记录
2. 在弹出的上下文菜单中点击 "Download as HAR"

**预期结果**：
- 显示 loading 提示 "Generating HAR file..."
- 浏览器下载一个 .har 文件
- 显示 Toast 消息 "Downloaded 1 request(s) as HAR"
- HAR 文件内容符合 HAR 1.2 规范

---

### TC-WTR-28：右键上下文菜单 - Export as .bifrost

**操作步骤**：
1. 在流量表格中右键点击一条请求记录
2. 在弹出的上下文菜单中点击 "Export as .bifrost"

**预期结果**：
- 浏览器下载一个 .bifrost 文件
- 文件包含该请求的完整信息

---

### TC-WTR-29：清空所有流量

**操作步骤**：
1. 确认当前流量表格中有流量记录
2. 点击工具栏中的清空按钮（垃圾桶图标）
3. 通过 API 验证：
   ```bash
   curl -X DELETE "http://127.0.0.1:8800/_bifrost/api/traffic"
   ```

**预期结果**：
- 流量表格中的所有记录被清空（活跃连接除外）
- API 返回 "All traffic data cleared successfully"
- 清空后表格显示为空

---

### TC-WTR-30：过滤面板显示与交互

**操作步骤**：
1. 在工具栏中点击过滤按钮打开过滤面板
2. 观察过滤面板的可用选项

**预期结果**：
- 过滤面板显示在流量表格上方或侧边
- 提供以下过滤维度：
  - Method（GET、POST、PUT、DELETE 等）
  - Status（状态码范围或精确值）
  - Protocol（http、https 等）
  - Host（模糊搜索）
  - Content-Type
  - 特殊类型：WebSocket、SSE、H3、Tunnel、Has Rule Hit
  - Client App / Client IP
- 选择过滤条件后，流量表格实时更新，仅显示匹配的记录
- 过滤条件可以组合使用（AND 逻辑）

---

### TC-WTR-31：固定过滤器（Pinned Filters）

**操作步骤**：
1. 在过滤面板中设置一个过滤条件（如 Method = GET）
2. 将该过滤条件固定（Pin）
3. 切换到其他页面后再回到 Traffic 页面

**预期结果**：
- 固定的过滤条件在页面切换后仍然保留
- 流量表格仍按照固定的过滤条件显示
- 工具栏或过滤面板中显示已固定的过滤器标识

---

### TC-WTR-32：虚拟滚动 - 大量记录

**前置条件**：生成大量流量（至少 200 条以上）：
```bash
for i in $(seq 1 200); do
  curl -s -x http://127.0.0.1:8800 http://httpbin.org/get > /dev/null &
done
wait
```

**操作步骤**：
1. 打开 Traffic 页面
2. 快速滚动流量表格到底部
3. 再快速滚动回顶部

**预期结果**：
- 表格采用虚拟滚动，仅渲染可视区域内的行
- 快速滚动时页面不卡顿，滚动流畅
- 滚动到底部后如有更多记录，自动加载更多
- 所有记录按序号有序排列

---

### TC-WTR-33：WebSocket 流量显示帧计数

**前置条件**：生成 WebSocket 流量

**操作步骤**：
1. 在流量表格中找到 WebSocket 类型的记录
2. 观察该记录的显示

**预期结果**：
- WebSocket 记录的 Method 列显示 `GET`（升级请求的方法）
- 流量表格行中显示帧计数信息
- `data-frame-count` 属性包含当前帧数量
- 点击该记录后，Response 区域的 Messages Tab 标签显示帧数，如 `Messages (10)`

---

### TC-WTR-34：SSE 流量显示事件计数

**前置条件**：生成 SSE 流量（需要 SSE 服务端支持，或使用支持 SSE 的 API）

**操作步骤**：
1. 在流量表格中找到 SSE 类型的记录
2. 点击该记录

**预期结果**：
- SSE 记录在表格中正常显示
- 点击后 Response 区域的 Messages Tab 标签显示事件计数，如 `Messages (15)`
- Messages Tab 中按时间顺序展示 SSE 事件
- 每个事件显示 event 类型、data 内容

---

### TC-WTR-35：详情面板搜索功能

**操作步骤**：
1. 点击一条流量记录打开详情面板
2. 在 Request 区域的搜索框中输入关键词（如某个请求头的名称）

**预期结果**：
- 搜索框位于面板内 Tab 区域
- 输入关键词后，当前 Tab 内容中匹配的文本被高亮显示
- 搜索支持在 Overview、Header、Body、Raw 等多个 Tab 中使用

---

### TC-WTR-36：Body 文本选择

**操作步骤**：
1. 点击一条返回 JSON 的请求记录
2. 在 Response 区域的 Body Tab 中，用鼠标拖选一段文本

**预期结果**：
- 文本可以正常选中，选中区域有高亮背景
- 可以通过 Ctrl+C / Cmd+C 复制选中的文本
- 粘贴后内容与选中内容一致

---

### TC-WTR-37：流量表格 Rules 列徽章显示

**前置条件**：已配置规则并生成命中规则的流量

**操作步骤**：
1. 在流量表格中找到命中规则的请求记录
2. 观察 Rules 列

**预期结果**：
- 命中规则的记录在 Rules 列显示蓝色闪电图标
- 图标旁边有蓝色数字徽章，显示命中规则数
- 鼠标悬停闪电图标时，Tooltip 显示 "X rule(s) matched" 和命中的协议列表
- 未命中规则的记录 Rules 列显示 "-"

---

### TC-WTR-38：右键上下文菜单 - 多选批量导出

**操作步骤**：
1. 在流量表格中按住 Ctrl（Mac 为 Cmd）或 Shift 键多选多条记录
2. 右键点击其中一条选中的记录

**预期结果**：
- 上下文菜单仅显示批量操作选项："Export X requests as .bifrost"
- 菜单中不显示单条记录的操作（Copy URL、Copy as cURL、Replay 等）
- 点击导出后，生成包含所有选中记录的 .bifrost 文件

---

### TC-WTR-48：Network 空 .bifrost 包导入必须明确失败

**操作步骤**：
1. 启动 Bifrost 服务（必须使用临时数据目录和 `--no-system-proxy`）：
   ```bash
   BIFROST_DATA_DIR=/tmp/bifrost-network-import-empty cargo run --bin bifrost -- start -p 18892 --unsafe-ssl --no-system-proxy
   ```
2. 使用空 Network 包调用导入 API：
   ```bash
   curl -sS -i -X POST \
     -H 'Content-Type: text/plain' \
     --data-binary @e2e-tests/test_data/bifrost-file/network-empty.bifrost \
     http://127.0.0.1:18892/_bifrost/api/bifrost-file/import
   ```
3. 打开 `http://127.0.0.1:18892/_bifrost/traffic`，将 `e2e-tests/test_data/bifrost-file/network-empty.bifrost` 拖到页面中。

**预期结果**：
- API 返回 HTTP `400`。
- 响应体包含 `Network file contains 0 records; nothing to import`。
- WebUI 不显示 `Imported ... successfully`。
- WebUI 不自动套用 Imported 筛选并制造“导入成功但列表为空”的错觉。

---

### TC-WTR-49：Network 导出不能生成 0 条记录的 .bifrost 包

**操作步骤**：
1. 启动 Bifrost 服务（必须使用临时数据目录和 `--no-system-proxy`）：
   ```bash
   BIFROST_DATA_DIR=/tmp/bifrost-network-import-empty cargo run --bin bifrost -- start -p 18892 --unsafe-ssl --no-system-proxy
   ```
2. 直接调用 Network 导出 API，传入空选中列表：
   ```bash
   curl -sS -i -X POST \
     -H 'Content-Type: application/json' \
     -d '{"record_ids":[],"include_body":true}' \
     http://127.0.0.1:18892/_bifrost/api/bifrost-file/export/network
   ```
3. 在 WebUI 导出公共入口层验证空选中列表：
   ```bash
   pnpm --dir web test:unit src/api/bifrost-file.test.ts
   ```
4. 再调用 Network 导出 API，传入不存在的 Traffic ID：
   ```bash
   curl -sS -i -X POST \
     -H 'Content-Type: application/json' \
     -d '{"record_ids":["REQ-NOT-EXIST"],"include_body":true}' \
     http://127.0.0.1:18892/_bifrost/api/bifrost-file/export/network
   ```

**预期结果**：
- 空选中列表返回 HTTP `400`，响应体包含 `Select at least one Network record`。
- WebUI 导出公共入口在 `record_ids: []` 时返回同一条用户提示，不调用导出下载流程。
- 不存在 ID 返回 HTTP `400`，响应体包含 `selected record(s) no longer exist`。
- 两种情况都不会下载或生成 `count = 0`、正文为 `[]` 的 `.bifrost` 文件。

---

### TC-WTR-39：详情面板 Request/Response 区域折叠

**操作步骤**：
1. 点击一条流量记录打开详情面板
2. 点击 Request 区域的折叠按钮
3. 再点击展开按钮恢复

**预期结果**：
- 点击 Request 区域折叠按钮后，Request 区域缩小为标题栏高度
- Response 区域自动占满剩余空间
- 再次点击展开按钮后，Request 区域恢复原始大小
- 同理，Response 区域也可独立折叠和展开
- Request 和 Response 不能同时折叠

---

### TC-WTR-40：右键上下文菜单 - TLS 拦截操作

**前置条件**：流量表格中有 CONNECT（Tunnel）类型的请求

**操作步骤**：
1. 在流量表格中右键点击一条 Tunnel 请求记录

**预期结果**：
- 上下文菜单中显示 TLS 拦截相关选项：
  - "Intercept {域名}" —— 将域名加入 TLS 拦截列表
  - "Intercept {应用名}" —— 将客户端应用加入拦截列表（如有 client_app）
  - "Intercept IP {IP}" —— 将客户端 IP 加入拦截列表（如有 client_ip）
- 对于 Tunnel 请求，不显示 "Replay" 选项
- 如果域名已在拦截列表中，则不显示对应的拦截选项

---

### TC-WTR-41：流量表格自动滚动到底部

**操作步骤**：
1. 打开 Traffic 页面，确保有持续的流量产生
2. 滚动表格到底部
3. 等待新的流量记录产生

**预期结果**：
- 当表格滚动位置在底部时，新记录产生后表格自动滚动以显示最新记录
- 当用户手动向上滚动离开底部后，不再自动滚动
- 有新记录未显示时，页面提示新记录数量，可点击滚动到底部

---

### TC-WTR-42：流量详情中请求序号跳转搜索

**操作步骤**：
1. 点击一条流量记录打开详情面板
2. 在详情面板顶部找到序号区域
3. 点击序号区域触发搜索
4. 输入目标序号（如 "5"）

**预期结果**：
- 显示序号搜索输入框
- 输入数字后，下拉列表显示匹配的请求记录（序号包含输入数字的记录）
- 每个选项显示 `#序号`、Method、Status、Host、Path
- 选中后跳转到对应的请求详情

---

### TC-WTR-43：双击流量记录在新窗口打开

**操作步骤**：
1. 在流量表格中双击一条请求记录

**预期结果**：
- 该请求的详情在新窗口/新标签页中打开
- 新窗口完整显示该请求的详情（Header、Body 等所有 Tab）

---

### TC-WTR-44：流量按协议类型筛选

**操作步骤**：
1. 通过 API 验证按协议筛选：
   ```bash
   curl "http://127.0.0.1:8800/_bifrost/api/traffic?protocol=http"
   ```
2. 验证 WebSocket 筛选：
   ```bash
   curl "http://127.0.0.1:8800/_bifrost/api/traffic?is_websocket=true"
   ```
3. 验证 SSE 筛选：
   ```bash
   curl "http://127.0.0.1:8800/_bifrost/api/traffic?is_sse=true"
   ```
4. 验证 Tunnel 筛选：
   ```bash
   curl "http://127.0.0.1:8800/_bifrost/api/traffic?is_tunnel=true"
   ```

**预期结果**：
- 各筛选条件正确过滤结果
- `protocol=http` 仅返回 HTTP 协议的请求
- `is_websocket=true` 仅返回 WebSocket 类型的请求
- `is_sse=true` 仅返回 SSE 类型的请求
- `is_tunnel=true` 仅返回 CONNECT Tunnel 类型的请求

---

### TC-WTR-45：流量全局搜索功能

**操作步骤**：
1. 在 Traffic 页面工具栏中找到搜索入口
2. 输入搜索关键词（如 "httpbin"）
3. 执行搜索

**预期结果**：
- 搜索引擎在所有流量记录中进行全文检索
- 搜索范围包括 URL、请求头、响应头、请求体、响应体
- 搜索结果高亮匹配的关键词
- 搜索支持通过 SSE 流式返回结果（实时显示匹配进度）

---

### TC-WTR-46：CONNECT Response 空状态按 Client IP 开启设备能力

**前置条件**：
1. 使用隔离数据目录启动最新 Bifrost，必须禁用系统代理和 Sync 自动登录弹窗：
   ```bash
   BIFROST_DATA_DIR="$(mktemp -d /tmp/bifrost-webui-client-ip.XXXXXX)" \
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
   cargo run --bin bifrost -- start --host 127.0.0.1 -p 18892 --unsafe-ssl --no-system-proxy --access-mode allow_all
   ```
2. 打开 `http://127.0.0.1:18892/_bifrost/traffic`。
3. 准备一条 CONNECT Tunnel 流量，详情记录中包含 host、client_app 和 client_ip。

**操作步骤**：
1. 在 Traffic 表格中点击一条 CONNECT Tunnel 请求。
2. 在 Response 区域打开 Header 或 Raw Tab。
3. 确认空状态展示以下按钮：
   - `Intercept this domain`
   - `Intercept this app`（当记录包含 client_app）
   - `Intercept this client`（当记录包含 client_ip）
   - `Allow this client`（当 client_ip 不是 `127.0.0.0/8` 或 `::1` 等本机 loopback）
4. 点击 `Intercept this client` 并确认弹窗。
5. 查询 `GET /_bifrost/api/config/tls`。
6. 点击 `Allow this client` 并确认弹窗。
7. 查询 `GET /_bifrost/api/whitelist`。

**预期结果**：
- `Intercept this client` 成功后，TLS 配置的 `ip_intercept_include` 包含该 `client_ip`。
- 成功提示包含 `Restart the target app and reopen the target domain to establish a new connection.`。
- `Allow this client` 只在非本机 Client IP 下出现；成功后访问控制白名单 `whitelist` 包含该 `client_ip` 或等价的单 IP 网段（如 IPv4 `/32`、IPv6 `/128`）。
- 本机 loopback Client IP 不显示 `Allow this client`，避免把本机设备误当远端设备。
- 域名、应用两个既有按钮仍可见且行为不变。

**执行记录（2026-06-08）**：
- 已执行命令：`source ~/.zshrc && pnpm --dir web test:ui traffic-push.spec.ts -g "CONNECT 详情的 Response 面板可按应用和非本机 Client IP 开启"`
- 第一次执行结果：失败。原因是测试断言只接受裸 IP，但后端访问控制白名单以 `IpNet` 输出单 IP 为 `192.168.50.24/32`；功能调用成功，断言和前端等价判定已修正。
- 复跑结果：PASS，后续修复提示边界和清理边界后最终复跑 `1 passed (30.9s)`。
- 实际验证：Playwright 打开真实 Traffic 页面，生成 CONNECT Tunnel 流量，将详情里的 `client_ip` 模拟为非本机 `192.168.50.24`；Response 空状态显示 `Intercept this app`、`Intercept this client`、`Allow this client`；点击 `Intercept this client` 后 `/_bifrost/api/config/tls` 的 `ip_intercept_include` 包含该 IP；点击 `Allow this client` 后 `/_bifrost/api/whitelist` 的 `whitelist` 包含等价单 IP 网段 `192.168.50.24/32`。

---

### TC-WTR-47：高并发流量下 Traffic、SSE 详情和 appinfo 仍可响应

**背景**：用户反馈管理端经常一直 loading，打开 SSE 请求详情页后结果不出来。日志中大量 CONNECT 请求触发客户端进程解析；该用例验证极限 CONNECT/HTTP 压力下，管理端 Traffic 列表、请求详情和 SSE frames 接口不会被明显阻塞，同时验证高并发短请求不会大量丢失客户端应用识别。客户端进程解析在极端情况下最多等待 2 秒，超过后应按未知客户端降级，不能继续阻塞请求链路；近期解析 miss/timeout 应命中 negative cache 快速跳过，进程解析 blocking 任务也必须受全局并发阀门限制，socket 快照刷新必须 singleflight，普通 HTTP 请求也应在请求开始时执行受限同步解析，`/_bifrost` 管理端接口应完全跳过进程识别。普通 HTTP 响应不得抢占 `ConnectionMonitor` 全局写锁；当 `BodyStore` 忙时允许后台最终一致保存响应体，但不能丢失 Traffic 记录或永久丢失 body。

**前置条件**：
1. 使用隔离数据目录和非 9900 端口启动最新 Bifrost，必须带 `--no-system-proxy`：
   ```bash
   export BIFROST_TEST_DIR="$(mktemp -d /tmp/bifrost-webui-traffic-perf.XXXXXX)"
   export BIFROST_TEST_PORT=18880
   BIFROST_DATA_DIR="$BIFROST_TEST_DIR" cargo run --bin bifrost -- start -p "$BIFROST_TEST_PORT" --unsafe-ssl --no-system-proxy
   ```
2. 打开管理端页面：
   ```text
   http://127.0.0.1:18880/_bifrost/traffic
   ```

**操作步骤**：
1. 启动本地 SSE mock 服务：
   ```bash
   python3 - <<'PY'
   from http.server import BaseHTTPRequestHandler, HTTPServer
   import time

   class Handler(BaseHTTPRequestHandler):
       def do_GET(self):
           if not self.path.startswith("/sse"):
               self.send_response(404)
               self.end_headers()
               return
           self.send_response(200)
           self.send_header("Content-Type", "text/event-stream")
           self.end_headers()
           for index in range(3):
               self.wfile.write(f"id: {index}\ndata: bifrost-event-{index}\n\n".encode())
               self.wfile.flush()
               time.sleep(0.05)

       def log_message(self, *_args):
           pass

   HTTPServer(("127.0.0.1", 18981), Handler).serve_forever()
   PY
   ```
2. 通过 Bifrost 代理发起一个 SSE 请求，并等待 Traffic 中出现 SSE 记录：
   ```bash
   NO_PROXY="" no_proxy="" curl -sS --max-time 8 -x "http://127.0.0.1:${BIFROST_TEST_PORT}" "http://127.0.0.1:18981/sse?count=3" -o /tmp/bifrost-sse.out
   curl -fsS "http://127.0.0.1:${BIFROST_TEST_PORT}/_bifrost/api/traffic?limit=50&is_sse=true"
   ```
3. 在另一个终端发起高并发 CONNECT 压力：
   ```bash
   seq 1 160 | xargs -n1 -P40 -I{} sh -c 'curl -ksS --max-time 5 -x "http://127.0.0.1:${BIFROST_TEST_PORT}" "https://example.com/?connect_pressure={}" -o /dev/null || true'
   ```
4. 压力运行期间或刚结束后，验证管理端列表接口仍能快速返回：
   ```bash
   time curl -fsS "http://127.0.0.1:${BIFROST_TEST_PORT}/_bifrost/api/traffic?limit=20" -o /tmp/bifrost-traffic-list.json
   ```
5. 从第 2 步的 SSE 列表结果中取一条 SSE 请求 ID，验证详情、SSE frames 元信息和响应体接口：
   ```bash
   export BIFROST_SSE_ID="$(curl -fsS "http://127.0.0.1:${BIFROST_TEST_PORT}/_bifrost/api/traffic?limit=50&is_sse=true" | jq -r '.records[0].id')"
   time curl -fsS "http://127.0.0.1:${BIFROST_TEST_PORT}/_bifrost/api/traffic/${BIFROST_SSE_ID}" -o /tmp/bifrost-sse-detail.json
   time curl -fsS "http://127.0.0.1:${BIFROST_TEST_PORT}/_bifrost/api/traffic/${BIFROST_SSE_ID}/frames" -o /tmp/bifrost-sse-frames.json
   time curl -fsS "http://127.0.0.1:${BIFROST_TEST_PORT}/_bifrost/api/traffic/${BIFROST_SSE_ID}/response-body" -o /tmp/bifrost-sse-response-body.json
   ```
6. 在浏览器 Traffic 页面点击该 SSE 请求，打开详情页并切到 SSE Messages/Frames 视图。
7. 以 200 QPS 混合普通 HTTP 与 CONNECT 请求持续压测 60 秒，同时每 0.5 秒轮询 `/_bifrost/api/proxy/address`、`/_bifrost/api/traffic?limit=20`、`/_bifrost/api/rules`、`/_bifrost/api/config`、`/_bifrost/api/values`、`/_bifrost/api/scripts` 和最新 Traffic 详情接口；压测结束后抽查最近 10 条 GET 详情。

**预期结果**：
- 高并发 CONNECT 压力期间，Traffic 列表接口返回 HTTP 200，且 wall time 不出现秒级以上的明显卡死。
- SSE 请求详情接口返回 HTTP 200，JSON 中包含该请求的 URL、状态码和 SSE 标识。
- SSE frames 接口返回 HTTP 200，`socket_status.frame_count` 至少为 1；如果闭合 SSE 的 frames 列表为空，则响应体接口必须包含实际 SSE event 数据。
- 浏览器中的 Traffic 页面不长期停留在 loading 状态；SSE Messages/Frames 视图能展示事件或明确的空状态。
- Bifrost 日志中不应出现持续增长的同步客户端进程解析排队阻塞；如出现 `Client process resolution timed out; continuing without app info`，对应请求应继续完成或按策略降级，不能因为等待客户端应用信息导致管理端接口超时。
- 在极端压力下，近期解析失败应短期命中 negative cache，不应反复创建同一连接的 blocking 解析任务；进程解析并发饱和时应在 2 秒总预算内等待解析机会，预算耗尽后降级为未知客户端，但不能把“饱和未执行解析”写成端口负缓存。
- 200 QPS 混合压测中，由同一本地客户端进程发起的短 HTTP 和 CONNECT 请求，Traffic DB 中保留记录的 `client_app` 不应出现大量空值；应用黑白名单关键路径不能因为高并发排队而系统性失效。
- 200 QPS 混合压测期间管理端 API 不超时；压测后的 GET 详情可返回 HTTP 200，且最终包含 `response_body_ref`，不得以丢失记录或丢失 body 换取性能。

---

### TC-WTR-回归-01：file:// 规则响应的请求在 Traffic 中可见

**背景**：修复 Bug——使用 file:// 规则（如 `a.com file://xxxx`）响应请求时，该请求不在 network traffic 中显示。

**前置条件**：
1. 创建一个临时文件作为 mock 响应：
   ```bash
   echo '{"mock":"from_file"}' > /tmp/bifrost-mock-test.json
   ```
2. 启动 Bifrost 服务并配置 file:// 规则：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl -r "test.local file:///tmp/bifrost-mock-test.json"
   ```

**操作步骤**：
1. 通过代理发起请求到匹配 file:// 规则的域名：
   ```bash
   curl -x http://127.0.0.1:8800 http://test.local/any-path
   ```
2. 在浏览器中打开 `http://127.0.0.1:8800/_bifrost/traffic`
3. 通过 API 验证 traffic 记录：
   ```bash
   curl "http://127.0.0.1:8800/_bifrost/api/traffic?limit=10"
   ```

**预期结果**：
- curl 请求返回 mock 文件内容 `{"mock":"from_file"}`
- Traffic 页面中可以看到该请求记录
- 请求记录 Host 为 `test.local`，Status 为 `200`
- 请求记录的 Rules 列显示规则命中标识（蓝色闪电图标 + 数字）
- API 返回的 records 数组中包含该请求记录，`h` 字段为 `test.local`，`s` 字段为 `200`，`rc` > 0

---

### TC-WTR-回归-02：redirect:// 规则响应的请求在 Traffic 中可见

**背景**：与 file:// 同属 HTTPS tunnel mock 响应路径，修复前 redirect:// 规则响应也不会被录制。

**操作步骤**：
1. 启动 Bifrost 服务并配置 redirect:// 规则：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl -r "test.local/old redirect://https://example.com/new"
   ```
2. 通过代理发起请求：
   ```bash
   curl -x http://127.0.0.1:8800 http://test.local/old -v
   ```
3. 通过 API 验证 traffic 记录：
   ```bash
   curl "http://127.0.0.1:8800/_bifrost/api/traffic?limit=10"
   ```

**预期结果**：
- curl 返回 302 重定向响应，`Location` 头为 `https://example.com/new`
- API 返回的 records 中包含该请求记录，`h` 字段为 `test.local`，`s` 字段为 `302`

---

### TC-WTR-回归-03：tpl:// 和 rawfile:// 规则响应的请求在 Traffic 中可见

**操作步骤**：
1. 启动 Bifrost 服务并配置规则：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl \
     -r "test.local/raw rawfile://(raw-content-test)" \
     -r "test.local/tpl tpl://(Template for {{url}})"
   ```
2. 分别发起请求：
   ```bash
   curl -x http://127.0.0.1:8800 http://test.local/raw
   curl -x http://127.0.0.1:8800 http://test.local/tpl
   ```
3. 通过 API 验证：
   ```bash
   curl "http://127.0.0.1:8800/_bifrost/api/traffic?limit=10"
   ```

**预期结果**：
- 两个请求都返回对应的 mock 内容
- API 返回的 records 中包含两条请求记录，Host 均为 `test.local`
- 两条记录的 Rules 计数均 > 0

---

## 清理

测试完成后清理临时数据：
```bash
rm -rf .bifrost-test
rm -f /tmp/bifrost-mock-test.json
```

## 执行记录

2026-05-20 Network `.bifrost` 空包导入/导出防误报执行记录：

- 已执行用例：`TC-WTR-48`、`TC-WTR-49`
- 使用隔离数据目录：`/tmp/bifrost-network-import-debug-data`
- 使用端口：`18892`，未使用 `9900`；启动命令包含 `--no-system-proxy --skip-cert-check`
- 已执行命令：`source ~/.zshrc; BIFROST_DATA_DIR=/tmp/bifrost-network-import-debug-data cargo run --bin bifrost -- start -p 18892 --unsafe-ssl --no-system-proxy --skip-cert-check`
- `TC-WTR-48` API 实际结果：对 `e2e-tests/test_data/bifrost-file/network-empty.bifrost`（内容等同用户提供的空 Network 包：`01 network ... count = 0 ... --- []`）执行 `POST /_bifrost/api/bifrost-file/import`，返回 HTTP `400`，响应体包含 `Network file contains 0 records; nothing to import`。
- `TC-WTR-48` WebUI 实际结果：Playwright 打开 `http://127.0.0.1:18892/_bifrost/traffic` 并模拟拖入 `e2e-tests/test_data/bifrost-file/network-empty.bifrost`，导入 API 返回 `400`，Toast 显示 `Import failed: Network file contains 0 records; nothing to import. Re-export from Network after selecting at least one visible request.`，未出现 `successfully` 成功 Toast。
- `TC-WTR-49` 空选中导出实际结果：`POST /_bifrost/api/bifrost-file/export/network` 请求体 `{"record_ids":[],"include_body":true}` 返回 HTTP `400`，响应体包含 `Select at least one Network record`。
- `TC-WTR-49` WebUI 导出入口实际结果：`pnpm --dir web test:unit src/api/bifrost-file.test.ts` 覆盖 `record_ids: []`，前端公共导出校验返回 `Select at least one Network record before exporting a .bifrost file`；`record_ids: ["REQ-1"]` 不返回阻断消息。
- `TC-WTR-49` 不存在 ID 导出实际结果：请求体 `{"record_ids":["REQ-NOT-EXIST"],"include_body":true}` 返回 HTTP `400`，响应体包含 `selected record(s) no longer exist: REQ-NOT-EXIST`。
- 结论：两个用例均通过；空 Network 包不会再被提示为导入成功，导出端也不会静默生成 `count = 0` / `[]` 的 `.bifrost` 文件。

2026-05-09 Traffic 主筛选器临时停用单条条件执行记录：

- 已执行用例：`TC-WTR-23C`
- 已执行命令：`source ~/.zshrc; pnpm --dir web test:ui traffic.spec.ts -g "主筛选器支持临时停用单条条件"`
- 使用隔离数据目录：Playwright UI 全局 setup 与用例内 `startIsolatedBackend()` 自动分配独立 `BIFROST_DATA_DIR`，启动 Bifrost 时包含 `--no-system-proxy`。
- 端口要求：UI 测试动态分配后端端口，未使用 `9900`。
- 实际结果：Playwright 本次执行 `1 passed`。新增筛选条件 checkbox 默认选中；输入 Path 条件后只展示 target 记录；取消勾选后不删除条件且 target/other 记录都可见；重新勾选后条件再次生效。
- 结论：`TC-WTR-23C` 已按文档完成执行并通过，本次无环境阻塞。

2026-05-08 Traffic 主筛选器端口过滤执行记录：

- 已执行命令：`source ~/.zshrc; pnpm --dir web test:ui traffic.spec.ts -g "主筛选器支持按代理端口过滤 Traffic"`
- 使用隔离数据目录：Playwright UI 全局 setup 自动分配独立 `BIFROST_DATA_DIR` 与独立后端端口，启动 Bifrost 时包含 `--no-system-proxy`。
- 端口要求：UI 测试动态分配后端端口，未使用 `9900`。
- 实际结果：Playwright 本次执行 `1 passed`，Traffic 主筛选器可选择 `Port`，输入临时端口后列表只保留对应入口端口记录。
- 结论：`TC-WTR-23B` 已按文档完成执行并通过，本次无环境阻塞。

2026-05-09 高并发 CONNECT 压力下 Traffic 和 SSE 详情执行记录：

- 已执行用例：`TC-WTR-47`
- 使用隔离数据目录：`/tmp/bifrost-webui-traffic-perf.YPeYko`
- 使用端口：`18880`，未使用 `9900`；启动命令包含 `--no-system-proxy`
- 已执行命令：`source ~/.zshrc; BIFROST_DATA_DIR=/tmp/bifrost-webui-traffic-perf.YPeYko cargo run --bin bifrost -- start -p 18880 --unsafe-ssl --no-system-proxy`
- 已执行命令：通过本地 SSE mock 服务发起 `NO_PROXY="" no_proxy="" curl -sS --max-time 8 -x http://127.0.0.1:18880 http://127.0.0.1:18981/sse?count=3`
- 已执行命令：`source ~/.zshrc; seq 1 160 | xargs -n1 -P40 ... curl -ksS --max-time 5 -x http://127.0.0.1:18880 https://example.com/?connect_pressure={}`
- 管理端列表接口实际结果：`/_bifrost/api/traffic?limit=20` 在压力期间返回 HTTP 200，`real 0.03s`
- SSE 详情接口实际结果：`/_bifrost/api/traffic/REQ-69fee228-000001` 返回 HTTP 200，`real 0.27s`
- SSE frames 元信息实际结果：`/_bifrost/api/traffic/REQ-69fee228-000001/frames` 返回 HTTP 200，`real 0.26s`，`socket_status.frame_count=3`
- SSE 响应体实际结果：`/_bifrost/api/traffic/REQ-69fee228-000001/response-body` 返回 HTTP 200，`real 0.26s`，包含 `bifrost-event-0`、`bifrost-event-1`、`bifrost-event-2`
- 浏览器验证实际结果：Playwright 打开 `/_bifrost/traffic` 和 `/_bifrost/traffic/detail?id=REQ-69fee228-000001`，页面未长期 loading，详情页展示 URL、`text/event-stream`、`SSE Status Closed`、`Receive Count 3`、`Frame Count 3` 和 `Messages (3)`；浏览器 console 未出现 error
- 进程解析超时和并发降级：代码增加了 2 秒硬超时、negative cache 快速返回和全局 blocking 并发阀门；本次压力未触发持续超时堆积，若极端系统调用或并发阀门饱和，请求会按未知客户端降级并继续处理。
- 热路径日志验证：CONNECT/SOCKS5 应用策略解析的逐请求日志降级为 debug，默认 info 日志不再为每个 CONNECT 输出 `requires synchronous client process resolution` / `succeeded` / `unknown`；已同步解析失败的 CONNECT 不再立即追加 background backfill。
- 结论：`TC-WTR-47` 已按文档完成执行并通过，本次性能优化没有改变 Traffic/SSE 可见性，CONNECT 压力下管理端接口和详情页保持可响应。

2026-05-09 负缓存和并发阀门补充后二次执行记录：

- 已执行用例：`TC-WTR-47`
- 使用隔离数据目录：`/tmp/bifrost-webui-traffic-perf2.9ZkGSR`
- 使用端口：`18881`，未使用 `9900`；启动命令包含 `--no-system-proxy`
- 已执行命令：通过本地 SSE mock 服务发起 `NO_PROXY="" no_proxy="" curl -sS --max-time 8 -x http://127.0.0.1:18881 http://127.0.0.1:18982/sse?count=3`
- 已执行命令：`source ~/.zshrc; seq 1 160 | xargs -P40 ... curl -ksS --max-time 5 -x http://127.0.0.1:18881 https://example.com/?connect_pressure={}`
- 管理端列表接口实际结果：`/_bifrost/api/traffic?limit=20` 在压力后返回 HTTP 200，`real 0.04s`
- SSE 详情接口实际结果：`/_bifrost/api/traffic/REQ-69fee87e-000001` 返回 HTTP 200，`real 0.25s`
- SSE frames 元信息实际结果：`/_bifrost/api/traffic/REQ-69fee87e-000001/frames` 返回 HTTP 200，`real 0.24s`，`socket_status.frame_count=3`
- SSE 响应体实际结果：`/_bifrost/api/traffic/REQ-69fee87e-000001/response-body` 返回 HTTP 200，`real 0.23s`，包含 `bifrost-event-0`、`bifrost-event-1`、`bifrost-event-2`
- 浏览器验证实际结果：Playwright 打开 `/_bifrost/traffic` 和 `/_bifrost/traffic/detail?id=REQ-69fee87e-000001`，页面未长期 loading，详情页展示请求 ID、`text/event-stream` 和 SSE 信息；浏览器 console 和 pageerror 均为空
- 负缓存、并发阀门与快照刷新验证：单元测试覆盖 async negative cache 快速返回和并发饱和快速降级；代码对 socket 快照刷新增加 singleflight，避免同一 TTL 窗口重复系统扫描；真实场景中 CONNECT 压力下管理端接口未出现秒级卡死。
- 结论：`TC-WTR-47` 二次执行通过，负缓存和并发阀门补充后没有造成功能可见性损失。

2026-05-09 200 QPS 管理端接口与数据完整性补充执行记录：

- 已执行用例：`TC-WTR-47`
- 使用隔离数据目录：`/tmp/bifrost-qps200-load8.UKDomc`
- 使用端口：`18886`，未使用 `9900`；启动命令包含 `--no-system-proxy`
- 已执行命令：`source ~/.zshrc; BIFROST_DATA_DIR=/tmp/bifrost-qps200-load8.UKDomc RUST_LOG=warn,bifrost_proxy::utils::process_info=warn cargo run --bin bifrost -- start -p 18886 --host 127.0.0.1 --no-system-proxy --access-mode allow_all --app-intercept-include DefinitelyNoSuchApp`
- 已配置规则：`**.load.test host://127.0.0.1:19083`
- 压测流量：60 秒、目标 200 QPS、混合普通 HTTP 与 CONNECT；总请求 `12000`，成功 `12000`，失败 `0`，实际 QPS `200.0`。
- 管理端接口轮询：共 `40` 次，成功 `40`，失败 `0`；覆盖 `proxy/address`、`traffic?limit=20`、`rules`、`config`、`values`、`scripts` 和最新 Traffic 详情。
- 管理端接口延迟：平均 `1005.4ms`，P50 `1044.6ms`，P95 `1108.9ms`，最大 `1205.5ms`。
- CPU 采样：平均 `42.6%`，P95 `62.4%`；`ps` 采样存在一次瞬时 `109.9%` 峰值，未观察到持续超过 `70%`。
- 数据完整性抽样：压测后抽查最近 10 条 GET 详情，详情 HTTP 200 为 `10/10`，`response_body_ref` 就绪为 `10/10`。
- 结论：`TC-WTR-47` 200 QPS 补充执行通过；管理端接口无超时，普通 HTTP/CONNECT 记录未丢失，响应体引用最终可见。本次优化不以丢失 Traffic 记录或响应体为代价。

2026-05-09 200 QPS appinfo 命中率补充执行记录：

- 已执行用例：`TC-WTR-47`
- 使用隔离数据目录：`/tmp/bifrost-appinfo-final2.pW1lFm`
- 使用端口：`18889`，未使用 `9900`；启动命令包含 `--no-system-proxy`
- 已执行命令：`source ~/.zshrc; BIFROST_DATA_DIR=/tmp/bifrost-appinfo-final2.pW1lFm RUST_LOG=warn,bifrost_proxy::utils::process_info=warn cargo run --bin bifrost -- start -p 18889 --host 127.0.0.1 --no-system-proxy --access-mode allow_all --app-intercept-include DefinitelyNoSuchApp`
- 压测流量：60 秒、目标 200 QPS、混合普通 HTTP 与 CONNECT；总请求 `12000`，完成 `12000`，普通 HTTP 成功 `8000/8000`，CONNECT 成功 `4000/4000`。
- 管理端接口轮询：共 `120` 次，成功 `120`，失败 `0`；平均 `33.3ms`，P95 `60ms`，最大 `67ms`。
- CPU 采样：平均 `46.2%`，P95 `54.7%`，最大瞬时 `78.5%`；未观察到持续超过 `70%`。
- appinfo 完整性：受 `traffic.max_records=5000` 保留策略影响，DB 保留最新 `4800` 条记录；保留记录中 `client_app` 空值 `0`，`node` 命中 `4800`，其中 CONNECT `1600`、GET `3200`。
- 结论：`TC-WTR-47` appinfo 补充执行通过；短 HTTP/CONNECT 高并发下，最终保留记录没有出现大量 unknown app，管理端接口可持续响应，CPU P95 低于 `70%` 目标。

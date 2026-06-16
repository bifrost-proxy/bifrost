## 端到端验证：SSE 请求详情「分批推送」(服务端改动必做)

目标：验证打开 SSE 请求详情时，历史事件不会一次性推送过多导致前端卡死；数据应分批到达并可持续增量更新。

### 1) 单独启动代理（快速编译模式 + 临时数据目录）

```bash
# 在项目根目录执行
export BIFROST_DATA_DIR=./.bifrost-e2e-test
rm -rf "$BIFROST_DATA_DIR"

# 快速启动：跳过前端构建；端口可自行调整
SKIP_FRONTEND_BUILD=1 cargo run --bin bifrost -- start -p 8890 --unsafe-ssl --skip-cert-check
```

### 2) 发起 SSE 代理流量（产生可回放的 SSE 请求记录）

推荐用 curl 持续拉取（会一直保持连接）：

```bash
curl -N --proxy http://127.0.0.1:8890 https://echo.websocket.org/.sse
```

如果外部站点不可用（网络受限/服务下线），可改用本仓库自带 SSE mock：

```bash
# 启动本地 SSE mock（另开一个终端）
python3 e2e-tests/mock_servers/sse_echo_server.py --port 8767

# 通过代理访问 mock（会生成 SSE 记录）
curl -N --proxy http://127.0.0.1:8890 "http://127.0.0.1:8767/sse/custom?count=200&interval=0.05"
```

如果本机未安装/信任代理 CA，或遇到 TLS 校验问题，可增加：

```bash
curl -N -k --proxy http://127.0.0.1:8890 https://echo.websocket.org/.sse
```

### 3) 打开管理端页面验证交互与性能

1. 打开：`http://127.0.0.1:8890/_bifrost/`
2. 在 Traffic 列表里找到刚产生的 SSE 记录（URL 包含 `echo.websocket.org/.sse`），进入详情页
3. 验证点：
   - 详情页可正常打开、滚动/点击不卡顿
   - Messages 面板事件逐步增长，不出现「打开即长时间卡死/无响应」
   - Response Body（如展示）不会无限制增长导致 UI 卡顿（必要时应出现截断表现）

### 4) 可选：用 Admin API 直接验证分批推送行为

```bash
# 先用列表接口找到 traffic_id（也可以从页面复制）
curl -s "http://127.0.0.1:8890/_bifrost/api/traffic?limit=20" | jq .

# 订阅 SSE 详情流；from=begin 表示从历史回放开始
curl -N "http://127.0.0.1:8890/_bifrost/api/traffic/{traffic_id}/sse/stream?from=begin"
```

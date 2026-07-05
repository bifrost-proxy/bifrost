# Trustworthy Traffic Metrics

## 功能模块说明

验证 Activity、Metrics API、Traffic API 与按应用/主机统计使用可信的流量指标口径：

- 每个代理请求只计数一次。
- HTTP/HTTPS 请求 body 计入 upload bytes。
- HTTP/HTTPS/Mock 响应 body 计入 download bytes。
- QPS 与实时网速来自最近事件窗口，不依赖 UI 轮询间隔。
- Traffic detail、compact list、host/app distribution 都能读取 trusted upload/download 字段。

## 前置条件

- 当前目录为 Bifrost 仓库根目录。
- 已安装 `jq`、`python3`、`curl`。
- 已构建当前分支二进制：

```bash
SKIP_FRONTEND_BUILD=1 cargo build -p bifrost-cli --bin bifrost
```

## 测试用例列表

### TC-TTM-01 真实 HTTP POST 上行/下行统计

操作步骤：

1. 执行：

```bash
BIFROST_BIN="$PWD/target/debug/bifrost" e2e-tests/tests/test_trustworthy_traffic_metrics.sh
```

2. 观察脚本输出中的真实 POST 断言。

预期结果：

- `proxy request counter is not double counted` 通过，5 个代理请求只增加 5。
- `upload bytes include real POST body` 通过。
- `traffic detail stores trusted POST upload bytes` 通过。
- `traffic detail stores trusted POST download bytes` 通过。

### TC-TTM-02 Mock 直接响应下行统计

操作步骤：

1. 继续执行 TC-TTM-01 中的同一脚本。
2. 观察 Mock direct response 断言。

预期结果：

- `Mock GET upload bytes remain zero` 通过。
- `Mock response body is stored as trusted download bytes` 通过，Mock body 长度精确入账。
- `download bytes include Mock body` 通过。

### TC-TTM-03 QPS 与实时网速可信性

操作步骤：

1. 继续执行 TC-TTM-01 中的同一脚本。
2. 观察 burst 后的 Metrics API 断言。

预期结果：

- `QPS reflects the recent burst` 通过，突发 5 个请求后 QPS 至少为 5。
- `upload rate reflects recent bytes` 通过。
- `download rate reflects recent bytes` 通过。

### TC-TTM-04 Host/App 分布使用 trusted bytes

操作步骤：

1. 继续执行 TC-TTM-01 中的同一脚本。
2. 观察 host/app distribution 断言。

预期结果：

- `host distribution uses trusted download bytes` 通过。
- `app distribution uses trusted download bytes` 通过。

## 清理步骤

脚本退出时自动执行：

- 停止临时 Bifrost 实例。
- 停止临时 mock HTTP server。
- 删除临时 `BIFROST_DATA_DIR`。
- 释放临时端口。

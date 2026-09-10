# Search Include Body / Headers 与 Traffic Batch Get 回归

## 功能与前置条件

验证真实 CLI 返回的 body/header 内容、正文上限和批量格式。需要 Rust、Python 3，从仓库根目录执行：

```bash
python3 e2e-tests/tests/test_traffic_search_matrix.py
```

脚本编译当前代码，创建本地 HTTP fixture 与隔离代理；动态端口避开 9900，临时数据目录，禁用 tray、Sync 登录提示和系统代理。

## 用例

在命令输出中逐条核对以下名称为 PASS，最终 failed=0。

| 编号 | 操作与输出断言名称 | 预期 |
|---|---|---|
| TC-SIB-01 | `include bodies headers json/json-pretty`：`search --include bodies,headers --max-body 32` | 两侧 base64 解码恰好 32 字节，truncated=true，headers 为数组 |
| TC-SIB-02 | `include bodies headers table/compact`，加 `--no-color` | 包含目标路径，不含 ANSI 转义 |
| TC-SIB-03 | `include bodies headers ndjson` | result 行保留两侧 body/header，逐行 JSON 可解析 |
| TC-SIB-04 | `batch sequence=False/True format=ndjson/json`、`batch default NDJSON` | 完整 ID 和序号解析正确；默认两行；缺失项 error=not_found 不影响成功项 |
| TC-SIB-05 | `batch sequence=False/True format=json-pretty` | 显式 json-pretty 可整体解析为 results 数组，不能误输出 NDJSON |
| TC-SIB-06 | `reject arguments`：ID 与 ids 同传、空 ids、201 个 ids | 全部非 0 退出，错误不伪装为空成功 |
| TC-SIB-回归-01 | `single get format` 的五种格式和 `single get full sequence` | 精确记录身份、正文可读；单条行为不因 batch 修复改变 |
| TC-SIB-回归-02 | `reject search/traffic search --include bodise` | 拼错 token 在 CLI 阶段拒绝，避免静默丢正文 |

## 数据与安全边界

batch 成功项为 `id/ok/record` 加请求的 body/header；单条 get 与 batch 的正文 schema 不同。所有 body payload 使用 `bytes_b64`，当前 batch 客户端读取完整响应后输出，不承诺常量内存流式处理。

当前 remote search/get 未暴露本地全部 include/batch 参数，本用例只验证本地两个搜索入口。正文和 headers 不自动脱敏，测试只用合成数据，禁止向低信任接收方发送真实 Cookie/Authorization。

## 清理

脚本 finally 关闭自身代理进程组与 mock 服务并删除临时目录，失败路径也执行；不修改正式服务、配置或系统代理。

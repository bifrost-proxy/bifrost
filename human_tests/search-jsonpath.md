# Search JSONPath / Header / 时间窗回归

## 功能与前置条件

验证真实 `search` / `traffic search` CLI 的高级过滤与格式，不使用 mock CLI 输出。需要 Rust、Python 3、可构建的当前 checkout；fixture 为本地 HTTP 服务，通过隔离代理生成流量，不依赖账号或外网。

从仓库根目录执行：

```bash
python3 e2e-tests/tests/test_traffic_search_matrix.py
```

脚本先编译、再使用动态端口与临时数据目录，设置 `BIFROST_DISABLE_TRAY=1`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`--no-system-proxy`。不得使用正式端口 9900。

## 用例

以下均由上面的命令逐条调用真实 CLI；在输出中核对对应名称为 PASS，最终 failed=0。

| 编号 | 操作与输出断言名称 | 预期 |
|---|---|---|
| TC-SJ-01 | `request JSONPath`：查询 `$.user.id=42`、省略根前缀、布尔、空串、null、值含等号 | 精确命中 object/gzip 两条，null 不等于空串 |
| TC-SJ-02 | `response JSONPath`、`multiple JSONPath AND`、`conflicting JSONPath AND` | 响应字段和多条件 AND 命中准确，矛盾条件为空 |
| TC-SJ-03 | `header equals`、`scope`：请求与响应 header 等值及独立关键词范围 | header 名大小写无关，作用域不串侧 |
| TC-SJ-04 | `timestamp bounds`、`empty old time window`、`reversed time window` | 正常窗口包含 fixture；过去或反向窗口为空 |
| TC-SJ-05 | `latest 30s/5m/2h/1d/1w` | 最近时间窗包含全部 fixture，不被误解为 limit=1 |
| TC-SJ-06 | `include bodies headers ndjson`、`aliases exact result identity` | 每行 JSON 可解析，两个本地入口命中一致 |
| TC-SJ-回归-01 | 根 `$=42`、根数组 `$[0].id=42`、`$.items[*].id=9` | 分别仅命中标量、根数组或包含匹配元素的对象 |
| TC-SJ-回归-02 | `reject search` / `reject traffic search` 的 JSONPath、header、since/until/latest | 非法语法在参数阶段非 0 退出，不静默丢弃过滤 |
| TC-SJ-回归-03 | `filter-only no keyword`、`traffic search filter-only no keyword` | 无关键词 JSON 查询成功，不进入 TUI |

## 边界与清理

支持 `$`、点路径、非负数组索引和 `[*]`；不支持递归下降、切片、过滤表达式。CLI 不提供 `--res-json-gt`，数字比较是 Admin FilterCondition 能力，不伪造 CLI 参数。

脚本 finally 停止本次拥有的代理和 mock 进程，删除自己的临时目录；不得清理正式实例。远端 parser/command 映射由单元测试验证，本用例不声称验证真实 relay 连通性。

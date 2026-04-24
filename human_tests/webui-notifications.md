# Web UI Notifications 页面

## 功能模块说明

验证管理端 `Notifications` 页面三个通知表的顶部状态筛选与固定分页行为：

1. `All Notifications`
2. `TLS Trust`
3. `Authorization`

每个表都应提供 `All / Read / Unread` 顶部筛选，并在首次进入该表时默认显示未读消息；表格分页固定为每页 `10` 条，只允许默认翻页，不允许切换 page size。

## 前置条件

```bash
rm -rf ./.bifrost-human-notifications
mkdir -p ./.bifrost-human-notifications

BIFROST_DATA_DIR=./.bifrost-human-notifications \
  cargo run --bin bifrost -- start --host 127.0.0.1 -p 8800 --unsafe-ssl --no-system-proxy
```

- 管理端地址：`http://127.0.0.1:8800/_bifrost/`
- 如需本地前端开发态验证，可额外启动：

```bash
cd web
BACKEND_PORT=8800 pnpm exec vite --host 127.0.0.1 --port 4017
```

- 用例执行前向 `./.bifrost-human-notifications/notifications.db` 写入以下测试数据：
  - `tls_trust_change`：至少 1 条 `unread`、1 条 `read`
  - `pending_authorization`：至少 1 条 `unread`、1 条 `read`
  - 额外补充 20+ 条未读通知，用于确认分页仍可使用但没有 page size 切换器

## 测试用例

### TC-WN-01：All Notifications 默认只显示未读消息

**步骤**：
1. 打开 `http://127.0.0.1:8800/_bifrost/notifications?tab=all`
2. 观察顶部筛选控件
3. 检查列表中未读和已读标记数据的展示情况

**预期结果**：
- 顶部存在 `All`、`Read`、`Unread` 三个筛选项
- 首次进入时默认选中 `Unread`
- 表格只展示未读通知，不展示预置的已读通知

### TC-WN-02：TLS Trust 与 Authorization 表首次进入时也默认选中 Unread

**步骤**：
1. 点击 `TLS Trust` Tab
2. 观察顶部筛选控件与表格内容
3. 点击 `Authorization` Tab
4. 再次观察顶部筛选控件与表格内容

**预期结果**：
- 两个表顶部都存在 `All`、`Read`、`Unread` 三个筛选项
- 首次进入各自表格时默认选中 `Unread`
- `TLS Trust` 仅显示未读的 TLS 通知
- `Authorization` 仅显示未读的授权通知

### TC-WN-03：切换筛选后展示结果正确，且分页不允许选择 page size

**步骤**：
1. 在 `All Notifications` 表切换到 `All`
2. 验证已读通知开始出现
3. 切换到 `Read`
4. 验证仅剩已读通知
5. 观察表格底部分页区域

**预期结果**：
- 切换到 `All` 后同时显示已读与未读通知
- 切换到 `Read` 后只显示已读通知
- 底部分页控件仍可正常翻页
- 分页区域不显示 `10 / page`、`20 / page` 之类的 page size 下拉选择器

## 清理步骤

```bash
pkill -f "bifrost -- start --host 127.0.0.1 -p 8800" || true
rm -rf ./.bifrost-human-notifications
```

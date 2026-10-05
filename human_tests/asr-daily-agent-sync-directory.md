# ASR Daily Agent 同步目录草稿回归

## 功能模块说明

验证 Daily Agent 返回列表和配置刷新不会覆盖尚未保存的报告同步目录，同时保留成功保存、失败重试、服务端更新和任务切换行为。

## 前置条件

- 使用支持 ASR Directory Tasks 的测试环境及两个独立临时任务 A、B；不要修改真实任务。
- 测试服务使用临时 `BIFROST_DATA_DIR`、非正式端口、`--no-system-proxy`、`BIFROST_DISABLE_TRAY=1` 和 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。
- 在 Chrome 中打开测试服务的 AI → ASR → Directory Task → Daily Agent。
- 准备两个临时报告目录，将实际路径记为 `REPORT_DIR_A`、`REPORT_DIR_B`。用实际路径替换下文变量名。
- 在 DevTools Network 中观察 `/asr/tasks/{task_id}/daily-agent` 及其 `/agents` 请求；需要放大请求与输入的竞态时开启网络限速。

## 测试用例

### TC-ADA-SYNC-01 返回列表和后台刷新保留草稿

1. 打开任务 A 的单 Agent 详情，再点击 `Daily Agents` 返回列表。
2. 在返回触发的配置及指令请求完成之前，将 `Optional report sync directory` 输入框改为 `REPORT_DIR_A`。
3. 等待这些请求完成，确认输入值仍为 `REPORT_DIR_A`，旁边的 `Save` 仍可点击。
4. 点击 `Refresh`，等待请求完成，再次确认草稿和值的可保存状态均保留。

### TC-ADA-SYNC-02 保存和清空目录

1. 保存 `REPORT_DIR_A`，确认成功提示，输入值保留且 `Save` 禁用。
2. 在 Network 确认 PUT 中的 `report_sync_dir` 与输入一致，后续 GET 返回已保存值。
3. 清空输入框，再点击 `Refresh`。确认输入仍为空且 `Save` 可用。
4. 保存空值，确认目录保持为空且 `Save` 禁用。
5. 通过同一测试任务的另一浏览器页保存 `REPORT_DIR_B`，回到当前无草稿页面点击 `Refresh`，确认显示新服务端值。

### TC-ADA-SYNC-03 保存失败可以重试

1. 输入 `REPORT_DIR_A`，在 DevTools Network 切换 Offline 后点击 `Save`。
2. 确认显示保存失败，草稿仍为 `REPORT_DIR_A`，`Save` 重新可用。
3. 恢复联网后点击 `Refresh`，确认未保存草稿仍然保留。
4. 再次点击 `Save`，确认 PUT 成功、目录保留且 `Save` 禁用。

### TC-ADA-SYNC-04 切换任务隔离草稿和迟到响应

1. 在任务 A 中编辑目录但不保存，并在限速下触发 `Refresh`。
2. 切换到任务 B，确认输入显示 B 的已保存值，而非 A 的草稿。
3. 编辑 B 的目录为 `REPORT_DIR_B`；等待 A 的旧请求完成，确认 B 的草稿及可保存状态不变。
4. 在 A 保存请求仍进行时切换到 B 并编辑 B，确认 A 的保存完成不会清除 B 的草稿或把 B 的 `Save` 禁用。

## 执行记录

- 2026-10-05：现有 CI 浏览器 trace 确认 TC-ADA-SYNC-01 的原始故障；配置读取及其指令请求完成后，输入草稿被空的服务端值覆盖。新增真实 React/Ant Design 组件回归以受控 API 响应顺序覆盖上述竞态。本环境浏览器 AF_UNIX IPC 受限，以上手工浏览器用例尚未执行，组件测试不计作手工用例通过。

## 清理

- 恢复 Chrome 的网络模式并关闭限速。
- 删除本次创建的临时任务和目录；只停止本次启动的测试进程，不操作正式服务。

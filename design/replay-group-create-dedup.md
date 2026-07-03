# Replay 创建分组去重

## 背景

Bifrost 管理端 Replay 页面左侧有分组树，用户可以创建 / 重命名 / 删除分组用来归档保存的 Replay 请求。当前实现里，前端 `createGroup` 在两条数据路径上都会往 store 追加同一条分组：

1. `POST /_bifrost/api/replay/groups` 返回体本身携带新创建的 group，前端把它 `append` 到 `groups`。
2. 后端在 Replay 广播频道推送 `replay_groups_update` 全量快照，前端 push handler 用整个快照覆盖 `groups`。

在生产环境网络与后端处理都很快，两条数据几乎同时到达；`append` 后再被 push 快照覆盖，用户会看到短暂的“两个同名分组”闪烁，尽管刷新页面就恢复正常。

问题只发生在前端状态同步，后端与数据库始终只有一条记录。本次修复保持后端行为不变，只在 store 里把 `append` 改成按 `group.id` 做 upsert。

## 用户目标验证清单

### 必须实现

- 新建分组后，无论 push 快照与 POST 响应哪一个先到，前端分组列表都只保留一条对应 `group.id` 的节点。
- 分组树按名称 / 排序仍然稳定，不因去重逻辑而错位。
- 分组创建、重命名、删除、拖拽移动请求到分组的既有行为不变。

### 必须不破坏

- Replay push 快照 (`replay_groups_update`) 继续用来做集合修正。
- 后端 `POST /_bifrost/api/replay/groups` 保持原返回结构。
- 分组重命名走同一套 upsert：同 id 覆盖，避免因为顺序问题渲染两个名字。
- 未登录 / push 断线场景，POST 响应仍能直接把新分组反映到 UI。

### 必须真实验证

- Playwright 用例在真实浏览器里点击“新建分组”，断言 `[data-group-id]` 只出现一次。
- 手工回归：断网重连 push、快速连续点新建分组均不出现重复。

## 产品语义

Replay 分组是一个由后端 authoritative 维护、通过全量 push 快照下发的“可归档集合”。前端所有本地写入路径都必须遵循两条原则：

1. **id 是唯一键**：任何本地写入都以 `group.id` 为主键做 upsert，避免任何“追加”。
2. **后端快照最终一致**：push 快照到达时以后端为准；本地 upsert 只是加快渲染，与 push 一致时应无副作用。

这种模式与 Replay saved requests / tabs / history 的处理方式一致，便于统一心智模型。

## 技术细节

### store 修改

`web/src/stores/useReplayStore.ts` 的 `createGroup`（line 762 附近）：

```ts
createGroup: async (name: string) => {
  try {
    const group = await replayApi.createGroup(name);
    set((state) => ({
      groups: state.groups.some((item) => item.id === group.id)
        ? state.groups.map((item) => (item.id === group.id ? group : item))
        : [...state.groups, group],
    }));
    return true;
  } catch (err) {
    // error handling
    return false;
  }
};
```

关键点：

- `state.groups.some(item.id === group.id)` 判定是否已经存在。
- 存在：`map` 覆盖（拿到最新字段，包含 push 里可能已经写入的排序等元数据）。
- 不存在：追加。

### push handler

- 现有 push handler 用 `set({ groups: snapshot })` 直接覆盖不受影响；它天然是幂等的。
- 若未来 push 变为增量 diff，同一 upsert 抽成公共方法（例如 `mergeGroupsById`）复用。

### 相关 API

- `replayApi.createGroup(name)` 调用 `POST /_bifrost/api/replay/groups`，返回创建后的 group 结构（含 `id`、`name`、`created_at`）。
- push 频道通过 `PushCenter` / `useReplayPush` hook 订阅 `replay_groups_update`，widget 层无需感知。

## CLI / Web / Admin API

### Admin API

- `POST /_bifrost/api/replay/groups`：body `{ "name": "..." }`。
- `PATCH /_bifrost/api/replay/groups/:id`：重命名。
- `DELETE /_bifrost/api/replay/groups/:id`：删除。
- 广播频道：`replay_groups_update` 全量快照。

### Web UI

Replay 页面左侧分组树、`New Group` 按钮不变；仅内部去重逻辑修改。

### CLI

Replay 分组当前无 CLI 子命令；不新增。

## Sync 边界

Replay 分组是本地管理端状态，不参与规则 Sync；push 广播是同一台机上 Web / 桌面客户端之间的通道，不跨设备同步。

## 实现切分

### Phase 1：store upsert

- 修改 `useReplayStore.createGroup` 逻辑。
- 抽出 `mergeGroupsById` 帮助函数（可选，让 rename、push handler 也可以复用）。

### Phase 2：其他写入路径审计

- 检查 `renameGroup`、`deleteGroup`、`moveRequestToGroup` 是否有类似“append 后被覆盖”问题；若有，同步转 upsert。

### Phase 3：Playwright 回归

- 新增 `admin-replay.spec.ts` 用例：新建分组后断言 `[data-group-id]` 只有 1 个。
- 覆盖“先断开 push 再新建”与“push 与 POST 双到”两种时序。

### Phase 4：文档

- human_tests 更新 Replay 分组场景描述（如已有对应用例，注明 dedup 已修复）。

## 测试方案

### Playwright UI

- `web/tests/ui/admin-replay.spec.ts`
  - 相关断言：`await expect(page.locator(`[data-group-id="${replayGroup!.id}"]`)).toHaveCount(1)`（line 91）。
  - 用例步骤：
    1. 打开 `/_bifrost/replay`。
    2. 点击 `New Group`，输入名字，提交。
    3. 通过管理端 API 拿到该 group id。
    4. 断言 `data-group-id={id}` 节点数量为 1。
    5. 断言分组名字与创建时输入一致。
  - 覆盖时序：可通过 `page.waitForResponse` 等待 POST 完成后立即断言，再等待 push 完成后再次断言，两次都是 1。

### store 单测

- 若有 store 单测框架，添加：
  - `createGroup_upserts_when_group_already_exists_from_push`
  - `createGroup_appends_when_group_missing`

### human_tests

- 更新 `human_tests/webui-replay.md` Replay 分组管理章节：说明 push 与 POST 并发到达时列表保持单条。
- 无需更新用例数量。

## Review / Fix / Test 闭环

### 第 1 轮

- 复核目标：分组只出现一次，push / POST 顺序不敏感。
- 复核 diff：`useReplayStore.ts`、Playwright 用例是否覆盖。
- 重点 review：
  - `renameGroup` / `deleteGroup` 是否共享类似模式，避免只修一个入口。
  - push handler 是否会把本地 upsert 后的字段再覆盖，导致视觉抖动。
- 复测：
  - `pnpm --filter web test:ui -- admin-replay`
  - 手工 UI：连续快速点两次 New Group（不同名字），确认无残留重复。

### 第 2 轮

- 复核问题修复。
- 再次跑 Playwright 与手工回归。
- 检查 push 快照重放（例如离线重连）不会破坏 upsert 结果。

## 风险与决策点

- **风险**：如果 push 快照晚到且缺失刚创建的 group（例如后端未来变更为“push 只在广播时刻的 snapshot”），upsert 无法阻止本地领先。缓解：以后端后续 snapshot 为最终一致源，若需强一致再补 refetch。
- **风险**：`group.id` 若为空字符串或未定义，`some` 判断会误合并。缓解：`replayApi.createGroup` 严格要求返回带 id 的 group；类型层用 `Group & { id: string }` 保护。
- **决策**：不改动后端；后端行为已经幂等，问题只在前端。
- **决策**：不引入 debounce / 手动锁；upsert 已经足够，简单可测。

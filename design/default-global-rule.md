# 全局默认规则 Default 设计方案

## 背景

当前 Bifrost 的主端口加载所有 enabled 的本地规则和有效 Group 规则；临时端口通过 `bifrost port bind/update` 绑定显式规则集，设计上不继承主端口 enabled-rule view。这个模型适合隔离调试端口，但缺少一个所有端口都共享的兜底规则层。

新增系统保留规则 `Default`，用于承载全局默认配置，例如所有域名统一走某个 DNS 解析服务、通用 header、TLS/CONNECT 默认策略或其他必须跨端口生效的基础规则。

## 用户目标验证清单

### 必须实现

- 首次启动或首次访问规则存储时自动初始化一个名为 `Default` 的规则。
- 如果磁盘上的 `Default.bifrost` 被删除，下一次启动或规则入口访问必须自动恢复。
- `Default` 在 Rules 列表置顶展示。
- `Default` 在主端口和临时端口的规则合并输入中始终排在最前面，active summary/网络导出也按此顺序展示。
- `Default` 规则文件默认 enabled，且后续不允许被停用。
- `Default` 不允许删除、不允许重命名、不允许被普通 reorder 移走置顶位置。
- `Default` 是大小写不敏感的系统保留名，`default`、`DEFAULT`、`DeFaUlT` 都不能作为普通规则创建或重命名目标。
- 用户可以编辑和保存 `Default` 的规则内容。
- `Default` 中的规则在主端口和所有临时端口上都生效。
- 临时端口仍保留显式绑定规则隔离语义，但额外继承 `Default`。

### 必须不破坏

- 普通规则的创建、删除、启停、重命名、排序、分享、导入导出继续工作。
- Group 规则不自动获得名为 `Default` 的特殊语义；特殊语义只作用于本地 My Rules 的保留规则。
- 临时端口绑定的 `--rule`、`--group-rule`、`--rule-file`、`--rule-text` 仍按原规则解析和热更新。
- 规则解析、inline values、`@rule` 引用、syntax check、traffic matched rules 展示保持可诊断。
- 已有数据目录里如果已经存在 `Default.bifrost`，应收编为系统 Default，而不是创建重复规则。

### 必须真实验证

- CLI 真实命令验证 `Default` 存在、不可删、不可停用、可编辑。
- Web UI 真实浏览器验证 `Default` 置顶、删除/停用/重命名/拖拽入口被禁用，编辑保存可用。
- 主端口和临时端口真实代理请求都命中 `Default`。
- 临时端口显式规则仍能覆盖或补充自己的端口规则，不被 Default 误吞。

## 产品语义

### Default 是系统保留规则，不是普通用户规则

`Default` 的身份由集中常量派生：

```rust
pub const DEFAULT_RULE_NAME: &str = "Default";
```

第一版不要求修改 `.bifrost` 文件格式。后端和前端通过大小写不敏感的规则名判断是否为系统 Default，并在 API 响应中返回显式能力标志：

```json
{
  "name": "Default",
  "enabled": true,
  "sort_order": -1000000000,
  "rule_count": 0,
  "is_system": true,
  "is_global_default": true,
  "can_delete": false,
  "can_disable": false,
  "can_rename": false,
  "can_reorder": false,
  "can_edit_content": true
}
```

这样实现迁移成本最低，已有 `Default.bifrost` 可以被自动收编。后续如果需要更强的跨名称标识，再扩展 `[meta] kind = "global_default"`，但第一版不做。

### 置顶是展示、管理与诊断合并顺序

Rules 列表必须把 `Default` 固定在顶部，因为它是全局配置入口。执行时应把它理解为所有 resolver 的基础规则集。所有主端口与临时端口的规则输入数组、active summary 和网络导出都必须先放 `Default`，再放普通 enabled 规则或临时端口显式绑定规则。

当前 `RulesResolver` 会按 matcher priority 排序，具体规则比泛匹配规则更优先，`@important` 仍按现有语义提升优先级。因此 `Default` 第一主要用于基础层加载与诊断顺序，不绕过现有 matcher specificity。若 `Default` 中配置 `*.example.com`，临时端口显式绑定 `api.example.com` 时，应由现有 matcher specificity 让更具体规则获胜。

### Default 不能停用，但内容可以为空

`Default.enabled` 始终为 `true`。用户如果暂时不需要全局默认配置，可以把内容清空或仅保留注释。规则语法内部已有的行级 `@disabled` 语义仍然保留，避免破坏现有规则语言。

## 初始化与迁移

新增集中入口：

```rust
impl RulesStorage {
    pub fn ensure_default_rule(&self) -> Result<RuleFile>;
    pub fn is_default_rule_name(name: &str) -> bool;
}
```

初始化策略：

1. 在 `RulesStorage::new()` 或更保守的上层启动路径调用 `ensure_default_rule()`。
   - `ensure_default_rule()` 同时负责恢复被手工删除的 `Default.bifrost`。
2. 如果 `Default` 不存在，创建空内容规则：
   ```text
   # Global default rules.
   # These rules are always enabled and apply to every proxy listener.
   ```
3. 如果 `Default` 已存在：
   - 保留用户已有 content、created_at、sync rule_id。
   - 强制 `enabled = true`。
   - 强制 `sort_order = DEFAULT_RULE_SORT_ORDER`，例如 `-1_000_000_000`。不要使用 `i32::MIN`，因为现有普通新建规则逻辑会基于最小 `sort_order - 1` 计算新优先级，直接使用 `i32::MIN` 有溢出风险。
   - 清理不应该存在的 remote sync 状态，避免作为普通规则同步。
4. 每次 `list/load_enabled/load_all` 之前不做昂贵迁移；只在进程启动、Admin API 初始化、CLI rule 子命令入口和 import 完成后做一次 ensure。

推荐调用点：

- `crates/bifrost-cli/src/commands/start.rs`：启动主代理前。
- `crates/bifrost-admin/src/handlers/rules.rs`：Rules API list/get/update 入口前。
- `crates/bifrost-cli/src/commands/rule.rs`：CLI rule list/show/update/delete/enable/disable 前。
- 导入规则后的保存路径：导入后再 normalize Default。

## 存储层保护

必须把保护放在 storage/admin/CLI 三层，不能只靠 Web UI。

`RulesStorage` 增加保护方法或在现有方法中拦截：

- `delete("Default")` 返回 `BifrostError::Config("Default rule cannot be deleted")`。
- `rename("Default", ...)` 返回 `Default rule cannot be renamed`。
- `rename(..., "default")`、`create("default")` 等大小写变体返回保留名错误。
- `set_enabled("Default", false)` 返回 `Default rule cannot be disabled`。
- `reorder(order)` 忽略对 `Default` 的移动，并保证其他规则排序从 `0` 或 `1` 开始，Default 保持固定置顶。
- `save(&RuleFile { name: "Default", enabled: false, ... })` 自动归一化为 enabled true，避免导入或旧代码路径绕过。
- 普通 create 计算最高优先级时必须排除 Default，避免把系统规则的保留 `sort_order` 当成用户规则排序基准。

`RuleSummary` 与 `RuleFileDetail` 增加能力字段。Web 类型 `RuleFile`、`RuleFileDetail` 同步扩展这些字段，默认值向后兼容。

## 规则加载与运行时合并

新增统一规则选择器，替代主端口和临时端口各自拼装规则的隐式逻辑：

```rust
pub enum RuleSelection {
    MainEnabledRules,
    TemporaryExplicitRefs(Vec<RuleSetRef>),
}

pub struct LoadedRuleFiles {
    pub default_rule: Option<RuleFile>,
    pub selected_rules: Vec<RuleFile>,
}
```

加载流程：

1. 调用 `ensure_default_rule()`。
2. 读取 `Default`，只要存在就加入 `default_rule`，不检查 enabled。
3. 主端口 `MainEnabledRules`：加载除 Default 以外的 enabled 本地规则和 Group 规则。
4. 临时端口 `TemporaryExplicitRefs`：加载端口显式绑定的规则来源。
5. 合并顺序用于 active summary 和诊断展示：`Default` 在前，端口/主规则在后。
6. 解析成 `Rule` 时给 Default 的 source label 设置为 `Default` 或 `global:Default`，traffic matched rules 能看出命中来源。

当前 `DynamicRulesResolver::new(cli_rules, stored_rules, values)` 的输入可以保持不变，只是 `stored_rules` 改为：

- 主端口：`parsed(Default) + parsed(enabled non-default rules and groups)`
- 临时端口：`parsed(Default) + parsed(explicit port bound rules)`

热更新策略：

- 更新 `Default` 内容后，主端口和所有临时端口 resolver 都必须重建。
- 普通 enabled 规则启停只影响主端口。
- 临时端口绑定规则内容更新只影响绑定它的临时端口，同时仍保留最新 Default。
- 删除普通规则导致临时端口 degraded 的现有语义不变；Default 不可删除，因此不产生 Default missing degraded 状态。

## CLI 交互

### rule list

`bifrost rule list` 输出：

```text
Rules (3):
  Default [enabled, global, protected]
  local-dev [enabled]
  mock-login [disabled]
```

JSON 输出如果已有或后续补齐，应包含 `is_global_default/can_delete/can_disable/can_rename/can_reorder/can_edit_content`。

### rule show

`bifrost rule show Default` 输出：

```text
Rule: Default
Status: enabled
Scope: global default, applies to all proxy listeners
Protection: cannot delete, disable, rename, or reorder
Content:
...
```

### rule update

允许：

```bash
bifrost rule update Default --content "*.corp.test host://10.0.0.53"
bifrost rule update Default --file ./global-default.bifrost
```

保存时仍执行 syntax validation。`--allow-invalid` 语义保持一致，但保存 invalid Default 有更高风险，CLI 应输出明确 warning：

```text
Warning: Default applies to every listener. Invalid or broad rules can affect all temporary ports.
```

### 禁止操作

以下命令失败，退出码使用现有业务错误语义：

```bash
bifrost rule delete Default
bifrost rule disable Default
bifrost rule rename Default NewDefault
```

错误消息必须稳定、可测试：

```text
Default rule cannot be deleted.
Default rule cannot be disabled.
Default rule cannot be renamed.
```

### 临时端口命令

`bifrost port active <port>` 需要显示 Default：

```text
Active rules for temporary port 18888:
  Default [global]
  local-debug [bound]
```

`bifrost port bind/update --rule Default` 第一版建议返回 bad request：

```text
Default is applied automatically to every temporary port and cannot be bound explicitly.
```

这样避免 active summary 中重复出现 Default。若兼容性需要，也可以接受但去重并输出 warning；第一版推荐拒绝，行为更清晰。

## Web UI 交互

### Rules 列表

- `Default` 固定置顶，不受搜索排序模式影响。搜索关键词不匹配时可以保留置顶但降低视觉权重，避免用户误以为 Default 消失。
- Default 行展示 `Global` 或 `Default` 标签，enabled 图标常亮。
- Switch 禁用或隐藏，tooltip：`Default is always enabled`。
- 右键菜单隐藏或禁用 Delete、Rename、Disable，仅保留 Copy、Export、Share 视产品决策。
- 拖拽排序时 Default 不可拖拽，也不可作为其他规则 drop 到其前面的目标。
- 批量选择删除时自动排除 Default，并提示 `Default cannot be deleted`。

### RuleEditor

- 标题显示 `Default` 和 `Global default` 标签。
- Monaco 编辑器可编辑。
- Save 按钮可用。
- Delete 按钮隐藏或 disabled，tooltip：`Default cannot be deleted`。
- metadata 展示 `Applies to all listeners`，替代普通 sync 状态的主视觉，sync 状态可以放次要位置或不展示。
- 保存 Default 前若内容包含非常宽泛规则，例如 `*`、`**`、`*.com`、`http://*`，可以先不做阻断，但在 syntax warning 中提示影响范围。第一版不新增复杂 lint，只保留后续扩展点。

### 空状态

首次打开 Rules 页面时即使用户从未创建过规则，也应看到 Default。右侧默认选中 Default，避免出现空规则列表。

## Admin API

扩展响应结构：

```rust
struct RuleFileInfo {
    name: String,
    enabled: bool,
    sort_order: i32,
    rule_count: usize,
    created_at: String,
    updated_at: String,
    is_system: bool,
    is_global_default: bool,
    can_delete: bool,
    can_disable: bool,
    can_rename: bool,
    can_reorder: bool,
    can_edit_content: bool,
}
```

相同字段加入 `RuleFileDetail`。

API 行为：

- `GET /api/rules`：ensure Default 后返回，Default 固定第一项。
- `GET /api/rules/Default`：返回 detail。
- `POST /api/rules { name: "Default" }`：如果 Default 已存在返回 409；如果不存在则走 ensure，不允许用普通 create 语义创建不同保护属性。
- `PUT /api/rules/Default`：只允许 content 变化；`enabled=false` 返回 400 或忽略并返回 warning。推荐返回 400，便于调用方修正。
- `DELETE /api/rules/Default`：400。
- `PUT /api/rules/Default/disable`：400。
- `PUT /api/rules/Default/enable`：幂等成功。
- `PUT /api/rules/Default/rename`：400。
- `PUT /api/rules/reorder`：请求里含 Default 时忽略 Default 的位置并仍保持置顶；也可以返回 400。推荐忽略位置并返回成功，因为前端 reorder 可能带全量列表。

## Sync、导入导出与分享

### Sync

Default 是本地系统规则，不应作为普通远端共享规则自动同步。原因：

- 它是每台设备的数据目录级默认策略，远端多人协作规则不应隐式修改本机所有端口。
- 远端如果也有名为 Default 的普通规则，语义会冲突。

实现方式：

- rule sync 枚举本地规则时过滤 `is_default_rule_name(rule.name)`。
- 如果历史上已经同步过 Default，迁移时清理本地 `sync.remote_id` 等字段，并不再推送。
- Group 同步不受影响。

### 导入

导入 `.bifrost` 中包含名为 `Default` 的规则时，按“更新 Default 内容”处理，而不是创建普通规则。导入完成后强制 enabled true 和 protected flags。

### 导出

允许用户手动导出 Default 用于备份。导出的文件重新导入时仍被识别为系统 Default。

### Share URL

第一版建议允许创建 Default 的 share URL，但导入方只把它作为普通分享预览规则，除非用户明确选择“导入为 Default”。如果当前 share import 没有交互选择能力，则先禁止分享 Default，避免别人点击链接后意外覆盖全局配置。

## 临时端口语义更新

现有文档写着临时端口“不继承主端口当前启用的规则集合”。新语义应改为：

- 临时端口不继承主端口的普通 enabled-rule view。
- 临时端口始终继承系统 Default。
- 临时端口显式绑定规则仍是该端口的专用规则选择器。

示例：

```bash
bifrost rule update Default --content "*.corp.test dns://10.0.0.53"
bifrost port bind --port 18888 --rule local-debug
bifrost port active 18888
```

输出应包含：

```text
Default [global]
local-debug [bound]
```

## 实现切分

### Phase 1：存储与 API 保护

- 新增 Default 常量和 `ensure_default_rule()`。
- 扩展 Rule summary/detail capability 字段。
- Admin API/CLI 拦截 delete/disable/rename/reorder。
- list/get/update 覆盖 Default 自动初始化。
- 单元测试覆盖 storage 和 handler 错误。

### Phase 2：运行时生效

- 抽出统一 rule selection loader。
- 主端口 resolver 注入 Default。
- 临时端口 resolver 注入 Default。
- rules watcher 更新 Default 时刷新所有 resolver。
- port active/active-summary 标记 Default 来源。

### Phase 3：Web UI

- Rules list 固定置顶 Default。
- 禁用/隐藏 Default 的删除、停用、重命名、拖拽和批量删除入口。
- RuleEditor 保留编辑保存，隐藏删除。
- Playwright 覆盖列表、编辑器、不可操作按钮状态。

### Phase 4：文档与迁移边界

- 更新 `design/temporary-port-rule-bindings.md` 中临时端口继承语义。
- 更新 README/docs/rule 或 CLI help。
- 更新 human_tests 与 E2E。
- 清理 sync/import/share 的 Default 边界。

## 测试方案

### 单元测试

- `RulesStorage::ensure_default_rule_creates_enabled_protected_default`
- `RulesStorage::ensure_default_rule_normalizes_existing_default`
- `RulesStorage::delete_default_is_rejected`
- `RulesStorage::set_enabled_default_false_is_rejected`
- `RulesStorage::rename_default_is_rejected`
- `RulesStorage::reorder_keeps_default_first`
- `load_stored_rules_includes_default_even_when_no_other_rules`
- `load_bound_rules_for_temp_port_includes_default_once`

### E2E 测试

新增或扩展 `e2e-tests/tests/test_temporary_port_bindings.sh`：

- 启动临时数据目录，首次 rule list 能看到 Default。
- 修改 Default 为 `global-default.test status://218 resBody://(global-default)`。
- 主端口请求 `global-default.test` 返回 `global-default`。
- 绑定临时端口到普通规则后，临时端口请求 `global-default.test` 也返回 `global-default`。
- `bifrost port active <port>` 包含 Default 和端口绑定规则。
- `bifrost rule delete/disable/rename Default` 失败且错误可读。

新增 Web UI Playwright：

- Rules 页面首次打开 Default 置顶且自动选中。
- Default 的 Switch disabled，Delete/Rename context menu 不可用。
- Default 编辑保存后刷新仍保留内容。
- 拖拽其他规则不能移动到 Default 前。

### 真实场景测试 human_tests

新增 `human_tests/default-global-rule.md`：

- TC-DGR-01：首次启动自动创建 Default，列表置顶。
- TC-DGR-02：CLI 禁止删除/停用/重命名 Default，但允许更新内容。
- TC-DGR-03：Web UI 禁止删除/停用/重命名/拖拽 Default，允许编辑保存。
- TC-DGR-04：主端口命中 Default。
- TC-DGR-05：临时端口命中 Default，同时端口绑定规则继续生效。
- TC-DGR-06：已有数据目录中存在 `Default.bifrost` 时被收编并强制 enabled。

所有服务启动必须使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1` 和 `--no-system-proxy`。

### 覆盖率与项目校验

实现阶段应运行：

- `cargo fmt --all -- --check`
- 相关 crate 单元测试：`cargo test -p bifrost-storage default_rule`、`cargo test -p bifrost-admin default_rule`、`cargo test -p bifrost-cli default_rule`
- 相关 E2E：临时端口绑定脚本和 Web UI Rules 用例
- `cargo test --workspace --all-features`
- `rust-project-validate`

本机当前有 no-local-coverage 约定时，不运行 `make coverage` / `make coverage-unit`；交付时说明 coverage 本地豁免，并依赖其他本地验证与远端 CI。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：Default 自动初始化、置顶、不可删除、不可停用、默认启用、可编辑、临时端口全局生效。
- 复核 diff：storage/admin/CLI/Web/runtime/doc/human_tests 是否都覆盖。
- 重点 review：是否存在绕过保护的 API 或 CLI 路径；临时端口是否重复注入 Default；sync/import 是否把 Default 当普通规则。
- 复测：storage/admin/CLI 单元测试、临时端口 E2E、Web UI targeted test。

### 第 2 轮

- 复核第 1 轮发现问题的修复。
- 再次检查 `git status --short`、`git diff`、新增文件和 human_tests 索引。
- 重点 review：Default 更新后所有 resolver 热更新是否一致；错误消息是否稳定可测试；文档是否同步改掉“临时端口完全不继承任何默认规则”的旧表述。
- 复测：失败路径重跑，必要时补充真实 CLI/Web 操作。

## 风险与决策点

- `Default` 是否参与 Sync：本方案建议不参与自动 sync。如果产品希望多设备共享全局默认规则，需要单独设计“个人全局默认配置同步”，不能复用普通规则 sync。
- `Default` share URL 是否允许：第一版建议禁止或显式导入确认，避免外部链接覆盖全局配置。
- 匹配冲突：Default 作为全局基础层可能很宽泛，应依赖现有 matcher priority 和更具体规则覆盖；如后续发现宽泛规则仍过强，再引入 Default-specific lint 或低优先级 source tier。
- 迁移边界：已有 `Default.bifrost` 会被收编，用户如果原本把它当普通规则且想删除，需要改名后再删除。实现时可在首次收编日志里提示。

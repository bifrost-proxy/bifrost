# Temporary Port Rule Bindings

## 功能目标

新增一类临时代理端口绑定能力：用户可以通过 CLI 创建一个额外监听端口，并把该端口绑定到一个明确选择的规则集列表。所有端口共享同一个 Bifrost 数据目录、配置、证书、values、scripts、traffic 存储和 Group 本地缓存；差异只在“这个监听端口使用哪些转发规则集”。临时端口生命周期独立于主端口，用完可以销毁；主端口不中断、不重启、不继承临时端口绑定，临时端口也不会自动继承主端口当前启用的规则集合。

核心体验：

```bash
bifrost port bind --port 18888 --rule local-dev --group-rule 7152084678483132446/abc
bifrost port list
bifrost port show 18888
bifrost port destroy 18888
```

推荐把这组能力命名为 `port`，而不是 `rule port` 或 `start --extra-port`。原因是用户心智是“开一个隔离端口”，规则集只是这个端口的绑定配置；销毁动作也自然落在端口对象上。

## 当前工程现状

### 主端口启动路径

当前 CLI 在 `crates/bifrost-cli/src/main.rs` 解析 `start` 参数后进入 `run_start`。`run_start` 负责：

- 读取 `BIFROST_DATA_DIR` 下的配置和规则存储。
- 处理 `--rules` / `--rules-file` 得到 CLI 规则。
- 创建 `ProxyConfig`，其中 `port` / `host` / TLS / access / badge / socks 等配置是端口级监听参数。
- 创建 `AdminState`，并把 `RulesStorage`、`ConfigManager`、traffic DB、push、metrics 等运行态能力挂进去。
- 调用 `load_stored_rules(...)` 加载已启用的本地规则和有效 Group 子目录规则。
- 创建 `DynamicRulesResolver::new(cli_rules, stored_rules, values)`，再用 `ProxyServer::new(config).with_rules(resolver)` 启动主监听。

### 规则加载与热更新

规则解析集中在 `crates/bifrost-cli/src/parsing/rules.rs`：

- `parse_cli_rules(...)` 只解析启动命令给的 inline/file 规则。
- `DynamicRulesResolver` 内部持有一个 `CoreRulesResolver`，由 `cli_rules + stored_rules` 构成。
- `update_stored_rules(...)` 热更新时保留 CLI 规则，替换存储规则。

存储规则加载在 `crates/bifrost-cli/src/commands/start.rs`：

- `load_stored_rules(...)` 从 `RulesStorage::load_enabled_with_subdirs_filtered(...)` 读取启用规则。
- 当前 Group 规则通过 `rules/<group-dir>/` 子目录进入同一个规则流。
- `spawn_rules_watcher_task(...)` 收到 `RulesChanged` / `ValuesChanged` 后重新加载同一批启用规则并更新主 resolver。

### Admin 与状态展示

`crates/bifrost-admin/src/handlers/rules.rs` 的 `/api/rules/active-summary` 当前展示的是主端口的合并规则摘要；`AdminState::refresh_badge_rules_cache()` 同样按主规则存储汇总 badge 数据。`bifrost rule active` 和 `bifrost status` 都读取主端口上的 active-summary。

这意味着临时端口不能简单复用主 active-summary，否则用户会看到“主端口启用了哪些规则”，但无法回答“18888 绑定了哪些规则”。

## 设计原则

1. 临时端口是主进程内的受管监听，不是再启动一个完整 Bifrost daemon。
2. 所有端口共享同一个数据目录和运行态基础设施；不创建临时数据目录，不复制规则文件，不维护第二套 values/scripts/certs。
3. 每个端口拥有自己的规则选择器和 `DynamicRulesResolver` 视图；resolver 的输入来自同一份规则存储中被端口绑定选中的规则集。
4. 临时端口绑定是显式、多选、可审计的配置对象。
5. 销毁临时端口只关闭该监听和对应 resolver/watch 任务，不触碰主监听，也不删除任何共享规则数据。
6. 初期只支持在已有主代理运行时通过 admin API 管理临时端口；主代理停止后临时端口全部消失，但共享数据目录内的规则、values、scripts、证书不受影响。

## 用户模型

### 临时端口绑定对象

```rust
pub struct TemporaryPortBinding {
    // Note (2026-06-17): the shipped struct in `crates/bifrost-admin/src/temp_ports.rs`
    // omits `id` and `inherit_values`; values are always inherited from shared storage,
    // and the port number itself is used as the binding key. The fields below are kept
    // here as the original design surface.
    pub id: String, // (planned, not yet shipped as of 2026-06-17)
    pub port: u16,
    pub host: String,
    pub name: Option<String>,
    pub status: TemporaryPortStatus,
    pub rule_refs: Vec<RuleSetRef>,
    pub inherit_values: bool, // (planned, not yet shipped as of 2026-06-17)
    pub created_at: i64,
    pub updated_at: i64,
}

pub enum RuleSetRef {
    LocalRule { name: String },
    GroupRule { group_id: String, name: String },
}
```

第一版只绑定已存在于共享数据目录中的本地规则和 Group 规则，不引入 `InlineRule` / `RulesFile` 这类端口私有规则来源。这样用户看到的“规则数据真相”只有一份：`data_dir/rules/` 和 Group 规则子目录。

2026-05 完备性验收补充：CLI/API 也支持一次性绑定规则文件和规则原文，用于临时排查或不希望写入共享规则目录的场景。该输入形态仍不创建第二套数据目录，也不修改 `data_dir/rules/`：

```rust
pub enum RuleSetRef {
    LocalRule { name: String },
    GroupRule { group_id: String, name: String },
    RuleFile { path: String },
    InlineRule { content: String },
}
```

`RuleFile` 在主进程内按路径读取并解析，错误必须包含文件路径和底层 IO/解析原因；`InlineRule` 直接解析请求体中的规则原文，错误必须包含可定位的行列和 parser message。二者只存在于临时端口 binding 内存对象中，不持久化为本地规则文件。

`inherit_values` 默认 `true`，表示临时端口仍使用共享数据目录中的 values 和规则文件内 inline values。它不表示继承主端口规则集合。当前实现（2026-06-17）尚未暴露这个开关（planned, not yet shipped as of 2026-06-17），临时端口始终继承共享 values。

### CLI 命令

```bash
# 创建并绑定一个临时端口
bifrost port bind \
  --port 18888 \
  --name nextagent-local \
  --rule local-dev \
  --rule mock-login \
  --group-rule 7152084678483132446/abc

# 使用自动分配端口
bifrost port bind --port 0 --rule local-dev
bifrost port bind --port 0 --rule-file ./rules/temp-debug.bifrost
bifrost port bind --port 18890 --rule-text "debug.test status://218 resBody://(debug)"

# 查看临时端口
bifrost port list
bifrost port show 18888
bifrost port active 18888

# 修改绑定规则集，监听端口不变
bifrost port update 18888 --rule local-dev --group-rule 7152084678483132446/abc

# 销毁端口绑定
bifrost port destroy 18888
```

文档与帮助要求：

- 顶层 `bifrost --help` 必须明确列出 `port` 命令，说明其用于“管理绑定到显式规则集的临时代理端口”。
- `bifrost port --help` 必须说明多端口模型：主端口继续承载默认启用规则，临时端口只承载 `bind/update` 时显式绑定的规则引用，并给出典型工作流示例。
- `bifrost port bind --help` / `bifrost port update --help` 必须解释四类规则来源：`--rule`、`--group-rule`、`--rule-file`、`--rule-text`，并提示至少需要提供一种。
- `docs/cli.md` 需要提供独立的“临时端口规则绑定”章节，覆盖命令清单、常见工作流、排障方式，以及 `traffic list/get` 中 `listener_port` / `lp` 的关联说明。
- 根目录 `SKILL.md`（供 `bifrost install-skill` 安装）必须增加多端口使用手册，指导用户如何在一个主代理进程下灵活绑定多个临时端口到不同规则集合。

建议 `--rule` 只引用共享数据目录中的本地规则名，`--group-rule` 使用 `<group_id>/<rule_name>`，避免本地规则名和 Group 规则名歧义。`--rule-file` 与 `--rule-text` 是端口私有的一次性规则输入，不进入共享规则列表，也不受默认 enabled/disabled 状态影响。后续可以补 `--all-rules-in-group <group_id>`，但第一版不做，避免“多选”变成隐式全选。

### 行为断言

- 主端口可以被理解为默认规则选择器：选择所有当前全局启用的本地规则和启用的 Group 规则。
- 临时端口是额外规则选择器：只选择 `port bind` 显式绑定的规则引用。
- 临时端口的流量完全不受默认规则启用状态影响。默认规则无论 enabled 还是 disabled，只要没有被该临时端口显式绑定，都不能进入该临时端口 resolver。
- 主端口启用/禁用规则不会影响临时端口，除非临时端口绑定的同名共享规则文件内容本身被更新。
- 临时端口绑定规则的启用状态不读取全局 `enabled`，因为绑定本身就是启用声明。
- 临时端口规则排序按 CLI 传入顺序执行；同一个规则文件内仍按文件内顺序解析。
- 同一端口不能重复绑定；端口冲突返回明确错误。
- 不能绑定 `9900` 作为测试端口；产品逻辑只做端口冲突校验，测试和文档明确避开正式端口。
- 临时端口不配置系统代理，不写 shell rc，不影响 `bifrost status` 的主端口 PID/port。
- `bifrost rule add/update/delete` 操作的仍是共享规则数据；端口绑定只保存规则引用关系，不保存规则副本。

## 运行时架构

新增 `TemporaryPortManager`，挂在主进程的 `AdminState` 或其相邻运行态容器中。

```text
main ProxyServer :9900
  ├─ AdminState
  │   ├─ RulesStorage
  │   ├─ ValuesStorage
  │   ├─ TrafficDbStore
  │   └─ TemporaryPortManager
  │       ├─ binding 18888 -> ProxyServer + port-scoped DynamicRulesResolver
  │       └─ binding 18889 -> ProxyServer + port-scoped DynamicRulesResolver
  └─ main DynamicRulesResolver
```

每个临时端口启动一个受管任务：

1. 根据 `RuleSetRef` 解析绑定规则。
2. 从同一个 `RulesStorage` / Group 规则目录读取规则内容。
3. 合并 values：共享 values + 被绑定规则文件中的 inline values。
4. 创建端口级 `DynamicRulesResolver::new([], bound_rules, values)`，它是共享数据的规则视图，不是独立数据域。
5. 复制主端口的安全/网络配置，生成新的 `ProxyConfig { port, host, socks5_port: None, enable_socks: true, ... }`。
6. 复用主进程的 `TlsConfig`、`AccessControl`、`TrafficDbStore`、`ConnectionRegistry`、`AdminState` 中可共享资源。
7. 调用 `ProxyServer::run_with_listener_ready(...)` 启动监听。

关键点是规则选择隔离、数据共享：临时端口不调用“加载全部已启用规则”的 `load_stored_rules(...)`，也不能在 `EnabledGlobalRules` 结果上追加端口规则；它必须调用新的 `load_bound_rules(rule_refs)`，只从同一份规则存储中取被绑定的规则。

因此实现上推荐把现有“读取全局 enabled 规则”的逻辑也收敛成一个 `RuleSelection`（planned, not yet shipped as of 2026-06-17）：

```rust
pub enum RuleSelection {
    EnabledGlobalRules,
    ExplicitRefs(Vec<RuleSetRef>),
}
```

主端口使用 `EnabledGlobalRules`，临时端口使用 `ExplicitRefs(...)`。两者共享同一个 loader、parser、values 合并和 resolver 更新流程，只是选择规则文件的入口不同。当前实现里主端口走 `load_stored_rules(...)`，临时端口走 `CliTemporaryPortManager::load_bound_rules(...)`，两条路径尚未收敛到统一枚举。

## 规则引用解析

新增一个规则集解析模块，例如 `crates/bifrost-admin/src/temp_ports/rules.rs`。解析必须发生在主进程内，因为主进程已经持有共享 `RulesStorage`、Group name cache、values storage 和 config manager。

解析逻辑：

- `LocalRule { name }`：从共享 `RulesStorage` 加载 `rules/<name>.bifrost`。
- `GroupRule { group_id, name }`：通过 group name cache 找到本地 group dir，再从 `rules/<group-dir>/<name>.bifrost` 加载。
- 所有引用不存在时，创建失败并列出缺失项，不启动半成品端口。
- 解析失败时返回具体文件名、行列和 parser error，和主规则加载日志保持一致。

建议新增 `RuleSourceLabel` 写入 `Rule.file`，格式为：

- `local:<name>`
- `group:<group_id>/<name>`

这样 `traffic get` 的 `matched_rules` 能解释请求究竟命中了哪个绑定规则集。

## 热更新策略

第一版采用“共享规则内容热更新，绑定集合不隐式变化”：

- 如果 `local-dev` 被绑定到 18888，用户更新 `local-dev` 内容后，18888 自动重载。
- 如果用户只是启用/禁用 `local-dev`，18888 不受影响，因为绑定集合不看全局 enabled。
- 如果用户启用或禁用任何未绑定规则，18888 不重载、不新增命中，也不改变 active-summary。
- 如果用户删除已绑定规则，18888 标记为 `degraded`，保留端口但该规则从 resolver 中移除，同时 `port show` 展示缺失引用；也可以通过 `port update` 修复。删除操作仍然只删除共享规则数据本身，不需要清理端口私有副本。
- 如果用户改 Group 名称映射或同步导致 group dir 变化，manager 重新解析 `group_id` 到本地目录。

这比“每次主规则启用状态变化都跟随”更符合隔离端口心智。

## Admin API

新增路径：

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/api/ports` | 列出临时端口绑定 |
| POST | `/api/ports` | 创建临时端口绑定 |
| GET | `/api/ports/:port` | 查看端口绑定详情 |
| PATCH | `/api/ports/:port` | 更新绑定规则集 |
| DELETE | `/api/ports/:port` | 销毁端口绑定 |
| GET | `/api/ports/:port/active-summary` | 查看该临时端口的 active summary |

CLI 通过当前运行主端口的 admin API 操作，因此 `bifrost port bind` 需要主代理已运行。若未运行，提示：

```text
No running Bifrost proxy found. Start the main proxy first:
  bifrost start --no-system-proxy
```

## 持久化选择

第一版建议默认“进程内临时绑定关系”，不落盘：

- 符合“临时端口，用完销毁”的定位。
- 避免主进程重启后自动占用一批旧端口。
- 减少与 daemon pid/runtime metadata 的兼容面。
- 注意：不落盘的是“端口绑定关系”，不是规则、values、scripts、证书等共享数据。这些仍然全部来自并保存在当前 `BIFROST_DATA_DIR`。

后续若需要可恢复，再增加（planned, not yet shipped as of 2026-06-17）：

```bash
bifrost port bind --port 18888 --rule local-dev --persist
bifrost port restore
```

持久化文件可放在 `data_dir/runtime/temp_ports.json`，只保存端口到规则引用的映射，与规则文件分离。当前实现只在主进程内存中维护临时端口绑定，没有 `--persist` 标志，也没有 `port restore` 子命令。

## Web UI 范围

第一版只做 CLI + Admin API。Web UI 可以只在状态页或规则页展示只读入口：

- 当前主端口。
- 临时端口列表。
- 每个端口绑定的规则集摘要。

如果后续做 Web UI 创建/销毁，必须按 WebUI 双主题规范补齐亮色/暗色验证。

## 实现阶段拆分

### Phase 1: 模型与 CLI/API 骨架

- `crates/bifrost-cli/src/cli.rs` 增加 `PortCommands`。
- `crates/bifrost-cli/src/commands/port.rs` 增加 admin client 调用与输出格式。
- `crates/bifrost-admin/src/handlers/ports.rs` 增加 CRUD 路由。
- `AdminState` 持有 `TemporaryPortManager`。

### Phase 2: 运行时受管监听

- 新增 `TemporaryPortManager`，管理 binding map、listener task、shutdown channel。
- 抽出主启动里可复用的 `build_proxy_server_for_config(...)` 或等价 builder，避免复制 `ProxyServer` 拼装逻辑。
- 临时端口使用端口级 resolver 视图，复用共享 TLS/access/traffic 组件。

### Phase 3: 规则绑定解析与热更新

- 新增 `load_bound_rules(...)`，从共享 `RulesStorage` 和 Group 规则目录读取被绑定规则。
- watcher 中在 `RulesChanged` / `ValuesChanged` 时通知 manager 重载各临时 resolver。
- active-summary 支持按端口返回绑定视角。

### Phase 4: 验证与文档

- 更新 `docs/cli.md` 或 CLI reference。
- 新增/更新 `human_tests/temporary-port-rule-bindings.md` 与 `human_tests/readme.md`。
- 补 E2E 和 unit tests。

## 测试方案

### 单元测试

- `test_parse_port_binding_local_rules`：验证 `--rule a --rule b` 解析为两个 `LocalRule` 且保持顺序。
- `test_parse_port_binding_group_rule_ref`：验证 `<group_id>/<name>` 解析，缺少 `/` 时返回明确错误。
- `test_parse_port_binding_file_and_inline_refs`：验证 `--rule-file` 转为 `RuleFile`、`--rule-text` 转为 `InlineRule`，并保持传入顺序。
- `test_parse_port_binding_requires_any_rule_input`：验证未传任何规则输入时，错误提示同时列出 `--rule`、`--rule-file`、`--rule-text`、`--group-rule`。
- `test_load_bound_rules_uses_shared_rule_storage`：绑定规则从共享 `RulesStorage` 读取，不创建端口私有副本。
- `test_load_bound_rules_ignores_global_enabled_state`：绑定 disabled 规则仍加载，因为绑定本身就是启用声明。
- `test_load_bound_rules_missing_ref_fails_atomically`：多个引用中任一缺失时创建失败且不启动监听。
- `test_temp_port_resolver_does_not_include_main_enabled_rules`：主规则启用但未绑定时，临时 resolver 不命中。
- `test_temp_port_resolver_ignores_default_rule_enabled_toggles`：切换默认规则 enabled/disabled 不改变临时端口 resolver 和 active-summary。
- `test_temp_port_update_replaces_bound_resolver_only`：更新绑定只替换对应端口 resolver，不影响主 resolver。

### E2E 测试

新增 `crates/bifrost-e2e/src/tests/temporary_ports.rs`：

- `temporary_port_uses_only_bound_rules`：启动主代理和临时端口，主端口命中主规则，临时端口只命中绑定规则。
- `temporary_port_destroy_does_not_stop_main_port`：销毁临时端口后，请求临时端口失败，主端口仍可请求成功。
- `temporary_port_multiple_rule_sets_preserve_order`：绑定多个规则集，断言优先级/排序符合传入顺序。
- `temporary_port_rule_content_hot_reload`：更新绑定规则内容后，临时端口生效；启用另一个未绑定规则不生效。
- `temporary_port_ignores_default_rule_enable_disable`：默认规则启用/禁用前后，临时端口命中结果不变。
- `temporary_port_group_rule_binding`：绑定 Group 规则，临时端口命中 Group 规则，主端口不因该绑定新增命中。
- `temporary_port_shares_values_scripts_and_certs`：临时端口规则可使用共享 values/scripts/TLS 证书能力。
- `temporary_port_file_and_inline_inputs`：验证 `--rule-file` 和 `--rule-text` 直接绑定可生效，且不写入共享规则目录。
- `temporary_port_records_listener_port`：验证 Traffic compact API `lp`、Traffic detail `listener_port`、CLI `traffic list/get` 与 Web Traffic list/detail 都能区分主端口和临时端口。
- `temporary_port_records_listener_port_without_rule_hit`：验证没有命中任何规则的直连代理请求也会记录入口监听端口，端口归因不依赖规则命中。
- `temporary_port_cli_error_quality`：验证重复端口、主端口冲突、未传规则、不存在规则、规则文件读取/解析失败、规则原文解析失败均返回可定位错误。
- `temporary_port_bind_script_allocates_distinct_ports`：验证 shell E2E 脚本连续分配 `MAIN_PORT/TEMP_PORT/...` 时不会因为子 shell 丢失 offset 而复用同一端口，也不会与主端口冲突。
- `temporary_port_web_list_rerenders_on_listener_port_change`：验证 Web Traffic 虚拟列表在 `listener_port` 从 compact API 注入后会立即触发行级重渲染，`Port` 列与详情 `Proxy Port` 保持一致。
- `temporary_port_settings_proxy_cards_show_bound_rules`：验证 Settings / Proxy 页面展示每个临时端口卡片，包含端口、绑定规则来源、resolved active rules 与端口级 merged content。
- `temporary_port_traffic_cli_accepts_port_on_list_get`：验证 `bifrost traffic list --port <MAIN_PORT> --format json` 和 `bifrost traffic get --port <MAIN_PORT> <id> --format json` 可被 CLI 正常解析，并继续按指定 admin API 端口查询 traffic。
- `temporary_port_badge_uses_listener_port_rules`：验证通过临时端口代理 HTML 时，Inject Bifrost Badge 中的规则列表和合并规则内容来自该临时端口绑定规则，而不是主端口默认 active summary。

执行命令建议：

```bash
BIFROST_DATA_DIR="$(mktemp -d)" cargo test -p bifrost-e2e temporary_ports -- --nocapture
```

### 真实场景测试 human_tests

实现阶段必须创建 `human_tests/temporary-port-rule-bindings.md`，并同步更新 `human_tests/readme.md`。用例至少包含：

- `TC-TPRB-01`：创建本地规则绑定临时端口，请求临时端口命中绑定规则。
- `TC-TPRB-02`：主端口已有其他启用规则，请求临时端口不命中主端口规则。
- `TC-TPRB-02B`：切换默认规则 enabled/disabled 后，临时端口请求结果和 `port active` 摘要不变。
- `TC-TPRB-03`：临时端口销毁后不可访问，主端口仍然可用。
- `TC-TPRB-04`：多选规则集按绑定顺序生效。
- `TC-TPRB-05`：绑定 Group 规则后只在临时端口生效。
- `TC-TPRB-06`：更新绑定规则内容后临时端口热更新。
- `TC-TPRB-07`：端口冲突或缺失规则引用返回清晰错误且不留下半启动端口。
- `TC-TPRB-08`：临时端口绑定规则使用共享 values/scripts/certs 数据，证明没有创建第二套数据目录。
- `TC-TPRB-09`：自动分配端口与指定端口创建均成功，指定端口冲突返回包含端口号和原因的错误。
- `TC-TPRB-10`：`--rule-file` 和 `--rule-text` 直接绑定成功，请求命中对应规则；文件读取/原文解析失败时错误可定位。
- `TC-TPRB-11`：Traffic list/detail、CLI traffic list/get 和 Web Traffic list/detail 展示主端口与临时端口的监听端口信息。
- `TC-TPRB-11B`：没有命中任何规则的主端口/临时端口直连请求也记录 `listener_port` / compact `lp`，且 `has_rule_hit=false`。
- `TC-TPRB-12`：Web Traffic 页面在 light/dark 主题下端口列和详情 `Proxy Port` 可读。
- `TC-TPRB-06-回归-01`：执行 `e2e-tests/tests/test_temporary_port_bindings.sh`，验证脚本内连续分配出来的各测试端口互不相同，且不会再报 `Port <same_port> is the main proxy port`。
- `TC-TPRB-11-回归-01`：执行 `pnpm --dir web test:ui traffic.spec.ts -g "加载流量列表并显示详情"`，验证 `listener_port` 更新后 Traffic 列表的 `Port` 列立即刷新，详情 `Proxy Port` 与列表一致。
- `TC-TPRB-11-回归-02`：执行 `pnpm --dir web test:ui admin-settings.spec.ts -g "Settings Proxy 展示临时端口绑定规则详情卡片"`，验证 Settings / Proxy 页面在 Proxy Address 下方展示临时端口卡片，卡片内容来自 `/_bifrost/api/ports/:port/active-summary`。
- `TC-TPRB-10-回归-01`：执行 `cargo run --bin bifrost -- traffic list --help`、`cargo run --bin bifrost -- traffic get --help` 和 `e2e-tests/tests/test_temporary_port_bindings.sh`，验证 Traffic `list/get` 子命令均暴露并接受 `--port`。
- `TC-TPRB-10-回归-02`：执行 `e2e-tests/tests/test_temporary_port_bindings.sh`，验证主端口 HTML Badge 只包含主端口规则，临时端口 HTML Badge 只包含当前临时端口绑定规则。
- `TC-TPRB-13`：`bifrost --help`、`bifrost port --help`、`bifrost port bind --help`、`bifrost port update --help` 均展示完整命令说明、参数来源与多端口工作流提示。
- `TC-TPRB-14`：安装用 `SKILL.md` 包含多端口工作流说明，能够指导用户创建主端口 + 多个临时端口，并区分 `--rule` / `--group-rule` / `--rule-file` / `--rule-text` 的适用场景。
- `TC-TPRB-15`：销毁临时端口后主端口仍可用，`bifrost status` 仍只报告主端口运行。

执行约束：

- 测试进程使用隔离 `BIFROST_DATA_DIR`，但该进程内主端口和临时端口必须共享同一个 data dir。
- 不使用 `9900`。
- 启动命令必须带 `--no-system-proxy`。
- 按文档逐条执行并记录实际结果，不用单元测试或 E2E 替代。

### 项目校验

实现完成后至少执行：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
SKIP_FRONTEND_BUILD=1 bash scripts/ci/local-ci.sh --skip-static --e2e-only rules
```

如果改动涉及 Web UI，再补前端 lint/build 与亮色/暗色 human_tests。

### 2026-05-07 回归修复补充

- shell E2E 回归根因：`test_temporary_port_bindings.sh` 原先通过 `$(allocate_local_test_port)` 多次取值；函数内部的 `TEMP_PORT_OFFSET` 自增发生在命令替换子 shell 中，主 shell 看不到递增后的 offset，导致多个变量可能拿到同一个端口。修复方式改为 `assign_local_test_port <var>`，直接在当前 shell 中写回目标变量。
- Web UI 回归根因：`VirtualTrafficTable` 行级 `memo` 比较器 `areRowPropsEqual` 漏掉 `listener_port`，导致 compact API / push 更新只改变端口字段时，虚拟列表行不会重新渲染。修复方式是在比较器中显式比较 `prevRecord.listener_port !== nextRecord.listener_port`。
- 本轮验证受当前沙箱 `127.0.0.1` 监听权限限制，shell E2E 与 Playwright UI 用例都只能执行到启动阶段并记录 `EPERM/Operation not permitted`；因此设计上仍保留“需在允许本地监听的环境中重跑 human_tests”的门禁说明。

### 2026-05-08 CLI `traffic --port` 回归修复补充

- CLI 回归根因：`TrafficListOptions` / `TrafficGetOptions` 已经包含 `port`，执行层也使用该端口请求 admin API，但 `TrafficCommands::List` 与 `TrafficCommands::Get` 的 clap 参数定义没有挂载子命令级 `--port`，导致 `bifrost traffic list --port "$MAIN_PORT"` 与 `bifrost traffic get --port "$MAIN_PORT" "$TEMP_RECORD_ID"` 在参数解析阶段直接失败。
- 修复方式：只在 CLI 参数解析和传递链路补 `port: Option<u16>`；子命令 `--port` 优先，否则沿用全局 `-p/--port` 或 runtime port fallback。admin API、traffic 渲染和临时端口运行时逻辑不变。

### 2026-05-08 临时端口 Badge 规则视图回归修复补充

- Badge 回归根因：HTTP/TLS 响应注入路径调用 `AdminState::badge_rules_json()`，该缓存只按主端口 enabled 规则生成；临时端口代理出来的页面会误展示主端口规则和主端口合并规则内容。
- 修复方式：Badge 注入根据 `RequestContext.port` 选择规则视图。主端口继续使用主端口缓存；非主端口先通过 `TemporaryPortManager::active_summary(listener_port)` 生成端口级 Badge JSON。TLS 拦截路径重建 `RequestContext` 时必须保留 listener port。

### 2026-05-08 Settings Proxy 临时端口卡片补充

- UI 需求：Settings / Proxy 模块下方需要直接展示当前进程内正在运行的临时端口，避免用户只能通过 CLI 才能确认每个端口绑定了哪些规则。
- 实现方式：WebUI 新增 `/_bifrost/api/ports` 与 `/_bifrost/api/ports/:port/active-summary` 客户端调用；Settings / Proxy 页面在 Proxy Address 下方渲染临时端口卡片，显示监听地址、运行状态、绑定规则来源、缺失引用、resolved active rules 与端口级 merged content。该视图只读，不写入持久化状态，Bifrost 重启后自然随内存态临时端口清空。

### 2026-05-08 Traffic 端口归因补充

- 需求澄清：`listener_port` 是流量入口元数据，不是规则命中结果。所有经由代理端口进入并被记录的流量都必须写入端口信息，包含未命中规则的直连请求、CONNECT tunnel、WebSocket、SOCKS5 TCP/UDP 与连接错误记录。
- 修复方式：补齐 HTTP CONNECT/TLS intercept/WebSocket/SOCKS 记录路径中的 `listener_port` 写入；E2E 增加主端口和临时端口无规则直连请求，断言 detail `listener_port` 正确且 `has_rule_hit=false`。

## 风险与取舍

- 多监听共享 `AdminState` 时要避免“临时端口也暴露 admin API”。建议临时端口默认仍能服务 `/_bifrost/api/ports/:port/active-summary` 之外的代理流量，但 admin API 只在主端口上开放；如果当前 router 是统一挂载，需要为临时 `ProxyConfig` 增加 `admin_enabled: bool`。
- 临时端口 traffic 记录应标记 `listener_port`，否则排查时无法区分主端口和临时端口来源。
- 2026-05 验收补齐：`TrafficRecord`、SQLite `traffic_records.listener_port`、compact API 字段 `lp`、Traffic detail 字段 `listener_port`、CLI `traffic list/get` 和 Web Traffic list/detail 均展示监听端口。主端口和临时端口共享同一个 traffic DB，但每条流量都能按监听端口区分。
- SOCKS5 统一端口当前跟随 `enable_socks`，临时端口是否支持 SOCKS5 需要明确。建议第一版支持 HTTP/HTTPS/SOCKS5 统一端口，但不支持单独 `--socks5-port`。
- access control 复用主配置比较安全；如果后续要每个临时端口单独认证，需要新建端口级 access profile，不放第一版。
- `port bind --port 0` 自动分配端口时，CLI 必须输出最终端口，并写入 binding map。

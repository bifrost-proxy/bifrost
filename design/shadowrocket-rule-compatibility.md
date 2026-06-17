# Shadowrocket 规则兼容与自动转换方案

> 实现状态：本设计文档描述的 Shadowrocket / Surge 规则兼容与自动转换能力 (planned, not yet shipped as of 2026-06-17)。当前仓库中尚未出现 `external_rule` 模块、`parse_shadowrocket` 解析器，也未在 `crates/bifrost-sync/src/normalize.rs` 或前端 Rules 编辑器中接入相关入口；下述章节均为目标设计。

## 功能模块描述

- 为 Bifrost 增加 Shadowrocket 风格配置的导入、兼容与自动转换能力，优先覆盖常见分流规则、规则集、改写、MITM 与本地 Mock 场景。
- 兼容目标不是“完整执行 Shadowrocket 全量 profile 语义”，而是提供一套可解释、可验证、可降级的“兼容子集 + 转换报告”方案。
- 方案同时兼容与 Shadowrocket 高度相似的 Surge 风格配置，优先以其公开规则语义为基线进行落地。

## 背景

- Bifrost 当前已经具备统一规则语言、路由、改写、脚本、TLS 控制与远端规则归一化能力，具备承接外部规则生态的基础。
- 目标用户常见需求不是手写 Bifrost 规则，而是直接复用现有 Shadowrocket / Surge 规则、规则集与改写配置。
- 当前项目已有远端规则归一化入口，适合在导入流程中新增 Shadowrocket 配置识别、解析、转换与告警输出，而不是把适配逻辑散落到多个入口。

## 目标

- 支持导入 Shadowrocket / Surge 常见规则子集并自动转换为 Bifrost 规则文本。
- 对无法等价映射的配置项输出结构化告警，避免静默失败或错误转换。
- 保证转换结果可读、可编辑、可继续被现有 Bifrost 规则解析器和同步链路消费。
- 支持两类触发方式：
  - 在 Rules 编辑器中粘贴规则文本时自动识别并解析来源格式
  - 在导入规则时由用户显式选择规则类型后自动解析
- 优先交付高频场景：
  - `[Rule]` 内常见分流规则
  - `RULE-SET` / `DOMAIN-SET`
  - URL Rewrite / Header Rewrite / Body Rewrite
  - MITM hostname
  - Map Local / API Mock

## 非目标

- 首期不承诺完整兼容 Shadowrocket 的所有 section、所有策略组、所有脚本能力。
- 首期不实现与 Shadowrocket 相同的“策略组运行时选择模型”。
- 首期不承诺 `GEOIP`、`PROCESS-NAME`、`SRC-IP`、`IN-PORT`、复杂逻辑规则、动态 policy group 的完全等价语义。
- 首期不做“后台静默猜测后直接覆盖用户内容”，自动识别必须以可见提示、可撤销预览或显式导入类型选择为前提。

## Bifrost 现状分析

- Bifrost 当前规则语法是统一模型：
  - `pattern operation [operations...] [filters...] [lineProps://...]`
  - 单条规则支持多个 operation、过滤器与规则属性。
- 当前已具备的核心协议足以承接大量 Shadowrocket 功能：
  - 路由类：`host`、`xhost`、`http`、`https`、`ws`、`wss`、`proxy`、`http3`、`pac`、`redirect`
  - 响应类：`file`、`tpl`、`rawfile`
  - 请求/响应改写类：`reqHeaders`、`resHeaders`、`reqReplace`、`resReplace`、`reqBody`、`resBody`、`urlParams`、`urlReplace`
  - 控制类：`tlsIntercept`、`tlsPassthrough`、`passthrough`、`delete`、`skip`
  - 脚本类：`reqScript`、`resScript`、`decode`
- 当前 pattern 能力已覆盖：
  - 精确域名
  - 域名路径
  - IP / CIDR
  - Regex
  - Wildcard
- 项目已有远端规则归一化入口，已支持：
  - legacy 变量展开
  - import 展开
  - 旧别名改写
  - 文本级 normalize

## Shadowrocket / Surge 能力拆解

### 规则系统

- 规则主体系以 `[Rule]` 为核心，常见类型包括：
  - `DOMAIN`
  - `DOMAIN-SUFFIX`
  - `DOMAIN-KEYWORD`
  - `IP-CIDR`
  - `IP-CIDR6`
  - `RULE-SET`
  - `DOMAIN-SET`
  - `FINAL`
- 每条规则本质是：
  - 匹配条件
  - 目标策略或策略组

### HTTP 处理

- 常见 section 包括：
  - `URL Rewrite`
  - `Header Rewrite`
  - `Body Rewrite`
  - `Script`
  - `MITM`
  - `Map Local`

### 运行时模型

- Shadowrocket / Surge 更接近“匹配规则后选择策略或策略组”。
- Bifrost 更接近“匹配规则后直接执行具体动作”。
- 两者最核心的不一致在于：
  - 上游出口在 Shadowrocket 中通常是 policy / policy group 名称
  - 上游出口在 Bifrost 中通常是显式目标，如 `proxy://host:port`、`host://host:port`、`passthrough://`

## 核心兼容边界

### 一、可直接映射

- `DOMAIN` → 精确域名 pattern
- `IP-CIDR` / `IP-CIDR6` → Bifrost IP / CIDR pattern
- `FINAL,DIRECT` → `* passthrough://`
- 静态单一代理策略 → `proxy://host:port`
- 302/307 重定向 → `redirect://...`
- Map Local / Mock → `file://...`、`tpl://...`、`rawfile://...`
- MITM hostname → `tlsIntercept://`
- Header Rewrite → `reqHeaders`、`resHeaders`、`delete`、`headerReplace`
- Body Rewrite → `reqReplace`、`resReplace`、`reqBody`、`resBody`

### 二、可近似映射

- `DOMAIN-SUFFIX`
  - Shadowrocket / Surge 语义通常覆盖裸域和其子域
  - Bifrost 不能只用单条 `**.example.com` 粗暴替代，否则会漏掉裸域
  - 建议展开为两条：
    - `example.com ...`
    - `**.example.com ...`
- `DOMAIN-KEYWORD`
  - 可转为 host 维度 regex
  - 不建议直接转为路径级模糊匹配，避免把 path 命中误当作 host 命中
- `RULE-SET` / `DOMAIN-SET`
  - 首期采取导入时拉取并展开的离线转换模型
  - 若规则集内出现不支持类型，保留告警并跳过该子规则
- `USER-AGENT`
  - 可近似转为 `includeFilter://h:User-Agent=...`
  - 仅对 HTTP 可见流量成立，对未解密 HTTPS / 原始 TCP 不完全等价

### 三、首期不等价支持

- `GEOIP`
- `PROCESS-NAME`
- `SRC-IP`
- `IN-PORT`
- 复杂 `AND` / `OR` / `NOT` 逻辑规则
- `SCRIPT` 作为“动态决策策略”规则
- `REJECT`、`REJECT-DROP`、`REJECT-TINYGIF` 的完整行为
- `select`、`url-test`、`fallback`、`load-balance` 等 policy group 的运行时语义

## 最大风险点

### 一、优先级模型不同

- Shadowrocket / Surge 规则是严格从上到下 first-match。
- Bifrost 当前规则优先级包含 pattern 类型优先级，不是纯文本顺序模型。
- 如果直接把 Shadowrocket 规则顺序平移到 Bifrost，可能在以下情况产生偏差：
  - 上层写了 regex，后面写了精确域名
  - 上层写了泛规则，希望先拦截，后面才写更具体规则
  - 依赖 `FINAL` 之前某条弱匹配规则的抢占行为

### 二、策略组无法直接落地

- `DOMAIN-SUFFIX,google.com,ProxyGroupA` 在 Shadowrocket 中是“交给策略组 A 决策”。
- Bifrost 当前没有与其同构的 policy group 运行时模型。
- 首期只能支持：
  - `DIRECT`
  - 可静态解析为单一代理目标的策略
  - 可配置映射表解析出的固定目标

### 三、Section 语法分散

- Shadowrocket / Surge 的 profile 被分散在多个 section。
- Bifrost 期望输出统一规则文本。
- 如果不先做 AST，后续很容易出现：
  - 解析重复
  - 改写链路不统一
  - 告警位置丢失

### 四、自动识别误判

- “粘贴时自动识别”与“导入时自动解析”都引入了格式识别问题。
- 如果误把普通 Bifrost 规则识别成 Shadowrocket / Surge，会导致：
  - 转换后的规则被意外改写
  - 用户难以理解保存前后的差异
  - 粘贴体验变得不可预期
- 因此识别策略必须分层：
  - 先识别明确的 Bifrost 格式
  - 再识别 Shadowrocket / Surge profile 特征
  - 无法确认时保持原文本，不自动改写

## 实现逻辑

### 第一阶段：建立 Shadowrocket Profile AST

- 新增独立解析层，将 profile 按 section 解析为中间表示：
  - General
  - Rule
  - Ruleset
  - URL Rewrite
  - Header Rewrite
  - Body Rewrite
  - Script
  - MITM
  - Map Local
  - Proxy / Proxy Group
- 每一条原始输入都保留：
  - section
  - 原始文本
  - 行号
  - 解析状态

### 第二阶段：策略解析与归一化

- 把 Shadowrocket / Surge 的策略名称归一化为三类：
  - `DIRECT`
  - `STATIC_PROXY`
  - `UNSUPPORTED_DYNAMIC_POLICY`
- 对静态可解析代理，产出：
  - `proxy://host:port`
  - 后续如有必要，再扩展认证与协议适配
- 对策略组：
  - 首期不执行组逻辑
  - 在转换报告中标记为不支持

### 第三阶段：规则与改写转换

- `[Rule]` 转 Bifrost 路由 / 控制规则。
- `RULE-SET` / `DOMAIN-SET` 在导入时拉取、缓存并展开成普通 Bifrost 规则。
- `URL Rewrite` 转：
  - `redirect://`
  - `urlReplace://`
  - 必要时补充 `host://` / `http://` / `https://`
- `Header Rewrite` 转：
  - `reqHeaders://`
  - `resHeaders://`
  - `delete://`
  - `headerReplace://`
- `Body Rewrite` 转：
  - 简单替换优先用 `reqReplace://` / `resReplace://`
  - 复杂替换或多段替换可降级为脚本
- `MITM` 转：
  - 命中 hostname 产出 `tlsIntercept://`
  - 排除项产出 `tlsPassthrough://`
- `Map Local` 转：
  - 静态文件 → `file://`
  - 原始响应 → `rawfile://`
  - 模板化内容 → `tpl://`

### 第四阶段：转换报告

- 每次导入产出一份转换报告，最少包含：
  - 成功转换条数
  - 近似转换条数
  - 跳过条数
  - 未支持能力列表
  - 可能存在语义偏差的规则列表
- 转换报告必须包含行号与原始规则片段，便于人工校对。

### 第五阶段：接入入口

- 优先接入规则编辑与规则导入入口，而不是修改核心匹配引擎。
- 推荐新增统一“外部规则解析器”层：
  - 输入原始文本
  - 输出 `source_type + normalized_bifrost_text + warnings + report`
  - 供粘贴入口与导入入口复用
- 识别顺序建议为：
  - 先识别 Bifrost 规则 / `.bifrost` 规则内容
  - 再识别 legacy 规则
  - 再识别 Shadowrocket / Surge profile
  - 无法确认时按原始 Bifrost 文本处理，不自动改写

## 交互方案

### 一、Rules 编辑器粘贴自动识别

- 目标：
  - 用户把 Shadowrocket / Surge 规则文本直接粘贴到 Rules 编辑器时，系统自动识别来源格式并解析。
- 行为建议：
  - 监听编辑器 paste 事件
  - 读取本次粘贴文本片段，而不是重扫整个文档
  - 调用统一“外部规则解析器”尝试识别
  - 若识别为 Shadowrocket / Surge：
    - 展示轻量提示或确认条
    - 说明“已识别为 Shadowrocket/Surge 规则，是否转换为 Bifrost 规则”
    - 用户确认后插入转换结果
  - 若识别不明确：
    - 保持原样粘贴
    - 不自动做破坏性替换
- 交互要求：
  - 用户应能查看转换警告
  - 用户应能撤销
  - 用户应能选择“按原文粘贴”

### 二、导入规则时选择特定类型

- 目标：
  - 在“导入规则”入口支持用户显式选择规则来源类型，然后自动按该类型解析。
- 建议支持的类型：
  - `Bifrost`
  - `Legacy`
  - `Shadowrocket / Surge`
  - `自动识别`
- 行为建议：
  - 当用户选择 `Shadowrocket / Surge` 时，不再依赖自动猜测，直接走对应 AST 解析与转换链路
  - 当用户选择 `自动识别` 时，使用统一识别逻辑
  - 当用户选择 `Bifrost` 时，严格按现有 Bifrost 规则解析，避免误改写
- 导入结果建议：
  - 展示导入摘要
  - 展示转换报告
  - 展示 warnings
  - 允许用户确认后再保存为本地规则文件

### 三、入口职责拆分

- 粘贴入口负责：
  - 文本片段识别
  - 用户确认
  - 将转换结果写入编辑器
- 导入入口负责：
  - 读取完整文件或文本
  - 支持来源类型选择
  - 统一展示导入摘要、告警与保存结果
- 两者都不应自行维护格式解析逻辑，必须复用同一套解析与转换服务。

## 识别与转换细节

### 一、来源类型枚举

- 建议统一定义来源类型：
  - `bifrost`
  - `legacy`
  - `shadowrocket`
  - `surge`
  - `auto`
  - `unknown`
- UI 展示层可合并为：
  - `Bifrost`
  - `Legacy`
  - `Shadowrocket / Surge`
  - `自动识别`
- 内部实现仍建议区分 `shadowrocket` 与 `surge`，便于后续做差异化兼容。

### 二、自动识别规则

- `bifrost` 识别信号：
  - 命中 `.bifrost` 文件头
  - 或文本明显符合 `pattern protocol://value` 连续规则格式
  - 或可直接被现有 Bifrost tolerant parser 高置信度解析
- `legacy` 识别信号：
  - 出现 `${var}`、`@user/name`、`ignore://host|rule`、`enable://intercept` 等旧语法特征
- `shadowrocket / surge` 识别信号：
  - 出现 section 头如 `[Rule]`、`[URL Rewrite]`、`[Header Rewrite]`、`[MITM]`
  - 行级规则出现 `DOMAIN-SUFFIX,`、`DOMAIN-KEYWORD,`、`IP-CIDR,`、`RULE-SET,`
  - 出现策略名在第三列的逗号分隔规则格式
- `unknown` 处理策略：
  - 粘贴场景：原样粘贴
  - 导入场景：提示无法识别，允许用户重新选择类型

### 三、识别结果置信度

- 建议识别器返回 `confidence`：
  - `high`
  - `medium`
  - `low`
- 交互策略：
  - `high`：可默认推荐转换
  - `medium`：展示确认提示后再转换
  - `low`：默认不转换，建议用户手动选择来源类型

### 四、转换结果结构

- 统一转换结果建议为：
  - `source_type`
  - `confidence`
  - `normalized_text`
  - `warnings`
  - `report`
  - `stats`
- `warnings` 面向用户快速阅读，强调风险与丢失能力。
- `report` 面向详细排查，保留逐条转换信息。
- `stats` 面向 UI 摘要展示，例如：
  - 转换成功条数
  - 近似转换条数
  - 跳过条数
  - 未支持条数

### 五、逐条转换结果结构

- 建议 `report.items` 至少包含：
  - `line`
  - `section`
  - `source`
  - `action`
  - `target`
  - `level`
  - `message`
- `action` 建议值：
  - `converted`
  - `approximated`
  - `skipped`
  - `unsupported`
  - `kept`
- `level` 建议值：
  - `info`
  - `warning`
  - `error`

## 前后端接口草案

### 一、前端粘贴识别接口

- 场景：
  - Rules 编辑器在 paste 时把“本次粘贴文本”发给后端或本地解析层进行识别。
- 请求建议：

```json
{
  "mode": "paste",
  "text": "DOMAIN-SUFFIX,example.com,DIRECT",
  "source_type": "auto"
}
```

- 响应建议：

```json
{
  "source_type": "shadowrocket",
  "confidence": "high",
  "normalized_text": "example.com passthrough://\n**.example.com passthrough://",
  "warnings": [
    "DOMAIN-SUFFIX 已展开为裸域和多级子域两条规则"
  ],
  "stats": {
    "converted": 1,
    "approximated": 1,
    "skipped": 0,
    "unsupported": 0
  },
  "report": {
    "items": [
      {
        "line": 1,
        "section": "Rule",
        "source": "DOMAIN-SUFFIX,example.com,DIRECT",
        "action": "approximated",
        "target": "example.com passthrough://\n**.example.com passthrough://",
        "level": "warning",
        "message": "suffix 语义通过展开实现"
      }
    ]
  }
}
```

### 二、规则导入接口

- 当前项目已有 `/bifrost-file/import`，默认根据内容检测 `.bifrost` 类型。
- 本方案建议在规则导入场景增加“来源类型”参数，避免只能依赖自动检测。
- 请求建议：

```json
{
  "name": "imported-shadowrocket-rules",
  "content": "...原始文本...",
  "source_type": "shadowrocket",
  "save_mode": "create_rule"
}
```

- `save_mode` 建议值：
  - `preview_only`
  - `create_rule`
  - `replace_existing`

### 三、后端职责拆分

- 建议将后端逻辑拆成三层：
  - `detect_source_type(text)`：只做识别
  - `convert_external_rules(text, source_type)`：只做转换
  - `import_converted_rules(name, normalized_text)`：只做保存
- 这样可同时满足：
  - 粘贴时只识别 / 转换，不保存
  - 导入时识别 / 转换 / 保存一体化

### 四、与现有接口的关系

- 现有 `/bifrost-file/detect` 与 `/bifrost-file/import` 更偏 `.bifrost` 文件导入。
- 建议不要把 Shadowrocket 识别硬塞进原有 `.bifrost` 文件类型检测逻辑里。
- 更合理的是：
  - 保留现有 `.bifrost` 文件接口
  - 为“外部规则文本解析”新增独立接口
  - 或在 rules 专用导入接口下增加可选 `source_type`

## 前端落地细节

### 一、Rules 编辑器

- 当前 Rules 编辑器通过 store 管理 `editingContent`，保存时走 `saveCurrentRule`。
- 粘贴转换建议在编辑器层完成，不修改 store 保存语义。
- 推荐流程：
  - 捕获 paste
  - 取本次文本
  - 调识别/转换接口
  - 弹出轻量确认 UI
  - 用户确认后再把 `normalized_text` 插入 editor model
  - 插入后沿用现有验证与保存流程

### 二、导入规则弹窗

- 建议新增规则专用导入弹窗，不复用仅支持 `.bifrost` 的上传按钮语义。
- 弹窗建议包含：
  - 文本输入区 / 文件上传
  - 来源类型选择器
  - 规则名输入框
  - 解析预览区
  - warnings 区
  - 转换报告区
- 操作按钮建议：
  - `仅预览`
  - `转换并创建`
  - `取消`

### 三、交互文案

- 粘贴识别提示建议尽量短：
  - “已识别为 Shadowrocket/Surge 规则，是否转换为 Bifrost 规则？”
- 导入摘要建议突出结果：
  - “共识别 48 条规则，成功转换 43 条，近似转换 3 条，跳过 2 条”

## 后端落地细节

### 一、模块划分建议

- 建议新增独立模块，例如：
  - `crates/bifrost-core/src/external_rule/`
  - 或 `crates/bifrost-sync/src/external_rule/`
- 子模块建议：
  - `detect.rs`
  - `ast.rs`
  - `parse_shadowrocket.rs`
  - `convert.rs`
  - `report.rs`

### 二、AST 层次建议

- `ProfileAst`
  - `sections: Vec<SectionAst>`
- `SectionAst`
  - `kind`
  - `line_start`
  - `line_end`
  - `items`
- `RuleItemAst`
  - `raw`
  - `line`
  - `rule_type`
  - `value`
  - `policy`
  - `options`
- `RewriteItemAst`
  - `raw`
  - `line`
  - `direction`
  - `pattern`
  - `action`
  - `args`

### 三、错误与告警模型

- 解析错误不应直接中断整个文件，建议按 item 级别收集。
- 可分三类：
  - `fatal`：整体无法解析
  - `item_error`：某一条规则无法解析
  - `item_warning`：某一条规则被近似转换或跳过

## 实施任务拆解

### 第一批：解析基础设施

- 定义来源类型枚举
- 定义识别结果结构
- 定义转换结果结构
- 实现自动识别器
- 实现 Shadowrocket / Surge Profile AST parser

### 第二批：规则转换核心

- 实现 `[Rule]` 基础类型转换
- 实现 `RULE-SET` / `DOMAIN-SET` 展开
- 实现 URL / Header / Body Rewrite 转换
- 实现 MITM / Map Local 转换
- 实现转换报告生成

### 第三批：前端入口接入

- 接入 Rules 编辑器 paste 识别
- 增加转换确认 UI
- 增加规则导入弹窗
- 增加来源类型选择
- 增加转换预览与 warnings 展示

### 第四批：后端入口接入

- 提供识别 / 转换接口
- 提供规则导入接口的来源类型参数
- 复用现有规则保存流程
- 增加观测日志

### 第五批：测试与验收

- 单元测试
- 前端交互测试
- 导入回归测试
- e2e 验证

## 兼容矩阵

| Shadowrocket / Surge 能力 | 首期策略 | Bifrost 落地方式 |
| --- | --- | --- |
| DOMAIN | 直接支持 | 精确 pattern |
| DOMAIN-SUFFIX | 近似展开 | 裸域 + 多级子域两条规则 |
| DOMAIN-KEYWORD | 近似支持 | host regex |
| IP-CIDR / IP-CIDR6 | 直接支持 | IP/CIDR pattern |
| FINAL,DIRECT | 直接支持 | `* passthrough://` |
| FINAL,静态代理 | 直接支持 | `* proxy://...` |
| RULE-SET | 直接支持 | 拉取并展开 |
| DOMAIN-SET | 直接支持 | 拉取并展开 |
| URL Rewrite | 直接/近似支持 | `redirect://` / `urlReplace://` |
| Header Rewrite | 直接支持 | `reqHeaders` / `resHeaders` / `delete` / `headerReplace` |
| Body Rewrite | 直接/近似支持 | `reqReplace` / `resReplace` / 脚本降级 |
| MITM | 直接支持 | `tlsIntercept://` / `tlsPassthrough://` |
| Map Local | 直接支持 | `file` / `tpl` / `rawfile` |
| SCRIPT 改写 | 近似支持 | `reqScript` / `resScript` |
| GEOIP | 暂不支持 | 告警并跳过 |
| PROCESS-NAME | 暂不支持 | 告警并跳过 |
| Policy Group | 暂不支持 | 告警并跳过 |

## MVP 落地顺序

1. 先接入 Rules 编辑器粘贴识别：
   - 支持识别 Shadowrocket / Surge 文本
   - 支持确认后转换插入
2. 接入规则导入入口的“来源类型选择”：
   - `自动识别`
   - `Bifrost`
   - `Legacy`
   - `Shadowrocket / Surge`
3. 仅支持导入静态 profile，不支持在线双向同步 Shadowrocket 原格式。
4. 仅支持静态策略：
   - `DIRECT`
   - 单一代理目标
5. 支持常见规则类型：
   - `DOMAIN`
   - `DOMAIN-SUFFIX`
   - `DOMAIN-KEYWORD`
   - `IP-CIDR`
   - `RULE-SET`
   - `FINAL`
6. 支持常见改写：
   - URL Rewrite
   - Header Rewrite
   - Body Rewrite
   - MITM
   - Map Local
7. 输出：
   - Bifrost 规则文本
   - 转换报告

## 依赖项

- 规则语法与协议定义：
  - `crates/bifrost-core/src/protocol.rs`
  - `crates/bifrost-core/src/rule/parser/mod.rs`
  - `crates/bifrost-core/src/matcher/factory.rs`
- 现有远端归一化入口：
  - `crates/bifrost-sync/src/normalize.rs`
  - `crates/bifrost-sync/src/manager.rs`
- 规则文档与能力说明：
  - `docs/rule.md`
  - `docs/rules/routing.md`
  - `docs/rules/filters.md`
  - `docs/rules/url-manipulation.md`
  - `docs/rules/scripts.md`

## 测试方案

- 新增解析单测覆盖：
  - section 识别
  - rule line AST 解析
  - policy / policy group 识别
  - `RULE-SET` / `DOMAIN-SET` 展开
- 新增转换单测覆盖：
  - `DOMAIN` → 精确规则
  - `DOMAIN-SUFFIX` → 裸域 + 子域展开
  - `DOMAIN-KEYWORD` → host regex
  - `FINAL,DIRECT` → `passthrough://`
  - `URL Rewrite` / `Header Rewrite` / `Body Rewrite` 转换
  - `MITM` / `Map Local` 转换
- 新增入口交互测试覆盖：
  - 编辑器粘贴 Shadowrocket 文本后能触发识别
  - 用户选择“按原文粘贴”时不做转换
  - 导入时选择 `Shadowrocket / Surge` 能直接走专用解析链路
  - 导入时选择 `Bifrost` 不会误触发外部规则转换
- 新增回归测试覆盖：
  - 转换结果能够被现有 Bifrost parser 正常解析
  - 转换结果能通过现有路由与改写 e2e 场景验证
  - 不支持项能稳定输出 warning，而不是 silent ignore
- 按仓库要求执行：
  - 先执行 `e2e-test`
  - 再执行 `rust-project-validate`
  - 最后执行 `cargo test --workspace --all-features`

## 校验要求

- 导入一份 Shadowrocket / Surge 示例 profile 后，必须同时得到：
  - 可解析的 Bifrost 规则文本
  - 可读的转换报告
- 对于不支持项，必须在输出中显式标记“未支持”或“近似转换”。
- 对于 `DOMAIN-SUFFIX`、`FINAL`、`RULE-SET` 这类高频规则，必须有专门用例验证语义不跑偏。
- 对于依赖当前 Bifrost 优先级模型可能产生偏差的规则，必须在转换报告中显式告警。
- 对于粘贴入口，必须保证：
  - 识别失败不影响原始粘贴行为
  - 识别成功后用户可以取消转换
  - 整个流程可撤销

## 文档更新要求

- 本阶段先输出设计文档，不立即修改 `README.md`。
- 当功能真正实现并对外暴露导入入口后，需要补充：
  - `README.md` 中的能力说明
  - `docs/` 中的导入文档
  - 兼容矩阵与已知限制说明

## 最终结论

- Shadowrocket 规则兼容是可行方向，但应以“兼容子集 + 自动转换 + 明确告警”为首期目标。
- 当前最优路线不是新建一套完整 Shadowrocket 运行时引擎，而是：
  - 先做 AST
  - 再做静态转换
  - 再接入“粘贴识别”和“导入类型选择”两个入口
  - 最终落到现有 Bifrost 规则体系
- 当真实用户配置中出现大量 policy group、动态脚本决策、复杂逻辑规则需求时，再评估是否补充 Shadowrocket 兼容执行模式。

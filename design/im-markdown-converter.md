# IM 消息 Markdown 格式转换器 设计方案

## 背景

Bifrost 的 Agent loop 输出的是标准 CommonMark Markdown（三反引号代码块、`- [ ]` 任务列表、`![alt](url)` 图片、HTML 标签、脚注、`***text***` 组合样式等），但飞书卡片 JSON 2.0 的 `markdown` 组件只支持一个受限子集：

- 代码块语言标识必须使用飞书定义的名称（例如 `python` 而不是 `py`、`c_sharp` 而不是 `cs`）。
- 图片只支持 `image_key`，不支持外链 URL。
- 水平分割线只识别 `---`。
- 不支持 GFM 任务列表语法 `- [ ]`。
- HTML 标签只支持白名单：`<br>`, `<hr>`, `<font>`, `<a>`, `<at>`, `<person>`, `<text_tag>`, `<local_datetime>`, `<link>`, `<raw>`, `<number_tag>`, `<audio>`, `<img>`。
- `***text***` 会被渲染成异常样式。
- `[^1]` 脚注会被当作普通文字保留。

如果 Agent 原样发送 Markdown，飞书卡片会出现代码高亮失效、图片显示为“无法预览”、任务列表出现原始 `[ ]` 文本、`<div>` / `<span>` 变成 escape 后的乱码等问题。IM Gateway 需要在把 Markdown 写入卡片 JSON 之前统一转换，确保飞书能正确渲染。

## 用户目标验证清单

### 必须实现

- LLM 常用的语言缩写（`ts`/`js`/`py`/`rb`/`sh`/`yml`/`cs`/`objc`/`glsl`/`vb`/`coffee`/`plaintext`/`dockerfile` 等）在代码块起始行被映射成飞书兼容名称。
- 代码块起始行携带 `python {title="x"}` 这类元数据时，只保留语言名，把元数据剥离。
- 图片语法 `![alt](url)` 被转换成 `[🖼 alt](url)` 或 `[🖼 图片链接](url)`，用户仍能在卡片里点击链接查看原图。
- 水平分割线 `***` / `___` / `* * *` / `- - -` 全部归一为 `---`，并保证前后有换行。
- 任务列表 `- [ ]` → `- ⬜`，`- [x]` / `- [X]` → `- ✅`。
- 不在白名单里的 HTML 标签（`<div>` / `<span>` / `<p>` / `<b>` / `<i>` / `<em>` / `<strong>` / `<u>` / `<s>` / `<sub>` / `<sup>` / `<details>` / `<summary>` / `<blockquote>` 等）被移除，但保留标签内文本。
- `***text***` → `**_text_**`。
- 脚注引用 `[^1]` 和脚注定义行 `[^1]: ...` 被完全移除。
- 代码块内部内容永远不被改写，只重写起始行的语言标识。
- 所有替换 UTF-8 安全，中日韩字符、emoji、4 字节字符不被截断或错位。

### 必须不破坏

- 已经是飞书兼容格式的 Markdown（例如已经写 `python`、`---`、`⬜`）不会被二次改写破坏。
- 白名单内的 HTML 标签（`<br>` / `<hr>` / `<font>` / `<a>` / `<at>` / `<person>` / `<text_tag>` / `<local_datetime>` / `<link>` / `<raw>` / `<number_tag>` / `<audio>` / `<img>`）原样透传。
- 反引号内联代码 `` `x` `` 不被 HTML 清理或 bold+italic 规则误伤。
- 非围栏代码块内的正文行仍按行独立处理，不引入跨行状态污染。
- 转换函数是幂等的：`convert_to_feishu_markdown(convert_to_feishu_markdown(x)) == convert_to_feishu_markdown(x)`，保证多次调用（例如 progress card 反复渲染同一 snapshot）不会累积 emoji 或字符。

### 必须真实验证

- 通过飞书 Bot 真实发送包含以上各类语法的消息，肉眼确认卡片渲染符合预期。
- Agent 主回复、带标题回复、带计划回复、错误卡、messages.send 富卡、通用 `build_markdown_card` 六个调用点都走这个函数，抓取真实 outbound payload 抽样确认。
- CJK / emoji / 4 字节字符混排样本不出现乱码。

## 产品语义

### 转换是发送前的最后一站

模块只承担“把标准 Markdown 翻译成飞书 Markdown”，不做业务侧的润色、脱敏或裁剪。所有裁剪、折叠、卡片组件拼装仍在上游 progress card / message builder 中完成。转换器不感知业务语义，只按行扫描。

### 保守清理 vs. 完整解析

第一版不引入完整 CommonMark AST 解析（`pulldown-cmark` 等），而是使用轻量的按行状态机：

1. 逐行扫描 `input.split('\n')`。
2. 用 `inside_code_block + code_fence` 状态跟踪围栏代码块（``` 或 ~~~，允许 4+ 反引号）。
3. 代码块内部行只做透传；非代码行依次跑水平线、任务列表、图片链接、脚注、bold+italic、HTML 清理。
4. 起始代码块行只改写 fence 之后的语言标识，剩余元数据抛弃。

好处：实现小、行为直观、易被单元测试穷举；坏处：跨行 Markdown 结构（例如引用块内的表格）不做特殊处理。这与 Feishu 卡片渲染器的能力一致，是本模块的取舍。

### 转换是幂等且 UTF-8 安全的

- 幂等：所有替换规则都在“已转换形态”下不再匹配（例如 `---` 已经是 `---`；`- ⬜` 里的 emoji 已经不是 `[ ]`）。
- UTF-8：不使用 byte 索引切片，任何截取都基于 char boundary；单元测试覆盖中日韩字符、emoji、4 字节字符（`𝓐`、`🀄`、`🇨🇳` 等）与代码块混排场景。

## 转换规则细则

### 1. 代码块语言标识标准化

映射表（大小写不敏感，统一转小写）：

| 输入 | 输出 |
| --- | --- |
| `ts` | `typescript` |
| `js` | `javascript` |
| `py` | `python` |
| `rb` | `ruby` |
| `sh` | `shell` |
| `yml` | `yaml` |
| `cs` / `csharp` / `c#` | `c_sharp` |
| `objc` / `objective-c` / `objectivec` | `objective_c` |
| `glsl` | `opengl_shading_language` |
| `vb` | `visual_basic` |
| `coffee` / `coffeescript` | `coffee_script` |
| `plaintext` / `text` / `txt` | `plain_text` |
| `dockerfile` | `docker_file` |

`bash` 直接透传（飞书同时支持 `bash` 和 `shell`）。带元数据后缀的 `python {title="x"}` 只保留 `python`。

### 2. 图片 URL 处理

- `![alt](https://...)` → `[🖼 alt](https://...)`
- `![](https://...)` → `[🖼 图片链接](https://...)`
- `image_key` 图片（`![alt](img_v3_...)`）由上游卡片构造器直接使用 `img` 组件，不走 Markdown 转换。

### 3. 水平分割线标准化

- `***` / `___` / `* * *` / `- - -` → `---`
- 前后各加 `\n`，避免与相邻段落粘连。

### 4. 任务列表转换

- `- [ ] text` → `- ⬜ text`
- `- [x] text` / `- [X] text` → `- ✅ text`
- 有序列表 `1. [ ]` 同样处理。

### 5. HTML 标签清理

白名单原样保留：`<br>`, `<br/>`, `<hr>`, `<hr/>`, `<font>`, `<a>`, `<at>`, `<person>`, `<text_tag>`, `<local_datetime>`, `<link>`, `<raw>`, `<number_tag>`, `<audio>`, `<img>`。

非白名单标签移除标签本身、保留内部文本：`<div>`, `<span>`, `<p>`, `<b>`, `<i>`, `<em>`, `<strong>`, `<u>`, `<s>`, `<sub>`, `<sup>`, `<details>`, `<summary>`, `<blockquote>` 等。

反引号内联代码 `` `<div>` `` 视为文本，不清理。

### 6. Bold+Italic 组合

`***text***` → `**_text_**`。避免飞书出现斜体嵌套加粗渲染异常。

### 7. 脚注移除

- 引用：`[^1]` / `[^abc]` → 删除。
- 定义：整行以 `[^1]: ` 开头的行 → 删除整行。

## 技术细节

### 入口签名

```rust
pub fn convert_to_feishu_markdown(input: &str) -> String;
```

- 位于 `crates/bifrost-admin/src/im_gateway/markdown_converter.rs`。
- 无外部状态，纯函数，可以并行调用。
- 内部按行迭代，输出行分隔沿用输入的 `\n`；如果输入不以 `\n` 结尾则输出也不追加。

### 内部结构

```text
convert_to_feishu_markdown
 ├─ detect_code_fence_open (判定 ``` / ~~~ 起始与语言)
 ├─ normalise_code_lang (映射表 + 元数据剥离)
 ├─ is_horizontal_rule / normalize_horizontal_rule
 ├─ convert_task_list
 ├─ convert_image_urls
 ├─ is_footnote_definition / remove_footnote_refs
 ├─ convert_bold_italic
 └─ remove_unsupported_html
```

每个子函数接受 / 返回 `Cow<'_, str>`，未改动时保持 borrowed，避免不必要的字符串拷贝。

### 状态机与边界情形

- 支持 ``` / ~~~ 两种围栏；4 个及以上反引号也可作 fence，闭合 fence 数量必须 ≥ 起始 fence。
- 缩进代码块（4 空格）视为普通段落，第一版不特殊处理。
- 嵌套代码 fence 不支持（Markdown 本身不允许 `` ` `` 嵌套）。
- HTML 标签解析只识别 `<name>` 与 `</name>` 形式，属性内含 `>` 时按第一个 `>` 断句；`<img src="a?b>c">` 这类恶意样本会被切错，需要在上游 sanitize。第一版接受该限制。
- 图片 URL 中括号 `(` / `)` 用简单深度计数处理，`![a](https://x.com/(y))` 能正确闭合。

## 应用点

所有面向飞书卡片的 Markdown 内容在写入 JSON 前都经 `convert_to_feishu_markdown()`：

1. `handlers/im_gateway/event_loop.rs` — 外部 Runner 主回复。
2. `handlers/im_gateway/agent_reply.rs::send_agent_reply_with_title` — 带标题回复。
3. `handlers/im_gateway/agent_reply.rs::send_agent_reply_with_plan` — 带任务计划回复。
4. `handlers/im_gateway/agent_reply.rs::send_error_card_to_owner` — 错误通知卡。
5. `handlers/im_gateway/messages.rs::build_rich_card_content` — `messages.send` API 富卡正文。
6. `im_gateway/feishu.rs::build_markdown_card` — 通用 Markdown 卡片构造助手。
7. `im_gateway/progress_card.rs` 内 Feishu snapshot renderer 的 output / thinking / process timeline 内容 —— 通过上游 helper 复用同一函数。

新增卡片构造入口时必须在评审阶段确认调用了 `convert_to_feishu_markdown()`。

## CLI + Web + Admin API

本模块无独立 CLI 或 Admin API 入口。它是纯内部工具函数：

- 不进入 `bifrost im` 子命令。
- 不出现在任何 REST 路径。
- 不作为 Remote Invoke tool。

Web 只能通过发送真实消息间接观察转换效果。

## Sync 边界

- 转换器无持久化状态。
- 无本地缓存文件，无迁移需求。
- Remote Invoke 不暴露它。
- 单元测试样本文件 (`tests/data/...` 若引入) 不进 sync；当前测试数据全部内联在 `#[cfg(test)]` 模块中。

## 实现切分

### Phase 1：基础转换器

- 建立 `markdown_converter.rs`，实现 `convert_to_feishu_markdown` 与所有子函数。
- 完成单元测试：语言映射、图片、水平线、任务列表、HTML 清理、脚注、bold+italic、代码块保护、UTF-8 安全、幂等性。

### Phase 2：接入所有出站点

- 逐个替换 `agent_chat.rs`、`agent_reply.rs`、`messages.rs`、`feishu.rs` 的 Markdown 写入路径。
- 补充 `progress_card.rs` 中 snapshot renderer 的 output / thinking / process timeline 转换。
- 抓取真实 outbound message log 抽样验证。

### Phase 3：新增缺失映射

- 根据线上实际 outbound payload 分析追加缺失的语言映射（例如 `pwsh`、`bat`、`makefile`）。
- 对 HTML 白名单变更同步跟进飞书官方文档。

### Phase 4：文档与真实场景测试

- 更新 `human_tests/im-markdown-converter.md`（当前尚未落地）。
- 更新 `human_tests/readme.md` 索引。

## 测试方案

### 单元测试

位于 `markdown_converter.rs` 的 `#[cfg(test)] mod tests`：

- `test_code_block_lang_normalization` — 各语言缩写映射。
- `test_code_block_lang_metadata_stripped` — `python {title="x"}` 只留 `python`。
- `test_image_url_to_link` — 图片 URL 转文字链接（含无 alt）。
- `test_hr_normalization` — `***`/`___`/`* * *`/`- - -` 归一。
- `test_task_list_conversion` — `- [ ]` / `- [x]`。
- `test_html_tag_cleanup` — 不支持标签移除、白名单保留。
- `test_code_block_content_preserved` — 代码块内部不被修改。
- `test_footnote_removal` — `[^1]` 引用与定义。
- `test_bold_italic_combo` — `***text***`。
- `test_mixed_content` — 综合场景。
- `test_idempotent` — `convert(convert(x)) == convert(x)`。
- `test_utf8_cjk` / `test_utf8_emoji` / `test_utf8_4byte` — UTF-8 安全套件。
- `test_nested_backticks` — 4+ 反引号 fence。

### E2E 测试

- IM Gateway Agent 回归中已经间接覆盖：真实 Agent loop 输出会走本函数，只要单元测试全绿且六个应用点没漏接，E2E 无需单独用例。

### 真实场景测试

- 新建 `human_tests/im-markdown-converter.md`（当前缺失）：
  - TC-IMC-01：Agent 回复包含 `py` / `ts` / `cs` 代码块，卡片显示为高亮的 Python/TypeScript/C#。
  - TC-IMC-02：Agent 回复包含 `![截图](https://example.com/x.png)`，卡片显示为可点击的 `🖼 截图` 链接。
  - TC-IMC-03：Agent 回复包含 `- [ ] todo` 与 `- [x] done`，卡片显示 `⬜ todo` 与 `✅ done`。
  - TC-IMC-04：Agent 回复包含 `<div>` / `<span>`，卡片不出现原始 HTML 标签。
  - TC-IMC-05：Agent 回复包含 `***重点***`，卡片渲染为加粗且斜体。
  - TC-IMC-06：Agent 回复包含中日韩 + emoji + 4 字节字符 + 代码块混排，字符无乱码。
- 更新 `human_tests/readme.md` 索引。

### 覆盖率与项目校验

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-admin markdown_converter`
- `cargo test --workspace --all-features`
- `rust-project-validate`

本机 no-local-coverage 约定下不跑 `make coverage`。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：语言映射、图片、水平线、任务列表、HTML 清理、脚注、bold+italic、代码块保护、UTF-8 安全、幂等性。
- 复核 diff：所有 Markdown 出站点是否都接入了 `convert_to_feishu_markdown()`；新增卡片构造函数是否漏接。
- 复测：`cargo test -p bifrost-admin markdown_converter`；抓取一批真实 outbound message payload 做人工检视。

### 第 2 轮

- 复核第 1 轮发现的映射缺口修复。
- 再次检查 `git status` / `git diff`，确认没有引入跨 crate 副作用。
- 复测：失败样本的单元测试重跑；飞书 Bot 真实发送综合场景消息。

## 风险与决策点

- **不采用完整 CommonMark AST**：换取实现小和易测。若后续飞书增强 Markdown 能力（例如支持嵌套结构），再评估切到 `pulldown-cmark`。
- **HTML 属性内 `>`**：第一版接受切错风险，业务侧不会主动构造这种字符串；如出现需在上游 sanitize。
- **图片 URL 转链接而非下载 upload**：Agent 输出的外链无法保证访问权，卡片端也会失败。上游若需要真正显示图片，应显式调用飞书上传得到 `image_key`，走 `img` 组件而非 Markdown。
- **语言映射维护成本**：飞书新增语言标识需人工更新映射表。作为兜底，无法识别的语言直接透传原字符串，飞书会退化为无高亮显示。

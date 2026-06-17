# IM 消息 Markdown 格式转换器

## 功能模块描述

LLM Agent 输出的是标准 CommonMark 格式的 Markdown，而飞书卡片 JSON 2.0 的 Markdown 组件有特定的语法差异和限制。本模块负责在发送消息前将标准 Markdown 转换为飞书兼容的 Markdown 格式。

## 转换规则

### 1. 代码块语言标识标准化
LLM 常用缩写需映射到飞书支持的语言名称：
- `ts` → `typescript`
- `js` → `javascript`
- `py` → `python`
- `rb` → `ruby`
- `sh` → `shell`（飞书同时支持 `bash` 和 `shell`；`bash` 直接透传不重写）
- `yml` → `yaml`
- `cs` / `csharp` / `c#` → `c_sharp`
- `objc` / `objective-c` / `objectivec` → `objective_c`
- `glsl` → `opengl_shading_language`
- `vb` → `visual_basic`
- `coffee` / `coffeescript` → `coffee_script`
- `plaintext` / `text` / `txt` → `plain_text`
- `dockerfile` → `docker_file`
- 带元数据后缀的（如 `python {title="x"}`）→ 仅保留语言名称
- 大小写不敏感，统一转小写

### 2. 图片 URL 处理
飞书卡片只支持 `image_key` 作为图片路径，不支持外部 HTTP URL。
- `![alt](https://...)` → `[🖼 alt](https://...)` 转为文字链接
- `![](https://...)` → `[🖼 图片链接](https://...)` 无 alt 时补充默认文案

### 3. 水平分割线标准化
- `***` / `___` / `* * *` / `- - -` → `---`
- 确保分割线前后有换行

### 4. 任务列表转换
飞书不支持 `- [ ]` / `- [x]` 语法：
- `- [ ] text` → `- ⬜ text`
- `- [x] text` / `- [X] text` → `- ✅ text`

### 5. HTML 标签清理
飞书仅支持特定 HTML 标签。不支持的标签应被移除或转义：
- 支持的标签（保留）：`<br>`, `<br/>`, `<hr>`, `<hr/>`, `<font>`, `<a>`, `<at>`, `<person>`, `<text_tag>`, `<local_datetime>`, `<link>`, `<raw>`, `<number_tag>`, `<audio>`, `<img>`
- 不支持的标签（移除标签但保留内容）：`<div>`, `<span>`, `<p>`, `<b>`, `<i>`, `<em>`, `<strong>`, `<u>`, `<s>`, `<sub>`, `<sup>`, `<details>`, `<summary>`, `<blockquote>` 等

### 6. Bold+Italic 组合处理
- `***text***` → `**_text_**`（避免飞书渲染异常）

### 7. 脚注移除
- `[^1]` 引用标记和 `[^1]: ...` 定义行 → 移除

## 实现逻辑

位于 `crates/bifrost-admin/src/im_gateway/markdown_converter.rs`，提供：
- `pub fn convert_to_feishu_markdown(input: &str) -> String` — 主入口函数
- 内部按行扫描：识别围栏代码块（``` 或 ~~~）跳过内容仅改写语言标识；非代码行依次应用 HR 归一、任务列表、图片 URL、脚注、`***` 加粗斜体、HTML 标签清理。

## 应用点

所有面向飞书卡片的 Markdown 内容在写入 JSON 前都会经 `convert_to_feishu_markdown()` 处理。当前实际调用点（路径相对 `crates/bifrost-admin/src/`）：
1. `handlers/im_gateway/agent_chat.rs::process_agent_chat`（调用位于 ~line 1818） — 主 agent 回复
2. `handlers/im_gateway/agent_reply.rs::send_agent_reply_with_title`（~line 1062） — 通用带标题回复
3. `handlers/im_gateway/agent_reply.rs::send_agent_reply_with_plan`（~line 1248） — 带任务计划的回复
4. `handlers/im_gateway/agent_reply.rs::send_error_card_to_owner`（~line 1408） — 错误通知卡片
5. `handlers/im_gateway/messages.rs::build_rich_card_content`（~line 528） — 由 `messages.send` API 构造的富卡片正文
6. `im_gateway/feishu.rs::build_markdown_card`（~line 407） — 通用 Markdown 卡片构造助手

## 测试方案

### 单元测试
- `test_code_block_lang_normalization` — 验证各语言缩写映射
- `test_image_url_to_link` — 验证图片 URL 转文字链接
- `test_hr_normalization` — 验证各种 HR 写法统一为 `---`
- `test_task_list_conversion` — 验证 `- [ ]` / `- [x]` 转换
- `test_html_tag_cleanup` — 验证不支持的 HTML 标签移除
- `test_code_block_content_preserved` — 验证代码块内容不被修改
- `test_footnote_removal` — 验证脚注移除
- `test_bold_italic_combo` — 验证 `***text***` 处理
- `test_mixed_content` — 验证综合场景

实际实现还包含 UTF-8 安全测试套件（中文/日文/韩文/emoji/4 字节字符等场景，见 `markdown_converter.rs` 测试模块 `tests::test_utf8_*`）。

### 真实场景测试
`human_tests/im-markdown-converter.md` — 通过飞书 IM 发送含各种 Markdown 格式的消息并验证卡片渲染效果（planned，截至 2026-06-16 `human_tests/` 目录下尚未落地该用例文件）。

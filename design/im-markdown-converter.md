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
- `sh` / `bash` → `shell`（飞书同时支持 `bash` 和 `shell`）
- `yml` → `yaml`
- `cs` / `csharp` → `c_sharp`
- `objc` / `objective-c` → `objective_c`
- `glsl` → `opengl_shading_language`
- `vb` → `visual_basic`
- `coffee` → `coffee_script`
- `plaintext` / `text` → `plain_text`
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
- 支持的标签（保留）：`<br>`, `<br/>`, `<hr>`, `<hr/>`, `<font>`, `<a>`, `<at>`, `<person>`, `<text_tag>`, `<local_datetime>`, `<link>`, `<raw>`, `<number_tag>`, `<audio>`
- 不支持的标签（移除标签但保留内容）：`<div>`, `<span>`, `<p>`, `<b>`, `<i>`, `<em>`, `<strong>`, `<u>`, `<s>`, `<sub>`, `<sup>`, `<details>`, `<summary>`, `<blockquote>` 等

### 6. Bold+Italic 组合处理
- `***text***` → `**_text_**`（避免飞书渲染异常）

### 7. 脚注移除
- `[^1]` 引用标记和 `[^1]: ...` 定义行 → 移除

## 实现逻辑

创建 `crates/bifrost-admin/src/im_gateway/markdown_converter.rs`，提供：
- `pub fn convert_to_feishu_markdown(input: &str) -> String` — 主入口函数
- 内部按行和块级解析，处理代码块（保持内容不变）和普通文本（应用转换规则）

## 应用点

在以下函数中，将 `content` 传入 `convert_to_feishu_markdown()` 后再赋值给卡片 JSON：
1. `process_agent_chat` — 主 agent 回复（line ~1409）
2. `send_agent_reply_with_title` — 通用回复帮助函数（line ~1553）
3. `send_error_card_to_owner` — 错误通知（line ~1632）

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

### 真实场景测试
`human_tests/im-markdown-converter.md` — 通过飞书 IM 发送含各种 Markdown 格式的消息并验证卡片渲染效果

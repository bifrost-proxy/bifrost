# Agent Chat GPT Web 来源引用渲染

## 用户目标验证清单

### 必须实现

- 识别 ChatGPT Web 输出的 `[favicon + 来源文字](文章链接)` Markdown 结构。
- 将来源 favicon 作为来源文字前的 14px 图标内联展示，不再按正文大图渲染。
- 多来源引用保持为一个可点击链接，并紧凑展示每个来源图标和名称。
- 来源 favicon 不进入 Agent Chat 图片灯箱。

### 必须不破坏

- 普通 Markdown 图片、带描述文字的链接图片和消息附件继续使用现有预览与灯箱。
- 普通 Markdown 链接继续在新窗口打开，并保留 `rel="noreferrer"`。
- 来源引用在亮色、暗色主题下都使用 Ant Design CSS token，保持可读性和焦点反馈。

### 必须真实验证

- 使用真实历史中 ChatGPT Web 的 Reuters/AP 双来源 Markdown 形态执行 Playwright 回归。
- 验证来源组件有两个 14px favicon、来源文字和原文章链接，且不带图片预览标识。
- 同一消息内加入普通正文图片，验证它仍是唯一可预览图片并可打开灯箱。
- 切换亮色与暗色主题，验证引用组件均可见且不撑高正文。

### 必须交付

- 实现、单元测试、Playwright E2E、`human_tests/` 用例和索引保持一致。
- 完成两轮 Review/Fix/Test、提交、PR 与远端 CI 90% 覆盖率门禁看护。

## 原始结构与根因

ChatGPT Web 在带搜索来源的回答中会输出如下 Markdown：

```markdown
[![](https://www.google.com/s2/favicons?domain=https://www.reuters.com&sz=128)Reuters+2![](https://www.google.com/s2/favicons?domain=https://apnews.com&sz=128)AP News+2](https://www.reuters.com/example)
```

两个 favicon 和来源文字都位于同一个链接节点中。旧实现只自定义了通用 `img` renderer，因此 favicon 会获得正文图片的最小尺寸、边框、点击预览和灯箱收集逻辑，最终显示为两张大图。

## 方案

在 Markdown 链接 renderer 中检查 AST 子节点，仅当链接内容满足以下全部条件时启用来源引用组件：

1. 子节点严格由空 `alt` 的 Google favicon 图片与后随文本组成。
2. favicon 使用 HTTPS，host 为 `google.com`/`www.google.com`，path 为 `/s2/favicons`，并含非空 `domain` 参数。
3. 每个 favicon 后都有非空来源名称。

匹配后由链接 renderer 直接输出 `agent-chat-citation`，内部为 14px favicon 与来源名称；不再调用正文图片 renderer。普通链接或普通链接图片不匹配该结构，继续走原实现。图片灯箱提取逻辑只排除位于完整 GPT Web 来源链接语法内部的 favicon，避免隐藏的引用图标污染灯箱计数；独立 favicon 或带描述性 `alt` 的链接图片仍按普通正文图片处理。

样式使用 `--ant-color-border-secondary`、`--ant-color-fill-quaternary`、`--ant-color-primary-border` 和 `--ant-color-primary-bg`，不新增硬编码主题分支。组件采用紧凑 pill 形态，来源之间以细分隔线区分，支持窄宽度换行但每个“图标 + 来源名”自身不拆分。

## 验证计划

- 单元测试：双来源、单来源、普通链接图片、描述性图片、非法 URL、缺失标签、混合节点、灯箱过滤。
- E2E：mock 真实 GPT Web Markdown，验证来源链接、尺寸、灯箱排除、普通图片回归和双主题。
- human_tests：在隔离服务与真实浏览器中执行同一场景并记录结果。
- Coverage：本地不运行高成本全量 coverage；由远端 `coverage-all.sh --json --gate` 90% 棘轮门禁兜底。

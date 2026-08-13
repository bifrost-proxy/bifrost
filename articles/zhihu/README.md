# Bifrost 知乎待发布稿件

本目录从 Bifrost 掘金账号的已发布文章拉取，保留原始正文，并按 `zhihu-publish` 的输入约束做最小兼容处理。当前仅准备本地稿件，没有在知乎创建草稿或发布文章。

| 稿件 | 来源 | 掘金文章 ID | 状态 |
| --- | --- | --- | --- |
| [为什么我们认为 Bifrost 是 AI 时代最好用的开源代理软件](./why-bifrost-is-the-open-source-proxy-for-the-ai-era.md) | [掘金原文](https://juejin.cn/post/7672968698513489955) | 7672968698513489955 | 知乎稿件已准备，尚未发布 |
| [使用飞书机器人让 Codex 24 小时在线：随时随地给你的 AI 工程师派活](./keep-codex-online-with-feishu-and-bifrost.md) | [掘金原文](https://juejin.cn/post/7672957997422346303) | 7672957997422346303 | 知乎稿件已准备，尚未发布 |
| [让 Codex 持续进化：用 Bifrost 把你每天点过的 API 变成 Skill](./turn-bifrost-traffic-into-codex-skills.md) | [掘金原文](https://juejin.cn/post/7672956343582818313) | 7672956343582818313 | 知乎稿件已准备，尚未发布 |

## 兼容处理

- 去掉正文开头重复的一级标题，由知乎使用 frontmatter 的 `title` 渲染文章标题。
- 将掘金 Markdown 表格转换为知乎发布脚本支持的分组列表。
- 将缩进代码块转换为 fenced code block。
- 将带时效签名的掘金私有图片下载为本地 WebP，并改为相对路径；正式发布时由 `zhihu-publish` 上传到知乎图床。

## 后续发布

先对单篇执行 dry-run，确认标题、字符数和图片数量；用户明确要求后，再执行 `--draft-only` 或 `--publish`。

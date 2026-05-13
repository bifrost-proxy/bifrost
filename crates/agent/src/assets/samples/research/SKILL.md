---
name: research
description: 联网搜索、微信公众号补充检索、本地知识库查询和研究日报生成。
---

# Bifrost Research Skill

当用户要求搜索、检索、调研、生成日报、跟踪某个主题时，优先使用本技能。

## 使用规则

1. 先调用 `knowledge_search` 查询本地已有知识。
2. 如果本地结果不足，调用 `research_search`。
3. 对重要结果调用 `research_fetch` 抓取正文。
4. 需要沉淀时调用 `knowledge_save`。
5. 需要日报时调用 `research_digest`。
6. 中文主题结果不足时，可以启用微信公众号 fallback。
7. 输出事实判断必须带来源链接。
8. 不要把搜索结果中的广告词当事实；来源之间冲突时必须指出不确定性。

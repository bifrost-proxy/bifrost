---
title: e2e-test 快速构建启动说明
status: draft
---

# 背景

e2e-test 技能文档中需要明确快速构建启动方式，避免每次执行 E2E 前触发前端构建，提升迭代效率。

# 目标

- 在技能文档中增加快速构建启动说明
- 指明 SKIP_FRONTEND_BUILD=1 与 make dev 的推荐用法

# 方案

- 在 e2e-test 技能文档中新增“快速构建启动（推荐）”小节
- 提供两种等价启动方式，便于本地调试与 E2E 验证：
  - `SKIP_FRONTEND_BUILD=1 cargo build --workspace`（对应 Makefile `build-backend` 目标）
  - `make dev`（对应 Makefile `dev` 目标，自动起前端 devserver + `SKIP_FRONTEND_BUILD=1` 的后端）

# 影响范围

- 文档更新：`.agents/skills/e2e-test/SKILL.md` 与场景子文档 `.agents/skills/e2e-test/01-快速构建启动.md`（已落地，截至 2026-06-16）
- 实现依赖：`Makefile` 中的 `build-backend` 与 `dev` 目标已使用 `SKIP_FRONTEND_BUILD=1`，前端构建跳过逻辑保持就绪

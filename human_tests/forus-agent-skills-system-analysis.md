# 开发版 Forus Agent/Skills 体系分析文档真实场景测试

## 功能模块说明

验证 `design/forus-agent-skills-system-analysis.md` 是否真实覆盖开发版 Forus 使用的 Agent/Skills 工程体系：`AGENTS.md` 约束、repo-local skills、技能开发与安装分发、Skills runtime、使用的平台/工具/能力、亮点、问题和学习路径。

## 前置条件

1. 当前工作目录为仓库根目录：`~/work/github/bifrost`
2. 已创建或更新：
   - `design/forus-agent-skills-system-analysis.md`
   - `human_tests/forus-agent-skills-system-analysis.md`
   - `human_tests/readme.md`

## 测试用例列表

### TC-FAS-01：分析文档覆盖 AGENTS 与 repo-local skills

**操作步骤：**

1. 执行：
   ```bash
   rg -n "AGENTS.md|e2e-test|e2e-verify|rust-project-validate|github-actions-pat|design-md|codex-task-inspector|site-cookie-login" design/forus-agent-skills-system-analysis.md
   ```
2. 检查输出是否覆盖总控规则和 7 个 repo-local skills。

**预期结果：**

- 文档说明 `AGENTS.md` 的任务模式、完成定义、human_tests、两轮 Review/Fix/Test、覆盖率和 CI 看护。
- 文档逐项分析 `.agents/skills/` 下的项目内技能。

### TC-FAS-02：分析文档覆盖技能开发、安装分发和 runtime

**操作步骤：**

1. 执行：
   ```bash
   rg -n "SKILL.md|skill_remote.md|install-skill|Claude Code|Codex|Trae|Cursor|GitHub Copilot|Universal|SkillManifest|SkillRegistry|SkillAuthoringSession|SkillPackager|渐进式披露" design/forus-agent-skills-system-analysis.md
   ```
2. 检查输出是否覆盖技能格式、安装平台、runtime 组件和渐进式披露。

**预期结果：**

- 文档说明技能如何写、如何安装、如何被 Bifrost runtime 消费。
- 文档列出 Claude Code、Codex、Trae、Cursor、GitHub Copilot 和通用 `.agents/skills` 平台。

### TC-FAS-03：分析文档覆盖平台、工具、能力、亮点和问题

**操作步骤：**

1. 执行：
   ```bash
   rg -n "用了哪些平台|用了哪些工具和能力|亮点|问题和风险|学习路径|建议改进清单" design/forus-agent-skills-system-analysis.md
   ```
2. 执行：
   ```bash
   rg -n "GitHub Actions|GitHub REST API|Bifrost CLI|Chrome|Puppeteer|Playwright|MCP|Feishu|Weixin|human_tests|coverage" design/forus-agent-skills-system-analysis.md
   ```
3. 检查输出是否覆盖平台、工具、能力和评价部分。

**预期结果：**

- 文档包含用户要求的体系设计、亮点、问题、平台、工具和能力分析。
- 文档提供学习路径和改进建议。

### TC-FAS-04：human_tests 索引同步

**操作步骤：**

1. 执行：
   ```bash
   rg -n "forus-agent-skills-system-analysis|开发版 Forus Agent/Skills 体系分析" human_tests/readme.md
   ```
2. 检查输出是否包含 `human_tests/forus-agent-skills-system-analysis.md` 的索引行。
3. 执行：
   ```bash
   ! rg -n "^\\*\\*总计：|总计：[0-9]+ 个测试文件" human_tests/readme.md
   ```
4. 检查索引更新没有新增全局总计数字。

**预期结果：**

- `human_tests/readme.md` 包含本测试文档索引。
- 索引行的测试用例数为 `4`。
- `human_tests/readme.md` 不包含全局测试文件数或测试用例数总计。

## 清理步骤

本测试只读取文档和索引，不启动服务、不写临时数据，无需清理。

## 执行记录

- 2026-07-08：TC-FAS-01 PASS。`rg` 输出覆盖 `AGENTS.md`、`e2e-test`、`e2e-verify`、`rust-project-validate`、`github-actions-pat`、`design-md`、`codex-task-inspector`、`site-cookie-login`。
- 2026-07-08：TC-FAS-02 PASS。`rg` 输出覆盖 `SKILL.md`、`skill_remote.md`、`install-skill`、Claude Code、Codex、Trae、Cursor、GitHub Copilot、Universal、`SkillManifest`、`SkillRegistry`、`SkillAuthoringSession`、`SkillPackager` 和渐进式披露。
- 2026-07-08：TC-FAS-03 PASS。`rg` 输出覆盖“用了哪些平台”“用了哪些工具和能力”“亮点”“问题和风险”“学习路径”“建议改进清单”，并覆盖 GitHub Actions、GitHub REST API、Bifrost CLI、Chrome、Puppeteer、Playwright、MCP、Feishu、Weixin、`human_tests`、`coverage`。
- 2026-07-08：TC-FAS-04 PASS。`human_tests/readme.md` 包含 `forus-agent-skills-system-analysis.md` 索引行，测试用例数为 4，且未新增全局测试文件数或测试用例数总计。

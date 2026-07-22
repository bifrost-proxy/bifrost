# Git 历史统计报告

## 目标

提供一个不依赖第三方 Python 包的仓库历史分析脚本，用统一、可复现的口径回答：

- 仓库从哪一天开始、当前可达历史共有多少提交；
- 项目最初 30 个自然日的提交活跃度；
- 指定月份每天有多少 commit，包括零提交日期；
- merge/non-merge、贡献者、Conventional Commit 类型和星期分布。

## 统计边界

- 默认 revision 为 `origin/main`，只统计主线可达提交，避免把未合并分支或废弃 worktree 重复计入。
- 默认使用 author date，并统一转换为 `Asia/Shanghai`。author date 更接近实际开发日期；committer date 可能在 rebase 后变化。
- merge commit 计入总数，但同时单列，便于不同合并策略之间比较。
- “初始阶段”定义为首个提交日期起连续 30 个自然日。脚本允许通过 `--initial-days` 调整。
- 报告记录 ref 解析出的完整 SHA。分支会移动，历史报告应以该 SHA 作为可复核基准。

## 接口

```bash
python3 scripts/analyze_git_history.py \
  --revision origin/main \
  --year 2026 \
  --month 2 \
  --timezone Asia/Shanghai \
  --date-field author \
  --initial-days 30 \
  --output reports/git-history-2026-02.md
```

脚本只读取 Git 对象；除显式 `--output` 文件外不修改仓库状态。

## 验证策略

- 单元测试使用临时 Git 仓库构造跨时区提交、零提交日和 merge commit。
- Shell E2E 在真实 Bifrost 仓库运行脚本，并用独立 `git log` 命令核对目标月总数和每日合计。
- `human_tests/git-history-report.md` 记录面向使用者的生成与复核步骤。

# Git 历史统计报告真实场景测试

## 功能模块说明

验证 `scripts/analyze_git_history.py` 能从真实 Bifrost 主线历史生成可复核的 Markdown 报告，完整列出 2026 年 2 月每个自然日的 commit 数，并清楚记录 revision、日期字段、时区和 merge 统计口径。

## 前置条件

- 位于 Bifrost Git 仓库根目录。
- 已执行 `git fetch origin main`，本地存在 `origin/main`。
- 系统提供 Python 3.9+ 和 IANA 时区数据库。

## 测试用例

### TC-GHR-01：生成 2026 年 2 月报告

操作步骤：

1. 执行：
   `python3 scripts/analyze_git_history.py --repo . --revision origin/main --year 2026 --month 2 --timezone Asia/Shanghai --date-field author --initial-days 30 --output reports/git-history-2026-02.md`
2. 打开 `reports/git-history-2026-02.md`。
3. 检查报告包含统计口径、仓库概览、月度趋势、初始阶段、逐日统计、贡献者、提交类型和结论。

预期结果：

- 命令退出码为 0，报告文件非空。
- 报告记录 `origin/main` 解析出的完整 commit SHA。
- 首个提交日期为 `2026-02-09`，初始 30 天窗口为 `2026-02-09` 至 `2026-03-10`。

### TC-GHR-02：独立核对 2 月总数和每日之和

操作步骤：

1. 执行 `bash e2e-tests/tests/test_git_history_report.sh`。
2. 该脚本独立读取 `git log HEAD --format=%aI`，转换到 `Asia/Shanghai` 后计算 2026 年 2 月总数。
3. 对比报告“全部 commit”汇总与 28 行每日数字之和。

预期结果：

- E2E 脚本退出码为 0。
- 当前主线 2026 年 2 月总数为 200，逐日之和同样为 200。
- 报告恰好包含 28 行 2 月日期。

### TC-GHR-03：零提交日期和 merge 口径可见

操作步骤：

1. 在报告逐日表中检查 `2026-02-01`、`2026-02-08`、`2026-02-17` 和 `2026-02-24`。
2. 检查本月汇总中的全部、非合并与 merge commit 数。

预期结果：

- 上述日期均存在且全部 commit 为 0，没有从表格中消失。
- 2026 年 2 月显示 200 个非合并 commit、0 个 merge commit，三项数字关系一致。

## 清理步骤

- 本用例生成的 `reports/git-history-2026-02.md` 是任务交付物，不删除。
- E2E 使用的临时报告通过 `trap` 自动清理，不残留临时文件或后台进程。

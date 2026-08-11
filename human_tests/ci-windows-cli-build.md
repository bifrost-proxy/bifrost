# CI Windows CLI Build

## 功能模块说明

验证 Windows CLI 冷缓存构建有足够预算完成正常 release binary 与 pinned upgrade target fixture，同时保留原有 artifact 和下游依赖契约。

## 前置条件

- 在仓库根目录执行。
- 本地静态验证需要 Ruby 标准库 YAML parser。
- 远端验证需要当前 PR 的 GitHub Actions `CI` run。

## 测试用例列表

### TC-WCB-01: Windows CLI 双构建预算静态回归

**操作步骤**：

1. 执行：
   ```bash
   ruby -ryaml -e 'job = YAML.load_file(".github/workflows/ci.yml")["jobs"]["build-cli-windows"]; raise job.inspect unless job["timeout-minutes"] == 90; step = job["steps"].find { |item| item["name"] == "Build pinned upgrade target fixture" }; raise step.inspect unless step && step["if"] == "matrix.target == '\''x86_64-pc-windows-msvc'\''"; puts "windows cli build budget ok"'
   ```
2. 执行：
   ```bash
   rg -n 'build-cli-windows:|timeout-minutes: 90|Build pinned upgrade target fixture|bifrost-upgrade-target-' .github/workflows/ci.yml
   ```

**预期结果**：

- Ruby 命令输出 `windows cli build budget ok`。
- `build-cli-windows` 外层预算为 90 分钟。
- pinned upgrade fixture 仍只在 Windows x86_64 构建，并上传独立 artifact。

### TC-WCB-02: Windows CLI 冷构建远端回归

**操作步骤**：

1. 推送修改到 PR 分支。
2. 使用 GitHub Actions PAT fail-fast 看护对应 head SHA 的 `CI` run。
3. 检查 `Build Windows CLI (x86_64-pc-windows-msvc)` 的 `Build CLI`、`Build pinned upgrade target fixture` 和两个 upload step。
4. 检查依赖 `build-cli-windows` 的 Windows desktop、shell、rules 与 runner jobs。

**预期结果**：

- x86_64 Windows CLI job 不再于 60 分钟被取消，两个 binary 均成功上传。
- Windows aarch64 CLI job 成功。
- 所有下游 Windows jobs 成功；若出现独立失败，必须按日志归因并修复或提供可核验的外部阻塞证据。

## 清理步骤

- 静态检查不创建临时文件、不启动 Bifrost、不使用 9900 端口。

## 执行记录

- 2026-08-12：TC-WCB-01 通过。PR #478 的 run `31514641216` attempt 1 与 attempt 2 均在 x86_64 pinned fixture 编译期间到达 60 分钟上限并被取消，日志未出现编译错误；将 job 预算调整为 90 分钟后，Ruby YAML 静态断言与 `rg` 契约检查均通过。TC-WCB-02 由推送后的新 CI run 验证。

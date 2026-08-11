# CI Windows CLI 构建预算

## 背景

`build-cli-windows` 同时产出正常 release CLI 和 Windows x86_64 升级 E2E 使用的 pinned-version fixture。冷缓存 runner 上两次构建的总耗时可能超过 60 分钟：PR #478 的 CI run `31514641216` 两次都在 `Build pinned upgrade target fixture` 编译期间到达 60 分钟上限并被平台取消，日志没有编译错误。

## 决策

- `build-cli-windows.timeout-minutes` 使用 90 分钟，覆盖冷缓存下的两次 release 编译及 artifact 上传。
- 保留两次构建和 pinned fixture 的版本注入，不通过删除升级测试或复用错误版本的 binary 缩短时间。
- 下游 Windows desktop、rules、shell 和 runner 仍只在 CLI artifact 成功产出后启动，依赖关系不变。

## 验证

- 静态解析 `.github/workflows/ci.yml`，断言 `jobs.build-cli-windows.timeout-minutes == 90`，并确认 pinned fixture step 仍仅对 `x86_64-pc-windows-msvc` 执行。
- 推送后看护 GitHub Actions `CI`：两个 Windows CLI matrix job 必须成功，x86_64 pinned fixture 必须完成并上传；随后所有依赖它的 Windows jobs 必须完成。
- 此变更只调整 CI 外层预算，不修改 Rust 生产代码；单元测试、业务 E2E 与 coverage 阈值不变。

## 风险

真正的编译错误仍会立即返回非零；90 分钟只避免健康的冷构建被外层预算误杀。若运行时间继续逼近 90 分钟，应拆分 fixture 构建或优化缓存，而不是继续无限放宽超时。

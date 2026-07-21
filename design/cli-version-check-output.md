# CLI version-check 输出修复

## 背景

`bifrost version-check` 是用户在本地检查“我这台机器上的 Bifrost 是不是最新”的入口，也是 `bifrost upgrade` 的前置检查。历史实现里存在一个 silent-success 的严重回归：

- 管理端 `/_bifrost/api/system/version-check` 已经能返回正确 JSON；
- CLI 打印逻辑却读的是旧的字段名（`current` / `latest` / `update_available`），实际返回是 `current_version` / `latest_version` / `has_update`；
- 命令 exit code 0，stdout 空白。

用户看到 “命令执行成功但没有任何输出”，会误以为：

- 命令没执行或被静默跳过；
- 网络异常；
- 版本检查功能坏了。

本方案的核心是把 CLI 与服务端的字段协议对齐，并给缺失最新版本的场景一个明确的提示行，避免再出现“成功却空白”的现象。同时，为无运行代理场景补一条 standalone fallback：直接查 GitHub。

仓库同时发布独立资源 release 后，版本选择还必须隔离产品版本与资源版本：`moss-runtime-v1.0.0` 之类的 tag 即使被 GitHub 标成 `Latest`，也不能进入 CLI、Admin、Tray 或 Desktop upgrade。只有严格匹配 `v<major>.<minor>.<patch>` 的非 draft、非 prerelease Bifrost release 才能成为稳定更新目标；若 `/releases/latest` 被资源 release 污染，则扫描已发布 release 列表，禁止回退到可能尚无安装资产的裸 git tag。

## 用户目标验证清单

### 必须实现

- `bifrost version-check` 在有正在运行的代理时，通过 admin API 拉到 `current_version` / `latest_version` / `has_update` / `release_highlights` / `release_url`，并按此字段打印。
- `has_update=true` 时输出 `Update available!`（含火箭 emoji），并列出 `release_highlights`（若非空），附 `release_url` 与升级提示 `To upgrade, run: bifrost upgrade`。
- `has_update=false` 时输出 `You are running the latest version.`（含 checkmark emoji）。
- 服务端返回但 `latest_version=null` 时输出 `Could not determine the latest version. Check your network connection.`（含警告 emoji）。
- 无运行中的代理时，回落到 `handle_version_check_standalone`：直接调用 `update_check::get_latest_version_fresh_with_diagnostics` 查 GitHub，组装成同样字段名的 JSON，交给同一个输出函数渲染。
- `/releases/latest` 的 redirect/API 结果必须通过严格 Bifrost release tag 校验；`moss-runtime-v*`、`vmoss-runtime-v*`、draft、prerelease 和非三段数字版本均拒绝。
- Latest 被其它 release 污染时，从 published releases API 选择最新稳定 Bifrost release；不得使用裸 tags API 作为 upgrade 目标来源。
- 独立 MOSS runtime workflow 必须设置 `make_latest: false`，不能改变仓库产品 Latest。
- 整段输出上下加黄色 `─` 分隔线，方便终端识别。
- 单元测试直接验证不同 JSON 返回下的行文本，不依赖 stdout 捕获。
- E2E 断言不再把 “空输出” 视为成功。

### 必须不破坏

- 服务端 `/_bifrost/api/system/version-check` 返回契约与其它调用方（Web Settings, `bifrost upgrade`）一致。
- `bifrost upgrade` 的前置流程不变。
- 缓存策略、GitHub 请求频率不变。
- 辅助资源 release 的发布与升级检查互不影响；资源版本仍可由 MOSS 初始化器按独立 tag 下载。
- 断网 / GitHub 5xx / rate-limit 场景，`version-check` 不 panic、不写坏本地缓存。
- CLI 其它子命令的输出格式不变。
- Exit code 语义：命令成功执行退出 0；GitHub / API 拉取失败但仍成功打印 fallback 提示，也退出 0；只有 CLI 本身内部错误退出非零。

### 必须真实验证

- 起一份本地 Bifrost + 有更新可用的 GitHub 环境：`bifrost version-check` 打印当前版本、最新版本、`Update available!`、highlights、release_url、upgrade 提示，包裹在两条 `─` 分隔线之间。
- 起一份本地 Bifrost + 已是最新版本：打印 `You are running the latest version.`。
- 起一份本地 Bifrost + 断网：打印 `Could not determine the latest version. Check your network connection.`。
- 不起 Bifrost 直接跑：走 standalone fallback，打印同格式输出。
- `has_update=true` 且 GitHub Release Notes 没有 highlights：升级提示打印但不渲染空的 highlights 段落。

## 产品语义

### 三种终端输出形态

#### 有更新（`has_update=true`）

```
──────────────────────────────────────────────
Current version: 0.0.135
Latest version:  0.0.137

🚀 Update available!

Highlights:
  - CLI version-check output regression fix
  - status Service Overview
  - remote file protocol GA

Release notes: https://github.com/bifrost-proxy/bifrost/releases/tag/v0.0.137

To upgrade, run: bifrost upgrade
──────────────────────────────────────────────
```

#### 已是最新（`has_update=false`）

```
──────────────────────────────────────────────
Current version: 0.0.137
Latest version:  0.0.137

✅ You are running the latest version.
──────────────────────────────────────────────
```

#### 无法获取最新版本（`latest_version=null`）

```
──────────────────────────────────────────────
Current version: 0.0.137

⚠ Could not determine the latest version. Check your network connection.
──────────────────────────────────────────────
```

### `has_update=true` 但 highlights 为空

打印升级提示、release_url、`bifrost upgrade` 提示；但不渲染 `Highlights:` 段落，避免出现空的空 heading。

### Standalone fallback 与 admin API 的一致性

`handle_version_check_standalone` 直接查 GitHub，返回值构造成与 admin API 相同字段名的 JSON：

```json
{
  "current_version": "0.0.137",
  "latest_version": "0.0.138",
  "has_update": true,
  "release_highlights": ["…"],
  "release_url": "https://…"
}
```

再交给同一个 `format_version_check_output(json) -> String` 输出函数。这样 CLI 无论是否有代理运行，用户看到的格式一致。

## 技术细节

### 关键代码位点

- `crates/bifrost-cli/src/commands/mod.rs::handle_version_check`：主入口。
  - 尝试拉本地代理 `ConfigApiClient::version_check`（`config/client.rs`），成功走 admin path；失败走 `handle_version_check_standalone`。
- `crates/bifrost-cli/src/commands/update_check.rs`：
  - `get_latest_version_fresh_with_diagnostics`：直查 GitHub。
  - `parse_release_highlights`：从 Release Notes markdown 抽取 highlights；有多种 fallback 分支。
  - `spawn_update_check_notice`：`start` 快路径用；与本方案共用同一份 highlights 解析。
- `crates/bifrost-core/src/version_check.rs`：`is_newer_version` / `release_page_url` 等辅助函数。
- `crates/bifrost-cli/src/commands/config/client.rs::ConfigApiClient::version_check`：POST `/system/version-check?refresh=true`。
- 抽取 `format_version_check_output(payload: &VersionCheckPayload) -> String`：可单测的输出格式化函数。

### `VersionCheckPayload` 契约

```rust
pub struct VersionCheckPayload {
    pub current_version: String,
    pub latest_version: Option<String>,
    pub has_update: bool,
    pub release_highlights: Vec<String>,
    pub release_url: Option<String>,
}
```

- `current_version` 必有：来自本地二进制 `env!("CARGO_PKG_VERSION")`。
- `latest_version=None`：GitHub 拉取失败 / rate-limited。
- `has_update`：`latest_version.is_some() && is_newer_version(latest, current)`。
- `release_highlights`：Release Notes markdown 解析结果；可能为空。
- `release_url`：GitHub Releases 页面 URL；可能为空。

### `format_version_check_output` 三分支

```rust
pub fn format_version_check_output(p: &VersionCheckPayload) -> String {
    let mut out = String::new();
    out.push_str(&yellow_line());
    writeln!(out, "Current version: {}", p.current_version).unwrap();
    match (p.latest_version.as_deref(), p.has_update) {
        (Some(latest), true) => {
            writeln!(out, "Latest version:  {}", latest).unwrap();
            writeln!(out, "\n🚀 Update available!\n").unwrap();
            if !p.release_highlights.is_empty() {
                writeln!(out, "Highlights:").unwrap();
                for h in &p.release_highlights {
                    writeln!(out, "  - {}", h).unwrap();
                }
                out.push('\n');
            }
            if let Some(url) = &p.release_url {
                writeln!(out, "Release notes: {}\n", url).unwrap();
            }
            writeln!(out, "To upgrade, run: bifrost upgrade").unwrap();
        }
        (Some(latest), false) => {
            writeln!(out, "Latest version:  {}", latest).unwrap();
            writeln!(out, "\n✅ You are running the latest version.").unwrap();
        }
        (None, _) => {
            writeln!(out, "\n⚠ Could not determine the latest version. Check your network connection.").unwrap();
        }
    }
    out.push_str(&yellow_line());
    out
}
```

### Admin API 契约

`GET /_bifrost/api/system/version-check?refresh=true` 返回：

```json
{
  "current_version": "0.0.137",
  "latest_version": "0.0.138",
  "has_update": true,
  "release_highlights": ["…"],
  "release_url": "https://github.com/bifrost-proxy/bifrost/releases/tag/v0.0.138"
}
```

- `refresh=true` 强制绕过本地缓存直查 GitHub。CLI `version-check` 默认带上，因为用户敲这条命令的目的就是主动查。
- 缓存策略、rate-limit、GitHub 5xx 处理都在服务端；CLI 只关心 payload。

### Standalone fallback 语义

Admin API 不可达（服务未启动 / 无网络到本机）时，`handle_version_check_standalone`：

1. 调用 `update_check::get_latest_version_fresh_with_diagnostics()`：直接 HTTP GET GitHub Releases API。
2. `parse_release_highlights` 抽取 highlights。
3. 组装 `VersionCheckPayload`。
4. 交给 `format_version_check_output`。

这一路不共享服务端缓存，是有意的：CLI standalone 使用场景本来就少，避免为它维护第二个缓存文件。

### 产品 Release 与独立资源 Release 的隔离

- 稳定更新目标只接受 `v<major>.<minor>.<patch>`，并同时排除 GitHub 标记为 draft / prerelease 的条目；`moss-runtime-v*`、`vmoss-runtime-v*` 和带后缀的 beta/alpha tag 都不能进入稳定升级路径。
- `releases/latest` redirect 与 API 仍是首选，以保持现有检查延迟和 GitHub 语义；若 Latest 被独立资源 Release 污染，则从 published Releases 列表恢复，不使用可能没有安装资产的裸 Git tag。
- Published Releases fallback 每页请求 100 条并扫描到空页，在所有已取回页面中选择最大稳定 semver；后续页偶发失败时保留此前页面的最佳候选。仓库 Release 集合有限且受控，因此不设置会在资源 Release 长期累积后重新产生 gap 的固定页数上限。
- 2026-07-21 对仓库现有 159 个 Release 做过全量审计：第 1 页含 97 个稳定产品 Release、2 个 prerelease 与 1 个 MOSS runtime Release；第 2 页是历史 alpha/beta，产品稳定 tag 均符合严格三段版本格式。当前 `v0.0.158` 的 macOS/Linux/Windows CLI、desktop 与 `bifrost-v0.0.158-checksums.txt` 资产矩阵完整。

## CLI / Web / Admin API 呈现

### CLI

- `bifrost version-check`：无新参数，无 flag 变化。
- 输出格式如上三分支。
- Exit code：始终 0（除非 CLI 内部致命错误）。

### Web

- Settings 页面已有 `Version Check` 卡片；本方案不改前端渲染。前端使用同一个 admin API，字段本来就是新版；只是 CLI 侧读错。

### Admin API

- `/_bifrost/api/system/version-check` 契约不变。

## Sync 边界

- 本方案不涉及同步。版本检查是每台机器本地行为；GitHub 结果不 sync。
- `release_highlights` 缓存在本机；不同机器可能显示不同（取决于缓存新鲜度）。

## 实现切分

### Phase 1：字段对齐 + fallback

- `handle_version_check` 读 `current_version` / `latest_version` / `has_update` / `release_highlights` / `release_url`。
- 无运行代理时走 `handle_version_check_standalone`。

### Phase 2：抽出可测试格式化函数

- `format_version_check_output(payload) -> String` 独立函数。
- 三分支：`has_update=true`、`has_update=false`、`latest_version=None`。
- 处理 `has_update=true` 但 highlights 为空的边界。

### Phase 3：CLI 回归测试

- 单元测试：4 个 case（见测试方案）。
- E2E：更新 `test_cli_online_commands_e2e.sh`，不再放过空输出。

### Phase 4：手工回归

- 更新 `human_tests/cli-import-export.md` TC-CIE-14，明确回归点。

## 测试方案

### 单元测试

放在 `crates/bifrost-cli/src/commands/mod.rs` 或 `commands/update_check.rs` 的 `#[cfg(test)]`：

- `version_check_prints_update_available`：`current_version=0.0.135, latest_version=0.0.137, has_update=true, release_highlights=["A","B"], release_url=Some(...)`。断言输出包含 `Current version: 0.0.135`、`Latest version:  0.0.137`、`🚀 Update available!`、`Highlights:`、`- A`、`- B`、release_url、`To upgrade, run: bifrost upgrade`。
- `version_check_prints_latest`：`has_update=false`。断言包含 `Current version:`、`Latest version:`、`✅ You are running the latest version.`；不包含 `Update available` / `Highlights` / `To upgrade`。
- `version_check_prints_missing_latest`：`latest_version=None`。断言包含 `Current version:`、`⚠ Could not determine the latest version. Check your network connection.`；不包含 `Latest version:` 行、不包含 `Update available` / `✅`。
- `version_check_prints_update_no_highlights`：`has_update=true` 但 `release_highlights=[]`。断言包含升级提示与 `release_url`，但不出现空的 `Highlights:` heading。

### E2E 测试

更新 `e2e-tests/tests/test_cli_online_commands_e2e.sh` 中的 `version-check` 断言：

- 允许的成功输出必须匹配以下三种之一：
  - `Update available!`（含 highlights + upgrade 提示）
  - `You are running the latest version.`
  - `Could not determine the latest version. Check your network connection.`
- 空输出必须 fail。
- 保留原有 exit code 0 断言。

执行方式：

```
BIFROST_DATA_DIR=./.bifrost-test-<run-id> bash e2e-tests/tests/test_cli_online_commands_e2e.sh
```

### 真实场景测试

- `human_tests/cli-import-export.md` TC-CIE-14：
  - 步骤 1：起本地服务，跑 `bifrost version-check`，逐条比对输出，不允许空。
  - 步骤 2：不起服务，跑 `bifrost version-check`（走 standalone fallback），逐条比对输出。
  - 步骤 3：断网跑 `bifrost version-check`，观察 `⚠` 提示行。

## Review / Fix / Test 闭环

- 提交前：`cargo fmt --all -- --check`、`cargo clippy --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`。
- E2E：`bash e2e-tests/tests/test_cli_online_commands_e2e.sh`。
- 本地 CI：`bash scripts/ci/local-ci.sh`。
- `rust-project-validate`。
- 手工：TC-CIE-14。

## 风险与决策

- **决策 1**：Standalone fallback 直查 GitHub，不使用共享缓存。理由：CLI standalone 使用场景少；再维护一份缓存不值。
- **决策 2**：`refresh=true` 默认打开。理由：用户敲这条命令的语义就是“现在告诉我最新版本”，走缓存反直觉。
- **决策 3**：GitHub 不可达时 exit code 依然 0。理由：用户本地二进制没坏，`version-check` 也成功产出“无法确定”这个明确信号；exit code 只表达 CLI 本身。反例是非零 exit code 让 shell 脚本感知 “未更新” 与 “未能查询” 的区别；这可以由后续的 `--strict` 开关补，不作为默认。
- **决策 4**：Emoji 保留（🚀 / ✅ / ⚠）。理由：终端可读性、与 Settings UI 视觉一致；不打算做 `--no-emoji`（可 Piped 场景由用户 grep）。
- **风险**：GitHub Release Notes 格式变化可能让 `parse_release_highlights` 抽不到 highlights。缓解：`parse_release_highlights` 已有多个 fallback 分支（highlights section、features section、curly apostrophe What's New、plain highlights）；用例已覆盖，未来可加更多。

## 文档更新要求

- 更新 `human_tests/cli-import-export.md` 中 TC-CIE-14 的说明与回归点。
- 同步 `human_tests/readme.md` 中对应条目的测试用例数与说明。
- 本方案不改 README；`bifrost version-check` 章节已经存在，不涉及行为变更。

## 依赖项

- `crates/bifrost-cli/src/commands/mod.rs`
- `crates/bifrost-cli/src/commands/update_check.rs`
- `crates/bifrost-core/src/version_check.rs`
- `crates/bifrost-cli/src/commands/config/client.rs`
- `e2e-tests/tests/test_cli_online_commands_e2e.sh`
- `human_tests/cli-import-export.md`
- `human_tests/readme.md`

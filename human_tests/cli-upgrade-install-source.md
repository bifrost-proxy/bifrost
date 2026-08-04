# CLI Upgrade 安装来源识别

## 功能模块说明

验证 `bifrost update` 与 `bifrost upgrade` 等价，并自动识别 Homebrew、npm、pnpm、官方安装脚本与手动二进制；npm/pnpm 必须沿原包管理器升级固定目标版本，并在升级后解析新平台二进制。所有动态测试使用临时目录、fake package-manager/global root 和当前 debug binary，不访问 registry、不修改用户全局包、不停止或替换正式 Bifrost 服务。

## 前置条件

1. 位于 Bifrost 仓库根目录。
2. 已安装 Rust、Node.js、Bash 和 Python 3；Windows CI 另行执行 PowerShell 动态回归。
3. 构建当前 CLI：

   ```bash
   SKIP_FRONTEND_BUILD=1 cargo build -p bifrost-cli --bin bifrost
   ```

## 测试用例

### TC-CUIS-01 来源矩阵与冲突优先级

操作步骤：

```bash
SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli install_method --lib -- --nocapture
```

预期结果：

- Homebrew、npm、pnpm、script marker、旧脚本路径、manual 和 unknown 均命中对应 variant。
- `/opt/homebrew/lib/node_modules/...` 判定为 npm 而不是 Homebrew formula。
- 非 Node 包路径上的伪造 `BIFROST_CLI_INSTALL_SOURCE` 不改变 manual 判定。
- npm global root 能解析到存在的平台二进制。
- Windows program mapping 精确为 `npm.cmd` / `pnpm.cmd`，包含 shell 元字符的目标版本在启动包管理器前被拒绝。

### TC-CUIS-02 npm/pnpm 黑盒 upgrade

操作步骤：

```bash
BIFROST_BIN="$PWD/target/debug/bifrost" \
  bash e2e-tests/tests/test_upgrade_install_source_e2e.sh
```

预期结果：

- npm 场景输出 `Install method: npm`，调用 `npm install --global --no-audit --progress=false @bifrost-proxy/bifrost@<target>`。
- pnpm 场景输出 `Install method: pnpm`，调用 `pnpm add --global @bifrost-proxy/bifrost@<target>`。
- 两条成功路径都通过升级后新二进制版本校验，且不出现 `Replacing binary at:`；预置在包目录的 `.backup` sentinel 保持存在，证明未调用手动安装清理。
- fake npm 非零退出时 CLI 非零退出，保留 `simulated npm failure` 和重试命令，不回退直接覆盖。

### TC-CUIS-03 Bash 脚本安装来源标记

操作步骤：

```bash
bash e2e-tests/tests/test_install_binary_adaptive_download.sh
```

预期结果：

- `script install provenance marker` 通过。
- 隔离目录中的 `bifrost.install-source` 内容精确为 `script`。
- 源码 `install.sh` 包含原子 marker，`uninstall.sh` 在隔离目录同时移除 binary/marker；既有下载源、checksum 和超时回归继续通过。

### TC-CUIS-04 PowerShell 脚本安装来源标记契约

操作步骤：

```bash
python3 - <<'PY'
from pathlib import Path

source = Path("install-binary.ps1").read_text()
assert '$markerPath = "$DestPath.install-source"' in source
assert 'Set-Content -Path $markerTempPath -Value "script" -NoNewline' in source
assert 'Move-Item -Path $markerTempPath -Destination $markerPath -Force' in source

test = Path("e2e-tests/tests/test_install_binary_windows_adaptive_download.ps1").read_text()
assert 'atomic installer records script provenance' in test
assert 'PowerShell installer writes script source marker' in test
print("PASS PowerShell script provenance marker contract")
PY
```

预期结果：

- 安装器包含同目录临时 marker 写入与原子 `Move-Item -Force`。
- Windows PowerShell 动态回归包含隔离目录断言；该脚本由 Windows CI 执行，macOS 本机不以缺少 `pwsh` 假装动态通过。

### TC-CUIS-05 npm 包装器来源传递

操作步骤：

```bash
node --test npm/bifrost/lib/install-source.test.js
node --check npm/bifrost/bin/bifrost
node --check npm/bifrost/lib/install-source.js
```

预期结果：

- pnpm global/content-addressed 路径返回 `pnpm`，普通 Node global package 路径返回 `npm`。
- 包装器与来源模块语法检查通过。

### TC-CUIS-06 `update` 等价别名

操作步骤：

```bash
target/debug/bifrost --help | grep -F 'alias: update'
diff -u <(target/debug/bifrost upgrade --help) <(target/debug/bifrost update --help)
BIFROST_BIN="$PWD/target/debug/bifrost" \
  bash e2e-tests/tests/test_upgrade_install_source_e2e.sh
```

预期结果：

- 顶层帮助在 `upgrade` 条目展示 `alias: update`。
- `update --help` 与 `upgrade --help` 完全一致，证明参数集合一致。
- 黑盒 npm 场景使用 `update` 后仍显示 `Install method: npm`，调用固定版本 npm 全局升级且不回退直接覆盖；pnpm 继续用 canonical `upgrade` 作为对照。

## 清理步骤

- 所有 shell/PowerShell 用例由 trap/finally 删除 `mktemp` / system temp 目录。
- 确认没有残留 fake npm/pnpm、Bifrost 测试 daemon 或测试监听端口。
- 不删除 Cargo build cache；它属于后续验证可复用产物。

## 执行记录

| 日期 | 用例 | 实际结果 | 结论 |
| --- | --- | --- | --- |
| 2026-08-04 | TC-CUIS-01 | `cargo build` 通过；定向 Rust 测试 7/7 通过，包含独立子进程 global-root resolver；`/opt/homebrew/lib/node_modules` 冲突回归通过。第 2 轮补 Windows `.cmd` mapping 和恶意版本字符拒绝后复跑仍 7/7 通过 | PASS |
| 2026-08-04 | TC-CUIS-02 | 首次 npm、pnpm、npm failure 三条黑盒场景均通过；第 1 轮 review 增加包目录 `.backup` sentinel 后重建 binary 复跑仍 3/3 通过，npm/pnpm 均未直接清理包文件 | PASS |
| 2026-08-04 | TC-CUIS-03 | 第 1 次 Bash installer suite 12 项通过；第 1 轮 review 补齐源码安装与卸载 marker 生命周期后复跑 13 项全部通过 | PASS |
| 2026-08-04 | TC-CUIS-04 | 初次尝试 `pwsh` 因本机未安装而停止，归因为环境依赖；随后按更新后的本机契约用例执行，marker 临时写入、原子 Move 和 Windows 动态测试断言均存在。真正 PowerShell 执行保留给 Windows CI | PASS（本机契约） |
| 2026-08-04 | TC-CUIS-05 | 初次 Node test 2/2 通过；第 1 轮 review 增加 child env 覆盖后复跑 3/3 通过；`bin/bifrost` 与 `lib/install-source.js` 的 `node --check` 均退出 0 | PASS |
| 2026-08-04 | TC-CUIS-06 | 首次执行发现测试误写为 Clap 多别名格式 `aliases: update`，实际单别名格式为 `alias: update`；修正断言后，顶层帮助可发现别名、两种拼写的 help 完全一致，npm 通过 `update` 和 pnpm 通过 `upgrade` 的黑盒来源升级均通过 | PASS |

清理确认：黑盒脚本 trap 已删除 fake global roots/package managers/data dirs；未启动测试 daemon，未触碰正式 Bifrost listener、全局 npm/pnpm 包或 `/Applications/Bifrost.app`。

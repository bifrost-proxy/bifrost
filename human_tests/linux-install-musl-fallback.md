# Linux 旧 glibc 安装 musl 回退

## 功能模块说明

验证 Linux 旧 glibc 环境（例如 Debian 10 / glibc 2.28）安装 Bifrost 时，不再选择要求 `GLIBC_2.39` 的 GNU 预编译包，而是自动回退到 musl/static 包。该用例覆盖 shell 安装脚本与 npm/npx 安装运行时的目标选择逻辑。

## 前置条件

- 当前目录为 Bifrost 仓库根目录。
- 不需要 sudo、apt-get、patchelf 或真实 Debian 10 容器；用例通过离线模拟 `ldd --version` 输出验证目标选择结果。
- 不访问 9900 端口，不启动 Bifrost 服务，不修改系统代理。

## 测试用例列表

### TC-LIMF-01 Debian 10 glibc 2.28 自动选择 shell musl target

操作步骤：

1. 执行：
   ```bash
   BIFROST_INSTALL_BINARY_SKIP_MAIN=1 source ./install-binary.sh
   get_glibc_version() { echo "2.28"; }
   get_linux_target x86_64 gnu
   ```
2. 观察输出 target。

预期结果：

- 输出为 `x86_64-unknown-linux-musl`。
- 不需要 sudo、apt-get、patchelf。

### TC-LIMF-02 新 glibc 环境保持 GNU target

操作步骤：

1. 执行：
   ```bash
   BIFROST_INSTALL_BINARY_SKIP_MAIN=1 source ./install-binary.sh
   get_glibc_version() { echo "2.39"; }
   get_linux_target x86_64 gnu
   ```
2. 观察输出 target。

预期结果：

- 输出为 `x86_64-unknown-linux-gnu`。
- 新 glibc 环境不被误切到 musl。

### TC-LIMF-03 npm/npx 在 Debian 10 glibc 2.28 选择 musl 平台包

操作步骤：

1. 执行：
   ```bash
   node - <<'NODE'
   const platform = require("./npm/bifrost/lib/platform.js");
   console.log(platform.getPlatformKey({
     platform: "linux",
     arch: "x64",
     lddOutput: "ldd (Debian GLIBC 2.28-10) 2.28",
     existsSync: () => false,
   }));
   console.log(platform.getPackageName({
     platform: "linux",
     arch: "x64",
     lddOutput: "ldd (Debian GLIBC 2.28-10) 2.28",
     existsSync: () => false,
   }));
   NODE
   ```
2. 观察输出平台 key 与 npm package。

预期结果：

- 第一行输出 `linux-x64-musl`。
- 第二行输出 `@bifrost-proxy/bifrost-linux-x64-musl`。

### TC-LIMF-04 bifrost upgrade 在旧 glibc release 下载路径选择 musl

操作步骤：

1. 执行：
   ```bash
   cargo test -p bifrost-cli glibc --all-features
   ```
2. 观察 `test_glibc_2_38_requires_musl_for_upgrade`、`test_glibc_2_39_keeps_gnu_for_upgrade`、`test_unknown_glibc_requires_musl_for_upgrade` 结果。

预期结果：

- glibc 2.38 判定需要 musl fallback。
- glibc 2.39 判定不需要 musl fallback。
- 无法识别 glibc 版本时判定需要 musl fallback。

## 清理步骤

- 本用例不产生持久化测试数据。
- 如手动创建临时 shell 函数，退出当前 shell 即可清理。

## 执行记录

| 日期 | 用例 | 命令 | 实际结果 |
|------|------|------|----------|
| 2026-04-28 | TC-LIMF-01 | `bash e2e-tests/tests/test_install_musl_fallback.sh` | PASS：glibc 2.28 输出 `x86_64-unknown-linux-musl`，aarch64 同步输出 `aarch64-unknown-linux-musl` |
| 2026-04-28 | TC-LIMF-02 | `bash e2e-tests/tests/test_install_musl_fallback.sh` | PASS：glibc 2.39 输出 `x86_64-unknown-linux-gnu` |
| 2026-04-28 | TC-LIMF-03 | `bash e2e-tests/tests/test_install_musl_fallback.sh` | PASS：npm 平台 key 为 `linux-x64-musl`，package 为 `@bifrost-proxy/bifrost-linux-x64-musl` |
| 2026-04-28 | TC-LIMF-04 | `cargo test -p bifrost-cli glibc --all-features` | PASS：lib 与 bin 测试目标中 2.38/2.39/unknown 三个 upgrade fallback 判断均通过 |

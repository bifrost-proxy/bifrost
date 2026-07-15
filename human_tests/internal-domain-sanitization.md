# 内部域名脱敏与回归门禁

## 功能模块说明

验证公开仓库不再包含受限企业内网域名，同时确认 URL/规则测试使用 IANA
保留的 `example.com` 子域，并且同步 Provider 分类和 CLI 帮助仍保持可用。

## 前置条件

1. 在仓库根目录执行。
2. 当前 checkout 使用仓库 `mise.toml` 锁定的 Rust 与 pnpm 版本。
3. 不启动 Bifrost 服务，不访问正式 `9900` 端口，不修改系统代理或用户数据。

## 测试用例

### TC-IDS-01：tracked 与工作区文本零受限域名命中

操作步骤：

```bash
restricted_suffix="bytedance"
restricted_suffix="${restricted_suffix}.net"
! rg -n -i -F "$restricted_suffix" --hidden -g '!.git/**' .
```

预期结果：命令退出码为 0 且无输出，证明 tracked 文件、新增未跟踪文件和
隐藏配置都没有完整受限域名。

### TC-IDS-02：E2E 防回归脚本可独立执行

操作步骤：

```bash
bash e2e-tests/tests/test_internal_domain_leaks.sh
```

预期结果：脚本退出码为 0，输出两条 `OK`，分别确认零受限域名命中和四类
中性 Rust fixture 仍存在。

### TC-IDS-03：受管 Provider 只匹配受控默认 URL

操作步骤：

```bash
cargo test -p bifrost-sync provider_and_github_gist_helpers_cover_routing_edges -- --nocapture
```

预期结果：测试通过；`bifrost.example.com` 被识别为既有受管 Provider，
`api.example.com` 和自定义同步地址仍被识别为 Bifrost Cloud，证明没有把
整个 `example.com` 后缀误判成受管服务。

### TC-IDS-04：真实 CLI help 只展示脱敏后的默认登录地址

操作步骤：

```bash
cargo run --quiet --bin bifrost -- login --help > /tmp/bifrost-domain-sanitization-help.txt
rg -F "https://bifrost.example.com/v4/sso/token-login" /tmp/bifrost-domain-sanitization-help.txt
```

预期结果：两个命令退出码均为 0，CLI help 包含脱敏后的 token 登录地址，
且不需要启动代理服务或连接远端 relay。

## 清理步骤

```bash
rm -f /tmp/bifrost-domain-sanitization-help.txt
```

本用例不启动进程、不创建 Bifrost 数据目录、不修改系统代理，因此没有额外
服务或用户数据需要清理。

## 实际执行记录

| 日期 | 用例 | 结果 | 证据 |
| --- | --- | --- | --- |
| 2026-07-15 | TC-IDS-01 | 通过 | 动态拼接受限后缀后执行全工作区 `rg`，无输出且否定断言退出码为 0。 |
| 2026-07-15 | TC-IDS-02 | 通过 | E2E 脚本输出 `OK: no restricted internal domains...` 与 `OK: neutral example.com fixtures...`，退出码为 0。 |
| 2026-07-15 | TC-IDS-03 | 通过 | `bifrost-sync` 专项测试 1 passed、0 failed；覆盖默认受管 URL 与两个非受管 URL。 |
| 2026-07-15 | TC-IDS-04 | 通过 | 首次编译后真实 CLI help 输出两处 `https://bifrost.example.com/v4/sso/token-login`，命令退出码为 0，未启动代理。 |

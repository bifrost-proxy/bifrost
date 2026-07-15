# 内部域名脱敏与回归门禁

## 功能模块说明

验证公开仓库只保留内网登录必需的唯一企业域名，其他 URL/规则测试使用 IANA
保留的 `example.com` 子域，并且同步 Provider 分类和 CLI 登录仍保持可用。

## 前置条件

1. 在仓库根目录执行。
2. 当前 checkout 使用仓库 `mise.toml` 锁定的 Rust 与 pnpm 版本。
3. 不启动 Bifrost 服务，不访问正式 `9900` 端口，不修改系统代理或用户数据。

## 测试用例

### TC-IDS-01：tracked 文本只命中唯一允许的内部域名

操作步骤：

```bash
internal_suffix="bytedance"
internal_suffix="${internal_suffix}.net"
allowed_host="bifrost.${internal_suffix}"
escaped_suffix="${internal_suffix//./\\.}"
actual_hosts="$(git grep -I -h -o -i -E "([[:alnum:]_-]+\\.)*${escaped_suffix}" -- . \
  | tr '[:upper:]' '[:lower:]' | sort -u)"
test "$actual_hosts" = "$allowed_host"
```

预期结果：命令退出码为 0，提取结果只有 `bifrost.bytedance.net`；其他服务名、
嵌套子域或大小写变体都不能绕过精确 allowlist。

### TC-IDS-02：E2E 防回归脚本可独立执行

操作步骤：

```bash
bash e2e-tests/tests/test_internal_domain_leaks.sh
```

预期结果：脚本退出码为 0，输出两条 `OK`，分别确认唯一登录域名 allowlist
和登录域名加三类中性 Rust fixture 仍存在。

### TC-IDS-03：受管 Provider 只匹配受控默认 URL

操作步骤：

```bash
cargo test -p bifrost-sync provider_and_github_gist_helpers_cover_routing_edges -- --nocapture
```

预期结果：测试通过；`bifrost.bytedance.net` 被识别为既有受管 Provider，
`api.example.com`、自定义同步地址和动态构造的其他内部子域仍被识别为
Bifrost Cloud，证明受管 Provider 只按完整默认 URL 判断。

### TC-IDS-04：真实 CLI help 保留可用的默认登录地址

操作步骤：

```bash
cargo run --quiet --bin bifrost -- login --help > /tmp/bifrost-domain-sanitization-help.txt
rg -F "https://bifrost.bytedance.net/v4/sso/token-login" /tmp/bifrost-domain-sanitization-help.txt
```

预期结果：两个命令退出码均为 0，CLI help 包含内网可用的默认 token 登录地址，
且不需要启动代理服务或连接远端 relay。该地址是 allowlist 中唯一允许的内部域名。

## 清理步骤

```bash
rm -f /tmp/bifrost-domain-sanitization-help.txt
```

本用例不启动进程、不创建 Bifrost 数据目录、不修改系统代理，因此没有额外
服务或用户数据需要清理。

## 实际执行记录

| 日期 | 用例 | 结果 | 证据 |
| --- | --- | --- | --- |
| 2026-07-16 | TC-IDS-01 | 通过 | 动态拼接后缀并提取 tracked 文本中的完整 host，去重结果仅为 `bifrost.bytedance.net`，精确比较退出码为 0。 |
| 2026-07-16 | TC-IDS-02 | 通过 | E2E 脚本输出唯一登录域名 allowlist 与中性 fixture 两条 `OK`，退出码为 0。 |
| 2026-07-16 | TC-IDS-03 | 通过 | `bifrost-sync` 专项测试 1 passed、0 failed；覆盖默认受管 URL、两个公开地址与动态构造的其他内部子域。 |
| 2026-07-16 | TC-IDS-04 | 通过 | 真实 CLI help 输出两处 `https://bifrost.bytedance.net/v4/sso/token-login`，命令退出码为 0，未启动代理。 |

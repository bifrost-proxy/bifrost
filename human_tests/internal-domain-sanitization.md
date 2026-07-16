# 内部域名脱敏与回归门禁

## 功能模块说明

验证公开仓库不保存内部域名明文；登录、默认同步和 Remote Invoke 必需的默认
host 仅以 Base64 保存并在消费时解码。其他 URL/规则测试使用 IANA 保留的
`example.com` 子域，同步 Provider 分类和 CLI 登录保持可用。

## 前置条件

1. 在仓库根目录执行。
2. 当前 checkout 使用仓库 `mise.toml` 锁定的 Rust 与 pnpm 版本。
3. 不启动 Bifrost 服务，不访问正式 `9900` 端口，不修改系统代理或用户数据。

## 测试用例

### TC-IDS-01：tracked 文本不包含内部域名明文

操作步骤：

```bash
internal_suffix="bytedance"
internal_suffix="${internal_suffix}.net"
! git grep -I -n -i -F "$internal_suffix" -- .
```

预期结果：命令退出码为 0 且没有输出；默认 host、其他服务名、嵌套子域或
大小写变体都不能以明文进入 tracked 文件。

### TC-IDS-02：E2E 防回归脚本可独立执行

操作步骤：

```bash
bash e2e-tests/tests/test_internal_domain_leaks.sh
```

预期结果：脚本退出码为 0，输出三条 `OK`，分别确认明文零命中、Base64 默认
host 可解码为 HTTPS URL，以及三类中性 Rust fixture 仍存在。

### TC-IDS-03：受管 Provider 只匹配受控默认 URL

操作步骤：

```bash
cargo test -p bifrost-sync provider_and_github_gist_helpers_cover_routing_edges -- --nocapture
```

预期结果：测试通过；运行时解码的默认 URL 被识别为既有受管 Provider，
`api.example.com`、自定义同步地址和动态构造的其他内部子域仍被识别为 Bifrost
Cloud，证明受管 Provider 只按完整默认 URL 判断。

### TC-IDS-04：真实 CLI help 保留可用的默认登录地址

操作步骤：

```bash
source e2e-tests/test_utils/default_remote.sh
BIFROST_DEFAULT_REMOTE_URL="$(bifrost_default_remote_base_url)"
cargo run --quiet --bin bifrost -- login --help > /tmp/bifrost-domain-sanitization-help.txt
rg -F "${BIFROST_DEFAULT_REMOTE_URL}/v4/sso/token-login" /tmp/bifrost-domain-sanitization-help.txt
```

预期结果：两个命令退出码均为 0，CLI help 包含内网可用的默认 token 登录地址，
且不需要启动代理服务或连接远端 relay；地址只在 helper 和 CLI 的实际消费点解码。

## 清理步骤

```bash
rm -f /tmp/bifrost-domain-sanitization-help.txt
```

本用例不启动进程、不创建 Bifrost 数据目录、不修改系统代理，因此没有额外
服务或用户数据需要清理。

## 实际执行记录

| 日期 | 用例 | 结果 | 证据 |
| --- | --- | --- | --- |
| 2026-07-16 | TC-IDS-01 | 通过 | 动态拼接受限后缀后，`git grep -I` 对 tracked 文件零命中，退出码符合预期。 |
| 2026-07-16 | TC-IDS-02 | 通过 | E2E 门禁输出明文零命中、Base64 可解码为 HTTPS URL、中性 fixture 完整三条 `OK`。 |
| 2026-07-16 | TC-IDS-03 | 通过 | `bifrost-sync` 专项 1 passed、0 failed；覆盖运行时默认 URL、公开地址与动态构造的其他内部子域。 |
| 2026-07-16 | TC-IDS-04 | 通过 | 从共享 helper 解码期望值后，真实 `bifrost login --help` 与 `bifrost sync login --help` 均命中默认 token URL；完整 sync-login E2E 通过，解码后的正式 `/v4/sso/check` 返回预期 HTTP 401，证明内网 Provider 可达且要求登录。 |

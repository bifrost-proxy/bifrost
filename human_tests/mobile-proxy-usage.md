# 移动端连接 Bifrost 教程

## 功能模块说明

本文档验证移动端连接教程是否完整说明：在 PC、Mac 或 Linux 上启动 Bifrost，让 iPhone/iPad/Android 设备通过同一局域网和 Wi-Fi HTTP Proxy 使用 Bifrost，并处理 Availability Check、访问控制和 HTTPS CA 信任。

## 前置条件

1. 当前工作目录为仓库根目录。
2. Node.js 22+ 和 pnpm 可用。
3. 不启动正式 9900 服务，不修改系统代理。
4. 站点依赖已安装；如果未安装，先执行：

   ```bash
   pnpm --dir site install
   ```

5. 如果执行真实手机链路用例，需要一台与电脑位于同一可互通局域网的 iPhone/iPad 或 Android 设备。

## 测试用例列表

### TC-MPU-01：中文和英文源文档提供教程入口

**操作步骤**：

1. 执行：

   ```bash
   rg -n "mobile-proxy|移动端连接|Connect a mobile device" docs/README.md docs/getting-started.md docs/mobile-proxy.md docs-en/README.md docs-en/getting-started.md docs-en/mobile-proxy.md
   ```
2. 打开 `docs/mobile-proxy.md` 和 `docs-en/mobile-proxy.md`。
3. 检查两份文档都包含启动、网络可达性、Availability Check、HTTP 代理、iOS、Android 和故障排查章节。

**预期结果**：

- 中文和英文文档索引均能发现移动端教程。
- 安装页均能跳转到移动端教程。
- 中英文教程结构覆盖同一条用户链路。

执行提示：中文页面的故障排查章节标题为 `常见问题`，英文页面对应标题为 `Troubleshooting`。

### TC-MPU-02：启动命令和地址边界与 CLI 语义一致

**操作步骤**：

1. 执行：

   ```bash
   BIFROST_DATA_DIR="$(mktemp -d)" cargo run -q -p bifrost-cli -- start --help
   ```
2. 执行：

   ```bash
   rg -n -- "--access-mode|--socks5-port|-H|--host|-p|--port" docs/mobile-proxy.md docs-en/mobile-proxy.md
   ```
3. 检查教程中的 HTTP、SOCKS5、`127.0.0.1` 和 `0.0.0.0` 示例。

**预期结果**：

- CLI help 包含教程使用的端口、监听地址、访问控制和 SOCKS5 参数。
- 文档明确 `127.0.0.1` 只能用于电脑本机管理端，手机代理必须使用电脑局域网 IP。
- 文档把 `interactive` 描述为推荐局域网访问模式，没有把 `allow_all` 描述为默认安全配置。

### TC-MPU-03：教程覆盖 Availability Check 和移动端配置步骤

**操作步骤**：

1. 执行：

   ```bash
   rg -n "Availability Check|Connected devices|Proxy configured|配置代理|Configure Proxy|Settings -> Wi-Fi|设置 -> Wi-Fi" docs/mobile-proxy.md docs-en/mobile-proxy.md
   ```
2. 检查 iOS 章节是否包含 `Manual` / `手动`，并检查教程是否明确默认端口 `9900`。
3. 检查 Android 章节是否包含手动代理、厂商差异和绕过列表说明。

**预期结果**：

- 教程要求先从 Availability Check 获取可达的 `<host>:<port>`，再配置手机 Wi-Fi HTTP Proxy。
- iOS 和 Android 均有可直接执行的设置步骤。
- 文档没有要求把管理端完整 URL 或 `http://` 前缀填入手机代理字段。

### TC-MPU-04：证书信任和 App 边界说明正确

**操作步骤**：

1. 执行：

   ```bash
   rg -n "Certificate Trust Settings|证书信任设置|Downloaded Profile|已下载描述文件|user-installed|用户安装|certificate pinning|证书固定|passthrough" docs/mobile-proxy.md docs-en/mobile-proxy.md
   ```
2. 检查 iOS 章节是否把 profile 安装和完全信任分成两个步骤。
3. 检查 Android 章节是否说明用户 CA 与 App 默认信任策略可能不同。

**预期结果**：

- iOS 文档明确安装 profile 后还要开启 Bifrost CA 完全信任。
- Android 文档不承诺所有 App 都信任用户 CA。
- 文档对证书固定、自定义 TLS 和 passthrough 给出边界说明，不建议强行全局拦截。

### TC-MPU-05：站点同步生成中文和英文稳定页面

**操作步骤**：

1. 执行：

   ```bash
   pnpm --dir site run docs:sync
   pnpm --dir site run docs:verify
   ```
2. 检查以下生成文件：

   ```bash
   test -f site/src/content/docs/getting-started/mobile-proxy.md
   test -f site/src/content/docs/en/getting-started/mobile-proxy.md
   ```
3. 检查生成文件的来源标记和标题：

   ```bash
   rg -n "docs/mobile-proxy.md|docs-en/mobile-proxy.md|移动端连接 Bifrost|Connect a Mobile Device" \
     site/src/content/docs/getting-started/mobile-proxy.md \
     site/src/content/docs/en/getting-started/mobile-proxy.md
   ```

**预期结果**：

- `docs:sync` 和 `docs:verify` 成功。
- 中文页面生成到 `getting-started/mobile-proxy.md`，英文页面生成到 `en/getting-started/mobile-proxy.md`。
- 生成页面包含正确的源文档标记，不需要直接编辑 `site/src/content/docs`。

### TC-MPU-06：真实 Availability Check 移动设备链路

**操作步骤**：

1. 在 PC、Mac 或 Linux 上启动 Bifrost：

   ```bash
   bifrost start -d
   ```
2. 在电脑浏览器打开：

   ```text
   http://127.0.0.1:9900/_bifrost/settings?tab=certificate
   ```

3. 在 `Availability Check` 中选择电脑局域网 IP，生成链接或二维码。
4. 在同一局域网的移动设备上打开链接。
5. 如果设备处于 pending，在电脑管理端批准它。
6. 按页面显示的 `<电脑局域网 IP>:<端口>` 设置手机 Wi-Fi HTTP Proxy。
7. 观察手机页面和管理端 `Connected devices`，确认代理配置状态更新。
8. 如需 HTTPS 检查，按页面说明安装 CA；iOS 继续在 `Settings -> General -> About -> Certificate Trust Settings` 开启完全信任。
9. 测试结束后，把手机 HTTP Proxy 改回 `Off`，并撤销不再需要的临时批准。

**预期结果**：

- 手机能打开 Availability Check，不使用 `127.0.0.1` 作为代理地址。
- pending 设备经管理端批准后可以继续检查。
- 配置 HTTP Proxy 后，手机页面和管理端都显示已配置或等价成功状态。
- iOS 只有安装 profile 并开启完全信任后，浏览器 HTTPS 检查才通过。
- 关闭手机代理后，设备状态会回落为未配置，且清理步骤不会留下不必要的访问授权。

## 清理步骤

1. 将移动设备 Wi-Fi 的 HTTP Proxy 设置回 `Off` 或关闭手动代理。
2. 在 Bifrost `Settings -> Access Control` 撤销本次测试产生的临时批准或白名单项。
3. 如果启动了测试服务，执行：

   ```bash
   bifrost stop
   ```

4. 删除测试过程中创建的临时 `BIFROST_DATA_DIR`。

## 本次执行记录

| 日期 | 用例 | 执行方式 | 结果 |
| --- | --- | --- | --- |
| 2026-07-27 | TC-MPU-01 | `rg` 检查中英文入口和章节；检查 `docs/mobile-proxy.md` 与 `docs-en/mobile-proxy.md` | 通过：中英文 README、安装页和教程入口均存在，章节结构对应；中文使用“常见问题”，英文使用 “Troubleshooting” |
| 2026-07-27 | TC-MPU-02 | `BIFROST_DATA_DIR="$(mktemp -d)" cargo run -q -p bifrost-cli -- start --help`；`rg` 检查参数和地址示例 | 通过：CLI help 提供 `--access-mode`、`--socks5-port`、`-H/--host`、`-p/--port`；教程区分 `127.0.0.1` 与局域网监听地址，并将 `interactive` 作为推荐模式 |
| 2026-07-27 | TC-MPU-03 | `rg` 检查 Availability Check、Connected devices、代理设置和端口说明 | 通过：教程要求先使用 Availability Check，再配置 iOS/Android 手动 HTTP Proxy；未要求填入管理端 URL 或 `http://` 前缀 |
| 2026-07-27 | TC-MPU-04 | `rg` 检查 iOS/Android CA、证书固定和 passthrough 边界 | 通过：iOS profile 安装与完全信任分步说明；Android 用户 CA、证书固定和 App 默认信任差异均已说明 |
| 2026-07-27 | TC-MPU-05 | `pnpm --dir site run docs:sync`、`pnpm --dir site run docs:verify`、根站 `site:build`、`SITE_URL=https://bifrost-proxy.github.io/ BASE_PATH=/ pnpm --dir site run site:verify-links` | 通过：同步 61 个页面，中文和英文稳定页面生成，生产构建、首页校验和根路径站内链接校验均通过；构建后生成产物已清理，不直接提交 |
| 2026-07-27 | TC-MPU-06 | 检查真实设备前置条件：`command -v adb`、`command -v cfgutil`，并检查当前电脑局域网接口 | 环境阻塞：当前环境没有 `adb`、`cfgutil`，也没有可操作的真实 iOS/Android 设备，因此未假设手机链路通过；需在同一局域网的真实移动设备上按步骤复测 |

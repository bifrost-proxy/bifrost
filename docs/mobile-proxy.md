# 移动端连接 PC、Mac 或 Linux 上的 Bifrost

本教程说明如何让 iPhone、iPad、Android 手机或平板，通过 Wi-Fi 连接到运行在 PC、Mac 或 Linux 上的 Bifrost 代理。

最短流程是：

1. 在电脑上启动 Bifrost，并确认代理监听在局域网可达地址。
2. 让电脑和移动设备连接到同一个 Wi-Fi 或其它可互通的局域网。
3. 在 Bifrost 管理端的 `Settings -> Certificate -> Availability Check` 生成检查链接或二维码。
4. 用移动设备打开检查页，按页面结果处理访问授权和 CA 信任。
5. 在移动设备的 Wi-Fi 设置中，把 HTTP 代理设置为检查页显示的 `<电脑局域网 IP>:<Bifrost 代理端口>`。

## 先理解两个地址

Bifrost 的主监听端口同时承载代理流量和本机管理端。例如默认端口为 `9900` 时：

| 用途 | 示例 | 在哪里使用 |
| --- | --- | --- |
| 管理端 | `http://127.0.0.1:9900/_bifrost/` | 电脑上的浏览器 |
| 移动端代理 | `192.168.1.20:9900` | 手机或平板的 Wi-Fi HTTP Proxy |
| SOCKS5 代理（可选） | `192.168.1.20:1080` | 支持 SOCKS5 的客户端 |

`127.0.0.1` 只代表电脑自己，不能填到手机的代理设置中。移动设备必须使用电脑在当前局域网中的实际 IPv4 地址，例如 `192.168.1.20` 或 `10.0.0.8`。

如果电脑上有多个网卡、VPN 或虚拟网卡，不要凭猜测选择地址。使用 Availability Check 选择一个局域网 IP 后生成的链接，检查页会用当前设备真实可达的地址展示代理配置。

## 电脑端启动 Bifrost

### PC、Mac 或 Linux 使用 CLI

安装并检查 CLI：

```bash
bifrost --version
```

推荐以后台服务启动，默认监听 `0.0.0.0:9900`：

```bash
bifrost start -d
```

如果希望显式指定端口和访问控制，可以使用：

```bash
bifrost -H 0.0.0.0 -p 9900 start -d --access-mode interactive
```

`interactive` 是推荐的局域网访问模式：本机直接允许，移动设备第一次访问时可以在管理端批准。也可以在启动后通过 `Settings -> Access Control` 管理访问模式、白名单或待批准设备。

不要把电脑绑定到 `127.0.0.1`，例如下面的启动方式只能供电脑本机使用，手机无法连接：

```bash
bifrost -H 127.0.0.1 -p 9900 start -d
```

如果需要单独的 SOCKS5 入口，可以额外启动一个 SOCKS5 端口：

```bash
bifrost -p 9900 --socks5-port 1080 start -d
```

移动设备的系统 Wi-Fi 设置通常只提供 HTTP 代理入口；优先使用主 HTTP 代理端口 `9900`。只有当具体 App 明确支持 SOCKS5 时，才填写 `1080`。

### macOS 或 Windows 使用桌面端

启动 Bifrost 桌面端后，桌面端会在应用内启动代理后端。打开桌面端管理界面，进入 `Settings -> Certificate -> Availability Check`，以页面显示的代理地址为准。

桌面端和 CLI 默认共享 `~/.bifrost` 下的配置、证书和运行时状态。如果 CLI 服务已经占用了目标端口，不要再启动第二个服务；直接使用已经运行的服务，并在管理端检查当前端口和访问控制。

### Linux

Linux 当前使用 CLI 启动代理：

```bash
bifrost start -d
```

Linux 没有桌面 App 时，仍然可以在同一局域网内用电脑浏览器打开管理端，再用手机打开 Availability Check 链接。只要 Bifrost 监听地址、操作系统防火墙和局域网路由允许访问，移动端连接方式与 macOS、Windows 相同。

## 让手机和电脑互通

在继续配置前确认：

- 手机和电脑连接到同一个 Wi-Fi，或连接到彼此可达的局域网。
- 手机没有使用访客 Wi-Fi、VPN 或移动网络把流量隔离到另一个网络。
- Wi-Fi 路由器没有开启 AP isolation、客户端隔离或类似的“无线设备互相隔离”功能。
- 电脑防火墙允许 Bifrost 代理端口的局域网入站连接。
- Bifrost 没有绑定到 `127.0.0.1`。

如果电脑可以访问互联网但手机打不开电脑的检查链接，优先检查局域网隔离和防火墙，而不是先安装 CA。

## 用 Availability Check 获取正确配置

1. 在电脑浏览器打开：

   ```text
   http://127.0.0.1:9900/_bifrost/settings?tab=certificate
   ```

   如果使用了其它端口，将 `9900` 替换为实际端口。
2. 在 Certificate 页面顶部找到 `Availability Check`。
3. 选择电脑当前可供手机访问的局域网 IP，生成检查链接或二维码。
4. 用手机相机或浏览器打开链接。
5. 等待检查页展示 `Connected`、网络、浏览器 HTTPS、访问授权和代理配置等状态。
6. 如果显示待批准，回到电脑管理端批准该设备；也可以在 `Settings -> Access Control` 调整白名单或访问模式。
7. 使用检查页展示的代理地址配置手机 Wi-Fi HTTP Proxy。
8. 返回检查页等待 `Proxy configured` 或等价的已配置状态。管理端的 `Connected devices` 会实时显示该设备的状态，不需要手动刷新。

Availability Check 是推荐入口，因为它会同时检查：

- 手机是否能访问 Bifrost 的公开检查入口和探针端口。
- 当前设备是否被 Bifrost 访问控制允许。
- 当前手机浏览器是否信任 Bifrost CA。
- 手机当前 Wi-Fi HTTP Proxy 是否确实指向 Bifrost。

检查页展示的 `<host>:<port>` 是移动端配置值；管理端使用的 `http://127.0.0.1:<port>/_bifrost/` 是电脑本机地址，两者不要混用。

## 在手机上配置 HTTP 代理

### iPhone 或 iPad

1. 打开 `设置 -> Wi-Fi`。
2. 点按当前连接的 Wi-Fi 网络右侧的信息按钮。
3. 找到 `配置代理`，选择 `手动`。
4. 在服务器中填写检查页显示的电脑局域网 IP。
5. 在端口中填写 Bifrost 代理端口，默认是 `9900`。
6. 保存后回到 Availability Check 页面，等待代理配置状态更新。

关闭代理时，把 `配置代理` 改回 `关闭`。页面和管理端应在下一轮检查后回落为未配置状态。

### Android 手机或平板

不同厂商的系统设置名称略有差异，通常步骤如下：

1. 打开 `设置 -> 网络和互联网` 或 `设置 -> WLAN/Wi-Fi`。
2. 点按当前 Wi-Fi 网络的编辑按钮。
3. 打开高级设置，找到代理或 Proxy。
4. 选择手动代理。
5. 填写检查页显示的电脑局域网 IP 和 Bifrost 代理端口，默认端口为 `9900`。
6. 保存后回到 Availability Check 页面，确认 `Proxy configured`。

如果 Android 系统要求填写绕过列表，可把本机地址和不需要代理的内网域名加入绕过列表；不要把 Bifrost 代理地址本身加入绕过列表。

## HTTPS 抓包与 CA 信任

只访问普通 HTTP 地址时，不需要安装 CA。要查看或修改 HTTPS 请求内容，移动设备和目标 App 必须信任 Bifrost CA，并且目标请求必须允许 TLS 拦截。

### iOS

在检查页或 Certificate 页面下载 Bifrost CA profile 后：

1. 在 iPhone/iPad 上允许下载配置描述文件。
2. 打开 `设置 -> 已下载描述文件`，安装 Bifrost CA profile。
3. 按提示完成安装。
4. 打开 `设置 -> 通用 -> 关于本机 -> 证书信任设置`。
5. 打开 Bifrost CA 的完全信任。
6. 回到 Availability Check 页面，等待浏览器 HTTPS 状态变为通过。

仅安装 profile 不等于完全信任；`证书信任设置` 中的开关是 iOS 完成 HTTPS 信任的必要步骤。macOS 上连接 iPhone 时，也可以在 Certificate 页面使用 Apple Configurator 发送 profile，但手机仍可能要求解锁、Trust 或在屏幕上确认。

### Android

在 Certificate 页面检测到 Android 设备时，可以使用 Bifrost 提供的设备安装流程；也可以下载 CA 后在手机系统设置中安装。Android 版本、厂商 ROM 和设备管理策略会影响证书安装入口。

安装用户 CA 后，浏览器通常可以用于验证 HTTPS 信任，但很多 Android App 默认不信任用户安装的 CA，或者使用证书固定。遇到这种情况：

- 先用手机浏览器访问 Availability Check，区分“浏览器信任失败”和“App 不支持拦截”。
- 对证书固定或自定义 TLS 栈的 App，使用应用排除或域名排除，不要强行开启全局 TLS 拦截。
- Bifrost 的 CA 只解决客户端到 Bifrost 的信任，不会替换上游网站的真实证书，也不会绕过 App 自己的安全策略。

## 常见问题

### 手机打不开检查链接

按以下顺序排查：

1. 用检查页重新选择电脑局域网 IP，不要使用 `127.0.0.1`。
2. 确认手机和电脑在同一可互通网络。
3. 临时检查电脑防火墙是否阻止 Bifrost 端口。
4. 确认 Bifrost 监听在 `0.0.0.0` 或电脑的局域网地址，而不是 `127.0.0.1`。
5. 检查路由器是否启用了 AP isolation、访客网络或客户端隔离。

### 手机能打开页面，但代理访问被拒绝

这是访问控制状态，不是 CA 问题。回到电脑管理端的 `Settings -> Access Control`，批准 pending 设备、加入合适的 IP/CIDR 白名单，或在可信的隔离局域网中选择合适的访问模式。不要为了绕过审批把服务暴露到公网并使用 `allow_all`。

### 管理端显示代理未配置

确认手机 Wi-Fi 中填写的是：

```text
电脑局域网 IP:Bifrost HTTP 代理端口
```

不要填写 `127.0.0.1`、管理端完整 URL、`http://` 前缀或 SOCKS5 端口。保存 Wi-Fi 设置后重新打开或等待检查页的下一轮检查。

### 浏览器 HTTPS 通过，但 App 没有流量

Availability Check 只证明当前手机浏览器完成了 Bifrost 的检查链路，不保证所有 App 都使用系统 HTTP 代理或信任用户 CA。检查 App 是否支持代理、是否使用证书固定，以及 Bifrost 是否对该域名启用了 TLS 拦截；必要时为该 App 或域名配置 passthrough。

### 不再使用手机代理

把手机 Wi-Fi 的 HTTP Proxy 改回 `关闭`，然后在 Bifrost 管理端撤销不再需要的临时批准或白名单项。不要在不可信网络中长期保留 `allow_all`。

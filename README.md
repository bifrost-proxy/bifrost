# Bifrost

<p align="center">
  <strong>高性能 HTTP/HTTPS/SOCKS5 代理服务器</strong>
</p>

<p align="center">
  <a href="https://github.com/bifrost-proxy/bifrost/actions"><img src="https://github.com/bifrost-proxy/bifrost/workflows/CI/badge.svg" alt="CI Status"></a>
  <a href="https://github.com/bifrost-proxy/bifrost/releases"><img src="https://img.shields.io/github/v/release/bifrost-proxy/bifrost" alt="Release"></a>
  <a href="https://github.com/bifrost-proxy/bifrost/releases"><img src="https://img.shields.io/github/downloads/bifrost-proxy/bifrost/total" alt="Downloads"></a>
  <a href="https://github.com/bifrost-proxy/bifrost/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License"></a>
</p>

> Language: **中文** | [English](README.en.md)
>
> [帮助文档](https://github.com/bifrost-proxy/bifrost/tree/main/docs) · [English docs](docs-en/README.md) · [Documentation site](https://bifrost-proxy.github.io/bifrost/)

Bifrost 是一个用 Rust 编写的高性能，AI 友好的代理服务器，灵感来源于 [Whistle](https://github.com/avwo/whistle)。它提供请求拦截、规则修改、TLS 拦截、脚本扩展、流量查看、请求重放以及 Web UI 管理能力。

## 快速开始

安装 CLI：

方法一：使用脚本安装

macOS / Linux / Git Bash：

```bash
curl -fsSL https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/install-binary.sh | bash
```

Windows PowerShell：

```powershell
irm https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/install-binary.ps1 | iex
```

该脚本默认会继续安装并信任 CA 证书、安装所有 Bifrost AI skills，并以后台服务启动 Bifrost；安装完成后可直接访问管理端。
Bash 与 PowerShell 安装脚本都会自动探测 GitHub 直连和内置镜像源，优先使用最快可用的 release 下载地址；受限网络中也可通过 `BIFROST_GITHUB_MIRROR` 指定优先镜像。Windows PowerShell 脚本会把安装目录加入当前会话和 Windows 用户 `Path`；Git Bash 脚本在 Windows 下也会同步写入 Windows 用户 `Path`，新打开的 PowerShell/CMD 可直接执行 `bifrost`。

安装指定版本

macOS / Linux / Git Bash：

```bash
curl -fsSL https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/install-binary.sh | bash -s -- --version v0.0.96
```

Windows PowerShell：

```powershell
$installer = irm https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/install-binary.ps1
& ([scriptblock]::Create($installer)) -Version v0.0.96
```

方法二：使用 npm 安装

```bash
npm i -g @bifrost-proxy/bifrost
```

更多安装方法：[`docs/getting-started.md`](docs/getting-started.md)

检查代理：

```bash
bifrost status
```

启动后访问管理端：

```text
http://127.0.0.1:9900/_bifrost/
```

## 设备可用性检查

手机、平板或另一台电脑配置代理前，优先打开 Web UI 的 `Settings -> Certificate -> Availability Check`。选择当前电脑的局域网 IP 后生成二维码或链接，用目标设备访问即可自动检查三件事：

- 该设备是否已被 Bifrost 代理访问控制允许。
- 设备是否能访问 Bifrost 的探针端口。
- 当前浏览器是否已经信任 Bifrost CA 签发的 HTTPS 证书。

检查通过后，页面会显示可点击复制的代理地址 `<局域网 IP>:<端口>`，也可以打开代理二维码继续配置。若检查失败，页面会按 iOS/Android 给出下一步：安装 CA、iOS 进入 `设置 > 通用 > 关于本机 > 证书信任设置` 开启完全信任，或到管理端批准该设备的代理访问。

## 和AI集成
```bash
bifrost install-skill -y
```

## 特性说明

![network.png](assets/network.png) <img width="1500" height="783" alt="image" src="https://github.com/user-attachments/assets/44062a96-47f3-481b-a2b6-e1bda9b3fda9" />
![scripts.png](assets/scripts.png)
![rules.png](assets/rules.png)
![replay.png](assets/replay.png)
![metrics.png](assets/metrics.png)

- 高性能代理内核：基于 Tokio + Hyper，支持高并发与连接复用
- 多协议支持：HTTP/1.1、HTTP/2、HTTP/3、HTTPS、SOCKS5、WebSocket、SSE、gRPC
- TLS 拦截能力：支持 CA 证书生成、按域名动态签发证书、按规则选择拦截/透传，并可对单条上游 HTTPS 规则显式允许不安全证书
- 规则引擎：支持路由、请求/响应改写、注入、延迟、限速、Mock、脚本处理
- 管理界面：内置 Web UI，支持规则编辑、流量查看、脚本管理、请求重放
- Breakpoint：支持在 Web UI 中暂停命中 `breakpoint://request` / `breakpoint://response` 规则的 HTTP request/response，编辑 headers/body 后继续；详见 [`docs/breakpoint.md`](docs/breakpoint.md)
- 资源风险告警：Performance 页与 `/_bifrost/api/system/memory` 会显示 body/ws 文件 writer 占用及接近句柄上限的告警状态
- 脚本沙箱：基于 QuickJS，支持 `reqScript`、`resScript`、`decode`

## 开发初始化

克隆仓库后，先执行一次 Git hook 初始化：

```bash
bash scripts/setup-git-hooks.sh
# 或
make setup
```

这会为当前仓库写入本地 `core.hooksPath=.githooks`，即使机器上配置了全局 `hooksPath`，后续 `git commit` 也会优先执行仓库内的 `.githooks/pre-commit`。默认 pre-commit 会检查工作区格式、桌面端 Tauri 格式，以及 `cargo clippy --workspace --all-targets --all-features -- -D warnings`。

## 用不习惯 CLI？想要使用桌面端 APP？

请直接到[releases](https://github.com/bifrost-proxy/bifrost/releases)中下载对应平台的桌面端程序

如果 `bifrost version-check`、`bifrost upgrade`、`bifrost install-skill`、Sync/登录、AI/Agent provider、ASR/语音模型下载、脚本 `net.fetch` 或规则远程 URL 在企业网络、Linux 沙箱或 CI 中访问 HTTPS 时报 `UnknownIssuer`、`invalid peer certificate`、`certificate verify failed` 等 TLS 证书错误，优先把企业/沙箱根证书安装进系统 trust store。系统 CA 不可控时，GitHub/升级链路可设置 `BIFROST_GITHUB_CA_BUNDLE=/path/to/ca.pem` 或 `BIFROST_UPGRADE_CA_BUNDLE=/path/to/ca.pem`；通用外部 HTTPS 链路可设置 `BIFROST_CA_BUNDLE=/path/to/ca.pem` 或 `BIFROST_CA_DIR=/path/to/certs`。同时兼容 `SSL_CERT_FILE`、`SSL_CERT_DIR`、`REQUESTS_CA_BUNDLE`、`CURL_CA_BUNDLE`、`NODE_EXTRA_CA_CERTS`、`GIT_SSL_CAINFO`、`AWS_CA_BUNDLE`、`PIP_CERT`、`NPM_CONFIG_CAFILE`、`GRPC_DEFAULT_SSL_ROOTS_FILE_PATH` 等常见 CA 环境变量。只有在受控临时环境且无法注入 CA 时，才使用 `BIFROST_UPGRADE_UNSAFE_SSL=1`、`BIFROST_GITHUB_UNSAFE_SSL=1` 或 `BIFROST_UNSAFE_SSL=1` 跳过相应外部 HTTPS 链路的证书校验。

## 基本用法摘要

常见命令：

```bash
# 查看状态
bifrost status

# 停止服务
bifrost stop

# 管理端远程访问与鉴权（Web UI）
bifrost admin remote status
bifrost admin remote enable
bifrost admin passwd
bifrost admin revoke-all

# 查看流量
bifrost traffic list
bifrost traffic search "keyword" --method POST --host api.openai.com --path /v1/responses
bifrost search "keyword" --req-header
bifrost search "keyword" --res-body

# 添加规则
bifrost rule add local-dev --content "example.com host://127.0.0.1:3000"

# 编辑全局默认规则，主端口和所有临时端口都会自动生效
bifrost rule update Default --content "internal.example.test dns://10.0.0.53"
```

搜索命令补充说明：

- `bifrost search` 与 `bifrost traffic search` 等价
- 基础过滤支持 `--method`、`--host`、`--path`、`--status`、`--protocol`
- 搜索范围支持 `--url`、`--req-header`、`--res-header`、`--req-body`、`--res-body`
- 兼容别名：`--headers` 会同时搜索请求头和响应头，`--body` 会同时搜索请求体和响应体

规则示例：

```txt
example.com host://127.0.0.1:3000
api.example.com reqHeaders://x-debug=1
chatgpt.com http3://
internal-api.example.test https://10.37.102.138:8080 upstreamUnsafeSsl://true
```

`Default` 是 Bifrost 自动创建的全局默认规则，始终启用且列表置顶。它不能删除、停用、重命名或同步到远端，但内容可以编辑；适合放统一 DNS、通用 header、TLS 兜底等所有端口都需要共享的配置。详见 [`docs/rule.md`](docs/rule.md#10-全局默认规则-default)。

## 文档索引

- 文档总览：[`docs/README.md`](docs/README.md)
- 项目概览：[`docs/overview.md`](docs/overview.md)
- 安装与启动：[`docs/getting-started.md`](docs/getting-started.md)
- CLI 详细命令：[`docs/cli.md`](docs/cli.md)
- 桌面版安装与构建：[`docs/desktop.md`](docs/desktop.md)
- 规则语法：[`docs/rule.md`](docs/rule.md)
- 操作符说明：[`docs/operation.md`](docs/operation.md)
- 匹配模式：[`docs/pattern.md`](docs/pattern.md)
- 规则协议手册：[`docs/rules/README.md`](docs/rules/README.md)
- Scripts 模块与脚本开发：[`docs/scripts.md`](docs/scripts.md)
- Values 使用说明：[`docs/values.md`](docs/values.md)
- 请求重放说明：[`docs/replay.md`](docs/replay.md)
- Breakpoint 使用手册：[`docs/breakpoint.md`](docs/breakpoint.md)
- 项目结构与模块说明：[`docs/architecture.md`](docs/architecture.md)
- Agent Skill 安装说明：[`docs/agent-skill.md`](docs/agent-skill.md)

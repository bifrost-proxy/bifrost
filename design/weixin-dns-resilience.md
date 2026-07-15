# 微信 Bot DNS 容错方案

## 背景与目标

正式微信 Bot 轮询 `https://ilinkai.weixin.qq.com` 时持续报 `error sending request`。现场交叉验证显示：同一 DNS 服务器用直接 DNS 查询可以返回多个 A 记录，指定任一地址也能完成 TLS 并收到 HTTP 响应，但 macOS `getaddrinfo` 路径会超时。由此会出现日报已经生成、微信投递却无法开始的情况。

目标是只为微信 provider 使用异步 Hickory DNS，绕过异常的系统 `getaddrinfo` 线程池，同时保留 Bifrost 其余受信任 HTTP client 的现有解析行为。

## 用户目标验证清单

### 必须实现

- 微信登录、轮询、文字发送和图片/CDN 请求共用的两个 HTTP client 都启用 Hickory DNS。
- 受信任 outbound client 默认仍使用原有系统解析器；只有微信构造器显式启用 Hickory。
- 不硬编码微信 IP，不修改 `/etc/hosts`，允许 DNS 轮转继续生效。
- 不重新扫码、不发送测试消息；正式验证只观察已有连接恢复及真实流水线投递。

### 必须不破坏

- TLS 证书、额外 CA、unsafe SSL 和 no-proxy 语义保持不变。
- 微信登录的 75 秒专用超时保持不变。
- 非微信 provider 不因依赖 feature 合并而被动切换解析器。

## 实现逻辑

1. `bifrost-admin` 直接依赖现有版本的 `hickory-resolver`，实现 reqwest `Resolve` adapter。
2. resolver 延迟初始化，读取系统 DNS 配置并使用 IPv4/IPv6 双栈策略；查询结果转换为 reqwest 可消费的地址迭代器。
3. `WeixinProvider` 构造普通和登录 client 时注入专用 resolver；`bifrost-core` 共享 builder 与其他 provider 不变。
4. 不缓存或固化应用内 IP 列表，地址轮转继续由 DNS 响应决定。

## 测试方案

- 编译和单元测试确认专用 resolver 与微信 provider 可构建，core async/blocking builder 不需要改动。
- 复跑现有 `test_weixin_provider_e2e.sh`，确认 provider 配置、固定 base URL 和未登录错误语义不回归。
- human test 在存在系统解析器异常的现场环境中运行当前二进制，请求微信登录起始接口只做 DNS/TLS 探针，不完成登录、不发送消息；随后正式服务观察 poll 恢复。
- 最终以日报或研究的真实待投递消息验证发送成功，不额外制造测试文章或 ChatGPT Pro 研究。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核依赖与 client 构造影响，确保只有微信注入专用 resolver。
- 执行微信单元测试、现有 provider E2E 和现场 DNS/TLS 探针。
- 检查正式服务没有新增测试消息或登录状态变更。

### 第 2 轮

- 复核最新 diff、Cargo.lock 和两类 client builder。
- 复跑 E2E、human test、fmt、clippy、workspace all-features 与项目校验。
- 正式服务安装新二进制后观察 poll 和真实流水线投递，确认失败重试不会重复发送。

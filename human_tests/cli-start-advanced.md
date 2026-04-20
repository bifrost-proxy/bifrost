# CLI Start 高级参数测试用例

## 功能模块说明

本文档覆盖 `bifrost start` 命令的高级启动参数，包括：
- 顶层 `bifrost --help` 中 `start` 参考区块与实际参数同步
- TLS 拦截域名排除/白名单（`--intercept-exclude`、`--intercept-include`）
- TLS 拦截应用排除/白名单（`--app-intercept-exclude`、`--app-intercept-include`）
- 系统代理配置（`--system-proxy`、`--no-system-proxy`、`--proxy-bypass`）
- CLI 代理环境变量（`--cli-proxy`、`--cli-proxy-no-proxy`）
- 访问控制模式与白名单（`--access-mode`、`--whitelist`）
- HTML Badge 注入（`--enable-badge-injection`、`--disable-badge-injection`）
- TLS 配置变更断连控制（`--no-disconnect-on-config-change`）
- 证书检查跳过（`--skip-cert-check`）
- 日志配置（`--log-level`、`--log-output`、`--log-dir`、`--log-retention-days`）

> 基础启动参数（`-p`、`-d`、`--intercept`、`--no-intercept`、`--unsafe-ssl`、`--rules`、`--rules-file`、`--socks5-port`、`--allow-lan`、`--proxy-user`、`-y`）已在 [cli-start-stop-status.md](./cli-start-stop-status.md) 中覆盖。

## 前置条件

1. 确保项目已编译或可编译：
   ```bash
   cd /path/to/bifrost
   ```
2. 确保端口 8800、8801 未被占用
3. 确保无正在运行的 Bifrost 测试实例（可先执行 `BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- stop`）
4. 所有启动命令统一使用临时数据目录，避免污染正式环境：
   ```bash
   export BIFROST_DATA_DIR=./.bifrost-test
   ```

---

## 测试用例

### TC-CSA-01：--intercept-exclude 排除指定域名的 TLS 拦截

**操作步骤**：
1. 执行以下命令启动服务（启用拦截，但排除 `*.apple.com` 和 `example.com`）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --intercept --intercept-exclude "*.apple.com,example.com"
   ```
2. 通过代理请求被排除的域名：
   ```bash
   curl -x http://127.0.0.1:8800 https://example.com -k -v 2>&1 | grep "SSL connection"
   ```
3. 通过代理请求未被排除的域名：
   ```bash
   curl -x http://127.0.0.1:8800 https://httpbin.org/get -k
   ```

**预期结果**：
- 服务正常启动，日志中显示 TLS 拦截已启用，并列出排除域名列表
- 步骤 2 请求 `example.com` 时，代理以 CONNECT 隧道方式通过，不进行 TLS 拦截（证书为目标服务器原始证书，非 Bifrost CA 签发）
- 步骤 3 请求 `httpbin.org` 时，代理进行 TLS 拦截（证书由 Bifrost CA 签发）

---

### TC-CSA-02：--intercept-include 强制拦截指定域名（即使全局拦截关闭）

**操作步骤**：
1. 执行以下命令启动服务（全局拦截关闭，但强制拦截 `*.api.local`）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-intercept --intercept-include "*.api.local"
   ```
2. 添加 hosts 映射或直接通过代理请求一个 `api.local` 域名：
   ```bash
   curl -x http://127.0.0.1:8800 https://test.api.local -k -v 2>&1 | head -30
   ```
3. 通过代理请求普通域名：
   ```bash
   curl -x http://127.0.0.1:8800 https://httpbin.org/get -k -v 2>&1 | grep "SSL connection"
   ```

**预期结果**：
- 服务正常启动，日志中显示全局 TLS 拦截已禁用，但 `*.api.local` 被强制拦截
- 步骤 2 请求 `test.api.local` 时，代理对该域名进行 TLS 拦截（`--intercept-include` 优先级最高）
- 步骤 3 请求 `httpbin.org` 时，代理以 CONNECT 隧道方式通过，不进行拦截

---

### TC-CSA-03：--intercept-include 与 --intercept-exclude 同时使用（include 优先级更高）

**操作步骤**：
1. 执行以下命令启动服务：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --intercept \
     --intercept-exclude "*.example.com" \
     --intercept-include "api.example.com"
   ```
2. 通过代理请求被 include 指定的域名：
   ```bash
   curl -x http://127.0.0.1:8800 https://api.example.com -k -v 2>&1 | head -30
   ```
3. 通过代理请求被 exclude 但未被 include 的域名：
   ```bash
   curl -x http://127.0.0.1:8800 https://www.example.com -k -v 2>&1 | head -30
   ```

**预期结果**：
- 服务正常启动
- 步骤 2 请求 `api.example.com` 时被拦截（`--intercept-include` 优先级最高，覆盖 exclude 规则）
- 步骤 3 请求 `www.example.com` 时不被拦截（匹配 `--intercept-exclude` 规则）

---

### TC-CSA-04：--app-intercept-exclude 排除指定应用的 TLS 拦截

**操作步骤**：
1. 执行以下命令启动服务（启用拦截，但排除 Finder 和 Spotlight）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --intercept \
     --app-intercept-exclude "*Finder,*Spotlight"
   ```
2. 使用 `curl` 通过代理请求 HTTPS 网站验证正常拦截：
   ```bash
   curl -x http://127.0.0.1:8800 https://httpbin.org/get -k
   ```
3. 通过 Web UI 查看配置确认应用排除列表已生效：
   ```bash
   curl http://127.0.0.1:8800/_bifrost/api/config | python3 -m json.tool | grep -A 5 "app_intercept_exclude"
   ```

**预期结果**：
- 服务正常启动
- 步骤 2 `curl` 请求正常被拦截（curl 不在排除列表中）
- 步骤 3 配置中显示 `app_intercept_exclude` 包含 `*Finder` 和 `*Spotlight`
- 来自被排除应用的流量将以 CONNECT 隧道方式通过，不进行 TLS 拦截

---

### TC-CSA-05：--app-intercept-include 强制拦截指定应用

**操作步骤**：
1. 执行以下命令启动服务（全局拦截关闭，但强制拦截 Chrome 和 curl 应用）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-intercept \
     --app-intercept-include "*Chrome,*curl"
   ```
2. 使用 `curl` 通过代理请求 HTTPS 网站：
   ```bash
   curl -x http://127.0.0.1:8800 https://httpbin.org/get -k
   ```
3. 通过 API 查看配置确认应用白名单已生效：
   ```bash
   curl http://127.0.0.1:8800/_bifrost/api/config | python3 -m json.tool | grep -A 5 "app_intercept_include"
   ```

**预期结果**：
- 服务正常启动，日志中显示全局拦截已禁用，但 `*Chrome` 和 `*curl` 被强制拦截
- 步骤 2 `curl` 发起的请求被拦截（匹配 `*curl` 规则，`--app-intercept-include` 优先级最高）
- 步骤 3 配置中显示 `app_intercept_include` 包含 `*Chrome` 和 `*curl`

---

### TC-CSA-06：--system-proxy 启动时自动启用系统代理

**操作步骤**：
1. 执行以下命令启动服务（启用系统代理）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --system-proxy
   ```
2. 检查系统代理状态：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- system-proxy status
   ```
3. 或通过 macOS 命令检查：
   ```bash
   networksetup -getwebproxy Wi-Fi
   ```

**预期结果**：
- 服务正常启动
- 步骤 2 显示系统代理已启用，代理地址指向 `127.0.0.1:8800`
- 步骤 3 macOS 网络设置中 Web 代理已启用，指向 `127.0.0.1:8800`
- 停止服务后系统代理自动恢复原始状态

**清理**：
```bash
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- stop
```

---

### TC-CSA-07：--system-proxy 配合 --proxy-bypass 设置系统代理绕过列表

**操作步骤**：
1. 执行以下命令启动服务：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl \
     --system-proxy --proxy-bypass "localhost,127.0.0.1,*.local,192.168.0.0/16"
   ```
2. 检查系统代理绕过列表：
   ```bash
   networksetup -getproxybypassdomains Wi-Fi
   ```

**预期结果**：
- 服务正常启动，系统代理已启用
- 步骤 2 系统代理绕过列表中包含 `localhost`、`127.0.0.1`、`*.local`、`192.168.0.0/16`
- 访问绕过列表中的地址不经过代理

**清理**：
```bash
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- stop
```

---

### TC-CSA-08：默认启动（无 CLI 参数）系统代理自动启用

**操作步骤**：
1. 删除临时数据目录以模拟全新安装：
   ```bash
   rm -rf ./.bifrost-test
   ```
2. 执行以下命令启动服务（不带任何系统代理参数）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl
   ```
3. 检查系统代理状态：
   ```bash
   networksetup -getwebproxy Wi-Fi
   ```

**预期结果**：
- 服务正常启动，日志中显示 `System proxy: enabled`
- 步骤 3 macOS 网络设置中 Web 代理已启用，指向 `127.0.0.1:8800`
- 默认行为变更：系统代理现在默认启用，无需显式指定 `--system-proxy`

**清理**：
```bash
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- stop
```

---

### TC-CSA-08a：--no-system-proxy 显式禁用系统代理

**操作步骤**：
1. 执行以下命令启动服务（显式禁用系统代理）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-system-proxy
   ```
2. 检查系统代理状态：
   ```bash
   networksetup -getwebproxy Wi-Fi
   ```

**预期结果**：
- 服务正常启动
- 日志中不显示 `System proxy: enabled`
- 步骤 2 系统代理未被启用
- 代理功能本身正常工作

**清理**：
```bash
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- stop
```

---

### TC-CSA-08b：--system-proxy 与 --no-system-proxy 互斥

**操作步骤**：
1. 执行以下命令（同时指定两个互斥参数）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --system-proxy --no-system-proxy
   ```

**预期结果**：
- 命令执行失败，退出码非 0
- 错误信息包含 `cannot be used with` 或类似的互斥冲突提示

---

### TC-CSA-09：--cli-proxy 写入 Shell RC 文件代理环境变量

**操作步骤**：
1. 备份当前 shell rc 文件：
   ```bash
   cp ~/.zshrc ~/.zshrc.bak 2>/dev/null || true
   ```
2. 执行以下命令启动服务：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --cli-proxy
   ```
3. 检查 shell rc 文件是否被写入代理环境变量：
   ```bash
   grep -i "http_proxy\|https_proxy\|all_proxy" ~/.zshrc
   ```

**预期结果**：
- 服务正常启动
- Shell rc 文件中写入了 `http_proxy`、`https_proxy`、`all_proxy` 等环境变量，指向 `http://127.0.0.1:8800`
- 新开终端窗口中 `echo $http_proxy` 输出代理地址

**清理**：
```bash
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- stop
cp ~/.zshrc.bak ~/.zshrc 2>/dev/null || true
```

---

### TC-CSA-10：--cli-proxy-no-proxy 设置 CLI 代理 no_proxy 列表

**操作步骤**：
1. 执行以下命令启动服务：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl \
     --cli-proxy --cli-proxy-no-proxy "localhost,127.0.0.1,*.internal.corp"
   ```
2. 检查 shell rc 文件中的 no_proxy 变量：
   ```bash
   grep -i "no_proxy" ~/.zshrc
   ```

**预期结果**：
- 服务正常启动
- Shell rc 文件中写入了 `no_proxy` 环境变量，值包含 `localhost,127.0.0.1,*.internal.corp`
- 新开终端中 `echo $no_proxy` 输出对应值

**清理**：
```bash
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- stop
```

---

### TC-CSA-11：--access-mode whitelist 启动时指定白名单访问模式

**操作步骤**：
1. 执行以下命令启动服务：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl \
     --access-mode whitelist
   ```
2. 检查访问控制模式：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- whitelist status
   ```
3. 从 localhost 发起代理请求：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/get
   ```

**预期结果**：
- 服务正常启动
- 步骤 2 显示访问模式为 `whitelist`
- 步骤 3 localhost（127.0.0.1）始终允许访问，请求正常返回
- 未在白名单中的外部 IP 访问时被拒绝

---

### TC-CSA-12：--access-mode whitelist 配合 --whitelist 指定初始白名单

**操作步骤**：
1. 执行以下命令启动服务：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl \
     --access-mode whitelist --whitelist "192.168.1.100,10.0.0.0/8"
   ```
2. 检查白名单列表：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- whitelist list
   ```

**预期结果**：
- 服务正常启动
- 步骤 2 白名单列表中包含 `192.168.1.100` 和 `10.0.0.0/8`
- 来自 `192.168.1.100` 或 `10.0.0.0/8` 网段的客户端可以使用代理
- 不在白名单中的 IP 被拒绝

---

### TC-CSA-13：--access-mode 使用各种合法值

**操作步骤**：
1. 分别使用以下 access-mode 值启动服务并验证：

   **local_only**：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --access-mode local_only
   ```
   验证后按 Ctrl+C 停止。

   **interactive**：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --access-mode interactive
   ```
   验证后按 Ctrl+C 停止。

   **allow_all**：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --access-mode allow_all
   ```
   验证后按 Ctrl+C 停止。

2. 每次启动后执行：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- whitelist status
   ```

**预期结果**：
- `local_only`：仅允许 localhost 访问
- `interactive`：新 IP 访问时进入待审批队列
- `allow_all`：允许所有客户端访问
- 每种模式下 `whitelist status` 显示对应的访问模式

---

### TC-CSA-14：--access-mode 使用非法值时拒绝启动

**操作步骤**：
1. 执行以下命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --access-mode invalid_mode
   ```

**预期结果**：
- CLI 报错，提示 `invalid_mode` 不是合法的 access-mode 值
- 提示可选值为 `local_only`、`whitelist`、`interactive`、`allow_all`
- 服务不会启动
- 进程以非零退出码退出

---

### TC-CSA-15：--enable-badge-injection 启用 HTML Badge 注入

**操作步骤**：
1. 执行以下命令启动服务：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --intercept --enable-badge-injection
   ```
2. 通过代理请求一个 HTML 页面：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/html -k
   ```
3. 查看配置确认 badge 注入状态：
   ```bash
   curl http://127.0.0.1:8800/_bifrost/api/config | python3 -m json.tool | grep -i badge
   ```

**预期结果**：
- 服务正常启动
- 步骤 2 返回的 HTML 内容中包含 Bifrost badge 注入的标记（如额外的 `<script>` 或 `<div>` 元素）
- 步骤 3 配置中 badge 注入相关字段为启用状态

---

### TC-CSA-16：--disable-badge-injection 禁用 HTML Badge 注入

**操作步骤**：
1. 执行以下命令启动服务：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --intercept --disable-badge-injection
   ```
2. 通过代理请求一个 HTML 页面：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/html -k
   ```
3. 查看配置确认 badge 注入状态：
   ```bash
   curl http://127.0.0.1:8800/_bifrost/api/config | python3 -m json.tool | grep -i badge
   ```

**预期结果**：
- 服务正常启动
- 步骤 2 返回的 HTML 内容不包含任何 Bifrost badge 注入的额外标记
- 步骤 3 配置中 badge 注入相关字段为禁用状态

---

### TC-CSA-17：--enable-badge-injection 与 --disable-badge-injection 互斥检查

**操作步骤**：
1. 执行以下命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --enable-badge-injection --disable-badge-injection
   ```

**预期结果**：
- CLI 报错，提示 `--enable-badge-injection` 和 `--disable-badge-injection` 不能同时使用（clap 的 `conflicts_with` 机制）
- 服务不会启动
- 进程以非零退出码退出

---

### TC-CSA-18：--no-disconnect-on-config-change 禁用 TLS 配置变更时自动断连

**操作步骤**：
1. 执行以下命令启动服务：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --intercept --no-disconnect-on-config-change
   ```
2. 通过代理建立一个长连接或持续请求：
   ```bash
   curl -x http://127.0.0.1:8800 https://httpbin.org/get -k
   ```
3. 通过 API 修改 TLS 配置（如添加 exclude 域名）：
   ```bash
   curl -X PUT http://127.0.0.1:8800/_bifrost/api/config/tls \
     -H "Content-Type: application/json" \
     -d '{"exclude": ["*.newdomain.com"]}'
   ```
4. 验证现有连接未被断开（再次请求正常返回）：
   ```bash
   curl -x http://127.0.0.1:8800 https://httpbin.org/get -k
   ```

**预期结果**：
- 服务正常启动
- 步骤 3 修改 TLS 配置后，现有连接不会被自动断开
- 步骤 4 代理请求仍然正常工作
- 如果不使用 `--no-disconnect-on-config-change`，修改 TLS 配置后受影响的连接会被主动断开

---

### TC-CSA-19：--skip-cert-check 跳过 CA 证书安装检查

**操作步骤**：
1. 执行以下命令启动服务（跳过 CA 证书检查）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --skip-cert-check
   ```
2. 验证代理正常工作：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/get
   ```

**预期结果**：
- 服务正常启动，不会提示 CA 证书未安装或未信任的警告
- 即使 CA 证书未安装到系统信任链，也不会阻止启动流程
- 代理功能正常工作

---

### TC-CSA-20：--log-level debug 设置日志级别为 debug

**操作步骤**：
1. 执行以下命令启动服务：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -l debug start -p 8800 --unsafe-ssl
   ```
2. 通过代理发起请求：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/get
   ```
3. 观察终端日志输出

**预期结果**：
- 服务正常启动
- 终端日志输出包含 `DEBUG` 级别的详细日志信息
- 日志中可见请求转发、连接建立等详细调试信息
- `verbose_logging` 自动设为 `true`（debug 级别触发详细业务日志）

---

### TC-CSA-21：--log-level trace 设置最详细日志级别

**操作步骤**：
1. 执行以下命令启动服务：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -l trace start -p 8800 --unsafe-ssl
   ```
2. 通过代理发起请求：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/get
   ```
3. 观察终端日志输出

**预期结果**：
- 服务正常启动
- 终端日志输出包含 `TRACE` 级别的最详细日志信息
- 日志量明显多于 debug 级别
- 包含底层连接、字节流等跟踪信息

---

### TC-CSA-22：--log-output file 仅输出日志到文件

**操作步骤**：
1. 执行以下命令启动服务：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- --log-output file start -p 8800 --unsafe-ssl
   ```
2. 观察终端是否有日志输出
3. 通过代理发起请求：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/get
   ```
4. 检查日志文件目录：
   ```bash
   ls -la .bifrost-test/logs/
   ```

**预期结果**：
- 服务正常启动
- 终端无日志输出（或仅有最基本的启动信息）
- 步骤 4 日志文件目录中存在日志文件，内容包含请求处理日志
- 代理功能正常工作

---

### TC-CSA-23：--log-output console 仅输出日志到终端

**操作步骤**：
1. 清理旧日志文件：
   ```bash
   rm -rf .bifrost-test/logs/
   ```
2. 执行以下命令启动服务：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- --log-output console start -p 8800 --unsafe-ssl
   ```
3. 通过代理发起请求：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/get
   ```
4. 检查日志文件目录：
   ```bash
   ls -la .bifrost-test/logs/ 2>&1
   ```

**预期结果**：
- 服务正常启动
- 终端有日志输出，包含请求处理信息
- 步骤 4 日志文件目录不存在或为空（日志不写入文件）

---

### TC-CSA-24：--log-dir 指定自定义日志目录

**操作步骤**：
1. 创建临时日志目录：
   ```bash
   mkdir -p /tmp/bifrost-test-logs
   ```
2. 执行以下命令启动服务：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- --log-dir /tmp/bifrost-test-logs start -p 8800 --unsafe-ssl
   ```
3. 通过代理发起请求：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/get
   ```
4. 检查自定义日志目录：
   ```bash
   ls -la /tmp/bifrost-test-logs/
   ```

**预期结果**：
- 服务正常启动
- 步骤 4 自定义日志目录中存在日志文件
- 日志文件内容包含代理请求处理信息
- 默认日志目录（`.bifrost-test/logs/`）不会产生新的日志文件

**清理**：
```bash
rm -rf /tmp/bifrost-test-logs
```

---

### TC-CSA-25：--log-retention-days 设置日志保留天数

**操作步骤**：
1. 执行以下命令启动服务（日志保留 3 天）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- --log-retention-days 3 start -p 8800 --unsafe-ssl
   ```
2. 验证服务正常启动：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/get
   ```
3. 检查日志文件：
   ```bash
   ls -la .bifrost-test/logs/
   ```

**预期结果**：
- 服务正常启动，日志保留天数配置为 3 天
- 超过 3 天的旧日志文件将被自动清理
- 当前日志文件正常生成
- 默认值为 7 天，此处验证可自定义覆盖

---

### TC-CSA-26：RUST_LOG 环境变量优先于 --log-level 参数

**操作步骤**：
1. 执行以下命令启动服务（`RUST_LOG` 设为 debug，`--log-level` 设为 warn）：
   ```bash
   RUST_LOG=debug BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -l warn start -p 8800 --unsafe-ssl
   ```
2. 通过代理发起请求：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/get
   ```
3. 观察终端日志输出级别

**预期结果**：
- 服务正常启动
- 终端日志输出包含 `DEBUG` 级别日志（`RUST_LOG` 优先）
- `--log-level warn` 被 `RUST_LOG=debug` 覆盖
- 符合日志级别优先级：`RUST_LOG` > `--log-level` > 默认值 `info`

---

### TC-CSA-27：多参数组合 —— TLS 拦截域名排除 + 应用白名单 + 系统代理

**操作步骤**：
1. 执行以下命令启动服务（组合多个高级参数）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --intercept \
     --intercept-exclude "*.apple.com,*.icloud.com" \
     --app-intercept-include "*curl" \
     --system-proxy \
     --proxy-bypass "localhost,127.0.0.1"
   ```
2. 验证 TLS 拦截排除生效：
   ```bash
   curl -x http://127.0.0.1:8800 https://www.apple.com -k -v 2>&1 | head -20
   ```
3. 验证 curl 应用被强制拦截（即使域名在 exclude 列表中）：
   ```bash
   curl -x http://127.0.0.1:8800 https://httpbin.org/get -k
   ```
4. 验证系统代理已启用：
   ```bash
   networksetup -getwebproxy Wi-Fi
   ```

**预期结果**：
- 服务正常启动，所有参数均生效
- 步骤 2 访问 `*.apple.com` 时不被拦截（匹配 exclude 规则）
- 步骤 3 curl 请求 `httpbin.org` 时被拦截（匹配 app include 规则）
- 步骤 4 系统代理已启用，bypass 列表包含 `localhost,127.0.0.1`
- 各参数之间不冲突，按优先级正确生效

**清理**：
```bash
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- stop
```

---

### TC-CSA-28：多参数组合 —— 白名单模式 + CLI 代理 + 日志配置

**操作步骤**：
1. 执行以下命令启动服务：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -l debug --log-output file --log-dir /tmp/bifrost-test-logs --log-retention-days 1 \
     start -p 8800 --unsafe-ssl \
     --access-mode whitelist --whitelist "192.168.1.0/24" \
     --cli-proxy --cli-proxy-no-proxy "localhost,127.0.0.1"
   ```
2. 验证白名单模式生效：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- whitelist status
   ```
3. 验证白名单列表：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- whitelist list
   ```
4. 检查日志文件：
   ```bash
   ls -la /tmp/bifrost-test-logs/
   ```
5. 检查 CLI 代理环境变量：
   ```bash
   grep -i "http_proxy\|no_proxy" ~/.zshrc
   ```

**预期结果**：
- 服务正常启动
- 步骤 2 显示访问模式为 `whitelist`
- 步骤 3 白名单包含 `192.168.1.0/24`
- 步骤 4 日志文件在 `/tmp/bifrost-test-logs/` 中，包含 debug 级别日志
- 步骤 5 shell rc 文件中包含代理环境变量和 `no_proxy` 配置

**清理**：
```bash
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- stop
rm -rf /tmp/bifrost-test-logs
```

---

### TC-CSA-29：--skip-cert-check 配合 --intercept 跳过证书检查并启用拦截

**操作步骤**：
1. 清理旧的证书数据：
   ```bash
   rm -rf .bifrost-test
   ```
2. 执行以下命令启动服务（全新环境，跳过证书安装检查并启用拦截）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --intercept --skip-cert-check
   ```
3. 验证代理正常工作：
   ```bash
   curl -x http://127.0.0.1:8800 https://httpbin.org/get -k
   ```

**预期结果**：
- 服务正常启动，不会因为 CA 证书未安装到系统信任链而中断启动流程
- CA 证书自动生成（如果不存在）
- 步骤 3 HTTPS 请求通过代理正常完成（使用 `-k` 跳过客户端证书验证）
- TLS 拦截正常工作

---

### TC-CSA-30：--log-level 使用非法值时拒绝启动

**操作步骤**：
1. 执行以下命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -l invalid start -p 8800 --unsafe-ssl
   ```

**预期结果**：
- CLI 报错，提示 `invalid` 不是合法的日志级别值
- 提示可选值为 `trace`、`debug`、`info`、`warn`、`error`
- 服务不会启动
- 进程以非零退出码退出

---

### TC-CSA-31：daemon 模式继承 --log-level 到文件日志

**操作步骤**：
1. 清理旧测试数据并创建独立日志目录：
   ```bash
   export TEST_DIR="$PWD/.bifrost-test-daemon-log"
   rm -rf "$TEST_DIR"
   mkdir -p "$TEST_DIR/logs"
   ```
2. 以 daemon 模式启动服务，并显式指定 `debug` 日志级别：
   ```bash
   BIFROST_DATA_DIR="$TEST_DIR" cargo run --bin bifrost -- -l debug --log-dir "$TEST_DIR/logs" start -p 8801 --unsafe-ssl --daemon
   ```
3. 等待服务启动后，通过代理发起一个请求，触发运行期日志：
   ```bash
   curl -x http://127.0.0.1:8801 http://httpbin.org/get
   ```
4. 检查 daemon 日志文件内容：
   ```bash
   grep -n "DEBUG" "$TEST_DIR/logs/bifrost.log" || grep -n "DEBUG" "$TEST_DIR/logs"/bifrost*.log
   ```
5. 停止测试进程：
   ```bash
   BIFROST_DATA_DIR="$TEST_DIR" cargo run --bin bifrost -- stop
   ```

**预期结果**：
- daemon 服务正常启动，监听 `127.0.0.1:8801`
- 步骤 3 的代理请求正常返回
- 步骤 4 的日志文件中存在 `DEBUG` 级别日志，说明 daemon 子进程继承了 CLI 传入的 `--log-level debug`
- 未显式设置 `RUST_LOG` 时，不会再退回为硬编码 `info`

---

### TC-CSA-32：顶层 `bifrost --help` 的 start 参考区块与实际参数保持一致

**操作步骤**：
1. 执行顶层帮助命令并保存输出：
   ```bash
   ./target/debug/bifrost --help > /tmp/bifrost-root-help.txt
   ```
2. 执行 `start` 子命令帮助并保存输出：
   ```bash
   ./target/debug/bifrost start --help > /tmp/bifrost-start-help.txt
   ```
3. 校验顶层帮助的 `start` 参考区块包含关键参数：
   ```bash
   rg --fixed-strings -- '--no-system-proxy' /tmp/bifrost-root-help.txt
   rg --fixed-strings -- '--proxy-bypass' /tmp/bifrost-root-help.txt
   rg --fixed-strings -- '--cli-proxy' /tmp/bifrost-root-help.txt
   rg --fixed-strings -- '--cli-proxy-no-proxy' /tmp/bifrost-root-help.txt
   rg --fixed-strings -- '-y, --yes' /tmp/bifrost-root-help.txt
   ```
4. 校验 `start --help` 也包含同一组关键参数：
   ```bash
   rg --fixed-strings -- '--no-system-proxy' /tmp/bifrost-start-help.txt
   rg --fixed-strings -- '--proxy-bypass' /tmp/bifrost-start-help.txt
   rg --fixed-strings -- '--cli-proxy' /tmp/bifrost-start-help.txt
   rg --fixed-strings -- '--cli-proxy-no-proxy' /tmp/bifrost-start-help.txt
   rg --fixed-strings -- '--yes' /tmp/bifrost-start-help.txt
   ```

**预期结果**：
- 顶层 `bifrost --help` 的 `SUBCOMMAND REFERENCE` 中能看到 `start [OPTIONS]`
- 顶层帮助包含 `--no-system-proxy`、`--proxy-bypass`、`--cli-proxy`、`--cli-proxy-no-proxy`、`-y, --yes`
- `bifrost start --help` 包含同一组关键参数
- 两份帮助输出对 `start` 参数的关键集合描述一致，不会遗漏 `--no-system-proxy`

---

## 清理

测试完成后清理临时数据：
```bash
rm -rf .bifrost-test
rm -rf /tmp/bifrost-test-logs
rm -rf .bifrost-test-daemon-log
```

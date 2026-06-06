# Mobile Device Trust Wizard 真实场景测试

## 功能模块说明

验证 Settings -> Certificate 中的手机证书安装向导、Availability Check、管理端全局 USB 设备监听自动提示、CLI `ca install --mobile` / `ca install --ios`，以及对应 Admin API。该功能默认只承诺普通手机的 guided install：Bifrost 可推送证书或提供 iOS profile，但最终安装和信任必须在手机端由用户确认。iOS Web UI 使用同一条安装和信任流程讲解，Apple Configurator 与扫码/文件安装只是把 profile 送到 iPhone 的两种入口。Availability Check 的核心判断不是读取设备证书库，而是由目标设备自己检查代理访问授权、探针端口可达性，并完成一次由当前 Bifrost CA 签发证书的真实 HTTPS 握手。

## 前置条件

1. 在仓库根目录启动测试服务，必须使用临时数据目录并禁用系统代理与 Sync 自动登录弹窗：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-mobile-test BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-system-proxy
   ```
2. 等待服务启动完成：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/cert/info | jq .
   ```
3. Web UI 验证时打开：
   ```text
   http://127.0.0.1:8800/_bifrost/settings?tab=certificate
   ```

## 测试用例列表

### TC-MDT-01：移动设备发现 API 返回普通手机确认边界

**操作步骤**：

1. 执行：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/mobile-devices | jq .
   ```

**预期结果**：

- 返回 HTTP 200。
- JSON 包含 `android.adb_available`、`android.devices`、`ordinary_device_notice`、`managed_device_notice`。
- JSON 包含 `ios.supported`、`ios.devices`、`ios.configurator` 和 `ios.message`。
- `ordinary_device_notice` 明确说明普通手机需要在手机端做最终确认。
- `ios.configurator.message` 明确说明 Apple Configurator/cfgutil 可用状态；未安装时提示安装 Apple Configurator。
- 未安装 ADB 时，`android.adb_available=false`，`android.message` 提示安装 Android Platform Tools 或配置 `BIFROST_ADB_PATH`。

### TC-MDT-02：iOS mobileconfig 下载包含 root payload 和手动信任提示

**操作步骤**：

1. 执行：
   ```bash
   curl -sI http://127.0.0.1:8800/_bifrost/public/mobile/ios-profile.mobileconfig
   ```
2. 执行：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/public/mobile/ios-profile.mobileconfig -o /tmp/bifrost-ca.mobileconfig
   ```
3. 执行：
   ```bash
   grep -E "com.apple.security.root|Certificate Trust Settings" /tmp/bifrost-ca.mobileconfig
   ```

**预期结果**：

- 第 1 步返回 HTTP 200。
- `Content-Type` 包含 `application/x-apple-aspen-config`。
- 下载文件包含 `com.apple.security.root`。
- 下载文件包含 `Certificate Trust Settings` 手动开启完全信任提示。

### TC-MDT-03：iOS profile QR endpoint 返回 SVG

**操作步骤**：

1. 执行：
   ```bash
   curl -sI "http://127.0.0.1:8800/_bifrost/public/mobile/ios-profile.mobileconfig/qrcode?ip=127.0.0.1"
   ```
2. 执行：
   ```bash
   curl -s "http://127.0.0.1:8800/_bifrost/public/mobile/ios-profile.mobileconfig/qrcode?ip=127.0.0.1" | head -5
   ```

**预期结果**：

- 返回 HTTP 200。
- `Content-Type` 为 `image/svg+xml`。
- 响应体包含 `<svg`。

### TC-MDT-03b：LAN 地址下载 iOS profile 不需要授权

**操作步骤**：

1. 获取当前 Bifrost 展示的局域网地址，例如 `192.168.8.34`。
2. 执行：
   ```bash
   curl -sSI "http://192.168.8.34:8800/_bifrost/public/mobile/ios-profile.mobileconfig"
   ```
3. 执行：
   ```bash
   curl -sS "http://192.168.8.34:8800/_bifrost/public/mobile/ios-profile.mobileconfig" -o /tmp/bifrost-lan-ios.mobileconfig
   plutil -lint /tmp/bifrost-lan-ios.mobileconfig
   grep -E "com.apple.security.root|Certificate Trust Settings" /tmp/bifrost-lan-ios.mobileconfig
   ```
4. 执行：
   ```bash
   curl -sS "http://192.168.8.34:8800/_bifrost/public/mobile/ios-profile.mobileconfig/qrcode?ip=192.168.8.34" | head -5
   ```

**预期结果**：

- 第 2 步返回 HTTP 200，而不是 401/403/connection reset。
- `Content-Type` 包含 `application/x-apple-aspen-config`。
- 下载文件 `plutil -lint` 返回 OK。
- 下载文件包含 `com.apple.security.root` 和 `Certificate Trust Settings`。
- 服务日志不会出现针对该 LAN 客户端的 pending authorization 提示。
- QR endpoint 返回 SVG，且扫码下载的必须是同一个 LAN profile URL。

### TC-MDT-04：Android install 缺少确认时拒绝

**操作步骤**：

1. 执行：
   ```bash
   curl -s -X POST http://127.0.0.1:8800/_bifrost/api/mobile-devices/test-device/install-ca \
     -H 'Content-Type: application/json' \
     -d '{"mode":"normal_guide"}' \
     -w "\n%{http_code}\n"
   ```

**预期结果**：

- 返回 HTTP 400。
- 响应体包含 `Missing install confirmation`。
- 不会执行 ADB 推送。

### TC-MDT-04b：本机 CA install 缺少确认时拒绝

**操作步骤**：

1. 执行：
   ```bash
   curl -s -X POST http://127.0.0.1:8800/_bifrost/api/cert/install \
     -H 'Content-Type: application/json' \
     -d '{}' \
     -w "\n%{http_code}\n"
   ```

**预期结果**：

- 返回 HTTP 400。
- 响应体包含 `Missing local CA install confirmation`。
- 不会触发 macOS keychain、sudo、Windows UAC 或 Linux trust store 安装提示。

### TC-MDT-05：Settings Certificate UI 展示手机安装向导且不承诺自动信任

**操作步骤**：

1. 打开 `http://127.0.0.1:8800/_bifrost/settings?tab=certificate`。
2. 查看 Certificate 页左侧导航和右侧章节顺序。
3. 查看 iOS、Android 和证书下载二维码区域。
4. 点击 Android 区域的 `Detect Devices`。

**预期结果**：

- 页面左侧展示固定导航 `Certificate Setup`。
- 左侧导航包含 `Availability check`、`Local install`、`iOS devices`、`Android devices`、`Certificate downloads`。
- 右侧内容为单列章节，不把 Android 和 iOS 做成左右并列卡片。
- 右侧章节顺序为 `Availability Check`、`Local Certificate Install`、`iOS Mobile Installation`、`Android Mobile Installation`、`Certificate Downloads and QR Codes`。
- 打开 `#certificate-mobile-ios`、`#certificate-local-install` 等锚点时，左侧导航有选中态且页面滚动到对应章节；点击 `Availability check` 会滚动到页面最顶部的可用性检查卡片，而不是跳到二维码下载区域。
- `Local Certificate Install` 章节在本机 CA 未完全信任时展示 `Install and Trust CA` 或 `Trust CA` 按钮，按钮说明等价于执行 `bifrost ca install`；CA 文件不可用时按钮禁用。
- 点击本机安装按钮并完成系统授权后，页面会持续刷新本机 CA 状态，最终自动显示 `Installed and trusted`；不需要用户重新强刷页面才能看到信任完成状态。
- `Availability Check` 章节位于页面顶部，可以选择本机局域网 IP，点击 `Generate Availability Check` 后展示二维码、分享链接、实时状态和最近事件。
- Availability Check 状态展示代理访问授权、页面是否打开、probe 端口是否可达、HTTPS trust check 是否通过。
- Availability Check 成功文案只承诺“当前设备浏览器信任 Bifrost CA”，并说明个别 App 仍可能因为 certificate pinning、自定义 TLS 或 Android user CA 策略无法解密。
- iOS 章节展示在 Android 章节之前。
- iPhone/iPad 区域先展示 `iOS uses one shared install and trust flow`，并明确四步：送达 profile、在 Settings 安装 profile、进入 `Settings > General > About > Certificate Trust Settings`、开启 Bifrost CA 完全信任。
- iPhone/iPad 区域随后展示 `1. Choose how to send the profile`。
- 送达方式里展示 `Send profile with Apple Configurator`，并显示每台设备的 `Configurator Install` 按钮。
- 送达方式里也展示 `Or send profile manually with QR or file`、`ios_qr_1`、`ios_qr_2`、`Download iOS Profile` 按钮和 profile QR。
- iPhone/iPad 区域在送达方式之后展示 `2. Finish the shared iOS Settings steps` 图文步骤。
- 页面说明明确表达：Apple Configurator 和扫码/文件安装只是把 profile 送到 iPhone；profile 到达后仍要按同一套 Settings 安装和信任步骤继续。
- Android 区域展示 `Detect Devices` 按钮。
- 最后一个章节展示 `Download CA Certificate` 按钮和证书 QR 下载。
- 页面没有出现 `auto trust`、`silent install`、`enable trust automatically` 等对普通手机的误导承诺。
- 点击 `Detect Devices` 后，ADB 未安装时展示 ADB 不可用提示；若 ADB 已安装则展示设备列表或无设备提示。

### TC-MDT-06：浅色与暗色主题下手机安装向导可读

**操作步骤**：

1. 在 Settings 页面切换到浅色主题，打开 Certificate Tab。
2. 检查 Android USB、iPhone/iPad、证书文件 QR 区域。
3. 切换到暗色主题，重复检查。

**预期结果**：

- 浅色和暗色主题下文本、边框、按钮、标签和二维码区域均可读。
- Android/iOS 区块没有文字重叠或按钮溢出。
- 安装与下载按钮状态清晰可识别。

### TC-MDT-07：任意管理端页面检测到 connected Android/iOS 后全局弹窗并跳转证书页

**操作步骤**：

1. 打开任意非 Certificate 页面，例如 `http://127.0.0.1:8800/_bifrost/traffic`，不刷新页面。
2. Mock `/api/mobile-devices/refresh` 第一轮返回空设备列表，第二轮返回一个 `status=connected` 的 Android 或 iOS 设备。
3. 等待 3 秒以上，让页面轮询完成。
4. 点击弹窗中的 `Open Certificate Setup`。

**预期结果**：

- 不需要手动进入 Certificate Tab，也不需要刷新页面。
- Android 设备自动弹出 `Install Bifrost CA on connected Android device?` 全局确认窗口。
- iOS 设备自动弹出 `Install Bifrost CA profile on connected iPhone?` 全局确认窗口。
- 弹窗按钮文案为 `Open Certificate Setup`，确认后跳转到 `Settings > Certificate`，URL 包含 `mobile_device=<device id>` 和 `mobile_platform=<android|ios>`。
- 弹窗说明手机仍需确认安装和信任；不会直接声称已自动信任。
- 点击 `Not now` 后，同一设备 id 在当前页面会话中不会在下一轮轮询重复弹出。
- 旧的浏览器 localStorage 记录不会阻止新页面会话弹出。
- 进入 Certificate 页后，设备列表继续自动刷新并展示对应 Android/iOS 安装引导。
- 目标设备卡片自动滚动到可见区域，即使用户原本在非 Certificate 页面或证书页高度较长，也不需要手动滚动查找。
- 目标设备卡片以主色边框/背景高亮；Android 目标设备的 `Install` 按钮或 iOS 目标设备的 `Configurator Install` 按钮播放脉冲动画。

### TC-MDT-07b：真实连接 iPhone 后 API 和 Web UI 显示 iOS USB 设备

**操作步骤**：

1. 将 iPhone 或 iPad 通过 USB 连接到运行 Bifrost 的 Mac，并在手机上完成“信任此电脑”提示。
2. 执行：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/mobile-devices | jq '.ios'
   ```
3. 打开 Certificate Tab，查看 `iPhone and iPad` 区域。

**预期结果**：

- API 返回 `ios.supported=true`。
- `ios.devices` 至少包含一个 `platform=ios`、`status=connected` 的设备。
- 设备 id 为 iOS USB serial，名称显示为 `iPhone` 或 `iPad`。
- Web UI 的 `iPhone and iPad` 区域显示该设备，并仍提示下载 profile 后在手机上手动启用完全信任。
- Web UI 同时显示 Apple Configurator 高级路径。如果当前 Mac 未安装 `cfgutil`，`Configurator Install` 按钮不可用并提示安装 Apple Configurator。
- 不承诺通过 USB 静默安装或自动启用 SSL/TLS 完全信任。

### TC-MDT-08：CLI `ca install --mobile --yes` 单设备自动选择并执行 guided install

**操作步骤**：

1. 创建 fake ADB：
   ```bash
   cat > /tmp/bifrost-fake-adb-single <<'SH'
   #!/bin/sh
   echo "$@" >> /tmp/bifrost-fake-adb-single.log
   if [ "$1" = "devices" ]; then
     printf 'List of devices attached\nandroid-1 device product:test model:Pixel_Test device:test transport_id:1\n'
     exit 0
   fi
   exit 0
   SH
   chmod +x /tmp/bifrost-fake-adb-single
   rm -f /tmp/bifrost-fake-adb-single.log
   ```
2. 执行：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-mobile-cli-test \
   BIFROST_ADB_PATH=/tmp/bifrost-fake-adb-single \
   cargo run --bin bifrost -- ca install --mobile --yes
   ```
3. 执行：
   ```bash
   cat /tmp/bifrost-fake-adb-single.log
   ```

**预期结果**：

- CLI 输出包含 `Mobile CA install guide`。
- CLI 输出包含 `Personal phones still require final confirmation on the phone`。
- CLI 输出包含 `Android CA status`，并明确显示 `unknown`、`pushed to device`、`not installed` 或 `installed` 之一。
- fake ADB 日志包含 `devices -l`。
- fake ADB 日志包含 `-s android-1 push`。
- fake ADB 日志包含 `android.intent.action.VIEW`。
- 不出现交互式选择提示。

### TC-MDT-09：CLI `ca install --mobile --yes` 多设备要求显式 `--device`

**操作步骤**：

1. 创建 fake ADB：
   ```bash
   cat > /tmp/bifrost-fake-adb-multi <<'SH'
   #!/bin/sh
   if [ "$1" = "devices" ]; then
     printf 'List of devices attached\nandroid-1 device product:test model:Pixel_One device:test transport_id:1\nandroid-2 device product:test model:Pixel_Two device:test transport_id:2\n'
     exit 0
   fi
   exit 0
   SH
   chmod +x /tmp/bifrost-fake-adb-multi
   ```
2. 执行：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-mobile-cli-test \
   BIFROST_ADB_PATH=/tmp/bifrost-fake-adb-multi \
   cargo run --bin bifrost -- ca install --mobile --yes
   ```

**预期结果**：

- 命令返回非 0。
- 输出包含 `Multiple ready Android devices detected`。
- 输出提示 `Pass --device <serial>`。

### TC-MDT-10：CLI `ca install --ios` 生成 iOS profile 并输出手动信任步骤

**操作步骤**：

1. 执行：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-mobile-cli-test \
   cargo run --bin bifrost -- ca install --ios
   ```
2. 检查生成文件：
   ```bash
   plutil -lint ./.bifrost-mobile-cli-test/certs/bifrost-ca.mobileconfig
   grep -E "com.apple.security.root|Certificate Trust Settings" ./.bifrost-mobile-cli-test/certs/bifrost-ca.mobileconfig
   ```

**预期结果**：

- CLI 输出包含 `iOS CA install guide`。
- CLI 输出包含 `Default path for personal iPhone/iPad`。
- CLI 输出包含 `Settings > General > About > Certificate Trust Settings`。
- CLI 输出说明网页、QR、AirDrop、邮件或 Files 打开的 profile 不会自动启用 SSL/TLS trust。
- CLI 输出包含后续高级命令 `bifrost ca install --ios --configurator`。
- mobileconfig 文件 `plutil -lint` 返回 OK。
- mobileconfig 文件包含 `com.apple.security.root` 和 `Certificate Trust Settings`。

### TC-MDT-11：Settings iPhone/iPad 区域展示两种 profile 送达方式

**操作步骤**：

1. 打开 `http://127.0.0.1:8800/_bifrost/settings?tab=certificate`。
2. 查看 `iPhone and iPad` 区域中的统一流程、Configurator 送达方式、扫码/文件送达方式和共享 Settings 步骤。
3. 若当前 Mac 未安装 Apple Configurator/cfgutil，检查 `Configurator Install` 按钮状态。
4. 若当前 Mac 已安装 Apple Configurator/cfgutil 且只连接一台 iPhone/iPad，点击 `Configurator Install`。

**预期结果**：

- 页面先展示 `iOS uses one shared install and trust flow`，并列出送达 profile、安装 profile、打开证书信任设置、手动开启完全信任四步。
- 页面随后展示 `1. Choose how to send the profile`。
- 页面在送达方式里展示 `Send profile with Apple Configurator` 和 `Or send profile manually with QR or file` 两种 profile 送达方式。
- 手动扫码送达方式展示 `ios_qr_1` 和 `ios_qr_2` 两张图文步骤：相机扫 LAN QR 并点击黄色链接、允许下载 configuration profile。
- 页面在送达方式之后展示 `2. Finish the shared iOS Settings steps`，让用户继续同一套 iOS 设置流程。
- 页面没有把 Configurator 和手动扫码拆成互相割裂的两套安装模式；两者都指向同一套 Settings 安装和信任步骤。
- 未安装 cfgutil 时，页面提示安装 Apple Configurator，`Configurator Install` 按钮禁用。
- 安装 cfgutil 且只连接一台 iOS 设备时，按钮可点击；点击后由本地 API 发起 `managed_auto_trust` 安装请求。
- 未监督设备相关文案提示仍可能需要 Trust、解锁或屏幕确认。
- 页面提示 Configurator 发送 profile 后，如果 iPhone 仍要求确认，要继续按同一套 Settings 步骤操作。

### TC-MDT-12：Apple Configurator 需要手机端交互时显示为待确认而非失败

**操作步骤**：

1. 连接未监督 iPhone，确保 Mac 已信任设备且已安装 Apple Configurator/cfgutil。
2. 打开 Certificate Tab，在 iPhone/iPad 区域点击 `Configurator Install`。
3. 当 iPhone 弹出描述文件安装窗口时，观察 Web UI 的安装 session 提示。

**预期结果**：

- 如果 `cfgutil` 返回 `ConfigurationUtilityKit.error Code: 625` 或“需要用户在设备上交互”，页面不显示 `Apple Configurator could not install the Bifrost profile`。
- 页面显示 Apple Configurator 已打开 iPhone 安装流程，并提示在手机上完成确认。
- session step 仍保留 `cfgutil_install_profile` 的原始信息，方便排查。
- 用户在 iPhone 上确认后，继续按页面统一流程检查 Settings 和 Certificate Trust Settings。

### TC-MDT-13：Settings iPhone/iPad 展示共享 iOS 设置图文步骤

**操作步骤**：

1. 打开 `http://127.0.0.1:8800/_bifrost/settings?tab=certificate`。
2. 滚动到 `iPhone and iPad` 区域。
3. 查看 `1. Choose how to send the profile` 后面的 `2. Finish the shared iOS Settings steps`。

**预期结果**：

- 页面展示 `2. Finish the shared iOS Settings steps`，且该图文步骤出现在 Configurator 和 QR 两种送达方式之后。
- 页面按图片名称展示 `ios_1`、`ios_2`、`ios_3`、`ios_4`、`ios_5`、`ios_6`、`ios_7` 七个步骤。
- 每个步骤都有对应截图，并且截图下方有文字说明。
- `ios_1` 说明 iOS 选择 iPhone 作为 profile 目标；Configurator 已按选中的 USB 设备定向送达。
- `ios_2` 说明 profile 到达后进入 Settings；Configurator 如果直接打开安装界面，也继续同一流程。
- `ios_3` 说明在 Settings 中点击 Downloaded Profile。
- `ios_4` 说明在 Bifrost CA profile 页面点击 Install。
- `ios_5` 说明未签名 profile 警告出现时，确认信任当前 Bifrost 实例后继续 Install。
- `ios_6` 说明 profile 安装后进入 Settings > General > About。
- `ios_7` 说明进入 Certificate Trust Settings，并打开 Bifrost CA 的 full trust。
- 页面文案明确表达 Configurator 和扫码/文件只是送达方式；安装描述文件和开启完全信任是后续共同阶段，不把下载或安装 profile 描述成已经完成 HTTPS 信任。
- 浅色和暗色主题下，七张截图、步骤标题和说明文字均可读，没有重叠或按钮遮挡。

### TC-MDT-14：Android 设备卡片展示当前 CA 状态并区分普通 ADB 与 root/emulator 验证能力

**操作步骤**：

1. 打开 `http://127.0.0.1:8800/_bifrost/settings?tab=certificate#certificate-mobile-android`。
2. 连接一台已授权 USB 调试的 Android 设备，或使用 fake ADB / emulator 测试环境。
3. 点击 Android 区域的 `Detect Devices`。
4. 查看目标 Android 设备卡片。
5. 点击该设备的 `Install`，在手机上完成系统证书安装确认。
6. 等待页面自动刷新一到两轮。
7. 如果使用 root/emulator/test device，确保 `/data/misc/user/0/cacerts-added` 可由 ADB 或 `su 0` 读取，再重复检测。

**预期结果**：

- 设备卡片除 ADB 连接状态外，还展示 CA 状态标签：`CA unknown`、`CA pushed`、`CA not installed` 或 `CA installed`。
- 普通未 root Android 设备不被误标为已安装；如果 Bifrost 只能确认 `/sdcard/Download/bifrost-ca.crt` 与当前 CA 文件一致，则显示 `CA pushed` 并提示普通 ADB 无法验证私有 user CA store。
- root/emulator/test device 可读取 Android user CA store 且当前 CA 指纹匹配时，设备卡片显示 `CA installed`。
- 页面文案仍提示 Android 7+ App 可能不信任用户 CA，证书 pinning 或未配置 Network Security Config 的 App 不保证可被 HTTPS 解密。
- 全局设备弹窗同样展示该 CA 状态，不只展示设备 ID。

### TC-MDT-15：Availability Check 用真实 HTTPS 握手验证设备信任

**操作步骤**：

1. 打开 Certificate Tab 顶部的 `Availability Check` 章节。
2. 在 `Local network address` 中选择当前电脑的局域网 IP。
3. 点击 `Generate Availability Check`。
4. 使用目标手机扫码打开 HTTP landing page。
5. 在手机页面等待检测完成；如失败，按手机页面的 iOS/Android 下一步提示安装并信任 CA 后点击 Retry。
6. 在电脑管理端观察 Availability Check 实时状态。
7. 自动化验证可执行：
   ```bash
   cargo run -p bifrost-e2e -- --test admin_trust_probe_verifies_https_trust_with_current_ca --test-timeout 120
   ```

**预期结果**：

- 管理端生成二维码后显示 session 状态、`Page waiting`、`Probe port pending`、`HTTPS trust pending`、`Proxy access pending`。
- 手机页标题为 `Bifrost Availability Check`，并显示代理访问授权检查结果。
- 手机打开 HTTP landing page 后，管理端状态变为 `Device opened page`。
- 手机能访问 probe 端口时，管理端显示 `Probe port reachable`。
- 手机 HTTPS trust check 成功时，管理端显示 `CA trusted`、`HTTPS trust passed`，并展示代理地址 `<host>:<adminPort>` 和 proxy QR 链接。
- 手机页成功后展示的代理地址是可点击复制的按钮，点击后显示 `Copied` 或清晰提示手动复制。
- 如果 netcheck 成功但 HTTPS trust check 失败，管理端显示 `Trust failed`，手机页提示安装 CA、iOS 开启完全信任或 Android App 信任边界，并明确提示安装/信任后仍失败时需要完整重启浏览器再重试。
- 如果 landing page 能打开但 probe 端口不可达，管理端显示 `Probe unreachable`，并提示检查防火墙、局域网隔离和 IP 选择。
- 成功文案不承诺所有 App 都能被解密，只说明当前设备浏览器 TLS 链路已信任 Bifrost CA。
- 自动化 E2E 使用当前 Bifrost CA 作为 root CA 访问 HTTPS check，并断言 session 最终为 `tls_trusted`。

### TC-MDT-16：Availability Check 公开入口和代理授权检查不被访问控制误拦截

**操作步骤**：

1. 使用局域网地址创建 Availability Check session，例如：
   ```bash
   curl -sS -X POST http://127.0.0.1:8800/_bifrost/api/trust-probe/sessions \
     -H 'Content-Type: application/json' \
     -d '{"host":"192.168.8.34"}' | jq .
   ```
2. 用返回的 `landingUrl` 执行 `curl -sSI`；用返回的 `qrCodeUrl`、`proxyQrCodeUrl` 分别执行 `curl -sS -D <headers> -o <body>`，按真实浏览器打开方式验证 GET。
3. 用返回的 token 访问：
   ```bash
   curl -sS "http://192.168.8.34:8800/_bifrost/public/trust-probe/<session_id>/proxy-access?t=<token>" | jq .
   ```
4. 如果 Bifrost 当前是 interactive 访问控制且客户端不是 loopback，打开管理端访问授权列表检查 pending 记录。

**预期结果**：

- `landingUrl` 返回 HTTP 200，页面标题包含 `Bifrost Availability Check`。
- `qrCodeUrl` 返回 HTTP 200 和 `image/svg+xml`。
- `proxyQrCodeUrl` 返回 HTTP 200 和 `image/svg+xml`，不会因为未授权设备访问而返回 403。
- `proxy-access` 返回 JSON，`status` 为 `allowed`、`pending`、`denied` 或 `unavailable` 之一，并包含可读的 `message`。
- interactive 模式下未授权局域网设备会被记录为 pending authorization，便于用户在管理端批准；loopback 或已授权设备显示 `allowed`。

### TC-MDT-17：代理交互式授权弹窗展示 Availability Check 二维码和链接

**操作步骤**：

1. 启动 Bifrost，并让访问控制处于 interactive 模式。
2. 从一台未授权局域网设备访问代理，触发管理端 `Pending Authorization Requests` 弹窗。
3. 观察弹窗内容，不切换到 Certificate 页面。
4. 使用目标设备扫描弹窗中的 Availability Check 二维码，或复制/打开弹窗中的 Availability Check 链接。
5. 继续在弹窗里批准或拒绝该设备授权请求。

**预期结果**：

- 弹窗仍显示每个待授权设备的 IP、首次出现时间、尝试次数，以及 `Allow` / `Deny` 操作。
- 弹窗中部展示 Availability Check 提示，说明遇到证书或代理问题时可用扫码/链接检查可用性。
- 弹窗打开时会自动生成一组 Availability Check session；用户无需先进入 Certificate 页面再点击生成。
- 弹窗展示局域网地址选择、二维码、可打开的检查链接和 `Copy link` 按钮；二维码和链接指向公开 landing page。
- 管理端轮询 session 状态 2 秒以上后，二维码 URL 和链接仍保留 `?t=<token>`，直接打开链接返回检查页而不是 `Missing trust probe token`。
- 二维码以普通图片展示，不出现 Ant Design 预览灰色遮罩或只显示小二维码图标的白屏/灰屏状态。
- 目标设备打开检查页后，会自动检查代理授权、probe 端口可达性和 HTTPS CA trust。
- `Allow` / `Deny` 操作不受 Availability Check 区块影响；审批后 pending 列表正常刷新。

## 清理步骤

```bash
rm -rf ./.bifrost-mobile-test
rm -rf ./.bifrost-mobile-cli-test
rm -f /tmp/bifrost-ca.mobileconfig
rm -f /tmp/bifrost-lan-ios.mobileconfig
rm -f /tmp/bifrost-fake-adb-single /tmp/bifrost-fake-adb-single.log /tmp/bifrost-fake-adb-multi
```

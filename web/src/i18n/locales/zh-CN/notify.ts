import type en from "../en/notify";

const zhCN: Record<keyof typeof en, string> = {
  // Notifications page: tabs
  "tabs.all": "全部通知",
  "tabs.tlsTrust": "TLS 信任",
  "tabs.authorization": "授权",
  "tabs.clientTrust": "客户端信任状态",

  // Trust status tags
  "trust.trusted": "已信任",
  "trust.notTrusted": "未信任",
  "trust.possiblyNotTrustedTip": "仅检测到一次明确失败，需要更多数据确认",
  "trust.possiblyNotTrusted": "可能未信任",
  "trust.likelyUntrusted": "很可能未信任（{percent}%）",

  // Notification status
  "status.unread": "未读",
  "status.read": "已读",
  "status.dismissed": "已忽略",

  // Notifications table
  "table.type": "类型",
  "table.domain": "域名",
  "table.title": "标题",
  "table.message": "消息",
  "table.time": "时间",
  "table.passthroughDone": "已直通 ✓",
  "table.ignored": "已忽略",
  "table.passthrough": "直通",
  "table.ignore": "忽略",
  "table.markAsRead": "标记为已读",
  "table.dismiss": "忽略",
  "table.count": "共 {count} 条通知",
  "table.passthroughAll": "全部直通（{count}）",
  "table.ignoreAll": "全部忽略（{count}）",
  "table.markAllRead": "全部标为已读",
  "table.empty": "暂无通知",

  // Client trust table
  "client.identifier": "标识",
  "client.trustStatus": "信任状态",
  "client.success": "成功",
  "client.failUntrust": "失败（未信任）",
  "client.failUntrustTip": "明确：{definite}，疑似：{probable}",
  "client.failOther": "失败（其他）",
  "client.lastFailure": "最近失败",
  "client.lastSeen": "最近活跃",
  "client.count": "共 {count} 个客户端",
  "client.untrustedCount": "{count} 个未信任",
  "client.empty": "暂无客户端信任数据",

  // Availability check panel
  "check.caTrusted": "CA 已信任",
  "check.caInstalled": "CA 已安装",
  "check.caPushed": "CA 已推送",
  "check.caNotInstalled": "CA 未安装",
  "check.caUnknown": "CA 状态未知",
  "check.pageOpened": "页面已打开",
  "check.pageWaiting": "等待打开页面",
  "check.networkOk": "网络正常",
  "check.networkPending": "网络待检测",
  "check.httpsPassed": "浏览器 HTTPS 已通过",
  "check.httpsFailed": "浏览器 HTTPS 失败",
  "check.httpsPending": "浏览器 HTTPS 待检测",
  "check.accessAllowed": "访问已允许",
  "check.accessPending": "访问待授权",
  "check.accessDenied": "访问已拒绝",
  "check.accessUnavailable": "访问不可用",
  "check.proxyConfigured": "代理已配置",
  "check.proxyMissing": "代理未配置",
  "check.proxyPending": "代理待检测",
  "check.deviceConnected": "已连接",
  "check.deviceUnauthorized": "未授权",
  "check.deviceOffline": "离线",
  "check.deviceUnsupported": "不支持",
  "check.selectHostFirst": "请先选择局域网地址。",
  "check.created": "可用性检测链接和二维码已生成。",
  "check.createFailed": "创建可用性检测失败",
  "check.linkCopied": "可用性检测链接已复制。",
  "check.copyManually": "请手动选中并复制链接。",
  "check.compactTitle": "设备无法使用代理时，可进行可用性检测",
  "check.title": "检测手机能否使用 Bifrost",
  "check.compactDesc":
    "在设备上扫描二维码或打开链接。Bifrost 会检查代理授权、探测端口连通性和 HTTPS CA 信任。",
  "check.desc":
    "在目标设备上打开分享链接或扫描二维码。页面会检查代理授权、探测端口连通性，并发起一次由当前 Bifrost CA 签发证书的真实 HTTPS 请求。",
  "check.devicesToCheck": "待检测设备",
  "check.devicesToCheckHint": "扫描可用性检测链接前，已连接的手机会显示在这里。",
  "check.deviceHint":
    "在该设备上打开下方链接或扫描二维码。检测页面打开后，Bifrost 会通过本地设备 ID 跟踪该浏览器。",
  "check.noPhoneTitle": "尚未检测到已连接的手机",
  "check.noPhoneDesc":
    "通过 USB 连接 iPhone 或 Android 设备，或将可用性检测链接分享给同一局域网内的任意手机。打开检测页面的设备会出现在实时状态列表中。",
  "check.localAddress": "局域网地址",
  "check.selectLanIp": "选择局域网 IP",
  "check.regenerate": "重新生成可用性检测",
  "check.generate": "生成可用性检测",
  "check.fingerprint": "CA SHA-256 指纹：",
  "check.qrAlt": "可用性检测二维码",
  "check.openPage": "打开可用性检测页面",
  "check.copyLink": "复制链接",
  "check.connectedDevices": "已连接设备",
  "check.lastSeenLower": "最近活跃 {time}",
  "check.noDeviceYet":
    "还没有设备打开此可用性检测链接。多台设备可以使用同一个固定链接，Bifrost 会通过本地设备 ID 分别跟踪每个浏览器。",
  "check.proxyNotConfiguredTitle": "该设备尚未配置代理",
  "check.proxyManualPrefix": "在 iPhone 上，请将当前 Wi-Fi 的 HTTP 代理手动设置为",
  "check.proxyManualSuffix": "。",
  "check.httpsPassedTitle": "浏览器 HTTPS 探测已通过",
  "check.httpsPassedDesc":
    "这表示该浏览器已使用 Bifrost CA 签发的证书完成专用 HTTPS 探测，但并不代表 Android 已为所有应用安装该 CA；应用仍可能因 Android 用户 CA 策略、证书固定或自定义 TLS 策略而拒绝 Bifrost。",
  "check.proxyLabel": "代理：",
  "check.openProxyQr": "打开代理二维码",
  "check.httpsFailedTitle": "浏览器 HTTPS 探测失败",
  "check.httpsFailedDesc":
    "设备已连通探测端口，但浏览器未能完成 HTTPS 握手。请安装 CA 证书，在 iOS 上开启完全信任，检查设备时间，然后彻底重启浏览器后重试。部分浏览器在重启前会沿用旧的证书信任结果。",
  "check.networkFailedTitle": "探测端口无法访问",
  "check.networkFailedDesc":
    "手机已打开落地页，但无法访问探测端口。请检查防火墙规则、局域网隔离设置以及所选 IP 地址。",
  "check.waitingHint":
    "请在目标设备上扫描二维码。设备打开页面或完成某个检测步骤后，Bifrost 会通过实时连接更新此状态。",
  "check.lastError": "最近错误：{error}",

  // Availability check notification center
  "center.httpsPassed": "浏览器 HTTPS 已通过",
  "center.httpsFailed": "浏览器 HTTPS 失败",
  "center.checkingHttps": "正在检测 HTTPS",
  "center.openedPage": "已打开页面",
  "center.waiting": "等待中",
  "center.title": "可用性检测",
  "center.closeAria": "关闭可用性检测通知",
  "center.desc":
    "有设备打开了可用性检测页面。可在证书页面查看代理访问、浏览器 HTTPS 探测和代理配置情况。",
  "center.lastSeen": "最近活跃 {time}",
  "center.open": "打开可用性检测",
  "center.bubbleAriaOne": "{count} 条可用性检测通知",
  "center.bubbleAriaOther": "{count} 条可用性检测通知",
};

export default zhCN;

const en = {
  // Notifications page: tabs
  "tabs.all": "All Notifications",
  "tabs.tlsTrust": "TLS Trust",
  "tabs.authorization": "Authorization",
  "tabs.clientTrust": "Client Trust Status",

  // Trust status tags
  "trust.trusted": "Trusted",
  "trust.notTrusted": "Not Trusted",
  "trust.possiblyNotTrustedTip": "Only one definitive failure detected — needs more data to confirm",
  "trust.possiblyNotTrusted": "Possibly Not Trusted",
  "trust.likelyUntrusted": "Likely Untrusted ({percent}%)",

  // Notification status
  "status.unread": "Unread",
  "status.read": "Read",
  "status.dismissed": "Dismissed",

  // Notifications table
  "table.type": "Type",
  "table.domain": "Domain",
  "table.title": "Title",
  "table.message": "Message",
  "table.time": "Time",
  "table.passthroughDone": "Passthrough ✓",
  "table.ignored": "Ignored",
  "table.passthrough": "Passthrough",
  "table.ignore": "Ignore",
  "table.markAsRead": "Mark as read",
  "table.dismiss": "Dismiss",
  "table.count": "{count} notification(s)",
  "table.passthroughAll": "Passthrough All ({count})",
  "table.ignoreAll": "Ignore All ({count})",
  "table.markAllRead": "Mark All Read",
  "table.empty": "No notifications",

  // Client trust table
  "client.identifier": "Identifier",
  "client.trustStatus": "Trust Status",
  "client.success": "Success",
  "client.failUntrust": "Fail (Untrust)",
  "client.failUntrustTip": "Definite: {definite}, Probable: {probable}",
  "client.failOther": "Fail (Other)",
  "client.lastFailure": "Last Failure",
  "client.lastSeen": "Last Seen",
  "client.count": "{count} client(s)",
  "client.untrustedCount": "{count} untrusted",
  "client.empty": "No client trust data",

  // Availability check panel
  "check.caTrusted": "CA trusted",
  "check.caInstalled": "CA installed",
  "check.caPushed": "CA pushed",
  "check.caNotInstalled": "CA not installed",
  "check.caUnknown": "CA unknown",
  "check.pageOpened": "Page opened",
  "check.pageWaiting": "Page waiting",
  "check.networkOk": "Network ok",
  "check.networkPending": "Network pending",
  "check.httpsPassed": "Browser HTTPS passed",
  "check.httpsFailed": "Browser HTTPS failed",
  "check.httpsPending": "Browser HTTPS pending",
  "check.accessAllowed": "Access allowed",
  "check.accessPending": "Access pending",
  "check.accessDenied": "Access denied",
  "check.accessUnavailable": "Access unavailable",
  "check.proxyConfigured": "Proxy configured",
  "check.proxyMissing": "Proxy missing",
  "check.proxyPending": "Proxy pending",
  "check.deviceConnected": "connected",
  "check.deviceUnauthorized": "unauthorized",
  "check.deviceOffline": "offline",
  "check.deviceUnsupported": "unsupported",
  "check.selectHostFirst": "Select a local network address first.",
  "check.created": "Availability check link and QR code are ready.",
  "check.createFailed": "Failed to create availability check",
  "check.linkCopied": "Availability check link copied.",
  "check.copyManually": "Select and copy the link manually.",
  "check.compactTitle": "Use Availability Check when this device cannot use the proxy",
  "check.title": "Check whether a phone can use Bifrost",
  "check.compactDesc":
    "Scan the QR code or open the link from the device. Bifrost will check proxy authorization, probe reachability, and HTTPS CA trust.",
  "check.desc":
    "Share the link or scan the QR code from the target device. The page checks proxy authorization, probe port reachability, and a real HTTPS request signed by the current Bifrost CA.",
  "check.devicesToCheck": "Devices to check",
  "check.devicesToCheckHint": "Connected phones are shown here before you scan the availability link.",
  "check.deviceHint":
    "Open the link or scan the QR code below from this device. Bifrost will track the browser with a local device ID after the check page opens.",
  "check.noPhoneTitle": "No connected phone detected yet",
  "check.noPhoneDesc":
    "Connect an iPhone or Android device over USB, or share the availability link with any phone on the same LAN. Devices that open the check page will appear in the live status list.",
  "check.localAddress": "Local network address",
  "check.selectLanIp": "Select a LAN IP",
  "check.regenerate": "Regenerate Availability Check",
  "check.generate": "Generate Availability Check",
  "check.fingerprint": "CA SHA-256 fingerprint: ",
  "check.qrAlt": "Availability Check QR Code",
  "check.openPage": "Open availability check page",
  "check.copyLink": "Copy link",
  "check.connectedDevices": "Connected devices",
  "check.lastSeenLower": "last seen {time}",
  "check.noDeviceYet":
    "No device has opened this availability check link yet. Multiple devices can use the same fixed link; Bifrost will track each browser by a local device ID.",
  "check.proxyNotConfiguredTitle": "Proxy is not configured on this device yet",
  "check.proxyManualPrefix": "On iPhone, manually set the current Wi-Fi HTTP Proxy to",
  "check.proxyManualSuffix": ".",
  "check.httpsPassedTitle": "Browser HTTPS probe passed",
  "check.httpsPassedDesc":
    "This confirms this browser completed the dedicated HTTPS probe with a certificate signed by Bifrost CA. It does not prove Android has installed the CA for every app; apps can still reject Bifrost through Android user CA policy, certificate pinning, or custom TLS policy.",
  "check.proxyLabel": "Proxy:",
  "check.openProxyQr": "Open proxy QR code",
  "check.httpsFailedTitle": "Browser HTTPS probe failed",
  "check.httpsFailedDesc":
    "The device reached the probe port, but the browser could not complete the HTTPS handshake. Install the CA, enable full trust on iOS, check device time, then fully restart the browser and retry. Some browsers keep old certificate trust decisions until restart.",
  "check.networkFailedTitle": "Probe port is not reachable",
  "check.networkFailedDesc":
    "The phone opened the landing page but could not reach the probe port. Check firewall rules, local network isolation, and the selected IP address.",
  "check.waitingHint":
    "Scan the QR code from the target device. Bifrost updates this status over the live connection when the device opens the page or completes a check step.",
  "check.lastError": "Last error: {error}",

  // Availability check notification center
  "center.httpsPassed": "Browser HTTPS passed",
  "center.httpsFailed": "Browser HTTPS failed",
  "center.checkingHttps": "Checking HTTPS",
  "center.openedPage": "Opened page",
  "center.waiting": "Waiting",
  "center.title": "Availability Check",
  "center.closeAria": "Close availability check notifications",
  "center.desc":
    "A device opened the availability check page. Review proxy access, browser HTTPS probe, and proxy configuration from the certificate page.",
  "center.lastSeen": "Last seen {time}",
  "center.open": "Open Availability Check",
  "center.bubbleAriaOne": "{count} availability check notification",
  "center.bubbleAriaOther": "{count} availability check notifications",
};

export default en;

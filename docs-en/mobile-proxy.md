# Connect a Mobile Device to Bifrost on PC, Mac, or Linux

This guide explains how to connect an iPhone, iPad, Android phone, or tablet to a Bifrost proxy running on a PC, Mac, or Linux machine over Wi-Fi.

The shortest path is:

1. Start Bifrost on the computer and make sure the proxy listens on a LAN-reachable address.
2. Connect the computer and mobile device to the same Wi-Fi or otherwise reachable network.
3. Open `Settings -> Certificate -> Availability Check` in the Bifrost Admin UI and generate a link or QR code.
4. Open the check page on the mobile device and resolve access-control and CA-trust results.
5. Set the mobile device's Wi-Fi HTTP proxy to the `<computer LAN IP>:<Bifrost proxy port>` shown by the check page.

## Understand the two addresses

Bifrost's main listener carries both proxy traffic and the local Admin UI. With the default port `9900`:

| Purpose | Example | Where to use it |
| --- | --- | --- |
| Admin UI | `http://127.0.0.1:9900/_bifrost/` | A browser on the computer |
| Mobile proxy | `192.168.1.20:9900` | The phone or tablet Wi-Fi HTTP Proxy setting |
| Optional SOCKS5 proxy | `192.168.1.20:1080` | A client that explicitly supports SOCKS5 |

`127.0.0.1` means the computer itself and cannot be entered as the mobile proxy host. A mobile device must use the computer's actual IPv4 address on the reachable LAN, such as `192.168.1.20` or `10.0.0.8`.

If the computer has multiple interfaces, VPNs, or virtual adapters, do not guess the address. Use Availability Check to select a LAN IP and generate the link; the check page uses the address that the current device can actually reach.

## Start Bifrost on the computer

### CLI on PC, Mac, or Linux

Install and verify the CLI:

```bash
bifrost --version
```

Start the background service. The default listener is `0.0.0.0:9900`:

```bash
bifrost start -d
```

To specify the listener and access-control mode explicitly:

```bash
bifrost -H 0.0.0.0 -p 9900 start -d --access-mode interactive
```

`interactive` is the recommended LAN mode: local loopback is allowed directly, while a mobile device can be approved from the Admin UI on its first access. You can also manage modes, allowlists, and pending clients in `Settings -> Access Control`.

Do not bind the service to `127.0.0.1`; the following example is intentionally local-only and cannot accept a phone connection:

```bash
bifrost -H 127.0.0.1 -p 9900 start -d
```

If a separate SOCKS5 listener is needed, start one in addition to the HTTP proxy:

```bash
bifrost -p 9900 --socks5-port 1080 start -d
```

Mobile Wi-Fi settings usually provide an HTTP proxy field. Prefer the main HTTP proxy port `9900`. Use `1080` only in an app or client that explicitly supports SOCKS5.

### Desktop app on macOS or Windows

Start the Bifrost desktop app. It starts the proxy backend inside the app. Open the Admin UI, go to `Settings -> Certificate -> Availability Check`, and use the proxy address shown there.

The desktop app and CLI share configuration, certificates, and runtime state under `~/.bifrost` by default. If a CLI service already owns the target port, do not start a second service; use the existing service and check its port and access-control state in the Admin UI.

### Linux

Linux currently uses the CLI to start Bifrost:

```bash
bifrost start -d
```

Without a desktop app, you can still open the Admin UI in a browser on the Linux computer and open the Availability Check link on the phone. The mobile workflow is the same as on macOS and Windows as long as the listener, firewall, and network routing permit the connection.

## Make the devices reachable

Before configuring the proxy, confirm:

- The phone and computer are on the same Wi-Fi or another mutually reachable network.
- The phone is not on a guest network, VPN, or mobile-only route that isolates it from the computer.
- The router does not enable AP isolation, client isolation, or a similar wireless-device isolation feature.
- The computer firewall permits LAN inbound connections to the Bifrost proxy port.
- Bifrost is not bound to `127.0.0.1`.

If the computer can reach the internet but the phone cannot open the check link, check network isolation and the firewall before installing a CA.

## Use Availability Check

1. Open the Admin UI on the computer:

   ```text
   http://127.0.0.1:9900/_bifrost/settings?tab=certificate
   ```

   Replace `9900` if Bifrost uses another port.
2. Find `Availability Check` at the top of the Certificate page.
3. Select a LAN IP reachable from the phone and generate a check link or QR code.
4. Open the link with the phone camera or browser.
5. Wait for the page to show network, browser HTTPS, access, and proxy-configuration status.
6. If access is pending, approve the device in the Admin UI or adjust `Settings -> Access Control`.
7. Configure the phone Wi-Fi HTTP proxy with the address shown on the check page.
8. Return to the check page and wait for `Proxy configured` or the equivalent configured state. The Admin UI `Connected devices` list updates live without a manual refresh.

Availability Check is the recommended entry point because it checks:

- Whether the device can reach Bifrost's public check entry and probe port.
- Whether Bifrost access control allows the current device.
- Whether the current mobile browser trusts the Bifrost CA.
- Whether the current Wi-Fi HTTP proxy actually points to Bifrost.

The `<host>:<port>` shown by the check page is for the mobile device. The `http://127.0.0.1:<port>/_bifrost/` URL is for the computer's local browser; do not substitute one for the other.

## Configure the mobile HTTP proxy

### iPhone or iPad

1. Open `Settings -> Wi-Fi`.
2. Tap the information button beside the connected Wi-Fi network.
3. Find `Configure Proxy` and select `Manual`.
4. Enter the computer LAN IP shown by Availability Check as the server.
5. Enter the Bifrost proxy port, usually `9900`.
6. Save, then return to Availability Check and wait for the proxy status to update.

To disable the proxy, set `Configure Proxy` back to `Off`. The check page and Admin UI should return to an unconfigured state on the next check.

### Android phone or tablet

The exact labels vary by vendor. The usual flow is:

1. Open `Settings -> Network & internet` or `Settings -> WLAN/Wi-Fi`.
2. Edit the connected Wi-Fi network.
3. Open advanced settings and find Proxy.
4. Select manual proxy.
5. Enter the computer LAN IP and Bifrost proxy port shown by Availability Check.
6. Save and wait for `Proxy configured` on the check page.

If Android asks for a bypass list, add only hosts that should not use the proxy. Do not bypass the Bifrost proxy address itself.

## HTTPS inspection and CA trust

You do not need a CA for ordinary HTTP traffic. To inspect or modify HTTPS content, the mobile device and the target app must trust the Bifrost CA, and the target request must allow TLS interception.

### iOS

After downloading the Bifrost CA profile from the check page or Certificate page:

1. Allow the configuration profile download on the iPhone or iPad.
2. Open `Settings -> Downloaded Profile` and install the Bifrost CA profile.
3. Complete the installation prompts.
4. Open `Settings -> General -> About -> Certificate Trust Settings`.
5. Enable full trust for the Bifrost CA.
6. Return to Availability Check and wait for the browser HTTPS result to pass.

Installing the profile is not the same as fully trusting it. The `Certificate Trust Settings` switch is required to complete HTTPS trust on iOS. When an iPhone is connected to a Mac, the Certificate page can also use Apple Configurator to deliver the profile, but the phone may still require unlock, Trust, or on-screen confirmation.

### Android

When the Certificate page detects an Android device, you can use Bifrost's device installation flow, or download the CA and install it from the phone's system settings. The exact path depends on the Android version, vendor ROM, and device-management policy.

After a user CA is installed, a browser can usually verify HTTPS trust, but many Android apps do not trust user-installed CAs by default or use certificate pinning. In that case:

- Use the mobile browser with Availability Check to distinguish browser trust failure from an app that cannot be intercepted.
- Exclude certificate-pinning or custom-TLS apps/domains instead of forcing global TLS interception.
- The Bifrost CA only establishes trust between the client and Bifrost. It does not replace the upstream site's certificate or bypass an app's security policy.

## Troubleshooting

### The phone cannot open the check link

Check in this order:

1. Regenerate the link after selecting the computer's LAN IP; do not use `127.0.0.1`.
2. Confirm that the phone and computer are on a mutually reachable network.
3. Temporarily check whether the computer firewall blocks the Bifrost port.
4. Confirm that Bifrost listens on `0.0.0.0` or the computer LAN address, not `127.0.0.1`.
5. Check for AP isolation, guest Wi-Fi, or client isolation on the router.

### The page opens but proxy access is denied

This is an access-control state, not a CA problem. In the computer Admin UI, approve the pending device, add an appropriate IP/CIDR allowlist entry, or choose a suitable mode on a trusted isolated LAN. Do not expose the proxy to the public internet and use `allow_all` just to bypass approval.

### The Admin UI says the proxy is not configured

The phone Wi-Fi proxy setting should contain:

```text
computer LAN IP:Bifrost HTTP proxy port
```

Do not enter `127.0.0.1`, the full Admin UI URL, an `http://` prefix, or the SOCKS5 port. Save the Wi-Fi setting and wait for the next check cycle.

### The browser passes HTTPS but an app has no traffic

Availability Check proves only that the current mobile browser completed Bifrost's check chain. It does not guarantee that every app uses the system HTTP proxy or trusts a user CA. Check whether the app supports proxies, whether it uses certificate pinning, and whether Bifrost has TLS interception enabled for the domain; configure passthrough or exclusions when needed.

### Stop using the mobile proxy

Set the mobile Wi-Fi HTTP Proxy back to `Off`, then revoke temporary approvals or allowlist entries that are no longer needed in the Bifrost Admin UI. Do not leave `allow_all` enabled on an untrusted network.

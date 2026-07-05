# Bifrost Enhanced Proxy for macOS

This app is the macOS host for Bifrost enhanced local capture. It is intentionally separate from the Rust proxy data plane:

- Bifrost continues to listen on the configured local proxy port.
- The host app owns Network Extension configuration and user approval.
- The extension is the only component that can transparently capture traffic from apps that ignore system proxy settings.

Building and running the extension requires Apple signing material with Network Extension and System Extension entitlements. Without those entitlements, Bifrost correctly reports `helper_missing`, `extension_missing`, or `approval_required` instead of claiming capture is active.

## Layout

- `Sources/BifrostEnhancedProxyHost`: command-line host scaffold that configures `NETransparentProxyManager`.
- `Sources/BifrostEnhancedProxyExtension`: `NETransparentProxyProvider` lifecycle entrypoint.
- `Project.yml`: XcodeGen project scaffold for app + system extension packaging.
- `Entitlements`: entitlement templates that must be replaced with a signed team profile before release.

## Local Status Contract

The Rust side looks for:

```text
/Applications/Bifrost Enhanced Proxy.app
/Applications/Bifrost Enhanced Proxy.app/Contents/Library/SystemExtensions/com.bifrost.proxy.enhanced.network-extension.systemextension
<BIFROST_DATA_DIR>/enhanced-proxy.sock
```

Only when the controller socket exists does `bifrost enhanced-proxy status` report `running`.

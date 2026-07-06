import { describe, expect, it } from "vitest";

import { resolveDesktopOpenTarget } from "./openTarget";

const CURRENT_HREF = "http://tauri.localhost/#/settings?tab=cert";
const buildDesktopBackendUrl = (path: string) =>
  `http://127.0.0.1:9900${path.startsWith("/_bifrost/") ? path : `/_bifrost${path}`}`;

describe("resolveDesktopOpenTarget", () => {
  it("keeps same-origin app hash links inside the desktop webview", () => {
    expect(resolveDesktopOpenTarget("#certificate-downloads", CURRENT_HREF)).toBeNull();
    expect(resolveDesktopOpenTarget("/#/traffic/detail?id=REQ-1", CURRENT_HREF)).toBeNull();
    expect(resolveDesktopOpenTarget("/settings", CURRENT_HREF)).toBeNull();
  });

  it("opens backend relative paths through the desktop backend origin", () => {
    expect(
      resolveDesktopOpenTarget("/_bifrost/swagger", CURRENT_HREF, buildDesktopBackendUrl),
    ).toBe("http://127.0.0.1:9900/_bifrost/swagger");
    expect(
      resolveDesktopOpenTarget("/api/system/overview", CURRENT_HREF, buildDesktopBackendUrl),
    ).toBe("http://127.0.0.1:9900/_bifrost/api/system/overview");
    expect(
      resolveDesktopOpenTarget(
        "/public/cert/mobileconfig",
        CURRENT_HREF,
        buildDesktopBackendUrl,
      ),
    ).toBe("http://127.0.0.1:9900/_bifrost/public/cert/mobileconfig");
  });

  it("opens supported external schemes with the native opener", () => {
    expect(resolveDesktopOpenTarget("https://bifrost.example/docs", CURRENT_HREF)).toBe(
      "https://bifrost.example/docs",
    );
    expect(resolveDesktopOpenTarget("mailto:support@example.com", CURRENT_HREF)).toBe(
      "mailto:support@example.com",
    );
    expect(resolveDesktopOpenTarget("macappstore://apps.apple.com/app/id123", CURRENT_HREF)).toBe(
      "macappstore://apps.apple.com/app/id123",
    );
    expect(resolveDesktopOpenTarget("bifrost://open/settings", CURRENT_HREF)).toBe(
      "bifrost://open/settings",
    );
  });

  it("opens supported external schemes when the current page uses a custom protocol", () => {
    const customHref = "tauri://localhost/#/settings";

    expect(resolveDesktopOpenTarget("#certificate-downloads", customHref)).toBeNull();
    expect(resolveDesktopOpenTarget("mailto:support@example.com", customHref)).toBe(
      "mailto:support@example.com",
    );
    expect(resolveDesktopOpenTarget("bifrost://open/settings", customHref)).toBe(
      "bifrost://open/settings",
    );
  });

  it("rejects unsupported schemes", () => {
    expect(resolveDesktopOpenTarget("file:///tmp/rules.bifrost", CURRENT_HREF)).toBeNull();
    expect(resolveDesktopOpenTarget("javascript:alert(1)", CURRENT_HREF)).toBeNull();
  });
});

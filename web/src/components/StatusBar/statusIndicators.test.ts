import { describe, expect, it } from "vitest";
import type { TlsConfig } from "../../api/config";
import { getTlsInterceptionIndicator } from "./statusIndicators";

function tlsConfig(overrides: Partial<TlsConfig> = {}): TlsConfig {
  return {
    enable_tls_interception: false,
    intercept_exclude: [],
    intercept_include: [],
    app_intercept_exclude: [],
    app_intercept_include: [],
    ip_intercept_exclude: [],
    ip_intercept_include: [],
    unsafe_ssl: false,
    disconnect_on_config_change: true,
    ...overrides,
  };
}

describe("getTlsInterceptionIndicator", () => {
  it("marks global HTTPS interception as full active", () => {
    const indicator = getTlsInterceptionIndicator(
      tlsConfig({ enable_tls_interception: true }),
    );

    expect(indicator).toMatchObject({
      active: true,
      text: "Full On",
      state: "full",
    });
    expect(indicator.detail).toContain("globally enabled");
  });

  it("shows scoped when only allow lists can trigger interception", () => {
    const indicator = getTlsInterceptionIndicator(
      tlsConfig({
        intercept_include: ["api.example.test"],
        app_intercept_include: ["Chrome"],
        ip_intercept_include: ["10.0.0.3"],
      }),
    );

    expect(indicator).toMatchObject({
      active: false,
      text: "Scoped",
      state: "limited",
    });
    expect(indicator.detail).toContain("3 domain, app, or IP rules");
  });

  it("uses an unknown state before config loads", () => {
    expect(getTlsInterceptionIndicator(null)).toMatchObject({
      active: false,
      text: "Unknown",
      state: "unknown",
    });
  });
});

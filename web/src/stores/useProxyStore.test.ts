import { describe, expect, it } from "vitest";
import type { SystemProxyStatus } from "../api/proxy";
import { doesSystemProxyMatchRequest } from "./useProxyStore";

const status = (overrides: Partial<SystemProxyStatus>): SystemProxyStatus => ({
  supported: true,
  enabled: false,
  host: "",
  port: 0,
  bypass: "",
  ...overrides,
});

describe("doesSystemProxyMatchRequest", () => {
  it("treats an external proxy left enabled as successful disable", () => {
    expect(
      doesSystemProxyMatchRequest(
        status({
          enabled: true,
          host: "127.0.0.1",
          port: 6152,
          managed_by_bifrost: false,
        }),
        false,
      ),
    ).toBe(true);
  });

  it("treats a Bifrost-managed proxy left enabled as failed disable", () => {
    expect(
      doesSystemProxyMatchRequest(
        status({
          enabled: true,
          host: "127.0.0.1",
          port: 8800,
          managed_by_bifrost: true,
        }),
        false,
      ),
    ).toBe(false);
  });
});

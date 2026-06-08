import { describe, expect, it } from "vitest";
import {
  isClientIpWhitelisted,
  isLoopbackClientIp,
  normalizeClientIp,
} from "./clientIp";

describe("TrafficDetail client IP helpers", () => {
  it("normalizes bracketed and padded IP strings", () => {
    expect(normalizeClientIp(" [::1] ")).toBe("::1");
    expect(normalizeClientIp(" 192.168.1.20 ")).toBe("192.168.1.20");
  });

  it("treats loopback addresses as local clients", () => {
    expect(isLoopbackClientIp("127.0.0.1")).toBe(true);
    expect(isLoopbackClientIp("127.12.34.56")).toBe(true);
    expect(isLoopbackClientIp("::1")).toBe(true);
    expect(isLoopbackClientIp("[::1]")).toBe(true);
    expect(isLoopbackClientIp("::ffff:127.0.0.1")).toBe(true);
  });

  it("treats LAN addresses as non-local clients", () => {
    expect(isLoopbackClientIp("192.168.1.20")).toBe(false);
    expect(isLoopbackClientIp("10.0.0.8")).toBe(false);
    expect(isLoopbackClientIp("172.16.0.8")).toBe(false);
  });

  it("checks exact whitelist entries case-insensitively", () => {
    expect(isClientIpWhitelisted("192.168.1.20", ["192.168.1.20"])).toBe(true);
    expect(isClientIpWhitelisted("[::1]", ["::1"])).toBe(true);
    expect(isClientIpWhitelisted("192.168.1.20", ["192.168.1.20/32"])).toBe(true);
    expect(isClientIpWhitelisted("2001:db8::1", ["2001:db8::1/128"])).toBe(true);
    expect(isClientIpWhitelisted("192.168.1.20", ["192.168.1.0/24"])).toBe(false);
  });
});

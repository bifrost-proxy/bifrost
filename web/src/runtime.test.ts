import { describe, expect, it } from "vitest";
import { normalizeDesktopPlatform } from "./runtime";

describe("desktop runtime platform normalization", () => {
  it("accepts Rust and JavaScript macOS platform names", () => {
    expect(normalizeDesktopPlatform("macos")).toBe("macos");
    expect(normalizeDesktopPlatform("darwin")).toBe("macos");
  });

  it("accepts Rust and JavaScript Windows platform names", () => {
    expect(normalizeDesktopPlatform("windows")).toBe("windows");
    expect(normalizeDesktopPlatform("win32")).toBe("windows");
  });

  it("falls back to web for unknown platform names", () => {
    expect(normalizeDesktopPlatform("freebsd")).toBe("web");
  });
});

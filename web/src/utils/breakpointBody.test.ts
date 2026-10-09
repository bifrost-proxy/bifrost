import { describe, expect, it } from "vitest";
import { breakpointBodyBytes, convertBreakpointBody } from "./breakpointBody";
describe("breakpoint body editing", () => {
  it("round trips UTF-8 bytes including non ASCII and null", () => {
    const text = "你好\u0000🧪";
    const encoded = convertBreakpointBody(text, "utf8", "base64");
    expect(convertBreakpointBody(encoded, "base64", "utf8")).toBe(text);
    expect(breakpointBodyBytes(text, "utf8").length).toBe(11);
  });
  it("keeps arbitrary binary lossless and refuses lossy text conversion", () => {
    expect([...breakpointBodyBytes("AP/+", "base64")]).toEqual([0, 255, 254]);
    expect(() => convertBreakpointBody("AP/+", "base64", "utf8")).toThrow();
  });
  it.each(["a", "a===", "YQ", "YR==", "Y Q==", "!!!!"])(
    "rejects malformed or noncanonical Base64 %s",
    (value) => {
      expect(() => breakpointBodyBytes(value, "base64")).toThrow();
    },
  );
});

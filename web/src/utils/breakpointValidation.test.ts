import { describe, expect, it } from "vitest";
import { isFinalBreakpointStatus } from "./breakpointValidation";

describe("breakpoint final response status", () => {
  it.each([100, 101, 199, 600, 999, 200.5, NaN, undefined])(
    "rejects informational, invalid, or missing final status %s",
    (status) => expect(isFinalBreakpointStatus(status)).toBe(false),
  );
  it.each([200, 204, 304, 404, 599])("accepts final status %s", (status) =>
    expect(isFinalBreakpointStatus(status)).toBe(true),
  );
});

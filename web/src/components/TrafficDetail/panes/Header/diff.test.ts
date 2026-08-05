import { describe, expect, it } from "vitest";
import { areHeadersEqual, buildHeaderDiff } from "./diff";

describe("response header diff provenance", () => {
  it("classifies Connection removal as protocol handling", () => {
    const result = buildHeaderDiff(
      [["content-type", "application/json"]],
      [
        ["content-type", "application/json"],
        ["connection", "keep-alive"],
      ],
    );

    expect(result.summary).toEqual({ configured: 0, protocol: 1 });
    expect(
      result.items.find((item) => item.name === "connection"),
    ).toMatchObject({
      diffType: "deleted",
      changeSource: "protocol",
    });
  });

  it("classifies headers named by Connection tokens as protocol handling", () => {
    const result = buildHeaderDiff(
      [],
      [
        ["Connection", "keep-alive, X-Hop"],
        ["X-Hop", "remove-me"],
      ],
    );

    expect(result.summary).toEqual({ configured: 0, protocol: 2 });
    expect(result.items.every((item) => item.changeSource === "protocol")).toBe(
      true,
    );
  });

  it("keeps ordinary additions, modifications, and deletions as configured changes", () => {
    const result = buildHeaderDiff(
      [
        ["x-added", "new"],
        ["x-modified", "after"],
      ],
      [
        ["x-deleted", "old"],
        ["x-modified", "before"],
      ],
    );

    expect(result.summary).toEqual({ configured: 3, protocol: 0 });
    expect(
      result.items.filter((item) => item.changeSource === "configured"),
    ).toHaveLength(3);
  });

  it("does not label representation metadata as protocol handling", () => {
    const result = buildHeaderDiff(
      [["content-length", "12"]],
      [["content-length", "18"]],
    );

    expect(result.summary).toEqual({ configured: 1, protocol: 0 });
    expect(result.items[0]).toMatchObject({
      diffType: "modified",
      changeSource: "configured",
    });
  });

  it("supports an empty delivered header set", () => {
    const result = buildHeaderDiff([], [["Connection", "close"]]);
    expect(result.items).toHaveLength(1);
    expect(result.summary.protocol).toBe(1);
    expect(areHeadersEqual([], [["Connection", "close"]])).toBe(false);
  });

  it("does not report a change for header order or name casing", () => {
    expect(
      areHeadersEqual(
        [
          ["X-Test", "value"],
          ["content-type", "application/json"],
        ],
        [
          ["Content-Type", "application/json"],
          ["x-test", "value"],
        ],
      ),
    ).toBe(true);
  });
});

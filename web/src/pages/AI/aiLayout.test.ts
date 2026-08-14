import { describe, expect, it } from "vitest";
import {
  resolveLegacyAiDestination,
} from "./aiLayout";

function params(query = "") {
  return new URLSearchParams(query);
}

describe("AI module routing", () => {
  it("maps legacy feature links to module detail pages", () => {
    expect(resolveLegacyAiDestination(params("view=asr"))).toBe("/ai/asr");
    expect(
      resolveLegacyAiDestination(params("aiSection=im-gateway-routes")),
    ).toBe("/ai/channels");
    expect(resolveLegacyAiDestination(params("settings=agent"))).toBe(
      "/ai/agents",
    );
  });

  it("maps removed chat and session details to summary-only runs", () => {
    expect(resolveLegacyAiDestination(params("view=chat&mode=new"))).toBe(
      "/ai/runs",
    );
    expect(resolveLegacyAiDestination(params("session=admin-chat-1"))).toBe(
      "/ai/runs",
    );
    expect(
      resolveLegacyAiDestination(params("historyPath=%2Ftmp%2Fsecret.jsonl")),
    ).toBe("/ai/runs");
  });

  it("leaves a clean AI home URL on the hub", () => {
    expect(resolveLegacyAiDestination(params())).toBeNull();
  });
});

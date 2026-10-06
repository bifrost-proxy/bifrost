import { describe, expect, it } from "vitest";
import {
  resolveLegacyAiDestination,
  resolveLegacyAiRedirect,
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

describe("legacy AI redirects", () => {
  it.each(["view=asr", "aiSection=tools-asr"])(
    "preserves ASR deep-link state for %s",
    (entry) => {
      const asrState = {
        asrTab: "voice",
        asrTask: "meeting task/1",
        asrTaskTab: "daily-agent-records",
        asrFile: "meeting/audio.wav",
        asrDay: "2026-05-14",
        asrDailyReport: "2026-05-14",
        asrDailyAgent: "daily_report",
        asrDailyAgentEdit: "custom_agent_2",
      };
      const query = params(entry);
      for (const [key, value] of Object.entries(asrState))
        query.set(key, value);
      query.set("historyPath", "/tmp/private-history.jsonl");
      query.set("session", "unrelated-session");
      query.set("settings", "agent");
      const redirect = resolveLegacyAiRedirect(query);
      expect(redirect).not.toBeNull();
      const url = new URL(redirect!, "http://localhost");
      expect(url.pathname).toBe("/ai/asr");
      expect(Object.fromEntries(url.searchParams)).toEqual(asrState);
    },
  );

  it("keeps absent ASR state absent", () => {
    expect(resolveLegacyAiRedirect(params("view=asr"))).toBe("/ai/asr");
  });

  it("keeps removed chat details out of the summary-only run URL", () => {
    expect(
      resolveLegacyAiRedirect(
        params(
          "view=chat&session=room%2Fone&historyPath=%2Ftmp%2Fsecret.jsonl&asrTask=other",
        ),
      ),
    ).toBe("/ai/runs?q=room%2Fone");
    expect(
      resolveLegacyAiRedirect(params("historyPath=%2Ftmp%2Fsecret.jsonl")),
    ).toBe("/ai/runs");
  });

  it("does not forward ASR state to unrelated modules", () => {
    expect(resolveLegacyAiRedirect(params("view=im&asrTab=voice"))).toBe(
      "/ai/channels",
    );
    expect(
      resolveLegacyAiRedirect(params("settings=agent&asrTask=task-1")),
    ).toBe("/ai/agents");
    expect(resolveLegacyAiRedirect(params())).toBeNull();
  });
});

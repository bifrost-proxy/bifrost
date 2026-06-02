import { describe, expect, it } from "vitest";
import { historyEventsToMessages, historyEventsToTelemetry } from "./AgentChatSection.timeline";
import {
  formatContextWindow,
  formatStatusMetricCount,
  type AgentThreadSummary,
  type HistoryEvent,
  type RunTelemetry,
} from "./AgentChatSection.helpers";

describe("historyEventsToTelemetry", () => {
  it("clears stale completed plans when a later turn records plan_cleared", () => {
    const events: HistoryEvent[] = [
      {
        timestamp: 1,
        event_type: "plan_updated",
        session_key: "plan-clear",
        content: {
          plan: [
            { step: "Inspect", status: "completed" },
            { step: "Ship", status: "completed" },
          ],
        },
      },
      {
        timestamp: 2,
        event_type: "plan_cleared",
        session_key: "plan-clear",
        content: { reason: "new_turn_after_completion" },
      },
      {
        timestamp: 3,
        event_type: "user_message",
        session_key: "plan-clear",
        content: { message: "next turn" },
      },
    ];

    const telemetry = historyEventsToTelemetry(events);

    expect(telemetry.plan).toEqual([]);
  });

  it("keeps running thread token and context metrics newer than stale history events", () => {
    const thread: AgentThreadSummary = {
      session_key: "feishu-main:user",
      status: "active",
      running: true,
      run_state: "running",
      state: "waiting_on_session",
      tokens: 29_668_709,
      estimated_tokens: 186_727,
      context_window_tokens: 250_000,
      context_usage_percent: 74.7,
      last_response_tokens: 181_900,
      compaction_count: 2,
    };
    const events: HistoryEvent[] = [
      {
        timestamp: 1,
        event_type: "compaction",
        session_key: "feishu-main:user",
        content: {
          total_tokens: 19_503_264,
          post_tokens: 22_077,
          compaction_count: 1,
        },
      },
    ];

    const telemetry = historyEventsToTelemetry(events, thread);

    expect(telemetry.status?.total_tokens_used).toBe(29_668_709);
    expect(telemetry.status?.estimated_context_tokens).toBe(186_727);
    expect(telemetry.status?.context_window_tokens).toBe(250_000);
    expect(telemetry.status?.context_usage_percent).toBe(74.7);
    expect(telemetry.status?.last_response_tokens).toBe(181_900);
    expect(telemetry.status?.compaction_count).toBe(2);
  });

  it("lets explicit idle thread status override stale running history state", () => {
    const thread: AgentThreadSummary = {
      session_key: "feishu-main:user",
      status: "active",
      running: false,
      run_state: "running",
      state: "idle",
      tokens: 29_668_709,
      estimated_tokens: 186_727,
    };
    const events: HistoryEvent[] = [
      {
        timestamp: 1,
        event_type: "run_state_changed",
        session_key: "feishu-main:user",
        content: { state: "running" },
      },
    ];

    const telemetry = historyEventsToTelemetry(events, thread);

    expect(telemetry.phase).toBe("idle");
    expect(telemetry.status?.state).toBe("idle");
  });

  it("keeps active detail status newer than stale history events when thread summary is missing", () => {
    const fallback: RunTelemetry = {
      phase: "running",
      status: {
        state: "model_response",
        total_tokens_used: 29_668_709,
        estimated_context_tokens: 186_727,
        context_window_tokens: 250_000,
        context_usage_percent: 74.7,
        last_response_tokens: 181_900,
        compaction_count: 2,
      },
      plan: [],
      tools: [],
      errors: [],
    };
    const events: HistoryEvent[] = [
      {
        timestamp: 1,
        event_type: "assistant_message",
        session_key: "feishu-main:user",
        content: {
          total_tokens: 19_503_264,
          context_tokens: 22_077,
        },
      },
    ];

    const telemetry = historyEventsToTelemetry(events, undefined, fallback);

    expect(telemetry.status?.total_tokens_used).toBe(29_668_709);
    expect(telemetry.status?.estimated_context_tokens).toBe(186_727);
    expect(telemetry.status?.context_window_tokens).toBe(250_000);
    expect(telemetry.status?.context_usage_percent).toBe(74.7);
    expect(telemetry.status?.last_response_tokens).toBe(181_900);
  });
});

describe("historyEventsToMessages", () => {
  it("renders persisted Plan Mode proposed plans as assistant results", () => {
    const messages = historyEventsToMessages([
      {
        timestamp: 1,
        event_type: "user_message",
        session_key: "plan-mode",
        content: { message: "Plan the migration" },
      },
      {
        timestamp: 2,
        event_type: "assistant_message",
        session_key: "plan-mode",
        content: { message: "" },
      },
      {
        timestamp: 3,
        event_type: "proposed_plan",
        session_key: "plan-mode",
        content: { content: "- Inspect\n- Implement\n- Verify" },
      },
    ]);

    expect(messages).toHaveLength(2);
    expect(messages[1]).toMatchObject({
      role: "assistant",
      meta: "Plan Mode",
    });
    expect(messages[1].content).toContain("Plan Mode result");
    expect(messages[1].content).toContain("- Verify");
  });
});

describe("agent token metric formatting", () => {
  it("matches IM status compact token units", () => {
    expect(formatStatusMetricCount(999)).toBe("999");
    expect(formatStatusMetricCount(1_234)).toBe("1.2K");
    expect(formatStatusMetricCount(181_900)).toBe("181.9K");
    expect(formatStatusMetricCount(19_503_264)).toBe("19.5M");
    expect(formatStatusMetricCount(29_668_709)).toBe("29.7M");
    expect(
      formatContextWindow(
        { estimated_context_tokens: 186_727, context_window_tokens: 250_000 },
      ),
    ).toBe("186.7K / 250K");
  });
});

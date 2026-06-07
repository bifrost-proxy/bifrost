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

  it("deduplicates repeated external runner tool events by call id", () => {
    const messages = historyEventsToMessages([
      {
        timestamp: 0,
        event_type: "session_start",
        session_key: "trae-streaming",
        content: { runtime: "external_cli", adapter: "traex" },
      },
      {
        timestamp: 1,
        event_type: "user_message",
        session_key: "trae-streaming",
        content: { message: "review this branch" },
      },
      {
        timestamp: 2,
        event_type: "assistant_delta",
        session_key: "trae-streaming",
        content: { message: "我先看 diff。" },
      },
      {
        timestamp: 3,
        event_type: "tool_call",
        session_key: "trae-streaming",
        content: {
          tool_name: "exec_command",
          call_id: "item_1",
          arguments: JSON.stringify({ command: "git diff --stat main..HEAD" }),
        },
      },
      {
        timestamp: 3,
        event_type: "tool_call",
        session_key: "trae-streaming",
        content: {
          tool_name: "exec_command",
          call_id: "item_1",
          arguments: JSON.stringify({ command: "git diff --stat main..HEAD" }),
        },
      },
      {
        timestamp: 4,
        event_type: "tool_result",
        session_key: "trae-streaming",
        content: {
          tool_name: "exec_command",
          call_id: "item_1",
          result: "3 files changed",
          success: true,
        },
      },
    ]);

    const assistant = messages.find((message) => message.role === "assistant");
    const steps = assistant?.processSteps || [];
    expect(steps).toHaveLength(2);
    expect(steps[0]).toMatchObject({
      type: "thinking",
      summary: "我先看 diff。",
    });
    expect(steps[1]).toMatchObject({
      type: "tool",
      summary: "exec_command",
      callId: "item_1",
      args: JSON.stringify({ command: "git diff --stat main..HEAD" }),
      result: "3 files changed",
      status: "success",
    });
  });

  it("places external runner content before trailing running tools in the same loop", () => {
    const messages = historyEventsToMessages([
      {
        timestamp: 0,
        event_type: "session_start",
        session_key: "trae-streaming-order",
        content: { runtime: "external_cli", adapter: "traex" },
      },
      {
        timestamp: 1,
        event_type: "user_message",
        session_key: "trae-streaming-order",
        content: { message: "review" },
      },
      {
        timestamp: 2,
        event_type: "tool_call",
        session_key: "trae-streaming-order",
        content: {
          tool_name: "exec_command",
          call_id: "item_1",
          arguments: JSON.stringify({ command: "git diff --stat" }),
        },
      },
      {
        timestamp: 3,
        event_type: "assistant_delta",
        session_key: "trae-streaming-order",
        content: { message: "Let me inspect the diff first." },
      },
      {
        timestamp: 4,
        event_type: "tool_result",
        session_key: "trae-streaming-order",
        content: {
          tool_name: "exec_command",
          call_id: "item_1",
          result: "ok",
          success: true,
        },
      },
    ]);

    const steps = messages.find((message) => message.role === "assistant")?.processSteps || [];
    expect(steps.map((step) => step.type)).toEqual(["thinking", "tool"]);
    expect(steps[0].summary).toBe("Let me inspect the diff first.");
    expect(steps[1]).toMatchObject({ callId: "item_1", status: "success" });
  });

  it("keeps non external runner assistant deltas as assistant content", () => {
    const messages = historyEventsToMessages([
      {
        timestamp: 0,
        event_type: "session_start",
        session_key: "chatgpt-web-history",
        content: { adapter: "chatgpt_web" },
      },
      {
        timestamp: 1,
        event_type: "user_message",
        session_key: "chatgpt-web-history",
        content: { message: "hello" },
      },
      {
        timestamp: 2,
        event_type: "assistant_delta",
        session_key: "chatgpt-web-history",
        content: { message: "normal streamed answer" },
      },
    ]);

    const assistant = messages.find((message) => message.role === "assistant");
    expect(assistant?.content).toBe("normal streamed answer");
    expect(assistant?.processSteps || []).toHaveLength(0);
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

import { describe, expect, it } from "vitest";
import {
  appendProcessStepToTimeline,
  historyEventsToMessages,
  historyEventsToTelemetry,
} from "./AgentChatSection.timeline";
import { isThreadActive } from "./AgentChatSection.timelinePolling";
import {
  buildProcessLogItems,
  formatCommandGroupSummary,
  formatContextWindow,
  formatModelRef,
  formatReasoningRef,
  formatStatusMetricCount,
  telemetryFromThread,
  type AgentThreadSummary,
  type HistoryEvent,
  type RunTelemetry,
} from "./AgentChatSection.helpers";

describe("external runner process rendering", () => {
  it("coalesces adjacent assistant deltas without crossing tool boundaries", () => {
    const first = appendProcessStepToTimeline([], {
      type: "thinking",
      summary: "你",
      status: "success",
      startedAt: 1,
    });
    const second = appendProcessStepToTimeline(first, {
      type: "thinking",
      summary: "说得对。",
      status: "success",
      startedAt: 2,
    });
    const withTool = appendProcessStepToTimeline(second, {
      type: "tool",
      summary: "exec_command",
      status: "success",
    });
    const afterTool = appendProcessStepToTimeline(withTool, {
      type: "thinking",
      summary: "继续",
      status: "success",
      startedAt: 3,
    });

    expect(afterTool).toEqual([
      {
        type: "thinking",
        summary: "你说得对。",
        status: "success",
        startedAt: 1,
      },
      { type: "tool", summary: "exec_command", status: "success" },
      {
        type: "thinking",
        summary: "继续",
        status: "success",
        startedAt: 3,
      },
    ]);
  });

  it("replaces token deltas with a cumulative runner snapshot without duplicating it", () => {
    const tokenized = [..."你说得对。这个问题需要修复。"].reduce(
      (steps, summary, index) =>
        appendProcessStepToTimeline(steps, {
          type: "thinking",
          summary,
          status: "success",
          startedAt: index,
        }),
      [] as ReturnType<typeof appendProcessStepToTimeline>,
    );
    const withSnapshot = appendProcessStepToTimeline(tokenized, {
      type: "thinking",
      summary: "你说得对。这个问题需要修复。",
      status: "success",
      startedAt: 30,
    });
    const repeatedShortToken = appendProcessStepToTimeline([], {
      type: "thinking",
      summary: "哈",
      status: "success",
    });

    expect(withSnapshot).toHaveLength(1);
    expect(withSnapshot[0]?.summary).toBe("你说得对。这个问题需要修复。");
    expect(
      appendProcessStepToTimeline(repeatedShortToken, {
        type: "thinking",
        summary: "哈",
        status: "success",
      })[0]?.summary,
    ).toBe("哈哈");
  });

  it("renders streamed reasoning as paragraphs and hides usage refresh events", () => {
    const messages = historyEventsToMessages([
      {
        timestamp: 1,
        event_type: "session_start",
        session_key: "stream-coalescing",
        content: { runtime: "external_cli", adapter: "codex" },
      },
      {
        timestamp: 2,
        event_type: "user_message",
        session_key: "stream-coalescing",
        content: { message: "请修复" },
      },
      ...["你", "说", "得", "对", "。"].map((message, index) => ({
        timestamp: 3 + index,
        event_type: "assistant_delta",
        session_key: "stream-coalescing",
        content: { message },
      })),
      {
        timestamp: 8,
        event_type: "assistant_delta",
        session_key: "stream-coalescing",
        content: { message: "token_usage: token usage updated" },
      },
      {
        timestamp: 9,
        event_type: "assistant_delta",
        session_key: "stream-coalescing",
        content: { message: "rate_limits: usage updated" },
      },
      {
        timestamp: 10,
        event_type: "tool_call",
        session_key: "stream-coalescing",
        content: { tool_name: "exec_command", call_id: "tool-1" },
      },
      {
        timestamp: 11,
        event_type: "tool_result",
        session_key: "stream-coalescing",
        content: {
          tool_name: "exec_command",
          call_id: "tool-1",
          result: "ok",
          success: true,
        },
      },
      ...["继续", "完成"].map((message, index) => ({
        timestamp: 12 + index,
        event_type: "assistant_delta",
        session_key: "stream-coalescing",
        content: { message },
      })),
      {
        timestamp: 14,
        event_type: "assistant_message",
        session_key: "stream-coalescing",
        content: { message: "最终结果" },
      },
    ]);

    expect(messages).toHaveLength(2);
    expect(messages[1].content).toBe("最终结果");
    expect(messages[1].processSteps?.map((step) => step.summary)).toEqual([
      "你说得对。",
      "exec_command",
      "继续完成",
    ]);
    expect(JSON.stringify(messages)).not.toContain("token_usage");
    expect(JSON.stringify(messages)).not.toContain("rate_limits");
  });

  it("removes a coalesced delta paragraph when it duplicates the final answer", () => {
    const messages = historyEventsToMessages([
      {
        timestamp: 1,
        event_type: "session_start",
        session_key: "stream-final-dedupe",
        content: { runtime: "external_cli", adapter: "codex" },
      },
      {
        timestamp: 2,
        event_type: "user_message",
        session_key: "stream-final-dedupe",
        content: { message: "继续" },
      },
      ...["修复", "已经", "完成", "。"].map((message, index) => ({
        timestamp: 3 + index,
        event_type: "assistant_delta",
        session_key: "stream-final-dedupe",
        content: { message },
      })),
      {
        timestamp: 7,
        event_type: "assistant_message",
        session_key: "stream-final-dedupe",
        content: { message: "修复已经完成。" },
      },
    ]);

    expect(messages).toHaveLength(2);
    expect(messages[1]).toMatchObject({
      role: "assistant",
      content: "修复已经完成。",
    });
    expect(messages[1].processSteps).toBeUndefined();
  });
});

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

  it("restores external runner plan updates from persisted history", () => {
    const events: HistoryEvent[] = [
      {
        timestamp: 1,
        event_type: "session_start",
        session_key: "external-plan",
        content: { source: "admin-api", runtime: "external_cli", adapter: "codex" },
      },
      {
        timestamp: 2,
        event_type: "user_message",
        session_key: "external-plan",
        content: { message: "run task" },
      },
      {
        timestamp: 3,
        event_type: "plan_updated",
        session_key: "external-plan",
        content: {
          plan: [
            { step: "inspect output", status: "completed" },
            { step: "map parser", status: "pending" },
          ],
        },
      },
      {
        timestamp: 4,
        event_type: "assistant_message",
        session_key: "external-plan",
        content: { message: "done" },
      },
    ];

    const telemetry = historyEventsToTelemetry(events);
    const messages = historyEventsToMessages(events);

    expect(telemetry.plan).toEqual([
      { step: "inspect output", status: "completed" },
      { step: "map parser", status: "pending" },
    ]);
    expect(messages[messages.length - 1].processSteps).toEqual([
      {
        type: "plan",
        summary: "Plan updated (2 steps)",
        status: "success",
      },
    ]);
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

  it("lets explicit idle detail run_state override stale running history state", () => {
    const thread: AgentThreadSummary = {
      session_key: "admin-chat",
      status: "active",
      run_state: "idle",
    };
    const events: HistoryEvent[] = [
      {
        timestamp: 1,
        event_type: "run_state_changed",
        session_key: "admin-chat",
        content: { state: "running" },
      },
    ];

    const telemetry = historyEventsToTelemetry(events, thread);

    expect(telemetry.phase).toBe("idle");
    expect(telemetry.status?.state).toBe("idle");
  });

  it("does not treat running false with idle state as active when run_state is stale", () => {
    const thread: AgentThreadSummary = {
      session_key: "admin-chat",
      status: "active",
      running: false,
      run_state: "running",
      state: "idle",
    };

    expect(isThreadActive(thread)).toBe(false);
  });

  it("lets running false override stale active run_state", () => {
    const thread: AgentThreadSummary = {
      session_key: "admin-chat",
      status: "active",
      running: false,
      run_state: "running",
    };
    const events: HistoryEvent[] = [
      {
        timestamp: 1,
        event_type: "run_state_changed",
        session_key: "admin-chat",
        content: { state: "running" },
      },
    ];

    const telemetry = historyEventsToTelemetry(events, thread);

    expect(isThreadActive(thread)).toBe(false);
    expect(telemetry.phase).toBe("idle");
    expect(telemetry.status?.state).toBe("idle");
    expect(telemetryFromThread(thread).status?.state).toBe("idle");
  });

  it("lets explicit terminal run_state override stale running true", () => {
    const thread: AgentThreadSummary = {
      session_key: "admin-chat",
      status: "active",
      running: true,
      run_state: "completed",
      state: "completed",
    };

    expect(isThreadActive(thread)).toBe(false);
  });

  it("treats detailed in-progress run_state values as active", () => {
    const activeStates = [
      "waiting_on_session",
      "model_response",
      "tool_running",
      "compacting",
      "stopping",
    ];

    for (const state of activeStates) {
      expect(
        isThreadActive({
          session_key: `admin-chat-${state}`,
          status: "active",
          run_state: state,
        }),
      ).toBe(true);
    }
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

  it("does not append a running placeholder when detail run_state is explicit idle", () => {
    const messages = historyEventsToMessages(
      [
        {
          timestamp: 1,
          event_type: "run_state_changed",
          session_key: "admin-chat",
          content: { state: "running" },
        },
        {
          timestamp: 2,
          event_type: "user_message",
          session_key: "admin-chat",
          content: { message: "queued-looking stale message" },
        },
      ],
      {
        ensureRunningAssistant: false,
        runningState: "idle",
      },
    );

    expect(messages).toHaveLength(1);
    expect(messages[0]).toMatchObject({
      role: "user",
      content: "queued-looking stale message",
    });
    expect(messages.some((message) => message.content === "Agent is running...")).toBe(false);
  });
});

describe("historyEventsToMessages", () => {
  it("restores user message images from persisted timeline events", () => {
    const messages = historyEventsToMessages([
      {
        timestamp: 1,
        event_type: "user_message",
        session_key: "image-text",
        content: {
          message: "图片里面说的啥？",
          images: [
            {
              mime_type: "image/png",
              data: "aGVsbG8=",
            },
          ],
        },
      },
      {
        timestamp: 2,
        event_type: "assistant_message",
        session_key: "image-text",
        content: { message: "图片里是 hello。" },
      },
    ]);

    expect(messages[0]).toMatchObject({
      role: "user",
      content: "图片里面说的啥？",
    });
    expect(messages[0].contentParts).toEqual([
      { type: "text", text: "图片里面说的啥？" },
      {
        type: "image_url",
        image_url: { url: "data:image/png;base64,aGVsbG8=", detail: "auto" },
      },
    ]);
  });

  it("keeps image-only user timeline messages visible", () => {
    const messages = historyEventsToMessages([
      {
        timestamp: 1,
        event_type: "user_message",
        session_key: "image-only",
        content: {
          images: [
            {
              mimeType: "image/jpeg",
              data: "aW1hZ2U=",
            },
          ],
        },
      },
    ]);

    expect(messages).toHaveLength(1);
    expect(messages[0]).toMatchObject({
      role: "user",
      content: "Attached image",
    });
    expect(messages[0].contentParts?.[0]).toEqual({
      type: "image_url",
      image_url: { url: "data:image/jpeg;base64,aW1hZ2U=", detail: "auto" },
    });
  });

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

  it("attaches external runner process steps to the final assistant message", () => {
    const messages = historyEventsToMessages([
      {
        timestamp: 0,
        event_type: "session_start",
        session_key: "feishu-main:ou_user",
        content: { runtime: "external_cli", adapter: "traex" },
      },
      {
        timestamp: 1,
        event_type: "user_message",
        session_key: "feishu-main:ou_user",
        content: { message: "当前分支有什么修改？" },
      },
      {
        timestamp: 2,
        event_type: "assistant_delta",
        session_key: "feishu-main:ou_user",
        content: { message: "我先看一下分支状态。" },
      },
      {
        timestamp: 3,
        event_type: "tool_call",
        session_key: "feishu-main:ou_user",
        content: {
          tool_name: "exec_command",
          call_id: "tool_1",
          arguments: JSON.stringify({ command: "git status --short --branch" }),
        },
      },
      {
        timestamp: 4,
        event_type: "tool_result",
        session_key: "feishu-main:ou_user",
        content: {
          tool_name: "exec_command",
          call_id: "tool_1",
          result: "## codex/fix-traex-feishu-progress",
          success: true,
        },
      },
      {
        timestamp: 5,
        event_type: "run_state_changed",
        session_key: "feishu-main:ou_user",
        content: { state: "completed" },
      },
      {
        timestamp: 6,
        event_type: "assistant_message",
        session_key: "feishu-main:ou_user",
        content: { message: "当前分支没有未提交修改。" },
      },
    ]);

    expect(messages).toHaveLength(2);
    expect(messages[1]).toMatchObject({
      role: "assistant",
      content: "当前分支没有未提交修改。",
    });
    expect(messages[1].content).not.toBe("Agent is running...");
    expect(messages[1].processSteps).toHaveLength(2);
    expect(messages[1].processSteps?.[0]).toMatchObject({
      type: "thinking",
      summary: "我先看一下分支状态。",
    });
    expect(messages[1].processSteps?.[1]).toMatchObject({
      type: "tool",
      summary: "exec_command",
      callId: "tool_1",
      status: "success",
      result: "## codex/fix-traex-feishu-progress",
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

  it("formats active runner model and reasoning details for the status panel", () => {
    expect(
      formatModelRef({
        model: "gpt-5.1-codex",
        model_provider: "codex config",
      }),
    ).toBe("gpt-5.1-codex (codex config)");
    expect(
      formatReasoningRef({
        modelReasoningEffort: "high",
        modelReasoningSummary: "auto",
      }),
    ).toBe("high / auto");
    expect(formatModelRef({})).toBe("-");
    expect(formatReasoningRef({})).toBe("-");
  });
});

describe("process log display helpers", () => {
  it("groups adjacent tool steps into compact command rows", () => {
    const items = buildProcessLogItems([
      { type: "thinking", summary: "I will inspect the route.", status: "success" },
      {
        type: "tool",
        summary: "exec_command",
        status: "success",
        durationMs: 1000,
      },
      {
        type: "tool",
        summary: "exec_command",
        status: "failed",
        durationMs: 2000,
      },
      { type: "thinking", summary: "Now I will patch it.", status: "success" },
    ]);

    expect(items).toHaveLength(3);
    expect(items[0]).toMatchObject({ type: "text", index: 0 });
    expect(items[1]).toMatchObject({ type: "commands", startIndex: 1 });
    expect(items[1].type === "commands" ? items[1].steps : []).toHaveLength(2);
    expect(items[2]).toMatchObject({ type: "text", index: 3 });
  });

  it("formats command group status from the grouped command states", () => {
    expect(
      formatCommandGroupSummary(
        [
          { type: "tool", summary: "exec_command", status: "success", durationMs: 1000 },
          { type: "tool", summary: "exec_command", status: "success", durationMs: 2000 },
        ],
        10,
      ),
    ).toBe("已运行 2 条命令");
    expect(
      formatCommandGroupSummary(
        [
          { type: "tool", summary: "exec_command", status: "failed", durationMs: 2000 },
        ],
        10,
      ),
    ).toBe("失败 1 条命令 · 2s");
    expect(
      formatCommandGroupSummary(
        [
          { type: "tool", summary: "exec_command", status: "running", startedAt: 4 },
        ],
        10,
      ),
    ).toBe("已运行 1 条命令");
  });
});

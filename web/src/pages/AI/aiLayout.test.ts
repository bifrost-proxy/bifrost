import { describe, expect, it } from "vitest";
import {
  buildRunnerOptions,
  resolveAiRouteState,
  selectDefaultRunner,
} from "./aiLayout";
import { isSelectedThread, type AgentThreadSummary } from "./AgentChatSection.helpers";

function params(query = "") {
  return new URLSearchParams(query);
}

describe("resolveAiRouteState", () => {
  it("defaults /ai to new chat mode", () => {
    expect(resolveAiRouteState(params())).toMatchObject({
      view: "chat",
      chatMode: "new",
      settings: null,
    });
  });

  it("keeps a targeted session in thread mode", () => {
    expect(resolveAiRouteState(params("session=admin-chat-1"))).toMatchObject({
      view: "chat",
      chatMode: "thread",
    });
  });

  it("maps legacy agent chat links to chat", () => {
    expect(
      resolveAiRouteState(params("aiSection=agent-chat&agentSection=chat")),
    ).toMatchObject({
      view: "chat",
      chatMode: "thread",
      settings: null,
      agentSection: "chat",
    });
  });

  it("maps legacy agent config sections into settings", () => {
    expect(
      resolveAiRouteState(params("aiSection=agent-model&agentSection=model")),
    ).toMatchObject({
      view: "settings",
      settings: "agent",
      agentSection: "model",
    });
  });

  it("keeps chat out of settings routes", () => {
    expect(
      resolveAiRouteState(params("settings=agent&agentSection=chat&session=admin-chat-1")),
    ).toMatchObject({
      view: "settings",
      settings: "agent",
      agentSection: "general",
    });
    expect(
      resolveAiRouteState(params("settings=chat&session=admin-chat-1")),
    ).toMatchObject({
      view: "settings",
      settings: "agent",
      agentSection: "general",
    });
  });

  it("lets explicit main views override stale settings params", () => {
    expect(
      resolveAiRouteState(params("view=asr&settings=agent&agentSection=model")),
    ).toMatchObject({
      view: "asr",
    });
    expect(
      resolveAiRouteState(params("view=videos&settings=im&imGatewaySection=routes")),
    ).toMatchObject({
      view: "videos",
    });
    expect(
      resolveAiRouteState(params("view=chat&settings=agent&agentSection=runners&mode=new")),
    ).toMatchObject({
      view: "chat",
      chatMode: "new",
    });
  });

  it("maps legacy ASR, Videos, and IM sections", () => {
    expect(resolveAiRouteState(params("aiSection=tools-asr")).view).toBe("asr");
    expect(resolveAiRouteState(params("aiSection=tools-videos")).view).toBe("videos");
    expect(
      resolveAiRouteState(params("aiSection=im-gateway-routes")),
    ).toMatchObject({
      view: "im",
      imGatewaySection: "routes",
    });
  });

  it("normalizes settings IM away from the deprecated Connections entry", () => {
    expect(resolveAiRouteState(params("settings=im"))).toMatchObject({
      view: "settings",
      settings: "im",
      imGatewaySection: "targets",
    });
    expect(
      resolveAiRouteState(params("view=settings&settings=im&imGatewaySection=connections")),
    ).toMatchObject({
      view: "settings",
      settings: "im",
      imGatewaySection: "targets",
    });
  });
});

describe("thread selection", () => {
  it("selects history threads under the new chat view when historyPath is present", () => {
    const thread: AgentThreadSummary = {
      session_key: "admin-chat-1",
      status: "ended",
      history_path: "/tmp/history.jsonl",
    };

    expect(isSelectedThread(thread, "admin-chat-1", "/tmp/history.jsonl", "chat")).toBe(true);
  });

  it("keeps legacy history view selection compatible", () => {
    const thread: AgentThreadSummary = {
      session_key: "admin-chat-1",
      status: "ended",
      history_path: "/tmp/history.jsonl",
    };

    expect(isSelectedThread(thread, "admin-chat-1", undefined, "history")).toBe(true);
  });
});

describe("runner options", () => {
  it("sorts product runners before custom runners", () => {
    const options = buildRunnerOptions({
      runners: {
        custom_runner: { enabled: true, adapter: "shell" },
        trae_runner: { enabled: true, adapter: "traex" },
        claude_runner: { enabled: true, adapter: "claude_code" },
        codex_runner: { enabled: true, adapter: "codex" },
      },
    });

    expect(options.map((option) => option.label)).toEqual([
      "Codex Runner",
      "Claude Code",
      "Trae X",
      "custom_runner (shell)",
    ]);
  });

  it("uses Codex as the default runner when available", () => {
    const options = buildRunnerOptions({
      runners: {
        claude_runner: { enabled: true, adapter: "claude_code" },
        codex_runner: { enabled: true, adapter: "codex" },
      },
    });

    expect(selectDefaultRunner(options)).toMatchObject({
      label: "Codex Runner",
      value: "codex_runner",
    });
  });

  it("falls back to the first available runner when Codex is unavailable", () => {
    const options = buildRunnerOptions({
      runners: {
        claude_runner: { enabled: true, adapter: "claude_code" },
      },
    });

    expect(selectDefaultRunner(options)).toMatchObject({
      label: "Claude Code",
      value: "claude_runner",
    });
  });

  it("filters disabled runners", () => {
    const options = buildRunnerOptions({
      runners: {
        codex_runner: { enabled: false, adapter: "codex" },
        traex_runner: { enabled: true, adapter: "traex" },
      },
    });

    expect(options.some((option) => option.value === "codex_runner")).toBe(false);
    expect(options.map((option) => option.label)).toContain("Trae X");
  });
});

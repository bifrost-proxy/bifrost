import { formatRunnerOptionLabel, type RunnerConfigPayload, type RunnerOption } from "./AgentChatSection.helpers";

export type AiMainView = "chat" | "asr" | "im" | "videos" | "settings";
export type AiChatMode = "new" | "thread";
export type AiSettingsTarget = "agent" | "im" | "chat" | null;

export type AiRouteState = {
  view: AiMainView;
  chatMode: AiChatMode;
  settings: AiSettingsTarget;
  agentSection?: string;
  imGatewaySection?: string;
};

const MAIN_VIEWS = new Set<AiMainView>(["chat", "asr", "im", "videos", "settings"]);

export function resolveAiRouteState(params: URLSearchParams): AiRouteState {
  const legacyAiSection = params.get("aiSection");
  const explicitView = params.get("view");
  let view: AiMainView =
    explicitView && MAIN_VIEWS.has(explicitView as AiMainView)
      ? (explicitView as AiMainView)
      : "chat";
  let settings = normalizeSettingsTarget(params.get("settings"));
  let agentSection = params.get("agentSection") || undefined;
  let imGatewaySection = params.get("imGatewaySection") || undefined;

  if (legacyAiSection) {
    if (legacyAiSection === "tools-asr") {
      view = "asr";
    } else if (legacyAiSection === "tools-videos") {
      view = "videos";
    } else if (legacyAiSection.startsWith("im-gateway-")) {
      view = "im";
      imGatewaySection ||= legacyAiSection.slice("im-gateway-".length);
    } else if (legacyAiSection.startsWith("agent-")) {
      const legacyAgentSection = legacyAiSection.slice("agent-".length);
      if (legacyAgentSection === "chat") {
        view = "chat";
      } else {
        view = "settings";
        settings = "agent";
        agentSection ||= legacyAgentSection;
      }
    }
  }

  if (agentSection && !settings && legacyAiSection?.startsWith("agent-") && legacyAiSection !== "agent-chat") {
    settings = "agent";
  }
  if (settings === "chat") {
    settings = "agent";
    agentSection = "general";
  }
  if (settings === "agent" && agentSection === "chat") {
    agentSection = "general";
  }
  if (settings === "im" && (!imGatewaySection || imGatewaySection === "connections")) {
    imGatewaySection = "targets";
  }
  const settingsOwnsRoute = !explicitView || explicitView === "settings";
  if (settings && settingsOwnsRoute) {
    view = "settings";
  }
  if (imGatewaySection && settings === "im" && settingsOwnsRoute) {
    view = "settings";
  }

  const hasThreadTarget = Boolean(params.get("session") || params.get("historyPath"));
  const legacyChatRoute = legacyAiSection === "agent-chat";
  const chatMode: AiChatMode =
    view === "chat" &&
    !hasThreadTarget &&
    (params.get("mode") === "new" || (!params.get("mode") && !legacyChatRoute))
      ? "new"
      : "thread";

  return {
    view,
    chatMode,
    settings,
    agentSection,
    imGatewaySection,
  };
}

function normalizeSettingsTarget(value: string | null): AiSettingsTarget {
  if (value === "agent" || value === "im" || value === "chat") {
    return value;
  }
  return null;
}

function runnerDisplayName(id: string, adapter?: string) {
  if (adapter === "codex" || id === "codex") return "Codex Runner";
  if (adapter === "claude_code" || id === "claude_code") return "Claude Code";
  if (adapter === "traex" || id === "traex") return "Trae X";
  if (adapter === "chatgpt_web" || id === "chatgpt_web") return "ChatGPT Web";
  return formatRunnerOptionLabel(id, adapter);
}

function runnerRank(option: RunnerOption) {
  const value = option.value.toLowerCase();
  const adapter = (option.adapter || "").toLowerCase();
  if (adapter === "codex" || value === "codex") return 0;
  if (adapter === "claude_code" || value === "claude_code") return 1;
  if (adapter === "traex" || value === "traex") return 2;
  if (adapter === "chatgpt_web" || value === "chatgpt_web") return 3;
  return 20;
}

export function buildRunnerOptions(payload?: RunnerConfigPayload): RunnerOption[] {
  const custom = Object.entries(payload?.runners || {})
    .filter(([, settings]) => settings.enabled !== false)
    .map(([id, settings]) => ({
      label: runnerDisplayName(id, settings.adapter),
      value: id,
      adapter: settings.adapter,
    }));
  return dedupeRunnerOptions(custom).sort((a, b) => {
    const rank = runnerRank(a) - runnerRank(b);
    return rank !== 0 ? rank : a.label.localeCompare(b.label);
  });
}

function dedupeRunnerOptions(options: RunnerOption[]) {
  const byValue = new Map<string, RunnerOption>();
  for (const option of options) {
    if (!byValue.has(option.value)) {
      byValue.set(option.value, option);
    }
  }
  return Array.from(byValue.values());
}

export function selectDefaultRunner(options: RunnerOption[]) {
  return (
    options.find((option) => option.adapter === "codex" || option.value === "codex") ||
    options[0] ||
    { label: "Codex Runner", value: "Codex", adapter: "codex" }
  );
}

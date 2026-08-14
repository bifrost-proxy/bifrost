import {
  formatRunnerOptionLabel,
  type RunnerConfigPayload,
  type RunnerOption,
} from "./AgentChatSection.helpers";

export type AiModulePath =
  | "/ai"
  | "/ai/asr"
  | "/ai/channels"
  | "/ai/agents"
  | "/ai/runs";

const MODULE_PATHS = new Set<AiModulePath>([
  "/ai",
  "/ai/asr",
  "/ai/channels",
  "/ai/agents",
  "/ai/runs",
]);

export function normalizeAiModulePath(pathname: string): AiModulePath {
  const normalized = pathname.replace(/\/+$/, "") || "/ai";
  return MODULE_PATHS.has(normalized as AiModulePath)
    ? (normalized as AiModulePath)
    : "/ai";
}

export function resolveLegacyAiDestination(
  params: URLSearchParams,
): AiModulePath | null {
  const view = params.get("view");
  const section = params.get("aiSection");
  const settings = params.get("settings");

  if (view === "asr" || section === "tools-asr") return "/ai/asr";
  if (
    view === "im" ||
    settings === "im" ||
    section?.startsWith("im-gateway-")
  ) {
    return "/ai/channels";
  }
  if (
    view === "chat" ||
    params.has("session") ||
    params.has("historyPath") ||
    section === "agent-chat"
  ) {
    return "/ai/runs";
  }
  if (
    view === "settings" ||
    settings === "agent" ||
    section?.startsWith("agent-")
  ) {
    return "/ai/agents";
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

export function buildRunnerOptions(
  payload?: RunnerConfigPayload,
): RunnerOption[] {
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
    options.find(
      (option) => option.adapter === "codex" || option.value === "codex",
    ) ||
    options[0] || { label: "Codex Runner", value: "Codex", adapter: "codex" }
  );
}

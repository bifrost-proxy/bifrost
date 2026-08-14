export type AiModulePath =
  | "/ai"
  | "/ai/asr"
  | "/ai/channels"
  | "/ai/agents"
  | "/ai/runs";

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

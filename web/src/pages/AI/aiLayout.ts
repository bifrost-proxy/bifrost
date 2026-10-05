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

const ASR_ROUTE_PARAMS = [
  "asrTab",
  "asrTask",
  "asrTaskTab",
  "asrFile",
  "asrDay",
  "asrDailyReport",
  "asrDailyAgent",
  "asrDailyAgentEdit",
] as const;

export function resolveLegacyAiRedirect(
  params: URLSearchParams,
): string | null {
  const destination = resolveLegacyAiDestination(params);
  if (!destination) return null;

  const next = new URLSearchParams();
  if (destination === "/ai/asr") {
    for (const key of ASR_ROUTE_PARAMS) {
      const value = params.get(key);
      if (value !== null) next.set(key, value);
    }
  } else if (destination === "/ai/runs") {
    const session = params.get("session");
    if (session) next.set("q", session);
  }
  return `${destination}${next.size ? `?${next.toString()}` : ""}`;
}

import { type AgentThreadSummary } from "./AgentChatSection.helpers";

export function isRunStateActive(state?: string) {
  return (
    state === "running" ||
    state === "queued" ||
    state === "waiting_for_tool" ||
    state === "waiting_on_session" ||
    state === "model_response" ||
    state === "tool_running" ||
    state === "compacting" ||
    state === "stopping"
  );
}

export function isRunStateTerminal(state?: string) {
  return (
    state === "completed" ||
    state === "failed" ||
    state === "cancelled" ||
    state === "stopped"
  );
}

export function isRunStateIdle(state?: string) {
  return state === "idle" || state === "ended" || isRunStateTerminal(state);
}

export function isThreadActive(thread?: AgentThreadSummary) {
  if (thread?.running === false) {
    return false;
  }
  const state = thread?.run_state || thread?.state;
  if (isRunStateIdle(state)) {
    return false;
  }
  return thread?.running === true || isRunStateActive(state);
}

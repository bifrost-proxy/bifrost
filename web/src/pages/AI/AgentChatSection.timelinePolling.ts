import { type AgentThreadSummary } from "./AgentChatSection.helpers";

export function isRunStateActive(state?: string) {
  return state === "running" || state === "queued" || state === "waiting_for_tool";
}

export function isThreadActive(thread?: AgentThreadSummary) {
  if (thread?.running === false) {
    return false;
  }
  return thread?.running === true || isRunStateActive(thread?.run_state || thread?.state);
}

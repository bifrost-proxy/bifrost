import { isRecord, stringFrom } from "./AgentChatSection.helpers";

export type QueuedInput = {
  seq: number;
  message: string;
};

export function queueItemsFromEvent(event: Record<string, unknown>): QueuedInput[] | null {
  const items = event.queueItems;
  if (!Array.isArray(items)) {
    return null;
  }
  return items
    .filter(isRecord)
    .map((item) => ({
      seq: Number(item.seq) || 0,
      message: stringFrom(item.message) || "",
    }))
    .filter((item) => item.seq > 0 && item.message.trim());
}

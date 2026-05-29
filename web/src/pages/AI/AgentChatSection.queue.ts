import { isRecord, stringFrom, type QueuedInput } from "./AgentChatSection.helpers";

export type { QueuedInput } from "./AgentChatSection.helpers";

export function queueItemsFromEvent(event: Record<string, unknown>): QueuedInput[] | null {
  return queueItemsFromUnknown(event.queueItems ?? event.queue_items);
}

export function queueItemsFromUnknown(value: unknown): QueuedInput[] | null {
  const items = value;
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

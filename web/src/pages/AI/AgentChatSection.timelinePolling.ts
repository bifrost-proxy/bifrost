import { useEffect, type Dispatch, type MutableRefObject, type SetStateAction } from "react";
import { apiFetch } from "../../api/apiFetch";
import {
  telemetryFromThread,
  type AgentThreadSummary,
  type ChatMessage,
  type HistoryEvent,
  type RunTelemetry,
} from "./AgentChatSection.helpers";
import {
  historyEventsToMessages,
  historyEventsToTelemetry,
} from "./AgentChatSection.timeline";

export function isRunStateActive(state?: string) {
  return state === "running" || state === "queued" || state === "waiting_for_tool";
}

export function isThreadActive(thread?: AgentThreadSummary) {
  if (thread?.running === false && !isRunStateActive(thread.state)) {
    return false;
  }
  return thread?.running === true || isRunStateActive(thread?.run_state || thread?.state);
}

export function useRunningTimelinePolling(params: {
  historyPath?: string;
  historyEventEndIndexRef?: MutableRefObject<number | undefined>;
  selectedThread?: AgentThreadSummary;
  telemetryPhase: RunTelemetry["phase"];
  userNearBottomRef: MutableRefObject<boolean>;
  refreshThreads: () => Promise<void>;
  replaceLoadedMessages: (restored: ChatMessage[], shouldStickToBottom: boolean) => void;
  mergeTimelineEvents?: (
    events: HistoryEvent[],
    page: {
      start_index?: number;
      end_index?: number;
      next_cursor?: number | null;
      has_more?: boolean;
    },
    shouldStickToBottom: boolean,
  ) => RunTelemetry | undefined;
  setRunning: Dispatch<SetStateAction<boolean>>;
  setTelemetry: Dispatch<SetStateAction<RunTelemetry>>;
}) {
  const {
    historyPath,
    historyEventEndIndexRef,
    selectedThread,
    telemetryPhase,
    userNearBottomRef,
    refreshThreads,
    mergeTimelineEvents,
    replaceLoadedMessages,
    setRunning,
    setTelemetry,
  } = params;

  useEffect(() => {
    const timelineHistoryPath = historyPath || selectedThread?.history_path;
    if (!timelineHistoryPath) {
      return;
    }
    if (telemetryPhase !== "running" && !isThreadActive(selectedThread)) {
      return;
    }

    let cancelled = false;
    let timeoutId: number | undefined;
    const pollTimeline = async () => {
      try {
        const since = historyEventEndIndexRef?.current;
        const query =
          since !== undefined
            ? `?since=${encodeURIComponent(String(since))}`
            : "?tail=true&limit=300";
        const response = await apiFetch(
          `/api/im-gateway/agent/sessions/history/${encodeURIComponent(
            timelineHistoryPath,
          )}${query}`,
        );
        if (!response.ok) {
          throw new Error(await response.text());
        }
        const payload = (await response.json()) as {
          events?: HistoryEvent[];
          start_index?: number;
          end_index?: number;
          next_cursor?: number | null;
          has_more?: boolean;
        };
        if (cancelled) {
          return;
        }
        const events = payload.events || [];
        if (mergeTimelineEvents) {
          const nextTelemetry = mergeTimelineEvents(
            events,
            payload,
            userNearBottomRef.current,
          );
          const stillRunning =
            nextTelemetry?.phase === "running" ||
            (!nextTelemetry && (telemetryPhase === "running" || isThreadActive(selectedThread)));
          setRunning(stillRunning);
          if (stillRunning) {
            timeoutId = window.setTimeout(pollTimeline, 1200);
          } else {
            void refreshThreads();
          }
          return;
        }
        const restored = historyEventsToMessages(events, {
          ensureRunningAssistant: isThreadActive(selectedThread),
          runningState: selectedThread?.run_state || selectedThread?.state,
        });
        if (restored.length > 0) {
          replaceLoadedMessages(restored, userNearBottomRef.current);
        }
        const nextTelemetry = historyEventsToTelemetry(
          events,
          selectedThread,
          telemetryFromThread(selectedThread),
        );
        setTelemetry(nextTelemetry);
        const stillRunning = nextTelemetry.phase === "running";
        setRunning(stillRunning);
        if (stillRunning) {
          timeoutId = window.setTimeout(pollTimeline, 1200);
        } else {
          void refreshThreads();
        }
      } catch {
        if (!cancelled) {
          timeoutId = window.setTimeout(pollTimeline, 2000);
        }
      }
    };

    timeoutId = window.setTimeout(pollTimeline, 600);
    return () => {
      cancelled = true;
      if (timeoutId !== undefined) {
        window.clearTimeout(timeoutId);
      }
    };
  }, [
    historyPath,
    historyEventEndIndexRef,
    mergeTimelineEvents,
    refreshThreads,
    replaceLoadedMessages,
    selectedThread,
    setRunning,
    setTelemetry,
    telemetryPhase,
    userNearBottomRef,
  ]);
}

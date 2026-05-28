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
  return thread?.running === true || isRunStateActive(thread?.run_state || thread?.state);
}

export function useRunningTimelinePolling(params: {
  historyPath?: string;
  selectedThread?: AgentThreadSummary;
  telemetryPhase: RunTelemetry["phase"];
  userNearBottomRef: MutableRefObject<boolean>;
  refreshThreads: () => Promise<void>;
  replaceLoadedMessages: (restored: ChatMessage[], shouldStickToBottom: boolean) => void;
  setRunning: Dispatch<SetStateAction<boolean>>;
  setTelemetry: Dispatch<SetStateAction<RunTelemetry>>;
}) {
  const {
    historyPath,
    selectedThread,
    telemetryPhase,
    userNearBottomRef,
    refreshThreads,
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
        const response = await apiFetch(
          `/api/im-gateway/agent/sessions/history/${encodeURIComponent(
            timelineHistoryPath,
          )}`,
        );
        if (!response.ok) {
          throw new Error(await response.text());
        }
        const payload = (await response.json()) as { events?: HistoryEvent[] };
        if (cancelled) {
          return;
        }
        const events = payload.events || [];
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
    refreshThreads,
    replaceLoadedMessages,
    selectedThread,
    setRunning,
    setTelemetry,
    telemetryPhase,
    userNearBottomRef,
  ]);
}

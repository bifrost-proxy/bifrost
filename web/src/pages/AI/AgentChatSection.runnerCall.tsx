import {
  useCallback,
  useMemo,
  useState,
  type CSSProperties,
  type Dispatch,
  type MutableRefObject,
  type SetStateAction,
} from "react";
import { Button, Tag, Tooltip, Typography, message as antdMessage } from "antd";
import { CloseOutlined, ExportOutlined, RobotOutlined } from "@ant-design/icons";
import {
  dedupeThreads,
  eventToProcessStep,
  reduceTelemetry,
  runRunnerCallStream,
  selectedRunnerAdapter,
  stringFrom,
  type AgentThreadSummary,
  type ChatMessage,
  type RunnerCallMessageMeta,
  type RunnerOption,
  type RunTelemetry,
} from "./AgentChatSection.helpers";

const { Text } = Typography;

export function useSlashRunnerSelection({
  draft,
  running,
  supplementSubmitting,
  runnerId,
  runnerOptions,
}: {
  draft: string;
  running: boolean;
  supplementSubmitting: boolean;
  runnerId: string;
  runnerOptions: RunnerOption[];
}) {
  const [slashRunner, setSlashRunner] = useState<RunnerOption | undefined>();
  const slashQuery =
    !slashRunner && draft.trimStart().startsWith("/")
      ? draft.trimStart().slice(1).trim().toLowerCase()
      : "";
  const slashRunnerOptions = useMemo(
    () =>
      runnerOptions
        .filter((option) => option.value !== runnerId)
        .filter((option) =>
          slashQuery
            ? option.label.toLowerCase().includes(slashQuery) ||
              option.value.toLowerCase().includes(slashQuery)
            : true,
        ),
    [runnerId, runnerOptions, slashQuery],
  );
  const showSlashRunnerPanel =
    !running &&
    !supplementSubmitting &&
    !slashRunner &&
    draft.trimStart().startsWith("/") &&
    slashRunnerOptions.length > 0;

  return {
    slashRunner,
    setSlashRunner,
    slashRunnerOptions,
    showSlashRunnerPanel,
  };
}

export function useRunnerCallHandler({
  historyPath,
  messages,
  pendingInstantScrollRef,
  refreshThreads,
  runnerId,
  runnerOptions,
  sessionKey,
  setDraft,
  setHistoryPath,
  setMessages,
  setRunning,
  setSearchParamsForActiveSession,
  setSlashRunner,
  setTelemetry,
  setThreads,
  workDir,
}: {
  historyPath?: string;
  messages: ChatMessage[];
  pendingInstantScrollRef: MutableRefObject<boolean>;
  refreshThreads: () => void;
  runnerId: string;
  runnerOptions: RunnerOption[];
  sessionKey: string;
  setDraft: Dispatch<SetStateAction<string>>;
  setHistoryPath: Dispatch<SetStateAction<string | undefined>>;
  setMessages: Dispatch<SetStateAction<ChatMessage[]>>;
  setRunning: Dispatch<SetStateAction<boolean>>;
  setSearchParamsForActiveSession: () => void;
  setSlashRunner: Dispatch<SetStateAction<RunnerOption | undefined>>;
  setTelemetry: Dispatch<SetStateAction<RunTelemetry>>;
  setThreads: Dispatch<SetStateAction<AgentThreadSummary[]>>;
  workDir: string;
}) {
  return useCallback(
    async (content: string, targetRunner: RunnerOption) => {
      const userMessage: ChatMessage = {
        id: `user-runner-call-${Date.now()}`,
        role: "user",
        content,
        timestamp: Date.now() / 1000,
        meta: "Runner call",
        runnerCall: {
          targetRunnerId: targetRunner.value,
          targetRunnerLabel: targetRunner.label,
          targetAdapter: targetRunner.adapter,
          status: "running",
        },
      };
      const assistantId = `assistant-runner-call-${Date.now()}`;
      const assistantMessage: ChatMessage = {
        id: assistantId,
        role: "assistant",
        content: "Runner is running...",
        timestamp: Date.now() / 1000,
        meta: targetRunner.label,
        runnerCall: {
          targetRunnerId: targetRunner.value,
          targetRunnerLabel: targetRunner.label,
          targetAdapter: targetRunner.adapter,
          status: "running",
        },
      };
      const callerMessages = messages
        .filter((message) => message.content.trim().length > 0)
        .map((message) => ({
          role: message.role,
          content: message.content,
        }));
      pendingInstantScrollRef.current = true;
      setMessages((prev) => [...prev, userMessage, assistantMessage]);
      setDraft("");
      setSlashRunner(undefined);
      setSearchParamsForActiveSession();
      setRunning(true);
      setThreads((prev) => {
        const now = Math.floor(Date.now() / 1000);
        const fallbackTitle =
          content.length > 40 ? `${content.slice(0, 40)}...` : content;
        return dedupeThreads([
          {
            session_key: sessionKey,
            status: "active",
            running: true,
            state: "running",
            title: fallbackTitle,
            source: "admin-api",
            start_time: now,
            last_active_time: now,
            duration_secs: 0,
            runner_id: runnerId === "bifrost_agent" ? undefined : runnerId,
            runner_type:
              runnerId === "bifrost_agent"
                ? "bifrost_agent"
                : selectedRunnerAdapter(runnerOptions, runnerId),
            work_dir: workDir || undefined,
          },
          ...prev.filter((thread) => thread.session_key !== sessionKey),
        ]);
      });
      setTelemetry({
        phase: "running",
        status: {
          work_dir: workDir || undefined,
          runner_id: runnerId === "bifrost_agent" ? undefined : runnerId,
          runner_type:
            runnerId === "bifrost_agent"
              ? "bifrost_agent"
              : selectedRunnerAdapter(runnerOptions, runnerId),
        },
        plan: [],
        tools: [],
        errors: [],
      });
      try {
        await runRunnerCallStream({
          message: content,
          sessionKey,
          historyPath,
          workDir: workDir || undefined,
          callerRunnerId: runnerId,
          callerRunnerAdapter: selectedRunnerAdapter(runnerOptions, runnerId),
          targetRunnerId: targetRunner.value,
          callerMessages,
          onEvent: (event) => {
            setTelemetry((prev) => reduceTelemetry(prev, event));
            const eventType = stringFrom(event.eventType);
            if (eventType === "runner_call_finished") {
              setTelemetry((prev) => ({ ...prev, phase: "finished" }));
            }
            if (eventType === "runner_call_started") {
              const callId = stringFrom(event.callId);
              const childSessionKey = stringFrom(event.childSessionKey);
              setMessages((prev) =>
                prev.map((message) =>
                  message.id === assistantId || message.id === userMessage.id
                    ? {
                        ...message,
                        runnerCall: {
                          ...(message.runnerCall || {
                            targetRunnerId: targetRunner.value,
                            targetRunnerLabel: targetRunner.label,
                          }),
                          callId,
                          childSessionKey,
                          status: "running",
                        },
                      }
                    : message,
                ),
              );
              return;
            }
            const step = eventToProcessStep(event);
            if (step) {
              setMessages((prev) =>
                prev.map((message) =>
                  message.id === assistantId
                    ? {
                        ...message,
                        processSteps: [...(message.processSteps || []), step],
                      }
                    : message,
                ),
              );
            }
          },
          onFinal: (response) => {
            setMessages((prev) =>
              prev.map((message) =>
                message.id === assistantId
                  ? {
                      ...message,
                      content: response || message.content,
                      runnerCall: message.runnerCall
                        ? { ...message.runnerCall, status: "success" }
                        : message.runnerCall,
                    }
                  : message.id === userMessage.id && message.runnerCall
                    ? {
                        ...message,
                        runnerCall: { ...message.runnerCall, status: "success" },
                      }
                  : message,
              ),
            );
          },
        });
        setHistoryPath(undefined);
        setSearchParamsForActiveSession();
        refreshThreads();
      } catch (error) {
        const text = error instanceof Error ? error.message : "Runner call failed";
        setTelemetry((prev) => ({
          ...prev,
          phase: "failed",
          errors: prev.errors.includes(text) ? prev.errors : [...prev.errors, text],
        }));
        setMessages((prev) =>
          prev.map((message) =>
            message.id === assistantId
              ? {
                  ...message,
                  content: text,
                  runnerCall: message.runnerCall
                    ? { ...message.runnerCall, status: "failed" }
                    : message.runnerCall,
                }
              : message.id === userMessage.id && message.runnerCall
                ? {
                    ...message,
                    runnerCall: { ...message.runnerCall, status: "failed" },
                  }
              : message,
          ),
        );
        antdMessage.error(text);
      } finally {
        setRunning(false);
      }
    },
    [
      historyPath,
      messages,
      pendingInstantScrollRef,
      refreshThreads,
      runnerId,
      runnerOptions,
      sessionKey,
      setDraft,
      setHistoryPath,
      setMessages,
      setRunning,
      setSearchParamsForActiveSession,
      setSlashRunner,
      setTelemetry,
      setThreads,
      workDir,
    ],
  );
}

export function RunnerCallChip({
  onOpenThread,
  role,
  runnerCall,
  style,
}: {
  onOpenThread?: () => void;
  role: ChatMessage["role"];
  runnerCall: RunnerCallMessageMeta;
  style: CSSProperties;
}) {
  return (
    <div data-testid="agent-chat-runner-call-chip" style={style}>
      <RobotOutlined />
      <span>
        {role === "user" ? "Run with" : "Runner"} {runnerCall.targetRunnerLabel}
      </span>
      <Tag
        color={
          runnerCall.status === "running"
            ? "processing"
            : runnerCall.status === "failed"
              ? "error"
              : "success"
        }
        style={{ marginInlineEnd: 0 }}
      >
        {runnerCall.status}
      </Tag>
      {onOpenThread ? (
        <Tooltip title="Open child thread">
          <button
            type="button"
            data-testid="agent-chat-runner-call-open-child"
            onClick={onOpenThread}
            style={{
              border: 0,
              background: "transparent",
              color: "inherit",
              cursor: "pointer",
              display: "inline-flex",
              alignItems: "center",
              padding: 0,
            }}
          >
            <ExportOutlined />
          </button>
        </Tooltip>
      ) : null}
    </div>
  );
}

export function SlashRunnerPanel({
  options,
  styles,
  onSelect,
}: {
  options: RunnerOption[];
  styles: Record<string, CSSProperties>;
  onSelect: (option: RunnerOption) => void;
}) {
  return (
    <div data-testid="agent-chat-slash-runner-panel" style={styles.slashRunnerPanel}>
      <Text type="secondary" style={{ fontSize: 12 }}>
        Choose a Runner
      </Text>
      <div style={styles.slashRunnerList}>
        {options.map((option) => (
          <button
            key={option.value}
            type="button"
            data-testid="agent-chat-slash-runner-option"
            style={styles.slashRunnerOption}
            onClick={() => onSelect(option)}
          >
            <RobotOutlined />
            <span style={styles.slashRunnerOptionText}>{option.label}</span>
          </button>
        ))}
      </div>
    </div>
  );
}

export function SelectedRunnerPill({
  runner,
  styles,
  onClear,
}: {
  runner: RunnerOption;
  styles: Record<string, CSSProperties>;
  onClear: () => void;
}) {
  return (
    <div data-testid="agent-chat-selected-runner" style={styles.selectedRunnerPill}>
      <RobotOutlined />
      <span>Run with {runner.label}</span>
      <Button
        size="small"
        type="text"
        icon={<CloseOutlined />}
        aria-label="Clear selected runner"
        onClick={onClear}
        style={styles.selectedRunnerClear}
      />
    </div>
  );
}

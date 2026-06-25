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
import { BulbOutlined, CloseOutlined, CompressOutlined, ExportOutlined, RobotOutlined } from "@ant-design/icons";
import {
  dedupeThreads,
  eventToProcessStep,
  reduceTelemetry,
  runRunnerCallStream,
  selectedRunnerAdapter,
  stringFrom,
  type AgentThreadSummary,
  type ChatMessage,
  type PendingChatImage,
  type RunnerCallMessageMeta,
  type RunnerOption,
  type RunTelemetry,
} from "./AgentChatSection.helpers";

const { Text } = Typography;

export type SlashCommandOption = {
  command: string;
  label: string;
  description: string;
  value: string;
  action: "send" | "insert";
  insertText?: string;
};

const SLASH_COMMAND_OPTIONS: SlashCommandOption[] = [
  {
    command: "/plan",
    label: "Plan mode",
    description: "进入规划模式，先产出方案再执行",
    value: "plan",
    action: "insert",
    insertText: "/plan ",
  },
  {
    command: "/compact",
    label: "Compact context",
    description: "立即压缩当前对话上下文",
    value: "compact",
    action: "send",
  },
];

const MODEL_SLASH_COMMAND_OPTIONS: SlashCommandOption[] = [
  {
    command: "/models",
    label: "Runner models",
    description: "列出当前 Runner 可用模型",
    value: "runner-models",
    action: "send",
  },
  {
    command: "/model",
    label: "Runner model",
    description: "查看或切换当前 Runner 的 session 模型",
    value: "runner-model",
    action: "insert",
    insertText: "/model ",
  },
];

export function useSlashRunnerSelection({
  enableCommands,
  enableModelCommands,
  draft,
  running,
  supplementSubmitting,
  runnerOptions,
}: {
  enableCommands: boolean;
  enableModelCommands: boolean;
  draft: string;
  running: boolean;
  supplementSubmitting: boolean;
  runnerOptions: RunnerOption[];
}) {
  const [slashRunner, setSlashRunner] = useState<RunnerOption | undefined>();
  const slashQuery =
    draft.trimStart().startsWith("/")
      ? draft.trimStart().slice(1).trim().toLowerCase()
      : "";
  const slashRunnerOptions = useMemo(
    () =>
      runnerOptions.filter((option) =>
        slashQuery
          ? option.label.toLowerCase().includes(slashQuery) ||
            option.value.toLowerCase().includes(slashQuery)
          : true,
      ),
    [runnerOptions, slashQuery],
  );
  const slashCommandOptions = useMemo(
    () => {
      const commands = [
        ...(enableCommands ? SLASH_COMMAND_OPTIONS : []),
        ...(enableModelCommands ? MODEL_SLASH_COMMAND_OPTIONS : []),
      ];
      const matches = commands.filter((option) => {
        if (!slashQuery) {
          return true;
        }
        const searchable = `${option.command} ${option.label} ${option.description}`;
        return searchable.toLowerCase().includes(slashQuery);
      });
      if (!slashQuery) {
        return matches;
      }
      return [...matches].sort((left, right) => {
        const leftExact = left.command.slice(1).toLowerCase() === slashQuery;
        const rightExact = right.command.slice(1).toLowerCase() === slashQuery;
        if (leftExact === rightExact) {
          return 0;
        }
        return leftExact ? -1 : 1;
      });
    },
    [enableCommands, enableModelCommands, slashQuery],
  );
  const showSlashRunnerPanel =
    !running &&
    !supplementSubmitting &&
    draft.trimStart().startsWith("/") &&
    (slashCommandOptions.length > 0 || slashRunnerOptions.length > 0);

  return {
    slashRunner,
    setSlashRunner,
    slashCommandOptions,
    slashRunnerOptions,
    showSlashRunnerPanel,
  };
}

export function useRunnerCallHandler({
  historyPath,
  messages,
  pendingInstantScrollRef,
  refreshSessionDetailTelemetry,
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
  refreshSessionDetailTelemetry: () => Promise<void>;
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
    async (content: string, targetRunner: RunnerOption, images: PendingChatImage[] = []) => {
      const userVisibleContent =
        content || (images.length === 1 ? "Attached 1 image" : `Attached ${images.length} images`);
      const userMessage: ChatMessage = {
        id: `user-runner-call-${Date.now()}`,
        role: "user",
        content: userVisibleContent,
        contentParts:
          images.length > 0
            ? [
                ...(content.trim() ? [{ type: "text" as const, text: content }] : []),
                ...images.map((image) => ({
                  type: "image_url" as const,
                  image_url: { url: image.previewUrl, detail: "auto" },
                })),
              ]
            : undefined,
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
        .filter(
          (
            message,
          ): message is ChatMessage & { role: "user" | "assistant" } =>
            (message.role === "user" || message.role === "assistant") &&
            message.content.trim().length > 0,
        )
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
          userVisibleContent.length > 40
            ? `${userVisibleContent.slice(0, 40)}...`
            : userVisibleContent;
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
          images,
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
        await refreshSessionDetailTelemetry();
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
      refreshSessionDetailTelemetry,
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
  commands,
  options,
  activeIndex,
  styles,
  onSelectCommand,
  onSelect,
  onActiveIndexChange,
}: {
  commands: SlashCommandOption[];
  options: RunnerOption[];
  activeIndex: number;
  styles: Record<string, CSSProperties>;
  onSelectCommand: (option: SlashCommandOption) => void;
  onSelect: (option: RunnerOption) => void;
  onActiveIndexChange: (index: number) => void;
}) {
  return (
    <div data-testid="agent-chat-slash-runner-panel" style={styles.slashRunnerPanel}>
      {commands.length > 0 ? (
        <>
          <Text type="secondary" style={{ fontSize: 12 }}>
            Commands
          </Text>
          <div style={styles.slashRunnerList}>
            {commands.map((option, index) => (
              <button
                key={option.value}
                type="button"
                data-testid="agent-chat-slash-command-option"
                data-active={activeIndex === index ? "true" : "false"}
                style={{
                  ...styles.slashRunnerOption,
                  ...(activeIndex === index ? styles.slashRunnerOptionActive : {}),
                }}
                onMouseEnter={() => onActiveIndexChange(index)}
                onClick={() => onSelectCommand(option)}
              >
                {option.value === "plan" ? <BulbOutlined /> : <CompressOutlined />}
                <span style={styles.slashRunnerOptionText}>
                  <strong>{option.command}</strong>
                  <Text type="secondary" style={{ marginLeft: 8 }}>
                    {option.description}
                  </Text>
                </span>
              </button>
            ))}
          </div>
        </>
      ) : null}
      {options.length > 0 ? (
        <>
          <Text type="secondary" style={{ fontSize: 12 }}>
            Choose a Runner
          </Text>
          <div style={styles.slashRunnerList}>
            {options.map((option, index) => {
              const optionIndex = commands.length + index;
              return (
              <button
                key={option.value}
                type="button"
                data-testid="agent-chat-slash-runner-option"
                data-active={activeIndex === optionIndex ? "true" : "false"}
                style={{
                  ...styles.slashRunnerOption,
                  ...(activeIndex === optionIndex ? styles.slashRunnerOptionActive : {}),
                }}
                onMouseEnter={() => onActiveIndexChange(optionIndex)}
                onClick={() => onSelect(option)}
              >
                <RobotOutlined />
                <span style={styles.slashRunnerOptionText}>{option.label}</span>
              </button>
              );
            })}
          </div>
        </>
      ) : null}
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

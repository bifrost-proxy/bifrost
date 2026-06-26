import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type CSSProperties,
  type FocusEvent,
} from "react";
import { Button, Space, Tag, Typography } from "antd";
import {
  CheckCircleOutlined,
  LoadingOutlined,
} from "@ant-design/icons";
import type { PlanStep } from "./AgentChatSection.helpers";

const { Text } = Typography;

type AgentChatPlanProps = {
  plan: PlanStep[];
  styles: Record<string, CSSProperties>;
  successColor: string;
  primaryColor: string;
};

const srOnlyStyle: CSSProperties = {
  position: "absolute",
  width: 1,
  height: 1,
  padding: 0,
  margin: -1,
  overflow: "hidden",
  clip: "rect(0, 0, 0, 0)",
  whiteSpace: "nowrap",
  border: 0,
};

function planStatusLabel(status: PlanStep["status"]): string {
  if (status === "completed") {
    return "Completed";
  }
  if (status === "in_progress") {
    return "In progress";
  }
  return "Pending";
}

function activePlanStep(plan: PlanStep[]): {
  step: PlanStep;
  extraCount: number;
  index: number;
  completedCount: number;
} {
  const currentIndex = plan.findIndex((step) => step.status === "in_progress");
  const nextIndex = plan.findIndex((step) => step.status !== "completed");
  const index =
    currentIndex >= 0 ? currentIndex : nextIndex >= 0 ? nextIndex : plan.length - 1;
  const current =
    plan[index] ??
    plan.find((step) => step.status !== "completed") ??
    plan[plan.length - 1];
  return {
    step: current,
    extraCount: Math.max(0, plan.length - 1),
    index,
    completedCount: plan.filter((step) => step.status === "completed").length,
  };
}

export function AgentChatPlan({
  plan,
  styles,
  successColor,
  primaryColor,
}: AgentChatPlanProps) {
  const [expanded, setExpanded] = useState(false);
  const panelRef = useRef<HTMLDivElement>(null);
  const collapseTimerRef = useRef<ReturnType<typeof window.setTimeout> | null>(null);

  const clearCollapseTimer = useCallback(() => {
    if (collapseTimerRef.current === null) {
      return;
    }
    window.clearTimeout(collapseTimerRef.current);
    collapseTimerRef.current = null;
  }, []);

  const openPlanDetails = useCallback(() => {
    clearCollapseTimer();
    setExpanded(true);
  }, [clearCollapseTimer]);

  const scheduleClosePlanDetails = useCallback(() => {
    clearCollapseTimer();
    collapseTimerRef.current = window.setTimeout(() => {
      collapseTimerRef.current = null;
      setExpanded(false);
    }, 180);
  }, [clearCollapseTimer]);

  const handlePlanBlur = useCallback(
    (event: FocusEvent<HTMLDivElement>) => {
      const nextTarget = event.relatedTarget;
      if (nextTarget instanceof Node && panelRef.current?.contains(nextTarget)) {
        return;
      }
      scheduleClosePlanDetails();
    },
    [scheduleClosePlanDetails],
  );

  useEffect(() => clearCollapseTimer, [clearCollapseTimer]);

  if (plan.length === 0) {
    return null;
  }

  const current = activePlanStep(plan);
  const progressPercent =
    plan.length > 0 ? Math.round((current.completedCount / plan.length) * 100) : 0;
  const progressLabel =
    current.completedCount === plan.length
      ? `Done ${plan.length}/${plan.length}`
      : `Step ${Math.min(current.index + 1, plan.length)}/${plan.length}`;

  return (
    <div
      ref={panelRef}
      data-testid="agent-chat-plan"
      style={styles.planPanel}
      onMouseEnter={openPlanDetails}
      onMouseLeave={scheduleClosePlanDetails}
      onFocus={openPlanDetails}
      onBlur={handlePlanBlur}
    >
      <button
        type="button"
        data-testid="agent-chat-plan-toggle"
        style={styles.planCapsule}
        aria-expanded={expanded}
        aria-label="Show task progress details"
      >
        <span style={styles.planHeaderLabel}>
          {current.step.status === "in_progress" ? (
            <LoadingOutlined
              spin
              aria-label="in progress"
              style={{ ...styles.planStatusIcon, color: primaryColor }}
            />
          ) : current.step.status === "completed" ? (
            <CheckCircleOutlined
              aria-label="completed"
              style={{ ...styles.planStatusIcon, color: successColor }}
            />
          ) : (
            <span aria-label="pending" style={styles.planPendingIcon} />
          )}
          <Text strong style={styles.planProgressLabel}>
            {progressLabel}
          </Text>
          <span
            data-testid="agent-chat-plan-progress"
            aria-hidden="true"
            style={styles.planProgressTrack}
          >
            <span
              data-testid="agent-chat-plan-progress-fill"
              style={{ ...styles.planProgressFill, width: `${progressPercent}%` }}
            />
          </span>
          <Text
            data-testid="agent-chat-plan-current"
            title={current.step.step}
            style={styles.planCurrentStep}
          >
            {current.step.step}
          </Text>
          {current.extraCount > 0 ? (
            <Tag data-testid="agent-chat-plan-more" style={styles.planCountTag}>
              +{current.extraCount}
            </Tag>
          ) : null}
        </span>
      </button>
      {expanded ? (
        <>
          <div
            aria-hidden="true"
            data-testid="agent-chat-plan-hover-bridge"
            style={styles.planHoverBridge}
            onMouseEnter={openPlanDetails}
            onMouseLeave={scheduleClosePlanDetails}
          />
          <div
            data-testid="agent-chat-plan-popover"
            role="dialog"
            aria-label="Task progress details"
            style={styles.planPopover}
            onMouseEnter={openPlanDetails}
            onMouseLeave={scheduleClosePlanDetails}
            onFocus={openPlanDetails}
            onBlur={handlePlanBlur}
          >
            <div style={styles.planPopoverHeader}>
              <Text strong style={{ fontSize: 13, lineHeight: "18px" }}>
                Task progress
              </Text>
              <Text type="secondary" style={{ fontSize: 12, lineHeight: "18px" }}>
                {current.completedCount}/{plan.length} completed
              </Text>
            </div>
            <div data-testid="agent-chat-plan-list" style={styles.planList}>
              {plan.map((step, index) => (
                <div
                  key={`${index}-${step.step}`}
                  data-testid="agent-chat-plan-item"
                  style={styles.planItem}
                >
                  {step.status === "completed" ? (
                    <CheckCircleOutlined
                      aria-label="completed"
                      data-testid="agent-chat-plan-status-completed"
                      style={{ ...styles.planStatusIcon, color: successColor }}
                    />
                  ) : step.status === "in_progress" ? (
                    <LoadingOutlined
                      spin
                      aria-label="in progress"
                      data-testid="agent-chat-plan-status-in-progress"
                      style={{ ...styles.planStatusIcon, color: primaryColor }}
                    />
                  ) : (
                    <span
                      aria-label="pending"
                      data-testid="agent-chat-plan-status-pending"
                      style={styles.planPendingIcon}
                    />
                  )}
                  <Text title={step.step} style={styles.planStepText}>
                    {step.step}
                  </Text>
                  <Text type="secondary" style={styles.planStatusText}>
                    {planStatusLabel(step.status)}
                  </Text>
                  <span style={srOnlyStyle}>{planStatusLabel(step.status)}</span>
                </div>
              ))}
            </div>
          </div>
        </>
      ) : null}
    </div>
  );
}

type AgentChatPromptChipsProps = {
  prompts: string[];
  onSelect: (prompt: string) => void;
};

export function AgentChatPromptChips({
  prompts,
  onSelect,
}: AgentChatPromptChipsProps) {
  if (prompts.length === 0) {
    return null;
  }

  return (
    <Space wrap data-testid="agent-chat-prompt-chips">
      {prompts.map((prompt) => (
        <Button key={prompt} size="small" onClick={() => onSelect(prompt)}>
          {prompt}
        </Button>
      ))}
    </Space>
  );
}

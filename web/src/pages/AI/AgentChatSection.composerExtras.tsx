import type { CSSProperties } from "react";
import { Button, Space, Tag, Typography } from "antd";
import {
  CheckCircleOutlined,
  DownOutlined,
  LoadingOutlined,
  RightOutlined,
} from "@ant-design/icons";
import type { PlanStep } from "./AgentChatSection.helpers";

const { Text } = Typography;

type AgentChatPlanProps = {
  plan: PlanStep[];
  collapsed: boolean;
  styles: Record<string, CSSProperties>;
  successColor: string;
  primaryColor: string;
  onToggle: () => void;
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

function collapsedPlanStep(plan: PlanStep[]): { step: PlanStep; extraCount: number } {
  const current =
    plan.find((step) => step.status === "in_progress") ??
    plan.find((step) => step.status !== "completed") ??
    plan[plan.length - 1];
  return {
    step: current,
    extraCount: Math.max(0, plan.length - 1),
  };
}

export function AgentChatPlan({
  plan,
  collapsed,
  styles,
  successColor,
  primaryColor,
  onToggle,
}: AgentChatPlanProps) {
  if (plan.length === 0) {
    return null;
  }

  const collapsedStep = collapsed ? collapsedPlanStep(plan) : null;

  return (
    <div data-testid="agent-chat-plan" style={styles.planPanel}>
      <button
        type="button"
        data-testid="agent-chat-plan-toggle"
        style={styles.planHeader}
        onClick={onToggle}
      >
        <span style={styles.planHeaderLabel}>
          {collapsedStep?.step.status === "in_progress" ? (
            <LoadingOutlined
              spin
              aria-label="in progress"
              style={{ ...styles.planStatusIcon, color: primaryColor }}
            />
          ) : collapsedStep?.step.status === "completed" ? (
            <CheckCircleOutlined
              aria-label="completed"
              style={{ ...styles.planStatusIcon, color: successColor }}
            />
          ) : collapsedStep ? (
            <span aria-label="pending" style={styles.planPendingIcon} />
          ) : (
            <CheckCircleOutlined />
          )}
          <Text strong style={{ fontSize: 13, lineHeight: "18px", flexShrink: 0 }}>
            Plan
          </Text>
          {collapsedStep ? (
            <>
              <Text
                data-testid="agent-chat-plan-current"
                title={collapsedStep.step.step}
                style={styles.planCurrentStep}
              >
                {collapsedStep.step.step}
              </Text>
              {collapsedStep.extraCount > 0 ? (
                <Tag
                  data-testid="agent-chat-plan-more"
                  style={styles.planCountTag}
                >
                  +{collapsedStep.extraCount}
                </Tag>
              ) : null}
            </>
          ) : (
            <Tag style={styles.planCountTag}>{plan.length}</Tag>
          )}
        </span>
        {collapsed ? <RightOutlined /> : <DownOutlined />}
      </button>
      {!collapsed ? (
        <div style={styles.planBody}>
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
                <span style={srOnlyStyle}>{planStatusLabel(step.status)}</span>
              </div>
            ))}
          </div>
        </div>
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

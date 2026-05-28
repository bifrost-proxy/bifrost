import type { CSSProperties } from "react";
import {
  formatContextWindow,
  formatNumber,
  type RunTelemetry,
} from "./AgentChatSection.helpers";

type AgentChatTokenHudProps = {
  telemetry: RunTelemetry;
  styles: Record<string, CSSProperties>;
};

export function AgentChatTokenHud({ telemetry, styles }: AgentChatTokenHudProps) {
  const totalTokens =
    telemetry.status?.total_tokens_used ?? telemetry.context?.totalTokensUsed;
  const contextPercent = contextUsagePercent(telemetry);
  const contextUsage = formatContextPercent(contextPercent);
  if (totalTokens === undefined && contextUsage === "-") {
    return null;
  }

  const label = `Tokens ${formatNumber(totalTokens)} · Context ${contextUsage}`;
  const progress = contextPercent ?? 0;
  const title = `${label} · Context window: ${formatContextWindow(telemetry.status, telemetry.context)}`;

  return (
    <div
      aria-label="Agent token usage"
      data-testid="agent-chat-token-hud"
      style={styles.tokenHud}
      title={title}
    >
      <span data-testid="agent-chat-token-hud-track" style={styles.tokenHudTrack}>
        <span
          data-testid="agent-chat-token-hud-progress"
          style={{ ...styles.tokenHudFill, width: `${progress}%` }}
        />
      </span>
      <span
        data-testid="agent-chat-token-hud-item"
        style={styles.tokenHudItem}
      >
        <span style={styles.tokenHudLabel}>Tokens </span>
        <span style={styles.tokenHudValue}>{formatNumber(totalTokens)}</span>
        <span style={styles.tokenHudLabel}> · Context </span>
        <span style={styles.tokenHudValue}>{contextUsage}</span>
      </span>
    </div>
  );
}

const DEFAULT_CONTEXT_WINDOW_TOKENS = 250_000;

function contextUsagePercent(telemetry: RunTelemetry) {
  const explicit =
    telemetry.status?.context_usage_percent ?? telemetry.context?.contextUsagePercent;
  if (typeof explicit === "number") {
    return clampPercent(explicit);
  }
  return clampPercent(
    ratioPercent(
      telemetry.status?.estimated_context_tokens ??
        telemetry.context?.estimatedContextTokens,
      telemetry.status?.context_window_tokens ??
        telemetry.context?.contextWindowTokens ??
        DEFAULT_CONTEXT_WINDOW_TOKENS,
    ),
  );
}

function formatContextPercent(value?: number) {
  if (typeof value !== "number") {
    return "-";
  }
  const rounded = Math.round(value * 10) / 10;
  return Number.isInteger(rounded) ? `${rounded}%` : `${rounded.toFixed(1)}%`;
}

function ratioPercent(used?: number, total?: number) {
  if (typeof used !== "number" || typeof total !== "number" || total <= 0) {
    return undefined;
  }
  return (used / total) * 100;
}

function clampPercent(value?: number) {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    return undefined;
  }
  return Math.max(0, Math.min(100, value));
}

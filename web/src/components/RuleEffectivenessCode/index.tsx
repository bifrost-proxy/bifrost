import { forwardRef, useMemo, type CSSProperties } from "react";
import { Tooltip, theme } from "antd";
import {
  analyzeRuleEffectiveness,
  type RuleLineEffect,
} from "../../utils/ruleEffectiveness";
import styles from "./index.module.css";

interface RuleEffectivenessCodeProps {
  content: string;
  emptyText?: string;
  className?: string;
  style?: CSSProperties;
  "data-testid"?: string;
}

const statusLabel: Record<RuleLineEffect["status"], string> = {
  active: "Effective",
  partial: "Partially effective",
  shadowed: "Covered",
  neutral: "Informational",
};

const RuleEffectivenessCode = forwardRef<HTMLPreElement, RuleEffectivenessCodeProps>(
  function RuleEffectivenessCode(
    {
      content,
      emptyText = "# No active rules",
      className,
      style,
      "data-testid": testId,
    },
    ref,
  ) {
    const { token } = theme.useToken();
    const displayContent = content.trim() || emptyText;
    const effects = useMemo(
      () => analyzeRuleEffectiveness(displayContent),
      [displayContent],
    );

    return (
      <pre
        ref={ref}
        className={`${styles.ruleEffectivenessCode} ${className ?? ""}`}
        data-testid={testId}
        style={{
          "--rule-effect-active-bg": token.colorSuccessBg,
          "--rule-effect-active-border": token.colorSuccess,
          "--rule-effect-active-text": token.colorSuccessText,
          "--rule-effect-partial-bg": token.colorWarningBg,
          "--rule-effect-partial-border": token.colorWarning,
          "--rule-effect-partial-text": token.colorWarningText,
          "--rule-effect-shadowed-bg": token.colorFillSecondary,
          "--rule-effect-shadowed-border": token.colorTextTertiary,
          "--rule-effect-shadowed-text": token.colorTextTertiary,
          "--rule-effect-line-hover": token.colorFillTertiary,
          ...style,
        } as CSSProperties}
      >
        {effects.map((effect) => (
          <RuleLine key={effect.lineNumber} effect={effect} />
        ))}
      </pre>
    );
  },
);

export default RuleEffectivenessCode;

function RuleLine({ effect }: { effect: RuleLineEffect }) {
  const hasEffect = effect.status !== "neutral";
  const tooltip = (
    <div className={styles.tooltip}>
      <div className={styles.tooltipTitle}>{statusLabel[effect.status]}</div>
      <div className={styles.tooltipMeta}>{effect.summary}</div>
      {effect.details.map((detail) => (
        <div className={styles.tooltipMeta} key={detail}>
          {detail}
        </div>
      ))}
    </div>
  );

  return (
    <Tooltip title={tooltip} mouseEnterDelay={0.25} placement="topLeft">
      <span
        className={styles.ruleLine}
        data-effect-status={effect.status}
        data-line-number={effect.lineNumber}
        data-covered-by-line={effect.coveredByLine ?? ""}
        data-status={effect.status}
      >
        <span className={styles.ruleText}>{effect.text || " "}</span>
        {hasEffect ? <span className={styles.statusDot} aria-hidden="true" /> : null}
      </span>
    </Tooltip>
  );
}

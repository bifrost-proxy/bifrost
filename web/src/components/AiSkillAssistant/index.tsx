import {
  CopyOutlined,
  FileTextOutlined,
  LinkOutlined,
  RobotOutlined,
  SearchOutlined,
  ThunderboltOutlined,
} from "@ant-design/icons";
import { Button, Popover, message, theme } from "antd";
import type { CSSProperties, ReactNode } from "react";
import { useMemo, useState } from "react";
import { useI18n, type TranslationKey } from "../../i18n";
import { copyToClipboard } from "../../utils/clipboard";
import styles from "./index.module.css";

const INSTALL_COMMAND = "bifrost install-skill -y";
const SKILL_DOC_URL = "https://github.com/bifrost-proxy/bifrost/blob/main/SKILL.md";

const scenarios: Array<{ icon: ReactNode; labelKey: TranslationKey }> = [
  {
    icon: <FileTextOutlined />,
    labelKey: "ai.skill.scenario.rules",
  },
  {
    icon: <SearchOutlined />,
    labelKey: "ai.skill.scenario.traffic",
  },
  {
    icon: <ThunderboltOutlined />,
    labelKey: "ai.skill.scenario.ports",
  },
];

export default function AiSkillAssistant() {
  const { token } = theme.useToken();
  const { t } = useI18n();
  const [open, setOpen] = useState(false);

  const cssVariables = useMemo(
    () =>
      ({
        "--ai-skill-text": token.colorText,
        "--ai-skill-muted": token.colorTextSecondary,
        "--ai-skill-border": token.colorBorderSecondary,
        "--ai-skill-accent": token.colorPrimary,
        "--ai-skill-accent-bg": token.colorPrimaryBg,
        "--ai-skill-panel-bg": token.colorBgElevated,
        "--ai-skill-command-bg": token.colorFillQuaternary,
        "--ai-skill-panel-shadow": token.boxShadow,
      }) as CSSProperties,
    [token],
  );

  const copyInstallCommand = async () => {
    const ok = await copyToClipboard(INSTALL_COMMAND);
    if (ok) {
      message.success(t("ai.skill.copied"));
      return;
    }
    message.error(t("ai.skill.copyFailed"));
  };

  const content = (
    <div
      className={styles.panel}
      data-testid="ai-skill-assistant-panel"
      style={cssVariables}
    >
      <div className={styles.header}>
        <div className={styles.badge}>
          <RobotOutlined />
        </div>
        <div className={styles.titleGroup}>
          <div className={styles.title}>{t("ai.skill.title")}</div>
          <div className={styles.subtitle}>{t("ai.skill.subtitle")}</div>
        </div>
      </div>
      <div className={styles.commandRow}>
        <code className={styles.command}>{INSTALL_COMMAND}</code>
        <Button
          className={styles.copyButton}
          data-testid="ai-skill-assistant-copy"
          size="small"
          type="primary"
          icon={<CopyOutlined />}
          onClick={copyInstallCommand}
        >
          {t("common.copy")}
        </Button>
      </div>
      <ul className={styles.scenarioList}>
        {scenarios.map((scenario) => (
          <li className={styles.scenario} key={scenario.labelKey}>
            <span className={styles.scenarioIcon}>{scenario.icon}</span>
            <span>{t(scenario.labelKey)}</span>
          </li>
        ))}
      </ul>
      <div className={styles.footer}>
        <span className={styles.hint}>{t("ai.skill.hint")}</span>
        <Button
          className={styles.link}
          data-testid="ai-skill-assistant-skill-link"
          size="small"
          icon={<LinkOutlined />}
          href={SKILL_DOC_URL}
          target="_blank"
          rel="noreferrer"
        >
          SKILL.md
        </Button>
      </div>
    </div>
  );

  return (
    <Popover
      content={content}
      trigger="click"
      placement="topRight"
      arrow={false}
      open={open}
      onOpenChange={setOpen}
      overlayInnerStyle={{ padding: 0, borderRadius: 8 }}
    >
      <button
        type="button"
        className={styles.statusButton}
        data-testid="ai-skill-assistant-trigger"
        aria-label={t("ai.skill.triggerAriaLabel")}
        style={cssVariables}
      >
        <RobotOutlined />
        <span>{t("ai.skill.trigger")}</span>
      </button>
    </Popover>
  );
}

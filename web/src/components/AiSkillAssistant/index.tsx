import {
  CopyOutlined,
  FileTextOutlined,
  LinkOutlined,
  RobotOutlined,
  SearchOutlined,
  ThunderboltOutlined,
} from "@ant-design/icons";
import { Button, Popover, message, theme } from "antd";
import type { CSSProperties } from "react";
import { useMemo, useState } from "react";
import { copyToClipboard } from "../../utils/clipboard";
import styles from "./index.module.css";

const INSTALL_COMMAND = "bifrost install-skill -y";
const SKILL_DOC_URL = "https://github.com/bifrost-proxy/bifrost/blob/main/SKILL.md";

const scenarios = [
  {
    icon: <FileTextOutlined />,
    label: "通过 AI 操作规则增删改查",
  },
  {
    icon: <SearchOutlined />,
    label: "流量搜索和问题排查",
  },
  {
    icon: <ThunderboltOutlined />,
    label: "多端口独立规则",
  },
];

export default function AiSkillAssistant() {
  const { token } = theme.useToken();
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
      message.success("Skill install command copied");
      return;
    }
    message.error("Failed to copy skill install command");
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
          <div className={styles.title}>AI Skill 加速 Bifrost 操作</div>
          <div className={styles.subtitle}>
            安装后让强大的 Agent 直接理解代理、规则、流量和远程能力。
          </div>
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
          Copy
        </Button>
      </div>
      <ul className={styles.scenarioList}>
        {scenarios.map((scenario) => (
          <li className={styles.scenario} key={scenario.label}>
            <span className={styles.scenarioIcon}>{scenario.icon}</span>
            <span>{scenario.label}</span>
          </li>
        ))}
      </ul>
      <div className={styles.footer}>
        <span className={styles.hint}>安装后可在 Agent 中直接调用 Bifrost 能力。</span>
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
        aria-label="Open Bifrost AI skill guide"
        style={cssVariables}
      >
        <RobotOutlined />
        <span>Skill</span>
      </button>
    </Popover>
  );
}

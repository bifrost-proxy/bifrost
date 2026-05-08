import {
  CheckOutlined,
  CopyOutlined,
  DragOutlined,
  FileTextOutlined,
  LinkOutlined,
  RobotOutlined,
  SearchOutlined,
  ThunderboltOutlined,
} from "@ant-design/icons";
import { Button, message, theme } from "antd";
import type { CSSProperties, PointerEvent as ReactPointerEvent } from "react";
import { useEffect, useMemo, useRef, useState } from "react";
import { copyToClipboard } from "../../utils/clipboard";
import styles from "./index.module.css";
import {
  clampSkillAssistantPosition,
  isSkillAssistantDrag,
  type SkillAssistantPosition,
} from "./position";

const INSTALL_COMMAND = "bifrost install-skill -y";
const SKILL_DOC_URL = "https://github.com/bifrost-proxy/bifrost/blob/main/SKILL.md";
const POSITION_STORAGE_KEY = "bifrost-ai-skill-assistant-position";
const HOVER_CLOSE_DELAY_MS = 450;

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

interface DragState {
  pointerId: number;
  startClientX: number;
  startClientY: number;
  startPosition: SkillAssistantPosition;
  hasDragged: boolean;
}

function readSavedPosition(): SkillAssistantPosition | null {
  try {
    const raw = window.localStorage.getItem(POSITION_STORAGE_KEY);
    if (!raw) {
      return null;
    }
    const parsed = JSON.parse(raw) as Partial<SkillAssistantPosition>;
    if (typeof parsed.x !== "number" || typeof parsed.y !== "number") {
      return null;
    }
    return parsed as SkillAssistantPosition;
  } catch {
    return null;
  }
}

function savePosition(position: SkillAssistantPosition) {
  try {
    window.localStorage.setItem(POSITION_STORAGE_KEY, JSON.stringify(position));
  } catch {
    void 0;
  }
}

export default function AiSkillAssistant() {
  const { token } = theme.useToken();
  const [open, setOpen] = useState(false);
  const [hidden, setHidden] = useState(false);
  const [position, setPosition] = useState<SkillAssistantPosition | null>(null);
  const dragStateRef = useRef<DragState | null>(null);
  const launcherRef = useRef<HTMLButtonElement | null>(null);
  const closeTimerRef = useRef<number | null>(null);

  useEffect(() => {
    const saved = readSavedPosition();
    if (saved) {
      setPosition(
        clampSkillAssistantPosition(saved, {
          width: window.innerWidth,
          height: window.innerHeight,
        }),
      );
    }
  }, []);

  useEffect(() => {
    if (!position) {
      return undefined;
    }

    const handleResize = () => {
      setPosition((current) => {
        if (!current) {
          return current;
        }
        const next = clampSkillAssistantPosition(current, {
          width: window.innerWidth,
          height: window.innerHeight,
        });
        savePosition(next);
        return next;
      });
    };

    window.addEventListener("resize", handleResize);
    return () => window.removeEventListener("resize", handleResize);
  }, [position]);

  useEffect(() => {
    return () => {
      if (closeTimerRef.current !== null) {
        window.clearTimeout(closeTimerRef.current);
      }
    };
  }, []);

  const cssVariables = useMemo(
    () =>
      ({
        "--ai-skill-text": token.colorText,
        "--ai-skill-muted": token.colorTextSecondary,
        "--ai-skill-border": token.colorBorderSecondary,
        "--ai-skill-accent": token.colorPrimary,
        "--ai-skill-accent-bg": token.colorPrimaryBg,
        "--ai-skill-launcher-text": token.colorPrimary,
        "--ai-skill-launcher-bg": token.colorBgElevated,
        "--ai-skill-panel-bg": token.colorBgElevated,
        "--ai-skill-command-bg": token.colorFillQuaternary,
        "--ai-skill-pulse": token.colorPrimaryBorder,
        "--ai-skill-shadow": token.boxShadowSecondary,
        "--ai-skill-panel-shadow": token.boxShadow,
      }) as CSSProperties,
    [token],
  );

  const rootPosition: CSSProperties = position
    ? { left: position.x, top: position.y }
    : { right: 24, bottom: 52 };

  const panelClassName = [
    styles.panel,
    position && position.x < 380 ? styles.panelLeft : "",
    position && position.y < 240 ? styles.panelBelow : "",
  ]
    .filter(Boolean)
    .join(" ");

  const copyInstallCommand = async () => {
    const ok = await copyToClipboard(INSTALL_COMMAND);
    if (ok) {
      message.success("Skill install command copied");
      return;
    }
    message.error("Failed to copy skill install command");
  };

  const clearCloseTimer = () => {
    if (closeTimerRef.current === null) {
      return;
    }
    window.clearTimeout(closeTimerRef.current);
    closeTimerRef.current = null;
  };

  const openPanel = () => {
    clearCloseTimer();
    setOpen(true);
  };

  const schedulePanelClose = () => {
    clearCloseTimer();
    closeTimerRef.current = window.setTimeout(() => {
      closeTimerRef.current = null;
      setOpen(false);
    }, HOVER_CLOSE_DELAY_MS);
  };

  const handlePointerDown = (event: ReactPointerEvent<HTMLButtonElement>) => {
    if (event.button !== 0) {
      return;
    }
    const rect = launcherRef.current?.getBoundingClientRect();
    const startPosition = position ?? {
      x: rect?.left ?? window.innerWidth - 80,
      y: rect?.top ?? window.innerHeight - 108,
    };
    dragStateRef.current = {
      pointerId: event.pointerId,
      startClientX: event.clientX,
      startClientY: event.clientY,
      startPosition,
      hasDragged: false,
    };
    launcherRef.current?.setPointerCapture(event.pointerId);
  };

  const handlePointerMove = (event: ReactPointerEvent<HTMLButtonElement>) => {
    const dragState = dragStateRef.current;
    if (!dragState || dragState.pointerId !== event.pointerId) {
      return;
    }

    const deltaX = event.clientX - dragState.startClientX;
    const deltaY = event.clientY - dragState.startClientY;
    if (!dragState.hasDragged && !isSkillAssistantDrag(deltaX, deltaY)) {
      return;
    }

    dragState.hasDragged = true;
    const next = clampSkillAssistantPosition(
      {
        x: dragState.startPosition.x + deltaX,
        y: dragState.startPosition.y + deltaY,
      },
      { width: window.innerWidth, height: window.innerHeight },
    );
    setPosition(next);
  };

  const handlePointerUp = (event: ReactPointerEvent<HTMLButtonElement>) => {
    const dragState = dragStateRef.current;
    if (!dragState || dragState.pointerId !== event.pointerId) {
      return;
    }

    launcherRef.current?.releasePointerCapture(event.pointerId);
    dragStateRef.current = null;

    if (dragState.hasDragged) {
      setPosition((current) => {
        if (current) {
          savePosition(current);
        }
        return current;
      });
      return;
    }

    setHidden(true);
  };

  if (hidden) {
    return null;
  }

  return (
    <div
      className={styles.root}
      data-testid="ai-skill-assistant"
      style={{ ...cssVariables, ...rootPosition }}
      onMouseEnter={openPanel}
      onMouseLeave={schedulePanelClose}
    >
      {open ? (
        <div className={panelClassName} data-testid="ai-skill-assistant-panel">
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
            <span className={styles.hint}>拖拽气泡可调整位置，点击气泡可隐藏。</span>
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
      ) : null}
      <button
        ref={launcherRef}
        type="button"
        className={styles.launcher}
        data-testid="ai-skill-assistant-launcher"
        aria-label="Hide Bifrost AI skill assistant"
        title="Hover for AI skill install guide. Drag to move. Click to hide."
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerUp={handlePointerUp}
        onPointerCancel={() => {
          dragStateRef.current = null;
        }}
      >
        <span className={styles.launcherPulse} aria-hidden="true" />
        <RobotOutlined className={styles.launcherIcon} />
        <span className={styles.dragHint} aria-hidden="true">
          {open ? <CheckOutlined /> : <DragOutlined />}
        </span>
      </button>
    </div>
  );
}

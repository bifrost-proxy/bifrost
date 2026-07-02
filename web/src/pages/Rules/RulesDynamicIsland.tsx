import { useState, useRef, useEffect, useCallback, type CSSProperties } from "react";
import { theme, Badge, Tooltip, Button, message } from "antd";
import {
  ThunderboltOutlined,
  TeamOutlined,
  WarningOutlined,
  CodeOutlined,
  CopyOutlined,
  CheckOutlined,
} from "@ant-design/icons";
import { getActiveSummary, type ActiveRuleItem, type VariableConflict } from "../../api/rules";
import { useRulesStore } from "../../stores/useRulesStore";
import { copyToClipboard } from "../../utils/clipboard";

interface Props {
  onNavigateRule: (name: string, groupId: string | null) => void;
}

const DRAG_THRESHOLD = 4;
const VIEWPORT_MARGIN = 8;

type IslandPosition = { x: number; y: number };

export default function RulesDynamicIsland({ onNavigateRule }: Props) {
  const { token } = theme.useToken();
  const [expanded, setExpanded] = useState(false);
  const [dragging, setDragging] = useState(false);
  const [position, setPosition] = useState<IslandPosition | null>(null);
  const [activeRules, setActiveRules] = useState<ActiveRuleItem[]>([]);
  const [variableConflicts, setVariableConflicts] = useState<VariableConflict[]>([]);
  const [mergedContent, setMergedContent] = useState("");
  const [showMerged, setShowMerged] = useState(false);
  const [copiedMergedContent, setCopiedMergedContent] = useState<string | null>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const copiedTimerRef = useRef<number | null>(null);
  const dragRef = useRef({
    startX: 0,
    startY: 0,
    startPosX: 0,
    startPosY: 0,
    hasMoved: false,
  });

  const rules = useRulesStore((s) => s.rules);
  const requestIdRef = useRef(0);

  const clampPosition = useCallback((next: IslandPosition, rect?: DOMRect) => {
    const elRect = rect ?? containerRef.current?.getBoundingClientRect();
    const width = elRect?.width ?? 160;
    const height = elRect?.height ?? 42;
    const maxX = Math.max(VIEWPORT_MARGIN, window.innerWidth - width - VIEWPORT_MARGIN);
    const maxY = Math.max(VIEWPORT_MARGIN, window.innerHeight - height - VIEWPORT_MARGIN);
    return {
      x: Math.max(VIEWPORT_MARGIN, Math.min(next.x, maxX)),
      y: Math.max(VIEWPORT_MARGIN, Math.min(next.y, maxY)),
    };
  }, []);

  const refreshActiveRules = useCallback(() => {
    const id = ++requestIdRef.current;
    getActiveSummary()
      .then((resp) => {
        if (id === requestIdRef.current) {
          setActiveRules(resp.rules);
          setVariableConflicts(resp.variable_conflicts ?? []);
          setMergedContent(resp.merged_content ?? "");
        }
      })
      .catch(() => {
        if (id === requestIdRef.current) {
          setActiveRules([]);
          setVariableConflicts([]);
          setMergedContent("");
        }
      });
  }, []);

  useEffect(() => {
    refreshActiveRules();
  }, [rules, refreshActiveRules]);

  const mergedCopied = copiedMergedContent === mergedContent && mergedContent.trim() !== "";

  useEffect(() => {
    return () => {
      if (copiedTimerRef.current !== null) {
        window.clearTimeout(copiedTimerRef.current);
      }
    };
  }, []);

  useEffect(() => {
    if (!expanded) return;
    const handler = (e: MouseEvent) => {
      if (
        containerRef.current &&
        !containerRef.current.contains(e.target as Node)
      ) {
        setExpanded(false);
      }
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [expanded]);

  useEffect(() => {
    const handleResize = () => {
      setPosition((current) => (current ? clampPosition(current) : current));
    };
    window.addEventListener("resize", handleResize);
    return () => window.removeEventListener("resize", handleResize);
  }, [clampPosition]);

  const initPosition = useCallback(() => {
    if (position !== null || !containerRef.current) return;
    const rect = containerRef.current.getBoundingClientRect();
    setPosition(clampPosition({ x: rect.left, y: rect.top }, rect));
  }, [clampPosition, position]);

  const handlePointerDown = useCallback(
    (e: React.PointerEvent) => {
      if (e.pointerType === "mouse" && e.button !== 0) return;
      initPosition();

      if (!containerRef.current) return;
      const rect = containerRef.current.getBoundingClientRect();

      dragRef.current = {
        startX: e.clientX,
        startY: e.clientY,
        startPosX: rect.left,
        startPosY: rect.top,
        hasMoved: false,
      };

      const handlePointerMove = (ev: PointerEvent) => {
        const dx = ev.clientX - dragRef.current.startX;
        const dy = ev.clientY - dragRef.current.startY;
        if (
          !dragRef.current.hasMoved &&
          Math.abs(dx) < DRAG_THRESHOLD &&
          Math.abs(dy) < DRAG_THRESHOLD
        ) {
          return;
        }
        dragRef.current.hasMoved = true;
        setDragging(true);

        setPosition(clampPosition({
          x: dragRef.current.startPosX + dx,
          y: dragRef.current.startPosY + dy,
        }));
      };

      const handlePointerUp = () => {
        document.removeEventListener("pointermove", handlePointerMove);
        document.removeEventListener("pointerup", handlePointerUp);
        setDragging(false);

        if (!dragRef.current.hasMoved) {
          setExpanded((v) => !v);
        }
      };

      document.addEventListener("pointermove", handlePointerMove);
      document.addEventListener("pointerup", handlePointerUp);
    },
    [clampPosition, initPosition],
  );

  const handleCopyMergedRules = useCallback(async (e: React.MouseEvent) => {
    e.stopPropagation();
    const text = mergedContent.trim();
    if (!text) return;
    const ok = await copyToClipboard(text);
    if (ok) {
      setCopiedMergedContent(mergedContent);
      message.success("Merged rules copied");
      if (copiedTimerRef.current !== null) {
        window.clearTimeout(copiedTimerRef.current);
      }
      copiedTimerRef.current = window.setTimeout(() => {
        setCopiedMergedContent(null);
        copiedTimerRef.current = null;
      }, 1600);
    } else {
      message.error("Failed to copy merged rules");
    }
  }, [mergedContent]);

  const positionStyle: CSSProperties =
    position !== null
      ? { left: position.x, top: position.y }
      : { top: 14, left: "50%", transform: "translateX(-50%)" };
  const panelOpensUp =
    position !== null && position.y > Math.max(180, window.innerHeight / 2);

  const ownRules = activeRules.filter((r) => !r.group_id);
  const groupRulesMap = new Map<string, { groupName: string; groupId: string; rules: ActiveRuleItem[] }>();
  for (const r of activeRules) {
    if (r.group_id) {
      if (!groupRulesMap.has(r.group_id)) {
        groupRulesMap.set(r.group_id, {
          groupName: r.group_name ?? r.group_id,
          groupId: r.group_id,
          rules: [],
        });
      }
      groupRulesMap.get(r.group_id)!.rules.push(r);
    }
  }

  return (
    <div
      ref={containerRef}
      style={{
        position: "fixed",
        ...positionStyle,
        display: "flex",
        justifyContent: "center",
        zIndex: 40,
        pointerEvents: "auto",
      }}
    >
      <div
        data-testid="rules-dynamic-island-trigger"
        onPointerDown={handlePointerDown}
        style={{
          display: "inline-flex",
          alignItems: "center",
          gap: 8,
          padding: expanded ? "8px 20px" : "6px 16px",
          borderRadius: expanded ? 16 : 20,
          backgroundColor: token.colorBgElevated,
          border: `1px solid ${token.colorBorderSecondary}`,
          boxShadow: expanded
            ? token.boxShadowSecondary
            : "0 1px 3px rgba(0,0,0,0.08)",
          cursor: dragging ? "grabbing" : "grab",
          touchAction: "none",
          transition: dragging
            ? "none"
            : "all 0.3s cubic-bezier(0.4, 0, 0.2, 1)",
          userSelect: "none",
          minWidth: expanded ? 220 : "auto",
        }}
      >
        <Badge
          count={activeRules.length}
          size="small"
          color={
            activeRules.length > 0
              ? token.colorSuccess
              : token.colorTextDisabled
          }
          overflowCount={99}
        >
          <ThunderboltOutlined
            style={{
              fontSize: 14,
              color:
                activeRules.length > 0
                  ? token.colorSuccess
                  : token.colorTextDisabled,
            }}
          />
        </Badge>
        <span
          style={{
            fontSize: 13,
            fontWeight: 500,
            color: token.colorText,
          }}
        >
          {activeRules.length} active
        </span>
        {variableConflicts.length > 0 && (
          <Tooltip title={`${variableConflicts.length} variable conflict${variableConflicts.length > 1 ? "s" : ""}`}>
            <WarningOutlined
              style={{
                fontSize: 13,
                color: token.colorWarning,
              }}
            />
          </Tooltip>
        )}
      </div>

      {expanded && (activeRules.length > 0 || variableConflicts.length > 0) && (
        <div
          style={{
            position: "absolute",
            top: panelOpensUp ? undefined : "100%",
            bottom: panelOpensUp ? "100%" : undefined,
            left: "50%",
            transform: "translateX(-50%)",
            marginTop: panelOpensUp ? undefined : 4,
            marginBottom: panelOpensUp ? 4 : undefined,
            minWidth: 360,
            maxWidth: "min(570px, calc(100vw - 24px))",
            width: "max-content",
            maxHeight: "min(500px, calc(100vh - 84px))",
            overflowY: "auto",
            backgroundColor: token.colorBgElevated,
            border: `1px solid ${token.colorBorderSecondary}`,
            borderRadius: 12,
            boxShadow: token.boxShadowSecondary,
            padding: "4px 0",
            animation: "islandFadeIn 0.2s ease",
          }}
          data-testid="rules-dynamic-island-panel"
        >
          {variableConflicts.length > 0 && (
            <div
              style={{
                margin: "6px 12px 4px",
                padding: "8px 12px",
                borderRadius: 8,
                backgroundColor: token.colorWarningBg,
                border: `1px solid ${token.colorWarningBorder}`,
              }}
            >
              <div
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: 6,
                  fontSize: 12,
                  fontWeight: 600,
                  color: token.colorWarning,
                  marginBottom: 4,
                }}
              >
                <WarningOutlined style={{ fontSize: 12 }} />
                Variable Conflicts
              </div>
              {variableConflicts.map((conflict) => (
                <div key={conflict.variable_name} style={{ marginTop: 4 }}>
                  <div
                    style={{
                      fontSize: 12,
                      fontWeight: 500,
                      color: token.colorText,
                      fontFamily: "monospace",
                    }}
                  >
                    {`{${conflict.variable_name}}`}
                  </div>
                  {conflict.definitions.map((def, idx) => (
                    <div
                      key={idx}
                      style={{
                        fontSize: 11,
                        color: token.colorTextSecondary,
                        paddingLeft: 8,
                        whiteSpace: "nowrap",
                        overflow: "hidden",
                        textOverflow: "ellipsis",
                      }}
                    >
                      <span style={{ fontWeight: 500 }}>{def.rule_name}</span>
                      {": "}
                      <span style={{ fontFamily: "monospace", fontSize: 10 }}>
                        {def.value_preview}
                      </span>
                    </div>
                  ))}
                </div>
              ))}
            </div>
          )}
          {ownRules.length > 0 && (
            <>
              <div
                style={{
                  padding: "6px 16px 2px",
                  fontSize: 11,
                  fontWeight: 600,
                  color: token.colorTextDescription,
                  textTransform: "uppercase",
                  letterSpacing: 0.5,
                }}
              >
                My Rules
              </div>
              {ownRules.map((rule) => (
                <RuleRow
                  key={`own-${rule.name}`}
                  rule={rule}
                  token={token}
                  onClick={() => {
                    setExpanded(false);
                    onNavigateRule(rule.name, null);
                  }}
                />
              ))}
            </>
          )}
          {[...groupRulesMap.values()].map((group) => (
            <div key={group.groupId}>
              <div
                style={{
                  padding: "8px 16px 2px",
                  fontSize: 11,
                  fontWeight: 600,
                  color: token.colorTextDescription,
                  display: "flex",
                  alignItems: "center",
                  gap: 4,
                }}
              >
                <TeamOutlined style={{ fontSize: 11 }} />
                <span
                  style={{
                    overflow: "hidden",
                    textOverflow: "ellipsis",
                    whiteSpace: "nowrap",
                  }}
                >
                  {group.groupName}
                </span>
              </div>
              {group.rules.map((rule) => (
                <RuleRow
                  key={`${group.groupId}-${rule.name}`}
                  rule={rule}
                  token={token}
                  onClick={() => {
                    setExpanded(false);
                    onNavigateRule(rule.name, rule.group_id);
                  }}
                />
              ))}
            </div>
          ))}
          {mergedContent.trim() && (
            <div style={{ margin: "4px 12px 8px" }}>
              <div
                data-testid="rules-dynamic-island-merged-toggle"
                onClick={(e) => {
                  e.stopPropagation();
                  setShowMerged((v) => !v);
                }}
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: 6,
                  padding: "6px 4px",
                  fontSize: 11,
                  fontWeight: 600,
                  color: token.colorPrimary,
                  cursor: "pointer",
                  userSelect: "none",
                  borderTop: `1px solid ${token.colorBorderSecondary}`,
                }}
              >
                <CodeOutlined style={{ fontSize: 11 }} />
                {showMerged ? "Hide" : "Show"} Merged Rules
              </div>
              {showMerged && (
                <div
                  data-testid="rules-dynamic-island-merged-panel"
                  style={{
                    position: "relative",
                  }}
                >
                  <Tooltip title={mergedCopied ? "Copied" : "Copy merged rules"}>
                    <Button
                      aria-label="Copy merged rules"
                      data-testid="rules-dynamic-island-copy-merged"
                      type="text"
                      size="small"
                      icon={mergedCopied ? <CheckOutlined /> : <CopyOutlined />}
                      onClick={handleCopyMergedRules}
                      style={{
                        position: "absolute",
                        top: 6,
                        right: 6,
                        zIndex: 1,
                        color: mergedCopied ? token.colorSuccess : token.colorTextSecondary,
                        backgroundColor: token.colorBgContainer,
                        border: `1px solid ${token.colorBorderSecondary}`,
                      }}
                    />
                  </Tooltip>
                  <pre
                    data-testid="rules-dynamic-island-merged-content"
                    style={{
                      margin: 0,
                      padding: "8px 42px 8px 10px",
                      borderRadius: 8,
                      backgroundColor: token.colorFillQuaternary,
                      border: `1px solid ${token.colorBorderSecondary}`,
                      fontSize: 11,
                      lineHeight: 1.5,
                      fontFamily: "monospace",
                      color: token.colorText,
                      whiteSpace: "pre",
                      overflowX: "auto",
                    }}
                  >
                    {mergedContent.trim()}
                  </pre>
                </div>
              )}
            </div>
          )}
        </div>
      )}

      {expanded && activeRules.length === 0 && variableConflicts.length === 0 && (
        <div
          style={{
            position: "absolute",
            top: panelOpensUp ? undefined : "100%",
            bottom: panelOpensUp ? "100%" : undefined,
            left: "50%",
            transform: "translateX(-50%)",
            marginTop: panelOpensUp ? undefined : 4,
            marginBottom: panelOpensUp ? 4 : undefined,
            minWidth: 200,
            backgroundColor: token.colorBgElevated,
            border: `1px solid ${token.colorBorderSecondary}`,
            borderRadius: 12,
            boxShadow: token.boxShadowSecondary,
            padding: "16px 20px",
            textAlign: "center",
            fontSize: 13,
            color: token.colorTextDescription,
          }}
          data-testid="rules-dynamic-island-empty"
        >
          No active rules
        </div>
      )}
    </div>
  );
}

function RuleRow({
  rule,
  token,
  onClick,
}: {
  rule: ActiveRuleItem;
  token: ReturnType<typeof theme.useToken>["token"];
  onClick: () => void;
}) {
  return (
    <div
      data-testid="rules-dynamic-island-rule-row"
      data-rule-name={rule.name}
      onClick={(e) => {
        e.stopPropagation();
        onClick();
      }}
      style={{
        padding: "8px 16px",
        fontSize: 13,
        color: token.colorText,
        cursor: "pointer",
        display: "flex",
        alignItems: "center",
        gap: 8,
        transition: "background-color 0.15s",
      }}
      onMouseEnter={(e) => {
        (e.currentTarget as HTMLDivElement).style.backgroundColor =
          token.colorBgTextHover;
      }}
      onMouseLeave={(e) => {
        (e.currentTarget as HTMLDivElement).style.backgroundColor =
          "transparent";
      }}
    >
      <ThunderboltOutlined
        style={{ fontSize: 12, color: token.colorSuccess, flexShrink: 0 }}
      />
      <span
        style={{
          flex: 1,
          overflow: "hidden",
          textOverflow: "ellipsis",
          whiteSpace: "nowrap",
        }}
      >
        {rule.name}
      </span>
      <span
        style={{
          fontSize: 12,
          color: token.colorTextDescription,
          flexShrink: 0,
        }}
      >
        {rule.rule_count} rules
      </span>
    </div>
  );
}

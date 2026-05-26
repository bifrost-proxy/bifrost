import { useCallback, useEffect, useMemo } from "react";
import { useSearchParams } from "react-router-dom";
import { Grid, theme, Typography } from "antd";
import { CloudOutlined, RobotOutlined, ToolOutlined } from "@ant-design/icons";
import AgentTab from "../Settings/tabs/AgentTab";
import ImGatewayTab from "../Settings/tabs/ImGatewayTab";
import ASR from "../ASR";
import AgentChatSection from "./AgentChatSection";
import {
  AGENT_SECTION_NAV,
  type AgentSectionId,
  IM_GATEWAY_SECTION_NAV,
  type ImGatewaySectionId,
} from "../Settings/tabs/aiSections";

const { Text } = Typography;
const { useBreakpoint } = Grid;

type AiSection =
  | {
      id: "tools-asr";
      group: "tools";
      section: "asr";
      label: string;
    }
  | {
      id: `agent-${AgentSectionId}`;
      group: "agent";
      section: AgentSectionId;
      label: string;
    }
  | {
      id: `im-gateway-${ImGatewaySectionId}`;
      group: "im-gateway";
      section: ImGatewaySectionId;
      label: string;
    };

const DEFAULT_AI_SECTION_ID = "agent-general";

export default function AI() {
  const [searchParams, setSearchParams] = useSearchParams();
  const { token } = theme.useToken();
  const screens = useBreakpoint();
  const isCompactNav = !screens.lg;

  const sections = useMemo<AiSection[]>(
    () => [
      {
        id: "tools-asr" as const,
        group: "tools" as const,
        section: "asr" as const,
        label: "ASR",
      },
      ...AGENT_SECTION_NAV.map((section) => ({
        id: `agent-${section.id}` as const,
        group: "agent" as const,
        section: section.id,
        label: section.label,
      })),
      ...IM_GATEWAY_SECTION_NAV.map((section) => ({
        id: `im-gateway-${section.id}` as const,
        group: "im-gateway" as const,
        section: section.id,
        label: section.label,
      })),
    ],
    [],
  );

  const sectionById = useMemo(
    () => new Map(sections.map((section) => [section.id, section])),
    [sections],
  );

  const activeSection = useMemo(() => {
    const fromAiSection = searchParams.get("aiSection") as
      | AiSection["id"]
      | null;
    if (fromAiSection && sectionById.has(fromAiSection)) {
      return sectionById.get(fromAiSection)!;
    }

    const agentSection = searchParams.get("agentSection");
    const agentId = `agent-${agentSection}` as AiSection["id"];
    if (agentSection && sectionById.has(agentId)) {
      return sectionById.get(agentId)!;
    }

    const imGatewaySection = searchParams.get("imGatewaySection");
    const imGatewayId = `im-gateway-${imGatewaySection}` as AiSection["id"];
    if (imGatewaySection && sectionById.has(imGatewayId)) {
      return sectionById.get(imGatewayId)!;
    }

    if (searchParams.get("session")) {
      return sectionById.get("agent-sessions")!;
    }

    return sectionById.get(DEFAULT_AI_SECTION_ID)!;
  }, [searchParams, sectionById]);

  useEffect(() => {
    const currentAiSection = searchParams.get("aiSection");
    if (currentAiSection === activeSection.id) {
      return;
    }
    setSearchParams(
      (prev) => {
        prev.set("aiSection", activeSection.id);
        if (activeSection.group === "agent") {
          prev.set("agentSection", activeSection.section);
        } else if (activeSection.group === "im-gateway") {
          prev.set("imGatewaySection", activeSection.section);
        }
        return prev;
      },
      { replace: true },
    );
  }, [activeSection, searchParams, setSearchParams]);

  const handleSelectSection = useCallback(
    (section: AiSection) => {
      setSearchParams(
        (prev) => {
          prev.set("aiSection", section.id);
          prev.delete("session");
          prev.delete("view");
          prev.delete("historyPath");
          if (section.group === "agent") {
            prev.set("agentSection", section.section);
            prev.delete("imGatewaySection");
          } else if (section.group === "im-gateway") {
            prev.set("imGatewaySection", section.section);
            prev.delete("agentSection");
          } else {
            prev.delete("agentSection");
            prev.delete("imGatewaySection");
          }
          return prev;
        },
        { replace: false },
      );
    },
    [setSearchParams],
  );

  const renderSectionButton = (section: AiSection) => {
    const active = activeSection.id === section.id;
    return (
      <button
        key={section.id}
        type="button"
        data-testid={`ai-nav-${section.id}`}
        aria-current={active ? "true" : undefined}
        onClick={() => handleSelectSection(section)}
        style={{
          width: isCompactNav ? "auto" : "100%",
          minWidth: isCompactNav ? 116 : undefined,
          height: isCompactNav ? 36 : undefined,
          flex: isCompactNav ? "0 0 auto" : undefined,
          display: "inline-flex",
          alignItems: "center",
          border: `1px solid ${
            active ? token.colorPrimaryBorder : token.colorBorderSecondary
          }`,
          borderRadius: 6,
          background: active ? token.colorPrimaryBg : token.colorBgContainer,
          color: active ? token.colorPrimaryText : token.colorTextSecondary,
          cursor: "pointer",
          font: "inherit",
          fontSize: 12,
          fontWeight: active ? 600 : 400,
          lineHeight: "18px",
          padding: "7px 10px",
          textAlign: "left",
          whiteSpace: "nowrap",
          transition: "background 0.2s, border-color 0.2s, color 0.2s",
        }}
      >
        {section.label}
      </button>
    );
  };

  const agentSections = sections.filter((section) => section.group === "agent");
  const imGatewaySections = sections.filter(
    (section) => section.group === "im-gateway",
  );
  const toolSections = sections.filter((section) => section.group === "tools");

  return (
    <div
      data-testid="ai-page-layout"
      style={{
        padding: "16px 16px 0",
        height: "100%",
        minHeight: 0,
        overflow: "hidden",
      }}
    >
      <div
        style={{
          display: "grid",
          gridTemplateColumns: isCompactNav ? "1fr" : "188px minmax(0, 1fr)",
          gridTemplateRows: isCompactNav ? "auto minmax(0, 1fr)" : undefined,
          gap: 16,
          height: "100%",
          minHeight: 0,
          overflow: "hidden",
        }}
      >
        <nav
          aria-label="AI sections"
          data-testid="ai-section-nav"
          style={{
            height: isCompactNav ? undefined : "100%",
            zIndex: 1,
            display: "flex",
            flexDirection: isCompactNav ? "row" : "column",
            gap: 12,
            alignItems: isCompactNav ? "center" : "stretch",
            overflowX: isCompactNav ? "auto" : undefined,
            overflowY: isCompactNav ? "hidden" : "auto",
            padding: isCompactNav ? "0 0 4px" : 0,
            minHeight: 0,
            maxHeight: "100%",
          }}
        >
          <div
            style={{
              display: "flex",
              flexDirection: isCompactNav ? "row" : "column",
              alignItems: isCompactNav ? "center" : "stretch",
              flex: isCompactNav ? "0 0 auto" : undefined,
              gap: 6,
            }}
          >
            <Text
              type="secondary"
              style={{
                display: "flex",
                alignItems: "center",
                gap: 6,
                fontSize: 11,
                fontWeight: 600,
                lineHeight: "18px",
                padding: isCompactNav ? "7px 2px" : "0 2px",
                textTransform: "uppercase",
                whiteSpace: "nowrap",
                flex: isCompactNav ? "0 0 auto" : undefined,
              }}
            >
              <ToolOutlined />
              Tools
            </Text>
            {toolSections.map(renderSectionButton)}
          </div>
          <div
            style={{
              display: "flex",
              flexDirection: isCompactNav ? "row" : "column",
              alignItems: isCompactNav ? "center" : "stretch",
              flex: isCompactNav ? "0 0 auto" : undefined,
              gap: 6,
            }}
          >
            <Text
              type="secondary"
              style={{
                display: "flex",
                alignItems: "center",
                gap: 6,
                fontSize: 11,
                fontWeight: 600,
                lineHeight: "18px",
                padding: isCompactNav ? "7px 2px" : "0 2px",
                textTransform: "uppercase",
                whiteSpace: "nowrap",
                flex: isCompactNav ? "0 0 auto" : undefined,
              }}
            >
              <RobotOutlined />
              Agent
            </Text>
            {agentSections.map(renderSectionButton)}
          </div>
          <div
            style={{
              display: "flex",
              flexDirection: isCompactNav ? "row" : "column",
              alignItems: isCompactNav ? "center" : "stretch",
              flex: isCompactNav ? "0 0 auto" : undefined,
              gap: 6,
            }}
          >
            <Text
              type="secondary"
              style={{
                display: "flex",
                alignItems: "center",
                gap: 6,
                fontSize: 11,
                fontWeight: 600,
                lineHeight: "18px",
                padding: isCompactNav ? "7px 2px" : "6px 2px 0",
                textTransform: "uppercase",
                whiteSpace: "nowrap",
                flex: isCompactNav ? "0 0 auto" : undefined,
              }}
            >
              <CloudOutlined />
              IM Gateway
            </Text>
            {imGatewaySections.map(renderSectionButton)}
          </div>
        </nav>

        <div
          data-testid="ai-section-content"
          style={{
            height: "100%",
            minHeight: 0,
            overflow: "hidden",
          }}
        >
          {activeSection.group === "agent" &&
          activeSection.section === "chat" ? (
            <AgentChatSection />
          ) : activeSection.group === "agent" ? (
            <AgentTab hideSectionNav />
          ) : activeSection.group === "im-gateway" ? (
            <ImGatewayTab hideSectionNav />
          ) : (
            <ASR />
          )}
        </div>
      </div>
    </div>
  );
}

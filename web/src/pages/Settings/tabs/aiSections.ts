export type AgentSectionId =
  | "chat"
  | "general"
  | "model"
  | "runtime"
  | "history"
  | "memories"
  | "skills"
  | "runners"
  | "memory-records"
  | "mcp-servers"
  | "sessions";

export const AGENT_SECTION_NAV: Array<{ id: AgentSectionId; label: string }> = [
  { id: "chat", label: "Chat" },
  { id: "general", label: "General" },
  { id: "model", label: "Model" },
  { id: "runtime", label: "Runtime" },
  { id: "history", label: "History" },
  { id: "memories", label: "Memories" },
  { id: "skills", label: "Skills" },
  { id: "runners", label: "Runners" },
  { id: "memory-records", label: "Memory Records" },
  { id: "mcp-servers", label: "MCP Servers" },
  { id: "sessions", label: "Sessions" },
];

export type ImGatewaySectionId =
  | "connections"
  | "targets"
  | "routes"
  | "schedules"
  | "history";

export const IM_GATEWAY_SECTION_NAV: Array<{
  id: ImGatewaySectionId;
  label: string;
}> = [
  { id: "connections", label: "Connections" },
  { id: "targets", label: "Targets" },
  { id: "routes", label: "Routes" },
  { id: "schedules", label: "Schedules" },
  { id: "history", label: "History" },
];

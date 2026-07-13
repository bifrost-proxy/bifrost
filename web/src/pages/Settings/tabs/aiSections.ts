export type AgentSectionId =
  | "chat"
  | "general"
  | "history"
  | "skills"
  | "runners"
  | "sessions";

export const AGENT_SECTION_NAV: Array<{ id: AgentSectionId; label: string }> = [
  { id: "chat", label: "Chat" },
  { id: "general", label: "General" },
  { id: "history", label: "History" },
  { id: "skills", label: "Skills" },
  { id: "runners", label: "Runners" },
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

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

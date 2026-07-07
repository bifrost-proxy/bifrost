import client from "./client";

export type FilterType = "client_ip" | "proxy_port" | "client_app" | "domain";

export interface PinnedFilter {
  id: string;
  type: FilterType;
  value: string;
  label: string;
}

export interface CollapsedSections {
  pinned: boolean;
  clientIp: boolean;
  proxyPort?: boolean;
  clientApp: boolean;
  domain: boolean;
}

export interface FilterPanelConfig {
  collapsed: boolean;
  width: number;
  collapsedSections: CollapsedSections;
}

export interface UiConfig {
  pinnedFilters: PinnedFilter[];
  filterPanel: FilterPanelConfig;
  detailPanelCollapsed: boolean;
  rulesSortMode: string;
}

export interface UpdateUiConfigRequest {
  pinnedFilters?: PinnedFilter[];
  filterPanel?: FilterPanelConfig;
  detailPanelCollapsed?: boolean;
  rulesSortMode?: string;
}

export async function getUiConfig(): Promise<UiConfig> {
  const response = await client.get<UiConfig>("/config/ui");
  return response.data;
}

export async function updateUiConfig(
  config: UpdateUiConfigRequest
): Promise<UiConfig> {
  const response = await client.put<UiConfig>("/config/ui", config);
  return response.data;
}

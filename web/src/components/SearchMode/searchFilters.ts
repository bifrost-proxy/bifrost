import { useFilterPanelStore } from "../../stores/useFilterPanelStore";
import {
  isFilterConditionApplicable,
  useTrafficStore,
} from "../../stores/useTrafficStore";
import type { SearchFilters } from "../../types";

export function buildSearchFilters(): SearchFilters {
  const { toolbarFilters, filterConditions } = useTrafficStore.getState();
  const panel = useFilterPanelStore.getState();
  const importedApps =
    toolbarFilters.imported.length > 0 ? ["Bifrost Import"] : [];

  return {
    protocols: toolbarFilters.protocol,
    status_ranges: toolbarFilters.status,
    content_types: toolbarFilters.type,
    has_rule_hit: toolbarFilters.rule.length > 0 ? true : undefined,
    conditions: filterConditions
      .filter(isFilterConditionApplicable)
      .map(({ field, operator, value }) => ({ field, operator, value })),
    client_ips: panel.selectedClientIps,
    client_apps: [...new Set([...panel.selectedClientApps, ...importedApps])],
    account_names: panel.selectedAccountNames,
    domains: panel.selectedDomains,
  };
}

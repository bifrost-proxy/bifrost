import type { TrafficSummary, ToolbarFilters, FilterCondition } from "../types";
import type { TrafficRecordsMutation } from "./trafficStoreTypes";
import {
  contentTypeMap,
  mergeSortedTrafficRecords,
} from "./trafficRecordModel";
export const isFilterConditionApplicable = (
  condition: FilterCondition,
): boolean => {
  return (
    condition.enabled !== false &&
    (condition.operator === "is_empty" ||
      condition.operator === "is_not_empty" ||
      condition.value.trim().length > 0)
  );
};

export const hasActiveFilters = (
  toolbar: ToolbarFilters,
  conditions: FilterCondition[],
): boolean => {
  return (
    toolbar.rule.length > 0 ||
    toolbar.protocol.length > 0 ||
    toolbar.status.length > 0 ||
    toolbar.type.length > 0 ||
    toolbar.imported.length > 0 ||
    conditions.some(isFilterConditionApplicable)
  );
};

interface CompiledCondition {
  field: string;
  operator: string;
  valueLower: string;
  regex: RegExp | null;
}

const compileConditions = (
  conditions: FilterCondition[],
): CompiledCondition[] => {
  return conditions.filter(isFilterConditionApplicable).map((c) => {
    let regex: RegExp | null = null;
    if (c.operator === "regex") {
      try {
        regex = new RegExp(c.value, "i");
      } catch {
        regex = null;
      }
    }
    return {
      field: c.field,
      operator: c.operator,
      valueLower: c.value.toLowerCase(),
      regex,
    };
  });
};

const matchRecord = (
  record: TrafficSummary,
  toolbar: ToolbarFilters,
  compiledConditions: CompiledCondition[],
  protocolSet: Set<string>,
  statusSet: Set<string>,
  typeSet: Set<string>,
): boolean => {
  if (toolbar.rule.length > 0 && !record.has_rule_hit) {
    return false;
  }

  if (toolbar.imported.length > 0) {
    const isImported =
      record.id.startsWith("OUT-") || record.client_app === "Bifrost Import";
    if (!isImported) {
      return false;
    }
  }

  if (protocolSet.size > 0) {
    const protocol = record.protocol?.toUpperCase() || "";
    let matched = false;
    if (protocolSet.has("H2") && protocol.includes("HTTP/2")) matched = true;
    else if (
      protocolSet.has("HTTP") &&
      (protocol === "HTTP/1.0" || protocol === "HTTP/1.1")
    )
      matched = true;
    else if (protocolSet.has("HTTPS") && protocol === "HTTPS") matched = true;
    else if (protocolSet.has("WS") && record.is_websocket && protocol === "WS")
      matched = true;
    else if (
      protocolSet.has("WSS") &&
      record.is_websocket &&
      protocol === "WSS"
    )
      matched = true;
    else if (protocolSet.has("H3") && (record.is_h3 || protocol === "H3"))
      matched = true;
    if (!matched) return false;
  }

  if (statusSet.size > 0) {
    const status = record.status;
    let matched = false;
    if (statusSet.has("error") && (status === 0 || status >= 500))
      matched = true;
    else if (statusSet.has("1xx") && status >= 100 && status < 200)
      matched = true;
    else if (statusSet.has("2xx") && status >= 200 && status < 300)
      matched = true;
    else if (statusSet.has("3xx") && status >= 300 && status < 400)
      matched = true;
    else if (statusSet.has("4xx") && status >= 400 && status < 500)
      matched = true;
    else if (statusSet.has("5xx") && status >= 500 && status < 600)
      matched = true;
    if (!matched) return false;
  }

  if (typeSet.size > 0) {
    const resContentType = (record.content_type || "").toLowerCase();
    const reqContentType = (record.request_content_type || "").toLowerCase();
    let matched = false;
    for (const t of typeSet) {
      const patterns = contentTypeMap[t] || [t.toLowerCase()];
      if (
        patterns.some(
          (pattern) =>
            resContentType.includes(pattern) ||
            reqContentType.includes(pattern),
        )
      ) {
        matched = true;
        break;
      }
    }
    if (!matched) return false;
  }

  for (const cond of compiledConditions) {
    let fieldValue = "";
    switch (cond.field) {
      case "url":
        fieldValue = `${record.host || ""}${record.path || ""}`;
        break;
      case "host":
        fieldValue = record.host || "";
        break;
      case "path":
        fieldValue = record.path || "";
        break;
      case "method":
        fieldValue = record.method || "";
        break;
      case "content_type":
        fieldValue = record.content_type || "";
        break;
      case "client_app":
        fieldValue = record.client_app || "";
        break;
      case "account_name":
        fieldValue = record.account_name || "";
        break;
      case "client_ip":
        fieldValue = record.client_ip || "";
        break;
      case "listener_port":
      case "port":
        fieldValue = record.listener_port ? String(record.listener_port) : "";
        break;
      default:
        continue;
    }

    const fieldValueLower = fieldValue.toLowerCase();
    let matched = false;

    switch (cond.operator) {
      case "contains":
        matched = fieldValueLower.includes(cond.valueLower);
        break;
      case "equals":
        matched = fieldValueLower === cond.valueLower;
        break;
      case "regex":
        matched = cond.regex ? cond.regex.test(fieldValue) : false;
        break;
      case "not_contains":
        matched = !fieldValueLower.includes(cond.valueLower);
        break;
      case "is_empty":
        matched = fieldValue.trim().length === 0;
        break;
      case "is_not_empty":
        matched = fieldValue.trim().length > 0;
        break;
      default:
        matched = fieldValueLower.includes(cond.valueLower);
    }

    if (!matched) return false;
  }

  return true;
};

export interface PanelFilters {
  clientIps: string[];
  proxyPorts: string[];
  clientApps: string[];
  accountNames: string[];
  domains: string[];
}

const hasPanelFilters = (panel: PanelFilters): boolean => {
  return (
    panel.clientIps.length > 0 ||
    panel.proxyPorts.length > 0 ||
    panel.clientApps.length > 0 ||
    panel.accountNames.length > 0 ||
    panel.domains.length > 0
  );
};

export const hasAnyTrafficFilters = (
  toolbar: ToolbarFilters,
  conditions: FilterCondition[],
  panel: PanelFilters,
): boolean => hasActiveFilters(toolbar, conditions) || hasPanelFilters(panel);

const matchPanelFilters = (
  record: TrafficSummary,
  panel: PanelFilters,
): boolean => {
  const clientIpMatch =
    panel.clientIps.length === 0 ||
    panel.clientIps.includes(record.client_ip || "");

  const proxyPortMatch =
    panel.proxyPorts.length === 0 ||
    panel.proxyPorts.includes(
      record.listener_port ? String(record.listener_port) : "",
    );

  const clientAppMatch =
    panel.clientApps.length === 0 ||
    panel.clientApps.includes(record.client_app || "");

  const accountNameMatch =
    panel.accountNames.length === 0 ||
    panel.accountNames.includes(record.account_name || "");

  const domainMatch =
    panel.domains.length === 0 ||
    panel.domains.some((domain) => (record.host || "").includes(domain));

  return (
    clientIpMatch &&
    proxyPortMatch &&
    clientAppMatch &&
    accountNameMatch &&
    domainMatch
  );
};

export const matchesTrafficFilters = (
  record: TrafficSummary,
  toolbar: ToolbarFilters,
  conditions: FilterCondition[],
  panel: PanelFilters = {
    clientIps: [],
    proxyPorts: [],
    clientApps: [],
    accountNames: [],
    domains: [],
  },
): boolean => {
  const hasToolbarOrConditions = hasActiveFilters(toolbar, conditions);
  const hasPanelActive = hasPanelFilters(panel);

  if (!hasToolbarOrConditions && !hasPanelActive) {
    return true;
  }

  const compiledConditions = compileConditions(conditions);
  const protocolSet = new Set(toolbar.protocol.map((p) => p.toUpperCase()));
  const statusSet = new Set(toolbar.status);
  const typeSet = new Set(toolbar.type);

  const toolbarMatch =
    !hasToolbarOrConditions ||
    matchRecord(
      record,
      toolbar,
      compiledConditions,
      protocolSet,
      statusSet,
      typeSet,
    );
  const panelMatch = !hasPanelActive || matchPanelFilters(record, panel);
  return toolbarMatch && panelMatch;
};

export const filterRecords = (
  records: TrafficSummary[],
  toolbar: ToolbarFilters,
  conditions: FilterCondition[],
  panel: PanelFilters = {
    clientIps: [],
    proxyPorts: [],
    clientApps: [],
    accountNames: [],
    domains: [],
  },
): TrafficSummary[] => {
  const hasToolbarOrConditions = hasActiveFilters(toolbar, conditions);
  const hasPanelActive = hasPanelFilters(panel);

  if (!hasToolbarOrConditions && !hasPanelActive) {
    return records;
  }

  const compiledConditions = compileConditions(conditions);
  const protocolSet = new Set(toolbar.protocol.map((p) => p.toUpperCase()));
  const statusSet = new Set(toolbar.status);
  const typeSet = new Set(toolbar.type);

  const result: TrafficSummary[] = [];
  for (const record of records) {
    const toolbarMatch =
      !hasToolbarOrConditions ||
      matchRecord(
        record,
        toolbar,
        compiledConditions,
        protocolSet,
        statusSet,
        typeSet,
      );
    const panelMatch = !hasPanelActive || matchPanelFilters(record, panel);

    if (toolbarMatch && panelMatch) {
      result.push(record);
    }
  }
  return result;
};

export const applyTrafficRecordsMutationToFilteredRecords = (
  current: TrafficSummary[],
  mutation: TrafficRecordsMutation,
  toolbar: ToolbarFilters,
  conditions: FilterCondition[],
  panel: PanelFilters = {
    clientIps: [],
    proxyPorts: [],
    clientApps: [],
    accountNames: [],
    domains: [],
  },
): TrafficSummary[] => {
  if (mutation.reset) {
    return current;
  }

  const oldestSequenceFloor = mutation.oldestSequenceFloor;
  let next =
    oldestSequenceFloor !== undefined && oldestSequenceFloor !== null
      ? current.filter((record) => record.sequence >= oldestSequenceFloor)
      : current;

  next =
    mutation.deletedIds.length > 0
      ? next.filter((record) => !mutation.deletedIds.includes(record.id))
      : next;

  if (mutation.updated.length > 0) {
    const updatedById = new Map(
      mutation.updated.map((record) => [record.id, record]),
    );
    const rebuilt: TrafficSummary[] = [];
    const promotedUpdates: TrafficSummary[] = [];

    for (const record of next) {
      const updated = updatedById.get(record.id);
      if (!updated) {
        rebuilt.push(record);
        continue;
      }

      if (matchesTrafficFilters(updated, toolbar, conditions, panel)) {
        rebuilt.push(updated);
      }
      updatedById.delete(record.id);
    }

    for (const updated of updatedById.values()) {
      if (matchesTrafficFilters(updated, toolbar, conditions, panel)) {
        promotedUpdates.push(updated);
      }
    }

    next =
      promotedUpdates.length > 0
        ? mergeSortedTrafficRecords(rebuilt, promotedUpdates)
        : rebuilt;
  }

  if (mutation.inserted.length === 0) {
    return next;
  }

  const matchingInserted = mutation.inserted.filter((record) =>
    matchesTrafficFilters(record, toolbar, conditions, panel),
  );

  if (matchingInserted.length === 0) {
    return next;
  }

  return mergeSortedTrafficRecords(next, matchingInserted);
};

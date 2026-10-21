import type { TFunction, TranslationKey } from "../../../i18n";

export type ImGatewaySectionId =
  | "connections"
  | "targets"
  | "routes"
  | "schedules"
  | "history";

/**
 * Section ids plus their translation keys. `label` is the English fallback
 * kept for existing consumers; render `t(section.labelKey)` (or use
 * `getImGatewaySectionNav(t)`) so the label follows the active language.
 */
export const IM_GATEWAY_SECTION_NAV: Array<{
  id: ImGatewaySectionId;
  label: string;
  labelKey: TranslationKey;
}> = [
  { id: "connections", label: "Connections", labelKey: "ai.imSection.connections" },
  { id: "targets", label: "Targets", labelKey: "ai.imSection.targets" },
  { id: "routes", label: "Routes", labelKey: "ai.imSection.routes" },
  { id: "schedules", label: "Schedules", labelKey: "ai.imSection.schedules" },
  { id: "history", label: "History", labelKey: "ai.imSection.history" },
];

export function getImGatewaySectionNav(
  t: TFunction,
): Array<{ id: ImGatewaySectionId; label: string }> {
  return IM_GATEWAY_SECTION_NAV.map(({ id, labelKey }) => ({
    id,
    label: t(labelKey),
  }));
}

import { useCallback, useEffect, useRef, useState, type CSSProperties } from "react";
import {
  Alert,
  Badge,
  Button,
  Card,
  Checkbox,
  Col,
  Descriptions,
  Divider,
  Empty,
  Form,
  Input,
  InputNumber,
  List,
  Modal,
  Popconfirm,
  Radio,
  Row,
  Select,
  Space,
  Spin,
  Switch,
  Tag,
  Tooltip,
  Typography,
  message,
  theme,
} from "antd";
import {
  ApiOutlined,
  CloudOutlined,
  CodeOutlined,
  CopyOutlined,
  DeleteOutlined,
  DisconnectOutlined,
  EditOutlined,
  ExportOutlined,
  EyeOutlined,
  FileOutlined,
  HistoryOutlined,
  KeyOutlined,
  LockOutlined,
  LoginOutlined,
  PlusOutlined,
  ReloadOutlined,
  SafetyOutlined,
  ScanOutlined,
  SearchOutlined,
  SettingOutlined,
  StopOutlined,
  ThunderboltOutlined,
} from "@ant-design/icons";
import {
  clearCalls,
  createRemoteInvokeSshKey,
  REMOTE_INVOKE_FILE_OPS,
  type RemoteInvokeFileOp,
  type RemoteInvokeSshKeySeedPolicy,
  enterDiscoveryMode,
  exitDiscoveryMode,
  getClientIdentity,
  getRemoteInvokeStatus,
  getRemoteInvokeSshKey,
  getRemoteInvokeSshPrivateKey,
  getRemoteShellConfig,
  listCalls,
  listGrants,
  refreshPairCode,
  resetRemoteInvokeSshKey,
  revokeGrant,
  revokeRemoteInvokeSshKey,
  updateGrant,
  updateRemoteShellConfig,
  matchRemoteShellCommand,
  getCallArgsPreviewSource,
  getCall,
  type CreateRemoteInvokeSshKeyInput,
  type Call,
  type ClientIdentity,
  type DiscoverySession,
  type Grant,
  type GrantMode,
  type RemoteInvokeSshCallerInfo,
  type RemoteInvokeSshKeyRecord,
  type RemoteInvokeSshKeySecretPayload,
  type RemoteInvokeStatus,
  type RemoteShellSet,
  type FileAccessScope,
  getFileAccessConfig,
  updateFileAccessConfig,
  type FileAccessConfig,
  type FileAccessGrantPolicy,
  type FileAccessPolicyAccess,
  type FileAccessPolicyRootScope,
  ALL_FILE_OPS,
  buildFileAccessPolicy,
  buildSshFileAccessPolicy,
  FILE_READ_OPS,
  findFileAccessPolicyForGrant,
  getFileAccessPolicyAccess,
  getFileAccessPolicyRootScope,
} from "../../../api/remoteInvoke";
import { isConnectionIssueError, isNotFoundError } from "../../../api/client";
import { copyToClipboard } from "../../../utils/clipboard";
import { usePairingRequestStore } from "../../../stores/usePairingRequestStore";
import PairingRequestModal from "../../../components/PairingRequestModal";
import type { SyncStatus } from "../../../api/sync";
import type { PairingRequest } from "../../../api/remoteInvoke";
import {
  getPowerStatus,
  setPowerOn,
  setPowerOff,
  setPowerMode,
  type KeepAwakeStatus,
  type KeepAwakeMode,
} from "../../../api/power";
import { useI18n, type TFunction } from "../../../i18n";

const { Text, Title } = Typography;
const { TextArea } = Input;
const { Option } = Select;

const recentCallItemStyle: CSSProperties = {
  cursor: "pointer",
  display: "block",
  paddingInline: 0,
};

const recentCallRowStyle: CSSProperties = {
  display: "grid",
  gridTemplateColumns: "minmax(0, 1fr) minmax(0, 500px) auto",
  alignItems: "center",
  gap: 8,
  width: "100%",
};

const recentCallPrimaryStyle: CSSProperties = {
  minWidth: 0,
  display: "flex",
  alignItems: "center",
  gap: 8,
  overflow: "hidden",
};

const recentCallMetaStyle: CSSProperties = {
  minWidth: 0,
  display: "flex",
  alignItems: "center",
  gap: 6,
  overflow: "hidden",
};

const recentCallTextEllipsisStyle: CSSProperties = {
  minWidth: 0,
  overflow: "hidden",
  textOverflow: "ellipsis",
  whiteSpace: "nowrap",
};

const callDetailPreStyle: CSSProperties = {
  margin: 0,
  maxHeight: 220,
  overflow: "auto",
  whiteSpace: "pre-wrap",
  wordBreak: "break-word",
};

function formatFingerprint(fp: string): string {
  if (!fp || fp.length < 16) return fp || "-";
  const short = fp.slice(0, 16);
  return `${short.slice(0, 4)}:${short.slice(4, 8)}:${short.slice(8, 12)}:${short.slice(12, 16)}`;
}

function formatArgsPreview(argsJson?: string | null): string {
  if (!argsJson) return "";
  try {
    const obj = JSON.parse(argsJson);
    return Object.entries(obj)
      .filter(([, v]) => v != null)
      .map(([k, v]) => `${k}=${typeof v === "string" ? v : JSON.stringify(v)}`)
      .join(" ");
  } catch {
    return argsJson.length > 60 ? argsJson.slice(0, 60) + "…" : argsJson;
  }
}

function getCallCommandPreview(call: Call): string {
  const summaryPreview = call.command_summary?.command_preview?.trim();
  const decryptedPreview = call.command?.command?.trim();
  const routeOnlyPreview =
    summaryPreview && call.command_kind && summaryPreview === call.command_kind;

  return (
    (!routeOnlyPreview && summaryPreview) ||
    decryptedPreview ||
    summaryPreview ||
    call.command_kind ||
    "-"
  );
}

function formatJsonForDisplay(value: unknown): string {
  if (value == null) return "-";
  if (typeof value === "string") {
    const trimmed = value.trim();
    if (!trimmed) return "-";
    try {
      return JSON.stringify(JSON.parse(trimmed), null, 2);
    } catch {
      return value;
    }
  }
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

function renderCallDetailBlock(value: unknown) {
  return (
    <pre style={callDetailPreStyle}>
      <Text code>{formatJsonForDisplay(value)}</Text>
    </pre>
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function getStringArrayField(
  metadata: Record<string, unknown> | undefined,
  key: string,
): string[] {
  const value = metadata?.[key];
  if (!Array.isArray(value)) return [];
  return value.filter((item): item is string => typeof item === "string");
}

function getStringField(
  metadata: Record<string, unknown> | undefined,
  key: string,
): string | undefined {
  const value = metadata?.[key];
  return typeof value === "string" && value.trim() ? value : undefined;
}

function getNumberField(
  metadata: Record<string, unknown> | undefined,
  key: string,
): number | undefined {
  const value = metadata?.[key];
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

type ShellExecMode = "argv_exec" | "shell_text";
type ShellAccessMode = "sandbox" | "full-access" | "custom";
const DEFAULT_SSH_KEY_SHELL_POLICY_ID = "ssh-key-full-access";

interface ShellPolicyEditorItem {
  id: string;
  name: string;
  description: string;
  enabled: boolean;
  profile_id?: string;
  exec_mode: ShellExecMode;
  allowed_executables: string[];
  allowed_shell_patterns: string[];
  cwd_allowlist: string[];
  env_allowlist: string[];
  default_cwd: string;
  max_timeout_ms: number | null;
  stdin_allowed: boolean;
  interactive_allowed: boolean;
  extra_metadata: Record<string, unknown>;
}

interface ShellProfileEditorItem {
  id: string;
  name: string;
  description: string;
  enabled: boolean;
  cwd_allowlist: string[];
  env_allowlist: string[];
  default_cwd: string;
  max_timeout_ms: number | null;
  stdin_allowed: boolean;
  interactive_allowed: boolean;
  extra_metadata: Record<string, unknown>;
}

interface ShellAccessCapability {
  allowed: boolean;
  text: string;
}

interface ShellAccessPresetDefinition {
  mode: Exclude<ShellAccessMode, "custom">;
  policies: ShellPolicyEditorItem[];
  profiles: ShellProfileEditorItem[];
}

const SHELL_POLICY_METADATA_KEYS = [
  "exec_mode",
  "allowed_executables",
  "allowed_shell_patterns",
  "cwd_allowlist",
  "env_allowlist",
  "default_cwd",
  "max_timeout_ms",
  "stdin_allowed",
  "interactive_allowed",
];

const SHELL_PROFILE_METADATA_KEYS = [
  "cwd_allowlist",
  "env_allowlist",
  "default_cwd",
  "max_timeout_ms",
  "stdin_allowed",
  "interactive_allowed",
];

function omitKnownMetadata(
  metadata: Record<string, unknown> | undefined,
  knownKeys: string[],
): Record<string, unknown> {
  if (!metadata) return {};
  return Object.fromEntries(
    Object.entries(metadata).filter(([key]) => !knownKeys.includes(key)),
  );
}

function nextShellItemId(prefix: string, existingIds: string[]): string {
  const seen = new Set(existingIds.map((item) => item.trim()).filter(Boolean));
  let nextIndex = existingIds.length + 1;
  let nextId = `${prefix}-${nextIndex}`;

  while (seen.has(nextId)) {
    nextIndex += 1;
    nextId = `${prefix}-${nextIndex}`;
  }

  return nextId;
}

function toShellPolicyEditorItem(
  policy: RemoteShellSet["policies"][number],
): ShellPolicyEditorItem {
  const metadata = isRecord(policy.metadata) ? policy.metadata : {};
  return {
    id: policy.id,
    name: policy.name,
    description: policy.description ?? "",
    enabled: policy.enabled,
    profile_id: policy.profile_id ?? undefined,
    exec_mode:
      getStringField(metadata, "exec_mode") === "shell_text"
        ? "shell_text"
        : "argv_exec",
    allowed_executables: getStringArrayField(metadata, "allowed_executables"),
    allowed_shell_patterns: getStringArrayField(metadata, "allowed_shell_patterns"),
    cwd_allowlist: getStringArrayField(metadata, "cwd_allowlist"),
    env_allowlist: getStringArrayField(metadata, "env_allowlist"),
    default_cwd: getStringField(metadata, "default_cwd") ?? "",
    max_timeout_ms: getNumberField(metadata, "max_timeout_ms") ?? null,
    stdin_allowed: Boolean(metadata.stdin_allowed),
    interactive_allowed: Boolean(metadata.interactive_allowed),
    extra_metadata: omitKnownMetadata(metadata, SHELL_POLICY_METADATA_KEYS),
  };
}

function toShellProfileEditorItem(
  profile: RemoteShellSet["profiles"][number],
): ShellProfileEditorItem {
  const metadata = isRecord(profile.metadata) ? profile.metadata : {};
  return {
    id: profile.id,
    name: profile.name,
    description: profile.description ?? "",
    enabled: profile.enabled,
    cwd_allowlist: getStringArrayField(metadata, "cwd_allowlist"),
    env_allowlist: getStringArrayField(metadata, "env_allowlist"),
    default_cwd: getStringField(metadata, "default_cwd") ?? "",
    max_timeout_ms: getNumberField(metadata, "max_timeout_ms") ?? null,
    stdin_allowed: Boolean(metadata.stdin_allowed),
    interactive_allowed: Boolean(metadata.interactive_allowed),
    extra_metadata: omitKnownMetadata(metadata, SHELL_PROFILE_METADATA_KEYS),
  };
}

function buildShellPolicyMetadata(
  item: ShellPolicyEditorItem,
): Record<string, unknown> {
  const metadata: Record<string, unknown> = {
    ...item.extra_metadata,
    exec_mode: item.exec_mode,
  };

  if (item.allowed_executables.length > 0) {
    metadata.allowed_executables = item.allowed_executables;
  }
  if (item.allowed_shell_patterns.length > 0) {
    metadata.allowed_shell_patterns = item.allowed_shell_patterns;
  }
  if (item.cwd_allowlist.length > 0) {
    metadata.cwd_allowlist = item.cwd_allowlist;
  }
  if (item.env_allowlist.length > 0) {
    metadata.env_allowlist = item.env_allowlist;
  }
  if (item.default_cwd.trim()) {
    metadata.default_cwd = item.default_cwd.trim();
  }
  if (item.max_timeout_ms != null) {
    metadata.max_timeout_ms = item.max_timeout_ms;
  }
  if (item.stdin_allowed) {
    metadata.stdin_allowed = true;
  }
  if (item.interactive_allowed) {
    metadata.interactive_allowed = true;
  }

  return metadata;
}

function buildShellProfileMetadata(
  item: ShellProfileEditorItem,
): Record<string, unknown> {
  const metadata: Record<string, unknown> = {
    ...item.extra_metadata,
  };

  if (item.cwd_allowlist.length > 0) {
    metadata.cwd_allowlist = item.cwd_allowlist;
  }
  if (item.env_allowlist.length > 0) {
    metadata.env_allowlist = item.env_allowlist;
  }
  if (item.default_cwd.trim()) {
    metadata.default_cwd = item.default_cwd.trim();
  }
  if (item.max_timeout_ms != null) {
    metadata.max_timeout_ms = item.max_timeout_ms;
  }
  if (item.stdin_allowed) {
    metadata.stdin_allowed = true;
  }
  if (item.interactive_allowed) {
    metadata.interactive_allowed = true;
  }

  return metadata;
}

const DEFAULT_SANDBOX_ENV_KEYS = ["PATH", "LANG", "LC_ALL", "TERM"];
const DEFAULT_SANDBOX_ENV = {
  PATH: "/usr/bin:/bin:/usr/sbin:/sbin",
  LANG: "C.UTF-8",
  LC_ALL: "C.UTF-8",
  TERM: "xterm-256color",
};

function createSandboxPreset(): ShellAccessPresetDefinition {
  return {
    mode: "sandbox",
    profiles: [
      {
        id: "default-sandbox",
        name: "Default Sandbox",
        description: "Baseline sandboxed shell execution.",
        enabled: true,
        cwd_allowlist: [],
        env_allowlist: [...DEFAULT_SANDBOX_ENV_KEYS],
        default_cwd: "",
        max_timeout_ms: 30000,
        stdin_allowed: false,
        interactive_allowed: false,
        extra_metadata: {
          inherit_env: false,
          default_env: DEFAULT_SANDBOX_ENV,
          reject_reason:
            "sandbox execution is not implemented yet on this target; choose Trusted (Full Access) or Custom Rules",
        },
      },
    ],
    policies: [
      {
        id: "default-sandbox",
        name: "Default Sandbox",
        description: "Run shell text inside the default sandbox constraints.",
        enabled: true,
        profile_id: "default-sandbox",
        exec_mode: "shell_text",
        allowed_executables: [],
        allowed_shell_patterns: ["^(?s:.*)$"],
        cwd_allowlist: [],
        env_allowlist: [],
        default_cwd: "",
        max_timeout_ms: null,
        stdin_allowed: false,
        interactive_allowed: false,
        extra_metadata: {
          shell: "/bin/bash",
          reject_reason:
            "sandbox execution is not implemented yet on this target; choose Trusted (Full Access) or Custom Rules",
        },
      },
    ],
  };
}

function createFullAccessPreset(): ShellAccessPresetDefinition {
  return {
    mode: "full-access",
    profiles: [],
    policies: [
      {
        id: "full-access",
        name: "Full Access",
        description: "Run any argv command or shell text with full caller access.",
        enabled: true,
        profile_id: undefined,
        exec_mode: "shell_text",
        allowed_executables: [],
        allowed_shell_patterns: ["^(?s:.*)$"],
        cwd_allowlist: [],
        env_allowlist: [],
        default_cwd: "",
        max_timeout_ms: null,
        stdin_allowed: true,
        interactive_allowed: true,
        extra_metadata: {
          shell: "/bin/bash",
          inherit_env: true,
          allowed_exec_modes: ["argv_exec", "shell_text"],
          allow_any_executable: true,
        },
      },
    ],
  };
}

const SHELL_ACCESS_PRESETS: ShellAccessPresetDefinition[] = [
  createSandboxPreset(),
  createFullAccessPreset(),
];

function shellPresetDefinition(
  mode: Exclude<ShellAccessMode, "custom">,
): ShellAccessPresetDefinition {
  return mode === "sandbox"
    ? createSandboxPreset()
    : createFullAccessPreset();
}

/** UI copy for a shell access preset (the preset data itself stays untranslated). */
function shellPresetDisplay(
  t: TFunction,
  mode: Exclude<ShellAccessMode, "custom">,
): { label: string; description: string; capabilities: ShellAccessCapability[] } {
  if (mode === "sandbox") {
    return {
      label: t("remoteInvoke.shellPreset.sandboxLabel"),
      description: t("remoteInvoke.shellPreset.sandboxDescription"),
      capabilities: [
        { allowed: true, text: t("remoteInvoke.shellPreset.sandboxCap1") },
        { allowed: false, text: t("remoteInvoke.shellPreset.sandboxCap2") },
        { allowed: false, text: t("remoteInvoke.shellPreset.sandboxCap3") },
        { allowed: false, text: t("remoteInvoke.shellPreset.sandboxCap4") },
      ],
    };
  }
  return {
    label: t("remoteInvoke.shellPreset.fullAccessLabel"),
    description: t("remoteInvoke.shellPreset.fullAccessDescription"),
    capabilities: [
      { allowed: true, text: t("remoteInvoke.shellPreset.fullAccessCap1") },
      { allowed: true, text: t("remoteInvoke.shellPreset.fullAccessCap2") },
      { allowed: true, text: t("remoteInvoke.shellPreset.fullAccessCap3") },
      { allowed: true, text: t("remoteInvoke.shellPreset.fullAccessCap4") },
      { allowed: false, text: t("remoteInvoke.shellPreset.fullAccessCap5") },
    ],
  };
}

function stableJson(value: unknown): string {
  if (Array.isArray(value)) {
    return `[${value.map((item) => stableJson(item)).join(",")}]`;
  }
  if (value && typeof value === "object") {
    return `{${Object.entries(value as Record<string, unknown>)
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([key, nested]) => `${JSON.stringify(key)}:${stableJson(nested)}`)
      .join(",")}}`;
  }
  return JSON.stringify(value);
}

function sameShellEditorItems<T>(left: T[], right: T[]): boolean {
  return stableJson(left) === stableJson(right);
}

function inferShellAccessMode(
  profiles: ShellProfileEditorItem[],
  policies: ShellPolicyEditorItem[],
): ShellAccessMode {
  for (const preset of SHELL_ACCESS_PRESETS) {
    if (
      sameShellEditorItems(profiles, preset.profiles) &&
      sameShellEditorItems(policies, preset.policies)
    ) {
      return preset.mode;
    }
  }
  return "custom";
}

function shellAccessModeLabel(t: TFunction, mode: ShellAccessMode): string {
  switch (mode) {
    case "sandbox":
      return t("remoteInvoke.shellMode.sandbox");
    case "full-access":
      return t("remoteInvoke.shellPreset.fullAccessLabel");
    default:
      return t("remoteInvoke.shellMode.custom");
  }
}

function formatBytes(bytes: number | null | undefined): string | null {
  if (bytes == null || bytes === 0) return null;
  if (bytes < 1024) return `${bytes}B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)}KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)}MB`;
}

function formatTimestamp(value: string | number | null | undefined): string {
  if (value == null || value === "") return "-";
  const date = typeof value === "number" ? new Date(value) : new Date(value);
  return Number.isNaN(date.getTime()) ? String(value) : date.toLocaleString();
}

function formatSshFingerprint(fingerprint?: string | null): string {
  if (!fingerprint) return "-";
  if (fingerprint.startsWith("SHA256:")) return fingerprint;
  return fingerprint.length > 48 ? `${fingerprint.slice(0, 24)}…` : fingerprint;
}

function formatCallerInfo(t: TFunction, info: RemoteInvokeSshCallerInfo | null): string {
  if (!info) return t("remoteInvoke.ssh.noCallerYet");

  const headline = [info.hostname, info.username && t("remoteInvoke.ssh.callerAsUser", { user: info.username })]
    .filter(Boolean)
    .join(" ");
  const details = [info.source_ip ?? info.ip, info.platform]
    .filter(Boolean)
    .join(" · ");

  return [headline, details].filter(Boolean).join(" — ") || t("remoteInvoke.ssh.noCallerYet");
}

function formatSshGrantMode(t: TFunction, mode: GrantMode): string {
  return mode === "permanent" ? t("remoteInvoke.grantMode.permanent") : mode;
}

function formatCountdown(t: TFunction, expiresAt: number): string {
  const remaining = Math.max(0, Math.floor((expiresAt - Date.now()) / 1000));
  if (remaining <= 0) return t("remoteInvoke.discovery.expired");
  const m = Math.floor(remaining / 60);
  const s = remaining % 60;
  return `${m}:${s.toString().padStart(2, "0")}`;
}

/**
 * Humanize a grant_scope value for display in UI tags.
 * Keeps the raw scope accessible via tooltip for power users.
 */
function describeGrantScope(t: TFunction, scope: string): { label: string; color: string } {
  switch (scope) {
    case "remote_query":
      return { label: t("remoteInvoke.grantScope.readOnly"), color: "default" };
    case "remote_shell_exec":
      return { label: t("remoteInvoke.grantScope.shellExec"), color: "purple" };
    case "remote_shell_interactive":
      return { label: t("remoteInvoke.grantScope.shellInteractive"), color: "magenta" };
    default:
      return { label: scope, color: "default" };
  }
}

/**
 * Humanize a grant_mode value for display in UI tags.
 * Keeps the raw mode accessible via tooltip.
 */
function describeGrantMode(t: TFunction, mode: string): string {
  switch (mode) {
    case "once":
      return t("remoteInvoke.grantMode.once");
    case "30m":
      return t("remoteInvoke.grantMode.30m");
    case "1h":
      return t("remoteInvoke.grantMode.1h");
    case "1d":
      return t("remoteInvoke.grantMode.1d");
    case "permanent":
      return t("remoteInvoke.grantMode.permanent");
    default:
      return mode;
  }
}

function describeGrantAuthMethod(
  t: TFunction,
  grant: Grant,
): { label: string; color: string; tooltip: string } {
  const method = grant.auth_method;
  if (method === "ssh_publickey" || grant.ssh_key_fingerprint || grant.ssh_key_id) {
    return {
      label: t("remoteInvoke.authMethod.sshKey"),
      color: "purple",
      tooltip: t("remoteInvoke.authMethod.sshKeyTooltip"),
    };
  }
  if (method === "pair_code" || method == null || method === "") {
    return {
      label: t("remoteInvoke.authMethod.pairCode"),
      color: "cyan",
      tooltip: t("remoteInvoke.authMethod.pairCodeTooltip"),
    };
  }
  return {
    label: method,
    color: "default",
    tooltip: `auth_method: ${method}`,
  };
}

/**
 * Humanize an exec_mode value (argv_exec / shell_text) for display.
 */
function describeExecMode(t: TFunction, mode: string): string {
  switch (mode) {
    case "argv_exec":
      return t("remoteInvoke.execMode.argvExec");
    case "shell_text":
      return t("remoteInvoke.execMode.shellText");
    default:
      return mode;
  }
}

function escapeRegExpForPrefix(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

// Build a "starts with" regex that allows the prefix followed by end-of-string,
// whitespace, or a non-word/non-slash separator. Stored in allowed_shell_patterns.
function buildPrefixPattern(prefix: string): string {
  const escaped = escapeRegExpForPrefix(prefix);
  return "^" + escaped + "(\\s|$)";
}

// If the stored pattern was produced by buildPrefixPattern, decode it back so
// the UI can show it as a plain prefix. Otherwise return null to signal "custom regex".
function tryDecodePrefix(pattern: string): string | null {
  const m = /^\^(.*)\(\\s\|\$\)$/.exec(pattern);
  if (!m) return null;
  const body = m[1];
  // Reject bodies that still contain unescaped regex metacharacters — means
  // the user hand-wrote a regex that happens to end with the same suffix.
  try {
    const unescaped = body.replace(/\\(.)/g, "$1");
    if (buildPrefixPattern(unescaped) !== pattern) return null;
    return unescaped;
  } catch {
    return null;
  }
}


function getGrantAccessMode(grant: Grant): "query" | "selected" | "all" {
  if (grant.grant_scope === "remote_query") {
    return "query";
  }
  const binding = isRecord(grant.policy_binding) ? grant.policy_binding : null;
  if (binding?.mode === "selected") {
    return "selected";
  }
  return "all";
}

function getGrantSelectedPolicies(grant: Grant): string[] {
  const binding = isRecord(grant.policy_binding) ? grant.policy_binding : null;
  if (!binding || binding.mode !== "selected" || !Array.isArray(binding.policy_ids)) {
    return [];
  }
  return binding.policy_ids.filter((item): item is string => typeof item === "string");
}

function getCallStatusColor(call: Call): string {
  switch (call.status) {
    case "completed":
      return call.exit_code === 0 ? "green" : "red";
    case "cancelled":
      return "orange";
    case "streaming":
    case "authorized":
    case "key_exchanged":
    case "pending":
    case "running":
      return "processing";
    case "failed":
    case "timeout":
      return "red";
    default:
      return "default";
  }
}

function renderStateTag(t: TFunction, state: string) {
  switch (state.toLowerCase()) {
    case "connected":
      return <Tag color="green">{t("remoteInvoke.state.connected")}</Tag>;
    case "connecting":
    case "registering":
      return <Tag color="processing">{t("remoteInvoke.state.connecting")}</Tag>;
    case "reconnecting":
      return <Tag color="orange">{t("remoteInvoke.state.reconnecting")}</Tag>;
    case "disconnected":
      return <Tag color="default">{t("remoteInvoke.state.disconnected")}</Tag>;
    default:
      return <Tag>{state}</Tag>;
  }
}

export interface RemoteInvokeTabProps {
  syncStatus?: SyncStatus | null;
  onGoToSyncTab?: () => void;
  onSyncSignIn?: () => void;
}

export default function RemoteInvokeTab({
  syncStatus = null,
  onGoToSyncTab,
  onSyncSignIn,
}: RemoteInvokeTabProps = {}) {
  const { t } = useI18n();
  const { token } = theme.useToken();
  const syncReady = !!syncStatus?.has_session;
  const lockedColStyle = syncReady
    ? undefined
    : {
        pointerEvents: "none" as const,
        opacity: 0.45,
        filter: "grayscale(0.2)",
      };
  const syncAccountLabel =
    syncStatus?.user?.email ||
    syncStatus?.user?.nickname ||
    syncStatus?.user?.user_id ||
    t("remoteInvoke.sync.signedIn");
  const [status, setStatus] = useState<RemoteInvokeStatus | null>(null);
  const [identity, setIdentity] = useState<ClientIdentity | null>(null);
  const [sshKey, setSshKey] = useState<RemoteInvokeSshKeyRecord | null>(null);
  const [sshApiAvailable, setSshApiAvailable] = useState(true);
  const [sshLoading, setSshLoading] = useState(false);
  const [sshAction, setSshAction] = useState<
    "create" | "download" | "reset" | "revoke" | null
  >(null);
  const [editorOpen, setEditorOpen] = useState(false);
  const [secretModal, setSecretModal] = useState<{
    title: string;
    description: string;
    payload: RemoteInvokeSshKeySecretPayload;
  } | null>(null);
  const [loading, setLoading] = useState(false);
  const [discoveryLoading, setDiscoveryLoading] = useState(false);
  const [countdown, setCountdown] = useState("");
  const [grants, setGrants] = useState<Grant[]>([]);
  const [calls, setCalls] = useState<Call[]>([]);
  const [shellConfig, setShellConfig] = useState<RemoteShellSet | null>(null);
  const [shellLoading, setShellLoading] = useState(false);
  const [shellEditorOpen, setShellEditorOpen] = useState(false);
  const [shellEditorMode, setShellEditorMode] = useState<ShellAccessMode>("custom");
  // Expert mode: when enabled, show raw Profiles & Policies dual-column view.
  // When disabled (default), the UI presents a unified "Command group" view
  // hiding the Profile/Policy distinction from end users.
  const [shellEditorExpertMode, setShellEditorExpertMode] = useState<boolean>(false);
  const [shellEditorTestInput, setShellEditorTestInput] = useState<Record<string, string>>({});
  const [shellEditorTestMode, setShellEditorTestMode] = useState<Record<string, "argv_exec" | "shell_text">>({});
  const [shellEditorTestResult, setShellEditorTestResult] = useState<
    Record<string, { matched: boolean; matched_policy_id: string | null; reason: string } | null>
  >({});
  const [shellEditorTestLoading, setShellEditorTestLoading] = useState<Record<string, boolean>>({});
  const [cliPreviewOpen, setCliPreviewOpen] = useState<boolean>(false);
  const [cliPreviewText, setCliPreviewText] = useState<string>("");
  const [powerStatus, setPowerStatus] = useState<KeepAwakeStatus | null>(null);
  const [powerLoading, setPowerLoading] = useState(false);
  const refreshPowerStatus = useCallback(async () => {
    try {
      const s = await getPowerStatus();
      setPowerStatus(s);
    } catch {
      // silent: unsupported OS returns error; ignore
    }
  }, []);

  const runShellPolicyTest = async (policyId: string) => {
    const command = (shellEditorTestInput[policyId] ?? "").trim();
    const execMode = shellEditorTestMode[policyId] ?? "shell_text";
    if (!command) {
      message.warning(t("remoteInvoke.shellTest.enterCommand"));
      return;
    }
    setShellEditorTestLoading((prev) => ({ ...prev, [policyId]: true }));
    try {
      const result = await matchRemoteShellCommand({
        command,
        exec_mode: execMode,
      });
      setShellEditorTestResult((prev) => ({ ...prev, [policyId]: result }));
    } catch (error) {
      setShellEditorTestResult((prev) => ({
        ...prev,
        [policyId]: {
          matched: false,
          matched_policy_id: null,
          reason: error instanceof Error ? error.message : String(error),
        },
      }));
    } finally {
      setShellEditorTestLoading((prev) => ({ ...prev, [policyId]: false }));
    }
  };

  const generateShellCliCommands = (
    profiles: ShellProfileEditorItem[],
    policies: ShellPolicyEditorItem[],
  ): string => {
    const sh = (v: string): string => {
      if (v === "") return "''" + "''";
      if (/^[A-Za-z0-9_@%+=:,./-]+$/.test(v)) return v;
      return "'" + v.replace(/'/g, "'\\''") + "'";
    };
    const lines: string[] = [];
    lines.push("# Generated from the current Shell Access editor state.");
    lines.push("# Run these on the TARGET device (the one receiving remote commands).");
    lines.push("");
    for (const pr of profiles) {
      const parts: string[] = [
        "bifrost remote shell profile add",
        "--id " + sh(pr.id),
        "--name " + sh(pr.name || pr.id),
      ];
      if (pr.description) parts.push("--description " + sh(pr.description));
      for (const c of pr.cwd_allowlist) if (c) parts.push("--cwd " + sh(c));
      for (const e of pr.env_allowlist) if (e) parts.push("--env " + sh(e));
      if (pr.default_cwd) parts.push("--default-cwd " + sh(pr.default_cwd));
      if (typeof pr.max_timeout_ms === "number")
        parts.push("--timeout-ms " + String(pr.max_timeout_ms));
      if (pr.stdin_allowed) parts.push("--stdin");
      if (pr.interactive_allowed) parts.push("--interactive");
      if (!pr.enabled) parts.push("--disabled");
      lines.push(parts.join(" \\\n  "));
      lines.push("");
    }
    for (const pl of policies) {
      const parts: string[] = [
        "bifrost remote shell policy add",
        "--id " + sh(pl.id),
        "--name " + sh(pl.name || pl.id),
        "--mode " + pl.exec_mode,
      ];
      if (pl.description) parts.push("--description " + sh(pl.description));
      if (pl.profile_id) parts.push("--profile " + sh(pl.profile_id));
      for (const ex of pl.allowed_executables) if (ex) parts.push("--program " + sh(ex));
      for (const pt of pl.allowed_shell_patterns) if (pt) parts.push("--pattern " + sh(pt));
      for (const c of pl.cwd_allowlist) if (c) parts.push("--cwd " + sh(c));
      for (const e of pl.env_allowlist) if (e) parts.push("--env " + sh(e));
      if (pl.default_cwd) parts.push("--default-cwd " + sh(pl.default_cwd));
      if (typeof pl.max_timeout_ms === "number")
        parts.push("--timeout-ms " + String(pl.max_timeout_ms));
      if (pl.stdin_allowed) parts.push("--stdin");
      if (pl.interactive_allowed) parts.push("--interactive");
      if (!pl.enabled) parts.push("--disabled");
      lines.push(parts.join(" \\\n  "));
      lines.push("");
    }
    return lines.join("\n");
  };

  const openCliPreview = () => {
    const text = generateShellCliCommands(shellEditorProfiles, shellEditorPolicies);
    setCliPreviewText(text);
    setCliPreviewOpen(true);
  };

  const [shellEditorPolicies, setShellEditorPolicies] = useState<
    ShellPolicyEditorItem[]
  >([]);
  // UI-only override: entries force the per-row Select to show "Regex"
  // even when the stored pattern is decodable as a prefix. Keyed by
  // `${policy.id}|${patIdx}`. Cleared when a pattern row is removed.
  const [shellPatternRegexOverride, setShellPatternRegexOverride] = useState<Set<string>>(
    () => new Set<string>(),
  );
  const [shellEditorProfiles, setShellEditorProfiles] = useState<
    ShellProfileEditorItem[]
  >([]);
  const [shellSaveLoading, setShellSaveLoading] = useState(false);
  const [fileAccessConfig, setFileAccessConfig] = useState<FileAccessConfig | null>(null);
  const [fileAccessLoading, setFileAccessLoading] = useState(false);
  const [fileAccessEditorOpen, setFileAccessEditorOpen] = useState(false);
  const [fileAccessEditingGrant, setFileAccessEditingGrant] = useState<Grant | null>(null);
  const [fileAccessDraft, setFileAccessDraft] = useState<FileAccessGrantPolicy | null>(null);
  const [fileAccessSaveLoading, setFileAccessSaveLoading] = useState(false);
  const [sshFilePolicyOpen, setSshFilePolicyOpen] = useState(false);
  const [sshFilePolicyAccess, setSshFilePolicyAccess] =
    useState<FileAccessPolicyAccess>("read_write");
  const [sshFilePolicyRootScope, setSshFilePolicyRootScope] =
    useState<FileAccessPolicyRootScope>("selected");
  const [sshFilePolicyRoots, setSshFilePolicyRoots] = useState("");
  const [sshFilePolicyAllowOverwrite, setSshFilePolicyAllowOverwrite] = useState(true);
  const [sshFilePolicyAllowRecursiveDelete, setSshFilePolicyAllowRecursiveDelete] =
    useState(false);
  const [sshFilePolicySaving, setSshFilePolicySaving] = useState(false);
  const [grantEditorOpen, setGrantEditorOpen] = useState(false);
  const [editingGrant, setEditingGrant] = useState<Grant | null>(null);
  const [grantEditorPreset, setGrantEditorPreset] = useState<"query" | "full" | "shell_only" | "file_only" | "custom">("full");
  const [grantEditorAccessMode, setGrantEditorAccessMode] = useState<"query" | "selected" | "all">("query");
  const [grantEditorSelectedPolicies, setGrantEditorSelectedPolicies] = useState<string[]>([]);
  const [grantEditorStdinAllowed, setGrantEditorStdinAllowed] = useState(false);
  const [grantEditorInteractiveAllowed, setGrantEditorInteractiveAllowed] = useState(false);
  const [grantEditorFileAccess, setGrantEditorFileAccess] = useState<FileAccessScope>("none");
  const [grantSaveLoading, setGrantSaveLoading] = useState(false);
  const [callDetailOpen, setCallDetailOpen] = useState(false);
  const [callDetailLoading, setCallDetailLoading] = useState(false);
  const [selectedCall, setSelectedCall] = useState<Call | null>(null);
  const pollRef = useRef<number | null>(null);
  // The form holds both the API fields and transient UI-only fields
  // (seed_roots / seed_ops / ...) that `handleSubmitSshForm` repacks
  // into a `RemoteInvokeSshKeySeedPolicy` before calling the API.
  type SshFormValues = Omit<CreateRemoteInvokeSshKeyInput, "seed_policy"> & {
    seed_roots?: string;
    seed_ops?: RemoteInvokeFileOp[];
    seed_allow_overwrite?: boolean;
    seed_allow_recursive_delete?: boolean;
  };
  const [sshForm] = Form.useForm<SshFormValues>();

  const [reviewPairing, setReviewPairing] = useState<PairingRequest | null>(null);
  const pendingPairings = usePairingRequestStore((s) => s.pendingList);
  const storeFetchPairings = usePairingRequestStore((s) => s.fetchPendingList);
  const enabledShellPolicies = shellConfig?.policies.filter((policy) => policy.enabled) ?? [];
  const activeGrants = grants.filter((grant) => grant.status === "active");
  const sshFilePolicy = sshKey
    ? fileAccessConfig?.grant?.find(
        (policy) =>
          policy.match?.ssh_fingerprint === sshKey.ssh_key_fingerprint,
      )
    : undefined;
  const sshFilePolicyAccessLabel = sshFilePolicy
    ? getFileAccessPolicyAccess(sshFilePolicy) === "read_write"
      ? t("remoteInvoke.fileAccess.readWriteShort")
      : t("remoteInvoke.fileAccess.readOnlyShort")
    : t("remoteInvoke.fileAccess.notConfigured");

  const refresh = useCallback(async () => {
    try {
      const [s, id] = await Promise.all([
        getRemoteInvokeStatus(),
        getClientIdentity(),
      ]);
      setStatus(s);
      setIdentity(id);
    } catch (e) {
      if (!isConnectionIssueError(e)) {
        console.error("Failed to fetch remote invoke status");
      }
    }
  }, []);

  const refreshGrants = useCallback(async () => {
    try {
      const res = await listGrants();
      const sorted = (res.grants ?? []).sort(
        (a, b) =>
          (b.first_connected_at ?? b.first_authorized_at ?? b.created_at ?? 0) -
          (a.first_connected_at ?? a.first_authorized_at ?? a.created_at ?? 0),
      );
      setGrants(sorted);
    } catch {
      // ignore
    }
  }, []);

  const refreshCalls = useCallback(async () => {
    try {
      const res = await listCalls({ limit: 100 });
      const sorted = (res.calls ?? []).sort((a, b) => (b.started_at ?? 0) - (a.started_at ?? 0));
      setCalls(sorted);
    } catch {
      // ignore
    }
  }, []);

  const handleClearCalls = useCallback(async () => {
    try {
      const res = await clearCalls();
      setCalls([]);
      setSelectedCall(null);
      message.success(t("remoteInvoke.calls.cleared", { count: res.removed ?? 0 }));
    } catch {
      message.error(t("remoteInvoke.calls.clearFailed"));
    }
  }, [t]);

  const handleOpenCallDetail = useCallback(async (call: Call) => {
    setSelectedCall(call);
    setCallDetailOpen(true);
    setCallDetailLoading(true);
    try {
      const res = await getCall(call.call_id);
      setSelectedCall(res.call);
    } catch (e) {
      message.warning(
        e instanceof Error
          ? t("remoteInvoke.calls.refreshDetailFailedWithReason", { reason: e.message })
          : t("remoteInvoke.calls.refreshDetailFailed"),
      );
    } finally {
      setCallDetailLoading(false);
    }
  }, [t]);

  const refreshShellConfig = useCallback(
    async (options?: { silent?: boolean }) => {
      const silent = options?.silent ?? false;
      if (!silent) {
        setShellLoading(true);
      }

      try {
        const config = await getRemoteShellConfig();
        setShellConfig(config);
      } catch (e) {
        if (!silent && !isConnectionIssueError(e)) {
          message.error(
            e instanceof Error ? e.message : t("remoteInvoke.shell.loadFailed"),
          );
        }
      } finally {
        if (!silent) {
          setShellLoading(false);
        }
      }
    },
    [t],
  );

  const refreshFileAccessConfig = useCallback(
    async (options?: { silent?: boolean }) => {
      const silent = options?.silent ?? false;
      if (!silent) {
        setFileAccessLoading(true);
      }
      try {
        const config = await getFileAccessConfig();
        setFileAccessConfig(config);
      } catch (e) {
        if (!silent && !isConnectionIssueError(e)) {
          message.error(
            e instanceof Error ? e.message : t("remoteInvoke.fileAccess.loadFailed"),
          );
        }
      } finally {
        if (!silent) {
          setFileAccessLoading(false);
        }
      }
    },
    [t],
  );

  const refreshSshKey = useCallback(
    async (options?: { silent?: boolean }) => {
      const silent = options?.silent ?? false;
      if (!silent) {
        setSshLoading(true);
      }

      try {
        const currentKey = await getRemoteInvokeSshKey();
        setSshApiAvailable(true);
        setSshKey(currentKey);
      } catch (e) {
        if (isNotFoundError(e)) {
          setSshApiAvailable(false);
          setSshKey(null);
          return;
        }
        if (!silent && !isConnectionIssueError(e)) {
          message.error(
            e instanceof Error ? e.message : t("remoteInvoke.ssh.loadFailed"),
          );
        }
      } finally {
        if (!silent) {
          setSshLoading(false);
        }
      }
    },
    [t],
  );

  useEffect(() => {
    void refresh();
    void refreshGrants();
    void refreshCalls();
    void refreshSshKey();
    void refreshShellConfig();
    void refreshFileAccessConfig();
  }, [refresh, refreshGrants, refreshCalls, refreshSshKey, refreshShellConfig, refreshFileAccessConfig]);

  useEffect(() => {
    void refreshPowerStatus();
    const id = window.setInterval(() => {
      void refreshPowerStatus();
    }, 10000);
    return () => window.clearInterval(id);
  }, [refreshPowerStatus]);

  useEffect(() => {
    pollRef.current = window.setInterval(() => {
      void refresh();
      void refreshGrants();
      void refreshCalls();
      void refreshShellConfig({ silent: true });
      void refreshFileAccessConfig({ silent: true });
      if (sshApiAvailable) {
        void refreshSshKey({ silent: true });
      }
    }, 3000);
    return () => {
      if (pollRef.current) window.clearInterval(pollRef.current);
    };
  }, [refresh, refreshGrants, refreshCalls, refreshSshKey, refreshShellConfig, refreshFileAccessConfig, sshApiAvailable]);

  useEffect(() => {
    const session = status?.discovery_session;
    if (!session) {
      setCountdown("");
      return;
    }
    const tick = () => {
      setCountdown(formatCountdown(t, session.expires_at));
    };
    tick();
    const timer = window.setInterval(tick, 1000);
    return () => window.clearInterval(timer);
  }, [status?.discovery_session, t]);

  const handleEnterDiscovery = async () => {
    setDiscoveryLoading(true);
    try {
      const res = await enterDiscoveryMode();
      setStatus((prev) =>
        prev ? { ...prev, discovery_session: res.session } : prev,
      );
      message.success(t("remoteInvoke.discovery.enabled"));
    } catch (e) {
      message.error(
        e instanceof Error ? e.message : t("remoteInvoke.discovery.enterFailed"),
      );
    } finally {
      setDiscoveryLoading(false);
    }
  };

  const handleExitDiscovery = async () => {
    setDiscoveryLoading(true);
    try {
      await exitDiscoveryMode();
      setStatus((prev) =>
        prev ? { ...prev, discovery_session: null } : prev,
      );
      message.success(t("remoteInvoke.discovery.disabled"));
    } catch (e) {
      message.error(
        e instanceof Error ? e.message : t("remoteInvoke.discovery.exitFailed"),
      );
    } finally {
      setDiscoveryLoading(false);
    }
  };

  const handleRefreshCode = async () => {
    setDiscoveryLoading(true);
    try {
      const res = await refreshPairCode();
      setStatus((prev) =>
        prev ? { ...prev, discovery_session: res.session } : prev,
      );
      message.success(t("remoteInvoke.discovery.codeRefreshed"));
    } catch (e) {
      message.error(
        e instanceof Error ? e.message : t("remoteInvoke.discovery.refreshCodeFailed"),
      );
    } finally {
      setDiscoveryLoading(false);
    }
  };

  const handleCopyCode = () => {
    const code = status?.discovery_session?.pair_code;
    if (code) {
      copyToClipboard(code);
      message.success(t("remoteInvoke.discovery.codeCopied"));
    }
  };

  const handleRevokeGrant = async (grantId: string) => {
    try {
      await revokeGrant(grantId);
      message.success(t("remoteInvoke.grants.revoked"));
      void refreshGrants();
      void refreshFileAccessConfig({ silent: true });
    } catch (e) {
      message.error(e instanceof Error ? e.message : t("remoteInvoke.grants.revokeFailed"));
    }
  };

  const openGrantEditor = (grant: Grant) => {
    const accessMode = getGrantAccessMode(grant);
    const fa = grant.file_access || "none";
    const isShell = accessMode === "selected" || accessMode === "all";
    const hasFile = fa === "read" || fa === "read_write";

    // Derive preset from current grant state
    let preset: "query" | "full" | "shell_only" | "file_only" | "custom";
    if (isShell && hasFile && grant.interactive_allowed) {
      preset = "full";
    } else if (isShell && hasFile && !grant.interactive_allowed) {
      preset = "shell_only";
    } else if (!isShell && hasFile) {
      preset = "file_only";
    } else if (!isShell && !hasFile) {
      preset = "query";
    } else {
      preset = "custom";
    }

    setEditingGrant(grant);
    setGrantEditorPreset(preset);
    setGrantEditorAccessMode(accessMode);
    setGrantEditorSelectedPolicies(getGrantSelectedPolicies(grant));
    setGrantEditorStdinAllowed(Boolean(grant.stdin_allowed));
    setGrantEditorInteractiveAllowed(Boolean(grant.interactive_allowed));
    setGrantEditorFileAccess(fa as FileAccessScope);
    setGrantEditorOpen(true);
  };

  const handleSaveGrant = async () => {
    if (!editingGrant) {
      return;
    }
    if (grantEditorPreset === "custom" && grantEditorAccessMode === "selected" && grantEditorSelectedPolicies.length === 0) {
      message.error(t("remoteInvoke.grants.choosePolicy"));
      return;
    }

    // Resolve preset to actual grant parameters
    const resolvePayload = () => {
      switch (grantEditorPreset) {
        case "query":
          return {
            grant_scope: "remote_query",
            file_access: "none" as const,
            policy_binding: null,
            interactive_allowed: false,
            stdin_allowed: false,
          };
        case "full":
          return {
            grant_scope: "remote_shell_interactive",
            file_access: "read_write" as const,
            policy_binding: { mode: "selected", policy_ids: [DEFAULT_SSH_KEY_SHELL_POLICY_ID] },
            interactive_allowed: true,
            stdin_allowed: true,
          };
        case "shell_only":
          return {
            grant_scope: "remote_shell_exec",
            file_access: "read_write" as const,
            policy_binding: { mode: "selected", policy_ids: [DEFAULT_SSH_KEY_SHELL_POLICY_ID] },
            interactive_allowed: false,
            stdin_allowed: false,
          };
        case "file_only":
          return {
            grant_scope: "remote_query",
            file_access: "read_write" as const,
            policy_binding: null,
            interactive_allowed: false,
            stdin_allowed: false,
          };
        case "custom":
          if (grantEditorAccessMode === "query") {
            return {
              grant_scope: "remote_query",
              file_access: grantEditorFileAccess,
              policy_binding: null,
              interactive_allowed: false,
              stdin_allowed: false,
            };
          }
          return {
            grant_scope: grantEditorInteractiveAllowed
              ? "remote_shell_interactive"
              : "remote_shell_exec",
            file_access: grantEditorFileAccess,
            policy_binding:
              grantEditorAccessMode === "all"
                ? { mode: "all" }
                : { mode: "selected", policy_ids: grantEditorSelectedPolicies },
            interactive_allowed: grantEditorInteractiveAllowed,
            stdin_allowed: grantEditorStdinAllowed,
          };
      }
    };

    const payload = resolvePayload();
    setGrantSaveLoading(true);
    try {
      await updateGrant(editingGrant.grant_id, payload);
      message.success(t("remoteInvoke.grants.updated"));
      setGrantEditorOpen(false);
      setEditingGrant(null);
      void refreshGrants();
    } catch (e) {
      const rawMsg = e instanceof Error ? e.message : t("remoteInvoke.grants.updateFailed");
      // Backend returns HTTP 409 when the grant is no longer active on the relay
      // (common when the caller re-paired and the old grant got rotated out).
      // Auto-refresh the list and surface a friendlier message.
      if (rawMsg.includes("grant_not_active") || rawMsg.includes("not found")) {
        message.warning(
          t("remoteInvoke.grants.noLongerActive"),
        );
        setGrantEditorOpen(false);
        setEditingGrant(null);
        void refreshGrants();
      } else {
        message.error(rawMsg);
      }
    } finally {
      setGrantSaveLoading(false);
    }
  };

  const openShellEditor = () => {
    const nextProfiles = shellConfig?.profiles.map(toShellProfileEditorItem) ?? [];
    const nextPolicies = shellConfig?.policies.map(toShellPolicyEditorItem) ?? [];
    setShellEditorProfiles(nextProfiles);
    setShellEditorPolicies(nextPolicies);
    setShellEditorMode(inferShellAccessMode(nextProfiles, nextPolicies));
    setShellEditorOpen(true);
  };

  const applyShellAccessMode = (mode: ShellAccessMode) => {
    setShellEditorMode(mode);
    if (mode === "custom") {
      return;
    }

    const preset = shellPresetDefinition(mode);
    setShellEditorProfiles(preset.profiles);
    setShellEditorPolicies(preset.policies);
  };

  const handleSaveShellConfig = async () => {
    const policyIds = new Set<string>();
    for (const policy of shellEditorPolicies) {
      if (!policy.id.trim()) {
        message.error(t("remoteInvoke.shell.policyNeedsId"));
        return;
      }
      if (!policy.name.trim()) {
        message.error(t("remoteInvoke.shell.policyNeedsName"));
        return;
      }
      if (policyIds.has(policy.id.trim())) {
        message.error(t("remoteInvoke.shell.duplicatePolicyId", { id: policy.id }));
        return;
      }
      policyIds.add(policy.id.trim());
    }

    const profileIds = new Set<string>();
    for (const profile of shellEditorProfiles) {
      if (!profile.id.trim()) {
        message.error(t("remoteInvoke.shell.profileNeedsId"));
        return;
      }
      if (!profile.name.trim()) {
        message.error(t("remoteInvoke.shell.profileNeedsName"));
        return;
      }
      if (profileIds.has(profile.id.trim())) {
        message.error(t("remoteInvoke.shell.duplicateProfileId", { id: profile.id }));
        return;
      }
      profileIds.add(profile.id.trim());
    }

    for (const policy of shellEditorPolicies) {
      if (policy.profile_id && !profileIds.has(policy.profile_id)) {
        message.error(t("remoteInvoke.shell.missingProfile", { name: policy.name }));
        return;
      }
    }

    const parsed: RemoteShellSet = {
      schema_version: shellConfig?.schema_version ?? 1,
      version: shellConfig?.version ?? 0,
      policies: shellEditorPolicies.map((policy) => ({
        id: policy.id.trim(),
        name: policy.name.trim(),
        description: policy.description.trim() || null,
        enabled: policy.enabled,
        profile_id: policy.profile_id?.trim() || null,
        metadata: buildShellPolicyMetadata(policy),
      })),
      profiles: shellEditorProfiles.map((profile) => ({
        id: profile.id.trim(),
        name: profile.name.trim(),
        description: profile.description.trim() || null,
        enabled: profile.enabled,
        metadata: buildShellProfileMetadata(profile),
      })),
    };

    setShellSaveLoading(true);
    try {
      const saved = await updateRemoteShellConfig(parsed);
      setShellConfig(saved);
      setShellEditorOpen(false);
      message.success(t("remoteInvoke.shell.saved"));
      void refreshGrants();
    } catch (e) {
      message.error(
        e instanceof Error ? e.message : t("remoteInvoke.shell.saveFailed"),
      );
    } finally {
      setShellSaveLoading(false);
    }
  };

  const openFileAccessEditor = (grant: Grant) => {
    const existing = findFileAccessPolicyForGrant(fileAccessConfig, grant);
    const fallbackAccess: FileAccessPolicyAccess =
      grant.file_access === "read_write" ? "read_write" : "read";
    setFileAccessEditingGrant(grant);
    setFileAccessDraft(
      existing
        ? { ...existing }
        : buildFileAccessPolicy(grant, fallbackAccess, "selected", []),
    );
    setFileAccessEditorOpen(true);
  };

  const openSshFilePolicyEditor = () => {
    if (!sshKey) return;
    const policy = sshFilePolicy;
    setSshFilePolicyAccess(
      policy ? getFileAccessPolicyAccess(policy) : "read_write",
    );
    setSshFilePolicyRootScope(
      policy ? getFileAccessPolicyRootScope(policy) : "selected",
    );
    setSshFilePolicyRoots((policy?.roots ?? []).join("\n"));
    setSshFilePolicyAllowOverwrite(policy?.allow_overwrite ?? true);
    setSshFilePolicyAllowRecursiveDelete(policy?.allow_recursive_delete ?? false);
    setSshFilePolicyOpen(true);
  };

  const handleSaveSshFilePolicy = async () => {
    if (!sshKey) return;
    const roots = sshFilePolicyRoots
      .split(/\r?\n|,/)
      .map((root) => root.trim())
      .filter(Boolean);
    if (sshFilePolicyRootScope === "selected" && roots.length === 0) {
      message.error(t("remoteInvoke.fileAccess.enterDirectory"));
      return;
    }

    const nextPolicy = buildSshFileAccessPolicy(
      sshKey.ssh_key_fingerprint,
      sshKey.label,
      sshFilePolicyAccess,
      sshFilePolicyRootScope,
      roots,
      sshFilePolicyAllowOverwrite,
      sshFilePolicyAllowRecursiveDelete,
    );
    const current = fileAccessConfig?.grant ?? [];
    const existingIndex = current.findIndex(
      (policy) =>
        policy.match?.ssh_fingerprint === sshKey.ssh_key_fingerprint,
    );
    const nextGrantPolicies =
      existingIndex >= 0
        ? current.map((policy, index) =>
            index === existingIndex ? nextPolicy : policy,
          )
        : [nextPolicy, ...current];

    setSshFilePolicySaving(true);
    try {
      const saved = await updateFileAccessConfig({
        grant: nextGrantPolicies,
      });
      setFileAccessConfig(saved);
      setSshFilePolicyOpen(false);
      message.success(t("remoteInvoke.ssh.filePolicySaved"));
    } catch (e) {
      message.error(
        e instanceof Error ? e.message : t("remoteInvoke.ssh.filePolicySaveFailed"),
      );
    } finally {
      setSshFilePolicySaving(false);
    }
  };

  const handleSaveFileAccessConfig = async () => {
    if (!fileAccessEditingGrant || !fileAccessDraft) {
      return;
    }
    const grantId = fileAccessEditingGrant.grant_id;
    if (!activeGrants.some((grant) => grant.grant_id === grantId)) {
      message.error(t("remoteInvoke.grants.notConnected", { id: grantId }));
      return;
    }
    if (
      getFileAccessPolicyRootScope(fileAccessDraft) === "selected" &&
      (fileAccessDraft.roots?.filter((root) => root.trim()).length ?? 0) === 0
    ) {
      message.error(t("remoteInvoke.fileAccess.enterDirectory"));
      return;
    }

    setFileAccessSaveLoading(true);
    try {
      const otherPolicies =
        fileAccessConfig?.grant?.filter((policy) => policy.grant_id !== grantId) ?? [];
      const nextPolicy = {
        ...fileAccessDraft,
        grant_id: grantId,
        match: undefined,
        name: fileAccessDraft.name?.trim() || undefined,
        roots: fileAccessDraft.roots?.map((root) => root.trim()).filter(Boolean) ?? [],
        denies:
          fileAccessDraft.denies?.map((deny) => deny.trim()).filter(Boolean) ?? [],
        write_denies:
          fileAccessDraft.write_denies?.map((deny) => deny.trim()).filter(Boolean) ?? [],
        ops:
          fileAccessDraft.ops && fileAccessDraft.ops.length > 0
            ? fileAccessDraft.ops
            : undefined,
      };
      const saved = await updateFileAccessConfig({
        grant: [...otherPolicies, nextPolicy],
      });
      setFileAccessConfig(saved);
      setFileAccessEditorOpen(false);
      setFileAccessEditingGrant(null);
      setFileAccessDraft(null);
      message.success(t("remoteInvoke.fileAccess.saved"));
    } catch (e) {
      message.error(
        e instanceof Error ? e.message : t("remoteInvoke.fileAccess.saveFailed"),
      );
    } finally {
      setFileAccessSaveLoading(false);
    }
  };

  const openCreateModal = () => {
    sshForm.setFieldsValue({
      label: sshKey?.label ?? "",
      grant_mode: "permanent",
    });
    setEditorOpen(true);
  };

  const presentSecretPayload = (
    payload: RemoteInvokeSshKeySecretPayload,
    mode: "created" | "downloaded" | "reset",
  ) => {
    const titleMap = {
      created: t("remoteInvoke.ssh.created"),
      downloaded: t("remoteInvoke.ssh.keyFileTitle"),
      reset: t("remoteInvoke.ssh.rotated"),
    };
    const descriptionMap = {
      created: t("remoteInvoke.ssh.createdDescription"),
      downloaded: t("remoteInvoke.ssh.downloadedDescription"),
      reset: t("remoteInvoke.ssh.rotatedDescription"),
    };

    setSecretModal({
      title: titleMap[mode],
      description: descriptionMap[mode],
      payload,
    });
  };

  const handleSubmitSshForm = async () => {
    const values = await sshForm.validateFields();
    setSshAction("create");
    try {
      // Repack the flat form values into the API shape; only send
      // seed_policy when the user actually customized it.
      const rawRoots =
        typeof values.seed_roots === "string"
          ? (values.seed_roots as string)
          : "";
      const roots = rawRoots
        .split(",")
        .map((s) => s.trim())
        .filter((s) => s.length > 0);
      const ops = Array.isArray(values.seed_ops)
        ? (values.seed_ops as RemoteInvokeFileOp[])
        : [];
      const seedPolicy: RemoteInvokeSshKeySeedPolicy = {
        roots,
        ops,
        allow_overwrite: Boolean(values.seed_allow_overwrite),
        allow_recursive_delete: Boolean(
          values.seed_allow_recursive_delete,
        ),
      };
      const payload = await createRemoteInvokeSshKey({
        label: values.label,
        grant_mode: values.grant_mode,
        seed_policy: seedPolicy,
      });
      presentSecretPayload(payload, "created");
      message.success(t("remoteInvoke.ssh.created"));
      setEditorOpen(false);
      await refreshSshKey({ silent: true });
    } catch (e) {
      message.error(
        e instanceof Error ? e.message : t("remoteInvoke.ssh.saveFailed"),
      );
    } finally {
      setSshAction(null);
    }
  };

  const handleFetchPrivateKey = async () => {
    setSshAction("download");
    try {
      const payload = await getRemoteInvokeSshPrivateKey();
      presentSecretPayload(payload, "downloaded");
      const copied = await copyToClipboard(payload.bifrost_key_file);
      message.success(copied ? t("remoteInvoke.ssh.keyFileCopied") : t("remoteInvoke.ssh.keyFileLoaded"));
    } catch (e) {
      message.error(
        e instanceof Error ? e.message : t("remoteInvoke.ssh.fetchKeyFileFailed"),
      );
    } finally {
      setSshAction(null);
    }
  };

  const handleResetSshKey = async () => {
    setSshAction("reset");
    try {
      const payload = await resetRemoteInvokeSshKey();
      presentSecretPayload(payload, "reset");
      await refreshSshKey({ silent: true });
      void refreshGrants();
      message.success(t("remoteInvoke.ssh.rotated"));
    } catch (e) {
      message.error(
        e instanceof Error ? e.message : t("remoteInvoke.ssh.rotateFailed"),
      );
    } finally {
      setSshAction(null);
    }
  };

  const handleRevokeSshKey = async () => {
    setSshAction("revoke");
    try {
      await revokeRemoteInvokeSshKey();
      setSshKey(null);
      void refreshGrants();
      message.success(t("remoteInvoke.ssh.revoked"));
    } catch (e) {
      message.error(
        e instanceof Error ? e.message : t("remoteInvoke.ssh.revokeFailed"),
      );
    } finally {
      setSshAction(null);
    }
  };

  const discoverySession: DiscoverySession | null =
    status?.discovery_session ?? null;
  const pairingList = pendingPairings;
  const enabledPolicyCount =
    shellConfig?.policies.filter((policy) => policy.enabled).length ?? 0;
  const enabledProfileCount =
    shellConfig?.profiles.filter((profile) => profile.enabled).length ?? 0;
  const shellAccessMode =
    shellConfig && (shellConfig.policies.length > 0 || shellConfig.profiles.length > 0)
      ? inferShellAccessMode(
          shellConfig.profiles.map(toShellProfileEditorItem),
          shellConfig.policies.map(toShellPolicyEditorItem),
        )
      : "custom";
  const shellProfileOptions = shellEditorProfiles.map((profile) => ({
    label: `${profile.name || profile.id} (${profile.id})`,
    value: profile.id,
  }));

  return (
    <div data-testid="settings-remote-invoke-tab" style={{ paddingBottom: 20 }}>
      <Row gutter={[16, 16]}>
        <Col xs={24}>
          <Alert
            showIcon
            type="info"
            message={t("remoteInvoke.intro.title")}
            description={t("remoteInvoke.intro.description")}
          />
        </Col>

        <Col xs={24} md={12} style={{ display: "flex" }}>
          <Card
            data-testid="settings-remote-invoke-status-card"
            title={
              <Space>
                <ApiOutlined />
                <span>{t("remoteInvoke.status.title")}</span>
              </Space>
            }
            extra={
              <Button
                size="small"
                icon={<ReloadOutlined />}
                onClick={() => {
                  setLoading(true);
                  void refresh().finally(() => setLoading(false));
                }}
                loading={loading}
              />
            }
            size="small"
            style={{ width: "100%", height: "100%" }}
            bodyStyle={{
              height: "100%",
              display: "flex",
              flexDirection: "column",
              gap: 16,
            }}
          >
            {!syncReady ? (
              <div
                data-testid="settings-remote-invoke-sync-signin-prompt"
                style={{
                  padding: "20px 16px",
                  background: token.colorWarningBg,
                  border: `1px solid ${token.colorWarningBorder}`,
                  borderRadius: 6,
                  display: "flex",
                  flexDirection: "column",
                  gap: 12,
                }}
              >
                <Space align="start" size={12}>
                  <LockOutlined
                    style={{ fontSize: 22, color: token.colorWarning, marginTop: 2 }}
                  />
                  <div style={{ flex: 1 }}>
                    <Text
                      strong
                      style={{
                        color: token.colorWarningText,
                        display: "block",
                        fontSize: 14,
                      }}
                    >
                      {t("remoteInvoke.sync.signInTitle")}
                    </Text>
                    <Text type="secondary" style={{ fontSize: 12 }}>
                      {t("remoteInvoke.sync.signInDescription")}
                    </Text>
                  </div>
                </Space>
                {syncStatus?.remote_base_url ? (
                  <Descriptions size="small" column={1}>
                    <Descriptions.Item label={t("remoteInvoke.sync.server")}>
                      <Text code style={{ fontSize: 11 }}>
                        {syncStatus.remote_base_url}
                      </Text>
                    </Descriptions.Item>
                  </Descriptions>
                ) : null}
                <Space wrap>
                  <Button
                    type="primary"
                    icon={<LoginOutlined />}
                    onClick={() => {
                      onSyncSignIn?.();
                    }}
                    disabled={!onSyncSignIn}
                    data-testid="settings-remote-invoke-sync-signin-btn"
                  >
                    {t("remoteInvoke.sync.signIn")}
                  </Button>
                  <Button
                    icon={<ExportOutlined />}
                    onClick={() => {
                      onGoToSyncTab?.();
                    }}
                    disabled={!onGoToSyncTab}
                    data-testid="settings-remote-invoke-sync-goto-btn"
                  >
                    {t("remoteInvoke.sync.openSettings")}
                  </Button>
                </Space>
              </div>
            ) : (
            <div
              data-testid="settings-remote-invoke-connection-section"
              style={{
                paddingBottom: 16,
                borderBottom: "1px solid #f0f0f0",
              }}
            >
              <Text strong style={{ display: "block", marginBottom: 12 }}>
                {t("remoteInvoke.status.connectionStatus")}
              </Text>
              <Descriptions size="small" column={1}>
                <Descriptions.Item label={t("remoteInvoke.status.relayConnection")}>
                  {status ? renderStateTag(t, status.state) : <Tag>{t("remoteInvoke.status.loading")}</Tag>}
                </Descriptions.Item>
                <Descriptions.Item label={t("remoteInvoke.status.instanceId")}>
                  <Text code style={{ fontSize: 11 }}>
                    {identity?.instance_id ?? "-"}
                  </Text>
                </Descriptions.Item>
                <Descriptions.Item label={t("remoteInvoke.status.device")}>
                  {identity?.device_name ?? "-"} ({identity?.platform ?? "-"})
                </Descriptions.Item>
                <Descriptions.Item label={t("remoteInvoke.sync.account")}>
                  <Space size={4}>
                    <CloudOutlined style={{ color: "#52c41a" }} />
                    <Text style={{ fontSize: 12 }}>{syncAccountLabel}</Text>
                    <Button
                      type="link"
                      size="small"
                      icon={<ExportOutlined />}
                      onClick={() => {
                        onGoToSyncTab?.();
                      }}
                      disabled={!onGoToSyncTab}
                      data-testid="settings-remote-invoke-sync-account-link"
                      style={{ padding: 0 }}
                    />
                  </Space>
                </Descriptions.Item>
              </Descriptions>
            </div>
            )}

            {syncReady && (
              <div
                data-testid="settings-remote-invoke-keepawake-section"
                style={{
                  paddingBottom: 16,
                  borderBottom: "1px solid #f0f0f0",
                }}
              >
                <Space
                  align="center"
                  style={{ width: "100%", justifyContent: "space-between", marginBottom: 8 }}
                >
                  <Text strong>
                    <ThunderboltOutlined style={{ marginRight: 6 }} />
                    {t("remoteInvoke.keepAwake.title")}
                  </Text>
                  <Switch
                    data-testid="settings-remote-invoke-keepawake-switch"
                    checked={!!powerStatus?.active}
                    loading={powerLoading}
                    disabled={!powerStatus || !powerStatus.supported}
                    onChange={async (checked) => {
                      setPowerLoading(true);
                      try {
                        if (checked) {
                          await setPowerOn();
                        } else {
                          await setPowerOff();
                        }
                        await refreshPowerStatus();
                      } catch (e) {
                        message.error(
                          e instanceof Error ? e.message : t("remoteInvoke.keepAwake.toggleFailed"),
                        );
                      } finally {
                        setPowerLoading(false);
                      }
                    }}
                  />
                </Space>
                {powerStatus && !powerStatus.supported ? (
                  <Alert
                    type="info"
                    showIcon
                    message={t("remoteInvoke.keepAwake.unsupported")}
                    style={{ marginTop: 4 }}
                  />
                ) : (
                  <>
                    <Text type="secondary" style={{ fontSize: 12, display: "block", marginBottom: 8 }}>
                      {t("remoteInvoke.keepAwake.description")}
                    </Text>
                    <Radio.Group
                      data-testid="settings-remote-invoke-keepawake-mode"
                      size="small"
                      value={powerStatus?.mode ?? "auto"}
                      disabled={powerLoading || !powerStatus}
                      onChange={async (e) => {
                        const next = e.target.value as KeepAwakeMode;
                        setPowerLoading(true);
                        try {
                          await setPowerMode(next);
                          await refreshPowerStatus();
                        } catch (err) {
                          message.error(
                            err instanceof Error ? err.message : t("remoteInvoke.keepAwake.setModeFailed"),
                          );
                        } finally {
                          setPowerLoading(false);
                        }
                      }}
                    >
                      <Radio.Button value="off">{t("remoteInvoke.keepAwake.modeOff")}</Radio.Button>
                      <Radio.Button value="auto">{t("remoteInvoke.keepAwake.modeAuto")}</Radio.Button>
                      <Radio.Button value="force_on">{t("remoteInvoke.keepAwake.modeForceOn")}</Radio.Button>
                    </Radio.Group>
                    {powerStatus ? (
                      <Descriptions size="small" column={1} style={{ marginTop: 8 }}>
                        <Descriptions.Item label={t("remoteInvoke.keepAwake.active")}>
                          {powerStatus.active ? (
                            <Tag color="green">{t("remoteInvoke.keepAwake.holding")}</Tag>
                          ) : (
                            <Tag>{t("remoteInvoke.keepAwake.idle")}</Tag>
                          )}
                        </Descriptions.Item>
                        {powerStatus.battery_warning ? (
                          <Descriptions.Item label={t("remoteInvoke.keepAwake.battery")}>
                            <Text type="warning" style={{ fontSize: 12 }}>{powerStatus.battery_warning}</Text>
                          </Descriptions.Item>
                        ) : null}
                      </Descriptions>
                    ) : null}
                  </>
                )}
              </div>
            )}

            {syncReady && (
            <div
              data-testid="settings-remote-invoke-discovery-section"
              style={{
                flex: 1,
                display: "flex",
                flexDirection: "column",
                minHeight: 180,
              }}
            >
              <Text strong style={{ display: "block", marginBottom: 12 }}>
                {t("remoteInvoke.discovery.title")}
              </Text>
              {discoverySession ? (
                <Space
                  direction="vertical"
                  size={12}
                  style={{
                    width: "100%",
                    flex: 1,
                    justifyContent: "flex-start",
                    paddingTop: 28,
                  }}
                >
                  <div style={{ textAlign: "center" }}>
                    <Title
                      level={2}
                      style={{
                        fontFamily: "monospace",
                        letterSpacing: "0.3em",
                        margin: 0,
                      }}
                    >
                      {discoverySession.pair_code}
                    </Title>
                    <Text type="secondary">
                      {countdown === t("remoteInvoke.discovery.expired") ? (
                        <Tag color="red">{t("remoteInvoke.discovery.expired")}</Tag>
                      ) : (
                        <>{t("remoteInvoke.discovery.expiresIn", { countdown })}</>
                      )}
                    </Text>
                  </div>
                  <Space wrap style={{ justifyContent: "center", width: "100%" }}>
                    <Button
                      icon={<CopyOutlined />}
                      onClick={handleCopyCode}
                      size="small"
                    >
                      {t("remoteInvoke.discovery.copyCode")}
                    </Button>
                    <Button
                      icon={<ReloadOutlined />}
                      onClick={handleRefreshCode}
                      loading={discoveryLoading}
                      size="small"
                    >
                      {t("common.refresh")}
                    </Button>
                    <Button
                      icon={<StopOutlined />}
                      onClick={handleExitDiscovery}
                      loading={discoveryLoading}
                      danger
                      size="small"
                    >
                      {t("remoteInvoke.discovery.exit")}
                    </Button>
                  </Space>
                </Space>
              ) : (
                <Space
                  direction="vertical"
                  size={12}
                  style={{
                    width: "100%",
                    flex: 1,
                    justifyContent: "flex-start",
                    paddingTop: 20,
                    textAlign: "center",
                  }}
                >
                  <DisconnectOutlined
                    style={{ fontSize: 32, color: "#bfbfbf" }}
                  />
                  <Text type="secondary">
                    {t("remoteInvoke.discovery.notActive")}
                  </Text>
                  <Button
                    type="primary"
                    icon={<ScanOutlined />}
                    onClick={handleEnterDiscovery}
                    loading={discoveryLoading}
                    disabled={status?.state?.toLowerCase() !== "connected"}
                  >
                    {t("remoteInvoke.discovery.enter")}
                  </Button>
                  {status?.state?.toLowerCase() !== "connected" && (
                    <Text type="warning" style={{ fontSize: 12 }}>
                      {t("remoteInvoke.discovery.relayRequired")}
                    </Text>
                  )}
                </Space>
              )}
            </div>
          )}
          </Card>
        </Col>

        <Col xs={24} md={12} style={{ display: "flex", ...(lockedColStyle || {}) }}>
          <Card
            data-testid="settings-remote-invoke-ssh-card"
            title={
              <Space>
                <KeyOutlined />
                <span>{t("remoteInvoke.ssh.title")}</span>
              </Space>
            }
            extra={
              <Button
                size="small"
                icon={<ReloadOutlined />}
                onClick={() => void refreshSshKey()}
                loading={sshLoading}
              />
            }
            size="small"
            style={{ width: "100%", height: "100%" }}
          >
            {!sshApiAvailable ? (
              <Alert
                showIcon
                type="warning"
                message={t("remoteInvoke.ssh.unavailableTitle")}
                description={t("remoteInvoke.ssh.unavailableDescription")}
              />
            ) : sshKey ? (
              <Space direction="vertical" size={16} style={{ width: "100%" }}>
                <Descriptions
                  size="small"
                  column={1}
                  items={[
                    {
                      key: "label",
                      label: t("remoteInvoke.ssh.label"),
                      children: sshKey.label,
                    },
                    {
                      key: "device_code",
                      label: t("remoteInvoke.ssh.deviceCode"),
                      children: (
                        <Space size={8} wrap>
                          <Text code>{sshKey.device_code}</Text>
                          <Button
                            size="small"
                            icon={<CopyOutlined />}
                            onClick={async () => {
                              const ok = await copyToClipboard(sshKey.device_code);
                              if (ok) message.success(t("remoteInvoke.ssh.deviceCodeCopied"));
                              else message.error(t("common.copyFailed"));
                            }}
                          >
                            {t("common.copy")}
                          </Button>
                        </Space>
                      ),
                    },
                    {
                      key: "fingerprint",
                      label: t("remoteInvoke.ssh.fingerprint"),
                      children: (
                        <Text code style={{ fontSize: 11 }}>
                          {formatSshFingerprint(sshKey.ssh_key_fingerprint)}
                        </Text>
                      ),
                    },
                    {
                      key: "grant_mode",
                      label: t("remoteInvoke.ssh.access"),
                      children: t("remoteInvoke.ssh.untilRevoke", { mode: formatSshGrantMode(t, sshKey.grant_mode) }),
                    },
                    {
                      key: "file_policy",
                      label: t("remoteInvoke.fileAccess.title"),
                      children: (
                        <Space size={8} wrap>
                          <Tag color={sshFilePolicy ? "orange" : "default"}>
                            {sshFilePolicyAccessLabel}
                          </Tag>
                          {sshFilePolicy?.roots?.length ? (
                            <Text type="secondary">
                              {getFileAccessPolicyRootScope(sshFilePolicy) === "all"
                                ? t("remoteInvoke.fileAccess.allDirectories")
                                : t("remoteInvoke.fileAccess.rootCount", { count: sshFilePolicy.roots.length })}
                            </Text>
                          ) : null}
                          <Button
                            size="small"
                            icon={<FileOutlined />}
                            onClick={openSshFilePolicyEditor}
                          >
                            {t("remoteInvoke.action.configure")}
                          </Button>
                        </Space>
                      ),
                    },
                    {
                      key: "status",
                      label: t("common.status"),
                      children: (
                        <Tag color={sshKey.status === "active" ? "green" : "default"}>
                          {sshKey.status}
                        </Tag>
                      ),
                    },
                    {
                      key: "last_used_at",
                      label: t("remoteInvoke.ssh.lastUsed"),
                      children: formatTimestamp(sshKey.last_used_at),
                    },
                    {
                      key: "last_caller",
                      label: t("remoteInvoke.ssh.lastCaller"),
                      children: (
                        <Text type={sshKey.last_caller_info ? undefined : "secondary"}>
                          {formatCallerInfo(t, sshKey.last_caller_info)}
                        </Text>
                      ),
                    },
                  ]}
                />

                <Space wrap>
                  <Button
                    icon={<CopyOutlined />}
                    onClick={() => void handleFetchPrivateKey()}
                    loading={sshAction === "download"}
                  >
                    {t("remoteInvoke.ssh.copyKeyFile")}
                  </Button>
                  <Popconfirm
                    title={t("remoteInvoke.ssh.rotateConfirmTitle")}
                    description={t("remoteInvoke.ssh.rotateConfirmDescription")}
                    okText={t("remoteInvoke.ssh.rotate")}
                    cancelText={t("common.cancel")}
                    onConfirm={() => void handleResetSshKey()}
                  >
                    <Button loading={sshAction === "reset"}>
                      {t("remoteInvoke.ssh.resetKey")}
                    </Button>
                  </Popconfirm>
                  <Popconfirm
                    title={t("remoteInvoke.ssh.revokeConfirmTitle")}
                    description={t("remoteInvoke.ssh.revokeConfirmDescription")}
                    okText={t("remoteInvoke.action.revoke")}
                    cancelText={t("common.cancel")}
                    onConfirm={() => void handleRevokeSshKey()}
                  >
                    <Button
                      danger
                      icon={<DeleteOutlined />}
                      loading={sshAction === "revoke"}
                    >
                      {t("remoteInvoke.ssh.revokeKey")}
                    </Button>
                  </Popconfirm>
                </Space>
              </Space>
            ) : (
              <Space
                direction="vertical"
                size={12}
                style={{ width: "100%", textAlign: "center" }}
              >
                <KeyOutlined style={{ fontSize: 32, color: "#bfbfbf" }} />
                <Text type="secondary">
                  {t("remoteInvoke.ssh.empty")}
                </Text>
                <Button
                  type="primary"
                  onClick={openCreateModal}
                  loading={sshAction === "create"}
                >
                  {t("remoteInvoke.ssh.create")}
                </Button>
              </Space>
            )}
          </Card>
        </Col>

        <Col xs={24} style={lockedColStyle}>
          <Card
            data-testid="settings-remote-invoke-shell-card"
            title={
              <Space>
                <SafetyOutlined />
                <span>{t("remoteInvoke.shell.title")}</span>
              </Space>
            }
            extra={
              <Space>
                <Button
                  size="small"
                  icon={<ReloadOutlined />}
                  onClick={() => void refreshShellConfig()}
                  loading={shellLoading}
                />
                <Button
                  size="small"
                  icon={<EditOutlined />}
                  onClick={openShellEditor}
                  disabled={shellLoading}
                  data-testid="settings-remote-invoke-manage-shell-access"
                >
                  {t("remoteInvoke.shell.manageAccess")}
                </Button>
              </Space>
            }
            size="small"
          >
            <Alert
              showIcon
              type="info"
              style={{ marginBottom: 16 }}
              message={t("remoteInvoke.shell.infoTitle")}
              description={t("remoteInvoke.shell.infoDescription")}
            />
            <Descriptions size="small" column={2} style={{ marginBottom: 16 }}>
              <Descriptions.Item label={t("remoteInvoke.shell.configurationMode")}>
                <Tag color={shellAccessMode === "custom" ? "default" : "blue"}>
                  {shellAccessModeLabel(t, shellAccessMode)}
                </Tag>
              </Descriptions.Item>
              <Descriptions.Item label={t("remoteInvoke.shell.policySetVersion")}>
                {shellConfig?.version ?? "-"}
              </Descriptions.Item>
              <Descriptions.Item label={t("remoteInvoke.shell.schemaVersion")}>
                {shellConfig?.schema_version ?? "-"}
              </Descriptions.Item>
              <Descriptions.Item label={t("remoteInvoke.shell.policies")}>
                {t("remoteInvoke.shell.totalEnabled", { total: shellConfig?.policies.length ?? 0, enabled: enabledPolicyCount })}
              </Descriptions.Item>
              <Descriptions.Item label={t("remoteInvoke.shell.profiles")}>
                {t("remoteInvoke.shell.totalEnabled", { total: shellConfig?.profiles.length ?? 0, enabled: enabledProfileCount })}
              </Descriptions.Item>
            </Descriptions>

            <Row gutter={[16, 16]}>
              <Col xs={24} lg={12}>
                <Card size="small" title={t("remoteInvoke.shell.policies")}>
                  {!shellConfig || shellConfig.policies.length === 0 ? (
                    <Empty
                      image={Empty.PRESENTED_IMAGE_SIMPLE}
                      description={t("remoteInvoke.shell.noPolicies")}
                    />
                  ) : (
                    <List
                      size="small"
                      dataSource={shellConfig.policies}
                      renderItem={(policy) => {
                        const metadata = isRecord(policy.metadata)
                          ? policy.metadata
                          : undefined;
                        const execMode = getStringField(metadata, "exec_mode");
                        const allowedExecutables = getStringArrayField(
                          metadata,
                          "allowed_executables",
                        );
                        const allowedShellPatterns = getStringArrayField(
                          metadata,
                          "allowed_shell_patterns",
                        );

                        return (
                          <List.Item>
                            <List.Item.Meta
                              title={
                                <Space wrap>
                                  <Text strong>{policy.name}</Text>
                                  <Tag color={policy.enabled ? "green" : "default"}>
                                    {policy.enabled ? t("remoteInvoke.state.enabled") : t("remoteInvoke.state.disabled")}
                                  </Tag>
                                  <Tag>{policy.id}</Tag>
                                  {execMode && <Tag color="blue">{execMode}</Tag>}
                                </Space>
                              }
                              description={
                                <Space size={4} wrap>
                                  {policy.profile_id && (
                                    <Text type="secondary" style={{ fontSize: 11 }}>
                                      {t("remoteInvoke.shell.profileRef", { id: policy.profile_id })}
                                    </Text>
                                  )}
                                  {allowedExecutables.length > 0 && (
                                    <Text type="secondary" style={{ fontSize: 11 }}>
                                      {t("remoteInvoke.shell.executablesCount", { count: allowedExecutables.length })}
                                    </Text>
                                  )}
                                  {allowedShellPatterns.length > 0 && (
                                    <Text type="secondary" style={{ fontSize: 11 }}>
                                      {t("remoteInvoke.shell.regexCount", { count: allowedShellPatterns.length })}
                                    </Text>
                                  )}
                                </Space>
                              }
                            />
                          </List.Item>
                        );
                      }}
                    />
                  )}
                </Card>
              </Col>

              <Col xs={24} lg={12}>
                <Card size="small" title={t("remoteInvoke.shell.profiles")}>
                  {!shellConfig || shellConfig.profiles.length === 0 ? (
                    <Empty
                      image={Empty.PRESENTED_IMAGE_SIMPLE}
                      description={t("remoteInvoke.shell.noProfiles")}
                    />
                  ) : (
                    <List
                      size="small"
                      dataSource={shellConfig.profiles}
                      renderItem={(profile) => {
                        const metadata = isRecord(profile.metadata)
                          ? profile.metadata
                          : undefined;
                        const cwdAllowlist = getStringArrayField(
                          metadata,
                          "cwd_allowlist",
                        );
                        const envAllowlist = getStringArrayField(
                          metadata,
                          "env_allowlist",
                        );
                        const defaultCwd = getStringField(metadata, "default_cwd");
                        const timeoutMs = getNumberField(metadata, "max_timeout_ms");

                        return (
                          <List.Item>
                            <List.Item.Meta
                              title={
                                <Space wrap>
                                  <Text strong>{profile.name}</Text>
                                  <Tag color={profile.enabled ? "green" : "default"}>
                                    {profile.enabled ? t("remoteInvoke.state.enabled") : t("remoteInvoke.state.disabled")}
                                  </Tag>
                                  <Tag>{profile.id}</Tag>
                                </Space>
                              }
                              description={
                                <Space size={4} wrap>
                                  {cwdAllowlist.length > 0 && (
                                    <Text type="secondary" style={{ fontSize: 11 }}>
                                      {t("remoteInvoke.shell.cwdCount", { count: cwdAllowlist.length })}
                                    </Text>
                                  )}
                                  {envAllowlist.length > 0 && (
                                    <Text type="secondary" style={{ fontSize: 11 }}>
                                      {t("remoteInvoke.shell.envCount", { count: envAllowlist.length })}
                                    </Text>
                                  )}
                                  {defaultCwd && (
                                    <Text type="secondary" style={{ fontSize: 11 }}>
                                      {t("remoteInvoke.shell.defaultCwdValue", { cwd: defaultCwd })}
                                    </Text>
                                  )}
                                  {timeoutMs != null && (
                                    <Text type="secondary" style={{ fontSize: 11 }}>
                                      {t("remoteInvoke.shell.timeoutValue", { ms: timeoutMs })}
                                    </Text>
                                  )}
                                </Space>
                              }
                            />
                          </List.Item>
                        );
                      }}
                    />
                  )}
                </Card>
              </Col>
            </Row>
          </Card>
        </Col>

        <Col xs={24} style={lockedColStyle}>
          <Card
            title={
              <Space>
                <span>{t("remoteInvoke.pairing.title")}</span>
                <Badge count={pairingList.length} />
              </Space>
            }
            extra={
              <Button
                size="small"
                icon={<ReloadOutlined />}
                onClick={() => void storeFetchPairings()}
              />
            }
            size="small"
          >
            {pairingList.length === 0 ? (
              <Empty
                image={Empty.PRESENTED_IMAGE_SIMPLE}
                description={t("remoteInvoke.pairing.empty")}
              />
            ) : (
              <List
                dataSource={pairingList}
                renderItem={(p) => (
                  <List.Item
                    actions={[
                      <Button
                        key="review"
                        type="primary"
                        size="small"
                        icon={<EyeOutlined />}
                        onClick={() => setReviewPairing(p)}
                      >
                        {t("remoteInvoke.pairing.review")}
                      </Button>,
                    ]}
                  >
                    <List.Item.Meta
                      avatar={
                        <Tooltip title={p.caller_info.fingerprint}>
                          <Tag color="blue" style={{ fontFamily: "monospace" }}>
                            {formatFingerprint(p.caller_info.fingerprint)}
                          </Tag>
                        </Tooltip>
                      }
                      title={
                        <Space>
                          <Text>
                            {p.caller_info.display_name || t("remoteInvoke.pairing.unknownCaller")}
                          </Text>
                          {p.caller_info.source_ip && (
                            <Text type="secondary" style={{ fontSize: 12 }}>
                              {t("remoteInvoke.pairing.fromIp", { ip: p.caller_info.source_ip })}
                            </Text>
                          )}
                        </Space>
                      }
                      description={
                        <Space size={4}>
                          <Tag>
                            {p.command_summary.command_preview || p.command.command}
                          </Tag>
                          {p.caller_info.platform && (
                            <Text type="secondary" style={{ fontSize: 11 }}>
                              {p.caller_info.platform}
                            </Text>
                          )}
                        </Space>
                      }
                    />
                  </List.Item>
                )}
              />
            )}
          </Card>
        </Col>

        <Col xs={24} style={lockedColStyle}>
          <Card
            data-testid="settings-remote-invoke-grants-card"
            title={
              <Space>
                <SafetyOutlined />
                <span>{t("remoteInvoke.grants.title")}</span>
                <Badge count={grants.filter((g) => g.status === "active").length} />
                <Text type="secondary" style={{ fontSize: 11, fontWeight: "normal" }}>
                  {t("remoteInvoke.grants.totalCommands", { count: grants.reduce((sum, g) => sum + (g.use_count ?? 0), 0) })}
                </Text>
              </Space>
            }
            extra={
              <Space size={4}>
                <Button
                  size="small"
                  icon={<ReloadOutlined />}
                  onClick={() => void refreshGrants()}
                />
              </Space>
            }
            size="small"
          >
            {grants.length === 0 ? (
              <Empty
                image={Empty.PRESENTED_IMAGE_SIMPLE}
                description={t("remoteInvoke.grants.empty")}
              />
            ) : (
              <List
                dataSource={grants}
                size="small"
                pagination={{
                  pageSize: 10,
                  size: "small",
                  hideOnSinglePage: true,
                  showSizeChanger: false,
                }}
                renderItem={(g) => {
                  const authMethod = describeGrantAuthMethod(t, g);
                  return (
                    <List.Item
                      actions={g.status === "removed" ? [] : [
                        <Button
                          key="file-access"
                          size="small"
                          icon={<FileOutlined />}
                          onClick={() => openFileAccessEditor(g)}
                          loading={fileAccessLoading}
                        >
                          {t("remoteInvoke.fileAccess.title")}
                        </Button>,
                        <Button
                          key="edit"
                          size="small"
                          icon={<EditOutlined />}
                          onClick={() => openGrantEditor(g)}
                        >
                          {t("remoteInvoke.grants.editAccess")}
                        </Button>,
                        <Popconfirm
                          key="revoke"
                          title={t("remoteInvoke.grants.revokeConfirmTitle")}
                          description={t("remoteInvoke.grants.revokeConfirmDescription")}
                          onConfirm={() => void handleRevokeGrant(g.grant_id)}
                          okText={t("remoteInvoke.action.revoke")}
                          cancelText={t("common.cancel")}
                        >
                          <Button
                            danger
                            size="small"
                            icon={<DeleteOutlined />}
                          >
                            {t("remoteInvoke.action.revoke")}
                          </Button>
                        </Popconfirm>,
                      ]}
                    >
                      <List.Item.Meta
                        title={
                          <Space>
                            <Text>{g.label || g.caller_display_name || formatFingerprint(g.caller_fingerprint)}</Text>
                            <Tag color={g.status === "active" ? "green" : "default"}>
                              {g.status}
                            </Tag>
                            <Tooltip title={authMethod.tooltip}>
                              <Tag color={authMethod.color}>{authMethod.label}</Tag>
                            </Tooltip>
                            <Tooltip title={`grant_mode: ${g.grant_mode}`}>
                              <Tag>{describeGrantMode(t, g.grant_mode)}</Tag>
                            </Tooltip>
                            <Tooltip title={`grant_scope: ${g.grant_scope}`}>
                              <Tag color={describeGrantScope(t, g.grant_scope).color}>
                                {describeGrantScope(t, g.grant_scope).label}
                              </Tag>
                            </Tooltip>
                            {g.file_access && g.file_access !== "none" && (
                              <Tag color="blue">
                                {g.file_access === "read_write" ? t("remoteInvoke.grants.fileRw") : t("remoteInvoke.grants.fileRead")}
                              </Tag>
                            )}
                            {g.shell_policy_set_version_snapshot != null && (
                              <Tag color="blue">
                                {t("remoteInvoke.grants.shellSetVersion", { version: g.shell_policy_set_version_snapshot })}
                              </Tag>
                            )}
                          </Space>
                        }
                        description={
                          <Space size={4} wrap>
                            <Text type="secondary" style={{ fontSize: 11 }}>
                              {t("remoteInvoke.grants.usedCount", { count: g.use_count ?? 0 })}
                            </Text>
                            <Text type="secondary" style={{ fontSize: 11 }}>
                              {t("remoteInvoke.grants.connectedAt", { time: formatTimestamp(g.first_connected_at ?? g.first_authorized_at ?? g.created_at) })}
                            </Text>
                            <Text type="secondary" style={{ fontSize: 11 }}>
                              {t("remoteInvoke.grants.lastCommandAt", { time: formatTimestamp(g.last_command_at) })}
                            </Text>
                            {(g.os_version || g.arch) && (
                              <Text type="secondary" style={{ fontSize: 11 }}>
                                · {[g.os_version, g.arch].filter(Boolean).join(" ")}
                              </Text>
                            )}
                            {g.expires_at != null && (
                              <Text type="secondary" style={{ fontSize: 11 }}>
                                {t("remoteInvoke.grants.expiresAt", { date: new Date(g.expires_at).toLocaleDateString() })}
                              </Text>
                            )}
                            <Tooltip title={g.caller_fingerprint}>
                              <Text type="secondary" style={{ fontSize: 10, fontFamily: "monospace" }}>
                                {formatFingerprint(g.caller_fingerprint)}
                              </Text>
                            </Tooltip>
                            {g.stdin_allowed != null && (
                              <Text type="secondary" style={{ fontSize: 11 }}>
                                {g.stdin_allowed ? t("remoteInvoke.grants.stdinOn") : t("remoteInvoke.grants.stdinOff")}
                              </Text>
                            )}
                            {g.interactive_allowed != null && (
                              <Text type="secondary" style={{ fontSize: 11 }}>
                                {g.interactive_allowed ? t("remoteInvoke.grants.interactiveOn") : t("remoteInvoke.grants.interactiveOff")}
                              </Text>
                            )}
                          </Space>
                        }
                      />
                    </List.Item>
                  );
                }}
              />
            )}
          </Card>
        </Col>

        <Col xs={24} style={lockedColStyle}>
          <Card
            title={
              <Space>
                <HistoryOutlined />
                <span>{t("remoteInvoke.calls.title")}</span>
                <Badge count={calls.length} />
              </Space>
            }
            extra={
              <Space size={4}>
                <Popconfirm
                  title={t("remoteInvoke.calls.clearConfirm")}
                  okText={t("common.clear")}
                  okButtonProps={{ danger: true }}
                  onConfirm={() => void handleClearCalls()}
                >
                  <Button
                    size="small"
                    danger
                    icon={<DeleteOutlined />}
                    disabled={calls.length === 0}
                  />
                </Popconfirm>
                <Button
                  size="small"
                  icon={<ReloadOutlined />}
                  onClick={() => void refreshCalls()}
                />
              </Space>
            }
            size="small"
          >
            {calls.length === 0 ? (
              <Empty
                image={Empty.PRESENTED_IMAGE_SIMPLE}
                description={t("remoteInvoke.calls.empty")}
              />
            ) : (
              <List
                dataSource={calls}
                size="small"
                pagination={{
                  pageSize: 10,
                  size: "small",
                  hideOnSinglePage: true,
                  showSizeChanger: false,
                }}
                renderItem={(c) => {
                  const argsPreviewSource = getCallArgsPreviewSource(c);
                  const argsPreview = formatArgsPreview(argsPreviewSource);
                  const commandPreview = getCallCommandPreview(c);
                  const bytesLabel = formatBytes(c.bytes_out);
                  return (
                      <List.Item
                        style={recentCallItemStyle}
                        onClick={() => void handleOpenCallDetail(c)}
                      >
                        <div style={recentCallRowStyle}>
                          <div style={{ minWidth: 0 }}>
                            <div style={recentCallPrimaryStyle}>
                              <Tooltip title={commandPreview}>
                                <Text
                                  code
                                  style={{
                                    ...recentCallTextEllipsisStyle,
                                    fontSize: 11,
                                  }}
                                >
                                  {commandPreview}
                                </Text>
                              </Tooltip>
                              {argsPreview && (
                                <Tooltip title={argsPreviewSource}>
                                  <Text
                                    type="secondary"
                                    style={{
                                      ...recentCallTextEllipsisStyle,
                                      flex: "0 1 260px",
                                      fontSize: 11,
                                    }}
                                  >
                                    {argsPreview}
                                  </Text>
                                </Tooltip>
                              )}
                            </div>
                            <Space size={4} style={{ marginTop: 4 }}>
                              <Text type="secondary" style={{ fontSize: 11 }}>
                                {new Date(c.started_at).toLocaleString()}
                              </Text>
                              {c.duration_ms != null && (
                                <Text type="secondary" style={{ fontSize: 11 }}>
                                  · {c.duration_ms}ms
                                </Text>
                              )}
                              {bytesLabel && (
                                <Text type="secondary" style={{ fontSize: 11 }}>
                                  · ↓ {bytesLabel}
                                </Text>
                              )}
                            </Space>
                          </div>

                          <div style={recentCallMetaStyle}>
                            <Tag color={getCallStatusColor(c)}>
                              {c.status}
                              {c.exit_code !== undefined && c.exit_code !== null
                                ? ` (${c.exit_code})`
                                : ""}
                            </Tag>
                            {c.caller_display_name && (
                              <Tooltip title={c.caller_display_name}>
                                <Text
                                  type="secondary"
                                  style={{
                                    ...recentCallTextEllipsisStyle,
                                    maxWidth: 220,
                                    fontSize: 11,
                                  }}
                                >
                                  {t("remoteInvoke.calls.byCaller", { caller: c.caller_display_name })}
                                </Text>
                              </Tooltip>
                            )}
                            {c.policy_id && (
                              <Tooltip title={c.policy_id}>
                                <Tag
                                  color="purple"
                                  style={{
                                    maxWidth: 120,
                                    overflow: "hidden",
                                    textOverflow: "ellipsis",
                                    whiteSpace: "nowrap",
                                  }}
                                >
                                  {c.policy_id}
                                </Tag>
                              </Tooltip>
                            )}
                            {c.exec_mode && (
                              <Tooltip title={`exec_mode: ${c.exec_mode}`}>
                                <Tag
                                  color="blue"
                                  style={{
                                    maxWidth: 140,
                                    overflow: "hidden",
                                    textOverflow: "ellipsis",
                                    whiteSpace: "nowrap",
                                  }}
                                >
                                  {describeExecMode(t, c.exec_mode)}
                                </Tag>
                              </Tooltip>
                            )}
                          </div>

                          <Tooltip title={t("remoteInvoke.calls.viewDetail")}>
                            <Button
                              size="small"
                              icon={<EyeOutlined />}
                              onClick={(event) => {
                                event.stopPropagation();
                                void handleOpenCallDetail(c);
                              }}
                            />
                          </Tooltip>
                        </div>
                      </List.Item>
                  );
                }}
              />
            )}
          </Card>
        </Col>
      </Row>
      <PairingRequestModal
        visible={reviewPairing !== null}
        pairing={reviewPairing}
        shellConfig={shellConfig}
        onClose={() => {
          setReviewPairing(null);
          void storeFetchPairings();
        }}
      />
      <Modal
        open={callDetailOpen}
        title={t("remoteInvoke.calls.detailTitle")}
        footer={null}
        width={820}
        onCancel={() => {
          setCallDetailOpen(false);
          setSelectedCall(null);
        }}
        destroyOnClose
      >
        <Spin spinning={callDetailLoading}>
          {selectedCall && (
            <Space direction="vertical" size={12} style={{ width: "100%" }}>
              <Descriptions
                size="small"
                column={2}
                bordered
                items={[
                  {
                    key: "call_id",
                    label: t("remoteInvoke.calls.callId"),
                    span: 2,
                    children: <Text code copyable>{selectedCall.call_id}</Text>,
                  },
                  {
                    key: "status",
                    label: t("common.status"),
                    children: (
                      <Tag color={getCallStatusColor(selectedCall)}>
                        {selectedCall.status}
                        {selectedCall.exit_code !== undefined &&
                        selectedCall.exit_code !== null
                          ? ` (${selectedCall.exit_code})`
                          : ""}
                      </Tag>
                    ),
                  },
                  {
                    key: "started_at",
                    label: t("remoteInvoke.calls.started"),
                    children: new Date(selectedCall.started_at).toLocaleString(),
                  },
                  {
                    key: "duration",
                    label: t("remoteInvoke.calls.duration"),
                    children:
                      selectedCall.duration_ms != null
                        ? `${selectedCall.duration_ms}ms`
                        : "-",
                  },
                  {
                    key: "bytes_out",
                    label: t("remoteInvoke.calls.bytesOut"),
                    children: formatBytes(selectedCall.bytes_out) ?? "-",
                  },
                  {
                    key: "grant_id",
                    label: t("remoteInvoke.calls.grant"),
                    span: 2,
                    children: <Text code>{selectedCall.grant_id}</Text>,
                  },
                  {
                    key: "client",
                    label: t("remoteInvoke.calls.client"),
                    span: 2,
                    children: <Text code>{selectedCall.client_instance_id}</Text>,
                  },
                  {
                    key: "caller",
                    label: t("remoteInvoke.calls.caller"),
                    span: 2,
                    children:
                      selectedCall.caller_display_name ||
                      formatFingerprint(selectedCall.caller_fingerprint),
                  },
                  {
                    key: "policy",
                    label: t("remoteInvoke.calls.policy"),
                    children: selectedCall.policy_id ? (
                      <Tag color="purple">{selectedCall.policy_id}</Tag>
                    ) : (
                      "-"
                    ),
                  },
                  {
                    key: "exec_mode",
                    label: t("remoteInvoke.calls.execMode"),
                    children: selectedCall.exec_mode ? (
                      <Tooltip title={`exec_mode: ${selectedCall.exec_mode}`}>
                        <Tag color="blue">{describeExecMode(t, selectedCall.exec_mode)}</Tag>
                      </Tooltip>
                    ) : (
                      "-"
                    ),
                  },
                ]}
              />
              <div>
                <Text strong>{t("remoteInvoke.calls.command")}</Text>
                {renderCallDetailBlock(getCallCommandPreview(selectedCall))}
              </div>
              <div>
                <Text strong>{t("remoteInvoke.calls.arguments")}</Text>
                {renderCallDetailBlock(getCallArgsPreviewSource(selectedCall))}
              </div>
              {selectedCall.command_detail && (
                <div>
                  <Text strong>{t("remoteInvoke.calls.commandDetail")}</Text>
                  {renderCallDetailBlock(selectedCall.command_detail)}
                </div>
              )}
              <div>
                <Text strong>{t("remoteInvoke.calls.rawCommand")}</Text>
                {renderCallDetailBlock(selectedCall.command)}
              </div>
            </Space>
          )}
        </Spin>
      </Modal>
      <Modal
        open={grantEditorOpen}
        title={t("remoteInvoke.grantEditor.title")}
        okText={t("common.save")}
        cancelText={t("common.cancel")}
        onCancel={() => {
          setGrantEditorOpen(false);
          setEditingGrant(null);
        }}
        onOk={() => void handleSaveGrant()}
        confirmLoading={grantSaveLoading}
        destroyOnClose
      >
        <Space direction="vertical" size={12} style={{ width: "100%" }}>
          <Descriptions
            size="small"
            column={1}
            bordered
            items={[
              {
                key: "grant",
                label: t("remoteInvoke.calls.grant"),
                children: <Text code>{editingGrant?.grant_id ?? "-"}</Text>,
              },
              {
                key: "caller",
                label: t("remoteInvoke.calls.caller"),
                children:
                  editingGrant?.caller_display_name ||
                  formatFingerprint(editingGrant?.caller_fingerprint ?? ""),
              },
            ]}
          />
          <Radio.Group
            value={grantEditorPreset}
            onChange={(e) => setGrantEditorPreset(e.target.value)}
            style={{ width: "100%" }}
          >
            <Space direction="vertical" style={{ width: "100%" }} size={2}>
              {([
                { value: "full" as const, icon: <ThunderboltOutlined />, name: t("remoteInvoke.grantEditor.fullName"), desc: t("remoteInvoke.grantEditor.fullDesc"), disabled: false },
                { value: "shell_only" as const, icon: <CodeOutlined />, name: t("remoteInvoke.grantEditor.shellOnlyName"), desc: t("remoteInvoke.grantEditor.shellOnlyDesc"), disabled: false },
                { value: "file_only" as const, icon: <FileOutlined />, name: t("remoteInvoke.grantEditor.fileOnlyName"), desc: t("remoteInvoke.grantEditor.fileOnlyDesc"), disabled: false },
                { value: "query" as const, icon: <SearchOutlined />, name: t("remoteInvoke.grantEditor.queryName"), desc: t("remoteInvoke.grantEditor.queryDesc"), disabled: false },
                { value: "custom" as const, icon: <SettingOutlined />, name: t("remoteInvoke.grantEditor.customName"), desc: t("remoteInvoke.grantEditor.customDesc"), disabled: false },
              ]).map((opt) => (
                <Radio key={opt.value} value={opt.value} disabled={opt.disabled} style={{ display: "flex", alignItems: "flex-start", width: "100%", padding: "6px 0" }}>
                  <div>
                    <div style={{ fontWeight: 600, lineHeight: "22px" }}>
                      <span style={{ color: "var(--ant-color-text-secondary)", marginRight: 6 }}>{opt.icon}</span>
                      {opt.name}
                    </div>
                    <Text type="secondary" style={{ fontSize: 12, marginLeft: 22 }}>{opt.desc}</Text>
                  </div>
                </Radio>
              ))}
            </Space>
          </Radio.Group>
          <Alert
            showIcon
            type={
              grantEditorPreset === "full"
                ? "warning"
                : grantEditorPreset === "query"
                ? "success"
                : "info"
            }
            message={t("remoteInvoke.grantEditor.previewTitle")}
            description={
              <ul style={{ margin: 0, paddingLeft: 18 }}>
                {(() => {
                  const bullets: string[] = [];
                  const shellAllowed =
                    grantEditorPreset === "full" ||
                    grantEditorPreset === "shell_only" ||
                    (grantEditorPreset === "custom" && grantEditorAccessMode !== "query");
                  const fileScope =
                    grantEditorPreset === "full" || grantEditorPreset === "shell_only"
                      ? "read_write"
                      : grantEditorPreset === "file_only"
                      ? "read_write"
                      : grantEditorPreset === "query"
                      ? "none"
                      : grantEditorFileAccess;
                  const interactive =
                    grantEditorPreset === "full" ||
                    (grantEditorPreset === "custom" && grantEditorInteractiveAllowed);
                  const stdin =
                    grantEditorPreset === "full" ||
                    (grantEditorPreset === "custom" && grantEditorStdinAllowed);

                  if (shellAllowed) {
                    if (grantEditorPreset === "full" || grantEditorPreset === "shell_only") {
                      bullets.push(t("remoteInvoke.grantEditor.bulletRunFullAccess"));
                    } else if (grantEditorPreset === "custom" && grantEditorAccessMode === "selected") {
                      const names = enabledShellPolicies
                        .filter((p) => grantEditorSelectedPolicies.includes(p.id))
                        .map((p) => p.name || p.id);
                      bullets.push(
                        names.length
                          ? t("remoteInvoke.grantEditor.bulletRunFrom", { names: names.join(", ") })
                          : t("remoteInvoke.grantEditor.bulletRunFromNone")
                      );
                    } else {
                      const names = enabledShellPolicies.map((p) => p.name || p.id);
                      bullets.push(
                        names.length
                          ? t("remoteInvoke.grantEditor.bulletRunFromAny", { names: names.join(", ") })
                          : t("remoteInvoke.grantEditor.bulletNoGroups")
                      );
                    }
                    bullets.push(
                      interactive
                        ? t("remoteInvoke.grantEditor.bulletInteractive")
                        : stdin
                        ? t("remoteInvoke.grantEditor.bulletStdin")
                        : t("remoteInvoke.grantEditor.bulletNoStdin")
                    );
                  } else {
                    bullets.push(t("remoteInvoke.grantEditor.bulletNoShell"));
                  }

                  if (fileScope === "read_write") {
                    bullets.push(t("remoteInvoke.grantEditor.bulletFileRw"));
                  } else if (fileScope === "read") {
                    bullets.push(t("remoteInvoke.grantEditor.bulletFileRead"));
                  } else {
                    bullets.push(t("remoteInvoke.grantEditor.bulletNoFile"));
                  }

                  bullets.push(t("remoteInvoke.grantEditor.bulletTraffic"));

                  return bullets.map((b, i) => <li key={i}>{b}</li>);
                })()}
              </ul>
            }
            style={{ marginTop: 4 }}
          />
          {grantEditorPreset === "custom" && (
            <div
              style={{
                padding: 12,
                background: "var(--ant-color-bg-container-disabled, #fafafa)",
                borderRadius: 8,
              }}
            >
              <Space direction="vertical" size={8} style={{ width: "100%" }}>
                <div>
                  <Text type="secondary" style={{ fontSize: 12 }}>{t("remoteInvoke.shell.title")}</Text>
                  <Select
                    value={grantEditorAccessMode}
                    onChange={setGrantEditorAccessMode}
                    style={{ width: "100%", marginTop: 4 }}
                    options={[
                      { value: "query", label: t("remoteInvoke.grantEditor.accessQuery") },
                      {
                        value: "selected",
                        label: t("remoteInvoke.grantEditor.accessSelected"),
                        disabled: enabledShellPolicies.length === 0,
                      },
                      {
                        value: "all",
                        label: t("remoteInvoke.grantEditor.accessAll"),
                        disabled: enabledShellPolicies.length === 0,
                      },
                    ]}
                  />
                </div>
                {grantEditorAccessMode === "selected" && (
                  <Select
                    mode="multiple"
                    placeholder={t("remoteInvoke.grantEditor.choosePolicies")}
                    value={grantEditorSelectedPolicies}
                    onChange={setGrantEditorSelectedPolicies}
                    style={{ width: "100%" }}
                    options={enabledShellPolicies.map((policy) => ({
                      value: policy.id,
                      label: `${policy.name} (${policy.id})`,
                    }))}
                  />
                )}
                {grantEditorAccessMode !== "query" && (
                  <Space>
                    <Text type="secondary">{t("remoteInvoke.shell.allowStdin")}</Text>
                    <Switch
                      checked={grantEditorStdinAllowed}
                      onChange={setGrantEditorStdinAllowed}
                    />
                    <Text type="secondary">{t("remoteInvoke.grantEditor.interactive")}</Text>
                    <Switch
                      checked={grantEditorInteractiveAllowed}
                      onChange={setGrantEditorInteractiveAllowed}
                    />
                  </Space>
                )}
                <div>
                  <Text type="secondary" style={{ fontSize: 12 }}>{t("remoteInvoke.fileAccess.title")}</Text>
                  <Select
                    value={grantEditorFileAccess}
                    onChange={setGrantEditorFileAccess}
                    style={{ width: "100%", marginTop: 4 }}
                    options={[
                      { value: "none", label: t("remoteInvoke.grantEditor.fileNone") },
                      { value: "read", label: t("remoteInvoke.grantEditor.fileRead") },
                      { value: "read_write", label: t("remoteInvoke.grantEditor.fileReadWrite") },
                    ]}
                  />
                </div>
              </Space>
            </div>
          )}
        </Space>
      </Modal>
      <Modal
        open={shellEditorOpen}
        title={t("remoteInvoke.shellEditor.title")}
        okText={t("common.save")}
        cancelText={t("common.cancel")}
        onCancel={() => setShellEditorOpen(false)}
        onOk={() => void handleSaveShellConfig()}
        confirmLoading={shellSaveLoading}
        width={960}
        destroyOnClose
      >
        <Alert
          showIcon
          type="warning"
          style={{ marginBottom: 16 }}
          message={t("remoteInvoke.shellEditor.warningTitle")}
          description={t("remoteInvoke.shellEditor.warningDescription")}
        />
        <Space direction="vertical" size={20} style={{ width: "100%" }}>
          <Card size="small" title={t("remoteInvoke.shellEditor.accessMode")}>
            <Space direction="vertical" size={12} style={{ width: "100%" }}>
              <Text type="secondary">
                {t("remoteInvoke.shellEditor.pickPresetPrefix")}
                <b>{t("remoteInvoke.shellMode.custom")}</b>
                {t("remoteInvoke.shellEditor.pickPresetSuffix")}
              </Text>
              <Radio.Group
                value={shellEditorMode}
                onChange={(e) => applyShellAccessMode(e.target.value)}
                optionType="button"
                buttonStyle="solid"
                style={{ display: "flex", flexWrap: "wrap", gap: 8 }}
              >
                <Radio.Button value="sandbox">{t("remoteInvoke.shellMode.sandbox")}</Radio.Button>
                <Radio.Button value="full-access">{t("remoteInvoke.shellPreset.fullAccessLabel")}</Radio.Button>
                <Radio.Button value="custom">{t("remoteInvoke.shellMode.custom")}</Radio.Button>
              </Radio.Group>
              {shellEditorMode === "custom" ? (
                <Alert
                  showIcon
                  type="info"
                  message={t("remoteInvoke.shellEditor.customTitle")}
                  description={
                    <div>
                      <div style={{ marginBottom: 4 }}>
                        {t("remoteInvoke.shellEditor.customIntro")}
                      </div>
                      <ul style={{ margin: 0, paddingLeft: 18 }}>
                        <li>
                          {t("remoteInvoke.shellEditor.customBullet1Prefix")}
                          <b>{t("remoteInvoke.shellEditor.customBullet1Bold")}</b>
                          {t("remoteInvoke.shellEditor.customBullet1Suffix")}
                        </li>
                        <li>{t("remoteInvoke.shellEditor.customBullet2")}</li>
                        <li>{t("remoteInvoke.shellEditor.customBullet3")}</li>
                      </ul>
                    </div>
                  }
                />
              ) : (
                <Alert
                  showIcon
                  type={shellEditorMode === "full-access" ? "warning" : "success"}
                  message={shellPresetDisplay(t, shellEditorMode).label}
                  description={
                    <div>
                      <div style={{ marginBottom: 6 }}>
                        {shellPresetDisplay(t, shellEditorMode).description}
                      </div>
                      <ul style={{ margin: 0, paddingLeft: 18 }}>
                        {shellPresetDisplay(t, shellEditorMode).capabilities.map((cap, idx) => (
                          <li
                            key={idx}
                            style={{
                              color: cap.allowed ? "var(--ant-color-success, #52c41a)" : "var(--ant-color-text-tertiary, #999)",
                            }}
                          >
                            <span style={{ fontWeight: 600, marginRight: 6 }}>
                              {cap.allowed ? "✓" : "✗"}
                            </span>
                            <span style={{ color: "var(--ant-color-text, inherit)" }}>{cap.text}</span>
                          </li>
                        ))}
                      </ul>
                    </div>
                  }
                />
              )}
            </Space>
          </Card>

          {shellEditorMode === "custom" ? (
          <>
          <div style={{ marginBottom: 8 }}>
            <Space>
              <Switch
                checked={shellEditorExpertMode}
                onChange={setShellEditorExpertMode}
                size="small"
              />
              <Text>{t("remoteInvoke.shellEditor.expertMode")}</Text>
              <Tooltip title={t("remoteInvoke.shellEditor.expertModeTooltip")}>
                <Text type="secondary" style={{ fontSize: 12, cursor: "help" }}>{t("remoteInvoke.shellEditor.whatIsThis")}</Text>
              </Tooltip>
              <Button size="small" onClick={openCliPreview}>
                {t("remoteInvoke.shellEditor.generateCli")}
              </Button>
            </Space>
          </div>
          {shellEditorExpertMode && (
          <div>
            <Space
              align="center"
              style={{ width: "100%", justifyContent: "space-between" }}
            >
              <Title level={5} style={{ margin: 0 }}>
                {t("remoteInvoke.shellEditor.executionEnvironments")}{" "}
                <Text type="secondary" style={{ fontWeight: 400, fontSize: 13 }}>{t("remoteInvoke.shellEditor.profilesHint")}</Text>
              </Title>
              <Button
                icon={<PlusOutlined />}
                onClick={() => {
                  const nextId = nextShellItemId(
                    "profile",
                    shellEditorProfiles.map((profile) => profile.id),
                  );
                  const nextIndex = shellEditorProfiles.length + 1;
                  setShellEditorProfiles((prev) => [
                    ...prev,
                    {
                      id: nextId,
                      name: `Profile ${nextIndex}`,
                      description: "",
                      enabled: true,
                      cwd_allowlist: [],
                      env_allowlist: [],
                      default_cwd: "",
                      max_timeout_ms: null,
                      stdin_allowed: false,
                      interactive_allowed: false,
                      extra_metadata: {},
                    },
                  ]);
                }}
              >
                {t("remoteInvoke.shellEditor.addExecutionEnvironment")}
              </Button>
            </Space>
            <Text type="secondary">
              <b>{t("remoteInvoke.shellEditor.executionEnvironments")}</b>
              {t("remoteInvoke.shellEditor.executionEnvironmentsHint")}
            </Text>
            <Divider style={{ margin: "12px 0" }} />

            <Space direction="vertical" size={12} style={{ width: "100%" }}>
              {shellEditorProfiles.length === 0 ? (
                <Empty
                  image={Empty.PRESENTED_IMAGE_SIMPLE}
                  description={t("remoteInvoke.shellEditor.noProfiles")}
                />
              ) : (
                shellEditorProfiles.map((profile, index) => (
                  <Card
                    key={`profile-editor-${profile.id}-${index}`}
                    size="small"
                    title={profile.name || profile.id || t("remoteInvoke.shellEditor.defaultProfileName", { index: index + 1 })}
                    extra={
                      <Button
                        size="small"
                        danger
                        icon={<DeleteOutlined />}
                        onClick={() =>
                          setShellEditorProfiles((prev) =>
                            prev.filter((_, current) => current !== index),
                          )
                        }
                      >
                        {t("common.remove")}
                      </Button>
                    }
                  >
                    <Row gutter={[12, 12]}>
                      <Col xs={24} md={10}>
                        <Text type="secondary">{t("remoteInvoke.shellEditor.profileName")}</Text>
                        <Input
                          value={profile.name}
                          onChange={(e) =>
                            setShellEditorProfiles((prev) =>
                              prev.map((item, current) =>
                                current === index
                                  ? { ...item, name: e.target.value }
                                  : item,
                              ),
                            )
                          }
                        />
                      </Col>
                      <Col xs={24} md={10}>
                        <Text type="secondary">{t("remoteInvoke.shellEditor.profileId")}</Text>
                        <Input
                          value={profile.id}
                          readOnly
                        />
                      </Col>
                      <Col xs={24} md={4}>
                        <Text type="secondary">{t("common.enabled")}</Text>
                        <div>
                          <Switch
                            checked={profile.enabled}
                            onChange={(checked) =>
                              setShellEditorProfiles((prev) =>
                                prev.map((item, current) =>
                                  current === index
                                    ? { ...item, enabled: checked }
                                    : item,
                                ),
                              )
                            }
                          />
                        </div>
                      </Col>
                      <Col xs={24}>
                        <Text type="secondary">{t("common.description")}</Text>
                        <Input
                          value={profile.description}
                          onChange={(e) =>
                            setShellEditorProfiles((prev) =>
                              prev.map((item, current) =>
                                current === index
                                  ? { ...item, description: e.target.value }
                                  : item,
                              ),
                            )
                          }
                        />
                      </Col>
                      <Col xs={24} md={12}>
                        <Text type="secondary">{t("remoteInvoke.shellEditor.allowedCwd")}</Text>
                        <Select
                          mode="tags"
                          style={{ width: "100%" }}
                          value={profile.cwd_allowlist}
                          onChange={(value) =>
                            setShellEditorProfiles((prev) =>
                              prev.map((item, current) =>
                                current === index
                                  ? { ...item, cwd_allowlist: value }
                                  : item,
                              ),
                            )
                          }
                          tokenSeparators={[","]}
                          placeholder={t("remoteInvoke.shellEditor.allowedCwdPlaceholder")}
                        />
                      </Col>
                      <Col xs={24} md={12}>
                        <Text type="secondary">{t("remoteInvoke.shellEditor.allowedEnv")}</Text>
                        <Select
                          mode="tags"
                          style={{ width: "100%" }}
                          value={profile.env_allowlist}
                          onChange={(value) =>
                            setShellEditorProfiles((prev) =>
                              prev.map((item, current) =>
                                current === index
                                  ? { ...item, env_allowlist: value }
                                  : item,
                              ),
                            )
                          }
                          tokenSeparators={[","]}
                          placeholder={t("remoteInvoke.shellEditor.allowedEnvPlaceholder")}
                        />
                      </Col>
                      <Col xs={24} md={12}>
                        <Text type="secondary">{t("remoteInvoke.shellEditor.defaultCwd")}</Text>
                        <Input
                          value={profile.default_cwd}
                          onChange={(e) =>
                            setShellEditorProfiles((prev) =>
                              prev.map((item, current) =>
                                current === index
                                  ? { ...item, default_cwd: e.target.value }
                                  : item,
                              ),
                            )
                          }
                        />
                      </Col>
                      <Col xs={24} md={12}>
                        <Text type="secondary">{t("remoteInvoke.shellEditor.maxTimeout")}</Text>
                        <InputNumber
                          style={{ width: "100%" }}
                          min={1}
                          value={profile.max_timeout_ms ?? undefined}
                          onChange={(value) =>
                            setShellEditorProfiles((prev) =>
                              prev.map((item, current) =>
                                current === index
                                  ? {
                                      ...item,
                                      max_timeout_ms:
                                        typeof value === "number"
                                          ? value
                                          : null,
                                    }
                                  : item,
                              ),
                            )
                          }
                          placeholder={t("common.optional")}
                        />
                      </Col>
                      <Col xs={12} md={6}>
                        <Text type="secondary">{t("remoteInvoke.shell.allowStdin")}</Text>
                        <div>
                          <Switch
                            checked={profile.stdin_allowed}
                            onChange={(checked) =>
                              setShellEditorProfiles((prev) =>
                                prev.map((item, current) =>
                                  current === index
                                    ? { ...item, stdin_allowed: checked }
                                    : item,
                                ),
                              )
                            }
                          />
                        </div>
                      </Col>
                      <Col xs={12} md={6}>
                        <Text type="secondary">{t("remoteInvoke.shellEditor.allowInteractive")}</Text>
                        <div>
                          <Switch
                            checked={profile.interactive_allowed}
                            onChange={(checked) =>
                              setShellEditorProfiles((prev) =>
                                prev.map((item, current) =>
                                  current === index
                                    ? { ...item, interactive_allowed: checked }
                                    : item,
                                ),
                              )
                            }
                          />
                        </div>
                      </Col>
                    </Row>
                  </Card>
                ))
              )}
            </Space>
          </div>
          )}
          <div>
            <Space
              align="center"
              style={{ width: "100%", justifyContent: "space-between" }}
            >
              <Title level={5} style={{ margin: 0 }}>
                {t("remoteInvoke.shellEditor.commandGroups")}
                {shellEditorExpertMode && (
                  <Text type="secondary" style={{ fontWeight: 400, fontSize: 13 }}>{t("remoteInvoke.shellEditor.policiesHint")}</Text>
                )}
              </Title>
              <Button
                icon={<PlusOutlined />}
                onClick={() => {
                  const nextId = nextShellItemId(
                    "policy",
                    shellEditorPolicies.map((policy) => policy.id),
                  );
                  const nextIndex = shellEditorPolicies.length + 1;
                  // In Simple mode, auto-create a matching profile so the new
                  // group has an execution environment attached.
                  let profileIdForNewGroup = shellEditorProfiles[0]?.id;
                  if (!shellEditorExpertMode) {
                    const profileId = nextShellItemId(
                      "profile",
                      shellEditorProfiles.map((profile) => profile.id),
                    );
                    profileIdForNewGroup = profileId;
                    setShellEditorProfiles((prev) => [
                      ...prev,
                      {
                        id: profileId,
                        name: `Profile ${nextIndex}`,
                        description: "Auto-created for command group",
                        enabled: true,
                        cwd_allowlist: [],
                        env_allowlist: [...DEFAULT_SANDBOX_ENV_KEYS],
                        default_cwd: "",
                        max_timeout_ms: 30000,
                        stdin_allowed: false,
                        interactive_allowed: false,
                        extra_metadata: {},
                      },
                    ]);
                  }
                  setShellEditorPolicies((prev) => [
                    ...prev,
                    {
                      id: nextId,
                      name: `Policy ${nextIndex}`,
                      description: "",
                      enabled: true,
                      profile_id: profileIdForNewGroup,
                      exec_mode: "argv_exec",
                      allowed_executables: [],
                      allowed_shell_patterns: [],
                      cwd_allowlist: [],
                      env_allowlist: [],
                      default_cwd: "",
                      max_timeout_ms: null,
                      stdin_allowed: false,
                      interactive_allowed: false,
                      extra_metadata: {},
                    },
                  ]);
                }}
              >
                {t("remoteInvoke.shellEditor.addCommandGroup")}
              </Button>
            </Space>
            <Text type="secondary">
              {t("remoteInvoke.shellEditor.policiesDescription")}
            </Text>
            <Divider style={{ margin: "12px 0" }} />

            <Space direction="vertical" size={12} style={{ width: "100%" }}>
              {shellEditorPolicies.length === 0 ? (
                <Empty
                  image={Empty.PRESENTED_IMAGE_SIMPLE}
                  description={t("remoteInvoke.shellEditor.noPolicies")}
                />
              ) : (
                shellEditorPolicies.map((policy, index) => (
                  <Card
                    key={`policy-editor-${policy.id}-${index}`}
                    size="small"
                    title={policy.name || policy.id || t("remoteInvoke.shellEditor.defaultPolicyName", { index: index + 1 })}
                    extra={
                      <Button
                        size="small"
                        danger
                        icon={<DeleteOutlined />}
                        onClick={() =>
                          setShellEditorPolicies((prev) =>
                            prev.filter((_, current) => current !== index),
                          )
                        }
                      >
                        {t("common.remove")}
                      </Button>
                    }
                  >
                    <Row gutter={[12, 12]}>
                      <Col xs={24} md={8}>
                        <Text type="secondary">{t("remoteInvoke.shellEditor.policyName")}</Text>
                        <Input
                          value={policy.name}
                          onChange={(e) =>
                            setShellEditorPolicies((prev) =>
                              prev.map((item, current) =>
                                current === index
                                  ? { ...item, name: e.target.value }
                                  : item,
                              ),
                            )
                          }
                        />
                      </Col>
                      <Col xs={24} md={8}>
                        <Text type="secondary">{t("remoteInvoke.shellEditor.policyId")}</Text>
                        <Input
                          value={policy.id}
                          readOnly
                        />
                      </Col>
                      <Col xs={24} md={4}>
                        <Text type="secondary">{t("common.enabled")}</Text>
                        <div>
                          <Switch
                            checked={policy.enabled}
                            onChange={(checked) =>
                              setShellEditorPolicies((prev) =>
                                prev.map((item, current) =>
                                  current === index
                                    ? { ...item, enabled: checked }
                                    : item,
                                ),
                              )
                            }
                          />
                        </div>
                      </Col>
                      <Col xs={24} md={6}>
                        <Tooltip title={t("remoteInvoke.shellEditor.matchByTooltip")}>
                          <Text type="secondary">{t("remoteInvoke.shellEditor.matchBy")}</Text>
                        </Tooltip>
                        <Select
                          style={{ width: "100%" }}
                          value={policy.exec_mode}
                          onChange={(value: ShellExecMode) =>
                            setShellEditorPolicies((prev) =>
                              prev.map((item, current) =>
                                current === index
                                  ? { ...item, exec_mode: value }
                                  : item,
                              ),
                            )
                          }
                        >
                          <Option value="argv_exec">{t("remoteInvoke.shellEditor.matchArgv")}</Option>
                          <Option value="shell_text">{t("remoteInvoke.shellEditor.matchShellText")}</Option>
                        </Select>
                      </Col>
                      <Col xs={24}>
                        <Text type="secondary">{t("common.description")}</Text>
                        <Input
                          value={policy.description}
                          onChange={(e) =>
                            setShellEditorPolicies((prev) =>
                              prev.map((item, current) =>
                                current === index
                                  ? { ...item, description: e.target.value }
                                  : item,
                              ),
                            )
                          }
                        />
                      </Col>
                      <Col xs={24} md={12}>
                        <Text type="secondary">{t("remoteInvoke.shellEditor.sandboxProfile")}</Text>
                        <Select
                          allowClear
                          style={{ width: "100%" }}
                          value={policy.profile_id}
                          options={shellProfileOptions}
                          onChange={(value) =>
                            setShellEditorPolicies((prev) =>
                              prev.map((item, current) =>
                                current === index
                                  ? { ...item, profile_id: value }
                                  : item,
                              ),
                            )
                          }
                          placeholder={t("remoteInvoke.shellEditor.chooseProfile")}
                        />
                      </Col>
                      <Col xs={24} md={12}>
                        <Text type="secondary">{t("remoteInvoke.shellEditor.allowedExecutables")}</Text>
                        <Select
                          mode="tags"
                          style={{ width: "100%" }}
                          value={policy.allowed_executables}
                          onChange={(value) =>
                            setShellEditorPolicies((prev) =>
                              prev.map((item, current) =>
                                current === index
                                  ? { ...item, allowed_executables: value }
                                  : item,
                              ),
                            )
                          }
                          tokenSeparators={[","]}
                          placeholder={t("remoteInvoke.shellEditor.allowedExecutablesPlaceholder")}
                        />
                      </Col>
                      <Col xs={24} md={12}>
                        <Tooltip title={t("remoteInvoke.shellEditor.patternsTooltip")}>
                          <Text type="secondary">{t("remoteInvoke.shellEditor.allowedPatterns")}</Text>
                        </Tooltip>
                        <Space direction="vertical" size={6} style={{ width: "100%" }}>
                          {policy.allowed_shell_patterns.length === 0 && (
                            <Text type="secondary" style={{ fontSize: 12 }}>
                              {t("remoteInvoke.shellEditor.noPatterns")}
                            </Text>
                          )}
                          {policy.allowed_shell_patterns.map((pat, patIdx) => {
                            const overrideKey = `${policy.id}|${patIdx}`;
                            const forcedRegex = shellPatternRegexOverride.has(overrideKey);
                            const decoded = forcedRegex ? null : tryDecodePrefix(pat);
                            const isPrefix = decoded !== null;
                            return (
                              <Space.Compact
                                key={`${policy.id}-pat-${patIdx}`}
                                style={{ width: "100%" }}
                              >
                                <Select
                                  style={{ width: 130 }}
                                  value={isPrefix ? "prefix" : "regex"}
                                  onChange={(nextMode: "prefix" | "regex") => {
                                    if (nextMode === "prefix") {
                                      // Clear any regex-override for this row. If the stored
                                      // pattern is not already a decodable prefix we reset the
                                      // body to empty rather than mechanically re-escaping an
                                      // arbitrary regex into a literal prefix (which would
                                      // silently produce a non-matching pattern).
                                      setShellPatternRegexOverride((prevSet) => {
                                        if (!prevSet.has(overrideKey)) return prevSet;
                                        const nextSet = new Set(prevSet);
                                        nextSet.delete(overrideKey);
                                        return nextSet;
                                      });
                                      setShellEditorPolicies((prev) =>
                                        prev.map((item, current) => {
                                          if (current !== index) return item;
                                          const next = [...item.allowed_shell_patterns];
                                          const body = tryDecodePrefix(pat) ?? "";
                                          next[patIdx] = buildPrefixPattern(body);
                                          return { ...item, allowed_shell_patterns: next };
                                        }),
                                      );
                                    } else {
                                      // Force Regex UI for this row, keeping the stored
                                      // pattern as-is so the user can hand-edit it.
                                      setShellPatternRegexOverride((prevSet) => {
                                        if (prevSet.has(overrideKey)) return prevSet;
                                        const nextSet = new Set(prevSet);
                                        nextSet.add(overrideKey);
                                        return nextSet;
                                      });
                                    }
                                  }}
                                  options={[
                                    { value: "prefix", label: t("remoteInvoke.shellEditor.patternPrefix") },
                                    { value: "regex", label: t("remoteInvoke.shellEditor.patternRegex") },
                                  ]}
                                />
                                <Input
                                  style={{ flex: 1 }}
                                  value={isPrefix ? (decoded as string) : pat}
                                  placeholder={isPrefix ? t("remoteInvoke.shellEditor.prefixPlaceholder") : t("remoteInvoke.shellEditor.regexPlaceholder")}
                                  onChange={(e) => {
                                    const raw = e.target.value;
                                    setShellEditorPolicies((prev) =>
                                      prev.map((item, current) => {
                                        if (current !== index) return item;
                                        const next = [...item.allowed_shell_patterns];
                                        next[patIdx] = isPrefix ? buildPrefixPattern(raw) : raw;
                                        return { ...item, allowed_shell_patterns: next };
                                      }),
                                    );
                                  }}
                                />
                                <Button
                                  danger
                                  icon={<DeleteOutlined />}
                                  onClick={() => {
                                    setShellPatternRegexOverride((prevSet) => {
                                      // Drop the deleted row and shift indices > patIdx
                                      // down by one within this policy.
                                      const nextSet = new Set<string>();
                                      prevSet.forEach((key) => {
                                        const sep = key.indexOf("|");
                                        if (sep < 0) return;
                                        const pid = key.slice(0, sep);
                                        const idxNum = Number(key.slice(sep + 1));
                                        if (pid !== policy.id) {
                                          nextSet.add(key);
                                          return;
                                        }
                                        if (idxNum === patIdx) return;
                                        const shifted = idxNum > patIdx ? idxNum - 1 : idxNum;
                                        nextSet.add(`${pid}|${shifted}`);
                                      });
                                      return nextSet;
                                    });
                                    setShellEditorPolicies((prev) =>
                                      prev.map((item, current) => {
                                        if (current !== index) return item;
                                        const next = item.allowed_shell_patterns.filter(
                                          (_, i) => i !== patIdx,
                                        );
                                        return { ...item, allowed_shell_patterns: next };
                                      }),
                                    );
                                  }}
                                />
                              </Space.Compact>
                            );
                          })}
                          <Space>
                            <Button
                              size="small"
                              icon={<PlusOutlined />}
                              onClick={() =>
                                setShellEditorPolicies((prev) =>
                                  prev.map((item, current) =>
                                    current === index
                                      ? {
                                          ...item,
                                          allowed_shell_patterns: [
                                            ...item.allowed_shell_patterns,
                                            buildPrefixPattern(""),
                                          ],
                                        }
                                      : item,
                                  ),
                                )
                              }
                            >
                              {t("remoteInvoke.shellEditor.addCommand")}
                            </Button>
                          </Space>
                        </Space>
                      </Col>
                      <Col xs={24} md={12}>
                        <Text type="secondary">{t("remoteInvoke.shellEditor.allowedCwd")}</Text>
                        <Select
                          mode="tags"
                          style={{ width: "100%" }}
                          value={policy.cwd_allowlist}
                          onChange={(value) =>
                            setShellEditorPolicies((prev) =>
                              prev.map((item, current) =>
                                current === index
                                  ? { ...item, cwd_allowlist: value }
                                  : item,
                              ),
                            )
                          }
                          tokenSeparators={[","]}
                          placeholder={t("remoteInvoke.shellEditor.extraCwdPlaceholder")}
                        />
                      </Col>
                      <Col xs={24} md={12}>
                        <Text type="secondary">{t("remoteInvoke.shellEditor.allowedEnv")}</Text>
                        <Select
                          mode="tags"
                          style={{ width: "100%" }}
                          value={policy.env_allowlist}
                          onChange={(value) =>
                            setShellEditorPolicies((prev) =>
                              prev.map((item, current) =>
                                current === index
                                  ? { ...item, env_allowlist: value }
                                  : item,
                              ),
                            )
                          }
                          tokenSeparators={[","]}
                          placeholder={t("remoteInvoke.shellEditor.extraEnvPlaceholder")}
                        />
                      </Col>
                      <Col xs={24} md={12}>
                        <Text type="secondary">{t("remoteInvoke.shellEditor.defaultCwd")}</Text>
                        <Input
                          value={policy.default_cwd}
                          onChange={(e) =>
                            setShellEditorPolicies((prev) =>
                              prev.map((item, current) =>
                                current === index
                                  ? { ...item, default_cwd: e.target.value }
                                  : item,
                              ),
                            )
                          }
                        />
                      </Col>
                      <Col xs={24} md={12}>
                        <Text type="secondary">{t("remoteInvoke.shellEditor.maxTimeout")}</Text>
                        <InputNumber
                          style={{ width: "100%" }}
                          min={1}
                          value={policy.max_timeout_ms ?? undefined}
                          onChange={(value) =>
                            setShellEditorPolicies((prev) =>
                              prev.map((item, current) =>
                                current === index
                                  ? {
                                      ...item,
                                      max_timeout_ms:
                                        typeof value === "number"
                                          ? value
                                          : null,
                                    }
                                  : item,
                              ),
                            )
                          }
                          placeholder={t("common.optional")}
                        />
                      </Col>
                      <Col xs={12} md={6}>
                        <Text type="secondary">{t("remoteInvoke.shell.allowStdin")}</Text>
                        <div>
                          <Switch
                            checked={policy.stdin_allowed}
                            onChange={(checked) =>
                              setShellEditorPolicies((prev) =>
                                prev.map((item, current) =>
                                  current === index
                                    ? { ...item, stdin_allowed: checked }
                                    : item,
                                ),
                              )
                            }
                          />
                        </div>
                      </Col>
                      <Col xs={12} md={6}>
                        <Text type="secondary">{t("remoteInvoke.shellEditor.allowInteractive")}</Text>
                        <div>
                          <Switch
                            checked={policy.interactive_allowed}
                            onChange={(checked) =>
                              setShellEditorPolicies((prev) =>
                                prev.map((item, current) =>
                                  current === index
                                    ? { ...item, interactive_allowed: checked }
                                    : item,
                                ),
                              )
                            }
                          />
                        </div>
                      </Col>
                    </Row>

                    <Divider style={{ margin: "12px 0" }} />
                    <div>
                      <Text strong>{t("remoteInvoke.shellEditor.testGroup")}</Text>
                      <Text type="secondary" style={{ marginLeft: 8, fontSize: 12 }}>
                        {t("remoteInvoke.shellEditor.testGroupHint")}
                      </Text>
                      <Row gutter={[8, 8]} style={{ marginTop: 8 }} align="middle">
                        <Col xs={24} md={6}>
                          <Select
                            style={{ width: "100%" }}
                            value={shellEditorTestMode[policy.id] ?? "shell_text"}
                            onChange={(value: "argv_exec" | "shell_text") =>
                              setShellEditorTestMode((prev) => ({ ...prev, [policy.id]: value }))
                            }
                            options={[
                              { value: "shell_text", label: t("remoteInvoke.execMode.shellText") },
                              { value: "argv_exec", label: t("remoteInvoke.shellEditor.argvExec") },
                            ]}
                          />
                        </Col>
                        <Col xs={24} md={12}>
                          <Input
                            placeholder={t("remoteInvoke.shellEditor.testPlaceholder")}
                            value={shellEditorTestInput[policy.id] ?? ""}
                            onChange={(e) =>
                              setShellEditorTestInput((prev) => ({
                                ...prev,
                                [policy.id]: e.target.value,
                              }))
                            }
                            onPressEnter={() => runShellPolicyTest(policy.id)}
                          />
                        </Col>
                        <Col xs={24} md={6}>
                          <Button
                            type="primary"
                            block
                            loading={shellEditorTestLoading[policy.id] ?? false}
                            onClick={() => runShellPolicyTest(policy.id)}
                          >
                            {t("remoteInvoke.shellEditor.test")}
                          </Button>
                        </Col>
                      </Row>
                      {shellEditorTestResult[policy.id] && (
                        <Alert
                          style={{ marginTop: 8 }}
                          showIcon
                          type={
                            shellEditorTestResult[policy.id]!.matched
                              ? shellEditorTestResult[policy.id]!.matched_policy_id === policy.id
                                ? "success"
                                : "warning"
                              : "error"
                          }
                          message={
                            shellEditorTestResult[policy.id]!.matched
                              ? shellEditorTestResult[policy.id]!.matched_policy_id === policy.id
                                ? t("remoteInvoke.shellEditor.testMatchedThis", { id: shellEditorTestResult[policy.id]!.matched_policy_id })
                                : t("remoteInvoke.shellEditor.testMatchedOther", { id: shellEditorTestResult[policy.id]!.matched_policy_id })
                              : t("remoteInvoke.shellEditor.testNotAllowed")
                          }
                          description={shellEditorTestResult[policy.id]!.reason}
                        />
                      )}
                      <Text type="secondary" style={{ fontSize: 11, display: "block", marginTop: 6 }}>
                        {t("remoteInvoke.shellEditor.testFootnote")}
                      </Text>
                    </div>
                  </Card>
                ))
              )}
            </Space>
          </div>
          </>
          ) : (
            <Card size="small" title={t("remoteInvoke.shellEditor.presetPreview")}>
              <Space direction="vertical" size={16} style={{ width: "100%" }}>
                <Descriptions size="small" column={1} bordered>
                  <Descriptions.Item label={t("remoteInvoke.shellEditor.mode")}>
                    {shellPresetDisplay(t, shellEditorMode).label}
                  </Descriptions.Item>
                  <Descriptions.Item label={t("remoteInvoke.shell.policies")}>
                    {shellEditorPolicies.map((policy) => (
                      <Tag key={policy.id} color="blue" style={{ marginBottom: 4 }}>
                        {policy.name} ({policy.id})
                      </Tag>
                    ))}
                  </Descriptions.Item>
                  <Descriptions.Item label={t("remoteInvoke.shell.profiles")}>
                    {shellEditorProfiles.length > 0 ? (
                      shellEditorProfiles.map((profile) => (
                        <Tag key={profile.id} color="green" style={{ marginBottom: 4 }}>
                          {profile.name} ({profile.id})
                        </Tag>
                      ))
                    ) : (
                      <Text type="secondary">{t("remoteInvoke.shellEditor.noExtraProfile")}</Text>
                    )}
                  </Descriptions.Item>
                </Descriptions>
                <Alert
                  showIcon
                  type="info"
                  message={t("remoteInvoke.shellEditor.presetStoreTitle")}
                  description={t("remoteInvoke.shellEditor.presetStoreDescription")}
                />
              </Space>
            </Card>
          )}
        </Space>
      </Modal>
      <Modal
        open={cliPreviewOpen}
        title={t("remoteInvoke.cliPreview.title")}
        onCancel={() => setCliPreviewOpen(false)}
        width={820}
        footer={[
          <Button
            key="copy"
            type="primary"
            onClick={async () => {
              const ok = await copyToClipboard(cliPreviewText);
              if (ok) {
                message.success(t("common.copied"));
              } else {
                message.error(t("common.copyFailed"));
              }
            }}
          >
            {t("common.copy")}
          </Button>,
          <Button key="close" onClick={() => setCliPreviewOpen(false)}>
            {t("common.close")}
          </Button>,
        ]}
      >
        <Alert
          showIcon
          type="info"
          style={{ marginBottom: 12 }}
          message={t("remoteInvoke.cliPreview.runOnTarget")}
          description={t("remoteInvoke.cliPreview.description")}
        />
        <pre
          style={{
            background: "var(--ant-color-fill-tertiary, #f5f5f5)",
            padding: 12,
            borderRadius: 6,
            maxHeight: 420,
            overflow: "auto",
            fontSize: 12,
            whiteSpace: "pre-wrap",
          }}
        >
          {cliPreviewText || t("remoteInvoke.cliPreview.empty")}
        </pre>
      </Modal>
      <Modal
        open={fileAccessEditorOpen}
        title={
          fileAccessEditingGrant
            ? t("remoteInvoke.fileAccess.editorTitle", {
                caller:
                  fileAccessEditingGrant.caller_display_name ||
                  formatFingerprint(fileAccessEditingGrant.caller_fingerprint),
              })
            : t("remoteInvoke.fileAccess.title")
        }
        okText={t("common.save")}
        cancelText={t("common.cancel")}
        onCancel={() => {
          setFileAccessEditorOpen(false);
          setFileAccessEditingGrant(null);
          setFileAccessDraft(null);
        }}
        onOk={() => void handleSaveFileAccessConfig()}
        confirmLoading={fileAccessSaveLoading}
        width={820}
        destroyOnClose
      >
        <Alert
          showIcon
          type="info"
          style={{ marginBottom: 16 }}
          message={t("remoteInvoke.fileAccess.boundTitle")}
          description={t("remoteInvoke.fileAccess.boundDescription")}
        />
        {fileAccessEditingGrant && fileAccessDraft ? (
          <Space direction="vertical" size={16} style={{ width: "100%" }}>
            <Row gutter={12}>
              <Col xs={24} md={12}>
                <Text type="secondary" style={{ fontSize: 12 }}>{t("remoteInvoke.fileAccess.grantId")}</Text>
                <Input value={fileAccessEditingGrant.grant_id} disabled />
              </Col>
              <Col xs={24} md={12}>
                <Text type="secondary" style={{ fontSize: 12 }}>{t("common.name")}</Text>
                <Input
                  value={fileAccessDraft.name ?? ""}
                  placeholder={t("remoteInvoke.fileAccess.namePlaceholder")}
                  onChange={(e) => {
                    const name = e.target.value;
                    setFileAccessDraft((prev) => prev ? { ...prev, name } : prev);
                  }}
                />
              </Col>
            </Row>

            <Row gutter={12}>
              <Col xs={24} md={12}>
                <Text type="secondary" style={{ fontSize: 12 }}>{t("common.type")}</Text>
                <Radio.Group
                  style={{ width: "100%" }}
                  value={getFileAccessPolicyAccess(fileAccessDraft)}
                  onChange={(e) => {
                    const access = e.target.value as FileAccessPolicyAccess;
                    const ops = access === "read_write"
                      ? [...ALL_FILE_OPS]
                      : [...FILE_READ_OPS];
                    setFileAccessDraft((prev) => prev ? { ...prev, ops } : prev);
                  }}
                  optionType="button"
                  buttonStyle="solid"
                  options={[
                    { label: t("remoteInvoke.fileAccess.readOnly"), value: "read" },
                    { label: t("remoteInvoke.fileAccess.readWrite"), value: "read_write" },
                  ]}
                />
              </Col>
              <Col xs={24} md={12}>
                <Text type="secondary" style={{ fontSize: 12 }}>{t("remoteInvoke.fileAccess.directories")}</Text>
                <Radio.Group
                  style={{ width: "100%" }}
                  value={getFileAccessPolicyRootScope(fileAccessDraft)}
                  onChange={(e) => {
                    const rootScope = e.target.value as FileAccessPolicyRootScope;
                    setFileAccessDraft((prev) =>
                      prev
                        ? {
                            ...prev,
                            roots: rootScope === "all" ? ["/"] : [],
                          }
                        : prev,
                    );
                  }}
                  optionType="button"
                  buttonStyle="solid"
                  options={[
                    { label: t("remoteInvoke.fileAccess.selected"), value: "selected" },
                    { label: t("common.all"), value: "all" },
                  ]}
                />
              </Col>
            </Row>

            {getFileAccessPolicyRootScope(fileAccessDraft) === "selected" && (
              <div>
                <Text type="secondary" style={{ fontSize: 12 }}>{t("remoteInvoke.fileAccess.allowedRoots")}</Text>
                <TextArea
                  data-testid="file-access-roots-input"
                  autoSize={{ minRows: 3, maxRows: 6 }}
                  value={(fileAccessDraft.roots ?? []).join("\n")}
                  placeholder="/Users/<yourname>/work/project"
                  onChange={(e) => {
                    const roots = e.target.value.split("\n");
                    setFileAccessDraft((prev) => prev ? { ...prev, roots } : prev);
                  }}
                />
              </div>
            )}

            <div>
              <Text type="secondary" style={{ fontSize: 12 }}>{t("remoteInvoke.fileAccess.denyPatterns")}</Text>
              <TextArea
                autoSize={{ minRows: 2, maxRows: 4 }}
                value={(fileAccessDraft.denies ?? []).join("\n")}
                placeholder={"**/.git/**\n**/target/**"}
                onChange={(e) => {
                  const denies = e.target.value.split("\n");
                  setFileAccessDraft((prev) => prev ? { ...prev, denies } : prev);
                }}
              />
            </div>

            <div>
              <Text type="secondary" style={{ fontSize: 12 }}>{t("remoteInvoke.fileAccess.writeDenyPatterns")}</Text>
              <TextArea
                autoSize={{ minRows: 1, maxRows: 4 }}
                value={(fileAccessDraft.write_denies ?? []).join("\n")}
                placeholder={"**/Cargo.lock\n**/*.lock"}
                onChange={(e) => {
                  const write_denies = e.target.value.split("\n");
                  setFileAccessDraft((prev) => prev ? { ...prev, write_denies } : prev);
                }}
              />
            </div>

            <Row gutter={12}>
              <Col xs={24} md={8}>
                <Text type="secondary" style={{ fontSize: 12 }}>{t("remoteInvoke.fileAccess.maxReadBytes")}</Text>
                <InputNumber
                  style={{ width: "100%" }}
                  value={fileAccessDraft.max_read_bytes}
                  placeholder="2097152"
                  min={0}
                  onChange={(val) => {
                    setFileAccessDraft((prev) =>
                      prev ? { ...prev, max_read_bytes: val ?? undefined } : prev,
                    );
                  }}
                />
              </Col>
              <Col xs={24} md={8}>
                <Text type="secondary" style={{ fontSize: 12 }}>{t("remoteInvoke.fileAccess.maxWriteBytes")}</Text>
                <InputNumber
                  style={{ width: "100%" }}
                  value={fileAccessDraft.max_write_bytes}
                  placeholder="2097152"
                  min={0}
                  onChange={(val) => {
                    setFileAccessDraft((prev) =>
                      prev ? { ...prev, max_write_bytes: val ?? undefined } : prev,
                    );
                  }}
                />
              </Col>
              <Col xs={24} md={8}>
                <Space direction="vertical" size={4}>
                  <Space>
                    <Switch
                      size="small"
                      checked={fileAccessDraft.respect_gitignore ?? true}
                      onChange={(val) => {
                        setFileAccessDraft((prev) =>
                          prev ? { ...prev, respect_gitignore: val } : prev,
                        );
                      }}
                    />
                    <Text type="secondary" style={{ fontSize: 12 }}>{t("remoteInvoke.fileAccess.respectGitignore")}</Text>
                  </Space>
                  <Space>
                    <Switch
                      size="small"
                      checked={fileAccessDraft.allow_overwrite ?? true}
                      onChange={(val) => {
                        setFileAccessDraft((prev) =>
                          prev ? { ...prev, allow_overwrite: val } : prev,
                        );
                      }}
                    />
                    <Text type="secondary" style={{ fontSize: 12 }}>{t("remoteInvoke.fileAccess.allowOverwrite")}</Text>
                  </Space>
                  <Space>
                    <Switch
                      size="small"
                      checked={fileAccessDraft.allow_recursive_delete ?? false}
                      onChange={(val) => {
                        setFileAccessDraft((prev) =>
                          prev ? { ...prev, allow_recursive_delete: val } : prev,
                        );
                      }}
                    />
                    <Text type="secondary" style={{ fontSize: 12 }}>{t("remoteInvoke.fileAccess.allowRecursiveDelete")}</Text>
                  </Space>
                </Space>
              </Col>
            </Row>
          </Space>
        ) : (
          <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description={t("remoteInvoke.fileAccess.noGrantSelected")} />
        )}
      </Modal>
      <Modal
        open={sshFilePolicyOpen}
        title={t("remoteInvoke.sshFilePolicy.title")}
        okText={t("common.save")}
        cancelText={t("common.cancel")}
        onCancel={() => setSshFilePolicyOpen(false)}
        onOk={() => void handleSaveSshFilePolicy()}
        confirmLoading={sshFilePolicySaving}
        destroyOnClose
      >
        <Alert
          showIcon
          type="info"
          style={{ marginBottom: 16 }}
          message={t("remoteInvoke.sshFilePolicy.infoTitle")}
          description={t("remoteInvoke.sshFilePolicy.infoDescription")}
        />
        <Space direction="vertical" size={16} style={{ width: "100%" }}>
          <div>
            <Text type="secondary" style={{ fontSize: 12 }}>{t("remoteInvoke.sshFilePolicy.access")}</Text>
            <Radio.Group
              style={{ width: "100%" }}
              value={sshFilePolicyAccess}
              onChange={(e) => setSshFilePolicyAccess(e.target.value)}
              optionType="button"
              buttonStyle="solid"
              options={[
                { label: t("remoteInvoke.fileAccess.readOnly"), value: "read" },
                { label: t("remoteInvoke.fileAccess.readWrite"), value: "read_write" },
              ]}
            />
          </div>
          <div>
            <Text type="secondary" style={{ fontSize: 12 }}>{t("remoteInvoke.fileAccess.directories")}</Text>
            <Radio.Group
              style={{ width: "100%" }}
              value={sshFilePolicyRootScope}
              onChange={(e) => setSshFilePolicyRootScope(e.target.value)}
              optionType="button"
              buttonStyle="solid"
              options={[
                { label: t("remoteInvoke.fileAccess.selected"), value: "selected" },
                { label: t("common.all"), value: "all" },
              ]}
            />
          </div>
          {sshFilePolicyRootScope === "selected" && (
            <div>
              <Text type="secondary" style={{ fontSize: 12 }}>{t("remoteInvoke.fileAccess.allowedRoots")}</Text>
              <TextArea
                autoSize={{ minRows: 3, maxRows: 6 }}
                value={sshFilePolicyRoots}
                placeholder="~/work/code/my-project"
                onChange={(e) => setSshFilePolicyRoots(e.target.value)}
              />
            </div>
          )}
          <Space direction="vertical" size={8}>
            <Space>
              <Switch
                size="small"
                checked={sshFilePolicyAllowOverwrite}
                onChange={setSshFilePolicyAllowOverwrite}
              />
              <Text type="secondary">{t("remoteInvoke.fileAccess.allowOverwriteLong")}</Text>
            </Space>
            <Space>
              <Switch
                size="small"
                checked={sshFilePolicyAllowRecursiveDelete}
                onChange={setSshFilePolicyAllowRecursiveDelete}
              />
              <Text type="secondary">{t("remoteInvoke.fileAccess.allowRecursiveDeleteLong")}</Text>
            </Space>
          </Space>
        </Space>
      </Modal>
      <Modal
        open={editorOpen}
        title={t("remoteInvoke.sshCreate.title")}
        okText={t("common.create")}
        cancelText={t("common.cancel")}
        onCancel={() => setEditorOpen(false)}
        onOk={() => void handleSubmitSshForm()}
        confirmLoading={sshAction === "create"}
        destroyOnClose
      >
        <Alert
          showIcon
          type="info"
          style={{ marginBottom: 16 }}
          message={t("remoteInvoke.sshCreate.infoTitle")}
          description={t("remoteInvoke.sshCreate.infoDescription")}
        />
        <Form
          form={sshForm}
          layout="vertical"
          initialValues={{
            label: "",
            grant_mode: "permanent" satisfies GrantMode,
            seed_roots: "",
            seed_ops: [...REMOTE_INVOKE_FILE_OPS],
            seed_allow_overwrite: true,
            seed_allow_recursive_delete: false,
          }}
        >
          <Form.Item
            label={t("remoteInvoke.ssh.label")}
            name="label"
            rules={[
              { required: true, message: t("remoteInvoke.sshCreate.labelRequired") },
              { max: 80, message: t("remoteInvoke.sshCreate.labelMax") },
            ]}
          >
            <Input placeholder="CI Agent" maxLength={80} />
          </Form.Item>
          <Form.Item name="grant_mode" hidden>
            <Input />
          </Form.Item>
          <Divider plain style={{ marginTop: 8 }}>
            {t("remoteInvoke.sshCreate.fileScope")}
          </Divider>
          <Alert
            showIcon
            type="info"
            style={{ marginBottom: 12 }}
            message={t("remoteInvoke.sshCreate.noConfirmTitle")}
            description={t("remoteInvoke.sshCreate.noConfirmDescription")}
          />
          <Form.Item
            label={t("remoteInvoke.sshCreate.rootDirs")}
            name="seed_roots"
            tooltip={t("remoteInvoke.sshCreate.rootDirsTooltip")}
          >
            <Input placeholder="~/work, /opt/projects" />
          </Form.Item>
          <Form.Item
            label={t("remoteInvoke.sshCreate.allowedOps")}
            name="seed_ops"
            tooltip={t("remoteInvoke.sshCreate.allowedOpsTooltip")}
          >
            <Checkbox.Group
              options={REMOTE_INVOKE_FILE_OPS.map((op) => ({
                label: op,
                value: op,
              }))}
            />
          </Form.Item>
          <Form.Item
            label={t("remoteInvoke.fileAccess.allowOverwriteLong")}
            name="seed_allow_overwrite"
            valuePropName="checked"
          >
            <Switch />
          </Form.Item>
          <Form.Item
            label={t("remoteInvoke.fileAccess.allowRecursiveDeleteLong")}
            name="seed_allow_recursive_delete"
            valuePropName="checked"
            tooltip={t("remoteInvoke.sshCreate.recursiveDeleteTooltip")}
          >
            <Switch />
          </Form.Item>
        </Form>
      </Modal>
      <Modal
        open={secretModal !== null}
        title={secretModal?.title}
        okText={t("common.close")}
        cancelButtonProps={{ style: { display: "none" } }}
        onOk={() => setSecretModal(null)}
        onCancel={() => setSecretModal(null)}
        width={760}
      >
        <Space direction="vertical" size={12} style={{ width: "100%" }}>
          <Alert showIcon type="warning" message={secretModal?.description} />
          <Descriptions
            size="small"
            column={1}
            items={[
              {
                key: "device_code",
                label: t("remoteInvoke.ssh.deviceCode"),
                children: <Text code>{secretModal?.payload.device_code ?? "-"}</Text>,
              },
              {
                key: "fingerprint",
                label: t("remoteInvoke.ssh.fingerprint"),
                children: (
                  <Text code style={{ fontSize: 11 }}>
                    {formatSshFingerprint(secretModal?.payload.ssh_key_fingerprint)}
                  </Text>
                ),
              },
            ]}
          />
          <div>
            <Space style={{ marginBottom: 8 }}>
              <Text strong>{t("remoteInvoke.ssh.keyFileTitle")}</Text>
              <Button
                size="small"
                icon={<CopyOutlined />}
                onClick={async () => {
                  if (!secretModal?.payload.bifrost_key_file) return;
                  const ok = await copyToClipboard(secretModal.payload.bifrost_key_file);
                  if (ok) message.success(t("remoteInvoke.ssh.keyFileCopied"));
                  else message.error(t("common.copyFailed"));
                }}
              >
                {t("common.copy")}
              </Button>
            </Space>
            <TextArea
              readOnly
              autoSize={{ minRows: 8, maxRows: 14 }}
              value={secretModal?.payload.bifrost_key_file ?? ""}
            />
          </div>
          <div>
            <Text strong>{t("remoteInvoke.ssh.cliExample")}</Text>
            <div style={{ marginTop: 8 }}>
              <Text code>bifrost remote connect --ssh-key /path/to/bifrost.key</Text>
            </div>
          </div>
        </Space>
      </Modal>

    </div>
  );
}
